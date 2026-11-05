//! Check re-runs (AF13, "Check replay"): the app runs a PR's recorded app check again at the commit it ran on, in a
//! detached scratch worktree the app makes and removes, and keeps the result beside the recorded one.
//!
//! What runs is only the app-written check command (`app-checks.json`, guard-protected) and the setup that
//! `repos.json` or the lockfile table gives, as `plugin/scripts/state.sh` does for a new worktree. Nothing a
//! transcript recorded is ever run. The contract worktree is never the working folder and is not even needed: its
//! path is only text that the recorded check folder is made relative to (`run/cleanup.rs` removes the worktrees of
//! merged runs). Records live in app data (`<app data>/flight/replays/<runId>/<PR>-<n>.json` and `.log`), never in the
//! run folder. One re-run runs at a time in the whole app.

use super::check::{check_folder, run_one_stoppable, CHECK_TIMEOUT};
use super::git::{canonical, git};
use super::janitor;
use super::status::{read_app_checks, valid_pr_id};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a setup step may run.
pub const SETUP_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Records (and logs) kept per run and PR.
pub const KEEP_PER_PR: usize = 5;
/// The refusal for a second request while one runs.
pub const ALREADY_RUNNING: &str = "a re-run is already running";

/// `state.sh` `detect_setup`: the first lockfile found wins.
pub const LOCKFILES: &[(&str, &str)] = &[
    ("package-lock.json", "npm ci"),
    ("pnpm-lock.yaml", "pnpm install --frozen-lockfile"),
    ("yarn.lock", "yarn install --frozen-lockfile"),
    ("go.mod", "go mod download"),
];

/// What the recorded check said (from `app-checks.json`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Recorded {
    pub result: String,
    pub exit_code: Option<i32>,
    pub finished_at: Option<String>,
    pub clean: Option<bool>,
}

/// One re-run (`record-formats.md`, "Replay records").
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RerunRecord {
    pub schema: u32,
    pub run_id: String,
    pub pr: String,
    pub n: u32,
    pub repo: String,
    pub worktree: String,
    pub commit: String,
    pub command: String,
    pub setup: Option<String>,
    /// The recorded check folder relative to the PR's worktree; empty for its root.
    pub cwd: String,
    pub recorded: Recorded,
    /// `preparing`, `setup`, `running`, `pass`, `fail`, `timeout`, `error`, `stopped`, `interrupted`.
    pub state: String,
    pub exit_code: Option<i32>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub detail: Option<String>,
    pub log: String,
    /// Removing the scratch worktree failed; it is retried at the next start.
    #[serde(default, skip_serializing_if = "is_false")]
    pub scratch_left: bool,
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn in_progress(state: &str) -> bool {
    matches!(state, "preparing" | "setup" | "running")
}

/// `<app data>/flight/replays/<runId>/`.
pub fn replays_dir(data: &Path, run_id: &str) -> PathBuf {
    data.join("flight").join("replays").join(run_id)
}

fn run_id_of(run_dir: &Path) -> String {
    run_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

// ---- the setup rule (state.sh) ----

/// The setup a new worktree gets, as `state.sh worktree` decides it: the repos.json entry's `setup` string when it
/// has one (stripped; `""` means none), else, only for a run whose meta.json has `reviewMode` (older runs get no
/// detected install), the first lockfile of [`LOCKFILES`] that `has` finds at the worktree root.
pub fn setup_command(run_dir: &Path, repo: &Path, has: impl Fn(&str) -> bool) -> Option<String> {
    let repos = super::folder::read_json_value(&run_dir.join("repos.json"))
        .and_then(|v| v.get("repos").and_then(|r| r.as_array().cloned()))
        .unwrap_or_default();
    let want = canonical(repo);
    let entry = repos.iter().find(|r| {
        r.get("path")
            .and_then(|p| p.as_str())
            .is_some_and(|p| canonical(Path::new(p)) == want)
    });
    if let Some(text) = entry.and_then(|e| e.get("setup")).and_then(|s| s.as_str()) {
        let text = text.trim();
        return (!text.is_empty()).then(|| text.to_string());
    }
    let new_run = super::folder::read_json_value(&run_dir.join("meta.json"))
        .and_then(|m| m.as_object().map(|o| o.contains_key("reviewMode")))
        .unwrap_or(false);
    if !new_run {
        return None;
    }
    LOCKFILES
        .iter()
        .find(|(file, _)| has(file))
        .map(|(_, cmd)| cmd.to_string())
}

// ---- what a re-run would do ----

/// A re-run's inputs, all from app-written records and git.
#[derive(Debug, Clone)]
pub struct Plan {
    pub run_id: String,
    pub pr: String,
    pub repo: PathBuf,
    pub commit: String,
    pub command: String,
    pub cwd: String,
    pub recorded: Recorded,
}

/// What the confirmation shows, or why there is none.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    /// Why a re-run cannot start (shown as the disabled button's caption).
    pub reason: Option<String>,
    pub command: Option<String>,
    pub setup: Option<String>,
    pub cwd: String,
    pub commit: Option<String>,
    /// The recorded check ran with no uncommitted changes (`false`: it ran on a dirty tree).
    pub clean: Option<bool>,
    pub recorded: Option<Recorded>,
}

fn pr_value(run_dir: &Path, pr_id: &str) -> Option<serde_json::Value> {
    super::folder::read_json_value(&run_dir.join("status.json"))?
        .get("prs")?
        .as_array()?
        .iter()
        .find(|p| p.get("id").and_then(|i| i.as_str()) == Some(pr_id))
        .cloned()
}

/// The re-run of `pr_id` as it would run, or the reason it cannot. Does not need the contract worktree.
pub fn plan(run_dir: &Path, pr_id: &str) -> Result<Plan, String> {
    if !valid_pr_id(pr_id) {
        return Err("This is not a PR.".into());
    }
    let run_id = run_id_of(run_dir);
    let check = read_app_checks(run_dir).remove(pr_id).ok_or_else(|| {
        "Only the app's own check can be re-run, and none was recorded.".to_string()
    })?;
    let commit = check
        .head
        .clone()
        .filter(|h| janitor::valid_commit(h))
        .ok_or_else(|| "The recorded check has no commit.".to_string())?;
    let command = check
        .command
        .clone()
        .filter(|c| !c.trim().is_empty())
        .ok_or_else(|| "The recorded check has no command.".to_string())?;
    let pr =
        pr_value(run_dir, pr_id).ok_or_else(|| "This PR is not in status.json.".to_string())?;
    let repo = pr
        .get("repoPath")
        .and_then(|p| p.as_str())
        .and_then(|p| janitor::verified_repo(run_dir, p))
        .ok_or_else(|| "The repo of this PR is not one of the run's repos.".to_string())?;
    let is_commit = git(&repo, &["cat-file", "-t", &commit]).is_ok_and(|kind| kind == "commit");
    if !is_commit {
        return Err(format!(
            "Commit {} is not in this repo (the branch may have been cleaned up).",
            &commit[..commit.len().min(7)]
        ));
    }
    // The recorded folder, relative to the PR's worktree path (text only: that folder may be gone).
    let prefix = janitor::expected_worktree(&repo, &run_id, pr_id)
        .ok_or_else(|| "The PR's worktree path is not known.".to_string())?;
    let cwd = match check
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        None => String::new(),
        Some(cwd) => {
            let rel = janitor::strip_worktree_prefix(Path::new(cwd), &prefix).ok_or_else(|| {
                "The recorded check folder is outside the PR's worktree.".to_string()
            })?;
            let mut parts = Vec::new();
            for part in rel.components() {
                match part {
                    std::path::Component::Normal(p) => parts.push(p.to_string_lossy().to_string()),
                    std::path::Component::CurDir => {}
                    _ => {
                        return Err(
                            "The recorded check folder is outside the PR's worktree.".to_string()
                        )
                    }
                }
            }
            parts.join("/")
        }
    };
    Ok(Plan {
        run_id,
        pr: pr_id.to_string(),
        repo,
        commit,
        command,
        cwd,
        recorded: Recorded {
            result: check.result.clone(),
            exit_code: check.exit_code,
            finished_at: check.finished_at.clone(),
            clean: check.clean,
        },
    })
}

