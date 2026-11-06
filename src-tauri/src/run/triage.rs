//! Triage runs (docs/run-folder.md, "Triage mode"; AD-13 to AD-17): a planning-only run proposes the next runs from the
//! open tickets of one ClickUp list. This module is the app's side of it:
//!
//! - **start and schedule**: the snapshot (open tickets only, newest first, at most 100), the refusal while the list's
//!   newest triage run is unfinished, and the dispatcher's once-per-local-day schedule;
//! - **approval**: only [`approve`] writes the signed record `triage-approval.json`; a file of that name in the run
//!   folder is never read;
//! - **apply**: the ClickUp writes (`create_task`, closing a ticket) are sent from that record only, once each.

use super::dispatch;
use super::folder;
use super::{ClickUpStart, StartParams};
use crate::clickup::client::ClickUp;
use crate::clickup::client::TaskRef;
use crate::clickup::intake::IntakeRef;
use crate::clickup::trust::{self, Key, TRIAGE_APPROVAL_FILE, TRIAGE_POSTED_FILE};
use crate::clickup::types::Task;
use crate::clickup::ClickUpError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tauri::AppHandle;

/// A snapshot holds at most this many open tickets.
pub const MAX_TICKETS: usize = 100;
/// A triage run whose status.json has not changed for this long no longer blocks its list.
pub const UNFINISHED_FOR: Duration = Duration::from_secs(6 * 60 * 60);
/// A suggested ticket's name and description are capped at these lengths before they are shown, signed or sent.
pub const NAME_CAP: usize = 200;
pub const DESCRIPTION_CAP: usize = 10_000;

/// The signing key: the keychain's in the app, a fixed in-memory one under test (the keychain is never touched).
fn signing_key() -> Result<Key, String> {
    #[cfg(test)]
    {
        Ok(trust::tests::key())
    }
    #[cfg(not(test))]
    {
        trust::install_key()
    }
}

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

fn cap(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

// ---- the snapshot ----------------------------------------------------------------------------------------------

/// The tickets a triage looks at: not closed or done, newest `date_updated` first, at most [`MAX_TICKETS`].
pub fn open_tasks(tasks: Vec<Task>) -> Vec<Task> {
    let mut open: Vec<Task> = tasks
        .into_iter()
        .filter(|t| !t.status.is_closed())
        .collect();
    let updated = |t: &Task| {
        t.date_updated
            .as_deref()
            .and_then(|d| d.parse::<i64>().ok())
            .unwrap_or(0)
    };
    open.sort_by_key(|t| std::cmp::Reverse(updated(t)));
    open.truncate(MAX_TICKETS);
    open
}

fn refs_of(tasks: &[Task]) -> Vec<IntakeRef> {
    tasks
        .iter()
        .map(|t| IntakeRef {
            kind: "task".into(),
            id: t.id.clone(),
            team_id: None,
            custom: false,
            line: 0,
        })
        .collect()
}

// ---- triage runs on disk ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct TriageRun {
    pub run_dir: PathBuf,
    pub run_id: String,
    pub list_id: Option<String>,
    pub created_at: Option<String>,
    pub stage: Option<String>,
    /// When status.json last changed.
    pub updated: Option<SystemTime>,
}

/// The app's own triage runs, newest first.
pub fn triage_runs(data: &Path) -> Vec<TriageRun> {
    folder::list_runs(data)
        .into_iter()
        .filter(|r| !r.adopted)
        .filter_map(|r| {
            let dir = PathBuf::from(&r.run_dir);
            (folder::run_mode_of(&dir).as_deref() == Some("triage")).then(|| TriageRun {
                list_id: folder::triage_list_of(&dir),
                created_at: r.created_at.clone(),
                stage: r.stage.clone(),
                updated: std::fs::metadata(dir.join("status.json"))
                    .and_then(|m| m.modified())
                    .ok(),
                run_id: r.run_id.clone(),
                run_dir: dir,
            })
        })
        .collect()
}

