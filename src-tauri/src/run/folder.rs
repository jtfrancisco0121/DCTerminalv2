//! The run folder the app creates (run-folder.md): `request.md`,
//! `repos.json` and `meta.json`, under `<app data>/runs/<runId>/`. The app
//! never writes a `test-mode:` line and never sets `DCV2_TEST_MODE`.

use super::repos::Repo;
use crate::store::settings_store::MAX_IMPLEMENTERS_LIMIT;
use crate::store::write_json_atomic;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Keys `state.sh preflight` reads at the start of a line in request.md.
const RECOGNISED_KEYS: &[&str] = &["folder", "repos", "test-mode"];

/// The longest request copied into request.md (the file stays short).
pub const MAX_REQUEST_CHARS: usize = 20_000;

pub fn runs_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("runs")
}

/// A line that looks like a recognised key (any case, `_` for `-`, spaces
/// before the colon, leading blanks or quote marks) is quoted with `> `.
fn looks_like_key(line: &str) -> bool {
    let trimmed =
        line.trim_start_matches(|c: char| c.is_whitespace() || c == '>' || c == '-' || c == '*');
    let Some((key, _)) = trimmed.split_once(':') else {
        return false;
    };
    let key: String = key
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c == '_' || c == ' ' { '-' } else { c })
        .collect();
    let key = key.trim_matches(|c: char| c == '`' || c == '"' || c == '\'' || c == '*');
    RECOGNISED_KEYS.contains(&key) || key == "testmode"
}

