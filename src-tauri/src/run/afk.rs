//! The app side of AFK runs (plan 2026-10-27): the merged-run cleanup sweep
//! and the typed-request sweep, each on its own thread, plus the app's
//! actions behind the request kinds. The decision logic and its tests live
//! in `cleanup` and `requests`; this module only wires them to the app.

use super::cleanup::{self, GhCli};
use super::requests::{self, Actions, Limiter};
use super::{data_dir, folder, janitor, status};
use crate::store::SettingsStore;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// How often the cleanup sweep looks (each run is asked at most every
/// [`cleanup::ASK_EVERY`]); a run that just reached `done` is seen within it.
const CLEANUP_TICK: Duration = Duration::from_secs(30);
/// How often the request sweep looks for new requests.
const REQUEST_TICK: Duration = Duration::from_secs(2);
/// How often a run's old answered requests are pruned.
const PRUNE_EVERY: Duration = Duration::from_secs(3600);

fn auto_cleanup_on(app: &AppHandle) -> bool {
    app.state::<Mutex<SettingsStore>>()
        .lock()
        .map(|s| s.run().auto_cleanup)
        .unwrap_or(false)
}

/// Starts both sweeps (app setup).
pub fn spawn_afk_sweeps(app: &AppHandle) {
    let cleanup_app = app.clone();
    std::thread::spawn(move || {
        let mut asked: HashMap<PathBuf, Instant> = HashMap::new();
        loop {
            let data = data_dir(&cleanup_app);
            let enabled = auto_cleanup_on(&cleanup_app);
            cleanup::sweep(&data, enabled, &GhCli, &mut asked, cleanup::ASK_EVERY);
            std::thread::sleep(CLEANUP_TICK);
        }
    });
    let request_app = app.clone();
    std::thread::spawn(move || {
        let mut limiter = Limiter::default();
        let mut first = true;
        // When each run's answered requests were last pruned (once an hour).
        let mut pruned: HashMap<PathBuf, Instant> = HashMap::new();
        loop {
            let data = data_dir(&request_app);
            // The app's own run folders only (adopted runs belong to the dev
            // app, as with auto-post), and only those with a requests/ folder:
            // one directory read per tick, no file parsing.
            let runs: Vec<PathBuf> = std::fs::read_dir(folder::runs_dir(&data))
                .map(|e| e.flatten().map(|e| e.path()).collect())
                .unwrap_or_default();
            for dir in runs {
                let hidden = dir
                    .file_name()
                    .is_none_or(|n| n.to_string_lossy().starts_with('.'));
                if hidden || !dir.join(requests::REQUESTS_DIR).is_dir() {
                    continue;
                }
                if first {
                    requests::fail_interrupted(&dir);
                }
                if pruned
                    .get(&dir)
                    .is_none_or(|at| at.elapsed() >= PRUNE_EVERY)
                {
                    pruned.insert(dir.clone(), Instant::now());
                    requests::prune(&dir, std::time::SystemTime::now());
                }
                for pending in requests::pending(&dir) {
                    let Some(kind) = requests::accept(&dir, &pending, &mut limiter, Instant::now())
                    else {
                        continue;
                    };
                    let app = request_app.clone();
                    let dir = dir.clone();
                    std::thread::spawn(move || {
                        let actions = AppActions { app };
                        requests::perform(&dir, &pending.id, &kind, &actions);
                    });
                }
            }
            first = false;
            std::thread::sleep(REQUEST_TICK);
        }
    });
}

/// The app's own actions for the request kinds. Each takes only the run
/// folder; nothing comes from the request file.
struct AppActions {
    app: AppHandle,
}

fn stage_of(dir: &Path) -> Option<String> {
    folder::read_json_value(&dir.join("status.json"))
        .filter(super::status::has_known_schema)
        .and_then(|v| v.get("stage").and_then(|s| s.as_str()).map(str::to_string))
}

impl Actions for AppActions {
    fn perform(&self, dir: &Path, kind: &str) -> Result<String, String> {
        match kind {
            "recheck" => recheck(dir),
            "cleanup" => clean_up(dir),
            "tickets" => tickets(&data_dir(&self.app), dir),
            "selftest" => selftest(&self.app, dir),
            _ => Err("unknown request".into()),
        }
    }
}