/// A run is unfinished while its stage is not done or failed and its status.json changed within [`UNFINISHED_FOR`]:
/// a run whose orchestrator died stops blocking.
pub fn unfinished(run: &TriageRun, now: SystemTime) -> bool {
    if matches!(run.stage.as_deref(), Some("done" | "failed")) {
        return false;
    }
    run.updated
        .and_then(|at| now.duration_since(at).ok())
        .is_none_or(|age| age < UNFINISHED_FOR)
}

/// Why a triage of `list_id` cannot start now: its newest triage run is unfinished, or one is already queued.
pub fn blocking_reason(data: &Path, list_id: &str, now: SystemTime) -> Option<String> {
    if let Some(entry) = dispatch::load(data).queue.iter().find(|e| {
        e.params.run_mode.as_deref() == Some("triage")
            && e.params.triage_list_id.as_deref() == Some(list_id)
    }) {
        return Some(format!(
            "a triage of this list is already queued ({})",
            entry.label
        ));
    }
    let newest = triage_runs(data)
        .into_iter()
        .find(|r| r.list_id.as_deref() == Some(list_id))?;
    unfinished(&newest, now).then(|| {
        format!(
            "the triage run {} of this list is not finished yet",
            newest.run_id
        )
    })
}

/// Queues a triage of `tasks` (the list's tasks, as read from ClickUp) through the dispatcher: the cap applies and the
/// start goes through the shared start path with `run_mode: "triage"` and BMAD off. Returns the queue entry's id.
pub fn enqueue_triage(
    data: &Path,
    account_id: &str,
    folder: &str,
    list_id: &str,
    list_name: &str,
    tasks: Vec<Task>,
    now: SystemTime,
) -> Result<String, String> {
    if let Some(why) = blocking_reason(data, list_id, now) {
        return Err(format!("No triage started: {why}."));
    }
    let open = open_tasks(tasks);
    if open.is_empty() {
        return Err("This list has no open tickets to triage.".into());
    }
    let ids: Vec<String> = open.iter().map(|t| t.id.clone()).collect();
    let params = StartParams {
        window_id: None,
        folder: folder.to_string(),
        request: format!(
            "Triage the {} open tickets of ClickUp list {list_name}: propose the next runs.",
            ids.len()
        ),
        role: "eagle-eye".into(),
        clickup: Some(ClickUpStart {
            refs: refs_of(&open),
            ticket_ids: ids,
        }),
        review_mode: Some("local".into()),
        bmad_mode: Some("off".into()),
        force: Some(true),
        repos: None,
        max_implementers: None,
        run_mode: Some("triage".into()),
        dispatch_entry_id: None,
        eval: None,
        triage_list_id: Some(list_id.to_string()),
    };
    let label = format!("Triage: {list_name}");
    dispatch::with(data, |d| {
        dispatch::enqueue(d, params, account_id, &label, None)
    })
}

// ---- the schedule ----------------------------------------------------------------------------------------------

/// Whether a scheduled triage should start now: the row is on, the local time has passed, the list's newest triage run
/// is not unfinished, did not start today (local date, from its `createdAt`, not from memory), and a finished
/// proposal has been answered (an approval record exists). A failed run is no proposal and does not block.
#[allow(clippy::too_many_arguments)]
pub fn schedule_due(
    entry: &crate::store::settings_store::TriageSchedule,
    now_local: chrono::NaiveDateTime,
    now: SystemTime,
    newest: Option<&TriageRun>,
    local_date_of: &dyn Fn(&str) -> Option<chrono::NaiveDate>,
    has_approval: &dyn Fn(&Path) -> bool,
    queued: bool,
) -> bool {
    use chrono::Timelike;
    let Some(minutes) = crate::store::settings_store::parse_hhmm(&entry.time) else {
        return false;
    };
    if !entry.on || entry.folder.is_empty() || queued {
        return false;
    }
    if now_local.time().hour() * 60 + now_local.time().minute() < minutes {
        return false;
    }
    let Some(run) = newest else {
        return true;
    };
    if unfinished(run, now) {
        return false;
    }
    if run
        .created_at
        .as_deref()
        .and_then(local_date_of)
        .is_some_and(|day| day == now_local.date())
    {
        return false;
    }
    run.stage.as_deref() != Some("done") || has_approval(&run.run_dir)
}