/// [`plan`] for the confirmation: the setup is looked up at the recorded commit (the real run looks in the scratch
/// worktree, which holds the same files).
pub fn info(run_dir: &Path, pr_id: &str) -> Info {
    match plan(run_dir, pr_id) {
        Err(reason) => Info {
            reason: Some(reason),
            command: None,
            setup: None,
            cwd: String::new(),
            commit: None,
            clean: None,
            recorded: None,
        },
        Ok(p) => {
            let setup = setup_command(run_dir, &p.repo, |file| {
                git(
                    &p.repo,
                    &["cat-file", "-e", &format!("{}:{file}", p.commit)],
                )
                .is_ok()
            });
            Info {
                reason: None,
                command: Some(p.command),
                setup,
                cwd: p.cwd,
                commit: Some(p.commit),
                clean: p.recorded.clean,
                recorded: Some(p.recorded),
            }
        }
    }
}

// ---- the one slot ----

struct Active {
    run_id: String,
    pr: String,
    n: u32,
    stop: Arc<AtomicBool>,
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

fn active_lock() -> std::sync::MutexGuard<'static, Option<Active>> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Holds the slot; dropping it (also while a panic unwinds) frees it.
pub struct SlotGuard(());

impl Drop for SlotGuard {
    fn drop(&mut self) {
        *active_lock() = None;
    }
}

/// Takes the one re-run slot, or says one is running.
pub fn claim(run_id: &str, pr: &str) -> Result<(SlotGuard, Arc<AtomicBool>), String> {
    let mut active = active_lock();
    if active.is_some() {
        return Err(ALREADY_RUNNING.into());
    }
    let stop = Arc::new(AtomicBool::new(false));
    *active = Some(Active {
        run_id: run_id.to_string(),
        pr: pr.to_string(),
        n: 0,
        stop: stop.clone(),
    });
    Ok((SlotGuard(()), stop))
}

fn set_active_n(n: u32) {
    if let Some(a) = active_lock().as_mut() {
        a.n = n;
    }
}

/// A re-run is running in this app (any run).
pub fn running() -> bool {
    active_lock().is_some()
}

/// Asks the running re-run to stop. Says so when none runs.
pub fn stop() -> Result<(), String> {
    match active_lock().as_ref() {
        Some(a) => {
            a.stop.store(true, Ordering::SeqCst);
            Ok(())
        }
        None => Err("No re-run is running.".into()),
    }
}

/// Runs `body` on its own thread holding `guard`: a panic in `body` still frees the slot.
pub fn spawn_guarded<F: FnOnce() + Send + 'static>(
    guard: SlotGuard,
    body: F,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("flight-rerun".into())
        .spawn(move || {
            let _guard = guard;
            body();
        })
}

// ---- records ----

fn record_path(dir: &Path, pr: &str, n: u32) -> PathBuf {
    dir.join(format!("{pr}-{n}.json"))
}

