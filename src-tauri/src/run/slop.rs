//! Slop check (AD-2, AD-3): the same verdict `plugin/scripts/stop-check.sh`
//! gives the Implementer, computed by the app for the run panel. The rules
//! are `plugin/scripts/slop-patterns.tsv` (id, scope, pattern, message), read
//! from the resolved plugin folder; only lines the PR added (the diff from
//! its base commit to HEAD, plus uncommitted changes to tracked files) are
//! matched. Reads only: it runs `git diff` in the verified worktree and
//! writes nothing.

use super::git::git;
use super::status::{parse_status, valid_pr_id};
use regex::Regex;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const DOC_EXT: &[&str] = &[".md", ".mdx", ".txt", ".rst"];
const LOCKS: &[&str] = &[
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "npm-shrinkwrap.json",
    "cargo.lock",
    "go.sum",
    "poetry.lock",
    "pipfile.lock",
    "composer.lock",
    "gemfile.lock",
    "uv.lock",
    "bun.lockb",
    "bun.lock",
];
const EXCLUDED: &[&str] = &[
    "plugin/scripts/slop-patterns.tsv",
    "fixtures/slop/",
    "plugin/bmad/",
];
/// A line longer than this is generated or minified: never matched.
const MAX_LINE: usize = 2000;

pub struct Rule {
    pub id: String,
    pub scope: String,
    pub regex: Regex,
    pub message: String,
}

/// The rules of a `slop-patterns.tsv` text; a row that is short or whose
/// pattern does not compile is skipped.
pub fn load_rules(tsv: &str) -> Vec<Rule> {
    tsv.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let (id, scope, pattern, message) =
                (cols.next()?, cols.next()?, cols.next()?, cols.next()?);
            Some(Rule {
                id: id.into(),
                scope: scope.into(),
                regex: Regex::new(pattern).ok()?,
                message: message.into(),
            })
        })
        .collect()
}

/// Scope: `*` every file; `code` every file except docs; `ext:a,b[@prefix/]`.
pub fn in_scope(scope: &str, path: &str) -> bool {
    let low = path.to_lowercase();
    if scope == "*" {
        return true;
    }
    if scope == "code" {
        return !DOC_EXT.iter().any(|e| low.ends_with(e));
    }
    if let Some(spec) = scope.strip_prefix("ext:") {
        let (exts, prefix) = spec.split_once('@').unwrap_or((spec, ""));
        return exts
            .split(',')
            .filter(|e| !e.is_empty())
            .any(|e| low.ends_with(&format!(".{}", e.to_lowercase())))
            && path.starts_with(prefix);
    }
    false
}