fn local_date(created_at: &str) -> Option<chrono::NaiveDate> {
    chrono::DateTime::parse_from_rfc3339(created_at)
        .ok()
        .map(|t| t.with_timezone(&chrono::Local).date_naive())
}

static SCHEDULE_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How long a failed scheduled start waits before it is tried again.
const START_RETRY: Duration = Duration::from_secs(30 * 60);

/// Last failed scheduled start per list, so a failure is not retried on every 2 s tick.
static FAILED_STARTS: std::sync::Mutex<BTreeMap<String, SystemTime>> =
    std::sync::Mutex::new(BTreeMap::new());

/// True when the list's last scheduled start failed less than `START_RETRY` ago.
fn start_backing_off(
    failed: &BTreeMap<String, SystemTime>,
    list_id: &str,
    now: SystemTime,
) -> bool {
    failed.get(list_id).is_some_and(|at| {
        now.duration_since(*at)
            .map(|d| d < START_RETRY)
            .unwrap_or(true)
    })
}

/// The dispatcher tick's part: starts at most one due scheduled triage, on its own thread (a ClickUp read).
pub fn schedule_tick(app: &AppHandle) {
    use std::sync::atomic::Ordering;
    use tauri::Manager;
    let entries = app
        .try_state::<std::sync::Mutex<crate::store::SettingsStore>>()
        .and_then(|s| s.lock().ok().map(|s| s.run().triage_schedule.clone()))
        .unwrap_or_default();
    if entries.is_empty() || SCHEDULE_BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    let data = crate::data_dir::app_data_dir(app).path;
    let now = SystemTime::now();
    let now_local = chrono::Local::now().naive_local();
    let Ok(key) = signing_key() else {
        SCHEDULE_BUSY.store(false, Ordering::SeqCst);
        return;
    };
    let runs = triage_runs(&data);
    let failed = FAILED_STARTS.lock().map(|f| f.clone()).unwrap_or_default();
    let due = entries.into_iter().find(|entry| {
        if start_backing_off(&failed, &entry.list_id, now) {
            return false;
        }
        let newest = runs
            .iter()
            .find(|r| r.list_id.as_deref() == Some(entry.list_id.as_str()));
        let queued =
            blocking_reason(&data, &entry.list_id, now).is_some_and(|why| why.contains("queued"));
        schedule_due(
            entry,
            now_local,
            now,
            newest,
            &local_date,
            &|dir| read_approval(&key, &data, dir).ok().flatten().is_some(),
            queued,
        )
    });
    let Some(entry) = due else {
        SCHEDULE_BUSY.store(false, Ordering::SeqCst);
        return;
    };
    std::thread::spawn(move || {
        let result = start_for(&data, &entry.account_id, &entry.folder, &entry.list_id);
        if let Ok(mut failed) = FAILED_STARTS.lock() {
            match &result {
                Ok(_) => {
                    failed.remove(&entry.list_id);
                }
                Err(why) => {
                    // Once per backoff window: the next try is 30 minutes away.
                    eprintln!(
                        "scheduled triage for list {} did not start: {why}",
                        entry.list_id
                    );
                    failed.insert(entry.list_id.clone(), SystemTime::now());
                }
            }
        }
        SCHEDULE_BUSY.store(false, Ordering::SeqCst);
    });
}

/// Reads the list's tasks with the account's token and queues the triage.
fn start_for(data: &Path, account_id: &str, folder: &str, list_id: &str) -> Result<String, String> {
    if let Some(why) = blocking_reason(data, list_id, SystemTime::now()) {
        return Err(format!("No triage started: {why}."));
    }
    let token = crate::clickup::commands::token_of(&crate::clickup::KeychainStore, account_id)?;
    let transport = crate::clickup::UreqTransport::new();
    let client = ClickUp::new(&transport, &token);
    let list_name = client
        .list(list_id)
        .map(|l| l.name)
        .unwrap_or_else(|_| list_id.to_string());
    let tasks = client.list_tasks(list_id).map_err(|e| e.to_string())?;
    enqueue_triage(
        data,
        account_id,
        folder,
        list_id,
        &list_name,
        tasks,
        SystemTime::now(),
    )
}

