//! Reading a run for the progress panel: `status.json` (tolerant: missing
//! fields are null, unknown fields and values pass through as-is),
//! `gates/<PR>.yml`, `log.md`, the app's own check results and
//! `report.md`. Dot-files and `*.tmp` are never read.

use super::folder::read_json_value;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The status.json schema this app understands (run-folder.md).
pub const KNOWN_SCHEMA: i64 = 1;
/// The timeline keeps at most this many of the newest log lines.
pub const MAX_LOG_LINES: usize = 300;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanView {
    pub approved: bool,
    pub verdict: Option<String>,
    pub review_round: Option<i64>,
    pub open_findings: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CheckView {
    pub command: Option<String>,
    pub result: Option<String>,
    pub summary: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewView {
    pub round: Option<i64>,
    pub verdict: Option<String>,
    pub open_findings: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GateFile {
    pub gate: Option<String>,
    pub reason: Option<String>,
    pub updated: Option<String>,
    /// Set when the file exists but could not be read or has no `gate:`.
    pub problem: Option<String>,
}

/// The app's own re-run of a PR's check (R1).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppCheck {
    /// `pass`, `fail`, `timeout`, `error`, `none` (no command), `running`.
    pub result: String,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub exit_code: Option<i32>,
    pub finished_at: Option<String>,
    /// Relative to the run folder.
    pub log: Option<String>,
    pub detail: Option<String>,
}

/// What the panel shows for a PR's done-proof (A1 + R1).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GateStatus {
    /// `proven` (green), `claimed`, `concerns`, `fail`, `unreadable`, `none`.
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrView {
    pub id: String,
    pub title: Option<String>,
    pub repo: Option<String>,
    pub repo_path: Option<String>,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub state: Option<String>,
    pub note: Option<String>,
    pub stories_done: Option<i64>,
    pub stories_total: Option<i64>,
    pub check: CheckView,
    pub review: ReviewView,
    pub pr_url: Option<String>,
    pub gate: Option<GateFile>,
    pub app_check: Option<AppCheck>,
    pub gate_status: Option<GateStatus>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub time: Option<String>,
    pub stage: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    pub run_id: String,
    pub run_dir: String,
    pub folder: Option<String>,
    /// status.json exists and was read.
    pub has_status: bool,
    /// Why status.json could not be used (unreadable, unknown schema).
    pub status_problem: Option<String>,
    pub stage: Option<String>,
    pub waiting_for: Option<String>,
    pub paused_reason: Option<String>,
    pub error: Option<String>,
    pub updated_at: Option<String>,
    pub plan: Option<PlanView>,
    pub prs: Vec<PrView>,
    pub ship_note: Option<String>,
    pub timeline: Vec<LogLine>,
    pub has_report: bool,
}

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| match x {
        Value::String(text) => Some(text.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    })
}

fn i(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(|x| {
        x.as_i64()
            .or_else(|| x.as_f64().map(|f| f as i64))
            .or_else(|| x.as_str().and_then(|t| t.trim().parse().ok()))
    })
}

fn obj<'a>(v: &'a Value, key: &str) -> &'a Value {
    static NULL: Value = Value::Null;
    v.get(key).filter(|x| x.is_object()).unwrap_or(&NULL)
}

/// A PR id safe to use in a file name (`PR1`, `PR-2`).
pub fn valid_pr_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// status.json as the panel's model, tolerating partial documents.
pub fn parse_status(value: &Value) -> Result<RunView, String> {
    if !value.is_object() {
        return Err("status.json is not an object".into());
    }
    match value.get("schema").and_then(Value::as_i64) {
        Some(KNOWN_SCHEMA) | None => {}
        Some(other) => {
            return Err(format!(
            "status.json has schema {other}; this app knows schema {KNOWN_SCHEMA}. Update the app."
        ))
        }
    }
    let plan = value
        .get("plan")
        .filter(|p| p.is_object())
        .map(|p| PlanView {
            approved: p.get("approved").and_then(Value::as_bool).unwrap_or(false)
                || p.get("approval").is_some_and(|a| a.is_object()),
            verdict: s(p, "verdict"),
            review_round: i(p, "reviewRound"),
            open_findings: i(p, "openFindings"),
        });
    let prs = value
        .get("prs")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter(|pr| pr.is_object())
                .enumerate()
                .map(|(n, pr)| parse_pr(pr, n))
                .collect()
        })
        .unwrap_or_default();
    let ship = obj(value, "ship");
    Ok(RunView {
        has_status: true,
        stage: s(value, "stage"),
        waiting_for: s(value, "waitingFor"),
        paused_reason: s(obj(value, "paused"), "reason"),
        error: s(value, "error"),
        updated_at: s(value, "updatedAt"),
        folder: s(value, "folder"),
        plan,
        prs,
        ship_note: s(ship, "note"),
        ..RunView::default()
    })
}

