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
        .map(Path::to_path_buf)
        .ok_or_else(|| "no main checkout found".into())
}

fn inspect(id: &str, wt: &Path, repo: Option<&str>, branch: Option<&str>) -> WorktreeItem {
    let mut item = WorktreeItem {
        pr_id: id.to_string(),
        worktree: wt.display().to_string(),
        repo_path: repo.map(str::to_string),
        branch: branch.map(str::to_string),
        exists: wt.is_dir(),
        clean: false,
        changed_files: 0,
        removable: false,
        problem: None,
    };
    if !item.exists {
        item.problem = Some("already gone".into());
        return item;
    }
    let main = match linked_of(wt) {
        Ok(main) => main,
        Err(err) => {
            item.problem = Some(err);
            return item;
        }
    };
    if let Some(repo) = repo {
        if canonical(Path::new(repo)) == canonical(wt) {
            item.problem = Some("this is the repo's main checkout".into());
            return item;
        }
        if canonical(Path::new(repo)) != canonical(&main) {
            item.problem = Some("its repo is not the one status.json names".into());
            return item;
        }
    }
    item.repo_path = Some(main.display().to_string());
    match git(wt, &["status", "--porcelain", "--untracked-files=normal"]) {
        Ok(out) => {
            item.changed_files = out.lines().filter(|l| !l.trim().is_empty()).count();
            item.clean = item.changed_files == 0;
            item.removable = item.clean;
        }
        Err(err) => item.problem = Some(err),
    }
    item
}

pub fn list_worktrees(run_dir: &Path) -> Vec<WorktreeItem> {
    recorded(run_dir)
        .iter()
        .map(|(id, wt, repo, branch)| inspect(id, wt, repo.as_deref(), branch.as_deref()))
        .collect()
}

/// Removes the clean worktrees of `pr_ids` (checked again right now).
pub fn remove_clean(run_dir: &Path, pr_ids: &[String]) -> Vec<RemoveResult> {
    let run_id = run_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
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
            if let Err(err) = git(repo, &["worktree", "remove", &item.worktree]) {
                result.message = format!("kept: {err}");
                return result;
            }
            result.removed = true;
            result.message = "worktree removed".into();
            let ours = format!("dcv2/{run_id}-");
            if let Some(branch) = item.branch.as_deref().filter(|b| b.starts_with(&ours)) {
                match git(repo, &["branch", "-d", branch]) {
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
    use crate::run::git::testing::{repo_at, run};
    use crate::test_support::TempDir;
    use serde_json::json;

    #[test]
    fn only_clean_linked_worktrees_are_removed_and_main_checkouts_never() {
        let dir = TempDir::new("janitor");
        let repo = dir.join("ws/mathlib");
        repo_at(&repo);
        let run_id = "r-20261022-1304";
        let wts = dir.join("ws/mathlib-worktrees");
        let clean = wts.join("dcv2-r-20261022-1304-pr1");
        let dirty = wts.join("dcv2-r-20261022-1304-pr2");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "dcv2/r-20261022-1304-pr1",
                clean.to_str().unwrap(),
            ],
        );
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "dcv2/r-20261022-1304-pr2",
                dirty.to_str().unwrap(),
            ],
        );
        std::fs::write(dirty.join("new.txt"), "wip").unwrap();
        let run_dir = dir.join("runs").join(run_id);
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(
            run_dir.join("status.json"),
            json!({"schema": 1, "stage": "done", "prs": [
                {"id": "PR1", "repoPath": repo, "worktree": clean, "branch": "dcv2/r-20261022-1304-pr1"},
                {"id": "PR2", "repoPath": repo, "worktree": dirty, "branch": "dcv2/r-20261022-1304-pr2"},
                {"id": "PR3", "repoPath": repo, "worktree": repo, "branch": "main"}
            ]})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            run_dir.join(".guard.json"),
            json!({"schema": 1, "worktrees": {"PR1": clean, "PR9": dir.join("gone")}}).to_string(),
        )
        .unwrap();
        let items = list_worktrees(&run_dir);
        assert_eq!(items.len(), 4, "{items:?}");
        let by = |id: &str| items.iter().find(|i| i.pr_id == id).unwrap().clone();
        assert!(by("PR1").clean && by("PR1").removable);
        assert!(!by("PR2").clean && !by("PR2").removable);
        assert_eq!(by("PR2").changed_files, 1);
        assert!(!by("PR3").removable, "main checkout");
        assert!(by("PR3").problem.unwrap().contains("main checkout"));
        assert!(!by("PR9").exists);

        let all: Vec<String> = ["PR1", "PR2", "PR3", "PR9"].map(String::from).to_vec();
        let results = remove_clean(&run_dir, &all);
        let r = |id: &str| results.iter().find(|x| x.pr_id == id).unwrap().clone();
        assert!(r("PR1").removed);
        assert!(r("PR1").branch_deleted, "{}", r("PR1").message);
        assert!(!r("PR2").removed);
        assert!(!r("PR3").removed);
        assert!(!clean.exists());
        assert!(dirty.join("new.txt").exists());
        assert!(repo.join("README.md").exists());
        assert!(git(
            &repo,
            &["rev-parse", "--verify", "dcv2/r-20261022-1304-pr2"]
        )
        .is_ok());
    }
}