/// "Triage open tickets…": queues a triage of `list_id` for the window's account, starting in `folder`.
#[tauri::command]
pub async fn triage_start(
    app: AppHandle,
    window_id: Option<String>,
    list_id: String,
    folder: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let data = crate::data_dir::app_data_dir(&app).path;
        let account = dispatch::account_of(&app, window_id.as_deref())?;
        start_for(&data, &account, folder.trim(), list_id.trim())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Why a triage of `list_id` cannot start now (the dialog's disabled Start), or null.
#[tauri::command]
pub async fn triage_blocking(app: AppHandle, list_id: String) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let data = crate::data_dir::app_data_dir(&app).path;
        blocking_reason(&data, list_id.trim(), SystemTime::now())
    })
    .await
    .map_err(|e| e.to_string())
}

// ---- the approval record ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupRec {
    pub n: u32,
    pub title: String,
    pub tickets: Vec<String>,
    pub depends_on: Vec<u32>,
    #[serde(default)]
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRec {
    pub index: usize,
    pub name: String,
    pub description: String,
}

/// The signed record of the operator's decision (`run-trust/<runId>/triage-approval.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRecord {
    pub schema: u32,
    pub triage_digest: String,
    /// `approved` or `rejected`.
    pub decision: String,
    pub groups: Vec<GroupRec>,
    pub creates: Vec<CreateRec>,
    pub closes: Vec<String>,
    pub at: String,
}

/// What the operator edited in the proposal: the groups as kept (in order, numbered by position), the suggested
/// tickets ticked (by index) and the tickets ticked to close.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edits {
    #[serde(default)]
    pub groups: Vec<EditGroup>,
    #[serde(default)]
    pub creates: Vec<usize>,
    #[serde(default)]
    pub closes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditGroup {
    pub title: String,
    pub tickets: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<u32>,
    #[serde(default)]
    pub why: String,
}

pub const STALE: &str = "The proposal changed. Review it again.";

/// The approval of the run's signed record, if any (a tampered record is an error).
pub fn read_approval(
    key: &Key,
    data: &Path,
    run_dir: &Path,
) -> Result<Option<ApprovalRecord>, String> {
    trust::read_triage(key, data, run_dir, TRIAGE_APPROVAL_FILE)
}

