//! Dispatch (AD-11 to AD-13): the queue of runs that wait for a slot, the slot accounting, batches
//! and epics, kept in `<app data>/dispatch.json` (schema 1, atomic, fields this version does not
//! know are kept). Starts go only through `start_run_inner`: a new run folder and a new tab, never
//! a write into an existing tab. This file's own mutex is held only while the file is read and
//! written, never across a spawn or a store lock.

use super::{start_run_inner, StartError, StartParams};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

pub const FILE: &str = "dispatch.json";
/// Stages in which a run waits for the user or is over: it holds no slot.
const FREE_STAGES: &[&str] = &["ship-approval", "done", "failed"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub params: StartParams,
    /// The account the start runs under (the entry's window must belong to it).
    pub account_id: String,
    /// A short title for the Runs view.
    pub label: String,
    /// `queued` or `starting`.
    pub state: String,
    #[serde(default)]
    pub reason: Option<String>,
    pub queued_at: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub batch_id: Option<String>,
    /// Starts through the cap check regardless (a user's "Start now").
    #[serde(default)]
    pub bypass_cap: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BatchRun {
    pub entry_id: String,
    pub ticket_ids: Vec<String>,
    #[serde(default)]
    pub label: String,
    /// Set when the entry started.
    #[serde(default)]
    pub run_dir: Option<String>,
    /// Taken out of the batch by the user: shown struck through, no longer counted.
    #[serde(default)]
    pub removed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Batch {
    pub id: String,
    pub created_at: String,
    pub runs: Vec<BatchRun>,
    /// Milestones already announced (`batch-planned`, `batch-ready`): each fires once.
    #[serde(default)]
    pub notified: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Epic {
    pub run_dir: String,
    #[serde(default)]
    pub notified: Vec<String>,
    /// Waves already queued or started.
    #[serde(default)]
    pub started: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Dispatch {
    pub schema: u32,
    #[serde(default)]
    pub queue: Vec<Entry>,
    #[serde(default)]
    pub batches: Vec<Batch>,
    #[serde(default)]
    pub epics: Vec<Epic>,
    /// Run folders the user released from holding a slot.
    #[serde(default)]
    pub released: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Dispatch {
    fn default() -> Self {
        Self {
            schema: 1,
            queue: vec![],
            batches: vec![],
            epics: vec![],
            released: vec![],
            extra: Map::new(),
        }
    }
}

pub fn path_in(data: &Path) -> PathBuf {
    data.join(FILE)
}

/// The file's state. A file that does not parse is kept as `dispatch.json.bad` and the state starts empty.
pub fn load(data: &Path) -> Dispatch {
    let path = path_in(data);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Dispatch::default();
    };
    match serde_json::from_str::<Dispatch>(&text) {
        Ok(d) => d,
        Err(_) => {
            let _ = std::fs::copy(&path, data.join("dispatch.json.bad"));
            Dispatch::default()
        }
    }
}

pub fn save(data: &Path, state: &Dispatch) -> Result<(), String> {
    crate::store::write_json_atomic(&path_in(data), state)
}

static FILE_LOCK: Mutex<()> = Mutex::new(());

/// One read-modify-write of the file at a time.
pub fn with<R>(data: &Path, f: impl FnOnce(&mut Dispatch) -> R) -> Result<R, String> {
    let _guard = FILE_LOCK.lock().map_err(|e| e.to_string())?;
    let mut state = load(data);
    let out = f(&mut state);
    save(data, &state)?;
    Ok(out)
}

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub fn new_id(prefix: &str) -> String {
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}-{ts}-{}", &hex[..4])
}

/// After a restart a `starting` entry either did start (a run folder's meta.json carries its id: the
/// entry is dropped) or did not (back to `queued`, with the reason).
pub fn recover(state: &mut Dispatch, started: &HashSet<String>) {
    state
        .queue
        .retain(|e| !(e.state == "starting" && started.contains(&e.id)));
    for e in state.queue.iter_mut().filter(|e| e.state == "starting") {
        e.state = "queued".into();
        e.started_at = None;
        e.reason = Some("app quit during start".into());
    }
}

/// What the slot accounting knows of one run folder.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSlotFacts {
    pub run_dir: String,
    pub run_id: String,
    pub stage: Option<String>,
    pub paused: bool,
    /// A tab of this app has the run folder open now.
    pub tab_open: bool,
    /// Milliseconds since status.json last changed (None: no status.json).
    pub changed_ago_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Holder {
    pub run_dir: String,
    pub run_id: String,
    pub stage: Option<String>,
    /// `tab open` or `recent change`.
    pub why: String,
}

/// The runs that hold a slot: stage not ship-approval/done/failed, not paused, not released, and live (a
/// tab is open on it, or its status.json changed within `watchdog_minutes`). A run with no tab and no
/// recent change holds none, whatever its stage.
pub fn holders(runs: &[RunSlotFacts], released: &[String], watchdog_minutes: u32) -> Vec<Holder> {
    let window_ms = u64::from(watchdog_minutes.max(1)) * 60_000;
    runs.iter()
        .filter(|r| !r.paused && !released.contains(&r.run_dir))
        .filter(|r| !r.stage.as_deref().is_some_and(|s| FREE_STAGES.contains(&s)))
        .filter_map(|r| {
            let why = if r.tab_open {
                "tab open"
            } else if r.changed_ago_ms.is_some_and(|ago| ago <= window_ms) {
                "recent change"
            } else {
                return None;
            };
            Some(Holder {
                run_dir: r.run_dir.clone(),
                run_id: r.run_id.clone(),
                stage: r.stage.clone(),
                why: why.into(),
            })
        })
        .collect()
}

/// A hold from the five-hour reading: `Some(reason)` when `percent` reached `hold` and the window has not reset.
/// No setting, no reading, or a reading past its `resets_at` is not a hold.
pub fn usage_hold(
    hold: Option<u32>,
    reading: Option<(f64, Option<i64>)>,
    now_s: i64,
) -> Option<String> {
    let hold = hold?;
    let (percent, resets_at) = reading?;
    if resets_at.is_some_and(|r| r <= now_s) || percent < f64::from(hold) {
        return None;
    }
    Some(format!(
        "the five-hour usage is {percent:.0}%, at or past your hold of {hold}%"
    ))
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// Start this entry now.
    Start(String),
    /// Nothing starts; the head entry waits for this reason.
    Wait(String),
    Nothing,
}

/// The next start: only the head of the queue, never while a start is in flight, and never past the cap
/// (held slots plus entries that are `starting`). `hold` gives an entry's usage hold reason.
pub fn pick(
    state: &Dispatch,
    held: usize,
    cap: u32,
    in_flight: bool,
    hold: impl Fn(&Entry) -> Option<String>,
) -> Pick {
    let Some(head) = state.queue.iter().find(|e| e.state == "queued") else {
        return Pick::Nothing;
    };
    if in_flight || state.queue.iter().any(|e| e.state == "starting") {
        return Pick::Nothing;
    }
    if !head.bypass_cap && held >= cap as usize {
        return Pick::Wait(format!("{held} of {cap} slots are in use"));
    }
    if !head.bypass_cap {
        if let Some(why) = hold(head) {
            return Pick::Wait(why);
        }
    }
    Pick::Start(head.id.clone())
}

pub fn enqueue(
    state: &mut Dispatch,
    params: StartParams,
    account_id: &str,
    label: &str,
    batch_id: Option<String>,
) -> String {
    let id = new_id("q");
    state.queue.push(Entry {
        id: id.clone(),
        params,
        account_id: account_id.to_string(),
        label: label.to_string(),
        state: "queued".into(),
        reason: None,
        queued_at: now_iso(),
        started_at: None,
        batch_id,
        bypass_cap: false,
    });
    id
}

/// Moves a queued entry one place up or down (a `starting` entry does not move).
pub fn move_entry(state: &mut Dispatch, id: &str, up: bool) -> bool {
    let Some(at) = state.queue.iter().position(|e| e.id == id) else {
        return false;
    };
    let to = if up {
        at.checked_sub(1)
    } else {
        Some(at + 1).filter(|n| *n < state.queue.len())
    };
    match to {
        Some(to) if state.queue[at].state == "queued" && state.queue[to].state == "queued" => {
            state.queue.swap(at, to);
            true
        }
        _ => false,
    }
}

// ---- the app side: facts, the tick, the commands -------------------------------------------

static IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static RECOVERED: AtomicBool = AtomicBool::new(false);

fn data_dir(app: &AppHandle) -> PathBuf {
    crate::data_dir::app_data_dir(app).path
}

fn canon(path: &str) -> String {
    Path::new(path)
        .canonicalize()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.to_string())
}

/// Facts about every run folder of this app: its stage, whether a tab has it open, and when it last changed.
pub fn run_facts(data: &Path, tab_run_dirs: &HashSet<String>) -> Vec<RunSlotFacts> {
    let open: HashSet<String> = tab_run_dirs.iter().map(|d| canon(d)).collect();
    super::folder::list_runs(data)
        .into_iter()
        .filter(|r| !r.adopted)
        .map(|r| {
            let status = Path::new(&r.run_dir).join("status.json");
            let changed = std::fs::metadata(&status)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map(|d| d.as_millis() as u64);
            let paused = std::fs::read_to_string(&status)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .is_some_and(|v| !v["paused"].is_null());
            RunSlotFacts {
                tab_open: open.contains(&canon(&r.run_dir)),
                run_dir: r.run_dir,
                run_id: r.run_id,
                stage: r.stage,
                paused,
                changed_ago_ms: changed,
            }
        })
        .collect()
}

/// The run folders whose meta.json records one of these entry ids.
fn started_entry_ids(data: &Path) -> HashSet<String> {
    super::folder::list_runs(data)
        .into_iter()
        .filter_map(|r| {
            std::fs::read_to_string(Path::new(&r.run_dir).join("meta.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| v["dispatchEntryId"].as_str().map(String::from))
        })
        .collect()
}

fn usage_reading(app: &AppHandle, account_id: &str) -> Option<(f64, Option<i64>)> {
    let config = {
        let settings = app.try_state::<Mutex<crate::store::SettingsStore>>()?;
        let settings = settings.lock().ok()?;
        crate::store::settings_store::resolved_account_config(settings.claude(), account_id).path
    };
    let usage = app.try_state::<Mutex<crate::usage::UsageStore>>()?;
    let snapshot = usage.lock().ok()?.snapshot(&config);
    let w = snapshot
        .windows
        .iter()
        .find(|w| w.rate_limit_type == "five_hour")?;
    Some((w.utilization?, w.resets_at))
}

/// The run settings the dispatcher reads: (cap, usage hold percent, watchdog minutes).
fn limits(app: &AppHandle) -> (u32, Option<u32>, u32) {
    app.try_state::<Mutex<crate::store::SettingsStore>>()
        .and_then(|s| {
            s.lock().ok().map(|s| {
                (
                    s.run().max_active_runs,
                    s.run().usage_hold_percent,
                    s.run().watchdog_minutes,
                )
            })
        })
        .unwrap_or((2, None, 10))
}

/// Called from the attention detector's tick with the run folders that have an open tab. At most one start
/// per tick, on its own thread, with no lock held.
pub fn on_tick(app: &AppHandle, tab_run_dirs: &HashSet<String>) {
    if IN_FLIGHT.load(Ordering::SeqCst) {
        return;
    }
    let data = data_dir(app);
    if !RECOVERED.swap(true, Ordering::SeqCst) {
        let started = started_entry_ids(&data);
        let _ = with(&data, |d| recover(d, &started));
    }
    let state = load(&data);
    if state.queue.is_empty() {
        return;
    }
    let (cap, hold, watchdog) = limits(app);
    let facts = run_facts(&data, tab_run_dirs);
    let held = holders(&facts, &state.released, watchdog).len();
    let now_s = chrono::Utc::now().timestamp();
    let decision = pick(&state, held, cap, false, |e| {
        usage_hold(hold, usage_reading(app, &e.account_id), now_s)
    });
    match decision {
        Pick::Nothing => {}
        Pick::Wait(why) => {
            if let Some(head) = state.queue.iter().find(|e| e.state == "queued") {
                if head.reason.as_deref() != Some(why.as_str()) {
                    let id = head.id.clone();
                    let _ = with(&data, |d| {
                        if let Some(e) = d.queue.iter_mut().find(|e| e.id == id) {
                            e.reason = Some(why.clone());
                        }
                    });
                }
            }
        }
        Pick::Start(id) => spawn_start(app, &id),
    }
}

/// Marks the entry `starting`, then starts it on its own thread (one at a time).
fn spawn_start(app: &AppHandle, id: &str) {
    if IN_FLIGHT
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let data = data_dir(app);
    let marked = with(&data, |d| {
        d.queue.iter_mut().find(|e| e.id == id).map(|e| {
            e.state = "starting".into();
            e.started_at = Some(now_iso());
            e.reason = None;
        })
    });
    if !matches!(marked, Ok(Some(()))) {
        IN_FLIGHT.store(false, Ordering::SeqCst);
        return;
    }
    let app = app.clone();
    let id = id.to_string();
    let spawned = std::thread::Builder::new()
        .name("dcv2-dispatch-start".into())
        .spawn({
            let (app, id) = (app.clone(), id.clone());
            move || {
                run_entry(&app, &id, &|app, params| start_run_inner(app, params));
                IN_FLIGHT.store(false, Ordering::SeqCst);
            }
        });
    if spawned.is_err() {
        let _ = requeue(&data_dir(&app), &id, "could not start a thread");
        IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}

fn requeue(data: &Path, id: &str, why: &str) -> Result<(), String> {
    with(data, |d| {
        if let Some(e) = d.queue.iter_mut().find(|e| e.id == id) {
            e.state = "queued".into();
            e.started_at = None;
            e.bypass_cap = false;
            e.reason = Some(why.to_string());
        }
    })
}

fn error_text(err: &StartError) -> String {
    match err {
        StartError::Message(m) => m.clone(),
        StartError::Duplicate { .. } => {
            "an unfinished run already has the same request, folder and tickets".into()
        }
    }
}

/// Runs one `starting` entry through `launcher` (the shared start path in the app, a fake in tests): a window
/// of the entry's account, then the start. Success removes the entry and records the run in its batch;
/// a failure puts it back as `queued` with the reason.
pub fn run_entry(
    app: &AppHandle,
    id: &str,
    launcher: &dyn Fn(&AppHandle, StartParams) -> Result<super::StartedRun, StartError>,
) {
    let data = data_dir(app);
    let Some(entry) = load(&data).queue.into_iter().find(|e| e.id == id) else {
        return;
    };
    let mut params = entry.params.clone();
    params.dispatch_entry_id = Some(entry.id.clone());
    // The entry's window when it is open, else the most recently focused one; it must be of the entry's account.
    if params
        .window_id
        .as_deref()
        .is_some_and(|w| app.get_webview_window(w).is_none())
    {
        params.window_id = None;
    }
    let account_ok = app
        .try_state::<Mutex<crate::store::StateStore>>()
        .and_then(|s| {
            s.lock().ok().map(|mut s| {
                s.bind_window(params.window_id.as_deref());
                let w = s.focus_window().to_string();
                s.account_for_window(&w) == entry.account_id
            })
        })
        .unwrap_or(false);
    if !account_ok {
        let _ = requeue(
            &data,
            id,
            "no open window belongs to the account this entry was made for",
        );
        return;
    }
    let outcome = launcher(app, params)
        .map(|started| started.run_dir)
        .map_err(|e| error_text(&e));
    let _ = with(&data, |d| finish(d, id, outcome));
}

/// The result of a start: success removes the entry and records the run in its batch; a failure (a ClickUp
/// fetch, a duplicate, no window) puts the entry back as `queued` with the reason.
pub fn finish(state: &mut Dispatch, id: &str, outcome: Result<Option<String>, String>) {
    match outcome {
        Ok(run_dir) => {
            let batch = state
                .queue
                .iter()
                .find(|e| e.id == id)
                .and_then(|e| e.batch_id.clone());
            state.queue.retain(|e| e.id != id);
            if let Some(run) = batch
                .and_then(|b| state.batches.iter_mut().find(|x| x.id == b))
                .and_then(|b| b.runs.iter_mut().find(|r| r.entry_id == id))
            {
                run.run_dir = run_dir;
            }
        }
        Err(why) => {
            if let Some(e) = state.queue.iter_mut().find(|e| e.id == id) {
                e.state = "queued".into();
                e.started_at = None;
                e.bypass_cap = false;
                e.reason = Some(why);
            }
        }
    }
}

// ---- batches: the overview and the milestones --------------------------------------------

const PLANNED_FROM: &[&str] = &[
    "plan-approval",
    "building",
    "ship-approval",
    "shipping",
    "done",
];
const READY_FROM: &[&str] = &["ship-approval", "shipping", "done"];

/// The milestone a batch has just reached, and has not announced yet: `batch-planned` when every counted run
/// has its plan ready for approval (or past it), `batch-ready` when every one is ready to ship. A run still in
/// the queue (no run folder yet) keeps both back; a removed run is not counted. `stages`: one per counted run.
pub fn batch_milestone(stages: &[Option<&str>], notified: &[String]) -> Option<&'static str> {
    if stages.is_empty() || stages.iter().any(|s| s.is_none()) {
        return None;
    }
    let all = |set: &[&str]| stages.iter().all(|s| s.is_some_and(|s| set.contains(&s)));
    for (name, set) in [("batch-ready", READY_FROM), ("batch-planned", PLANNED_FROM)] {
        if all(set) && !notified.iter().any(|n| n == name) {
            return Some(name);
        }
    }
    None
}

fn stage_of(run_dir: &str) -> Option<String> {
    std::fs::read_to_string(Path::new(run_dir).join("status.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["stage"].as_str().map(String::from))
        .or_else(|| Some("planning".to_string()))
}

/// The milestone events of every batch, each once (remembered in dispatch.json). They go through the notifier like
/// any attention event, so the desktop is always told and the phone only when the push target and `batchPushes`
/// allow it. Nothing here touches approval-wait notifications.
pub fn milestone_events(app: &AppHandle) -> Vec<crate::observe::attention::Event> {
    use crate::observe::attention::{Event, Kind};
    let data = data_dir(app);
    if !path_in(&data).exists() {
        return vec![];
    }
    let fired = with(&data, |d| {
        let mut fired = Vec::new();
        for batch in d.batches.iter_mut() {
            let stages: Vec<Option<String>> = batch
                .runs
                .iter()
                .filter(|r| !r.removed)
                .map(|r| r.run_dir.as_deref().and_then(stage_of))
                .collect();
            let refs: Vec<Option<&str>> = stages.iter().map(|s| s.as_deref()).collect();
            if let Some(name) = batch_milestone(&refs, &batch.notified) {
                batch.notified.push(name.to_string());
                // Reaching ready also covers planned: the earlier announcement is not made late.
                if name == "batch-ready" && !batch.notified.iter().any(|n| n == "batch-planned") {
                    batch.notified.push("batch-planned".into());
                }
                fired.push((batch.id.clone(), name));
            }
        }
        fired
    })
    .unwrap_or_default();
    fired
        .into_iter()
        .map(|(id, name)| Event {
            kind: if name == "batch-ready" {
                Kind::BatchReady
            } else {
                Kind::BatchPlanned
            },
            subject: id.clone(),
            tab_id: None,
            window_label: None,
            tab_label: None,
            run_id: Some(id),
            run_dir: None,
            detail: None,
        })
        .collect()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchRowView {
    pub entry_id: String,
    pub label: String,
    pub ticket_ids: Vec<String>,
    pub removed: bool,
    pub run_dir: Option<String>,
    /// `queued`, `starting` or the run's stage.
    pub stage: String,
    /// `plan-approval` or `ship-approval` while the run waits for you.
    pub waiting_for: Option<String>,
    pub pr_url: Option<String>,
    /// From app-github.json: `pass`, `fail`, `pending`, `none`, `unknown`.
    pub ci: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchView {
    pub id: String,
    pub created_at: String,
    pub rows: Vec<BatchRowView>,
    /// Counted rows that wait for the user now.
    pub waiting_for_you: usize,
}

fn row_view(d: &Dispatch, run: &BatchRun) -> BatchRowView {
    let (mut stage, mut waiting, mut pr_url, mut ci) = ("queued".to_string(), None, None, None);
    if let Some(e) = d.queue.iter().find(|e| e.id == run.entry_id) {
        stage = e.state.clone();
    } else if let Some(dir) = &run.run_dir {
        let status: Option<Value> = std::fs::read_to_string(Path::new(dir).join("status.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok());
        stage = status
            .as_ref()
            .and_then(|v| v["stage"].as_str())
            .unwrap_or("planning")
            .to_string();
        waiting = status
            .as_ref()
            .and_then(|v| v["waitingFor"].as_str())
            .map(String::from);
        let gh = super::github::read_github(Path::new(dir));
        let first = status
            .as_ref()
            .and_then(|v| v["prs"].as_array())
            .and_then(|a| a.first().cloned());
        if let Some(pr) = first {
            pr_url = pr["prUrl"].as_str().map(String::from);
            ci = pr["id"]
                .as_str()
                .and_then(|id| gh.prs.get(id))
                .map(|g| g.ci.clone());
        }
    }
    BatchRowView {
        entry_id: run.entry_id.clone(),
        label: run.label.clone(),
        ticket_ids: run.ticket_ids.clone(),
        removed: run.removed,
        run_dir: run.run_dir.clone(),
        stage,
        waiting_for: waiting,
        pr_url,
        ci,
    }
}

pub fn batch_views(d: &Dispatch) -> Vec<BatchView> {
    d.batches
        .iter()
        .map(|b| {
            let rows: Vec<BatchRowView> = b.runs.iter().map(|r| row_view(d, r)).collect();
            let waiting_for_you = rows
                .iter()
                .filter(|r| !r.removed && r.waiting_for.is_some())
                .count();
            BatchView {
                id: b.id.clone(),
                created_at: b.created_at.clone(),
                rows,
                waiting_for_you,
            }
        })
        .collect()
}

#[tauri::command]
pub async fn batch_overview(app: AppHandle) -> Result<Vec<BatchView>, String> {
    tauri::async_runtime::spawn_blocking(move || batch_views(&load(&data_dir(&app))))
        .await
        .map_err(|e| e.to_string())
}

/// Takes a run out of its batch (it keeps running; it no longer counts for the batch's milestones).
#[tauri::command]
pub async fn batch_remove_run(
    app: AppHandle,
    batch_id: String,
    entry_id: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        with(&data_dir(&app), |d| {
            if let Some(run) = d
                .batches
                .iter_mut()
                .find(|b| b.id == batch_id)
                .and_then(|b| b.runs.iter_mut().find(|r| r.entry_id == entry_id))
            {
                run.removed = true;
            }
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub cap: u32,
    pub usage_hold_percent: Option<u32>,
    pub queue: Vec<Entry>,
    pub holders: Vec<Holder>,
    pub at_cap: bool,
}

/// The run folders that have a tab with a live session now (a tab restored with no running session does not count).
pub fn open_run_dirs(app: &AppHandle) -> HashSet<String> {
    let tabs: Vec<(String, String)> = app
        .try_state::<Mutex<crate::store::StateStore>>()
        .and_then(|s| {
            s.lock().ok().map(|s| {
                s.sorted_tabs()
                    .into_iter()
                    .filter_map(|t| Some((t.id.clone(), t.launch.as_ref()?.run_dir.clone()?)))
                    .collect()
            })
        })
        .unwrap_or_default();
    let Some(registry) = app.try_state::<Mutex<crate::pty::PtyRegistry>>() else {
        return HashSet::new();
    };
    let Ok(registry) = registry.lock() else {
        return HashSet::new();
    };
    tabs.into_iter()
        .filter(|(id, _)| registry.live_count_of(std::slice::from_ref(id)) > 0)
        .map(|(_, dir)| dir)
        .collect()
}

pub fn overview_for(app: &AppHandle) -> Overview {
    let data = data_dir(app);
    let (cap, hold, watchdog) = limits(app);
    let open = open_run_dirs(app);
    let state = load(&data);
    let held = holders(&run_facts(&data, &open), &state.released, watchdog);
    let starting = state.queue.iter().filter(|e| e.state == "starting").count();
    Overview {
        cap,
        usage_hold_percent: hold,
        at_cap: held.len() + starting >= cap as usize,
        holders: held,
        queue: state.queue,
    }
}

#[tauri::command]
pub async fn dispatch_overview(app: AppHandle) -> Result<Overview, String> {
    tauri::async_runtime::spawn_blocking(move || overview_for(&app))
        .await
        .map_err(|e| e.to_string())
}

/// "Queue it" (the cap dialog, the intake): the start is stored and the dispatcher starts it when a slot is free.
#[tauri::command]
pub async fn dispatch_enqueue(
    app: AppHandle,
    params: StartParams,
    label: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let account_id = account_of(&app, params.window_id.as_deref())?;
        with(&data_dir(&app), |d| {
            enqueue(d, params, &account_id, &label, None)
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One run of a batch: the tickets it takes and its title.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TicketGroup {
    pub clickup: super::ClickUpStart,
    pub label: String,
}

/// A batch and its queue entries: one entry per group, each with that group's ticket ids and the intake's refs,
/// the same folder, repos, review and BMAD mode, the account, and the batch id.
pub fn plan_batch(
    base: &StartParams,
    groups: &[TicketGroup],
    account_id: &str,
    batch_id: &str,
) -> (Batch, Vec<Entry>) {
    let mut runs = Vec::new();
    let mut entries = Vec::new();
    for group in groups {
        let mut params = base.clone();
        params.clickup = Some(group.clickup.clone());
        params.dispatch_entry_id = None;
        let mut scratch = Dispatch::default();
        let id = enqueue(
            &mut scratch,
            params,
            account_id,
            &group.label,
            Some(batch_id.to_string()),
        );
        entries.extend(scratch.queue);
        runs.push(BatchRun {
            entry_id: id,
            ticket_ids: group.clickup.ticket_ids.clone(),
            label: group.label.clone(),
            run_dir: None,
            removed: false,
        });
    }
    (
        Batch {
            id: batch_id.to_string(),
            created_at: now_iso(),
            runs,
            notified: vec![],
        },
        entries,
    )
}

/// One run per ticket group: the groups become queue entries of one new batch (the dispatcher starts them
/// as slots allow, each through the shared start path with its own tickets snapshot). Returns the batch id.
#[tauri::command]
pub async fn dispatch_runs(
    app: AppHandle,
    base: StartParams,
    groups: Vec<TicketGroup>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if groups.is_empty() {
            return Err("No tickets to start.".to_string());
        }
        let account_id = account_of(&app, base.window_id.as_deref())?;
        let batch_id = new_id("b");
        let (batch, entries) = plan_batch(&base, &groups, &account_id, &batch_id);
        with(&data_dir(&app), |d| {
            d.queue.extend(entries);
            d.batches.push(batch);
        })?;
        Ok(batch_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The account of the window a start is made from (the focused window when none is given).
pub fn account_of(app: &AppHandle, window_id: Option<&str>) -> Result<String, String> {
    let state = app
        .try_state::<Mutex<crate::store::StateStore>>()
        .ok_or("state is not ready")?;
    let mut state = state.lock().map_err(|e| e.to_string())?;
    state.bind_window(window_id);
    let window = state.focus_window().to_string();
    Ok(state.account_for_window(&window))
}

#[tauri::command]
pub async fn dispatch_move(app: AppHandle, id: String, up: bool) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || with(&data_dir(&app), |d| move_entry(d, &id, up)))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn dispatch_cancel(app: AppHandle, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        with(&data_dir(&app), |d| {
            d.queue.retain(|e| !(e.id == id && e.state == "queued"))
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// "Start now": the entry moves to the head and ignores the cap (the page asks the user first).
#[tauri::command]
pub async fn dispatch_start_now(app: AppHandle, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        with(&data_dir(&app), |d| {
            if let Some(at) = d
                .queue
                .iter()
                .position(|e| e.id == id && e.state == "queued")
            {
                let mut e = d.queue.remove(at);
                e.bypass_cap = true;
                d.queue.insert(0, e);
            }
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// "Release slot": the run keeps going but no longer counts against the cap.
#[tauri::command]
pub async fn dispatch_release(app: AppHandle, run_dir: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = super::checked_run_dir(&data_dir(&app), &run_dir)?
            .display()
            .to_string();
        with(&data_dir(&app), |d| {
            if !d.released.contains(&dir) {
                d.released.push(dir);
            }
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(name: &str) -> StartParams {
        StartParams {
            window_id: None,
            folder: "/w".into(),
            request: name.into(),
            role: "eagle-eye".into(),
            clickup: None,
            review_mode: None,
            bmad_mode: None,
            force: None,
            repos: None,
            max_implementers: None,
            run_mode: None,
            dispatch_entry_id: None,
        }
    }

    fn facts(id: &str, stage: Option<&str>, tab: bool, ago: Option<u64>) -> RunSlotFacts {
        RunSlotFacts {
            run_dir: format!("/runs/{id}"),
            run_id: id.into(),
            stage: stage.map(String::from),
            paused: false,
            tab_open: tab,
            changed_ago_ms: ago,
        }
    }

    fn queue_of(names: &[&str]) -> Dispatch {
        let mut d = Dispatch::default();
        for n in names {
            enqueue(&mut d, params(n), "acct", n, None);
        }
        d
    }

    #[test]
    fn paused_ship_approval_done_and_failed_free_a_slot_and_a_released_run_holds_none() {
        let mut paused = facts("p", Some("building"), true, Some(0));
        paused.paused = true;
        let runs = vec![
            facts("a", Some("building"), true, Some(0)),
            paused,
            facts("s", Some("ship-approval"), true, Some(0)),
            facts("d", Some("done"), true, Some(0)),
            facts("f", Some("failed"), true, Some(0)),
            facts("r", Some("building"), true, Some(0)),
        ];
        let held = holders(&runs, &["/runs/r".to_string()], 10);
        assert_eq!(
            held.iter().map(|h| h.run_id.as_str()).collect::<Vec<_>>(),
            ["a"]
        );
    }

    #[test]
    fn a_run_with_no_tab_and_an_old_status_holds_no_slot_but_a_tab_or_a_recent_change_does() {
        let ten_min = 10 * 60_000;
        let runs = vec![
            facts("old", Some("planning"), false, Some(ten_min + 1)),
            facts("tab", Some("plan-approval"), true, Some(ten_min * 5)),
            facts("recent", Some("building"), false, Some(ten_min - 1)),
            facts("nostatus", None, true, None),
            facts("nostatus-notab", None, false, None),
        ];
        let held = holders(&runs, &[], 10);
        let why: Vec<(&str, &str)> = held
            .iter()
            .map(|h| (h.run_id.as_str(), h.why.as_str()))
            .collect();
        assert_eq!(
            why,
            [
                ("tab", "tab open"),
                ("recent", "recent change"),
                ("nostatus", "tab open")
            ]
        );
    }

    #[test]
    fn never_more_slot_holders_than_the_cap_across_ticks_and_a_freed_slot_starts_the_head() {
        let cap = 2u32;
        let mut state = queue_of(&["one", "two", "three", "four"]);
        let mut live: Vec<String> = vec![];
        let mut peak = 0;
        for tick in 0..12 {
            let held = live.len();
            match pick(&state, held, cap, false, |_| None) {
                Pick::Start(id) => {
                    let e = state.queue.iter_mut().find(|e| e.id == id).unwrap();
                    e.state = "starting".into();
                    // The launcher finishes within the tick: the entry leaves the queue and its run holds a slot.
                    let id2 = id.clone();
                    state.queue.retain(|e| e.id != id2);
                    live.push(id);
                }
                Pick::Wait(_) | Pick::Nothing => {}
            }
            peak = peak.max(live.len());
            assert!(live.len() <= cap as usize, "tick {tick}");
            if tick == 5 || tick == 8 {
                let freed = live.remove(0); // a run reaches ship-approval
                let _ = freed;
            }
        }
        assert!(peak <= cap as usize);
        assert!(
            state.queue.is_empty(),
            "the freed slot started the rest: {:?}",
            state.queue.len()
        );
    }

    #[test]
    fn a_slow_launcher_never_lets_a_second_start_begin_and_starting_holds_a_slot() {
        let mut state = queue_of(&["one", "two"]);
        let first = match pick(&state, 0, 2, false, |_| None) {
            Pick::Start(id) => id,
            other => panic!("{other:?}"),
        };
        state
            .queue
            .iter_mut()
            .find(|e| e.id == first)
            .unwrap()
            .state = "starting".into();
        for _ in 0..5 {
            assert_eq!(
                pick(&state, 0, 2, false, |_| None),
                Pick::Nothing,
                "a start is in flight"
            );
            assert_eq!(pick(&state, 0, 2, true, |_| None), Pick::Nothing);
        }
        // With the cap at 1 the starting entry itself fills it once the flag is gone.
        let cap1 = pick(&state, 1, 1, false, |_| None);
        assert_eq!(cap1, Pick::Nothing);
    }

    #[test]
    fn a_full_cap_makes_the_head_wait_with_its_reason() {
        let state = queue_of(&["one"]);
        assert_eq!(
            pick(&state, 2, 2, false, |_| None),
            Pick::Wait("2 of 2 slots are in use".into())
        );
        assert_eq!(
            pick(&state, 1, 2, false, |_| Some("held".into())),
            Pick::Wait("held".into())
        );
        assert!(matches!(
            pick(&state, 1, 2, false, |_| None),
            Pick::Start(_)
        ));
    }

    #[test]
    fn start_now_bypasses_the_cap_and_the_hold() {
        let mut state = queue_of(&["one", "two"]);
        state.queue[1].bypass_cap = true;
        let moved = state.queue.remove(1);
        state.queue.insert(0, moved);
        assert!(matches!(
            pick(&state, 5, 2, false, |_| Some("held".into())),
            Pick::Start(_)
        ));
    }

    #[test]
    fn the_usage_hold_blocks_only_at_or_past_the_percent_and_not_past_the_reset() {
        let now = 1_000;
        assert!(
            usage_hold(None, Some((99.0, Some(2_000))), now).is_none(),
            "off"
        );
        assert!(usage_hold(Some(80), None, now).is_none(), "no reading");
        assert!(
            usage_hold(Some(80), Some((79.9, Some(2_000))), now).is_none(),
            "below"
        );
        assert!(usage_hold(Some(80), Some((80.0, Some(2_000))), now)
            .unwrap()
            .contains("80%"));
        assert!(
            usage_hold(Some(80), Some((95.0, Some(900))), now).is_none(),
            "the window already reset"
        );
        assert!(usage_hold(Some(80), Some((95.0, None)), now).is_some());
    }

    #[test]
    fn up_and_down_move_queued_entries_and_the_file_round_trips_with_unknown_fields() {
        let mut state = queue_of(&["a", "b", "c"]);
        let ids: Vec<String> = state.queue.iter().map(|e| e.id.clone()).collect();
        assert!(move_entry(&mut state, &ids[2], true));
        assert_eq!(
            state
                .queue
                .iter()
                .map(|e| e.label.as_str())
                .collect::<Vec<_>>(),
            ["a", "c", "b"]
        );
        assert!(
            !move_entry(&mut state, &ids[0], true),
            "the head cannot go up"
        );
        state.queue[0].state = "starting".into();
        assert!(
            !move_entry(&mut state, &ids[2], true),
            "nothing moves above a starting entry"
        );
        let dir = std::env::temp_dir().join(format!("dcv2-dispatch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        state
            .extra
            .insert("futureField".into(), serde_json::json!({"x": 1}));
        save(&dir, &state).unwrap();
        let back = load(&dir);
        assert_eq!(back, state);
        assert_eq!(back.extra["futureField"]["x"], 1);
        // A corrupt file starts empty and keeps a .bad copy.
        std::fs::write(path_in(&dir), "{nope").unwrap();
        assert_eq!(load(&dir), Dispatch::default());
        assert!(dir.join("dispatch.json.bad").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_starting_entry_reloads_queued_with_the_reason_or_is_dropped_when_it_did_start() {
        let mut state = queue_of(&["did", "did-not", "waiting"]);
        state.queue[0].state = "starting".into();
        state.queue[1].state = "starting".into();
        let started: HashSet<String> = [state.queue[0].id.clone()].into();
        recover(&mut state, &started);
        assert_eq!(state.queue.len(), 2);
        assert_eq!(state.queue[0].label, "did-not");
        assert_eq!(state.queue[0].state, "queued");
        assert_eq!(
            state.queue[0].reason.as_deref(),
            Some("app quit during start")
        );
        assert_eq!(state.queue[1].state, "queued");
    }

    #[test]
    fn a_failed_start_keeps_the_entry_with_its_reason_and_a_good_one_leaves_the_queue_and_marks_its_batch(
    ) {
        let mut state = queue_of(&["one", "two"]);
        let (first, second) = (state.queue[0].id.clone(), state.queue[1].id.clone());
        state.queue[0].state = "starting".into();
        finish(&mut state, &first, Err("ClickUp did not answer".into()));
        assert_eq!(state.queue.len(), 2);
        assert_eq!(state.queue[0].state, "queued");
        assert_eq!(
            state.queue[0].reason.as_deref(),
            Some("ClickUp did not answer")
        );
        state.queue[1].batch_id = Some("b-1".into());
        state.batches.push(Batch {
            id: "b-1".into(),
            created_at: "t".into(),
            runs: vec![BatchRun {
                entry_id: second.clone(),
                ticket_ids: vec!["t2".into()],
                label: "two".into(),
                run_dir: None,
                removed: false,
            }],
            notified: vec![],
        });
        finish(&mut state, &second, Ok(Some("/runs/r-2".into())));
        assert_eq!(state.queue.len(), 1);
        assert_eq!(
            state.batches[0].runs[0].run_dir.as_deref(),
            Some("/runs/r-2")
        );
    }

    fn group(id: &str) -> TicketGroup {
        TicketGroup {
            clickup: super::super::ClickUpStart {
                refs: vec![crate::clickup::intake::IntakeRef {
                    kind: "list".into(),
                    id: "L1".into(),
                    team_id: None,
                    custom: false,
                    line: 1,
                }],
                ticket_ids: vec![id.into()],
            },
            label: format!("Ticket {id}"),
        }
    }

    #[test]
    fn three_tickets_become_three_entries_of_one_batch_each_with_only_its_own_ticket() {
        let mut base = params("Do the tickets");
        base.repos = Some(vec!["api".into()]);
        base.review_mode = Some("github".into());
        base.bmad_mode = Some("auto".into());
        let groups = vec![group("t1"), group("t2"), group("t3")];
        let (batch, entries) = plan_batch(&base, &groups, "acct-9", "b-1");
        assert_eq!(entries.len(), 3);
        let ids: HashSet<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids.len(), 3, "distinct entry ids");
        for (entry, tid) in entries.iter().zip(["t1", "t2", "t3"]) {
            let c = entry.params.clickup.as_ref().unwrap();
            assert_eq!(c.ticket_ids, [tid], "only its own ticket");
            assert_eq!(c.refs.len(), 1, "the intake's refs go with each run");
            assert_eq!(entry.account_id, "acct-9");
            assert_eq!(entry.batch_id.as_deref(), Some("b-1"));
            assert_eq!(entry.params.repos, base.repos);
            assert_eq!(entry.params.review_mode.as_deref(), Some("github"));
            assert_eq!(entry.params.bmad_mode.as_deref(), Some("auto"));
        }
        assert_eq!(
            batch
                .runs
                .iter()
                .map(|r| r.ticket_ids.clone())
                .collect::<Vec<_>>(),
            [["t1"], ["t2"], ["t3"]]
        );
        assert!(batch.runs.iter().all(|r| !r.removed && r.run_dir.is_none()));
    }

    #[test]
    fn three_runs_of_one_repo_get_distinct_run_ids_folders_and_branches_with_no_ticket_bodies_in_request_md(
    ) {
        use super::super::folder::{create_run, RunMeta};
        let dir = crate::test_support::TempDir::new("batch_runs");
        let repos = vec![crate::run::repos::Repo {
            name: "api".into(),
            path: "/w/api".into(),
            check: None,
            setup: None,
            copy_files: vec![],
            live_tests: false,
        }];
        let meta = RunMeta {
            account_id: "a".into(),
            ..Default::default()
        };
        let mut made = Vec::new();
        for tid in ["t1", "t2", "t3"] {
            // The request is the user's words plus "- <id> <title>" lines: never a ticket body.
            let request = format!("Do the tickets\n\nTickets from ClickUp (1; their details are in tickets.json):\n- {tid} Title {tid}\n");
            made.push(
                create_run(
                    dir.path(),
                    std::path::Path::new("/w"),
                    &repos,
                    &[],
                    &request,
                    &meta,
                )
                .unwrap(),
            );
        }
        let run_ids: HashSet<&str> = made.iter().map(|m| m.run_id.as_str()).collect();
        let run_dirs: HashSet<&str> = made.iter().map(|m| m.run_dir.as_str()).collect();
        assert_eq!((run_ids.len(), run_dirs.len()), (3, 3));
        let branches: HashSet<String> = made
            .iter()
            .map(|m| format!("dcv2/{}-pr1", m.run_id))
            .collect();
        assert_eq!(
            branches.len(),
            3,
            "branch names derived from the run ids do not collide"
        );
        for (m, tid) in made.iter().zip(["t1", "t2", "t3"]) {
            let request =
                std::fs::read_to_string(std::path::Path::new(&m.run_dir).join("request.md"))
                    .unwrap();
            assert!(request.contains(&format!("- {tid} Title {tid}")));
            assert!(
                !request.contains("description"),
                "no ticket bodies in request.md"
            );
        }
    }

    #[test]
    fn a_batch_and_its_removed_flags_survive_a_reload_and_the_milestones_fire_once() {
        let (mut batch, _) = plan_batch(&params("x"), &[group("t1"), group("t2")], "a", "b-1");
        batch.runs[1].removed = true;
        let mut state = Dispatch::default();
        state.batches.push(batch);
        let back: Dispatch = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert!(back.batches[0].runs[1].removed && !back.batches[0].runs[0].removed);
        let views = batch_views(&back);
        assert!(
            views[0].rows[1].removed,
            "a removed member is reported removed"
        );
        // Milestones: a queued member keeps both back; planned fires once; ready covers the rest.
        let none: Vec<String> = vec![];
        assert_eq!(batch_milestone(&[None, Some("plan-approval")], &none), None);
        assert_eq!(
            batch_milestone(&[Some("planning"), Some("plan-approval")], &none),
            None
        );
        assert_eq!(
            batch_milestone(&[Some("plan-approval"), Some("building")], &none),
            Some("batch-planned")
        );
        assert_eq!(
            batch_milestone(
                &[Some("plan-approval"), Some("building")],
                &["batch-planned".to_string()]
            ),
            None,
            "once only"
        );
        assert_eq!(
            batch_milestone(
                &[Some("ship-approval"), Some("done")],
                &["batch-planned".to_string()]
            ),
            Some("batch-ready")
        );
        assert_eq!(
            batch_milestone(&[Some("ship-approval"), Some("failed")], &none),
            None,
            "a failed member holds both back"
        );
        assert_eq!(batch_milestone(&[], &none), None);
    }

    #[test]
    fn a_batch_counts_the_runs_waiting_for_you() {
        let dir = std::env::temp_dir().join(format!("dcv2-batch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("status.json"),
            r#"{"stage":"plan-approval","waitingFor":"plan-approval","prs":[]}"#,
        )
        .unwrap();
        let (mut batch, _) = plan_batch(&params("x"), &[group("t1"), group("t2")], "a", "b-2");
        batch.runs[0].run_dir = Some(dir.display().to_string());
        let mut state = Dispatch::default();
        state.batches.push(batch);
        let view = &batch_views(&state)[0];
        assert_eq!(view.waiting_for_you, 1);
        assert_eq!(view.rows[0].stage, "plan-approval");
        assert_eq!(view.rows[0].waiting_for.as_deref(), Some("plan-approval"));
        assert_eq!(view.rows[1].stage, "queued");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_queued_entry_keeps_its_refs_and_account_across_a_reload() {
        let mut p = params("tickets");
        p.clickup = Some(super::super::ClickUpStart {
            refs: vec![crate::clickup::intake::IntakeRef {
                kind: "task".into(),
                id: "abc".into(),
                team_id: None,
                custom: false,
                line: 0,
            }],
            ticket_ids: vec!["abc".into()],
        });
        let mut state = Dispatch::default();
        enqueue(
            &mut state,
            p.clone(),
            "acct-2",
            "Ticket abc",
            Some("b-1".into()),
        );
        let json = serde_json::to_string(&state).unwrap();
        let back: Dispatch = serde_json::from_str(&json).unwrap();
        assert_eq!(back.queue[0].params.clickup, p.clickup);
        assert_eq!(back.queue[0].account_id, "acct-2");
        assert_eq!(back.queue[0].batch_id.as_deref(), Some("b-1"));
    }
}
