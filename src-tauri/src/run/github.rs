//! GitHub CI and PR state per PR of a run (plan 2026-10-27, CAP-7). One
//! sweep, in the cleanup thread right after `cleanup::sweep` and under the
//! same lock, so at most one `gh` call runs app-wide. For each PR with a
//! `prUrl` that passes the cleanup's URL and repo checks it runs one
//! read-only `gh pr view <url> -R <owner/name> --json
//! state,headRefName,statusCheckRollup` (with a timeout) and records the
//! result in `<run>/app-github.json`, which only the app writes (guard and
//! `state.sh put` refuse it), whatever `run.autoCleanup` says.
//!
//! status.json is written by the LLM-driven skill, so every URL and branch
//! from it is checked against the run's contract (`cleanup::pr_target`, and
//! the head must be the run's own `dcv2/<runId>-<pr>`) before it is trusted.

use super::cleanup::{self, run_with_timeout, CALL_TIMEOUT, MAX_AGE_DAYS};
use super::folder::{append_app_log, read_json_value};
use super::janitor;
use super::status::PrView;
use crate::store::write_json_atomic;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const GITHUB_FILE: &str = "app-github.json";
/// One run is asked about at most this often.
pub const ASK_EVERY: Duration = Duration::from_secs(5 * 60);
/// At most this many `gh` calls per pass, so the cleanup cadence is not starved.
pub const MAX_CALLS_PER_PASS: usize = 10;

/// What the app knows of one PR on GitHub.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrGithub {
    /// `pass`, `fail`, `pending`, `none`, or `unknown` (the last call failed).
    pub ci: String,
    /// `OPEN`, `MERGED`, `CLOSED`; empty when never read.
    #[serde(default)]
    pub pr_state: String,
    pub at: String,
    /// The last failed call's one-line reason.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GithubFile {
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub prs: BTreeMap<String, PrGithub>,
}

pub fn read_github(run_dir: &Path) -> GithubFile {
    read_json_value(&run_dir.join(GITHUB_FILE))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

/// What `gh pr view --json state,headRefName,statusCheckRollup` said.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GhPr {
    pub state: String,
    pub head_ref_name: String,
    /// `statusCheckRollup`: None when the field was absent.
    pub rollup: Option<Vec<Value>>,
}

/// Asks GitHub about one PR (`gh` in the app; a fake in tests).
pub trait CiLookup {
    fn view(&self, url: &str, repo: &str) -> Result<GhPr, String>;
}

/// `gh pr view`, no prompts, read-only, with a timeout.
pub struct GhCi;

impl CiLookup for GhCi {
    fn view(&self, url: &str, repo: &str) -> Result<GhPr, String> {
        let mut cmd = crate::platform::quiet_command("gh");
        cmd.args([
            "pr",
            "view",
            url,
            "-R",
            repo,
            "--json",
            "state,headRefName,statusCheckRollup",
        ])
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1");
        let text = run_with_timeout(cmd, CALL_TIMEOUT).map_err(|e| format!("gh pr view: {e}"))?;
        let v: Value =
            serde_json::from_str(&text).map_err(|_| "gh pr view printed no JSON".to_string())?;
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        Ok(GhPr {
            state: s("state"),
            head_ref_name: s("headRefName"),
            rollup: v
                .get("statusCheckRollup")
                .and_then(Value::as_array)
                .cloned(),
        })
    }
}

/// `ci` from a `statusCheckRollup`. Any failed conclusion (FAILURE, ERROR,
/// CANCELLED, TIMED_OUT, ACTION_REQUIRED) or failed status (FAILURE, ERROR)
/// is `fail`; else any check not COMPLETED, PENDING or EXPECTED, or of a
/// shape the app does not know, is `pending`; else `pass`. No checks (or no
/// field at all) is `none`.
pub fn map_rollup(rollup: Option<&[Value]>) -> &'static str {
    let Some(items) = rollup.filter(|items| !items.is_empty()) else {
        return "none";
    };
    let text = |item: &Value, key: &str| {
        item.get(key)
            .and_then(Value::as_str)
            .map(str::to_ascii_uppercase)
            .unwrap_or_default()
    };
    let mut pending = false;
    for item in items {
        let (status, conclusion, state) = (
            text(item, "status"),
            text(item, "conclusion"),
            text(item, "state"),
        );
        if matches!(
            conclusion.as_str(),
            "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED"
        ) || matches!(state.as_str(), "FAILURE" | "ERROR")
        {
            return "fail";
        }
        let finished = if !status.is_empty() {
            status == "COMPLETED"
                && matches!(conclusion.as_str(), "SUCCESS" | "NEUTRAL" | "SKIPPED")
        } else {
            state == "SUCCESS"
        };
        if !finished {
            pending = true;
        }
    }
    if pending {
        "pending"
    } else {
        "pass"
    }
}