pub fn excluded(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    EXCLUDED.iter().any(|p| path.starts_with(p))
        || LOCKS.contains(&name.as_str())
        || name.ends_with(".lock")
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Finding {
    pub path: String,
    pub line: u32,
    pub rule: String,
    pub message: String,
}

/// `(path, line)` -> text of every added line of `git diff --unified=0
/// <base>...HEAD` and `git diff --unified=0 HEAD`.
pub fn added_lines(worktree: &Path, base: Option<&str>) -> BTreeMap<(String, u32), String> {
    let mut found = BTreeMap::new();
    let mut ranges: Vec<String> = base.map(|b| format!("{b}...HEAD")).into_iter().collect();
    ranges.push("HEAD".into());
    for range in ranges {
        let diff = match git_diff(worktree, &range) {
            Some(d) => d,
            None => continue,
        };
        let mut path: Option<String> = None;
        let mut line_no = 0u32;
        for line in diff.lines() {
            if let Some(rest) = line.strip_prefix("+++ ") {
                path = rest.strip_prefix("b/").map(str::to_string);
            } else if line.starts_with("@@") {
                line_no = parse_hunk_start(line);
            } else if let (Some(text), Some(p)) = (line.strip_prefix('+'), path.as_ref()) {
                found.insert((p.clone(), line_no), text.to_string());
                line_no += 1;
            }
        }
    }
    found
}

fn parse_hunk_start(header: &str) -> u32 {
    // @@ -a,b +c,d @@
    header
        .split_whitespace()
        .nth(2)
        .and_then(|plus| plus.trim_start_matches('+').split(',').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

fn git_diff(worktree: &Path, range: &str) -> Option<String> {
    // `git()` trims the end, which is harmless for a diff.
    git(
        worktree,
        &[
            "-c",
            "core.quotepath=off",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-renames",
            "--unified=0",
            range,
            "--",
        ],
    )
    .ok()
}

/// Every rule match on the added lines, sorted by (path, line, rule).
pub fn scan(worktree: &Path, base: Option<&str>, rules: &[Rule]) -> Vec<Finding> {
    let mut out = Vec::new();
    for ((path, line), text) in added_lines(worktree, base) {
        if excluded(&path) || text.len() > MAX_LINE {
            continue;
        }
        for rule in rules {
            if in_scope(&rule.scope, &path) && rule.regex.is_match(&text) {
                out.push(Finding {
                    path: path.clone(),
                    line,
                    rule: rule.id.clone(),
                    message: rule.message.clone(),
                });
            }
        }
    }
    out.sort();
    out
}

/// The base commit the worktree was made from (`.guard.json` `bases`), else
/// the merge-base with the PR's base branch.
pub fn slop_base(
    run_dir: &Path,
    pr_id: &str,
    worktree: &Path,
    base_ref: Option<&str>,
) -> Option<String> {
    let recorded = std::fs::read_to_string(run_dir.join(".guard.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|g| g["bases"][pr_id].as_str().map(str::to_string))
        .filter(|b| b.len() >= 7 && b.chars().all(|c| c.is_ascii_hexdigit()));
    if recorded.is_some() {
        return recorded;
    }
    let mut candidates: Vec<String> = Vec::new();
    if let Some(b) = base_ref.filter(|b| {
        !b.is_empty()
            && b.chars()
                .all(|c| c.is_ascii_alphanumeric() || "./_-".contains(c))
    }) {
        candidates.push(format!("origin/{b}"));
        candidates.push(b.to_string());
    }
    candidates.extend(["origin/HEAD", "main", "master"].map(String::from));
    candidates
        .iter()
        .find_map(|c| git(worktree, &["merge-base", "HEAD", c]).ok())
        .filter(|s| !s.is_empty())
}

/// What the run panel shows.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SlopReport {
    /// `ok` | `worktree-missing` | `no-rules`.
    pub state: String,
    pub findings: Vec<Finding>,
    pub total: usize,
    pub detail: Option<String>,
}

/// At most this many findings are returned (the hook lists 30 as well).
pub const MAX_FINDINGS: usize = 200;

/// The verdict for one PR of a run. The worktree must be where the contract
/// puts it (`janitor::verify_worktree`); a missing folder is a state, not an error.
pub fn check_pr(run_dir: &Path, pr_id: &str, plugin: &Path) -> Result<SlopReport, String> {
    if !valid_pr_id(pr_id) {
        return Err("not a PR id".into());
    }
    let text = std::fs::read_to_string(run_dir.join("status.json"))
        .map_err(|_| "This run has no status.json yet.".to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("status.json: {e}"))?;
    let status = parse_status(&value)?;
    let pr = status
        .prs
        .iter()
        .find(|p| p.id == pr_id)
        .ok_or_else(|| format!("{pr_id} is not in status.json"))?;
    let run_id = run_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let wt = PathBuf::from(
        pr.worktree
            .clone()
            .ok_or_else(|| "the PR has no worktree yet".to_string())?,
    );
    if !wt.is_dir() {
        return Ok(SlopReport {
            state: "worktree-missing".into(),
            findings: vec![],
            total: 0,
            detail: Some(format!("{} is gone", wt.display())),
        });
    }
    super::janitor::verify_worktree(&run_id, pr_id, &wt, pr.repo_path.as_deref())?;
    let tsv = std::fs::read_to_string(plugin.join("scripts").join("slop-patterns.tsv"))
        .map_err(|e| format!("slop-patterns.tsv: {e}"))?;
    let rules = load_rules(&tsv);
    if rules.is_empty() {
        return Ok(SlopReport {
            state: "no-rules".into(),
            findings: vec![],
            total: 0,
            detail: None,
        });
    }
    let base_ref = value["prs"]
        .as_array()
        .and_then(|prs| prs.iter().find(|p| p["id"] == pr_id))
        .and_then(|p| p["base"].as_str());
    let base = slop_base(run_dir, pr_id, &wt, base_ref);
    let mut findings = scan(&wt, base.as_deref(), &rules);
    let total = findings.len();
    findings.truncate(MAX_FINDINGS);
    Ok(SlopReport {
        state: "ok".into(),
        findings,
        total,
        detail: base
            .is_none()
            .then(|| "no base commit found: only uncommitted changes were checked".to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
    }

    fn rules() -> Vec<Rule> {
        let tsv =
            std::fs::read_to_string(repo_root().join("plugin/scripts/slop-patterns.tsv")).unwrap();
        load_rules(&tsv)
    }

    #[test]
    fn every_pattern_compiles_in_rust() {
        let tsv =
            std::fs::read_to_string(repo_root().join("plugin/scripts/slop-patterns.tsv")).unwrap();
        let rows = tsv
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .count();
        assert!(rows >= 5);
        assert_eq!(load_rules(&tsv).len(), rows, "a pattern did not compile");
    }

    #[test]
    fn the_fixture_gives_the_expected_findings() {
        let dir = std::env::temp_dir().join(format!("dcv2-slop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let out = std::process::Command::new("sh")
            .arg(repo_root().join("fixtures/slop/setup.sh"))
            .arg(&dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let base = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let found: Vec<String> = scan(&dir, Some(&base), &rules())
            .iter()
            .map(|f| format!("{}:{}:{}", f.path, f.line, f.rule))
            .collect();
        let mut expected: Vec<String> =
            std::fs::read_to_string(repo_root().join("fixtures/slop/expected.txt"))
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect();
        expected.sort();
        let mut sorted = found.clone();
        sorted.sort();
        assert_eq!(sorted, expected);
        // Unchanged context is never flagged: old.ts line 2 holds a console.log from the base commit.
        assert!(!found.iter().any(|f| f.starts_with("src/old.ts:2:")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_pattern_hits_the_tracked_files_at_head() {
        let root = repo_root();
        let rules = rules();
        let listing = git(&root, &["ls-files"]).unwrap();
        let mut hits = Vec::new();
        for file in listing.lines() {
            if excluded(file) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(root.join(file)) else {
                continue;
            };
            for (n, line) in text.lines().enumerate() {
                for rule in &rules {
                    if in_scope(&rule.scope, file) && rule.regex.is_match(line) {
                        hits.push(format!("{file}:{}:{}", n + 1, rule.id));
                    }
                }
            }
        }
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn scope_rules() {
        assert!(in_scope("*", "a.md"));
        assert!(!in_scope("code", "docs/a.MD"));
        assert!(in_scope("code", "a.rs"));
        assert!(in_scope("ext:ts,tsx@src/", "src/Upper.TS"));
        assert!(!in_scope("ext:ts,tsx@src/", "scripts/a.ts"));
        assert!(
            excluded("yarn.lock") && excluded("sub/Cargo.lock") && excluded("plugin/bmad/x.rs")
        );
        assert!(!excluded("src/lib.rs"));
    }

    #[test]
    fn a_missing_worktree_is_a_state() {
        let dir = std::env::temp_dir().join(format!("dcv2-slop-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("status.json"),
            r#"{"schema":1,"runId":"r-x","stage":"building","prs":[{"id":"PR1","worktree":"/nonexistent/wt","state":"building"}]}"#,
        )
        .unwrap();
        let report = check_pr(&dir, "PR1", &repo_root().join("plugin")).unwrap();
        assert_eq!(report.state, "worktree-missing");
        assert!(report.findings.is_empty());
        assert!(check_pr(&dir, "PR9", &repo_root().join("plugin")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