/// User or ticket text made safe for request.md: key-like lines quoted, no
/// NUL or other control characters but tab, capped in size.
pub fn neutralise(text: &str) -> String {
    let clean: String = text
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(MAX_REQUEST_CHARS)
        .collect();
    clean
        .lines()
        .map(|line| {
            if looks_like_key(line) {
                format!("> {line}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The whole request.md: `folder:` exactly once, `repos:` when limited.
pub fn request_md(folder: &Path, repos_limit: &[String], request: &str) -> String {
    let mut out = format!("# Request\n\nfolder: {}\n", folder.display());
    let limit: Vec<&str> = repos_limit
        .iter()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty() && !name.contains([',', '\n', ':']))
        .collect();
    if !limit.is_empty() {
        out.push_str(&format!("repos: {}\n", limit.join(", ")));
    }
    out.push_str("\n## What to do\n\n");
    out.push_str(neutralise(request).trim());
    out.push('\n');
    out
}

/// The words after `## What to do` in a request.md.
pub fn request_md_body(text: &str) -> &str {
    text.split("## What to do").nth(1).unwrap_or(text)
}

/// `r-YYYYMMDD-HHMM`, with `-2`, `-3`, … when that folder exists.
pub fn new_run_id(runs: &Path, now: chrono::DateTime<chrono::Utc>) -> String {
    // Seconds and a random suffix: an id is never reused, so no record kept
    // for an earlier run (the app's run-trust folder) can match a new run.
    let base = format!("r-{}", now.format("%Y%m%d-%H%M%S"));
    let trust = runs
        .parent()
        .map(|data| data.join(crate::clickup::trust::TRUST_DIR));
    let taken =
        |id: &str| runs.join(id).exists() || trust.as_ref().is_some_and(|t| t.join(id).exists());
    loop {
        let suffix: String = uuid::Uuid::new_v4().simple().to_string()[..4].to_string();
        let id = format!("{base}-{suffix}");
        if !taken(&id) {
            return id;
        }
    }
}

/// What a new run records about itself.
#[derive(Debug, Clone, Default)]
pub struct RunMeta {
    pub plugin_version: Option<String>,
    pub claude_version: Option<String>,
    pub account_id: String,
    pub account_name: String,
    pub config_dir: String,
    /// `github` (draft PRs + review comments) or `local`; `None` is not
    /// recorded (the skill then runs the local flow).
    pub review_mode: Option<String>,
    /// `auto`, `on` or `off` (BMAD planning); `None` is not recorded (the
    /// skill reads a missing `bmad` as off).
    pub bmad_mode: Option<String>,
    /// First line of `plugin/bmad/VERSION`, recorded with the mode.
    pub bmad_version: Option<String>,
    /// This run's `DCV2_MAX_IMPLEMENTERS` override (1..=8), set on the start
    /// screen; `None` is not recorded (the setting applies).
    pub max_implementers: Option<u32>,
    /// `epic-plan`: the run only plans an epic in waves (no code, no PR); `None`
    /// is a normal run and is not recorded.
    pub mode: Option<String>,
    /// The queue entry (dispatch.json) that started this run; a restart reads it to tell the entry started.
    pub dispatch_entry_id: Option<String>,
}

/// The run modes meta.json can record (`mode`); a normal run has none.
pub const RUN_MODES: &[&str] = &["epic-plan"];

/// The review modes meta.json can record.
pub const REVIEW_MODES: &[&str] = &["github", "local"];

/// The BMAD planning modes meta.json can record.
pub const BMAD_MODES: &[&str] = &["auto", "on", "off"];

/// meta.json's `maxImplementers` (the start screen's override), when it is an
/// integer in 1..=8; anything else is `None` and the setting applies.
pub fn max_implementers_of(run_dir: &Path) -> Option<u32> {
    read_json_value(&run_dir.join("meta.json"))?
        .get("maxImplementers")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| (1..=MAX_IMPLEMENTERS_LIMIT).contains(n))
}

/// The repos a run is limited to, from the ticked names. `None` or every
/// detected repo ticked → no limit (an empty list, so request.md gets no
/// `repos:` line). A name that was not detected, or none ticked, is refused.
pub fn repos_limit_for(
    detected: &[String],
    ticked: Option<&[String]>,
) -> Result<Vec<String>, String> {
    let Some(ticked) = ticked else {
        return Ok(Vec::new());
    };
    if ticked.is_empty() {
        return Err("Tick at least one repo.".into());
    }
    if let Some(unknown) = ticked.iter().find(|name| !detected.contains(name)) {
        return Err(format!("{unknown} is not a repo of this folder."));
    }
    let picked: Vec<String> = detected
        .iter()
        .filter(|name| ticked.contains(name))
        .cloned()
        .collect();
    if picked.len() == detected.len() {
        Ok(Vec::new())
    } else {
        Ok(picked)
    }
}

/// meta.json's `bmad.mode`: `auto`, `on` or `off`; missing or unknown → `off`.
pub fn bmad_mode_of(run_dir: &Path) -> String {
    read_json_value(&run_dir.join("meta.json"))
        .and_then(|m| {
            m.get("bmad")
                .and_then(|b| b.get("mode"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|m| BMAD_MODES.contains(&m.as_str()))
        .unwrap_or_else(|| "off".into())
}

/// meta.json's `bmad.version` (the vendored BMAD pin the run started with).
pub fn bmad_version_of(run_dir: &Path) -> Option<String> {
    read_json_value(&run_dir.join("meta.json"))
        .and_then(|m| {
            m.get("bmad")
                .and_then(|b| b.get("version"))
                .and_then(Value::as_str)
                .map(|v| v.chars().take(80).collect::<String>())
        })
        .filter(|v| !v.is_empty())
}

/// meta.json's `reviewMode`: `github` or `local`; missing or unknown → `local`.
pub fn review_mode_of(run_dir: &Path) -> String {
    read_json_value(&run_dir.join("meta.json"))
        .and_then(|m| {
            m.get("reviewMode")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|m| REVIEW_MODES.contains(&m.as_str()))
        .unwrap_or_else(|| "local".into())
}

/// meta.json's `mode`: `epic-plan`, or `None` for a normal run (missing or unknown).
pub fn run_mode_of(run_dir: &Path) -> Option<String> {
    read_json_value(&run_dir.join("meta.json"))
        .and_then(|m| m.get("mode").and_then(Value::as_str).map(str::to_string))
        .filter(|m| RUN_MODES.contains(&m.as_str()))
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreatedRun {
    pub run_id: String,
    pub run_dir: String,
}

/// Makes `<data>/runs/<runId>/` with request.md, repos.json, meta.json.
pub fn create_run(
    data_dir: &Path,
    folder: &Path,
    repos: &[Repo],
    repos_limit: &[String],
    request: &str,
    meta: &RunMeta,
) -> Result<CreatedRun, String> {
    if request.trim().is_empty() {
        return Err("Say what should be done.".into());
    }
    let runs = runs_dir(data_dir);
    std::fs::create_dir_all(&runs).map_err(|e| format!("runs folder: {e}"))?;
    let now = chrono::Utc::now();
    let run_id = new_run_id(&runs, now);
    let dir = runs.join(&run_id);
    std::fs::create_dir(&dir).map_err(|e| format!("run folder: {e}"))?;
    let dir = dir.canonicalize().unwrap_or(dir);
    crate::store::write_run_file(
        &dir.join("request.md"),
        request_md(folder, repos_limit, request).as_bytes(),
    )?;
    write_json_atomic(
        &dir.join("repos.json"),
        &json!({"schema": 1, "repos": repos}),
    )?;
    let mut meta_json = json!({
        "schema": 1,
        "runId": run_id,
        "createdAt": now.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        "folder": folder.display().to_string(),
        "pluginVersion": meta.plugin_version,
        "claudeVersion": meta.claude_version,
        "account": {"id": meta.account_id, "name": meta.account_name, "configDir": meta.config_dir},
        "app": format!("dcterminalv2 {}", env!("CARGO_PKG_VERSION")),
    });
    if let Some(mode) = meta
        .review_mode
        .as_deref()
        .filter(|m| REVIEW_MODES.contains(m))
    {
        meta_json["reviewMode"] = json!(mode);
    }
    if let Some(mode) = meta.bmad_mode.as_deref().filter(|m| BMAD_MODES.contains(m)) {
        meta_json["bmad"] = json!({"mode": mode, "version": meta.bmad_version});
    }
    if let Some(mode) = meta.mode.as_deref().filter(|m| RUN_MODES.contains(m)) {
        meta_json["mode"] = json!(mode);
    }
    if let Some(id) = meta.dispatch_entry_id.as_deref() {
        meta_json["dispatchEntryId"] = json!(id);
    }
    if let Some(n) = meta.max_implementers {
        meta_json["maxImplementers"] = json!(n.clamp(1, MAX_IMPLEMENTERS_LIMIT));
    }
    write_json_atomic(&dir.join("meta.json"), &meta_json)?;
    Ok(CreatedRun {
        run_id,
        run_dir: dir.display().to_string(),
    })
}

/// The start folder of a run: meta.json's, else request.md's `folder:` line.
pub fn run_folder_of(run_dir: &Path) -> Option<String> {
    let meta = read_json_value(&run_dir.join("meta.json"));
    if let Some(folder) = meta
        .as_ref()
        .and_then(|m| m.get("folder"))
        .and_then(Value::as_str)
    {
        return Some(folder.to_string());
    }
    let text = std::fs::read_to_string(run_dir.join("request.md")).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("folder:"))
        .map(|f| f.trim().to_string())
}

/// One line of the app's own in `log.md` (`<UTC> <stage> app: <text>`), the
/// run's stage from status.json. One `write` in append mode, so it never
/// splits a line state.sh appends at the same time. Control characters
/// become spaces; at most 500 characters.
pub fn append_app_log(run_dir: &Path, text: &str) -> Result<(), String> {
    let stage = read_json_value(&run_dir.join("status.json"))
        .and_then(|v| v.get("stage").and_then(Value::as_str).map(str::to_string))
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c == '-'))
        .unwrap_or_else(|| "run".into());
    let text: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(500)
        .collect();
    let line = format!(
        "{} {stage} app: {text}\n",
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
    );
    crate::store::append_run_file(&run_dir.join("log.md"), line.as_bytes())
        .map_err(|e| format!("log.md: {e}"))
}

pub fn read_json_value(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// `<data>/adopted-runs.json`: run folders another data folder made (the
/// dev app's), used in place: `{"schema":1,"runs":["/abs/run/dir", …]}`.
pub const ADOPTED_FILE: &str = "adopted-runs.json";

/// The adopted run folders that still exist, canonical, in file order.
pub fn adopted_runs(data_dir: &Path) -> Vec<PathBuf> {
    let Some(value) = read_json_value(&data_dir.join(ADOPTED_FILE)) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    value
        .get("runs")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .filter_map(|p| p.canonicalize().ok())
                .filter(|p| p.is_dir() && seen.insert(p.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Adds run folders to the adopted index (kept, deduplicated). Returns
/// how many were new.
pub fn adopt_runs(data_dir: &Path, dirs: &[PathBuf]) -> Result<usize, String> {
    let mut list: Vec<String> = read_json_value(&data_dir.join(ADOPTED_FILE))
        .and_then(|v| v.get("runs").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    let mut added = 0;
    for dir in dirs {
        let dir = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        let text = dir.display().to_string();
        if !list.contains(&text) {
            list.push(text);
            added += 1;
        }
    }
    std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    write_json_atomic(
        &data_dir.join(ADOPTED_FILE),
        &json!({"schema": 1, "runs": list}),
    )?;
    Ok(added)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunListItem {
    pub run_id: String,
    pub run_dir: String,
    pub folder: Option<String>,
    pub created_at: Option<String>,
    /// From status.json; null before the skill wrote one.
    pub stage: Option<String>,
    pub updated_at: Option<String>,
    /// The request's first line, short.
    pub title: Option<String>,
    /// Used in place from another data folder (the dev app's).
    #[serde(default)]
    pub adopted: bool,
    /// The run record's totals (`app-usage.json`); None for a run without one.
    #[serde(default)]
    pub record: Option<RecordTotals>,
    /// status.json's PR count (None before the skill wrote one).
    #[serde(default)]
    pub pr_count: Option<u32>,
    /// The ClickUp tickets of the run (tickets.json `tasks[]` id and custom id).
    #[serde(default)]
    pub ticket_ids: Vec<String>,
    /// report.md exists.
    #[serde(default)]
    pub has_report: bool,
    /// How many repos the run works on: request.md's `repos:` limit when it
    /// has one, else repos.json's count (None when neither is readable).
    #[serde(default)]
    pub repo_count: Option<u32>,
    /// Whether BMAD planning was applied: status.json `bmad.applied`; else
    /// mode `on` is true and `off` is false (None for `auto` not yet decided).
    #[serde(default)]
    pub bmad_applied: Option<bool>,
}

/// The repos of a run: request.md's `repos:` names, else repos.json's.
fn repo_count_of(run_dir: &Path) -> Option<u32> {
    let limited = std::fs::read_to_string(run_dir.join("request.md"))
        .ok()
        .and_then(|text| {
            text.lines()
                .take_while(|line| !line.starts_with("## "))
                .find_map(|line| line.strip_prefix("repos:").map(str::to_string))
        })
        .map(|names| names.split(',').filter(|n| !n.trim().is_empty()).count() as u32)
        .filter(|n| *n > 0);
    limited.or_else(|| {
        read_json_value(&run_dir.join("repos.json"))?
            .get("repos")?
            .as_array()
            .map(|repos| repos.len() as u32)
    })
}

fn bmad_applied_of(run_dir: &Path, status: &Option<Value>) -> Option<bool> {
    let recorded = status
        .as_ref()
        .and_then(|v| v.get("bmad"))
        .and_then(|b| b.get("applied"))
        .and_then(Value::as_bool);
    recorded.or_else(|| match bmad_mode_of(run_dir).as_str() {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    })
}

/// What the Runs view shows from the run record.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecordTotals {
    pub outcome: Option<String>,
    pub total_tokens: u64,
    pub estimate_usd: Option<f64>,
    /// Active minutes over all stages.
    pub minutes: f64,
    pub pr_urls: Vec<String>,
    pub first_activity: Option<String>,
    pub last_activity: Option<String>,
}

fn record_totals(run_dir: &Path) -> Option<RecordTotals> {
    let record = super::record::read_record(run_dir)?;
    // A record that only lists sessions (the run is not finished) has no totals yet.
    record.outcome.as_ref()?;
    let tokens = &record.tokens;
    Some(RecordTotals {
        outcome: record.outcome.clone(),
        total_tokens: tokens.input
            + tokens.output
            + tokens.cache_write_5m
            + tokens.cache_write_1h
            + tokens.cache_read,
        estimate_usd: record.estimate_usd,
        minutes: record.active_minutes_by_stage.values().sum(),
        pr_urls: record.pr_urls,
        first_activity: record.first_activity,
        last_activity: record.last_activity,
    })
}

/// tickets.json `tasks[]`: `id` and `custom_id` of each (nothing else is read).
fn ticket_ids(run_dir: &Path) -> Vec<String> {
    let Some(value) = read_json_value(&run_dir.join("tickets.json")) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for task in value
        .get("tasks")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .take(200)
    {
        for key in ["id", "custom_id"] {
            if let Some(id) = task.get(key).and_then(Value::as_str) {
                if !id.is_empty() && id.len() <= 64 && !out.iter().any(|seen| seen == id) {
                    out.push(id.to_string());
                }
            }
        }
    }
    out
}

/// The runs under `<data>/runs` and the adopted ones, newest first.
/// Dot-folders are skipped.
pub fn list_runs(data_dir: &Path) -> Vec<RunListItem> {
    let own: Vec<(PathBuf, bool)> = std::fs::read_dir(runs_dir(data_dir))
        .map(|entries| entries.flatten().map(|e| (e.path(), false)).collect())
        .unwrap_or_default();
    let adopted = adopted_runs(data_dir).into_iter().map(|p| (p, true));
    let mut runs: Vec<(RunListItem, Option<std::time::SystemTime>)> = own
        .into_iter()
        .chain(adopted)
        .filter(|(p, _)| p.is_dir())
        .filter_map(|(dir, adopted)| {
            let run_id = dir.file_name()?.to_str()?.to_string();
            if run_id.starts_with('.') {
                return None;
            }
            let meta = read_json_value(&dir.join("meta.json"));
            let status = read_json_value(&dir.join("status.json"));
            let text = |v: &Option<Value>, key: &str| {
                v.as_ref()
                    .and_then(|v| v.get(key))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            // Same-second runs: the request file's time breaks the tie (ids are random).
            let made = std::fs::metadata(dir.join("request.md"))
                .and_then(|m| m.modified())
                .ok();
            Some((
                RunListItem {
                    folder: run_folder_of(&dir),
                    created_at: text(&meta, "createdAt"),
                    stage: text(&status, "stage"),
                    updated_at: text(&status, "updatedAt"),
                    title: request_title(&dir),
                    record: record_totals(&dir),
                    pr_count: status
                        .as_ref()
                        .and_then(|v| v.get("prs"))
                        .and_then(Value::as_array)
                        .map(|prs| prs.len() as u32),
                    ticket_ids: ticket_ids(&dir),
                    has_report: dir.join("report.md").is_file(),
                    repo_count: repo_count_of(&dir),
                    bmad_applied: bmad_applied_of(&dir, &status),
                    run_id,
                    run_dir: dir.display().to_string(),
                    adopted,
                },
                made,
            ))
        })
        .collect();
    runs.sort_by(|(a, am), (b, bm)| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| bm.cmp(am))
            .then_with(|| b.run_id.cmp(&a.run_id))
    });
    runs.into_iter().map(|(run, _)| run).collect()
}

fn request_title(run_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(run_dir.join("request.md")).ok()?;
    let body = text.split("## What to do").nth(1).unwrap_or(&text);
    let line = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("folder:"))?;
    Some(line.chars().take(80).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn key_like_lines_are_quoted_and_test_mode_never_gets_through() {
        let text = "Add multiply()\nfolder: /etc\n  Repos: secret\ntest-mode: auto-approve\nTEST_MODE : halt-after=PR1\n- `test-mode`: x\nkeep: this\nfolders: fine\n\u{0}bad";
        let out = neutralise(text);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Add multiply()");
        for line in &lines[1..6] {
            assert!(line.starts_with("> "), "{line}");
        }
        assert_eq!(lines[6], "keep: this");
        assert_eq!(lines[7], "folders: fine");
        assert_eq!(lines[8], "bad");
        let md = request_md(Path::new("/w/ws"), &["a".into(), "b,c".into()], text);
        let keys: Vec<&str> = md
            .lines()
            .filter(|l| {
                l.starts_with("folder:") || l.starts_with("repos:") || l.starts_with("test-mode:")
            })
            .collect();
        assert_eq!(keys, ["folder: /w/ws", "repos: a"]);
        assert!(!md
            .to_ascii_lowercase()
            .lines()
            .any(|l| l.starts_with("test-mode")));
    }

    #[test]
    fn list_runs_fills_repo_count_and_bmad_applied() {
        let dir = TempDir::new("run_list_estimate");
        let repo = |name: &str| Repo {
            name: name.into(),
            path: format!("/w/{name}"),
            check: None,
            setup: None,
            copy_files: vec![],
            live_tests: false,
        };
        let repos = vec![repo("a"), repo("b"), repo("c")];
        let meta = |mode: Option<&str>| RunMeta {
            account_id: "default".into(),
            bmad_mode: mode.map(String::from),
            ..Default::default()
        };
        let all = create_run(
            dir.path(),
            Path::new("/w"),
            &repos,
            &[],
            "all",
            &meta(Some("on")),
        )
        .unwrap();
        let some = create_run(
            dir.path(),
            Path::new("/w"),
            &repos,
            &["a".to_string(), "b".to_string()],
            "some",
            &meta(Some("auto")),
        )
        .unwrap();
        std::fs::write(
            Path::new(&some.run_dir).join("status.json"),
            r#"{"schema":1,"bmad":{"applied":true}}"#,
        )
        .unwrap();
        let list = list_runs(dir.path());
        let of = |id: &str| list.iter().find(|r| r.run_id == id).unwrap();
        assert_eq!(of(&all.run_id).repo_count, Some(3));
        assert_eq!(of(&all.run_id).bmad_applied, Some(true), "mode on");
        assert_eq!(of(&some.run_id).repo_count, Some(2));
        assert_eq!(
            of(&some.run_id).bmad_applied,
            Some(true),
            "status.json says applied"
        );
    }

    #[test]
    fn meta_json_records_the_epic_plan_mode_and_nothing_else_for_a_normal_run() {
        let dir = TempDir::new("run_mode");
        let repos = vec![Repo {
            name: "a".into(),
            path: "/w/a".into(),
            check: None,
            setup: None,
            copy_files: vec![],
            live_tests: false,
        }];
        let make = |mode: Option<&str>| {
            let meta = RunMeta {
                account_id: "default".into(),
                mode: mode.map(String::from),
                ..Default::default()
            };
            let created =
                create_run(dir.path(), Path::new("/w"), &repos, &[], "Plan", &meta).unwrap();
            read_json_value(&Path::new(&created.run_dir).join("meta.json")).unwrap()
        };
        assert_eq!(make(Some("epic-plan"))["mode"], "epic-plan");
        assert!(make(None).get("mode").is_none(), "a normal run has no mode");
        assert!(
            make(Some("bogus")).get("mode").is_none(),
            "an unknown mode is not recorded"
        );
    }

    #[test]
    fn ticked_repos_become_a_limit_only_for_a_strict_subset() {
        let detected: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        let ticked = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert!(repos_limit_for(&detected, None).unwrap().is_empty());
        assert!(repos_limit_for(&detected, Some(&ticked(&["c", "a", "b"])))
            .unwrap()
            .is_empty());
        assert_eq!(
            repos_limit_for(&detected, Some(&ticked(&["c", "a"]))).unwrap(),
            ["a", "c"]
        );
        assert!(repos_limit_for(&detected, Some(&ticked(&["a", "zzz"]))).is_err());
        assert!(repos_limit_for(&detected, Some(&[])).is_err());
    }

    #[test]
    fn a_subset_limits_request_md_but_repos_json_keeps_every_repo() {
        let dir = TempDir::new("run_subset");
        let repo = |name: &str| Repo {
            name: name.into(),
            path: format!("/w/{name}"),
            check: None,
            setup: None,
            copy_files: vec![],
            live_tests: false,
        };
        let repos = vec![repo("a"), repo("b")];
        let limit = repos_limit_for(&["a".into(), "b".into()], Some(&["b".to_string()])).unwrap();
        let meta = RunMeta {
            account_id: "default".into(),
            ..Default::default()
        };
        let created =
            create_run(dir.path(), Path::new("/w"), &repos, &limit, "Do it", &meta).unwrap();
        let run = Path::new(&created.run_dir);
        let request = std::fs::read_to_string(run.join("request.md")).unwrap();
        assert!(request.contains("repos: b\n"));
        assert_eq!(
            read_json_value(&run.join("repos.json")).unwrap()["repos"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn the_implementer_override_is_recorded_clamped_and_read_back() {
        let dir = TempDir::new("run_max_impl");
        let repos = vec![Repo {
            name: "a".into(),
            path: "/w/a".into(),
            check: None,
            setup: None,
            copy_files: vec![],
            live_tests: false,
        }];
        let make = |n: Option<u32>, request: &str| {
            let meta = RunMeta {
                account_id: "default".into(),
                max_implementers: n,
                ..Default::default()
            };
            let created =
                create_run(dir.path(), Path::new("/w"), &repos, &[], request, &meta).unwrap();
            PathBuf::from(created.run_dir)
        };
        let none = make(None, "one");
        assert_eq!(max_implementers_of(&none), None);
        assert!(read_json_value(&none.join("meta.json"))
            .unwrap()
            .get("maxImplementers")
            .is_none());
        assert_eq!(max_implementers_of(&make(Some(5), "two")), Some(5));
        assert_eq!(max_implementers_of(&make(Some(50), "three")), Some(8));
        assert_eq!(max_implementers_of(&make(Some(0), "four")), Some(1));
        // A hand-edited value outside 1..=8 is ignored.
        std::fs::write(none.join("meta.json"), r#"{"maxImplementers": 99}"#).unwrap();
        assert_eq!(max_implementers_of(&none), None);
    }

    #[test]
    fn a_run_folder_has_its_inputs_and_lists_newest_first() {
        let dir = TempDir::new("run_create");
        let repos = vec![Repo {
            name: "mathlib".into(),
            path: "/w/mathlib".into(),
            check: Some("npm test".into()),
            setup: None,
            copy_files: vec![".env".into()],
            live_tests: true,
        }];
        let meta = RunMeta {
            plugin_version: Some("abc123".into()),
            claude_version: None,
            account_id: "default".into(),
            account_name: "Personal".into(),
            config_dir: "/tmp/acct".into(),
            review_mode: None,
            bmad_mode: None,
            bmad_version: None,
            max_implementers: None,
            mode: None,
            dispatch_entry_id: None,
        };
        assert!(create_run(dir.path(), Path::new("/w"), &repos, &[], "  ", &meta).is_err());
        let one = create_run(
            dir.path(),
            Path::new("/w"),
            &repos,
            &[],
            "Add multiply()",
            &meta,
        )
        .unwrap();
        let two = create_run(dir.path(), Path::new("/w"), &repos, &[], "Second", &meta).unwrap();
        assert_ne!(one.run_id, two.run_id);
        let run = Path::new(&one.run_dir);
        assert!(std::fs::read_to_string(run.join("request.md"))
            .unwrap()
            .contains("folder: /w\n"));
        let repos_json = read_json_value(&run.join("repos.json")).unwrap();
        assert_eq!(repos_json["schema"], 1);
        assert_eq!(repos_json["repos"][0]["check"], "npm test");
        let meta_json = read_json_value(&run.join("meta.json")).unwrap();
        assert_eq!(meta_json["pluginVersion"], "abc123");
        assert_eq!(repos_json["repos"][0]["copyFiles"][0], ".env");
        assert!(repos_json["repos"][0]["setup"].is_null());
        assert_eq!(repos_json["repos"][0]["liveTests"], true);
        assert!(meta_json.get("reviewMode").is_none(), "not recorded");
        assert_eq!(review_mode_of(run), "local");
        assert!(meta_json.get("bmad").is_none(), "no mode, nothing recorded");
        assert_eq!(bmad_mode_of(run), "off", "an old run reads as off");
        assert_eq!(bmad_version_of(run), None);
        assert_eq!(meta_json["account"]["name"], "Personal");
        assert!(meta_json["claudeVersion"].is_null());
        std::fs::write(
            run.join("status.json"),
            r#"{"schema":1,"stage":"building"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(runs_dir(dir.path()).join(".tmp-x")).unwrap();
        let list = list_runs(dir.path());
        assert_eq!(list.len(), 2);
        let first = list.iter().find(|r| r.run_id == one.run_id).unwrap();
        assert_eq!(first.stage.as_deref(), Some("building"));
        assert_eq!(first.folder.as_deref(), Some("/w"));
        assert_eq!(first.title.as_deref(), Some("Add multiply()"));
        assert_eq!(list[0].run_id, two.run_id);

        let gh = create_run(
            dir.path(),
            Path::new("/w"),
            &repos,
            &[],
            "Third",
            &RunMeta {
                review_mode: Some("github".into()),
                ..meta.clone()
            },
        )
        .unwrap();
        assert_eq!(review_mode_of(Path::new(&gh.run_dir)), "github");
        assert_eq!(
            read_json_value(&Path::new(&gh.run_dir).join("meta.json")).unwrap()["reviewMode"],
            "github"
        );

        // BMAD: a valid mode is recorded with the version; an invalid one is not.
        let bm = create_run(
            dir.path(),
            Path::new("/w"),
            &repos,
            &[],
            "Fourth",
            &RunMeta {
                bmad_mode: Some("auto".into()),
                bmad_version: Some("bda3c59 6.13.0-next".into()),
                ..meta.clone()
            },
        )
        .unwrap();
        let bm_dir = Path::new(&bm.run_dir);
        let bm_meta = read_json_value(&bm_dir.join("meta.json")).unwrap();
        assert_eq!(bm_meta["bmad"]["mode"], "auto");
        assert_eq!(bm_meta["bmad"]["version"], "bda3c59 6.13.0-next");
        assert_eq!(bmad_mode_of(bm_dir), "auto");
        assert_eq!(
            bmad_version_of(bm_dir).as_deref(),
            Some("bda3c59 6.13.0-next")
        );
        let bad = create_run(
            dir.path(),
            Path::new("/w"),
            &repos,
            &[],
            "Fifth",
            &RunMeta {
                bmad_mode: Some("maybe".into()),
                bmad_version: Some("x".into()),
                ..meta.clone()
            },
        )
        .unwrap();
        let bad_dir = Path::new(&bad.run_dir);
        assert!(read_json_value(&bad_dir.join("meta.json"))
            .unwrap()
            .get("bmad")
            .is_none());
        assert_eq!(bmad_mode_of(bad_dir), "off");
        // A meta.json with an unknown or malformed bmad reads as off.
        std::fs::write(bad_dir.join("meta.json"), r#"{"bmad": {"mode": "ON"}}"#).unwrap();
        assert_eq!(bmad_mode_of(bad_dir), "off");
        std::fs::write(bad_dir.join("meta.json"), r#"{"bmad": "on"}"#).unwrap();
        assert_eq!(bmad_mode_of(bad_dir), "off");

        // An adopted run elsewhere is listed too, once; a missing one is not.
        let other = dir.join("dev/runs/r-20260101-000000-abcd");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            other.join("request.md"),
            "# Request\n\nfolder: /w\n\n## What to do\n\nOld\n",
        )
        .unwrap();
        adopt_runs(
            dir.path(),
            &[other.clone(), other.clone(), dir.join("gone")],
        )
        .unwrap();
        assert_eq!(
            adopt_runs(dir.path(), std::slice::from_ref(&other)).unwrap(),
            0
        );
        let list = list_runs(dir.path());
        assert_eq!(list.len(), 6);
        let adopted: Vec<_> = list.iter().filter(|r| r.adopted).collect();
        assert_eq!(adopted.len(), 1);
        assert_eq!(adopted[0].title.as_deref(), Some("Old"));
        assert_eq!(adopted_runs(dir.path()), [other.canonicalize().unwrap()]);
    }

    #[test]
    fn the_list_carries_the_record_pr_count_tickets_and_report() {
        let dir = TempDir::new("run_list_fields");
        let runs = runs_dir(dir.path());
        let make = |id: &str, created: &str| {
            let run = runs.join(id);
            std::fs::create_dir_all(&run).unwrap();
            std::fs::write(
                run.join("meta.json"),
                format!(r#"{{"createdAt":"{created}","folder":"/w"}}"#),
            )
            .unwrap();
            std::fs::write(run.join("request.md"), "Do the thing\n").unwrap();
            run
        };
        let done = make("r-20261027-1000-aaaa", "2026-10-27T10:00:00Z");
        let old = make("r-20261020-1000-bbbb", "2026-10-20T10:00:00Z");
        std::fs::write(
            done.join("status.json"),
            r#"{"schema":1,"stage":"done","prs":[{"id":"PR1"},{"id":"PR2"}]}"#,
        )
        .unwrap();
        std::fs::write(
            done.join("tickets.json"),
            r#"{"schema":1,"tasks":[{"id":"86b1aa001","custom_id":"QA-129","name":"x"},{"id":"86b1aa002","custom_id":null}]}"#,
        )
        .unwrap();
        std::fs::write(done.join("report.md"), "# report").unwrap();
        std::fs::write(
            done.join("app-usage.json"),
            r#"{"schema":1,"sessions":[],"outcome":"done","prCount":2,"prUrls":["https://github.com/o/r/pull/1"],
                "tokens":{"input":10,"output":5,"cacheWrite5m":1,"cacheWrite1h":2,"cacheRead":100},
                "estimateUsd":1.5,"activeMinutesByStage":{"planning":10.0,"building":20.5},
                "firstActivity":"2026-10-27T10:00:00Z","lastActivity":"2026-10-27T11:00:00Z","statusUpdatedAt":"t"}"#,
        )
        .unwrap();

        let list = list_runs(dir.path());
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].run_id, "r-20261027-1000-aaaa", "newest first");
        let first = &list[0];
        assert_eq!(first.pr_count, Some(2));
        assert_eq!(first.ticket_ids, ["86b1aa001", "QA-129", "86b1aa002"]);
        assert!(first.has_report);
        let record = first.record.as_ref().unwrap();
        assert_eq!(record.total_tokens, 118);
        assert_eq!(record.estimate_usd, Some(1.5));
        assert_eq!(record.minutes, 30.5);
        assert_eq!(record.pr_urls, ["https://github.com/o/r/pull/1"]);
        // A run without a record, tickets or status reads as none.
        let second = &list[1];
        assert_eq!(second.run_id, old.file_name().unwrap().to_str().unwrap());
        assert!(second.record.is_none() && second.pr_count.is_none());
        assert!(second.ticket_ids.is_empty() && !second.has_report);
        // A record that only lists sessions (not finished) has no totals.
        std::fs::write(
            old.join("app-usage.json"),
            r#"{"schema":1,"sessions":[{"sessionId":"s","configDir":"/c"}]}"#,
        )
        .unwrap();
        assert!(list_runs(dir.path())[1].record.is_none());
    }

    #[test]
    fn run_ids_do_not_collide() {
        let dir = TempDir::new("run_ids");
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-22T13:04:05Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let runs = dir.join("runs");
        let ids: std::collections::HashSet<String> =
            (0..50).map(|_| new_run_id(&runs, now)).collect();
        assert!(ids.len() > 45, "random suffix");
        let id = ids.iter().next().unwrap();
        assert!(
            id.starts_with("r-20261022-130405-") && id.len() == "r-20261022-130405-abcd".len(),
            "{id}"
        );
        assert!(!id.contains('/') && !id.starts_with('.'));
    }

    #[cfg(unix)]
    #[test]
    fn log_md_planted_as_a_link_is_not_appended_through() {
        let dir = TempDir::new("log_md_link");
        let outside = dir.join("outside.txt");
        std::fs::write(&outside, "outside").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("log.md")).unwrap();
        assert!(append_app_log(dir.path(), "cleanup done").is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "outside");
        std::fs::remove_file(dir.join("log.md")).unwrap();
        append_app_log(dir.path(), "cleanup done").unwrap();
        assert!(std::fs::read_to_string(dir.join("log.md"))
            .unwrap()
            .contains("app: cleanup done"));
    }
}