fn now_text() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// A MERGED or CLOSED PR whose CI is final is not asked again.
fn settled(rec: &PrGithub) -> bool {
    matches!(rec.pr_state.as_str(), "MERGED" | "CLOSED")
        && matches!(rec.ci.as_str(), "pass" | "fail" | "none")
}

/// One PR's new record: from `gh`, or `unknown` with the reason.
fn ask(run_dir: &Path, pr: &PrView, old: Option<&PrGithub>, lookup: &dyn CiLookup) -> PrGithub {
    let failed = |why: String| PrGithub {
        // A lookup error keeps the last known CI, so fail -> error -> fail is not a new edge.
        ci: old
            .map(|o| o.ci.clone())
            .unwrap_or_else(|| "unknown".into()),
        pr_state: old.map(|o| o.pr_state.clone()).unwrap_or_default(),
        at: now_text(),
        error: Some(why),
    };
    let Some(url) = pr.pr_url.as_deref() else {
        return failed("no PR URL".into());
    };
    let slug = match cleanup::pr_target(run_dir, &pr.id, url) {
        Ok((_, slug)) => slug,
        Err(why) => return failed(why),
    };
    let run_id = run_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    match lookup.view(url, &slug) {
        Err(err) => failed(err),
        Ok(info) => {
            let branch = janitor::expected_branch(&run_id, &pr.id);
            if info.head_ref_name != branch {
                return failed(format!(
                    "the PR's head is {}, not {branch}",
                    info.head_ref_name
                ));
            }
            let pr_state = match info.state.as_str() {
                s @ ("OPEN" | "MERGED" | "CLOSED") => s.to_string(),
                _ => String::new(),
            };
            PrGithub {
                ci: map_rollup(info.rollup.as_deref()).into(),
                pr_state,
                at: now_text(),
                error: None,
            }
        }
    }
}

/// Asks about the PRs of one run (at most `budget` calls); saves the file
/// when anything was read. Returns (calls made, every PR was handled).
fn ask_run(run_dir: &Path, lookup: &dyn CiLookup, budget: usize) -> (usize, bool) {
    let (_, _, mut prs) = cleanup::prs_with_url(run_dir);
    let mut file = read_github(run_dir);
    // Never-read PRs first, then the longest unread: a run with more PRs
    // than the call limit is read in turns, not the same ten every time.
    prs.sort_by_key(|pr| file.prs.get(&pr.id).map(|rec| rec.at.clone()));
    let (mut calls, mut all, mut changed) = (0, true, false);
    for pr in &prs {
        let old = file.prs.get(&pr.id).cloned();
        if old.as_ref().is_some_and(settled) {
            continue;
        }
        if calls >= budget {
            all = false;
            break;
        }
        calls += 1;
        let next = ask(run_dir, pr, old.as_ref(), lookup);
        if let Some(err) = &next.error {
            // Logged once per outcome, not on every retry.
            if old.as_ref().and_then(|o| o.error.as_ref()) != Some(err) {
                let _ = append_app_log(
                    run_dir,
                    &format!("github {}: could not read the PR: {err}", pr.id),
                );
            }
        }
        if old.as_ref() != Some(&next) {
            changed = true;
        }
        file.prs.insert(pr.id.clone(), next);
    }
    if changed {
        file.schema = 1;
        if let Err(err) = write_json_atomic(&run_dir.join(GITHUB_FILE), &file) {
            eprintln!("DCTerminalv2: {GITHUB_FILE}: {err}");
        }
    }
    (calls, all)
}

