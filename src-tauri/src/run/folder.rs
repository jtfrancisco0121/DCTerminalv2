//! The run folder the app creates (run-folder.md): `request.md`,
//! `repos.json` and `meta.json`, under `<app data>/runs/<runId>/`. The app
//! never writes a `test-mode:` line and never sets `DCV2_TEST_MODE`.

use super::repos::Repo;
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
    std::fs::write(
        dir.join("request.md"),
        request_md(folder, repos_limit, request),
    )
    .map_err(|e| format!("request.md: {e}"))?;
    write_json_atomic(
        &dir.join("repos.json"),
        &json!({"schema": 1, "repos": repos}),
    )?;
    write_json_atomic(
        &dir.join("meta.json"),
        &json!({
            "schema": 1,
            "runId": run_id,
            "createdAt": now.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            "folder": folder.display().to_string(),
            "pluginVersion": meta.plugin_version,
            "claudeVersion": meta.claude_version,
            "account": {"id": meta.account_id, "name": meta.account_name, "configDir": meta.config_dir},
            "app": format!("dcterminalv2 {}", env!("CARGO_PKG_VERSION")),
        }),
    )?;
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

pub fn read_json_value(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
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
}

/// The runs under `<data>/runs`, newest first. Dot-folders are skipped.
pub fn list_runs(data_dir: &Path) -> Vec<RunListItem> {
    let Ok(entries) = std::fs::read_dir(runs_dir(data_dir)) else {
        return Vec::new();
    };
    let mut runs: Vec<(RunListItem, Option<std::time::SystemTime>)> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let run_id = e.file_name().to_str()?.to_string();
            if run_id.starts_with('.') {
                return None;
            }
            let dir = e.path();
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
                    run_id,
                    run_dir: dir.display().to_string(),
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
    fn a_run_folder_has_its_inputs_and_lists_newest_first() {
        let dir = TempDir::new("run_create");
        let repos = vec![Repo {
            name: "mathlib".into(),
            path: "/w/mathlib".into(),
            check: Some("npm test".into()),
        }];
        let meta = RunMeta {
            plugin_version: Some("abc123".into()),
            claude_version: None,
            account_id: "default".into(),
            account_name: "Personal".into(),
            config_dir: "/tmp/acct".into(),
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
}
