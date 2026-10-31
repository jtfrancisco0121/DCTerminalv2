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
    /// The last Risk Reviewer round (`plan.risk`): its verdict and MEDIUM+ count.
    pub risk_verdict: Option<String>,
    pub risk_open_findings: Option<i64>,
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
    /// `pass`, `fail`, `timeout`, `error`, `none` (no check known),
    /// `running`, `interrupted` (the app stopped while it ran).
    pub result: String,
    /// The command the app ran: repos.json's, never status.json's.
    pub command: Option<String>,
    /// status.json's command when it differs from the app's (not run).
    #[serde(default)]
    pub skill_command: Option<String>,
    /// The worktree's HEAD when the check ran.
    #[serde(default)]
    pub head: Option<String>,
    /// The worktree had no uncommitted changes when the check ran.
    #[serde(default)]
    pub clean: Option<bool>,
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
    /// The check the app would run (repos.json), if any.
    pub app_command: Option<String>,
    /// repos.json names a check for this PR's repo.
    pub check_known: bool,
    /// The worktree's HEAD now.
    pub current_head: Option<String>,
    /// The worktree has no uncommitted changes now.
    pub worktree_clean: Option<bool>,
    /// The app's check ran on another HEAD than the worktree's now.
    pub app_check_stale: bool,
    /// `prs[].draft`: the PR is a draft on GitHub (review on GitHub).
    pub draft: Option<bool>,
    /// `prs[].comments`: one review comment per round, posted on the PR.
    pub comments: Vec<PrComment>,
    /// GitHub CI from `app-github.json` (app-written): `pass`, `fail`,
    /// `pending`, `none` or `unknown` (the last `gh` call failed); None when
    /// the PR was never read.
    pub ci: Option<String>,
    /// When the app read the PR on GitHub (UTC).
    pub ci_at: Option<String>,
    /// The last failed `gh` call's reason.
    pub ci_error: Option<String>,
    /// The PR's state on GitHub: `OPEN`, `MERGED` or `CLOSED`; None until read.
    pub pr_state: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrComment {
    pub round: Option<i64>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub time: Option<String>,
    pub stage: Option<String>,
    pub text: String,
}

/// BMAD planning (meta.json `bmad` for the mode and pin, status.json `bmad`
/// for the rest). None for a run whose mode is off or missing.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BmadView {
    /// `auto` or `on`.
    pub mode: String,
    /// The vendored BMAD pin the run started with (meta.json).
    pub version: Option<String>,
    /// `Small`, `Medium` or `Large`, once the sizing is recorded.
    pub size: Option<String>,
    pub reasons: Vec<String>,
    pub applied: bool,
    /// `pass` or `fail`, from the last `bmad-check`.
    pub check_result: Option<String>,
    /// Documents of the last check, relative to the run folder (openable
    /// with `read_run_file`).
    pub documents: Vec<String>,
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
    /// meta.json's `reviewMode` (`github` / `local`; missing is `local`).
    pub review_mode: String,
    /// meta.json's `mode` (`epic-plan`); `None` is a normal run.
    pub run_mode: Option<String>,
    /// status.json has `shipApproval` (the yes at stop #2).
    pub ship_approved: bool,
    /// BMAD planning, when the run's mode is auto or on.
    pub bmad: Option<BmadView>,
    /// What the app cleaned up for merged PRs (app-cleanup.json).
    pub cleanup: Vec<super::cleanup::CleanupRecord>,
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

/// `schema` is the integer 1 (not missing, `"1"`, `1.0` or `true`): for readers that
/// look into status.json without going through [`parse_status`].
pub fn has_known_schema(value: &Value) -> bool {
    value.get("schema").and_then(Value::as_u64) == Some(KNOWN_SCHEMA as u64)
}