/// R1, then one line per PR from app-checks.json.
fn recheck(dir: &Path) -> Result<String, String> {
    super::rerun_checks_once(dir)?;
    let checks = status::read_app_checks(dir);
    if checks.is_empty() {
        return Ok("no PR to check yet".into());
    }
    let lines: Vec<String> = checks
        .iter()
        .map(|(id, c)| {
            let log = c
                .log
                .as_deref()
                .map(|l| format!(" (log {l})"))
                .unwrap_or_default();
            format!(
                "{id} {}{}",
                c.result,
                if c.result == "pass" {
                    String::new()
                } else {
                    log
                }
            )
        })
        .collect();
    let all = checks
        .values()
        .all(|c| c.result == "pass" || c.result == "none");
    let text = format!("app checks: {}", lines.join(", "));
    if all {
        Ok(text)
    } else {
        Err(text)
    }
}

/// Finished runs only: the merged-PR cleanup, then the janitor's clean
/// worktrees (branches kept unless merged; remote branches only of merged PRs).
fn clean_up(dir: &Path) -> Result<String, String> {
    let stage = stage_of(dir);
    if !matches!(stage.as_deref(), Some("done") | Some("failed")) {
        return Err(format!(
            "the run is not finished (stage {}): cleanup waits for done or failed",
            stage.as_deref().unwrap_or("none")
        ));
    }
    let mut lines: Vec<String> = cleanup::cleanup_run(dir, &GhCli)
        .iter()
        .map(cleanup::CleanupRecord::line)
        .collect();
    let removable: Vec<String> = janitor::list_worktrees(dir)
        .into_iter()
        .filter(|item| item.removable)
        .map(|item| item.pr_id)
        .collect();
    for r in janitor::remove_clean(dir, &removable) {
        lines.push(format!("{}: {}", r.pr_id, r.message));
    }
    let kept: Vec<String> = janitor::list_worktrees(dir)
        .into_iter()
        .filter(|item| item.exists)
        .map(|item| {
            let why = item
                .problem
                .unwrap_or_else(|| format!("{} changed files", item.changed_files));
            format!("{} kept ({why})", item.pr_id)
        })
        .collect();
    lines.extend(kept);
    if lines.is_empty() {
        return Ok("nothing to clean up".into());
    }
    Ok(lines.join("; "))
}

/// R8: ticket ids and the changed fields only (no ticket text).
fn tickets(data: &Path, dir: &Path) -> Result<String, String> {
    let changed = crate::clickup::commands::changed_tickets(data, dir)?;
    if changed.is_empty() {
        return Ok("no ticket changed in ClickUp since the run started".into());
    }
    let lines: Vec<String> = changed
        .iter()
        .map(|t| {
            if t.missing {
                return format!("{} (no longer visible in ClickUp)", t.id);
            }
            let mut fields: Vec<&str> = t
                .changes
                .iter()
                .map(|c| c.split(':').next().unwrap_or("").trim())
                .filter(|f| !f.is_empty() && f.len() <= 30)
                .collect();
            fields.dedup();
            format!("{} ({})", t.id, fields.join(", "))
        })
        .collect();
    Ok(format!(
        "{} ticket(s) changed in ClickUp: {}; pulling them in is done in the app's run panel",
        changed.len(),
        lines.join(", ")
    ))
}

/// R3 with the run's own account (meta.json `account.id`).
fn selftest(app: &AppHandle, dir: &Path) -> Result<String, String> {
    let account = folder::read_json_value(&dir.join("meta.json"))
        .and_then(|m| m["account"]["id"].as_str().map(str::to_string))
        .ok_or("the run's meta.json names no account")?;
    let provider = {
        let settings = app.state::<Mutex<SettingsStore>>();
        let settings = settings.lock().map_err(|e| e.to_string())?;
        if !crate::store::settings_store::account_exists(settings.claude(), &account) {
            return Err("the run's Claude account is no longer in Settings".into());
        }
        crate::provider::ClaudeProvider::for_account(settings.claude(), &account)
    };
    let result = crate::observe::selftest_with(app, provider)?;
    let version = result.version.as_deref().unwrap_or("unknown version");
    if result.ok {
        Ok(format!(
            "self-test passed (Claude Code {version}, {} s)",
            result.seconds
        ))
    } else {
        Err(format!(
            "self-test failed (Claude Code {version}): {}",
            result.detail.as_deref().unwrap_or("see the app's banner")
        ))
    }
}