fn write_record(dir: &Path, rec: &RerunRecord) -> Result<(), String> {
    crate::store::write_json_atomic(&record_path(dir, &rec.pr, rec.n), rec)
}

fn read_record_file(path: &Path) -> Option<RerunRecord> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Every record of a run, newest first within each PR.
pub fn list(data: &Path, run_id: &str) -> Vec<RerunRecord> {
    let dir = replays_dir(data, run_id);
    let mut out: Vec<RerunRecord> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|e| read_record_file(&e.path()))
        .filter(|r| r.run_id == run_id)
        .collect();
    out.sort_by(|a, b| a.pr.cmp(&b.pr).then(b.n.cmp(&a.n)));
    out
}

/// The `<n>` of a `<pr>-<n>.json` or `.log` file name.
fn number_of(name: &str, pr: &str) -> Option<u32> {
    let rest = name.strip_prefix(pr)?.strip_prefix('-')?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let tail = &rest[digits.len()..];
    (tail == ".json" || tail == ".log")
        .then(|| digits.parse().ok())
        .flatten()
}

/// Above the highest existing record or log of this PR.
fn next_n(dir: &Path, pr: &str) -> u32 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| number_of(&e.file_name().to_string_lossy(), pr))
        .max()
        .map_or(1, |n| n + 1)
}

/// Keeps the newest [`KEEP_PER_PR`] records of a PR (and their logs).
pub fn prune(dir: &Path, pr: &str) {
    let mut numbers: Vec<u32> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.ends_with(".json")
                .then(|| number_of(&name, pr))
                .flatten()
        })
        .collect();
    numbers.sort_unstable_by(|a, b| b.cmp(a));
    for n in numbers.into_iter().skip(KEEP_PER_PR) {
        let _ = std::fs::remove_file(record_path(dir, pr, n));
        let _ = std::fs::remove_file(dir.join(format!("{pr}-{n}.log")));
    }
}

// ---- running one ----

/// Starts a re-run and returns its first record (`preparing`). The work runs on its own thread; poll [`list`].
pub fn start(data: &Path, run_dir: &Path, pr_id: &str) -> Result<RerunRecord, String> {
    let plan = plan(run_dir, pr_id)?;
    let (guard, stop) = claim(&plan.run_id, &plan.pr)?;
    let dir = replays_dir(data, &plan.run_id);
    let prepared = prepare(&dir, run_dir, &plan);
    let rec = prepared?;
    set_active_n(rec.n);
    let first = rec.clone();
    let body_dir = dir.clone();
    let run_dir = run_dir.to_path_buf();
    spawn_guarded(guard, move || {
        let mut rec = rec;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            execute(&body_dir, &run_dir, &plan, &mut rec, &stop)
        }));
        if result.is_err() {
            rec.state = "error".into();
            rec.detail = Some("the re-run stopped unexpectedly".into());
            rec.finished_at = Some(now());
            if janitor::remove_scratch(&plan.repo, Path::new(&rec.worktree)).is_err() {
                rec.scratch_left = true;
            }
            let _ = write_record(&body_dir, &rec);
        }
    })
    .map_err(|e| format!("could not start the re-run: {e}"))?;
    Ok(first)
}

/// The first record, written before the scratch worktree is made.
fn prepare(dir: &Path, run_dir: &Path, plan: &Plan) -> Result<RerunRecord, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    let mut n = next_n(dir, &plan.pr);
    // A folder or record already at this number (an earlier run's leftovers) moves it up.
    let scratch = loop {
        let path = janitor::scratch_worktree(&plan.repo, &plan.run_id, &plan.pr, n)
            .ok_or_else(|| "cannot name a scratch worktree for this run".to_string())?;
        if std::fs::symlink_metadata(&path).is_err() {
            break path;
        }
        n += 1;
    };
    let setup = setup_command(run_dir, &plan.repo, |file| {
        git(
            &plan.repo,
            &["cat-file", "-e", &format!("{}:{file}", plan.commit)],
        )
        .is_ok()
    });
    let rec = RerunRecord {
        schema: 1,
        run_id: plan.run_id.clone(),
        pr: plan.pr.clone(),
        n,
        repo: canonical(&plan.repo).display().to_string(),
        worktree: scratch.display().to_string(),
        commit: plan.commit.clone(),
        command: plan.command.clone(),
        setup,
        cwd: plan.cwd.clone(),
        recorded: plan.recorded.clone(),
        state: "preparing".into(),
        exit_code: None,
        started_at: now(),
        finished_at: None,
        detail: None,
        log: format!("{}-{n}.log", plan.pr),
        scratch_left: false,
    };
    write_record(dir, &rec)?;
    Ok(rec)
}

fn finish(
    dir: &Path,
    rec: &mut RerunRecord,
    state: &str,
    exit: Option<i32>,
    detail: Option<String>,
) {
    rec.state = state.into();
    rec.exit_code = exit;
    rec.detail = detail;
    let _ = write_record(dir, rec);
}

/// Writes `$ <command>` as the next line of the log (the first step truncates it).
fn log_header(log: &Path, command: &str, append: bool) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = if append {
        crate::store::append_run_log(log)?
    } else {
        crate::store::create_run_log(log)?
    };
    writeln!(file, "$ {command}")
}

