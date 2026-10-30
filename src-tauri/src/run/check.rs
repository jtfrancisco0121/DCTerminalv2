//! R1: before stop #2 the app re-runs each PR's check itself, in the PR's
//! worktree (or `check.cwd` inside it), with a timeout. The command is the
//! one the app wrote to repos.json for the PR's repo, never status.json's
//! (the skill's file is LLM-written); the worktree must be where the
//! contract puts it (`janitor::verify_worktree`). Full output goes to
//! `<run>/logs/<PR>-app-check.txt`; results to `<run>/app-checks.json`
//! (app-owned; the skill never reads it). Runs with no store lock held.

use super::status::{app_checks_path, parse_status, read_app_checks, valid_pr_id, AppCheck};
use crate::store::write_json_atomic;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long one PR's check may run.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(15 * 60);

fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// The folder a check runs in: the verified worktree, or `cwd` inside it
/// (a relative `cwd` is joined onto the worktree). Anything outside it is
/// refused.
pub fn check_folder(cwd: Option<&str>, worktree: &Path) -> Result<PathBuf, String> {
    let root = super::git::canonical(worktree);
    let folder = match cwd.map(str::trim).filter(|c| !c.is_empty()) {
        None => root.clone(),
        Some(cwd) if Path::new(cwd).is_absolute() => PathBuf::from(cwd),
        Some(cwd) => root.join(cwd),
    };
    if !folder.is_dir() {
        return Err(format!("{} does not exist", folder.display()));
    }
    let folder = super::git::canonical(&folder);
    if !folder.starts_with(&root) {
        return Err(format!(
            "check folder {} is outside the PR's worktree",
            folder.display()
        ));
    }
    Ok(folder)
}

/// Process groups of checks running now, killed when the app exits.
static RUNNING: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

fn kill_group(pid: u32) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-9", &format!("-{pid}")])
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Stops every check the app is running (app exit).
pub fn kill_all() {
    if let Ok(mut running) = RUNNING.lock() {
        for pid in running.drain(..) {
            kill_group(pid);
        }
    }
}

fn shell_command(command: &str) -> Command {
    if cfg!(windows) {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", command]);
        cmd
    } else {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", command]);
        cmd
    }
}

/// Runs `command` in `cwd`, output to `log`, killed after `timeout`.
/// Returns (`pass`|`fail`|`timeout`|`error`, exit code, detail).
pub fn run_one(
    command: &str,
    cwd: &Path,
    log: &Path,
    timeout: Duration,
) -> (String, Option<i32>, Option<String>) {
    if let Some(parent) = log.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = match crate::store::create_run_log(log) {
        Ok(file) => file,
        Err(err) => return ("error".into(), None, Some(format!("log file: {err}"))),
    };
    let err_file = match file.try_clone() {
        Ok(file) => file,
        Err(err) => return ("error".into(), None, Some(format!("log file: {err}"))),
    };
    let mut cmd = shell_command(command);
    cmd.current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(err_file))
        .env("CI", "1")
        .env_remove("DCV2_TEST_MODE");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            return (
                "error".into(),
                None,
                Some(format!("could not start: {err}")),
            )
        }
    };
    let pid = child.id();
    if let Ok(mut running) = RUNNING.lock() {
        running.push(pid);
    }
    let untrack = || {
        if let Ok(mut running) = RUNNING.lock() {
            running.retain(|p| *p != pid);
        }
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                untrack();
                let code = status.code();
                let result = if status.success() { "pass" } else { "fail" };
                return (result.into(), code, None);
            }
            Ok(None) if started.elapsed() >= timeout => {
                kill_group(pid);
                untrack();
                let _ = child.kill();
                let _ = child.wait();
                return (
                    "timeout".into(),
                    None,
                    Some(format!("stopped after {} s", timeout.as_secs())),
                );
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(err) => {
                untrack();
                return ("error".into(), None, Some(err.to_string()));
            }
        }
    }
}

fn save(run_dir: &Path, checks: &std::collections::BTreeMap<String, AppCheck>) {
    let _ = write_json_atomic(
        &app_checks_path(run_dir),
        &json!({"schema": 1, "checks": checks}),
    );
}