/// status.json as the panel's model, tolerating partial documents.
pub fn parse_status(value: &Value) -> Result<RunView, String> {
    if !value.is_object() {
        return Err("status.json is not an object".into());
    }
    // Only the integer 1 is a schema: missing, "1", 1.0 and true are not.
    match value.get("schema") {
        Some(Value::Number(n)) if n.as_u64() == Some(KNOWN_SCHEMA as u64) => {}
        Some(Value::Number(n)) if n.as_u64().is_some_and(|v| v > KNOWN_SCHEMA as u64) => {
            return Err(format!(
                "status.json has schema {n}; this app knows schema {KNOWN_SCHEMA}. Update the app."
            ))
        }
        _ => {
            return Err(format!(
                "status.json has no valid schema (it must be the number {KNOWN_SCHEMA}), so the app does not read it."
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
            risk_verdict: p.get("risk").and_then(|r| s(r, "verdict")),
            risk_open_findings: p.get("risk").and_then(|r| i(r, "openFindings")),
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
        ship_approved: value.get("shipApproval").is_some_and(Value::is_object),
        bmad: parse_bmad(value),
        ..RunView::default()
    })
}

/// status.json's `bmad` object, tolerant of missing, partial or malformed
/// values (the mode and pin are filled in from meta.json by `read_run`).
fn parse_bmad(value: &Value) -> Option<BmadView> {
    let b = value.get("bmad").filter(|b| b.is_object())?;
    let check = obj(b, "check");
    // Only run-relative document paths the run panel can open: no absolute
    // paths, no `..`, no hidden parts.
    let documents = check
        .get("documents")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .filter(|p| {
                    !p.is_empty()
                        && Path::new(p).components().all(|c| {
                            matches!(c, std::path::Component::Normal(n) if !n.to_string_lossy().starts_with('.'))
                        })
                })
                .map(str::to_string)
                .take(20)
                .collect()
        })
        .unwrap_or_default();
    Some(BmadView {
        size: s(b, "size").filter(|z| matches!(z.as_str(), "Small" | "Medium" | "Large")),
        reasons: b
            .get("reasons")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(|r| r.chars().take(200).collect())
                    .take(10)
                    .collect()
            })
            .unwrap_or_default(),
        applied: b.get("applied").and_then(Value::as_bool).unwrap_or(false),
        check_result: s(check, "result").filter(|r| matches!(r.as_str(), "pass" | "fail")),
        documents,
        ..BmadView::default()
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
        draft: pr.get("draft").and_then(Value::as_bool),
        comments: pr
            .get("comments")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|c| {
                        let url = s(c, "url")?;
                        Some(PrComment {
                            round: i(c, "round"),
                            url,
                        })
                    })
                    .take(50)
                    .collect()
            })
            .unwrap_or_default(),
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
        let value = if value.starts_with('"') {
            serde_json::from_str::<String>(value)
                .unwrap_or_else(|_| value.trim_matches('"').to_string())
        } else {
            value
                .strip_prefix('\'')
                .and_then(|v| v.strip_suffix('\''))
                .unwrap_or(value)
                .to_string()
        };
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

/// What the green rule needs to know about a PR besides its gate.
#[derive(Debug, Clone, Default)]
pub struct Proof<'a> {
    pub app: Option<&'a AppCheck>,
    pub state: Option<&'a str>,
    pub check_known: bool,
    pub current_head: Option<&'a str>,
    pub worktree_clean: Option<bool>,
}

/// PR states in which the work is finished and may show green.
pub const FINISHED_STATES: &[&str] = &["reviewed", "opened", "pushed", "local-only"];