/// preparing, setup, running, then the scratch worktree is removed and the final record written.
fn execute(dir: &Path, run_dir: &Path, plan: &Plan, rec: &mut RerunRecord, stop: &AtomicBool) {
    let scratch = PathBuf::from(&rec.worktree);
    let log = dir.join(&rec.log);
    let outcome = run_steps(dir, run_dir, plan, rec, &scratch, &log, stop);
    // Stop wins whatever the step reported.
    let (state, exit, detail) = if stop.load(Ordering::SeqCst) {
        ("stopped".to_string(), None, Some("stopped".to_string()))
    } else {
        outcome
    };
    if janitor::remove_scratch(&plan.repo, &scratch).is_err() {
        rec.scratch_left = true;
    }
    rec.finished_at = Some(now());
    finish(dir, rec, &state, exit, detail);
    prune(dir, &plan.pr);
}

type Outcome = (String, Option<i32>, Option<String>);

fn run_steps(
    dir: &Path,
    run_dir: &Path,
    plan: &Plan,
    rec: &mut RerunRecord,
    scratch: &Path,
    log: &Path,
    stop: &AtomicBool,
) -> Outcome {
    if let Err(why) = janitor::add_scratch(&plan.repo, scratch, &plan.commit) {
        return (
            "error".into(),
            None,
            Some(format!("could not make the scratch worktree: {why}")),
        );
    }
    if stop.load(Ordering::SeqCst) {
        return ("stopped".into(), None, None);
    }
    // Setup is decided on, and run in, the scratch worktree root, as state.sh does for a new worktree.
    let root = canonical(scratch);
    let setup = setup_command(run_dir, &plan.repo, |file| root.join(file).is_file());
    rec.setup = setup.clone();
    let mut appended = false;
    if let Some(setup) = &setup {
        finish(dir, rec, "setup", None, None);
        if let Err(e) = log_header(log, setup, false) {
            return ("error".into(), None, Some(format!("log file: {e}")));
        }
        appended = true;
        let (result, code, detail) =
            run_one_stoppable(setup, &root, log, SETUP_TIMEOUT, true, stop);
        match result.as_str() {
            "pass" => {}
            "fail" => {
                return (
                    "error".into(),
                    code,
                    Some(format!(
                        "setup failed (exit {})",
                        code.map_or("?".into(), |c| c.to_string())
                    )),
                )
            }
            "timeout" => return ("error".into(), None, Some("setup timed out".into())),
            _ => return (result, code, detail),
        }
    }
    let folder = match check_folder(Some(&plan.cwd), scratch) {
        Ok(folder) => folder,
        Err(_) => {
            return (
                "error".into(),
                None,
                Some(format!(
                    "the check folder {} does not exist at that commit",
                    if plan.cwd.is_empty() { "." } else { &plan.cwd }
                )),
            )
        }
    };
    finish(dir, rec, "running", None, None);
    if let Err(e) = log_header(log, &plan.command, appended) {
        return ("error".into(), None, Some(format!("log file: {e}")));
    }
    run_one_stoppable(&plan.command, &folder, log, CHECK_TIMEOUT, true, stop)
}

// ---- leftovers ----