/// Re-runs every PR's check of the run, one after another.
pub fn rerun_checks(run_dir: &Path, timeout: Duration) -> Result<(), String> {
    let text = std::fs::read_to_string(run_dir.join("status.json"))
        .map_err(|_| "This run has no status.json yet.".to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("status.json: {e}"))?;
    let status = parse_status(&value)?;
    let run_id = run_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let repo_checks = super::status::repo_checks(run_dir);
    let mut checks = read_app_checks(run_dir);
    let prs: Vec<_> = status
        .prs
        .into_iter()
        .filter(|pr| valid_pr_id(&pr.id))
        .collect();
    for pr in &prs {
        checks.insert(
            pr.id.clone(),
            AppCheck {
                result: "running".into(),
                ..AppCheck::default()
            },
        );
    }
    save(run_dir, &checks);
    for pr in prs {
        let verified = pr
            .worktree
            .as_deref()
            .ok_or_else(|| "the PR has no worktree yet".to_string())
            .and_then(|wt| {
                super::janitor::verify_worktree(
                    &run_id,
                    &pr.id,
                    Path::new(wt),
                    pr.repo_path.as_deref(),
                )
                .map(|main| (PathBuf::from(wt), main))
            });
        let command = super::status::check_for(
            &repo_checks,
            verified.as_ref().ok().map(|(_, main)| main.as_path()),
            pr.repo_path.as_deref(),
        );
        let skill = pr.check.command.clone().filter(|c| !c.trim().is_empty());
        let mut result = AppCheck {
            command: command.clone(),
            skill_command: skill.filter(|s| Some(s) != command.as_ref()),
            ..AppCheck::default()
        };
        let worktree = verified.map(|(wt, _)| wt);
        match (
            command,
            worktree.and_then(|wt| check_folder(pr.check.cwd.as_deref(), &wt).map(|cwd| (wt, cwd))),
        ) {
            (None, _) => {
                result.result = "none".into();
                result.detail = Some("no check known".into());
            }
            (Some(_), Err(problem)) => {
                result.result = "error".into();
                result.detail = Some(problem);
            }
            (Some(command), Ok((wt, cwd))) => {
                let (head, clean) = super::status::worktree_state(&wt);
                result.head = head;
                result.clean = clean;
                let rel = format!("logs/{}-app-check.txt", pr.id);
                let (outcome, code, detail) = run_one(&command, &cwd, &run_dir.join(&rel), timeout);
                result.result = outcome;
                result.exit_code = code;
                result.detail = detail;
                result.cwd = Some(cwd.display().to_string());
                result.log = Some(rel);
            }
        }
        result.finished_at = Some(now());
        checks.insert(pr.id.clone(), result);
        save(run_dir, &checks);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::git::testing::{repo_at, run};
    use crate::test_support::TempDir;

    const RUN: &str = "r-20261022-1304";

    #[cfg(unix)]
    #[test]
    fn two_prs_of_one_repo_are_each_checked_in_their_own_worktree() {
        let dir = TempDir::new("app_check_two");
        let repo = dir.join("ws/mathlib");
        repo_at(&repo);
        let wts = dir.join("ws/mathlib-worktrees");
        let wt = |n: u32| wts.join(format!("dcv2-{RUN}-pr{n}"));
        for n in [1, 2] {
            run(
                &repo,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    &format!("dcv2/{RUN}-pr{n}"),
                    wt(n).to_str().unwrap(),
                ],
            );
        }
        std::fs::write(wt(1).join("marker.txt"), "only PR1 has it").unwrap();
        let run_dir = dir.join("runs").join(RUN);
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(
            run_dir.join("repos.json"),
            json!({"schema": 1, "repos": [{"name": "mathlib", "path": repo, "check": "test -f marker.txt"}]})
                .to_string(),
        )
        .unwrap();
        std::fs::write(
            run_dir.join("status.json"),
            json!({"schema": 1, "stage": "ship-approval", "prs": [
                {"id": "PR1", "repo": "mathlib", "repoPath": repo, "worktree": wt(1), "check": {"cwd": wt(1)}},
                {"id": "PR2", "repo": "mathlib", "repoPath": repo, "worktree": wt(2), "check": {"cwd": wt(2)}}
            ]})
            .to_string(),
        )
        .unwrap();
        rerun_checks(&run_dir, Duration::from_secs(30)).unwrap();
        let checks = read_app_checks(&run_dir);
        assert_eq!(checks["PR1"].result, "pass", "{:?}", checks["PR1"]);
        assert_eq!(checks["PR2"].result, "fail", "{:?}", checks["PR2"]);
        let head = |n: u32| crate::run::git::git(&wt(n), &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(checks["PR1"].head.as_deref(), Some(head(1).as_str()));
        assert_eq!(checks["PR2"].head.as_deref(), Some(head(2).as_str()));
    }

    #[cfg(unix)]
    #[test]
    fn the_app_runs_repos_json_checks_in_verified_worktrees_only() {
        let dir = TempDir::new("app_check");
        let repo = dir.join("ws/mathlib");
        repo_at(&repo);
        std::fs::create_dir_all(repo.join("server")).unwrap();
        std::fs::write(repo.join("server/ok.txt"), "x").unwrap();
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-q", "-m", "server"]);
        let wts = dir.join("ws/mathlib-worktrees");
        let wt = |n: u32| wts.join(format!("dcv2-{RUN}-pr{n}"));
        for n in [1, 2, 4] {
            run(
                &repo,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    &format!("dcv2/{RUN}-pr{n}"),
                    wt(n).to_str().unwrap(),
                ],
            );
        }
        let docs = dir.join("ws/docs");
        repo_at(&docs);
        let docs_wt = dir.join(format!("ws/docs-worktrees/dcv2-{RUN}-pr3"));
        run(
            &docs,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &format!("dcv2/{RUN}-pr3"),
                docs_wt.to_str().unwrap(),
            ],
        );
        let stray = dir.join("stray");
        std::fs::create_dir_all(&stray).unwrap();
        let run_dir = dir.join("runs").join(RUN);
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(
            run_dir.join("repos.json"),
            json!({"schema": 1, "repos": [
                {"name": "mathlib", "path": repo, "check": "test -f ok.txt && echo fine"},
                {"name": "docs", "path": docs, "check": null}]})
            .to_string(),
        )
        .unwrap();
        let marker = dir.join("pwned");
        let status = json!({"schema": 1, "stage": "ship-approval", "prs": [
            // status.json's own command is never run.
            {"id": "PR1", "repo": "mathlib", "repoPath": repo, "worktree": wt(1),
             "check": {"command": format!("touch {}", marker.display()), "cwd": "server"}},
            {"id": "PR2", "repo": "mathlib", "repoPath": repo, "worktree": wt(2), "check": {"cwd": wt(2)}},
            // The check is found by the verified repo path, not the name.
            {"id": "PR3", "repo": "mathlib", "repoPath": docs, "worktree": docs_wt},
            {"id": "PR4", "repo": "mathlib", "repoPath": repo, "worktree": wt(4), "check": {"cwd": stray}},
            {"id": "PR5", "repo": "mathlib", "repoPath": repo, "worktree": stray},
            {"id": "PR6", "repo": "mathlib", "check": {"command": "true"}}
        ]});
        std::fs::write(run_dir.join("status.json"), status.to_string()).unwrap();
        rerun_checks(&run_dir, Duration::from_secs(30)).unwrap();
        let checks = read_app_checks(&run_dir);
        assert!(!marker.exists(), "status.json's command ran");
        assert_eq!(checks["PR1"].result, "pass", "{:?}", checks["PR1"]);
        assert_eq!(
            checks["PR1"].command.as_deref(),
            Some("test -f ok.txt && echo fine")
        );
        assert!(checks["PR1"]
            .skill_command
            .as_deref()
            .unwrap()
            .starts_with("touch"));
        assert_eq!(checks["PR1"].clean, Some(true));
        let head = crate::run::git::git(&wt(1), &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(checks["PR1"].head.as_deref(), Some(head.as_str()));
        assert_eq!(
            checks["PR2"].result, "fail",
            "the worktree root has no ok.txt"
        );
        assert_eq!(checks["PR3"].result, "none");
        assert_eq!(checks["PR3"].detail.as_deref(), Some("no check known"));
        assert_eq!(checks["PR4"].result, "error", "{:?}", checks["PR4"].detail);
        assert_eq!(checks["PR5"].result, "error");
        // No repoPath and no worktree: no repos.json check can be matched.
        assert_eq!(checks["PR6"].result, "none");
        let log = std::fs::read_to_string(run_dir.join("logs/PR1-app-check.txt")).unwrap();
        assert!(log.contains("fine"));
    }

    #[test]
    fn check_folders_stay_inside_the_worktree() {
        let dir = TempDir::new("check_folder");
        let wt = dir.join("wt");
        std::fs::create_dir_all(wt.join("server")).unwrap();
        assert_eq!(check_folder(None, &wt).unwrap(), wt.canonicalize().unwrap());
        assert!(check_folder(Some("server"), &wt)
            .unwrap()
            .ends_with("server"));
        assert!(check_folder(Some(".."), &wt).is_err());
        assert!(check_folder(Some("../wt/../.."), &wt).is_err());
        assert!(check_folder(Some(dir.path().to_str().unwrap()), &wt).is_err());
        assert!(check_folder(Some("missing"), &wt).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_check_that_runs_too_long_is_stopped() {
        let dir = TempDir::new("app_check_timeout");
        let started = Instant::now();
        let (result, _, _) = run_one(
            "sleep 20",
            dir.path(),
            &dir.join("log.txt"),
            Duration::from_millis(300),
        );
        assert_eq!(result, "timeout");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[cfg(unix)]
    #[test]
    fn a_log_planted_as_a_link_is_not_followed() {
        let dir = TempDir::new("app_check_log_link");
        let outside = dir.join("outside.txt");
        std::fs::write(&outside, "outside").unwrap();
        let log = dir.join("logs/PR1-app-check.txt");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &log).unwrap();
        let (result, _, detail) = run_one("echo hi", dir.path(), &log, Duration::from_secs(5));
        assert_eq!(result, "error");
        assert!(detail.unwrap().contains("log file"));
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "outside");
    }
    /// R1 for a stacked PR: its worktree (made from its parent's branch head) is checked like any other.
    #[cfg(unix)]
    #[test]
    fn a_stacked_pr_is_checked_in_its_own_worktree_made_from_its_parents_head() {
        let dir = TempDir::new("app_check_stack");
        let repo = dir.join("ws/mathlib");
        repo_at(&repo);
        let wts = dir.join("ws/mathlib-worktrees");
        let wt = |n: u32| wts.join(format!("dcv2-{RUN}-pr{n}"));
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &format!("dcv2/{RUN}-pr1"),
                wt(1).to_str().unwrap(),
            ],
        );
        std::fs::write(wt(1).join("parent.txt"), "from the parent").unwrap();
        run(&wt(1), &["add", "."]);
        run(&wt(1), &["commit", "-q", "-m", "parent"]);
        let parent_head = crate::run::git::git(&wt(1), &["rev-parse", "HEAD"]).unwrap();
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &format!("dcv2/{RUN}-pr2"),
                wt(2).to_str().unwrap(),
                &parent_head,
            ],
        );
        let stray = dir.join("stray");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "dcv2/not-this-run",
                stray.to_str().unwrap(),
                &parent_head,
            ],
        );
        let run_dir = dir.join("runs").join(RUN);
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(
            run_dir.join("repos.json"),
            json!({"schema": 1, "repos": [{"name": "mathlib", "path": repo, "check": "test -f parent.txt"}]})
                .to_string(),
        )
        .unwrap();
        std::fs::write(
            run_dir.join("status.json"),
            json!({"schema": 1, "stage": "ship-approval", "prs": [
                {"id": "PR1", "repo": "mathlib", "repoPath": repo, "worktree": wt(1), "check": {"cwd": wt(1)}},
                {"id": "PR2", "repo": "mathlib", "repoPath": repo, "worktree": wt(2), "stackedOn": "PR1", "check": {"cwd": wt(2)}},
                {"id": "PR3", "repo": "mathlib", "repoPath": repo, "worktree": stray, "check": {"cwd": stray}}
            ]})
            .to_string(),
        )
        .unwrap();
        rerun_checks(&run_dir, Duration::from_secs(30)).unwrap();
        let checks = read_app_checks(&run_dir);
        assert_eq!(checks["PR1"].result, "pass", "{:?}", checks["PR1"]);
        assert_eq!(
            checks["PR2"].result, "pass",
            "a stacked worktree is verified: {:?}",
            checks["PR2"]
        );
        assert_eq!(
            checks["PR3"].result, "error",
            "an off-path, off-branch worktree is still refused"
        );
    }
}