/// A1 + R1: green only when the gate is PASS/WAIVED, the PR is finished,
/// and the app's own check (repos.json's command) passed on the worktree's
/// current HEAD with no uncommitted changes then or now.
pub fn gate_status(gate: Option<&GateFile>, proof: &Proof) -> GateStatus {
    let status = |kind: &str, text: String| GateStatus {
        kind: kind.into(),
        text,
    };
    let Some(gate) = gate else {
        return status("none", "No gate yet".into());
    };
    if let Some(problem) = &gate.problem {
        return status("unreadable", format!("Gate unreadable: {problem}"));
    }
    let word = gate.gate.clone().unwrap_or_default();
    let reason = gate.reason.clone().unwrap_or_default();
    match word.as_str() {
        "PASS" | "WAIVED" => {
            if !proof.check_known {
                return status(
                    "review-only",
                    format!("{word} — review only (no check exists)"),
                );
            }
            let app = proof.app;
            let why = match app.map(|a| a.result.as_str()) {
                None => Some("the app has not re-run the check".to_string()),
                Some("pass") => None,
                Some("running") => Some("the app's check is running".to_string()),
                Some(result) => Some(format!("the app's check: {result}")),
            }
            .or_else(
                || match (app.and_then(|a| a.head.as_deref()), proof.current_head) {
                    (Some(then), Some(now)) if then == now => None,
                    (Some(_), Some(_)) => {
                        Some("the code changed since the app's check".to_string())
                    }
                    _ => Some("the worktree's HEAD is not known".to_string()),
                },
            )
            .or_else(|| {
                (app.and_then(|a| a.clean) != Some(true) || proof.worktree_clean != Some(true))
                    .then(|| "the worktree has uncommitted changes".to_string())
            })
            .or_else(|| {
                let state = proof.state.unwrap_or("unknown");
                (!FINISHED_STATES.contains(&state)).then(|| format!("the PR is {state}"))
            });
            match why {
                None => status("proven", format!("{word}, and the app's check passed")),
                Some(why) if reason.is_empty() => {
                    status("claimed", format!("{word} claimed; {why}"))
                }
                Some(why) => status("claimed", format!("{word} claimed ({reason}); {why}")),
            }
        }
        "CONCERNS" => status("concerns", format!("CONCERNS: {reason}")),
        _ => status("fail", format!("FAIL: {reason}")),
    }
}

/// repos.json's check per repo, keyed by the repo's canonical path (the
/// app wrote it; the skill never does).
pub fn repo_checks(run_dir: &Path) -> std::collections::BTreeMap<PathBuf, Option<String>> {
    read_json_value(&run_dir.join("repos.json"))
        .and_then(|v| v.get("repos").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|repo| {
            let path = s(repo, "path")?;
            let check = s(repo, "check").filter(|c| !c.trim().is_empty());
            Some((super::git::canonical(Path::new(&path)), check))
        })
        .collect()
}

/// The repos.json check for a PR: by its verified main checkout, else by
/// status.json's `repoPath` (display only; nothing runs unverified).
pub fn check_for(
    checks: &std::collections::BTreeMap<PathBuf, Option<String>>,
    main: Option<&Path>,
    repo_path: Option<&str>,
) -> Option<String> {
    let key = main
        .map(super::git::canonical)
        .or_else(|| repo_path.map(|p| super::git::canonical(Path::new(p))))?;
    checks.get(&key).cloned().flatten()
}