/// One pass over the app's own runs (adopted ones are the dev app's)
/// changed within 30 days: each run not asked within `every`, at most
/// [`MAX_CALLS_PER_PASS`] `gh` calls in all, under the cleanup lock. A run
/// cut short by the call limit is asked again on the next pass. Returns how
/// many calls were made.
pub fn sweep(
    data: &Path,
    lookup: &dyn CiLookup,
    asked: &mut HashMap<PathBuf, Instant>,
    every: Duration,
) -> usize {
    let now = chrono::Utc::now();
    let mut calls = 0;
    // The dependency runs of an open epic are asked about whatever their age: a wave waits on their merges.
    let pinned = super::epics::dependency_runs(data);
    for run in super::folder::list_runs(data) {
        if calls >= MAX_CALLS_PER_PASS {
            break;
        }
        let recent = run
            .updated_at
            .as_deref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .is_some_and(|t| now.signed_duration_since(t) <= chrono::Duration::days(MAX_AGE_DAYS));
        if run.adopted || (!recent && !pinned.contains(&run.run_dir)) {
            continue;
        }
        let dir = PathBuf::from(&run.run_dir);
        if asked.get(&dir).is_some_and(|at| at.elapsed() < every) {
            continue;
        }
        // The same lock as the cleanup: one `gh` call app-wide.
        let _lock = cleanup::CLEANUP_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (made, all) = ask_run(&dir, lookup, MAX_CALLS_PER_PASS - calls);
        calls += made;
        if all {
            asked.insert(dir, Instant::now());
        }
    }
    calls
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use serde_json::json;
    use std::cell::RefCell;

    const RUN: &str = "r-20261027-0900-abcd";

    #[derive(Default)]
    struct Fake {
        prs: HashMap<String, Result<GhPr, String>>,
        calls: RefCell<Vec<String>>,
    }

    impl CiLookup for Fake {
        fn view(&self, url: &str, repo: &str) -> Result<GhPr, String> {
            self.calls.borrow_mut().push(format!("{repo} {url}"));
            self.prs
                .get(url)
                .cloned()
                .unwrap_or_else(|| Err("no such PR".into()))
        }
    }

    fn url(n: u32) -> String {
        format!("https://github.com/o/r/pull/{n}")
    }

    fn gh(state: &str, n: u32, rollup: Option<Vec<Value>>) -> Result<GhPr, String> {
        Ok(GhPr {
            state: state.into(),
            head_ref_name: format!("dcv2/{RUN}-pr{n}"),
            rollup,
        })
    }

    /// A run at `stage` with PRs 1..=n, each with a prUrl, and the recorded remote.
    fn setup(stage: &str, n: u32) -> (TempDir, PathBuf, PathBuf) {
        let dir = TempDir::new("github");
        let data = dir.join("data");
        let run_dir = data.join("runs").join(RUN);
        std::fs::create_dir_all(&run_dir).unwrap();
        let prs: Vec<Value> = (1..=n)
            .map(|i| json!({"id": format!("PR{i}"), "state": "opened", "prUrl": url(i)}))
            .collect();
        std::fs::write(
            run_dir.join("status.json"),
            json!({"schema": 1, "stage": stage, "updatedAt": now_text(), "prs": prs}).to_string(),
        )
        .unwrap();
        let remotes: serde_json::Map<String, Value> = (1..=n)
            .map(|i| {
                (
                    format!("PR{i}"),
                    json!({"url": "https://github.com/o/r.git", "repo": "o/r"}),
                )
            })
            .collect();
        std::fs::write(
            run_dir.join(".guard.json"),
            json!({"remotes": remotes}).to_string(),
        )
        .unwrap();
        (dir, data, run_dir)
    }

    fn check(status: &str, conclusion: &str) -> Value {
        json!({"__typename": "CheckRun", "status": status, "conclusion": conclusion})
    }

    #[test]
    fn the_rollup_maps_to_pass_fail_pending_or_none() {
        let m = |items: Vec<Value>| map_rollup(Some(&items));
        assert_eq!(map_rollup(None), "none");
        assert_eq!(m(vec![]), "none");
        assert_eq!(m(vec![check("COMPLETED", "SUCCESS")]), "pass");
        assert_eq!(
            m(vec![
                check("COMPLETED", "SKIPPED"),
                check("COMPLETED", "NEUTRAL")
            ]),
            "pass"
        );
        for bad in [
            "FAILURE",
            "ERROR",
            "CANCELLED",
            "TIMED_OUT",
            "ACTION_REQUIRED",
        ] {
            assert_eq!(
                m(vec![check("COMPLETED", "SUCCESS"), check("COMPLETED", bad)]),
                "fail",
                "{bad}"
            );
        }
        // Failed beats pending.
        assert_eq!(
            m(vec![
                check("IN_PROGRESS", ""),
                check("COMPLETED", "FAILURE")
            ]),
            "fail"
        );
        for running in ["IN_PROGRESS", "QUEUED", "PENDING", "WAITING"] {
            assert_eq!(
                m(vec![check("COMPLETED", "SUCCESS"), check(running, "")]),
                "pending",
                "{running}"
            );
        }
        // Commit statuses (StatusContext) carry `state`.
        assert_eq!(m(vec![json!({"state": "SUCCESS"})]), "pass");
        assert_eq!(m(vec![json!({"state": "PENDING"})]), "pending");
        assert_eq!(m(vec![json!({"state": "EXPECTED"})]), "pending");
        assert_eq!(m(vec![json!({"state": "FAILURE"})]), "fail");
        assert_eq!(m(vec![json!({"state": "ERROR"})]), "fail");
        // Unknown shapes are pending, never pass.
        assert_eq!(m(vec![json!({"something": "new"})]), "pending");
        assert_eq!(
            m(vec![check("COMPLETED", "STARTUP_FAILURE_NEW")]),
            "pending"
        );
    }

    #[test]
    fn a_pr_with_a_url_is_recorded_and_asked_again_only_after_the_cadence() {
        let (_dir, data, run_dir) = setup("building", 1);
        let mut fake = Fake::default();
        fake.prs.insert(
            url(1),
            gh("OPEN", 1, Some(vec![check("COMPLETED", "SUCCESS")])),
        );
        let mut asked = HashMap::new();
        assert_eq!(sweep(&data, &fake, &mut asked, ASK_EVERY), 1);
        assert_eq!(
            fake.calls.borrow().as_slice(),
            ["o/r https://github.com/o/r/pull/1"]
        );
        let rec = read_github(&run_dir).prs["PR1"].clone();
        assert_eq!((rec.ci.as_str(), rec.pr_state.as_str()), ("pass", "OPEN"));
        assert!(rec.error.is_none());
        // Within the cadence: nothing.
        assert_eq!(sweep(&data, &fake, &mut asked, ASK_EVERY), 0);
        // Past it: asked again (an OPEN PR is never settled).
        assert_eq!(sweep(&data, &fake, &mut asked, Duration::ZERO), 1);
    }

    #[test]
    fn runs_with_auto_cleanup_off_are_still_read_and_adopted_runs_are_not() {
        // The sweep takes no cleanup setting at all.
        let (_dir, data, run_dir) = setup("done", 1);
        let mut fake = Fake::default();
        fake.prs.insert(url(1), gh("OPEN", 1, None));
        let mut asked = HashMap::new();
        assert_eq!(sweep(&data, &fake, &mut asked, ASK_EVERY), 1);
        assert_eq!(read_github(&run_dir).prs["PR1"].ci, "none");
        // An adopted run (listed in adopted-runs.json) belongs to the dev app.
        let (d2, data2, run2) = setup("building", 1);
        let elsewhere = d2.join("dev-data/runs").join(RUN);
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::fs::rename(&run2, &elsewhere).unwrap();
        std::fs::write(
            data2.join("adopted-runs.json"),
            json!({"runs": [elsewhere.display().to_string()]}).to_string(),
        )
        .unwrap();
        assert_eq!(
            super::super::folder::list_runs(&data2).len(),
            1,
            "it is listed"
        );
        let fake2 = Fake::default();
        assert_eq!(sweep(&data2, &fake2, &mut HashMap::new(), ASK_EVERY), 0);
    }

    #[test]
    fn a_run_not_changed_within_thirty_days_is_skipped() {
        let (_dir, data, run_dir) = setup("building", 1);
        let old = (chrono::Utc::now() - chrono::Duration::days(40))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        std::fs::write(
            run_dir.join("status.json"),
            json!({"schema": 1, "stage": "building", "updatedAt": old,
                   "prs": [{"id": "PR1", "prUrl": url(1)}]})
            .to_string(),
        )
        .unwrap();
        let fake = Fake::default();
        assert_eq!(sweep(&data, &fake, &mut HashMap::new(), ASK_EVERY), 0);
    }

    #[test]
    fn a_merged_or_closed_pr_with_a_final_ci_is_not_asked_again_but_pending_is() {
        let (_dir, data, run_dir) = setup("done", 2);
        let mut fake = Fake::default();
        fake.prs.insert(
            url(1),
            gh("MERGED", 1, Some(vec![check("COMPLETED", "SUCCESS")])),
        );
        fake.prs.insert(
            url(2),
            gh("CLOSED", 2, Some(vec![check("IN_PROGRESS", "")])),
        );
        assert_eq!(sweep(&data, &fake, &mut HashMap::new(), Duration::ZERO), 2);
        let file = read_github(&run_dir);
        assert_eq!(file.prs["PR1"].pr_state, "MERGED");
        // PR1 is settled; PR2 closed with a still pending CI is asked again.
        fake.calls.borrow_mut().clear();
        assert_eq!(sweep(&data, &fake, &mut HashMap::new(), Duration::ZERO), 1);
        assert_eq!(
            fake.calls.borrow().as_slice(),
            ["o/r https://github.com/o/r/pull/2"]
        );
    }

    #[test]
    fn a_failed_call_is_recorded_as_unknown_with_the_reason_and_asked_again() {
        let (_dir, data, run_dir) = setup("building", 1);
        let mut fake = Fake::default();
        fake.prs
            .insert(url(1), Err("gh pr view: could not resolve host".into()));
        assert_eq!(sweep(&data, &fake, &mut HashMap::new(), Duration::ZERO), 1);
        let rec = read_github(&run_dir).prs["PR1"].clone();
        assert_eq!(rec.ci, "unknown");
        assert!(rec
            .error
            .as_deref()
            .unwrap()
            .contains("could not resolve host"));
        // The next pass asks again and a good answer replaces it.
        fake.prs.insert(
            url(1),
            gh("OPEN", 1, Some(vec![check("COMPLETED", "FAILURE")])),
        );
        assert_eq!(sweep(&data, &fake, &mut HashMap::new(), Duration::ZERO), 1);
        let rec = read_github(&run_dir).prs["PR1"].clone();
        assert_eq!(rec.ci, "fail");
        assert!(rec.error.is_none());
        let log = std::fs::read_to_string(run_dir.join("log.md")).unwrap();
        assert_eq!(log.matches("github PR1: could not read").count(), 1);
    }

    #[test]
    fn a_gh_error_keeps_the_last_known_ci() {
        let (_dir, data, run_dir) = setup("building", 1);
        let mut fake = Fake::default();
        fake.prs.insert(
            url(1),
            gh("OPEN", 1, Some(vec![check("COMPLETED", "FAILURE")])),
        );
        sweep(&data, &fake, &mut HashMap::new(), Duration::ZERO);
        fake.prs.insert(url(1), Err("gh pr view: offline".into()));
        sweep(&data, &fake, &mut HashMap::new(), Duration::ZERO);
        let rec = read_github(&run_dir).prs["PR1"].clone();
        assert_eq!(rec.ci, "fail");
        assert!(rec.error.is_some());
    }

    #[test]
    fn a_forged_url_or_a_foreign_head_is_never_trusted() {
        let (_dir, data, run_dir) = setup("building", 2);
        // PR1's URL points at another repo; PR2's head is not the run's branch.
        std::fs::write(
            run_dir.join("status.json"),
            json!({"schema": 1, "stage": "building", "updatedAt": now_text(), "prs": [
                {"id": "PR1", "prUrl": "https://github.com/evil/other/pull/9"},
                {"id": "PR2", "prUrl": url(2)}]})
            .to_string(),
        )
        .unwrap();
        let mut fake = Fake::default();
        fake.prs.insert(
            url(2),
            Ok(GhPr {
                state: "OPEN".into(),
                head_ref_name: "main".into(),
                rollup: Some(vec![check("COMPLETED", "SUCCESS")]),
            }),
        );
        sweep(&data, &fake, &mut HashMap::new(), Duration::ZERO);
        assert_eq!(
            fake.calls.borrow().len(),
            1,
            "the forged URL is never asked"
        );
        let file = read_github(&run_dir);
        assert_eq!(file.prs["PR1"].ci, "unknown");
        assert_eq!(file.prs["PR2"].ci, "unknown");
        assert!(file.prs["PR2"]
            .error
            .as_deref()
            .unwrap()
            .contains("head is main"));
    }

    #[test]
    fn at_most_ten_calls_per_pass_and_a_cut_short_run_is_continued() {
        let (_dir, data, run_dir) = setup("building", 12);
        let mut fake = Fake::default();
        for i in 1..=12 {
            fake.prs.insert(url(i), gh("OPEN", i, None));
        }
        let mut asked = HashMap::new();
        assert_eq!(
            sweep(&data, &fake, &mut asked, ASK_EVERY),
            MAX_CALLS_PER_PASS
        );
        assert_eq!(read_github(&run_dir).prs.len(), 10);
        assert!(asked.is_empty(), "not marked asked: it was cut short");
        // The next pass reads the two never-read PRs first.
        sweep(&data, &fake, &mut asked, ASK_EVERY);
        assert_eq!(read_github(&run_dir).prs.len(), 12);
    }

    #[test]
    fn a_dependency_run_of_an_open_epic_older_than_thirty_days_is_still_asked_about() {
        let (_d, data, run) = setup("done", 1);
        let old = (chrono::Utc::now() - chrono::Duration::days(MAX_AGE_DAYS + 5))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        let mut status: Value =
            serde_json::from_str(&std::fs::read_to_string(run.join("status.json")).unwrap())
                .unwrap();
        status["updatedAt"] = json!(old);
        std::fs::write(run.join("status.json"), status.to_string()).unwrap();
        let fake = Fake {
            prs: HashMap::from([(url(1), gh("MERGED", 1, None))]),
            ..Default::default()
        };
        assert_eq!(
            sweep(&data, &fake, &mut HashMap::new(), Duration::ZERO),
            0,
            "too old, no epic: not asked"
        );
        // An epic-plan run (done) that started wave 1 as this run, with a wave 2 still to do, pins it.
        let epic = data.join("runs").join("r-epic");
        std::fs::create_dir_all(&epic).unwrap();
        std::fs::write(
            epic.join("meta.json"),
            json!({"mode": "epic-plan"}).to_string(),
        )
        .unwrap();
        std::fs::write(
            epic.join("status.json"),
            json!({"stage": "done", "updatedAt": now_text(), "prs": []}).to_string(),
        )
        .unwrap();
        std::fs::write(
            epic.join("waves.json"),
            json!({"waves": [
                {"n": 1, "title": "a", "tickets": ["t1"], "dependsOn": []},
                {"n": 2, "title": "b", "tickets": ["t2"], "dependsOn": [1]}
            ]})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            epic.join("tickets.json"),
            json!({"tasks": [{"id": "t1"}, {"id": "t2"}]}).to_string(),
        )
        .unwrap();
        let mut state = super::super::dispatch::Dispatch::default();
        state.epics.push(super::super::dispatch::Epic {
            run_dir: epic.display().to_string(),
            wave_runs: vec![super::super::dispatch::WaveRun {
                n: 1,
                entry_id: "q".into(),
                run_dir: Some(run.display().to_string()),
            }],
            ..Default::default()
        });
        super::super::dispatch::save(&data, &state).unwrap();
        assert_eq!(
            sweep(&data, &fake, &mut HashMap::new(), Duration::ZERO),
            1,
            "pinned by the open epic"
        );
    }
}
