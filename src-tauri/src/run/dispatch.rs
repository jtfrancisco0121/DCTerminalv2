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
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
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
    /// The epic wave this entry starts, when it is one.
    #[serde(default)]
    pub wave: Option<WaveRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WaveRef {
    pub epic_run: String,
    pub n: u32,
}

/// One started (or queued) wave of an epic: its entry and, once started, its run folder.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WaveRun {
    pub n: u32,
    pub entry_id: String,
    #[serde(default)]
    pub run_dir: Option<String>,
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
    /// Milestones already announced (`batch-planned`, `batch-ready`, `eval-finished`): each fires once.
    #[serde(default)]
    pub notified: Vec<String>,
    /// `eval` for an eval group (the same task N times); missing is a ticket batch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// An eval group: the run id it repeats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// An eval group: how many runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    /// Fields a later version adds are kept when this one rewrites the file.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
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
    #[serde(default)]
    pub wave_runs: Vec<WaveRun>,
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

/// The usage hold of an eval entry. Eval is the one feature that spends usage on purpose, so with a hold set
/// and no five-hour reading at all it holds ("not known"); a reading past its `resets_at` is clear (the
/// window has reset, so usage is near 0%, the same rule as `usage_hold`); else it is `usage_hold`. With no
/// hold set it is clear.
pub fn eval_hold(
    hold: Option<u32>,
    reading: Option<(f64, Option<i64>)>,
    now_s: i64,
) -> Option<String> {
    hold?;
    if reading.is_none() {
        return Some("the five-hour usage is not known".into());
    }
    usage_hold(hold, reading, now_s)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// Start this entry now.
    Start(String),
    /// Nothing starts; the head entry waits for this reason.
    Wait(String),
    Nothing,
}

/// The next start, never while a start is in flight, and never past the cap (held slots plus entries that
/// are `starting`). It looks at the head of the queue, with one exception: an eval entry whose hold applies
/// is passed over, so a held eval group cannot stall normal, wave or other-account entries behind it.
/// The first entry that is not a held eval entry gets today's rules (the cap, `bypass_cap`, the usage hold);
/// a normal head is never passed over. For an eval entry the hold is never skipped, not even by `bypass_cap`.
/// With only held eval entries queued the answer is `Wait` with the first one's reason. `hold` gives an
/// entry's usage hold reason (`eval_hold` for an entry with `params.eval`, `usage_hold` for the others).
pub fn pick(
    state: &Dispatch,
    held: usize,
    cap: u32,
    in_flight: bool,
    hold: impl Fn(&Entry) -> Option<String>,
) -> Pick {
    if !state.queue.iter().any(|e| e.state == "queued") {
        return Pick::Nothing;
    }
    if in_flight || state.queue.iter().any(|e| e.state == "starting") {
        return Pick::Nothing;
    }
    let mut first_held: Option<String> = None;
    for entry in state.queue.iter().filter(|e| e.state == "queued") {
        let is_eval = entry.params.eval.is_some();
        if is_eval {
            if let Some(why) = hold(entry) {
                first_held.get_or_insert(why);
                continue;
            }
        }
        if !entry.bypass_cap && held >= cap as usize {
            return Pick::Wait(format!("{held} of {cap} slots are in use"));
        }
        if !entry.bypass_cap && !is_eval {
            if let Some(why) = hold(entry) {
                return Pick::Wait(why);
            }
        }
        return Pick::Start(entry.id.clone());
    }
    Pick::Wait(first_held.unwrap_or_default())
}