fn parse_pr(pr: &Value, index: usize) -> PrView {
    let stories = obj(pr, "stories");
    let check = obj(pr, "check");
    let review = obj(pr, "review");
    PrView {
        id: s(pr, "id").unwrap_or_else(|| format!("PR{}", index + 1)),
        title: s(pr, "title"),
        repo: s(pr, "repo"),
        repo_path: s(pr, "repoPath"),
        branch: s(pr, "branch"),
        worktree: s(pr, "worktree"),
        state: s(pr, "state"),
        note: s(pr, "note"),
        stories_done: i(stories, "done"),
        stories_total: i(stories, "total"),
        check: CheckView {
            command: s(check, "command"),
            result: s(check, "result"),
            summary: s(check, "summary"),
            cwd: s(check, "cwd"),
        },
        review: ReviewView {
            round: i(review, "round"),
            verdict: s(review, "verdict"),
            open_findings: i(review, "openFindings"),
        },
        pr_url: s(pr, "prUrl"),
        ..PrView::default()
    }
}

/// `gates/<PR>.yml`: flat `key: value` lines (quotes stripped). None when
/// the file does not exist; a file that cannot be used says why.
pub fn read_gate(run_dir: &Path, pr_id: &str) -> Option<GateFile> {
    if !valid_pr_id(pr_id) {
        return None;
    }
    let path = run_dir.join("gates").join(format!("{pr_id}.yml"));
    if std::fs::symlink_metadata(&path).is_err() {
        return None;
    }
    Some(match std::fs::read_to_string(&path) {
        Ok(text) => parse_gate(&text),
        Err(err) => GateFile {
            problem: Some(format!("gates/{pr_id}.yml could not be read: {err}")),
            ..GateFile::default()
        },
    })
}

pub fn parse_gate(text: &str) -> GateFile {
    let mut gate = GateFile::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if line.starts_with([' ', '\t', '#', '-']) {
            continue;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value)
            .to_string();
        let value = (!value.is_empty()).then_some(value);
        match key.trim() {
            "gate" => gate.gate = value.map(|v| v.to_ascii_uppercase()),
            "reason" => gate.reason = value,
            "updated" => gate.updated = value,
            _ => {}
        }
    }
    match gate.gate.as_deref() {
        Some("PASS" | "CONCERNS" | "FAIL" | "WAIVED") => {}
        Some(other) => gate.problem = Some(format!("unknown gate value {other:?}")),
        None => gate.problem = Some("no gate: line".into()),
    }
    gate
}

/// A1 + R1: green only when the gate is PASS/WAIVED and the app's own
/// check re-run passed.
pub fn gate_status(gate: Option<&GateFile>, app: Option<&AppCheck>) -> GateStatus {
    let reason = |g: &GateFile| g.reason.clone().unwrap_or_default();
    let Some(gate) = gate else {
        return GateStatus {
            kind: "none".into(),
            text: "No gate yet".into(),
        };
    };
    if let Some(problem) = &gate.problem {
        return GateStatus {
            kind: "unreadable".into(),
            text: format!("Gate unreadable: {problem}"),
        };
    }
    let word = gate.gate.clone().unwrap_or_default();
    match word.as_str() {
        "PASS" | "WAIVED" => match app.map(|a| a.result.as_str()) {
            Some("pass") => GateStatus {
                kind: "proven".into(),
                text: format!("{word}, and the app's check passed"),
            },
            other => {
                let why = match other {
                    None => "the app has not re-run the check".to_string(),
                    Some("none") => "no check command to re-run".to_string(),
                    Some("running") => "the app's check is running".to_string(),
                    Some(result) => format!("the app's check: {result}"),
                };
                let reason = reason(gate);
                let text = if reason.is_empty() {
                    format!("{word} claimed; {why}")
                } else {
                    format!("{word} claimed ({reason}); {why}")
                };
                GateStatus {
                    kind: "claimed".into(),
                    text,
                }
            }
        },
        "CONCERNS" => GateStatus {
            kind: "concerns".into(),
            text: format!("CONCERNS: {}", reason(gate)),
        },
        _ => GateStatus {
            kind: "fail".into(),
            text: format!("FAIL: {}", reason(gate)),
        },
    }
}

