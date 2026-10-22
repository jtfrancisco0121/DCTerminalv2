//! R5: a finished or abandoned run's worktrees. Lists them (status.json and
//! `.guard.json`), says which are clean, and removes only clean linked
//! worktrees, with `git worktree remove` (no `--force`, so git refuses a
//! dirty one too). A main checkout is never touched. The branch is deleted
//! with `git branch -d` only (kept when not merged anywhere).

use super::folder::read_json_value;
use super::git::{canonical, git};
use super::status::parse_status;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeItem {
    pub pr_id: String,
    pub worktree: String,
    pub repo_path: Option<String>,
    pub branch: Option<String>,
    pub exists: bool,
    /// No uncommitted or untracked changes.
    pub clean: bool,
    pub changed_files: usize,
    /// Clean, a linked worktree of its repo, and not a main checkout.
    pub removable: bool,
    /// Removable, with ignored build/dependency folders that go with it.
    pub disposable_only: bool,
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoveResult {
    pub pr_id: String,
    pub removed: bool,
    pub branch_deleted: bool,
    pub message: String,
}

/// The run's worktrees: from status.json, plus `.guard.json` ones it lacks.
fn recorded(run_dir: &Path) -> Vec<(String, PathBuf, Option<String>, Option<String>)> {
    let mut out = Vec::new();
    if let Some(status) = read_json_value(&run_dir.join("status.json")) {
        if let Ok(view) = parse_status(&status) {
            for pr in view.prs {
                if let Some(wt) = pr.worktree.filter(|w| !w.trim().is_empty()) {
                    out.push((pr.id, PathBuf::from(wt), pr.repo_path, pr.branch));
                }
            }
        }
    }
    if let Some(guard) = read_json_value(&run_dir.join(".guard.json")) {
        if let Some(map) = guard.get("worktrees").and_then(|w| w.as_object()) {
            for (id, path) in map {
                let Some(path) = path.as_str() else { continue };
                let path = PathBuf::from(path);
                if !out
                    .iter()
                    .any(|(_, p, _, _)| canonical(p) == canonical(&path))
                {
                    out.push((id.clone(), path, None, None));
                }
            }
        }
    }
    out
}

/// The main checkout `worktree` belongs to, and whether `worktree` is a
/// linked worktree (not that main checkout).
fn linked_of(worktree: &Path) -> Result<PathBuf, String> {
    let git_dir = git(worktree, &["rev-parse", "--absolute-git-dir"])?;
    let common = git(
        worktree,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    if canonical(Path::new(&git_dir)) == canonical(Path::new(&common)) {
        return Err("this is a main checkout, not a worktree".into());
    }
    let common = PathBuf::from(common);
    common
        .parent()
        .map(canonical)
        .ok_or_else(|| "no main checkout found".into())
}

/// The branch the skill must use for `pr_id` (run-folder.md).
pub fn expected_branch(run_id: &str, pr_id: &str) -> String {
    format!("dcv2/{run_id}-{}", pr_id.to_ascii_lowercase())
}

/// The worktree path the skill must use: `<repo parent>/<repo>-worktrees/dcv2-<runId>-<pr>`.
pub fn expected_worktree(repo: &Path, run_id: &str, pr_id: &str) -> Option<PathBuf> {
    let repo = canonical(repo);
    let name = repo.file_name()?.to_string_lossy().to_string();
    Some(
        repo.parent()?
            .join(format!("{name}-worktrees"))
            .join(format!("dcv2-{run_id}-{}", pr_id.to_ascii_lowercase())),
    )
}

/// The status.json / .guard.json paths are written by an LLM-driven skill,
/// so a worktree is trusted only when it is exactly where the contract puts
/// it, a linked worktree of that repo, on the run's branch for that PR.
/// Returns the main checkout.
pub fn verify_worktree(
    run_id: &str,
    pr_id: &str,
    worktree: &Path,
    repo: Option<&str>,
) -> Result<PathBuf, String> {
    if !super::status::valid_pr_id(pr_id) {
        return Err("not a PR id".into());
    }
    if !worktree.is_dir() {
        return Err("already gone".into());
    }
    let main = linked_of(worktree)?;
    if let Some(repo) = repo {
        if canonical(Path::new(repo)) == canonical(worktree) {
            return Err("this is the repo's main checkout".into());
        }
        if canonical(Path::new(repo)) != main {
            return Err("its repo is not the one status.json names".into());
        }
    }
    let expected = expected_worktree(&main, run_id, pr_id).ok_or("no repo folder name")?;
    if canonical(worktree) != canonical(&expected) {
        return Err(format!(
            "not where this run's worktrees go ({})",
            expected.display()
        ));
    }
    let branch = expected_branch(run_id, pr_id);
    match git(worktree, &["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        Ok(head) if head == branch => Ok(main),
        Ok(head) => Err(format!("on branch {head}, not {branch}")),
        Err(_) => Err("detached HEAD".into()),
    }
}

/// Ignored build and dependency folders a worktree may be removed with.
pub const DISPOSABLE: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    "coverage",
    ".turbo",
    "__pycache__",
    ".venv",
    ".pytest_cache",
    ".gradle",
];

/// An ignored path (`git status --ignored` form) inside a disposable folder.
pub fn disposable(path: &str) -> bool {
    let components: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    // `build/` itself, or anything under it; a file named `target` is not.
    let dirs = if path.ends_with('/') {
        &components[..]
    } else {
        &components[..components.len().saturating_sub(1)]
    };
    dirs.iter().any(|c| DISPOSABLE.contains(c))
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dirt {
    /// Tracked changes and untracked (not ignored) files.
    pub changed: usize,
    /// Ignored folders that are only build output or dependencies.
    pub disposable: usize,
    /// Other ignored files (`.env`, …): the worktree is kept.
    pub kept_ignored: Vec<String>,
}

pub fn dirt(worktree: &Path) -> Result<Dirt, String> {
    let out = git(
        worktree,
        &[
            "status",
            "--porcelain",
            "--ignored",
            "--untracked-files=normal",
        ],
    )?;
    let mut dirt = Dirt::default();
    for line in out.lines().filter(|l| l.len() > 3) {
        let path = line[3..].trim_matches('"');
        if line.starts_with("!!") {
            if disposable(path) {
                dirt.disposable += 1;
            } else {
                dirt.kept_ignored.push(path.to_string());
            }
        } else {
            dirt.changed += 1;
        }
    }
    Ok(dirt)
}

fn inspect(
    run_id: &str,
    id: &str,
    wt: &Path,
    repo: Option<&str>,
    branch: Option<&str>,
) -> WorktreeItem {
    let mut item = WorktreeItem {
        pr_id: id.to_string(),
        worktree: wt.display().to_string(),
        repo_path: repo.map(str::to_string),
        branch: branch.map(str::to_string),
        exists: wt.is_dir(),
        clean: false,
        changed_files: 0,
        removable: false,
        disposable_only: false,
        problem: None,
    };
    let main = match verify_worktree(run_id, id, wt, repo) {
        Ok(main) => main,
        Err(err) => {
            item.problem = Some(err);
            return item;
        }
    };
    item.repo_path = Some(main.display().to_string());
    item.branch = Some(expected_branch(run_id, id));
    match dirt(wt) {
        Ok(dirt) => {
            item.changed_files = dirt.changed;
            item.clean = dirt.changed == 0;
            if !dirt.kept_ignored.is_empty() {
                let names: Vec<&str> = dirt
                    .kept_ignored
                    .iter()
                    .take(3)
                    .map(String::as_str)
                    .collect();
                let more = dirt.kept_ignored.len().saturating_sub(3);
                let more = if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                };
                item.problem = Some(format!(
                    "has ignored files ({}{more}), kept",
                    names.join(", ")
                ));
            }
            item.removable = dirt.changed == 0 && dirt.kept_ignored.is_empty();
            item.disposable_only = item.removable && dirt.disposable > 0;
        }
        Err(err) => item.problem = Some(err),
    }
    item
}

fn run_id_of(run_dir: &Path) -> String {
    run_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

pub fn list_worktrees(run_dir: &Path) -> Vec<WorktreeItem> {
    let run_id = run_id_of(run_dir);
    recorded(run_dir)
        .iter()
        .map(|(id, wt, repo, branch)| inspect(&run_id, id, wt, repo.as_deref(), branch.as_deref()))
        .collect()
}

/// Removes the clean worktrees of `pr_ids` (checked again right now).
pub fn remove_clean(run_dir: &Path, pr_ids: &[String]) -> Vec<RemoveResult> {
    list_worktrees(run_dir)
        .into_iter()
        .filter(|item| pr_ids.contains(&item.pr_id))
        .map(|item| {
            let mut result = RemoveResult {
                pr_id: item.pr_id.clone(),
                removed: false,
                branch_deleted: false,
                message: String::new(),
            };
            let (true, Some(repo)) = (item.removable, item.repo_path.as_deref()) else {
                result.message = item
                    .problem
                    .clone()
                    .unwrap_or_else(|| format!("kept: {} changed files", item.changed_files));
                return result;
            };
            let repo = Path::new(repo);
            if let Err(err) = git(repo, &["worktree", "remove", "--", &item.worktree]) {
                result.message = format!("kept: {err}");
                return result;
            }
            result.removed = true;
            result.message = "worktree removed".into();
            if let Some(branch) = item.branch.as_deref() {
                match git(repo, &["branch", "-d", "--", branch]) {
                    Ok(_) => {
                        result.branch_deleted = true;
                        result.message = format!("worktree removed, branch {branch} deleted");
                    }
                    Err(_) => {
                        result.message =
                            format!("worktree removed; branch {branch} kept (not merged)")
                    }
                }
            }
            result
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_build_and_dependency_folders_are_disposable() {
        for path in [
            "node_modules/",
            "server/node_modules/",
            "target/",
            "web/.next/",
            "a/__pycache__/x.pyc",
            "build/out.js",
        ] {
            assert!(disposable(path), "{path}");
        }
        for path in [
            ".env",
            "server/.env.local",
            "target",
            "secrets/",
            "notes.txt",
        ] {
            assert!(!disposable(path), "{path}");
        }
    }

    use crate::run::git::testing::{repo_at, run};
    use crate::test_support::TempDir;
    use serde_json::json;

    const RUN: &str = "r-20261022-1304";

    fn add(repo: &Path, branch: &str, path: &Path) {
        run(
            repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                path.to_str().unwrap(),
            ],
        );
    }

    #[test]
    fn only_clean_contract_worktrees_are_removed_and_main_checkouts_never() {
        let dir = TempDir::new("janitor");
        let repo = dir.join("ws/mathlib");
        repo_at(&repo);
        std::fs::write(repo.join(".gitignore"), ".env\nnode_modules/\ntarget/\n").unwrap();
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-q", "-m", "ignore"]);
        let wts = dir.join("ws/mathlib-worktrees");
        let clean = wts.join(format!("dcv2-{RUN}-pr1"));
        let dirty = wts.join(format!("dcv2-{RUN}-pr2"));
        let ignored = wts.join(format!("dcv2-{RUN}-pr4"));
        let elsewhere = dir.join("ws/elsewhere");
        let wrong_branch = wts.join(format!("dcv2-{RUN}-pr6"));
        let detached = wts.join(format!("dcv2-{RUN}-pr7"));
        add(&repo, &format!("dcv2/{RUN}-pr1"), &clean);
        add(&repo, &format!("dcv2/{RUN}-pr2"), &dirty);
        add(&repo, &format!("dcv2/{RUN}-pr4"), &ignored);
        add(&repo, &format!("dcv2/{RUN}-pr5"), &elsewhere);
        add(&repo, "feature/other", &wrong_branch);
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                detached.to_str().unwrap(),
            ],
        );
        std::fs::write(dirty.join("new.txt"), "wip").unwrap();
        std::fs::create_dir_all(dir.join("ws/foreign")).unwrap();
        std::fs::write(ignored.join(".env"), "SECRET=1").unwrap();
        std::fs::create_dir_all(ignored.join("node_modules/x")).unwrap();
        std::fs::write(ignored.join("node_modules/x/a.js"), "1").unwrap();
        // Build output and dependencies alone do not keep a worktree.
        std::fs::create_dir_all(clean.join("node_modules/x")).unwrap();
        std::fs::write(clean.join("node_modules/x/a.js"), "1").unwrap();
        std::fs::create_dir_all(clean.join("target/debug")).unwrap();
        std::fs::write(clean.join("target/debug/bin"), "1").unwrap();
        let run_dir = dir.join("runs").join(RUN);
        std::fs::create_dir_all(&run_dir).unwrap();
        let pr = |id: &str, wt: &Path| json!({"id": id, "repoPath": repo, "worktree": wt});
        std::fs::write(
            run_dir.join("status.json"),
            json!({"schema": 1, "stage": "done", "prs": [
                pr("PR1", &clean), pr("PR2", &dirty), pr("PR3", &repo), pr("PR4", &ignored),
                pr("PR5", &elsewhere), pr("PR6", &wrong_branch), pr("PR7", &detached)
            ]})
            .to_string(),
        )
        .unwrap();
        // .guard.json-only entries: one where the contract puts it would be
        // trusted; a foreign path never.
        std::fs::write(
            run_dir.join(".guard.json"),
            json!({"schema": 1, "worktrees": {"PR1": clean, "PR8": dir.join("ws/foreign"), "PR9": dir.join("gone")}}).to_string(),
        )
        .unwrap();
        let items = list_worktrees(&run_dir);
        let by = |id: &str| items.iter().find(|i| i.pr_id == id).unwrap().clone();
        assert_eq!(items.len(), 9, "{items:?}");
        assert!(by("PR1").removable, "{:?}", by("PR1"));
        assert!(by("PR1").disposable_only);
        assert!(!by("PR2").removable && by("PR2").changed_files == 1);
        assert!(by("PR3").problem.unwrap().contains("main checkout"));
        assert_eq!(by("PR4").problem.unwrap(), "has ignored files (.env), kept");
        assert!(!by("PR4").removable);
        assert!(by("PR5").problem.unwrap().contains("not where"));
        assert!(by("PR6").problem.unwrap().contains("feature/other"));
        assert!(by("PR7").problem.is_some() && !by("PR7").removable);
        assert!(!by("PR8").removable);
        assert!(!by("PR9").exists);

        let all: Vec<String> = (1..=9).map(|n| format!("PR{n}")).collect();
        let results = remove_clean(&run_dir, &all);
        let r = |id: &str| results.iter().find(|x| x.pr_id == id).unwrap().clone();
        assert!(r("PR1").removed);
        assert!(r("PR1").branch_deleted, "{}", r("PR1").message);
        for id in ["PR2", "PR3", "PR4", "PR5", "PR6", "PR7", "PR8"] {
            assert!(!r(id).removed, "{id}");
        }
        assert!(!clean.exists());
        assert!(dirty.join("new.txt").exists());
        assert!(ignored.join(".env").exists());
        assert!(elsewhere.exists() && wrong_branch.exists() && detached.exists());
        assert!(repo.join("README.md").exists());
    }
}