/// A verified worktree's HEAD and whether it has uncommitted changes
/// (ignored files do not count: build output is expected after a check).
pub fn worktree_state(worktree: &Path) -> (Option<String>, Option<bool>) {
    let head = super::git::git(worktree, &["rev-parse", "HEAD"]).ok();
    let clean = super::git::git(
        worktree,
        &["status", "--porcelain", "--untracked-files=normal"],
    )
    .ok()
    .map(|out| out.trim().is_empty());
    (head, clean)
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

/// Everything the panel shows for one run. `checks_running`: the app is
/// re-running this run's checks now (else a `running` result is stale).
pub fn read_run(run_dir: &Path, checks_running: bool) -> RunView {
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
    view.review_mode = super::folder::review_mode_of(run_dir);
    view.run_mode = super::folder::run_mode_of(run_dir);
    // BMAD: shown only for a run started with auto or on; an old run (no
    // `bmad` in meta.json) shows nothing.
    let bmad_mode = super::folder::bmad_mode_of(run_dir);
    view.bmad = if bmad_mode == "off" {
        None
    } else {
        let mut b = view.bmad.take().unwrap_or_default();
        b.mode = bmad_mode;
        b.version = super::folder::bmad_version_of(run_dir);
        Some(b)
    };
    if view.folder.is_none() {
        view.folder = super::folder::run_folder_of(run_dir);
    }
    let checks = read_app_checks(run_dir);
    let github = super::github::read_github(run_dir);
    let repo_checks = repo_checks(run_dir);
    for pr in &mut view.prs {
        pr.gate = read_gate(run_dir, &pr.id);
        if let Some(gh) = github.prs.get(&pr.id) {
            pr.ci = Some(gh.ci.clone());
            pr.ci_at = Some(gh.at.clone()).filter(|at| !at.is_empty());
            pr.ci_error = gh.error.clone();
            pr.pr_state = Some(gh.pr_state.clone()).filter(|s| !s.is_empty());
        }
        pr.app_check = checks.get(&pr.id).cloned().map(|mut check| {
            if check.result == "running" && !checks_running {
                check.result = "interrupted".into();
                check.detail = Some("the app stopped before this check finished".into());
            }
            check
        });
        // No git runs in a status.json path that is not this PR's worktree.
        let verified = pr.worktree.as_deref().and_then(|wt| {
            super::janitor::verify_worktree(
                &view.run_id,
                &pr.id,
                Path::new(wt),
                pr.repo_path.as_deref(),
            )
            .ok()
            .map(|main| (PathBuf::from(wt), main))
        });
        pr.app_command = check_for(
            &repo_checks,
            verified.as_ref().map(|(_, main)| main.as_path()),
            pr.repo_path.as_deref(),
        );
        pr.check_known = pr.app_command.is_some();
        let (head, clean) = verified
            .as_ref()
            .map(|(wt, _)| worktree_state(wt))
            .unwrap_or((None, None));
        pr.current_head = head;
        pr.worktree_clean = clean;
        pr.app_check_stale = pr.app_check.as_ref().is_some_and(|check| {
            check.head.is_some() && pr.current_head.is_some() && check.head != pr.current_head
        });
        pr.gate_status = Some(gate_status(
            pr.gate.as_ref(),
            &Proof {
                app: pr.app_check.as_ref(),
                state: pr.state.as_deref(),
                check_known: pr.check_known,
                current_head: pr.current_head.as_deref(),
                worktree_clean: pr.worktree_clean,
            },
        ));
    }
    view.timeline = std::fs::read_to_string(run_dir.join("log.md"))
        .map(|t| parse_log(&t))
        .unwrap_or_default();
    view.has_report = run_dir.join("report.md").is_file();
    view.cleanup = super::cleanup::read_cleanup(run_dir)
        .prs
        .into_values()
        .collect();
    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use serde_json::json;

    #[test]
    fn github_ci_and_pr_state_come_from_the_app_written_file_only() {
        let dir = TempDir::new("status_github");
        std::fs::write(
            dir.join("status.json"),
            json!({"schema": 1, "stage": "building", "prs": [
                {"id": "PR1", "prUrl": "https://github.com/o/r/pull/1", "ci": "pass", "prState": "MERGED"},
                {"id": "PR2"}]})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            dir.join("app-github.json"),
            json!({"schema": 1, "prs": {
                "PR1": {"ci": "fail", "prState": "OPEN", "at": "2026-10-27T14:00:00Z"},
                "PR2": {"ci": "unknown", "prState": "", "at": "2026-10-27T14:01:00Z", "error": "gh pr view: timed out"}}})
            .to_string(),
        )
        .unwrap();
        let view = read_run(dir.path(), false);
        let (one, two) = (&view.prs[0], &view.prs[1]);
        // status.json's own `ci` / `prState` keys are ignored; the app's file is the source.
        assert_eq!(one.ci.as_deref(), Some("fail"));
        assert_eq!(one.pr_state.as_deref(), Some("OPEN"));
        assert_eq!(one.ci_at.as_deref(), Some("2026-10-27T14:00:00Z"));
        assert_eq!(two.ci.as_deref(), Some("unknown"));
        assert_eq!(two.pr_state, None);
        assert_eq!(two.ci_error.as_deref(), Some("gh pr view: timed out"));
        let none = TempDir::new("status_github_none");
        std::fs::write(
            none.join("status.json"),
            json!({"schema": 1, "prs": [{"id": "PR1"}]}).to_string(),
        )
        .unwrap();
        assert_eq!(read_run(none.path(), false).prs[0].ci, None);
    }

    #[test]
    fn a_partial_status_with_unknown_fields_still_reads() {
        let view = parse_status(&json!({
            "schema": 1, "stage": "building", "somethingNew": {"x": 1},
            "plan": {"approved": true, "verdict": "APPROVED", "reviewRound": "2",
                     "risk": {"round": 2, "verdict": "RISKS FOUND", "openFindings": 1}},
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
        assert_eq!(plan.risk_verdict.as_deref(), Some("RISKS FOUND"));
        assert_eq!(plan.risk_open_findings, Some(1));
        assert_eq!(view.prs.len(), 2);
        assert_eq!(view.prs[0].state.as_deref(), Some("teleporting"));
        assert_eq!(view.prs[0].stories_done, Some(1));
        assert_eq!(view.prs[0].stories_total, None);
        assert_eq!(view.prs[0].check, CheckView::default());
        assert_eq!(view.prs[1].id, "PR2");
        assert!(parse_status(&json!({"schema": 2}))
            .unwrap_err()
            .contains("schema 2; this app knows schema 1. Update the app."));
        // Only the integer 1 is a schema (CAP-16).
        for bad in [
            json!({}),
            json!({"schema": "1"}),
            json!({"schema": 1.0}),
            json!({"schema": true}),
            json!({"schema": null}),
            json!({"schema": [1]}),
            json!({"schema": 0}),
            json!({"schema": -1}),
        ] {
            let err = parse_status(&bad).unwrap_err();
            assert!(
                err.contains("status.json has no valid schema (it must be the number 1), so the app does not read it."),
                "{bad}: {err}"
            );
        }
        assert!(parse_status(&json!({"schema": 1})).is_ok());
        assert!(has_known_schema(&json!({"schema": 1})));
        assert!(!has_known_schema(&json!({"schema": 1.0})));
        assert!(!has_known_schema(&json!({})));
        assert!(parse_status(&json!([1])).is_err());
    }

    #[test]
    fn gates_parse_and_are_green_only_with_a_fresh_clean_app_pass_on_a_finished_pr() {
        let gate = parse_gate(
            "gate: pass\nreason: \"all \\\"good\\\" \\u00e9\"\nupdated: 2026-10-22T13:00:00Z\n",
        );
        assert_eq!(gate.gate.as_deref(), Some("PASS"));
        assert_eq!(gate.reason.as_deref(), Some("all \"good\" \u{e9}"));
        assert!(gate.problem.is_none());
        let pass = AppCheck {
            result: "pass".into(),
            head: Some("abc".into()),
            clean: Some(true),
            ..AppCheck::default()
        };
        let ok = Proof {
            app: Some(&pass),
            state: Some("reviewed"),
            check_known: true,
            current_head: Some("abc"),
            worktree_clean: Some(true),
        };
        assert_eq!(gate_status(Some(&gate), &ok).kind, "proven");
        let kind = |proof: Proof| gate_status(Some(&gate), &proof);
        let moved = kind(Proof {
            current_head: Some("def"),
            ..ok.clone()
        });
        assert_eq!(moved.kind, "claimed");
        assert!(moved.text.contains("code changed"), "{}", moved.text);
        assert!(kind(Proof {
            worktree_clean: Some(false),
            ..ok.clone()
        })
        .text
        .contains("uncommitted"));
        assert!(kind(Proof {
            state: Some("fixing"),
            ..ok.clone()
        })
        .text
        .contains("the PR is fixing"));
        let fail = AppCheck {
            result: "fail".into(),
            ..pass.clone()
        };
        let claimed = kind(Proof {
            app: Some(&fail),
            ..ok.clone()
        });
        assert_eq!(claimed.kind, "claimed");
        assert!(claimed.text.contains("all"), "{}", claimed.text);
        assert_eq!(
            kind(Proof {
                app: None,
                ..ok.clone()
            })
            .kind,
            "claimed"
        );
        let review_only = kind(Proof {
            check_known: false,
            app: None,
            ..ok.clone()
        });
        assert_eq!(review_only.kind, "review-only");
        assert_eq!(review_only.text, "PASS — review only (no check exists)");
        assert_eq!(
            gate_status(Some(&parse_gate("gate: FAIL\n")), &ok).kind,
            "fail"
        );
        assert_eq!(
            gate_status(Some(&parse_gate("gate: CONCERNS\n")), &ok).kind,
            "concerns"
        );
        let broken = parse_gate("{{{ not yaml");
        assert_eq!(gate_status(Some(&broken), &ok).kind, "unreadable");
        assert_eq!(
            gate_status(Some(&parse_gate("gate: MAYBE")), &ok).kind,
            "unreadable"
        );
        assert_eq!(gate_status(None, &ok).kind, "none");
    }

    #[test]
    fn the_run_mode_comes_from_meta_json_and_only_when_known() {
        let dir = TempDir::new("run_mode");
        let run = dir.join("r-1");
        std::fs::create_dir_all(&run).unwrap();
        assert_eq!(read_run(&run, false).run_mode, None);
        std::fs::write(run.join("meta.json"), r#"{"mode":"epic-plan"}"#).unwrap();
        assert_eq!(read_run(&run, false).run_mode.as_deref(), Some("epic-plan"));
        std::fs::write(run.join("meta.json"), r#"{"mode":"bogus"}"#).unwrap();
        assert_eq!(read_run(&run, false).run_mode, None);
    }

    #[test]
    fn a_run_folder_reads_into_one_view_ignoring_temp_files() {
        let dir = TempDir::new("run_read");
        let repo = dir.join("ws/a");
        crate::run::git::testing::repo_at(&repo);
        let wt = dir.join("ws/a-worktrees/dcv2-r-20261022-1304-pr1");
        crate::run::git::testing::run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "dcv2/r-20261022-1304-pr1",
                wt.to_str().unwrap(),
            ],
        );
        let b = dir.join("ws/b");
        crate::run::git::testing::repo_at(&b);
        let wt_b = dir.join("ws/b-worktrees/dcv2-r-20261022-1304-pr4");
        crate::run::git::testing::run(
            &b,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "dcv2/r-20261022-1304-pr4",
                wt_b.to_str().unwrap(),
            ],
        );
        let head = crate::run::git::git(&wt, &["rev-parse", "HEAD"]).unwrap();
        let run = dir.join("r-20261022-1304");
        std::fs::create_dir_all(run.join("gates")).unwrap();
        std::fs::write(
            run.join("status.json"),
            json!({"schema": 1, "stage": "ship-approval", "waitingFor": "ship-approval",
                   "prs": [{"id": "PR1", "repo": "a", "repoPath": repo, "state": "reviewed", "worktree": wt},
                           {"id": "PR2", "repo": "a", "repoPath": repo, "state": "reviewed"},
                           {"id": "../x"},
                           // Named "a" but in repo b: b's (null) check applies.
                           {"id": "PR4", "repo": "a", "repoPath": b, "state": "reviewed", "worktree": wt_b},
                           // A path that is not this PR's worktree: no git runs there.
                           {"id": "PR5", "repo": "a", "repoPath": repo, "state": "reviewed", "worktree": repo}]})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            run.join("repos.json"),
            json!({"schema": 1, "repos": [{"name": "a", "path": repo, "check": "npm test"}, {"name": "b", "path": b, "check": null}]}).to_string(),
        )
        .unwrap();
        std::fs::write(run.join(".status.json.abc.tmp"), "{half").unwrap();
        std::fs::write(run.join("gates/PR1.yml"), "gate: PASS\nreason: ok\n").unwrap();
        std::fs::write(run.join("gates/PR2.yml"), "garbage").unwrap();
        std::fs::write(run.join("gates/PR4.yml"), "gate: PASS\n").unwrap();
        std::fs::write(
            run.join("log.md"),
            "2026-10-22T13:04:05Z planning Planner wrote plan.md: 3 stories\nloose line\n",
        )
        .unwrap();
        crate::store::write_json_atomic(
            &app_checks_path(&run),
            &json!({"schema": 1, "checks": {
                "PR1": {"result": "pass", "head": head, "clean": true},
                "PR2": {"result": "running"}}}),
        )
        .unwrap();
        let view = read_run(&run, false);
        assert_eq!(view.run_id, "r-20261022-1304");
        assert_eq!(view.waiting_for.as_deref(), Some("ship-approval"));
        let kind = |n: usize| view.prs[n].gate_status.as_ref().unwrap().kind.clone();
        assert_eq!(kind(0), "proven", "{:?}", view.prs[0].gate_status);
        assert_eq!(view.prs[0].app_command.as_deref(), Some("npm test"));
        assert!(!view.prs[0].app_check_stale);
        assert_eq!(kind(1), "unreadable");
        assert_eq!(
            view.prs[1].app_check.as_ref().unwrap().result,
            "interrupted"
        );
        assert_eq!(
            read_run(&run, true).prs[1]
                .app_check
                .as_ref()
                .unwrap()
                .result,
            "running"
        );
        assert_eq!(kind(2), "none");
        assert_eq!(kind(3), "review-only");
        assert!(!view.prs[3].check_known);
        assert_eq!(view.prs[4].current_head, None, "unverified worktree");
        assert_eq!(view.prs[4].app_command.as_deref(), Some("npm test"));
        assert_eq!(view.timeline.len(), 2);
        assert_eq!(view.timeline[0].stage.as_deref(), Some("planning"));
        assert_eq!(view.timeline[1].time, None);
        assert!(!view.has_report);

        // A new commit makes the app's check stale and the gate claimed.
        std::fs::write(wt.join("more.txt"), "x").unwrap();
        crate::run::git::testing::run(&wt, &["add", "."]);
        crate::run::git::testing::run(&wt, &["commit", "-q", "-m", "more"]);
        let moved = read_run(&run, false);
        assert!(moved.prs[0].app_check_stale);
        assert_eq!(moved.prs[0].gate_status.as_ref().unwrap().kind, "claimed");

        std::fs::write(run.join("status.json"), "{\"stage\": ").unwrap();
        let broken = read_run(&run, false);
        assert!(broken.status_problem.is_some());
        assert!(!broken.has_status);
        let empty = read_run(&dir.join("none"), false);
        assert!(!empty.has_status && empty.status_problem.is_none());
    }
}

#[cfg(test)]
mod draft_pr_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn draft_prs_and_their_review_comments_are_read_tolerantly() {
        let view = parse_status(&json!({
            "schema": 1, "stage": "reviewing", "shipApproval": null,
            "prs": [
                {"id": "PR1", "state": "drafted", "draft": true, "prUrl": "https://github.com/o/r/pull/7",
                 "comments": [{"round": 1, "url": "https://github.com/o/r/pull/7#issuecomment-1"},
                              {"round": "2", "url": "https://github.com/o/r/pull/7#issuecomment-2"},
                              {"round": 3}, "junk", {"url": 5}]},
                {"id": "PR2", "draft": "yes", "comments": {"round": 1}},
                {"id": "PR3", "state": "opened", "draft": false}
            ]
        }))
        .unwrap();
        assert!(!view.ship_approved);
        let one = &view.prs[0];
        assert_eq!(one.draft, Some(true));
        assert_eq!(one.state.as_deref(), Some("drafted"));
        assert_eq!(one.comments.len(), 3, "{:?}", one.comments);
        assert_eq!(one.comments[1].round, Some(2));
        assert_eq!(one.comments[2].url, "5");
        assert_eq!(view.prs[1].draft, None);
        assert!(view.prs[1].comments.is_empty());
        assert_eq!(view.prs[2].draft, Some(false));
        let approved =
            parse_status(&json!({"schema": 1, "shipApproval": {"by": "user", "at": "t"}})).unwrap();
        assert!(approved.ship_approved);
        // An old run (no reviewMode in meta.json) reads as local.
        assert_eq!(
            read_run(Path::new("/nonexistent/r-1"), false).review_mode,
            "local"
        );
    }

    #[test]
    fn a_bmad_object_is_read_and_a_missing_or_malformed_one_is_survived() {
        let view = parse_status(&json!({"schema": 1, "bmad": {
            "mode": "auto", "size": "Large", "applied": true,
            "reasons": ["more than one repo", 7, "new data model"],
            "docs": ["prd"],
            "check": {"result": "pass", "digest": "x", "documents": [
                "bmad/output/r-1/spec-a/spec-a.md", "/etc/passwd", "../x.md", "bmad/.hidden/a.md", 3
            ]}
        }}))
        .unwrap();
        let b = view.bmad.expect("bmad read");
        assert_eq!(b.size.as_deref(), Some("Large"));
        assert!(b.applied);
        assert_eq!(b.reasons, vec!["more than one repo", "new data model"]);
        assert_eq!(b.check_result.as_deref(), Some("pass"));
        assert_eq!(b.documents, vec!["bmad/output/r-1/spec-a/spec-a.md"]);
        // Missing, null, a string, or odd field types: no panic, no error.
        for v in [
            json!({"schema": 1}),
            json!({"schema": 1, "bmad": null}),
            json!({"schema": 1, "bmad": "on"}),
            json!({"schema": 1, "bmad": []}),
        ] {
            assert!(parse_status(&v).unwrap().bmad.is_none(), "{v}");
        }
        let odd = parse_status(
            &json!({"schema": 1, "bmad": {"size": "Huge", "applied": "yes", "reasons": "x",
            "check": {"result": 5, "documents": "no"}}}),
        )
        .unwrap()
        .bmad
        .unwrap();
        assert_eq!(odd, BmadView::default());
    }

    #[test]
    fn the_run_view_shows_bmad_only_for_auto_and_on() {
        let dir = std::env::temp_dir().join(format!("dcv2-bmad-{}", std::process::id()));
        let run = dir.join("r-20261027-000000-abcd");
        std::fs::create_dir_all(&run).unwrap();
        // No meta.json (an old run): nothing, even when status.json has a bmad object.
        std::fs::write(
            run.join("status.json"),
            r#"{"schema":1,"stage":"planning","bmad":{"size":"Large","applied":true}}"#,
        )
        .unwrap();
        assert!(read_run(&run, false).bmad.is_none());
        std::fs::write(
            run.join("meta.json"),
            r#"{"bmad":{"mode":"off","version":"v"}}"#,
        )
        .unwrap();
        assert!(read_run(&run, false).bmad.is_none());
        std::fs::write(
            run.join("meta.json"),
            r#"{"bmad":{"mode":"auto","version":"bda3c59 6.13.0-next"}}"#,
        )
        .unwrap();
        let b = read_run(&run, false).bmad.expect("shown");
        assert_eq!(b.mode, "auto");
        assert_eq!(b.version.as_deref(), Some("bda3c59 6.13.0-next"));
        assert_eq!(b.size.as_deref(), Some("Large"));
        // Mode on with no status.json bmad yet: the badge still shows.
        std::fs::write(run.join("meta.json"), r#"{"bmad":{"mode":"on"}}"#).unwrap();
        std::fs::write(
            run.join("status.json"),
            r#"{"schema":1,"stage":"planning"}"#,
        )
        .unwrap();
        let b = read_run(&run, false).bmad.expect("shown");
        assert_eq!((b.mode.as_str(), b.size), ("on", None));
        let _ = std::fs::remove_dir_all(&dir);
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
        let mut view = read_run(&root.join("r-20261022-1304"), false);
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
        assert_eq!(
            kinds,
            ["claimed", "claimed", "unreadable", "none", "review-only"]
        );
    }
}
