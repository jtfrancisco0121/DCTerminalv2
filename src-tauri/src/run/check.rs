//! R1: before stop #2 the app re-runs each PR's check itself, in the PR's
//! worktree (or `check.cwd` inside it), with a timeout. Full output goes to
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

/// The folder a check runs in: `check.cwd`, else the worktree. A `cwd`
/// outside the worktree is refused.
pub fn check_folder(cwd: Option<&str>, worktree: Option<&str>) -> Result<PathBuf, String> {
    let worktree = worktree.map(PathBuf::from);
    let folder = cwd
        .map(PathBuf::from)
        .or_else(|| worktree.clone())
        .ok_or("the PR has no worktree yet")?;
    if !folder.is_dir() {
        return Err(format!("{} does not exist", folder.display()));
    }
    if let Some(worktree) = worktree {
        let inside = super::git::canonical(&folder).starts_with(super::git::canonical(&worktree));
        if !inside {
            return Err(format!(
                "check folder {} is outside the PR's worktree",
                folder.display()
            ));
        }
    }
    Ok(folder)
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
    let file = match std::fs::File::create(log) {
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
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let code = status.code();
                let result = if status.success() { "pass" } else { "fail" };
                return (result.into(), code, None);
            }
            Ok(None) if started.elapsed() >= timeout => {
                #[cfg(unix)]
                {
                    let _ = Command::new("kill")
                        .args(["-9", &format!("-{}", child.id())])
                        .stderr(Stdio::null())
                        .status();
                }
                let _ = child.kill();
                let _ = child.wait();
                return (
                    "timeout".into(),
                    None,
                    Some(format!("stopped after {} s", timeout.as_secs())),
                );
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(err) => return ("error".into(), None, Some(err.to_string())),
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
        let command = pr.check.command.clone().filter(|c| !c.trim().is_empty());
        let mut result = AppCheck {
            command: command.clone(),
            ..AppCheck::default()
        };
        match (
            command,
            check_folder(pr.check.cwd.as_deref(), pr.worktree.as_deref()),
        ) {
            (None, _) => {
                result.result = "none".into();
                result.detail = Some("no check command".into());
            }
            (Some(_), Err(problem)) => {
                result.result = "error".into();
                result.detail = Some(problem);
            }
            (Some(command), Ok(cwd)) => {
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
    use crate::test_support::TempDir;

    #[cfg(unix)]
    #[test]
    fn the_app_reruns_each_check_in_its_worktree_and_records_the_result() {
        let dir = TempDir::new("app_check");
        let run = dir.join("run");
        let wt1 = dir.join("wt1");
        let wt2 = dir.join("wt2");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::create_dir_all(&wt1).unwrap();
        std::fs::create_dir_all(&wt2).unwrap();
        std::fs::write(wt1.join("ok.txt"), "x").unwrap();
        let status = json!({"schema": 1, "stage": "ship-approval", "prs": [
            {"id": "PR1", "worktree": wt1, "check": {"command": "test -f ok.txt && echo fine"}},
            {"id": "PR2", "worktree": wt2, "check": {"command": "echo broken; exit 3"}},
            {"id": "PR3", "worktree": wt2, "check": {"command": null}},
            {"id": "PR4", "worktree": wt2, "check": {"command": "true", "cwd": dir.path()}},
            {"id": "PR5", "check": {"command": "true"}}
        ]});
        std::fs::write(run.join("status.json"), status.to_string()).unwrap();
        rerun_checks(&run, Duration::from_secs(30)).unwrap();
        let checks = read_app_checks(&run);
        assert_eq!(checks["PR1"].result, "pass");
        assert_eq!(checks["PR2"].result, "fail");
        assert_eq!(checks["PR2"].exit_code, Some(3));
        assert_eq!(checks["PR3"].result, "none");
        assert_eq!(checks["PR4"].result, "error", "{:?}", checks["PR4"].detail);
        assert_eq!(checks["PR5"].result, "error");
        let log = std::fs::read_to_string(run.join("logs/PR2-app-check.txt")).unwrap();
        assert!(log.contains("broken"));
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
}