/// Checks the operator's edits against the proposal and the run's snapshot and builds the record. Refuses a stale
/// digest, an empty group, a ticket in two groups or outside the snapshot, a forward dependency, a create that is not
/// one of the suggestions and a close that is not one of the proposed closes.
pub fn build_approval(
    triage: &Value,
    current_digest: &str,
    requested_digest: &str,
    decision: &str,
    edits: &Edits,
    ticket_ids: &HashSet<String>,
    at: &str,
) -> Result<ApprovalRecord, String> {
    if requested_digest != current_digest {
        return Err(STALE.into());
    }
    if !matches!(decision, "approved" | "rejected") {
        return Err("The decision is approved or rejected.".into());
    }
    let mut record = ApprovalRecord {
        schema: 1,
        triage_digest: current_digest.to_string(),
        decision: decision.to_string(),
        groups: vec![],
        creates: vec![],
        closes: vec![],
        at: at.to_string(),
    };
    if decision == "rejected" {
        return Ok(record);
    }
    // The groups, through the same check the epic dispatcher applies to a waves file.
    let waves: Vec<Value> = edits
        .groups
        .iter()
        .enumerate()
        .map(|(at, g)| {
            json!({"n": at + 1, "title": g.title, "tickets": g.tickets, "dependsOn": g.depends_on, "why": g.why})
        })
        .collect();
    let mut seen = HashSet::new();
    for g in &edits.groups {
        for t in &g.tickets {
            if !ticket_ids.contains(t) {
                return Err(format!("{t} is not a ticket of this proposal's snapshot."));
            }
            if !seen.insert(t.clone()) {
                return Err(format!("{t} is in two groups."));
            }
        }
    }
    if !waves.is_empty() {
        let specs = super::epics::validate_waves(&json!({ "waves": waves }), ticket_ids)?;
        record.groups = specs
            .into_iter()
            .zip(&edits.groups)
            .map(|(s, g)| GroupRec {
                n: s.n,
                title: cap(g.title.trim(), NAME_CAP),
                tickets: s.tickets,
                depends_on: s.depends_on,
                why: cap(&g.why, 1000),
            })
            .collect();
    }
    let suggestions = triage["suggestions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut creates = BTreeMap::new();
    for index in &edits.creates {
        let s = suggestions
            .get(*index)
            .ok_or_else(|| format!("There is no suggested ticket {index}."))?;
        let name = cap(s["title"].as_str().unwrap_or("").trim(), NAME_CAP);
        if name.is_empty() {
            return Err(format!("Suggested ticket {index} has no title."));
        }
        creates.insert(
            *index,
            CreateRec {
                index: *index,
                name,
                description: cap(s["description"].as_str().unwrap_or(""), DESCRIPTION_CAP),
            },
        );
    }
    record.creates = creates.into_values().collect();
    let proposed: HashSet<String> = triage["closes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["ticket"].as_str().map(str::to_string))
        .collect();
    for id in &edits.closes {
        if !proposed.contains(id) || !ticket_ids.contains(id) {
            return Err(format!("{id} is not a ticket this proposal closes."));
        }
        if !record.closes.contains(id) {
            record.closes.push(id.clone());
        }
    }
    Ok(record)
}

fn ticket_ids_of(run_dir: &Path) -> HashSet<String> {
    read_json(&run_dir.join("tickets.json"))
        .and_then(|v| v.get("tasks").cloned().or(Some(v)))
        .and_then(|t| t.as_array().cloned())
        .map(|a| {
            a.iter()
                .filter_map(|t| t["id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// The sha-256 of triage.md, hex (the digest `state.sh triage` records).
fn digest_of_markdown(run_dir: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let text = std::fs::read_to_string(run_dir.join("triage.md")).ok()?;
    Some(
        Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

/// The proposal of a finished triage run: triage.json, only when its digest is the current triage.md's.
fn current_proposal(run_dir: &Path) -> Result<(Value, String), String> {
    let triage =
        read_json(&run_dir.join("triage.json")).ok_or("This run has no triage proposal.")?;
    let digest = triage["digest"].as_str().unwrap_or("").to_string();
    if digest.is_empty() || digest_of_markdown(run_dir).as_deref() != Some(digest.as_str()) {
        return Err(STALE.into());
    }
    Ok((triage, digest))
}

/// Approves or rejects the proposal of a finished triage run of this app: refuses a stale digest, a bad edit and a
/// second decision. Nothing is queued or sent before this record exists.
pub fn approve(
    key: &Key,
    data: &Path,
    run_dir: &Path,
    triage_digest: &str,
    decision: &str,
    edits: &Edits,
    at: &str,
) -> Result<ApprovalRecord, String> {
    if folder::run_mode_of(run_dir).as_deref() != Some("triage") {
        return Err("Not a triage run.".into());
    }
    if read_json(&run_dir.join("status.json")).is_none_or(|s| s["stage"] != "done") {
        return Err("The triage run has not finished.".into());
    }
    let (triage, digest) = current_proposal(run_dir)?;
    if read_approval(key, data, run_dir)?.is_some() {
        return Err("This proposal was already decided.".into());
    }
    let record = build_approval(
        &triage,
        &digest,
        triage_digest,
        decision,
        edits,
        &ticket_ids_of(run_dir),
        at,
    )?;
    trust::write_triage(key, data, run_dir, TRIAGE_APPROVAL_FILE, &record)?;
    let _ = folder::append_app_log(
        run_dir,
        &format!(
            "triage {}: {} group(s), {} to create, {} to close",
            record.decision,
            record.groups.len(),
            record.creates.len(),
            record.closes.len()
        ),
    );
    Ok(record)
}

/// The wave specs of an approved triage run, as a `waves.json` document, for the epic dispatcher (AD-15): from the
/// signed record only, never from a file in the run folder. `None` when the run is not an approved triage run.
pub fn approved_waves(run_dir: &Path) -> Option<Value> {
    let data = run_dir.parent()?.parent()?;
    let key = signing_key().ok()?;
    let record = read_approval(&key, data, run_dir).ok().flatten()?;
    if record.decision != "approved" || record.groups.is_empty() {
        return None;
    }
    Some(
        json!({"schema": 1, "waves": record.groups.iter().map(|g| json!({
        "n": g.n, "title": g.title, "tickets": g.tickets, "dependsOn": g.depends_on, "why": g.why,
    })).collect::<Vec<_>>()}),
    )
}

/// The page's view of one finished triage run.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TriageView {
    pub run_dir: String,
    pub run_id: String,
    pub list_id: Option<String>,
    /// triage.json (groups, needs-you, suggestions, closes, out of scope), with the ticket names added.
    pub proposal: Value,
    pub digest: String,
    /// The decision record, once there is one.
    pub approval: Option<ApprovalRecord>,
    /// The ClickUp writes after approval.
    pub posted: Option<PostedRecord>,
    /// Why the proposal cannot be shown or decided.
    pub problem: Option<String>,
}

fn names_of(run_dir: &Path) -> BTreeMap<String, String> {
    read_json(&run_dir.join("tickets.json"))
        .and_then(|v| v.get("tasks").cloned().or(Some(v)))
        .and_then(|t| t.as_array().cloned())
        .map(|a| {
            a.iter()
                .filter_map(|t| {
                    Some((
                        t["id"].as_str()?.to_string(),
                        cap(t["name"].as_str().unwrap_or(""), NAME_CAP),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The finished triage runs with a proposal, newest first (at most 10), each with its decision and ClickUp writes.
pub fn overview(data: &Path) -> Vec<TriageView> {
    let key = signing_key();
    triage_runs(data)
        .into_iter()
        .filter(|r| r.stage.as_deref() == Some("done"))
        .take(10)
        .map(|r| {
            let dir = r.run_dir.clone();
            let mut view = TriageView {
                run_dir: dir.display().to_string(),
                run_id: r.run_id.clone(),
                list_id: r.list_id.clone(),
                proposal: Value::Null,
                digest: String::new(),
                approval: None,
                posted: None,
                problem: None,
            };
            match current_proposal(&dir) {
                Ok((mut proposal, digest)) => {
                    proposal["names"] = json!(names_of(&dir));
                    view.proposal = proposal;
                    view.digest = digest;
                }
                Err(why) => view.problem = Some(why),
            }
            match &key {
                Ok(key) => {
                    match read_approval(key, data, &dir) {
                        Ok(a) => view.approval = a,
                        Err(why) => view.problem = Some(why),
                    }
                    if let Ok(p) = read_posted(key, data, &dir) {
                        view.posted = p;
                    }
                }
                Err(why) => view.problem = Some(why.clone()),
            }
            view
        })
        .collect()
}

#[tauri::command]
pub async fn triage_overview(app: AppHandle) -> Result<Vec<TriageView>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        overview(&crate::data_dir::app_data_dir(&app).path)
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn triage_approve(
    app: AppHandle,
    run_dir: String,
    triage_digest: String,
    decision: String,
    edits: Option<Edits>,
) -> Result<ApprovalRecord, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let data = crate::data_dir::app_data_dir(&app).path;
        let dir = super::checked_run_dir(&data, &run_dir)?;
        let key = signing_key()?;
        approve(
            &key,
            &data,
            &dir,
            &triage_digest,
            &decision,
            &edits.unwrap_or_default(),
            &chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---- the ClickUp writes (AD-16) --------------------------------------------------------------------------------

/// One write's result: `sending` (saved before the call), then `posted`, `failed` (may be retried), `maybe` (ClickUp
/// may have done it; never resent) or `blocked` (the text looks like a secret; never sent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostItem {
    pub result: String,
    pub at: String,
    #[serde(default)]
    pub message: Option<String>,
}

/// `triage-posted.json`: one entry per write, keyed `create:<index>` or `close:<ticket id>`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostedRecord {
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub items: BTreeMap<String, PostItem>,
}

pub fn read_posted(key: &Key, data: &Path, run_dir: &Path) -> Result<Option<PostedRecord>, String> {
    trust::read_triage(key, data, run_dir, TRIAGE_POSTED_FILE)
}

/// Where each step of [`apply`] is saved (a signed record in the app; a vector under test).
pub trait PostStore {
    fn save(&mut self, posted: &PostedRecord) -> Result<(), String>;
}

struct TrustStore<'a> {
    key: &'a Key,
    data: &'a Path,
    run_dir: &'a Path,
}

impl PostStore for TrustStore<'_> {
    fn save(&mut self, posted: &PostedRecord) -> Result<(), String> {
        trust::write_triage(
            self.key,
            self.data,
            self.run_dir,
            TRIAGE_POSTED_FILE,
            posted,
        )
    }
}

/// One line per item that was sent, retried or skipped, for the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyOutcome {
    pub item: String,
    pub result: String,
    pub message: String,
}

fn may_have_done_it(err: &ClickUpError) -> bool {
    use crate::clickup::transport::NoReply;
    matches!(
        err,
        ClickUpError::NoReply(NoReply::Timeout | NoReply::Failed)
            | ClickUpError::Server(_)
            | ClickUpError::Unexpected(_)
    )
}

/// Sends what the approved `record` names, once each, from the record only (never from triage.md or triage.json).
/// A `create` goes to `list_id` (name and description capped and secret-scanned); a `close` moves a ticket of the
/// snapshot to the list's first closed-type status. An item already `posted`, `sending`, `maybe` or `blocked` is
/// not sent again; a `failed` one is retried. Each step is saved before and after the call.
pub fn apply(
    client: &ClickUp,
    list_id: &str,
    record: &ApprovalRecord,
    snapshot_ids: &HashSet<String>,
    posted: &mut PostedRecord,
    store: &mut dyn PostStore,
    now: &dyn Fn() -> String,
) -> Result<Vec<ApplyOutcome>, (Vec<ApplyOutcome>, String)> {
    let mut out: Vec<ApplyOutcome> = Vec::new();
    if record.decision != "approved" {
        return Ok(out);
    }
    posted.schema = 1;
    let mut closed_status: Option<Result<String, String>> = None;
    macro_rules! save {
        () => {
            if let Err(err) = store.save(posted) {
                return Err((std::mem::take(&mut out), err));
            }
        };
    }
    let already = |posted: &PostedRecord, key: &str| {
        posted.items.get(key).is_some_and(|p| {
            matches!(
                p.result.as_str(),
                "posted" | "sending" | "maybe" | "blocked"
            )
        })
    };
    for create in &record.creates {
        let key = format!("create:{}", create.index);
        if already(posted, &key) {
            continue;
        }
        let name = cap(&create.name, NAME_CAP);
        let description = cap(&create.description, DESCRIPTION_CAP);
        if let Some(kind) =
            crate::clickup::secret_scan::first_hit(&format!("{name}\n{description}"))
        {
            let message = format!("Not sent: the text looks like it contains {kind}.");
            posted.items.insert(
                key.clone(),
                PostItem {
                    result: "blocked".into(),
                    at: now(),
                    message: Some(message.clone()),
                },
            );
            save!();
            out.push(ApplyOutcome {
                item: key,
                result: "blocked".into(),
                message,
            });
            continue;
        }
        posted.items.insert(
            key.clone(),
            PostItem {
                result: "sending".into(),
                at: now(),
                message: None,
            },
        );
        save!();
        let (result, message) = match client.create_task(list_id, &name, &description) {
            Ok(()) => ("posted", "created".to_string()),
            Err(err) if may_have_done_it(&err) => (
                "maybe",
                format!(
                    "{err} ClickUp may have created it: check the list before creating it again."
                ),
            ),
            Err(err) => ("failed", err.to_string()),
        };
        posted.items.insert(
            key.clone(),
            PostItem {
                result: result.into(),
                at: now(),
                message: Some(message.clone()),
            },
        );
        save!();
        out.push(ApplyOutcome {
            item: key,
            result: result.into(),
            message,
        });
    }
    for id in &record.closes {
        let key = format!("close:{id}");
        if already(posted, &key) {
            continue;
        }
        if !snapshot_ids.contains(id) || !crate::clickup::valid_id(id) {
            continue;
        }
        let status = closed_status.get_or_insert_with(|| {
            client
                .list(list_id)
                .map_err(|e| e.to_string())
                .and_then(|l| {
                    l.statuses
                        .iter()
                        .find(|s| s.kind == "closed")
                        .or_else(|| l.statuses.iter().find(|s| s.is_closed()))
                        .map(|s| s.status.clone())
                        .ok_or_else(|| "The list has no closed status.".to_string())
                })
        });
        let status = match status {
            Ok(s) => s.clone(),
            Err(message) => {
                posted.items.insert(
                    key.clone(),
                    PostItem {
                        result: "failed".into(),
                        at: now(),
                        message: Some(message.clone()),
                    },
                );
                save!();
                out.push(ApplyOutcome {
                    item: key,
                    result: "failed".into(),
                    message: message.clone(),
                });
                continue;
            }
        };
        posted.items.insert(
            key.clone(),
            PostItem {
                result: "sending".into(),
                at: now(),
                message: None,
            },
        );
        save!();
        let task = TaskRef::Id(id.clone());
        let (result, message) = match client.set_status(&task, &status) {
            Ok(()) => ("posted", format!("moved to {status}")),
            Err(err) if may_have_done_it(&err) => (
                "maybe",
                format!("{err} ClickUp may have changed it: check the ticket."),
            ),
            Err(err) => ("failed", err.to_string()),
        };
        posted.items.insert(
            key.clone(),
            PostItem {
                result: result.into(),
                at: now(),
                message: Some(message.clone()),
            },
        );
        save!();
        out.push(ApplyOutcome {
            item: key,
            result: result.into(),
            message,
        });
    }
    Ok(out)
}

/// "Apply" (and Retry for the failed ones): the approved record's ClickUp writes, under the run's post lock.
#[tauri::command]
pub async fn triage_apply(app: AppHandle, run_dir: String) -> Result<Vec<ApplyOutcome>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let data = crate::data_dir::app_data_dir(&app).path;
        let dir = super::checked_run_dir(&data, &run_dir)?;
        let key = signing_key()?;
        let record = read_approval(&key, &data, &dir)?.ok_or("This proposal is not approved.")?;
        let list_id = folder::triage_list_of(&dir).ok_or("This triage run records no list.")?;
        let trusted = trust::load(&key, &data, &dir)?.ok_or(trust::TAMPERED)?;
        let snapshot_ids: HashSet<String> = trusted
            .snapshot
            .tasks
            .iter()
            .map(|t| t.id.clone())
            .collect();
        let _lock = trust::lock_posts(&data, &dir)?;
        let token = crate::clickup::commands::token_of(
            &crate::clickup::KeychainStore,
            &trusted.account_id,
        )?;
        let transport = crate::clickup::UreqTransport::new();
        let client = ClickUp::new(&transport, &token);
        let mut posted = read_posted(&key, &data, &dir)?.unwrap_or_default();
        let mut store = TrustStore {
            key: &key,
            data: &data,
            run_dir: &dir,
        };
        let now = || chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        match apply(
            &client,
            &list_id,
            &record,
            &snapshot_ids,
            &mut posted,
            &mut store,
            &now,
        ) {
            Ok(out) => Ok(out),
            Err((_, err)) => Err(err),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests;