/// At app start: a record still `preparing`, `setup` or `running` that no live re-run holds becomes
/// `interrupted` and its scratch worktree is removed; a record whose scratch removal failed is retried; the records
/// of a run folder that is gone are deleted. Runs once, before the index thread's first sleep.
pub fn cleanup_leftovers(data: &Path) {
    let root = data.join("flight").join("replays");
    let held = active_lock()
        .as_ref()
        .map(|a| (a.run_id.clone(), a.pr.clone(), a.n));
    for entry in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        let run_id = entry.file_name().to_string_lossy().to_string();
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let mut stuck = false;
        for mut rec in list(data, &run_id) {
            if held
                .as_ref()
                .is_some_and(|(r, p, n)| *r == rec.run_id && *p == rec.pr && *n == rec.n)
            {
                stuck = true;
                continue;
            }
            let leftover = in_progress(&rec.state);
            if !leftover && !rec.scratch_left {
                continue;
            }
            match janitor::remove_scratch(Path::new(&rec.repo), Path::new(&rec.worktree)) {
                Ok(()) => rec.scratch_left = false,
                Err(_) => {
                    rec.scratch_left = true;
                    stuck = true;
                }
            }
            if leftover {
                rec.state = "interrupted".into();
                rec.finished_at = Some(now());
                rec.detail = Some("the app quit".into());
            }
            let _ = write_record(&dir, &rec);
        }
        if !stuck && !run_exists(data, &run_id) {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

fn run_exists(data: &Path, run_id: &str) -> bool {
    super::folder::runs_dir(data).join(run_id).is_dir()
        || super::folder::adopted_runs(data)
            .iter()
            .any(|p| p.file_name().is_some_and(|n| n.to_string_lossy() == run_id) && p.is_dir())
}

/// The tail of a re-run's log (at most 256 KiB), for the result block. A link or a missing file is an error.
pub fn read_log(data: &Path, run_dir: &Path, pr: &str, n: u32) -> Result<String, String> {
    const CAP: u64 = 256 * 1024;
    if !valid_pr_id(pr) {
        return Err("This is not a PR.".into());
    }
    let path = replays_dir(data, &run_id_of(run_dir)).join(format!("{pr}-{n}.log"));
    let bytes = match crate::store::read_capped(&path, 32 * 1024 * 1024) {
        Ok(bytes) => bytes,
        Err(crate::store::CappedError::TooLarge) => {
            return Err("The log is too large to show here.".into())
        }
        Err(_) => return Err("The log is not available.".into()),
    };
    let start = bytes.len().saturating_sub(CAP as usize);
    Ok(String::from_utf8_lossy(&bytes[start..]).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::git::testing::{repo_at, run};
    use crate::test_support::TempDir;
    use serde_json::json;
    #[cfg(unix)]
    use std::sync::atomic::AtomicBool;

    /// The re-run slot is one per process: tests that use it take turns.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    const RUN: &str = "r-20261105-130000-cd34";

    /// A run on disk over a toy repo: PR1 failed its app check at `head` in `<worktree>/sub`.
    struct Toy {
        dir: TempDir,
        data: PathBuf,
        run_dir: PathBuf,
        repo: PathBuf,
        wt: PathBuf,
        head: String,
    }

    impl Toy {
        #[cfg(unix)]
        fn out(&self, name: &str) -> PathBuf {
            self.dir.join("out").join(name)
        }
    }

    /// `setup`: repos.json setup string; `check`: the app check command; `lockfile`: committed at the repo root.
    fn toy(setup: Option<&str>, check: &str, lockfile: Option<&str>) -> Toy {
        let dir = TempDir::new("rerun_toy");
        let repo = dir.join("ws/toy");
        repo_at(&repo);
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        std::fs::write(repo.join("sub/keep.txt"), "k\n").unwrap();
        if let Some(lock) = lockfile {
            std::fs::write(repo.join(lock), "{}\n").unwrap();
        }
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-q", "-m", "sub"]);
        let head = crate::run::git::git(&repo, &["rev-parse", "HEAD"]).unwrap();
        let wt = janitor::expected_worktree(&repo, RUN, "PR1").unwrap();
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "dcv2/pr1",
                wt.to_str().unwrap(),
            ],
        );
        let wt = canonical(&wt);
        std::fs::create_dir_all(dir.join("out")).unwrap();
        let data = dir.join("data");
        let run_dir = data.join("runs").join(RUN);
        std::fs::create_dir_all(&run_dir).unwrap();
        let write = |name: &str, v: serde_json::Value| {
            std::fs::write(run_dir.join(name), v.to_string()).unwrap()
        };
        write(
            "status.json",
            json!({"schema":1,"stage":"done","prs":[{"id":"PR1","repoPath":repo,"worktree":wt}]}),
        );
        let mut entry = json!({"name":"toy","path":repo});
        if let Some(setup) = setup {
            entry["setup"] = json!(setup);
        }
        write("repos.json", json!({"schema":1,"repos":[entry]}));
        write("meta.json", json!({"schema":1,"reviewMode":"local"}));
        write(
            "app-checks.json",
            json!({"schema":1,"checks":{"PR1":{
                "result":"fail","command":check,"head":head,"clean":true,"exitCode":1,
                "cwd":wt.join("sub"),"finishedAt":"2026-11-05T13:00:00Z"}}}),
        );
        Toy {
            dir,
            data,
            run_dir,
            repo,
            wt,
            head,
        }
    }

    #[cfg(unix)]
    fn wait_done(t: &Toy) -> RerunRecord {
        for _ in 0..600 {
            if let Some(rec) = list(&t.data, RUN).into_iter().next() {
                if !in_progress(&rec.state) && !running() {
                    return rec;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the re-run did not finish: {:?}", list(&t.data, RUN));
    }

    /// Every file under `dir` (not `.git`) with its mtime.
    #[cfg(unix)]
    fn snapshot(dir: &Path) -> Vec<(String, std::time::SystemTime)> {
        let mut out = Vec::new();
        fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, std::time::SystemTime)>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let path = e.path();
                if path.file_name().is_some_and(|n| n == ".git") {
                    continue;
                }
                if path.is_dir() {
                    walk(base, &path, out);
                } else {
                    let rel = path.strip_prefix(base).unwrap().display().to_string();
                    out.push((rel, e.metadata().unwrap().modified().unwrap()));
                }
            }
        }
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    fn branches(repo: &Path) -> String {
        crate::run::git::git(repo, &["branch", "--list"]).unwrap()
    }

    #[cfg(unix)]
    fn check_cmd(t: &Toy) -> String {
        format!(
            "pwd -P > '{}'; echo checked; exit 0",
            t.dir.join("out/check-cwd").display()
        )
    }

    #[cfg(unix)]
    fn setup_cmd(t_dir: &TempDir) -> String {
        format!(
            "pwd -P > '{}'; echo installed",
            t_dir.join("out/setup-cwd").display()
        )
    }

    #[cfg(unix)]
    fn toy_with_scripts() -> Toy {
        // The scripts name the out folder, which exists only once the toy is made: write them afterwards.
        let t = toy(Some("true"), "true", None);
        let setup = setup_cmd(&t.dir);
        let check = check_cmd(&t);
        let write = |name: &str, v: serde_json::Value| {
            std::fs::write(t.run_dir.join(name), v.to_string()).unwrap()
        };
        write(
            "repos.json",
            json!({"schema":1,"repos":[{"name":"toy","path":t.repo,"setup":setup}]}),
        );
        write(
            "app-checks.json",
            json!({"schema":1,"checks":{"PR1":{
                "result":"fail","command":check,"head":t.head,"clean":true,"exitCode":1,
                "cwd":t.wt.join("sub"),"finishedAt":"2026-11-05T13:00:00Z"}}}),
        );
        t
    }

    #[cfg(unix)]
    fn assert_rerun_in_scratch(t: &Toy) {
        let rec = {
            let first = start(&t.data, &t.run_dir, "PR1").unwrap();
            assert_eq!((first.state.as_str(), first.n), ("preparing", 1));
            wait_done(t)
        };
        assert_eq!(rec.state, "pass", "{rec:?}");
        assert_eq!(rec.cwd, "sub");
        assert_eq!(rec.commit, t.head);
        assert_eq!(rec.recorded.result, "fail");
        assert!(rec.worktree.contains("dcv2-replay-"), "{}", rec.worktree);
        let scratch = canonical(Path::new(&rec.worktree));
        let setup_dir = std::fs::read_to_string(t.out("setup-cwd")).unwrap();
        let check_dir = std::fs::read_to_string(t.out("check-cwd")).unwrap();
        assert_eq!(
            setup_dir.trim(),
            scratch.display().to_string(),
            "setup at the scratch root"
        );
        assert_eq!(
            check_dir.trim(),
            scratch.join("sub").display().to_string(),
            "the check in the recorded folder of the scratch worktree"
        );
        assert!(!scratch.exists(), "the scratch worktree is removed");
        assert!(!rec.scratch_left);
        // One log, both steps.
        let log = std::fs::read_to_string(replays_dir(&t.data, RUN).join(&rec.log)).unwrap();
        let setup_at = log.find("$ pwd -P > ").unwrap();
        assert!(
            log.contains("installed") && log.contains("checked"),
            "{log}"
        );
        assert!(log.find("installed").unwrap() > setup_at);
        assert!(log.find("checked").unwrap() > log.find("installed").unwrap());
        assert_eq!(
            log.matches("\n$ ").count() + usize::from(log.starts_with("$ ")),
            2,
            "{log}"
        );
        // Nothing in the run folder, the branch is intact.
        assert!(!t.run_dir.join("flight").exists());
        assert!(branches(&t.repo).contains("dcv2/pr1"));
    }

    #[cfg(unix)]
    #[test]
    fn a_rerun_sets_up_at_the_scratch_root_and_checks_in_the_recorded_subfolder_leaving_the_run_untouched(
    ) {
        let _serial = serial();
        let t = toy_with_scripts();
        let before = snapshot(&t.wt);
        let run_before = snapshot(&t.run_dir);
        assert_rerun_in_scratch(&t);
        assert_eq!(
            snapshot(&t.wt),
            before,
            "the contract worktree is untouched"
        );
        assert_eq!(
            snapshot(&t.run_dir),
            run_before,
            "the run folder is untouched"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_rerun_works_after_the_contract_worktree_was_removed() {
        let _serial = serial();
        let t = toy_with_scripts();
        run(
            &t.repo,
            &["worktree", "remove", "--force", t.wt.to_str().unwrap()],
        );
        assert!(!t.wt.exists());
        assert_rerun_in_scratch(&t);
    }

    #[test]
    fn a_recorded_folder_outside_the_worktree_is_refused() {
        let t = toy(None, "true", None);
        let outside = t.dir.join("elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        for cwd in [
            outside.display().to_string(),
            format!("{}/../..", t.wt.display()),
        ] {
            let p = t.run_dir.join("app-checks.json");
            let mut v: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
            v["checks"]["PR1"]["cwd"] = json!(cwd);
            std::fs::write(&p, v.to_string()).unwrap();
            let err = plan(&t.run_dir, "PR1").unwrap_err();
            assert!(err.contains("outside"), "{cwd}: {err}");
        }
    }

    #[test]
    fn the_plan_says_why_there_is_no_rerun() {
        let t = toy(None, "true", None);
        assert!(plan(&t.run_dir, "PR9")
            .unwrap_err()
            .contains("none was recorded"));
        assert!(plan(&t.run_dir, "../x").is_err());
        let p = t.run_dir.join("app-checks.json");
        let set = |key: &str, value: serde_json::Value| {
            let mut v: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
            v["checks"]["PR1"][key] = value;
            std::fs::write(&p, v.to_string()).unwrap();
        };
        set("head", json!("0123456789abcdef0123456789abcdef01234567"));
        assert!(plan(&t.run_dir, "PR1")
            .unwrap_err()
            .contains("is not in this repo"));
        set("head", json!("--output=x"));
        assert!(plan(&t.run_dir, "PR1").unwrap_err().contains("no commit"));
        set("head", json!(t.head));
        set("command", json!(" "));
        assert!(plan(&t.run_dir, "PR1").unwrap_err().contains("no command"));
        set("command", json!("true"));
        let ok = plan(&t.run_dir, "PR1").unwrap();
        assert_eq!(
            (ok.cwd.as_str(), ok.commit.as_str()),
            ("sub", t.head.as_str())
        );
    }

    #[test]
    fn the_setup_comes_from_repos_json_or_a_lockfile_only_for_a_new_run() {
        let t = toy(None, "true", Some("package-lock.json"));
        let i = info(&t.run_dir, "PR1");
        assert_eq!(i.reason, None);
        assert_eq!(
            i.setup.as_deref(),
            Some("npm ci"),
            "a lockfile at the root of the commit"
        );
        assert_eq!(i.cwd, "sub");
        assert_eq!(i.command.as_deref(), Some("true"));
        // An older run (no reviewMode) gets no detected install; a repos.json string wins; "" is none.
        let meta = t.run_dir.join("meta.json");
        std::fs::write(&meta, r#"{"schema":1}"#).unwrap();
        assert_eq!(info(&t.run_dir, "PR1").setup, None);
        std::fs::write(&meta, r#"{"schema":1,"reviewMode":"github"}"#).unwrap();
        let repos = |entry: serde_json::Value| {
            std::fs::write(
                t.run_dir.join("repos.json"),
                json!({"schema":1,"repos":[entry]}).to_string(),
            )
            .unwrap()
        };
        repos(json!({"name":"toy","path":t.repo,"setup":"  make deps  "}));
        assert_eq!(info(&t.run_dir, "PR1").setup.as_deref(), Some("make deps"));
        repos(json!({"name":"toy","path":t.repo,"setup":""}));
        assert_eq!(info(&t.run_dir, "PR1").setup, None);
        // The lockfile table order.
        assert_eq!(
            setup_command(&t.run_dir, &t.repo, |f| f == "yarn.lock" || f == "go.mod").as_deref(),
            None,
            "an empty setup string still means none"
        );
    }

    #[test]
    fn the_setup_rule_equals_state_sh_detect_setup() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugin/scripts/state.sh");
        let text = std::fs::read_to_string(path).unwrap();
        let body = &text[text.find("def detect_setup(wt):").unwrap()..];
        let body = &body[..body.find("def run_setup").unwrap()];
        let mut table = Vec::new();
        for line in body.lines() {
            let mut rest = line;
            while let Some(at) = rest.find("(\"") {
                let item = &rest[at + 2..];
                let (file, tail) = item.split_once("\", \"").unwrap();
                let (cmd, tail) = tail.split_once("\")").unwrap();
                table.push((file.to_string(), cmd.to_string()));
                rest = tail;
            }
        }
        let ours: Vec<(String, String)> = LOCKFILES
            .iter()
            .map(|(f, c)| (f.to_string(), c.to_string()))
            .collect();
        assert_eq!(table, ours, "the lockfile table drifted from state.sh");
        // The caller: only a new run (reviewMode in meta.json) detects, the string wins, and the setup runs at the worktree root.
        assert!(text.contains("new_run = isinstance(meta, dict) and \"reviewMode\" in meta"));
        assert!(text.contains(
            "cmd = setup.strip() if isinstance(setup, str) else (detect_setup(wt) if new_run else None)"
        ));
        assert!(text.contains("result, log = run_setup(run, wt, pr_id, cmd or None)"));
        assert!(text.contains("portable.spawn_group(portable.check_argv(cmd), cwd=wt,"));
    }

    #[cfg(unix)]
    #[test]
    fn only_the_recorded_command_runs_never_a_recorded_tool_input() {
        let _serial = serial();
        let t = toy(None, "echo only-this", None);
        let sentinel = t.dir.join("sentinel");
        std::fs::create_dir_all(&sentinel).unwrap();
        // A transcript that recorded `rm -rf <sentinel>` sits where Claude keeps it; a re-run never reads it.
        let projects = t.dir.join("config/projects/-work-toy");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::write(
            projects.join("bbbb1111-2222-4333-8444-555555555555.jsonl"),
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"Bash",
                "input":{"command":format!("rm -rf {}", sentinel.display())}}]}})
            .to_string(),
        )
        .unwrap();
        start(&t.data, &t.run_dir, "PR1").unwrap();
        let rec = wait_done(&t);
        assert_eq!(rec.state, "pass", "{rec:?}");
        assert!(sentinel.is_dir(), "the recorded rm -rf never ran");
        let log = std::fs::read_to_string(replays_dir(&t.data, RUN).join(&rec.log)).unwrap();
        assert!(log.starts_with("$ echo only-this\n"), "{log}");
        let live = crate::test_support::without_test_items(include_str!("rerun.rs"));
        assert!(!live.contains(concat!("flight", "::")) && !live.contains("transcript_path"));
    }

    #[test]
    fn a_second_request_is_refused_and_a_panicking_body_frees_the_slot() {
        let _serial = serial();
        let (guard, _stop) = claim(RUN, "PR1").unwrap();
        assert!(running());
        assert_eq!(claim(RUN, "PR2").err().as_deref(), Some(ALREADY_RUNNING));
        // start() refuses too, before touching anything.
        let t = toy(None, "true", None);
        assert_eq!(
            start(&t.data, &t.run_dir, "PR1").err().as_deref(),
            Some(ALREADY_RUNNING)
        );
        assert!(list(&t.data, RUN).is_empty(), "nothing was written");
        drop(guard);
        assert!(!running());
        let (guard, _) = claim(RUN, "PR1").unwrap();
        let handle = spawn_guarded(guard, || panic!("test hook: the re-run body panics")).unwrap();
        assert!(handle.join().is_err());
        assert!(!running(), "the drop guard freed the slot");
        drop(claim(RUN, "PR1").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn stop_kills_a_sleeping_check_and_ends_stopped_with_the_worktree_removed() {
        let _serial = serial();
        let t = toy(None, "true", None);
        let pidfile = t.dir.join("out/pid");
        let check = format!("echo $$ > '{}'; sleep 60", pidfile.display());
        let p = t.run_dir.join("app-checks.json");
        let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        v["checks"]["PR1"]["command"] = json!(check);
        std::fs::write(&p, v.to_string()).unwrap();
        start(&t.data, &t.run_dir, "PR1").unwrap();
        // Wait until the check runs.
        for _ in 0..200 {
            if pidfile.exists()
                && std::fs::read_to_string(&pidfile).is_ok_and(|s| !s.trim().is_empty())
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let pid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let started = std::time::Instant::now();
        stop().unwrap();
        let rec = wait_done(&t);
        assert_eq!(rec.state, "stopped", "{rec:?}");
        assert!(started.elapsed() < Duration::from_secs(20));
        assert!(!Path::new(&rec.worktree).exists());
        let alive = crate::platform::quiet_command("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .unwrap()
            .success();
        assert!(!alive, "the sleeping child was killed");
        assert!(stop().is_err(), "nothing runs now");
    }

    #[cfg(unix)]
    #[test]
    fn the_stoppable_run_one_kills_a_sleeping_child_and_can_append() {
        let dir = TempDir::new("rerun_stoppable");
        let log = dir.join("log.txt");
        std::fs::write(&log, "$ first\n").unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let setter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            flag.store(true, Ordering::SeqCst);
        });
        let t = std::time::Instant::now();
        let (result, code, _) = run_one_stoppable(
            "echo started; sleep 30",
            dir.path(),
            &log,
            Duration::from_secs(60),
            true,
            &stop,
        );
        setter.join().unwrap();
        assert_eq!((result.as_str(), code), ("stopped", None));
        assert!(t.elapsed() < Duration::from_secs(10));
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(
            text.starts_with("$ first\n") && text.contains("started"),
            "{text}"
        );
        // Without the flag it behaves like run_one.
        let never = AtomicBool::new(false);
        let (result, code, _) = run_one_stoppable(
            "exit 3",
            dir.path(),
            &log,
            Duration::from_secs(10),
            false,
            &never,
        );
        assert_eq!((result.as_str(), code), ("fail", Some(3)));
        assert!(
            !std::fs::read_to_string(&log).unwrap().contains("$ first"),
            "truncated without append"
        );
    }

    fn leftover(t: &Toy, n: u32, state: &str, scratch_left: bool) -> RerunRecord {
        let dir = replays_dir(&t.data, RUN);
        std::fs::create_dir_all(&dir).unwrap();
        let scratch = janitor::scratch_worktree(&t.repo, RUN, "PR1", n).unwrap();
        janitor::add_scratch(&t.repo, &scratch, &t.head).unwrap();
        let rec = RerunRecord {
            schema: 1,
            run_id: RUN.into(),
            pr: "PR1".into(),
            n,
            repo: canonical(&t.repo).display().to_string(),
            worktree: scratch.display().to_string(),
            commit: t.head.clone(),
            command: "true".into(),
            cwd: String::new(),
            state: state.into(),
            started_at: now(),
            log: format!("PR1-{n}.log"),
            scratch_left,
            ..Default::default()
        };
        write_record(&dir, &rec).unwrap();
        rec
    }

    fn state_of(t: &Toy, n: u32) -> RerunRecord {
        read_record_file(&record_path(&replays_dir(&t.data, RUN), "PR1", n)).unwrap()
    }

    #[test]
    fn leftovers_become_interrupted_are_removed_retried_and_the_live_slot_is_skipped() {
        let _serial = serial();
        let t = toy(None, "true", None);
        let a = leftover(&t, 1, "running", false);
        let b = leftover(&t, 2, "pass", true);
        let c = leftover(&t, 3, "setup", false);
        let fine = leftover(&t, 4, "pass", false);
        assert!(Path::new(&a.worktree).exists());
        // Record 3 is held by a live re-run.
        let (guard, _stop) = claim(RUN, "PR1").unwrap();
        set_active_n(3);
        cleanup_leftovers(&t.data);
        drop(guard);
        let a2 = state_of(&t, 1);
        assert_eq!(a2.state, "interrupted");
        assert!(
            !Path::new(&a.worktree).exists(),
            "its scratch worktree was removed"
        );
        let b2 = state_of(&t, 2);
        assert_eq!(
            (b2.state.as_str(), b2.scratch_left),
            ("pass", false),
            "retried and cleared"
        );
        assert!(!Path::new(&b.worktree).exists());
        let c2 = state_of(&t, 3);
        assert_eq!(c2.state, "setup", "the live slot's record is skipped");
        assert!(Path::new(&c.worktree).exists());
        assert!(
            Path::new(&fine.worktree).exists(),
            "a finished record's worktree is not touched"
        );
        // The contract worktree and branch are untouched.
        assert!(t.wt.exists() && branches(&t.repo).contains("dcv2/pr1"));
        // The records of a run folder that is gone are deleted.
        for rec in [&c, &fine] {
            janitor::remove_scratch(&t.repo, Path::new(&rec.worktree)).unwrap();
        }
        std::fs::remove_dir_all(&t.run_dir).unwrap();
        cleanup_leftovers(&t.data);
        assert!(!replays_dir(&t.data, RUN).exists());
    }

    #[test]
    fn only_the_newest_five_records_and_logs_per_pr_are_kept() {
        let dir = TempDir::new("rerun_prune");
        for n in 1..=7u32 {
            let rec = RerunRecord {
                schema: 1,
                run_id: RUN.into(),
                pr: "PR1".into(),
                n,
                state: "pass".into(),
                log: format!("PR1-{n}.log"),
                ..Default::default()
            };
            write_record(dir.path(), &rec).unwrap();
            std::fs::write(dir.join(format!("PR1-{n}.log")), "x").unwrap();
        }
        let other = RerunRecord {
            schema: 1,
            run_id: RUN.into(),
            pr: "PR2".into(),
            n: 1,
            ..Default::default()
        };
        write_record(dir.path(), &other).unwrap();
        prune(dir.path(), "PR1");
        let mut names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert!(
            !names.contains(&"PR1-1.json".to_string()) && !names.contains(&"PR1-2.log".to_string())
        );
        for n in 3..=7 {
            assert!(
                names.contains(&format!("PR1-{n}.json")) && names.contains(&format!("PR1-{n}.log"))
            );
        }
        assert!(names.contains(&"PR2-1.json".to_string()));
        assert_eq!(next_n(dir.path(), "PR1"), 8);
        assert_eq!(next_n(dir.path(), "PR3"), 1);
    }
}