/// Each queued eval entry whose hold applies, with the reason (shown on the queue while entries behind it
/// start).
pub fn held_eval_entries(
    state: &Dispatch,
    hold: impl Fn(&Entry) -> Option<String>,
) -> Vec<(String, String)> {
    state
        .queue
        .iter()
        .filter(|e| e.state == "queued" && e.params.eval.is_some())
        .filter_map(|e| hold(e).map(|why| (e.id.clone(), why)))
        .collect()
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
        wave: None,
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
/// When the last start finished (epoch ms). The tab snapshot handed to `on_tick` can predate that moment, so a
/// new start waits a moment for the tab to show up in the facts; else the cap could be exceeded by one.
static LAST_FINISH_MS: AtomicI64 = AtomicI64::new(0);
const SETTLE_MS: i64 = 3000;

fn finish_start() {
    LAST_FINISH_MS.store(chrono::Utc::now().timestamp_millis(), Ordering::SeqCst);
    IN_FLIGHT.store(false, Ordering::SeqCst);
}

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
    super::epics::auto_start(app);
    let state = load(&data);
    eval_check_tick(app, &state);
    if state.queue.is_empty() {
        return;
    }
    if chrono::Utc::now().timestamp_millis() - LAST_FINISH_MS.load(Ordering::SeqCst) < SETTLE_MS {
        return;
    }
    let (cap, hold, watchdog) = limits(app);
    let facts = run_facts(&data, tab_run_dirs);
    let held = holders(&facts, &state.released, watchdog).len();
    let now_s = chrono::Utc::now().timestamp();
    let entry_hold = |e: &Entry| {
        let reading = usage_reading(app, &e.account_id);
        if e.params.eval.is_some() {
            eval_hold(hold, reading, now_s)
        } else {
            usage_hold(hold, reading, now_s)
        }
    };
    let decision = pick(&state, held, cap, false, entry_hold);
    // A held eval entry shows its own wait reason, whatever starts behind it.
    let held_eval = held_eval_entries(&state, entry_hold);
    let set_reason = |id: &str, why: &str| {
        if state
            .queue
            .iter()
            .find(|e| e.id == id)
            .is_some_and(|e| e.reason.as_deref() != Some(why))
        {
            let _ = with(&data, |d| {
                if let Some(e) = d.queue.iter_mut().find(|e| e.id == id) {
                    e.reason = Some(why.to_string());
                }
            });
        }
    };
    for (id, why) in &held_eval {
        set_reason(id, why);
    }
    match decision {
        Pick::Nothing => {}
        Pick::Wait(why) => {
            // The entry that waits is the first one that is not a held eval entry (the head, as today).
            if let Some(waiting) = state
                .queue
                .iter()
                .find(|e| e.state == "queued" && !held_eval.iter().any(|(id, _)| *id == e.id))
            {
                set_reason(&waiting.id, &why);
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
                finish_start();
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
            let wave = state
                .queue
                .iter()
                .find(|e| e.id == id)
                .and_then(|e| e.wave.clone());
            state.queue.retain(|e| e.id != id);
            if let Some(w) = wave {
                if let Some(run) = state
                    .epics
                    .iter_mut()
                    .find(|e| e.run_dir == w.epic_run)
                    .and_then(|e| {
                        e.wave_runs
                            .iter_mut()
                            .find(|r| r.n == w.n && r.entry_id == id)
                    })
                {
                    run.run_dir = run_dir.clone();
                }
            }
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

// ---- the app check of finished eval runs (AD-48) ---------------------------------------------

/// What the selector needs to know of one run of an eval batch.
#[derive(Debug, Clone, PartialEq)]
pub struct EvalCheckFacts {
    pub run_dir: String,
    /// `finished_eval`: an eval run that ended at `ship-approval`.
    pub finished_eval: bool,
    pub has_app_checks: bool,
    /// At least one of its worktrees still exists.
    pub has_worktree: bool,
}

/// The one finished eval run to give an app check (R1) next, or `None`: it belongs to an eval batch, is a
/// finished eval run, has no app-checks.json, still has a worktree, and was not tried in this app session. At
/// most one; a normal batch's runs are never picked.
pub fn eval_run_to_check(
    batches: &[Batch],
    facts: &[EvalCheckFacts],
    tried: &HashSet<String>,
) -> Option<String> {
    batches
        .iter()
        .filter(|b| is_eval_batch(b))
        .flat_map(|b| b.runs.iter().filter(|r| !r.removed))
        .filter_map(|r| r.run_dir.as_deref())
        .find(|dir| {
            !tried.contains(*dir)
                && facts.iter().any(|f| {
                    f.run_dir == *dir && f.finished_eval && !f.has_app_checks && f.has_worktree
                })
        })
        .map(str::to_string)
}

/// The facts of every started run of the eval batches, read cheaply (files only, no git).
pub fn eval_check_facts(batches: &[Batch]) -> Vec<EvalCheckFacts> {
    batches
        .iter()
        .filter(|b| is_eval_batch(b))
        .flat_map(|b| b.runs.iter().filter(|r| !r.removed))
        .filter_map(|r| r.run_dir.as_deref())
        .map(|dir| {
            let path = Path::new(dir);
            let status = super::folder::read_json_value(&path.join("status.json"));
            let stage = status
                .as_ref()
                .and_then(|v| v["stage"].as_str().map(String::from));
            let has_worktree = status
                .as_ref()
                .and_then(|v| v["prs"].as_array())
                .is_some_and(|prs| {
                    prs.iter()
                        .filter_map(|pr| pr["worktree"].as_str())
                        .any(|wt| !wt.is_empty() && Path::new(wt).is_dir())
                });
            EvalCheckFacts {
                run_dir: dir.to_string(),
                finished_eval: super::folder::finished_eval(
                    super::folder::eval_of(path).as_ref(),
                    stage.as_deref(),
                ),
                has_app_checks: path.join("app-checks.json").is_file(),
                has_worktree,
            }
        })
        .collect()
}

/// One eval check at a time, app-wide.
static EVAL_CHECK_RUNNING: AtomicBool = AtomicBool::new(false);
/// Runs given a check in this app session: a failed check is not retried, and neither is a run whose slot a
/// cleanup holds.
static EVAL_CHECK_TRIED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Gives the next finished eval run its app check (R1: the repo's own check commands in its worktrees, no Claude
/// usage), on a thread of its own, never on the tick, and never twice in a session for the same run.
fn eval_check_tick(app: &AppHandle, state: &Dispatch) {
    if state.batches.is_empty() || EVAL_CHECK_RUNNING.load(Ordering::SeqCst) {
        return;
    }
    let facts = eval_check_facts(&state.batches);
    let Some(dir) = ({
        let Ok(mut tried) = EVAL_CHECK_TRIED.lock() else {
            return;
        };
        let tried = tried.get_or_insert_with(HashSet::new);
        let picked = eval_run_to_check(&state.batches, &facts, tried);
        if let Some(dir) = &picked {
            tried.insert(dir.clone());
        }
        picked
    }) else {
        return;
    };
    if EVAL_CHECK_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let data = data_dir(app);
    let spawned = std::thread::Builder::new()
        .name("dcv2-eval-check".into())
        .spawn(move || {
            // The same checked folder the R1 slot is keyed by; a refusal (a cleanup holds it) is not retried.
            if let Ok(run_dir) = super::checked_run_dir(&data, &dir) {
                let _ = super::rerun_checks_once(&run_dir);
            }
            EVAL_CHECK_RUNNING.store(false, Ordering::SeqCst);
        });
    if spawned.is_err() {
        EVAL_CHECK_RUNNING.store(false, Ordering::SeqCst);
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

/// An eval group: `kind` says so, or (when an older build dropped `kind` on a rewrite) any of its runs'
/// meta.json has `eval`. Such a batch never raises `batch-ready`.
pub fn is_eval_batch(batch: &Batch) -> bool {
    batch.kind.as_deref() == Some("eval")
        || batch.runs.iter().any(|r| {
            r.run_dir
                .as_deref()
                .is_some_and(|d| super::folder::eval_of(Path::new(d)).is_some())
        })
}

/// The one milestone of an eval group: `eval-finished`, once, when every counted run has ended (at
/// `ship-approval`, which only an eval run's end writes, or `failed`). A run still queued (no stage) holds
/// it back. An eval group never gets `batch-planned` or `batch-ready`.
pub fn eval_milestone(stages: &[Option<&str>], notified: &[String]) -> Option<&'static str> {
    let ended = |s: &Option<&str>| matches!(s, Some("ship-approval" | "failed"));
    (!stages.is_empty()
        && stages.iter().all(ended)
        && !notified.iter().any(|n| n == "eval-finished"))
    .then_some("eval-finished")
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
    let mut waves = super::epics::wave_ready_events(app);
    if !path_in(&data).exists() {
        return waves;
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
            if is_eval_batch(batch) {
                if let Some(name) = eval_milestone(&refs, &batch.notified) {
                    batch.notified.push(name.to_string());
                    fired.push((batch.id.clone(), name));
                }
            } else if let Some(name) = batch_milestone(&refs, &batch.notified) {
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
    let batch_events = fired.into_iter().map(|(id, name)| Event {
        kind: match name {
            "batch-ready" => Kind::BatchReady,
            "eval-finished" => Kind::EvalFinished,
            _ => Kind::BatchPlanned,
        },
        subject: id.clone(),
        tab_id: None,
        window_label: None,
        tab_label: None,
        run_id: Some(id),
        run_dir: None,
        detail: None,
    });
    waves.extend(batch_events);
    waves
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
    /// Set for an eval group: it reads "k of n finished", never "waiting for you".
    pub eval: Option<EvalBatchView>,
}

/// What the overview shows of an eval group.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalBatchView {
    /// How many runs the group has.
    pub n: u32,
    /// The run it repeats.
    pub source: String,
    /// Counted runs that have ended (at `ship-approval`, or failed).
    pub finished: usize,
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
            let eval = is_eval_batch(b).then(|| EvalBatchView {
                n: b.n.unwrap_or(rows.len() as u32),
                source: b.source.clone().unwrap_or_default(),
                finished: rows
                    .iter()
                    .filter(|r| {
                        !r.removed && matches!(r.stage.as_str(), "ship-approval" | "failed")
                    })
                    .count(),
            });
            BatchView {
                id: b.id.clone(),
                created_at: b.created_at.clone(),
                rows,
                waiting_for_you,
                eval,
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
        let mut params = params;
        params.eval = None; // only `dispatch_eval_start` makes an eval entry
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
            kind: None,
            source: None,
            n: None,
            extra: Map::new(),
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
        let mut base = base;
        base.eval = None; // only `dispatch_eval_start` makes an eval entry
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

// ---- eval groups (AF14): the same task N times, on the dispatch queue --------------------------

/// Why an eval group cannot start from this source run, or `Ok`. Pure: the settings, the window's account,
/// the hold and the clock come in as values.
pub fn eval_refusal(
    source: &Path,
    n: u32,
    eval_enabled: bool,
    window_account: &str,
    hold: Option<u32>,
    reading: Option<(f64, Option<i64>)>,
    now_s: i64,
) -> Result<(), String> {
    use super::folder;
    if !eval_enabled {
        return Err("Eval mode is off: turn it on in Settings > Activity.".into());
    }
    if !folder::EVAL_SIZES.contains(&n) {
        return Err("An eval group has 2 to 10 runs.".into());
    }
    let stage = folder::read_json_value(&source.join("status.json"))
        .and_then(|v| v["stage"].as_str().map(String::from));
    if !matches!(stage.as_deref(), Some("done" | "failed")) {
        return Err("Only a finished run (done or failed) can be evaluated.".into());
    }
    if folder::eval_of(source).is_some() {
        return Err("An eval run is not evaluated again.".into());
    }
    if folder::read_json_value(&source.join("meta.json"))
        .is_some_and(|m| m.get("mode").is_some_and(|mode| !mode.is_null()))
    {
        return Err(
            "An epic-plan run builds nothing, so there is no task to repeat: evaluate a wave run."
                .into(),
        );
    }
    let source_account = folder::read_json_value(&source.join("meta.json"))
        .and_then(|m| m["account"]["id"].as_str().map(String::from));
    if source_account.as_deref() != Some(window_account) {
        return Err(
            "This window's account is not the account of the source run: open the run's account."
                .into(),
        );
    }
    match eval_hold(hold, reading, now_s) {
        Some(why) => Err(format!("Not started: {why}.")),
        None => Ok(()),
    }
}

/// What a source run built each repo on: its status.json `prs[]` (`repoPath`, `base`, `baseCommit`), the
/// entries with a `baseCommit` only; the first PR of a repo wins.
fn source_bases(
    source: &Path,
) -> Option<std::collections::BTreeMap<String, super::folder::EvalBase>> {
    let status = super::folder::read_json_value(&source.join("status.json"))?;
    let mut bases = std::collections::BTreeMap::new();
    for pr in status.get("prs")?.as_array()? {
        let (Some(path), Some(commit)) = (
            pr["repoPath"].as_str().filter(|p| !p.is_empty()),
            pr["baseCommit"].as_str().filter(|c| !c.is_empty()),
        ) else {
            continue;
        };
        bases
            .entry(path.to_string())
            .or_insert_with(|| super::folder::EvalBase {
                base: pr["base"].as_str().unwrap_or("").to_string(),
                base_commit: commit.to_string(),
            });
    }
    (!bases.is_empty()).then_some(bases)
}

/// The words of a source run's request.md for a new run: the body after `## What to do`, cut before the
/// `Tickets from ClickUp (` block (the start re-adds it from the fresh snapshot), not normalised.
fn eval_request_words(text: &str) -> String {
    let body = super::folder::request_md_body(text);
    let kept: Vec<&str> = body
        .lines()
        .take_while(|line| !line.starts_with("Tickets from ClickUp ("))
        .collect();
    kept.join("\n").trim().to_string()
}

/// request.md's `repos:` limit, when it has one.
fn request_repos_limit(text: &str) -> Option<Vec<String>> {
    let names: Vec<String> = text
        .lines()
        .take_while(|line| !line.starts_with("## "))
        .find_map(|line| line.strip_prefix("repos:"))?
        .split(',')
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect();
    (!names.is_empty()).then_some(names)
}

/// The `n` starts of an eval group, read from the source run's folder: the same start folder, request words,
/// repos limit and tickets, the source's BMAD mode and Implementer cap, `local` review, `force` (a
/// finished source is not a duplicate), and `eval` {group, index, of, source, bases}.
pub fn eval_params(
    source: &Path,
    window_id: Option<&str>,
    group: &str,
    n: u32,
) -> Result<Vec<StartParams>, String> {
    use super::folder;
    let folder_path = folder::run_folder_of(source).ok_or("The source run has no start folder.")?;
    let text = std::fs::read_to_string(source.join("request.md"))
        .map_err(|e| format!("The source run's request.md: {e}"))?;
    let request = eval_request_words(&text);
    if request.is_empty() {
        return Err("The source run has no request to repeat.".into());
    }
    let clickup = folder::read_json_value(&source.join("tickets.json")).and_then(|t| {
        let refs: Vec<crate::clickup::intake::IntakeRef> =
            serde_json::from_value(t["source"]["refs"].clone()).ok()?;
        let ticket_ids: Vec<String> = t["tasks"]
            .as_array()?
            .iter()
            .filter_map(|task| task["id"].as_str().map(String::from))
            .collect();
        (!ticket_ids.is_empty()).then_some(super::ClickUpStart { refs, ticket_ids })
    });
    let bases = source_bases(source);
    let source_id = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("The source run has no id.")?
        .to_string();
    Ok((1..=n)
        .map(|index| StartParams {
            window_id: window_id.map(String::from),
            folder: folder_path.clone(),
            request: request.clone(),
            role: "eagle-eye".into(),
            clickup: clickup.clone(),
            review_mode: Some("local".into()),
            bmad_mode: Some(folder::bmad_mode_of(source)),
            force: Some(true),
            repos: request_repos_limit(&text),
            max_implementers: folder::max_implementers_of(source),
            run_mode: None,
            dispatch_entry_id: None,
            eval: Some(folder::EvalRef {
                group: group.to_string(),
                index,
                of: n,
                source: source_id.clone(),
                bases: bases.clone(),
            }),
        })
        .collect())
}

/// The batch and queue entries of an eval group, in order.
pub fn plan_eval_batch(
    params: Vec<StartParams>,
    account_id: &str,
    batch_id: &str,
    source_id: &str,
) -> (Batch, Vec<Entry>) {
    let n = params.len() as u32;
    let mut scratch = Dispatch::default();
    let mut runs = Vec::new();
    for (i, p) in params.into_iter().enumerate() {
        let ticket_ids = p
            .clickup
            .as_ref()
            .map(|c| c.ticket_ids.clone())
            .unwrap_or_default();
        let label = format!("Eval run {} of {n}", i + 1);
        let id = enqueue(
            &mut scratch,
            p,
            account_id,
            &label,
            Some(batch_id.to_string()),
        );
        runs.push(BatchRun {
            entry_id: id,
            ticket_ids,
            label,
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
            kind: Some("eval".into()),
            source: Some(source_id.to_string()),
            n: Some(n),
            extra: Map::new(),
        },
        scratch.queue,
    )
}

/// "Evaluate N times…": records one eval batch and queues its n entries on the window's account. Refused with
/// the reason unless Eval mode is on, the source is a finished normal run of that account and no usage hold
/// applies (see `eval_refusal`). Returns the batch id.
#[tauri::command]
pub async fn dispatch_eval_start(
    app: AppHandle,
    window_id: Option<String>,
    source_run_dir: String,
    n: u32,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let data = data_dir(&app);
        let source = super::checked_run_dir(&data, &source_run_dir)?;
        let account_id = account_of(&app, window_id.as_deref())?;
        let eval_enabled = app
            .try_state::<Mutex<crate::store::SettingsStore>>()
            .and_then(|s| s.lock().ok().map(|s| s.observe().eval_enabled))
            .unwrap_or(false);
        let (_, hold, _) = limits(&app);
        eval_refusal(
            &source,
            n,
            eval_enabled,
            &account_id,
            hold,
            usage_reading(&app, &account_id),
            chrono::Utc::now().timestamp(),
        )?;
        let batch_id = new_id("b");
        let params = eval_params(&source, window_id.as_deref(), &batch_id, n)?;
        let source_id = source
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let (batch, entries) = plan_eval_batch(params, &account_id, &batch_id, &source_id);
        with(&data, |d| {
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
            eval: None,
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
            kind: None,
            source: None,
            n: None,
            extra: Map::new(),
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

    fn eval_ref(index: u32) -> super::super::folder::EvalRef {
        super::super::folder::EvalRef {
            group: "b-1".into(),
            index,
            of: 3,
            source: "r-0".into(),
            bases: None,
        }
    }

    fn eval_queue(names: &[&str]) -> Dispatch {
        let mut d = Dispatch::default();
        for (i, n) in names.iter().enumerate() {
            let mut p = params(n);
            if n.starts_with("eval") {
                p.eval = Some(eval_ref(i as u32 + 1));
            }
            enqueue(&mut d, p, "acct", n, None);
        }
        d
    }

    #[test]
    fn eval_hold_fails_closed_only_with_no_reading_at_all() {
        let now = 1_000_000;
        assert_eq!(eval_hold(None, None, now), None, "no hold set is clear");
        assert_eq!(eval_hold(None, Some((99.0, None)), now), None);
        assert_eq!(
            eval_hold(Some(80), None, now).as_deref(),
            Some("the five-hour usage is not known")
        );
        assert_eq!(
            eval_hold(Some(80), Some((95.0, Some(now - 1))), now),
            None,
            "a reading past its reset is clear: the window has reset"
        );
        assert!(eval_hold(Some(80), Some((80.0, Some(now + 600))), now).is_some());
        assert_eq!(
            eval_hold(Some(80), Some((79.0, Some(now + 600))), now),
            None
        );
        // A normal entry still starts on unknown usage, as today.
        assert_eq!(usage_hold(Some(80), None, now), None);
    }

    #[test]
    fn pick_never_skips_the_hold_of_an_eval_entry_even_on_start_now() {
        let mut d = eval_queue(&["eval-1"]);
        d.queue[0].bypass_cap = true;
        let held = |e: &Entry| {
            e.params
                .eval
                .as_ref()
                .map(|_| "usage is not known".to_string())
        };
        assert_eq!(
            pick(&d, 5, 2, false, held),
            Pick::Wait("usage is not known".into()),
            "Start now skips the cap but not the hold"
        );
        // Not held: bypass_cap skips the cap.
        assert_eq!(
            pick(&d, 5, 2, false, |_| None),
            Pick::Start(d.queue[0].id.clone())
        );
        // A normal entry with bypass_cap still bypasses both, as today.
        let mut n = queue_of(&["normal"]);
        n.queue[0].bypass_cap = true;
        assert_eq!(
            pick(&n, 5, 2, false, |_| Some("held".into())),
            Pick::Start(n.queue[0].id.clone())
        );
    }

    #[test]
    fn a_held_eval_entry_at_the_head_does_not_stop_the_queue_behind_it() {
        let d = eval_queue(&["eval-1", "normal", "eval-3"]);
        let hold = |e: &Entry| {
            e.params
                .eval
                .as_ref()
                .map(|_| "usage is not known".to_string())
        };
        // The normal entry behind the held eval head starts.
        assert_eq!(
            pick(&d, 0, 2, false, hold),
            Pick::Start(d.queue[1].id.clone())
        );
        // A wave entry and another account's entry behind it start too.
        let mut w = eval_queue(&["eval-1", "wave"]);
        w.queue[1].wave = Some(WaveRef {
            epic_run: "/e".into(),
            n: 2,
        });
        assert_eq!(
            pick(&w, 0, 2, false, hold),
            Pick::Start(w.queue[1].id.clone())
        );
        let mut other = eval_queue(&["eval-1", "other"]);
        other.queue[1].account_id = "second".into();
        assert_eq!(
            pick(&other, 0, 2, false, hold),
            Pick::Start(other.queue[1].id.clone())
        );
        // Today's rules still apply to the entry behind: it waits on the cap.
        assert_eq!(
            pick(&d, 2, 2, false, hold),
            Pick::Wait("2 of 2 slots are in use".into())
        );
        // Only held eval entries: Wait with the first one's reason.
        let only = eval_queue(&["eval-1", "eval-2"]);
        assert_eq!(
            pick(&only, 0, 2, false, hold),
            Pick::Wait("usage is not known".into())
        );
        // A normal head waiting on the cap is never passed over for an eval entry behind it.
        let normal_head = eval_queue(&["normal", "eval-2"]);
        assert_eq!(
            pick(&normal_head, 2, 2, false, |_| None),
            Pick::Wait("2 of 2 slots are in use".into())
        );
        // A normal head with its own hold waits, and nothing behind it starts.
        let held_normal = eval_queue(&["normal", "eval-2"]);
        assert_eq!(
            pick(&held_normal, 0, 2, false, |e| e
                .params
                .eval
                .is_none()
                .then(|| "hold".to_string())),
            Pick::Wait("hold".into())
        );
        // Held eval entries each get their reason.
        let reasons = held_eval_entries(&d, hold);
        assert_eq!(reasons.len(), 2);
    }

    fn eval_source(dir: &crate::test_support::TempDir, account: &str, stage: &str) -> PathBuf {
        let meta = super::super::folder::RunMeta {
            account_id: account.into(),
            account_name: "A".into(),
            bmad_mode: Some("off".into()),
            max_implementers: Some(2),
            ..Default::default()
        };
        let created = super::super::folder::create_run(
            dir.path(),
            Path::new("/ws"),
            &[],
            &["r1".to_string()],
            "Add  multiply()\n\nTickets from ClickUp (1; their details are in tickets.json):\n- t1 (K-1) Title",
            &meta,
        )
        .unwrap();
        let run = PathBuf::from(created.run_dir);
        std::fs::write(
            run.join("status.json"),
            serde_json::json!({"schema":1,"stage":stage,"prs":[
                {"id":"PR1","repoPath":"/ws/r1","base":"main","baseCommit":"abc123"},
                {"id":"PR2","repoPath":"/ws/r2","base":"main","baseCommit":""},
                {"id":"PR3","repoPath":"/ws/r1","base":"dev","baseCommit":"zzz"}]})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            run.join("tickets.json"),
            r#"{"schema":1,"source":{"kind":"clickup","refs":[{"kind":"task","id":"t1"}]},"tasks":[{"id":"t1","custom_id":"K-1","name":"T"}]}"#,
        )
        .unwrap();
        run
    }

    #[test]
    fn eval_start_is_refused_with_the_reason_and_allowed_when_everything_holds() {
        let dir = crate::test_support::TempDir::new("eval_refusal");
        let run = eval_source(&dir, "acct", "done");
        let now = 1_000_000;
        let ok = |n, enabled, account: &str, hold, reading| {
            eval_refusal(&run, n, enabled, account, hold, reading, now)
        };
        assert!(
            ok(3, true, "acct", None, None).is_ok(),
            "no hold set: clear"
        );
        assert!(ok(3, false, "acct", None, None)
            .unwrap_err()
            .contains("Eval mode is off"));
        assert!(ok(1, true, "acct", None, None)
            .unwrap_err()
            .contains("2 to 10"));
        assert!(ok(11, true, "acct", None, None)
            .unwrap_err()
            .contains("2 to 10"));
        assert!(ok(3, true, "other", None, None)
            .unwrap_err()
            .contains("account"));
        let past = ok(3, true, "acct", Some(80), Some((90.0, Some(now + 100)))).unwrap_err();
        assert!(
            past.contains("90%") && past.contains("hold of 80%"),
            "{past}"
        );
        let unknown = ok(3, true, "acct", Some(80), None).unwrap_err();
        assert!(unknown.contains("not known"), "{unknown}");
        assert!(ok(3, true, "acct", Some(80), Some((90.0, Some(now - 5)))).is_ok());

        // The source must be finished, not an eval run and not an epic-plan run.
        let running = eval_source(&dir, "acct", "building");
        assert!(eval_refusal(&running, 3, true, "acct", None, None, now)
            .unwrap_err()
            .contains("finished run"));
        let failed = eval_source(&dir, "acct", "failed");
        assert!(eval_refusal(&failed, 3, true, "acct", None, None, now).is_ok());
        let meta_path = run.join("meta.json");
        let mut meta: Value =
            serde_json::from_str(&std::fs::read_to_string(&meta_path).unwrap()).unwrap();
        meta["mode"] = "epic-plan".into();
        std::fs::write(&meta_path, meta.to_string()).unwrap();
        assert!(ok(3, true, "acct", None, None)
            .unwrap_err()
            .contains("epic-plan"));
        meta.as_object_mut().unwrap().remove("mode");
        meta["eval"] = serde_json::json!({"group":"b","index":1,"of":2,"source":"r-0"});
        std::fs::write(&meta_path, meta.to_string()).unwrap();
        assert!(ok(3, true, "acct", None, None)
            .unwrap_err()
            .contains("eval run"));
    }

    #[test]
    fn the_eval_params_come_from_the_source_run_without_a_second_ticket_block() {
        let dir = crate::test_support::TempDir::new("eval_params");
        let run = eval_source(&dir, "acct", "done");
        let all = eval_params(&run, Some("w1"), "b-9", 3).unwrap();
        assert_eq!(all.len(), 3);
        let p = all[1].clone();
        let p = &p;
        assert_eq!(p.folder, "/ws");
        assert_eq!(
            p.request, "Add  multiply()",
            "words only, not normalised, no ticket block"
        );
        assert!(!p.request.contains("Tickets from ClickUp"));
        assert_eq!(p.review_mode.as_deref(), Some("local"));
        assert_eq!(p.force, Some(true));
        assert_eq!(p.role, "eagle-eye");
        assert_eq!(p.window_id.as_deref(), Some("w1"));
        assert_eq!(p.repos.as_deref(), Some(&["r1".to_string()][..]));
        assert_eq!(p.max_implementers, Some(2));
        assert_eq!(p.bmad_mode.as_deref(), Some("off"));
        let tickets = p.clickup.as_ref().unwrap();
        assert_eq!(tickets.ticket_ids, ["t1"]);
        assert_eq!(tickets.refs.len(), 1);
        let eval = p.eval.as_ref().unwrap();
        assert_eq!((eval.group.as_str(), eval.index, eval.of), ("b-9", 2, 3));
        assert_eq!(eval.source, run.file_name().unwrap().to_str().unwrap());
        let bases = eval.bases.as_ref().unwrap();
        assert_eq!(
            bases.len(),
            1,
            "only a repo with a baseCommit; the first PR of a repo wins"
        );
        assert_eq!(bases["/ws/r1"].base_commit, "abc123");
        assert_eq!(bases["/ws/r1"].base, "main");
        assert_eq!(
            all.iter()
                .map(|p| p.eval.as_ref().unwrap().index)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );

        // The batch and entries: kind eval, in order, each tied to the batch.
        let (batch, entries) = plan_eval_batch(all, "acct", "b-9", "r-src");
        assert_eq!(
            (batch.kind.as_deref(), batch.n, batch.source.as_deref()),
            (Some("eval"), Some(3), Some("r-src"))
        );
        assert_eq!(batch.runs.len(), 3);
        assert_eq!(entries.len(), 3);
        assert!(entries
            .iter()
            .all(|e| e.batch_id.as_deref() == Some("b-9") && e.account_id == "acct"));
        assert_eq!(entries[0].label, "Eval run 1 of 3");
        assert!(is_eval_batch(&batch));

        // The created run folder's meta.json carries the eval ref and review mode local.
        let meta = super::super::folder::RunMeta {
            review_mode: p.review_mode.clone(),
            eval: p.eval.clone(),
            ..Default::default()
        };
        let created = super::super::folder::create_run(
            dir.path(),
            Path::new("/ws"),
            &[],
            &[],
            &p.request,
            &meta,
        )
        .unwrap();
        let written: Value = serde_json::from_str(
            &std::fs::read_to_string(Path::new(&created.run_dir).join("meta.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(written["reviewMode"], "local");
        assert_eq!(written["eval"]["index"], 2);
        assert_eq!(written["eval"]["bases"]["/ws/r1"]["baseCommit"], "abc123");
    }

    #[test]
    fn an_eval_batch_gets_one_eval_finished_and_never_batch_ready() {
        let none: Vec<String> = vec![];
        // Held back by a run still queued, or still working.
        assert_eq!(eval_milestone(&[None, Some("ship-approval")], &none), None);
        assert_eq!(
            eval_milestone(&[Some("building"), Some("ship-approval")], &none),
            None
        );
        assert_eq!(eval_milestone(&[], &none), None);
        // Every run ended (a failed one counts): once.
        assert_eq!(
            eval_milestone(&[Some("ship-approval"), Some("failed")], &none),
            Some("eval-finished")
        );
        assert_eq!(
            eval_milestone(
                &[Some("ship-approval"), Some("ship-approval")],
                &["eval-finished".to_string()]
            ),
            None,
            "once only"
        );
        // The ticket-batch rule would have said batch-ready for the same stages; an eval batch never asks it.
        assert_eq!(
            batch_milestone(&[Some("ship-approval"), Some("ship-approval")], &none),
            Some("batch-ready")
        );
        let (mut batch, _) = plan_batch(&params("x"), &[group("t1"), group("t2")], "a", "b-1");
        assert!(
            !is_eval_batch(&batch),
            "a ticket batch keeps today's behaviour"
        );
        batch.kind = Some("eval".into());
        assert!(is_eval_batch(&batch));
    }

    #[test]
    fn a_batch_whose_kind_was_dropped_is_still_an_eval_batch_by_its_runs_meta() {
        let dir = crate::test_support::TempDir::new("eval_batch_meta");
        let run = dir.join("r-1");
        std::fs::create_dir_all(&run).unwrap();
        let (mut batch, _) = plan_batch(&params("x"), &[group("t1"), group("t2")], "a", "b-1");
        batch.runs[0].run_dir = Some(run.display().to_string());
        assert!(!is_eval_batch(&batch), "no meta.json eval yet");
        std::fs::write(
            run.join("meta.json"),
            r#"{"eval":{"group":"b-1","index":1,"of":2,"source":"r-0"}}"#,
        )
        .unwrap();
        assert!(
            is_eval_batch(&batch),
            "kind missing, but the run is an eval run"
        );
        // A batch written by a newer build keeps its unknown fields through a rewrite.
        let text = r#"{"id":"b","createdAt":"t","runs":[],"kind":"eval","source":"r-0","n":3,"later":{"x":1}}"#;
        let parsed: Batch = serde_json::from_str(text).unwrap();
        assert_eq!((parsed.kind.as_deref(), parsed.n), (Some("eval"), Some(3)));
        let back: Value = serde_json::to_value(&parsed).unwrap();
        assert_eq!(back["later"]["x"], 1);
        assert_eq!(back["source"], "r-0");
    }

    #[test]
    fn an_eval_batch_view_counts_its_finished_runs_and_waits_for_nobody() {
        let (mut batch, _) = plan_batch(&params("x"), &[group("t1"), group("t2")], "a", "b-1");
        assert!(batch_views(&Dispatch {
            batches: vec![batch.clone()],
            ..Dispatch::default()
        })[0]
            .eval
            .is_none());
        batch.kind = Some("eval".into());
        batch.n = Some(2);
        batch.source = Some("r-0".into());
        let view = &batch_views(&Dispatch {
            batches: vec![batch],
            ..Dispatch::default()
        })[0];
        let eval = view.eval.as_ref().expect("an eval group");
        assert_eq!((eval.n, eval.source.as_str(), eval.finished), (2, "r-0", 0));
        assert_eq!(view.waiting_for_you, 0);
    }

    fn member(dir: &str) -> BatchRun {
        BatchRun {
            entry_id: format!("q-{dir}"),
            ticket_ids: vec![],
            label: dir.into(),
            run_dir: Some(format!("/runs/{dir}")),
            removed: false,
        }
    }

    fn fact(dir: &str, finished: bool, checks: bool, worktree: bool) -> EvalCheckFacts {
        EvalCheckFacts {
            run_dir: format!("/runs/{dir}"),
            finished_eval: finished,
            has_app_checks: checks,
            has_worktree: worktree,
        }
    }

    #[test]
    fn the_eval_check_selector_picks_one_finished_unchecked_run_with_a_worktree_not_tried_yet() {
        let (mut eval, _) = plan_batch(&params("x"), &[group("t1")], "a", "b-eval");
        eval.kind = Some("eval".into());
        eval.runs = vec![
            member("e1"),
            member("e2"),
            member("e3"),
            member("e4"),
            member("e5"),
        ];
        eval.runs[4].removed = true;
        let (normal, _) = plan_batch(&params("x"), &[group("t2")], "a", "b-normal");
        let mut normal = normal;
        normal.runs = vec![member("n1")];
        let batches = vec![normal, eval];
        let facts = vec![
            fact("n1", true, false, true),  // a normal batch's run: never
            fact("e1", true, true, true),   // already has app-checks.json
            fact("e2", false, false, true), // not finished
            fact("e3", true, false, false), // no worktree left
            fact("e4", true, false, true),  // the one
            fact("e5", true, false, true),  // removed from the batch
        ];
        let mut tried = HashSet::new();
        assert_eq!(
            eval_run_to_check(&batches, &facts, &tried).as_deref(),
            Some("/runs/e4")
        );
        // Tried in this app session: not again, and nothing else qualifies.
        tried.insert("/runs/e4".to_string());
        assert_eq!(eval_run_to_check(&batches, &facts, &tried), None);
        // At most one at a time: two candidates give the first only.
        let two = vec![fact("e4", true, false, true), fact("e2", true, false, true)];
        assert_eq!(
            eval_run_to_check(&batches, &two, &HashSet::new()).as_deref(),
            Some("/runs/e2"),
            "the batch's run order decides: e2 comes before e4"
        );
        assert!(eval_run_to_check(&[], &facts, &HashSet::new()).is_none());
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