/// `log.md` lines: `<UTC time> <stage> <text>`; other lines kept as text.
pub fn parse_log(text: &str) -> Vec<LogLine> {
    let mut lines: Vec<LogLine> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut parts = line.splitn(3, ' ');
            let time = parts.next().unwrap_or_default();
            let looks_time = time.len() >= 19 && time.as_bytes()[4] == b'-' && time.contains('T');
            if looks_time {
                let stage = parts.next().map(str::to_string);
                let rest = parts.next().unwrap_or_default().to_string();
                LogLine {
                    time: Some(time.to_string()),
                    stage,
                    text: rest,
                }
            } else {
                LogLine {
                    time: None,
                    stage: None,
                    text: line.to_string(),
                }
            }
        })
        .collect();
    if lines.len() > MAX_LOG_LINES {
        lines.drain(..lines.len() - MAX_LOG_LINES);
    }
    lines
}

pub fn app_checks_path(run_dir: &Path) -> PathBuf {
    run_dir.join("app-checks.json")
}

/// The app's check results by PR id.
pub fn read_app_checks(run_dir: &Path) -> std::collections::BTreeMap<String, AppCheck> {
    read_json_value(&app_checks_path(run_dir))
        .and_then(|v| v.get("checks").cloned())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

/// Everything the panel shows for one run.
pub fn read_run(run_dir: &Path) -> RunView {
    let run_id = run_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let status_path = run_dir.join("status.json");
    let mut view = if status_path.is_file() {
        match std::fs::read_to_string(&status_path)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()))
            .and_then(|v| parse_status(&v))
        {
            Ok(view) => view,
            Err(problem) => RunView {
                status_problem: Some(problem),
                ..RunView::default()
            },
        }
    } else {
        RunView::default()
    };
    view.run_id = run_id;
    view.run_dir = run_dir.display().to_string();
    if view.folder.is_none() {
        view.folder = super::folder::run_folder_of(run_dir);
    }
    let checks = read_app_checks(run_dir);
    for pr in &mut view.prs {
        pr.gate = read_gate(run_dir, &pr.id);
        pr.app_check = checks.get(&pr.id).cloned();
        pr.gate_status = Some(gate_status(pr.gate.as_ref(), pr.app_check.as_ref()));
    }
    view.timeline = std::fs::read_to_string(run_dir.join("log.md"))
        .map(|t| parse_log(&t))
        .unwrap_or_default();
    view.has_report = run_dir.join("report.md").is_file();
    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use serde_json::json;

    #[test]
    fn a_partial_status_with_unknown_fields_still_reads() {
        let view = parse_status(&json!({
            "stage": "building", "somethingNew": {"x": 1},
            "plan": {"approved": true, "verdict": "APPROVED", "reviewRound": "2"},
            "prs": [
                {"id": "PR1", "repo": "mathlib", "state": "teleporting", "stories": {"done": 1}, "check": "oops"},
                {"title": "no id"},
                "not an object"
            ]
        }))
        .unwrap();
        assert_eq!(view.stage.as_deref(), Some("building"));
        assert_eq!(view.waiting_for, None);
        let plan = view.plan.unwrap();
        assert!(plan.approved);
        assert_eq!(plan.review_round, Some(2));
        assert_eq!(view.prs.len(), 2);
        assert_eq!(view.prs[0].state.as_deref(), Some("teleporting"));
        assert_eq!(view.prs[0].stories_done, Some(1));
        assert_eq!(view.prs[0].stories_total, None);
        assert_eq!(view.prs[0].check, CheckView::default());
        assert_eq!(view.prs[1].id, "PR2");
        assert!(parse_status(&json!({"schema": 2}))
            .unwrap_err()
            .contains("schema 2"));
        assert!(parse_status(&json!([1])).is_err());
    }

    #[test]
    fn gates_parse_and_are_green_only_with_the_apps_own_pass() {
        let gate = parse_gate("gate: pass\nreason: \"all good\"\nupdated: 2026-10-22T13:00:00Z\nreview: reviews/PR1-round1.md\n");
        assert_eq!(gate.gate.as_deref(), Some("PASS"));
        assert_eq!(gate.reason.as_deref(), Some("all good"));
        assert!(gate.problem.is_none());
        let pass = AppCheck {
            result: "pass".into(),
            ..AppCheck::default()
        };
        let fail = AppCheck {
            result: "fail".into(),
            ..AppCheck::default()
        };
        assert_eq!(gate_status(Some(&gate), Some(&pass)).kind, "proven");
        let claimed = gate_status(Some(&gate), Some(&fail));
        assert_eq!(claimed.kind, "claimed");
        assert!(claimed.text.contains("all good"), "{}", claimed.text);
        assert_eq!(gate_status(Some(&gate), None).kind, "claimed");
        let waived = parse_gate("gate: WAIVED\nreason: no tests in repo\n");
        assert_eq!(gate_status(Some(&waived), None).kind, "claimed");
        assert_eq!(
            gate_status(Some(&parse_gate("gate: FAIL\n")), Some(&pass)).kind,
            "fail"
        );
        assert_eq!(
            gate_status(Some(&parse_gate("gate: CONCERNS\n")), Some(&pass)).kind,
            "concerns"
        );
        let broken = parse_gate("{{{ not yaml");
        assert!(broken.problem.is_some());
        assert_eq!(gate_status(Some(&broken), Some(&pass)).kind, "unreadable");
        assert_eq!(
            gate_status(Some(&parse_gate("gate: MAYBE")), Some(&pass)).kind,
            "unreadable"
        );
        assert_eq!(gate_status(None, Some(&pass)).kind, "none");
    }

    #[test]
    fn a_run_folder_reads_into_one_view_ignoring_temp_files() {
        let dir = TempDir::new("run_read");
        let run = dir.join("r-20261022-1304");
        std::fs::create_dir_all(run.join("gates")).unwrap();
        std::fs::write(
            run.join("status.json"),
            json!({"schema": 1, "stage": "ship-approval", "waitingFor": "ship-approval",
                   "prs": [{"id": "PR1", "state": "reviewed"}, {"id": "PR2", "state": "reviewed"}, {"id": "../x"}]})
            .to_string(),
        )
        .unwrap();
        std::fs::write(run.join(".status.json.abc.tmp"), "{half").unwrap();
        std::fs::write(run.join("gates/PR1.yml"), "gate: PASS\nreason: ok\n").unwrap();
        std::fs::write(run.join("gates/PR2.yml"), "garbage").unwrap();
        std::fs::write(
            run.join("log.md"),
            "2026-10-22T13:04:05Z planning Planner wrote plan.md: 3 stories\nloose line\n",
        )
        .unwrap();
        crate::store::write_json_atomic(
            &app_checks_path(&run),
            &json!({"schema": 1, "checks": {"PR1": {"result": "pass"}}}),
        )
        .unwrap();
        let view = read_run(&run);
        assert_eq!(view.run_id, "r-20261022-1304");
        assert_eq!(view.waiting_for.as_deref(), Some("ship-approval"));
        assert_eq!(view.prs[0].gate_status.as_ref().unwrap().kind, "proven");
        assert_eq!(view.prs[1].gate_status.as_ref().unwrap().kind, "unreadable");
        assert_eq!(view.prs[2].gate_status.as_ref().unwrap().kind, "none");
        assert_eq!(view.timeline.len(), 2);
        assert_eq!(view.timeline[0].stage.as_deref(), Some("planning"));
        assert_eq!(view.timeline[1].time, None);
        assert!(!view.has_report);

        std::fs::write(run.join("status.json"), "{\"stage\": ").unwrap();
        let broken = read_run(&run);
        assert!(broken.status_problem.is_some());
        assert!(!broken.has_status);
        let empty = read_run(&dir.join("none"));
        assert!(!empty.has_status && empty.status_problem.is_none());
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::*;

    /// `fixtures/run/<run>/` read into the view the page's tests render
    /// (`fixtures/run/run-view.json`). Set `DCTV2_UPDATE_FIXTURES=1` to rewrite it.
    #[test]
    fn the_fixture_run_reads_into_the_view_the_page_tests_use() {
        let root = crate::run::launch::dev_repo_root().join("fixtures/run");
        let mut view = read_run(&root.join("r-20261022-1304"));
        view.run_dir = String::new();
        let actual = serde_json::to_value(&view).unwrap();
        let expected_path = root.join("run-view.json");
        if std::env::var_os("DCTV2_UPDATE_FIXTURES").is_some() {
            let text = serde_json::to_string_pretty(&actual).unwrap() + "\n";
            std::fs::write(&expected_path, text).unwrap();
        }
        let expected: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&expected_path).unwrap()).unwrap();
        assert_eq!(actual, expected);
        let kinds: Vec<&str> = view
            .prs
            .iter()
            .map(|pr| pr.gate_status.as_ref().unwrap().kind.as_str())
            .collect();
        assert_eq!(kinds, ["proven", "claimed", "unreadable", "none"]);
    }
}
