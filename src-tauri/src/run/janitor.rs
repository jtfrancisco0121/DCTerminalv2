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

/// The git common dir of the repo `path` is a checkout of (the shared `.git`
/// of a normal repo, of a linked worktree, `.git/modules/<name>` of a
/// submodule, or the folder `--separate-git-dir` named).
fn common_dir_of(path: &Path) -> Result<PathBuf, String> {
    git(
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .map(|common| canonical(Path::new(&common)))
}

/// The common dir `worktree` shares with its repo, once `worktree` is known to
/// be a linked worktree (not a main checkout).
fn linked_common(worktree: &Path) -> Result<PathBuf, String> {
    let git_dir = git(worktree, &["rev-parse", "--absolute-git-dir"])?;
    let common = common_dir_of(worktree)?;
    if canonical(Path::new(&git_dir)) == common {
        return Err("this is a main checkout, not a worktree".into());
    }
    Ok(common)
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

/// The worktree of `pr_id`, canonical, only when it passes
/// [`verify_worktree`] (where the contract puts it, a linked worktree of
/// the PR's repo, on the run's branch). What the PR card's Reveal, editor
/// and shell actions act on; never a path the page or the skill hands over
/// by itself.
pub fn verified_worktree(run_dir: &Path, pr_id: &str) -> Result<PathBuf, String> {
    let run_id = run_id_of(run_dir);
    let (_, worktree, repo, _) = recorded(run_dir)
        .into_iter()
        .find(|(id, ..)| id == pr_id)
        .ok_or_else(|| "no worktree is recorded for this PR".to_string())?;
    verify_worktree(&run_id, pr_id, &worktree, repo.as_deref())?;
    Ok(canonical(&worktree))
}

/// One PR's worktree, verified or not, for the PR card's buttons.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeCheck {
    pub pr_id: String,
    pub verified: bool,
    pub reason: Option<String>,
}

pub fn check_worktrees(run_dir: &Path) -> Vec<WorktreeCheck> {
    recorded(run_dir)
        .into_iter()
        .map(|(id, ..)| {
            let verdict = verified_worktree(run_dir, &id);
            WorktreeCheck {
                verified: verdict.is_ok(),
                reason: verdict.err(),
                pr_id: id,
            }
        })
        .collect()
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
    let common = linked_common(worktree)?;
    // The repo status.json names owns the worktree when both share one git common
    // dir. That holds for a start folder that is itself a linked worktree, a
    // submodule and a `--separate-git-dir` repo, where the common dir's parent
    // is not the checkout. Without a recorded repo the common dir's parent is it.
    let main = match repo {
        Some(repo) => {
            let repo = canonical(Path::new(repo));
            if repo == canonical(worktree) {
                return Err("this is the repo's main checkout".into());
            }
            if common_dir_of(&repo).ok().as_ref() != Some(&common) {
                return Err("its repo is not the one status.json names".into());
            }
            repo
        }
        None => common
            .parent()
            .map(canonical)
            .ok_or("no main checkout found")?,
    };
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

/// `.guard.json` `copied`: the git-ignored files `state.sh worktree` copied
/// into each PR's worktree from the main checkout (copies: they go with it).
fn copied_files(run_dir: &Path) -> std::collections::HashMap<String, Vec<String>> {
    read_json_value(&run_dir.join(".guard.json"))
        .and_then(|g| g.get("copied").cloned())
        .and_then(|c| serde_json::from_value(c).ok())
        .unwrap_or_default()
}

fn inspect(
    run_id: &str,
    id: &str,
    wt: &Path,
    repo: Option<&str>,
    branch: Option<&str>,
    copied: &[String],
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
        Ok(mut dirt) => {
            dirt.kept_ignored.retain(|path| !copied.contains(path));
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
    let copied = copied_files(run_dir);
    recorded(run_dir)
        .iter()
        .map(|(id, wt, repo, branch)| {
            let files = copied.get(id).map(Vec::as_slice).unwrap_or(&[]);
            inspect(&run_id, id, wt, repo.as_deref(), branch.as_deref(), files)
        })
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

/// A branch a finished eval run left behind after its worktree was removed: `git branch -d` keeps it
/// because its Implementer commits are not merged anywhere. Listed in the eval results with a delete the
/// user confirms (`eval_delete_branch`).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KeptBranch {
    pub pr_id: String,
    pub branch: String,
    pub repo_path: String,
    /// Commits on the branch past the run's base commit (`git rev-list --count <baseCommit>..<branch>`);
    /// `None` when it could not be counted.
    pub unmerged_commits: Option<u32>,
}

/// The repo a status.json `repoPath` names, only when it is a git repo top that is one of the run's repos
/// (listed in repos.json, or inside the start folder): the file is written by an LLM-driven skill.
pub(crate) fn verified_repo(run_dir: &Path, repo: &str) -> Option<PathBuf> {
    let repo = canonical(Path::new(repo));
    if !super::git::is_repo_top(&repo) {
        return None;
    }
    let listed = read_json_value(&run_dir.join("repos.json"))
        .and_then(|v| v.get("repos").and_then(|r| r.as_array().cloned()))
        .is_some_and(|repos| {
            repos.iter().any(|r| {
                r.get("path")
                    .and_then(|p| p.as_str())
                    .is_some_and(|p| canonical(Path::new(p)) == repo)
            })
        });
    let inside = super::folder::run_folder_of(run_dir)
        .map(|f| canonical(Path::new(&f)))
        .is_some_and(|folder| repo.starts_with(folder));
    (listed || inside).then_some(repo)
}

/// Whether some worktree of `repo` has `branch` checked out.
fn branch_has_worktree(repo: &Path, branch: &str) -> bool {
    let want = format!("branch refs/heads/{branch}");
    git(repo, &["worktree", "list", "--porcelain"])
        .map(|out| out.lines().any(|line| line == want))
        .unwrap_or(true) // when in doubt, a worktree is on it
}

/// Each PR's `expected_branch` that exists in its verified repo with no worktree on it.
pub fn kept_branches(run_dir: &Path) -> Vec<KeptBranch> {
    let run_id = run_id_of(run_dir);
    let Some(status) = read_json_value(&run_dir.join("status.json")) else {
        return Vec::new();
    };
    let prs = status
        .get("prs")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for pr in &prs {
        let Some(id) = pr.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(repo) = pr
            .get("repoPath")
            .and_then(|v| v.as_str())
            .and_then(|p| verified_repo(run_dir, p))
        else {
            continue;
        };
        if !super::status::valid_pr_id(id) || out.iter().any(|b: &KeptBranch| b.pr_id == id) {
            continue;
        }
        let branch = expected_branch(&run_id, id);
        let exists = git(
            &repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .is_ok();
        if !exists || branch_has_worktree(&repo, &branch) {
            continue;
        }
        let base = pr
            .get("baseCommit")
            .and_then(|v| v.as_str())
            .filter(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_hexdigit()));
        let unmerged_commits = base.and_then(|base| {
            git(
                &repo,
                &[
                    "rev-list",
                    "--count",
                    &format!("{base}..refs/heads/{branch}"),
                ],
            )
            .ok()
            .and_then(|n| n.parse().ok())
        });
        out.push(KeptBranch {
            pr_id: id.to_string(),
            branch,
            repo_path: repo.display().to_string(),
            unmerged_commits,
        });
    }
    out
}

// ---- scratch worktrees of check re-runs (AF13) ------------------------------------------------
//
// A re-run builds a detached worktree at the recorded commit, runs there and removes it. It is the only place the
// app removes a worktree with `--force`, so it acts on nothing but a path that matches the scratch name, sits in
// the repo's own `-worktrees` folder and that git lists as `detached`. Contract worktrees (`dcv2-<run>-<pr>`) never
// match, and no branch is touched.

/// A commit id as git prints it: hex only, so it can never be read as an option or a ref expression.
pub fn valid_commit(text: &str) -> bool {
    (7..=64).contains(&text.len()) && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// `dcv2-replay-<runId>-<pr>-<n>`; `r-` starts every run id.
fn scratch_name_ok(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("dcv2-replay-r-") else {
        return false;
    };
    let Some((head, n)) = rest.rsplit_once('-') else {
        return false;
    };
    let Some((id, pr)) = head.rsplit_once("-pr") else {
        return false;
    };
    !id.is_empty()
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !pr.is_empty()
        && pr.chars().all(|c| c.is_ascii_digit())
        && !n.is_empty()
        && n.chars().all(|c| c.is_ascii_digit())
}

/// The scratch worktree path of re-run `n` of `pr_id`: `<repo parent>/<repo>-worktrees/dcv2-replay-<runId>-<pr>-<n>`.
pub fn scratch_worktree(repo: &Path, run_id: &str, pr_id: &str, n: u32) -> Option<PathBuf> {
    if !super::status::valid_pr_id(pr_id) {
        return None;
    }
    let name = format!("dcv2-replay-{run_id}-{}-{n}", pr_id.to_ascii_lowercase());
    if !scratch_name_ok(&name) {
        return None;
    }
    let repo = canonical(repo);
    let repo_name = repo.file_name()?.to_string_lossy().to_string();
    Some(
        repo.parent()?
            .join(format!("{repo_name}-worktrees"))
            .join(name),
    )
}

/// The same path, folded for comparison (lowercase on Windows).
fn ident(path: &Path) -> String {
    let text = canonical(path).to_string_lossy().to_string();
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

/// The scratch path is where [`scratch_worktree`] puts it for `repo`: the name matches and the parent is the repo's
/// own `-worktrees` folder.
fn scratch_in_place(repo: &Path, path: &Path) -> Result<(), String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if !scratch_name_ok(&name) {
        return Err("not a re-run scratch worktree".into());
    }
    let repo = canonical(repo);
    let repo_name = repo
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let want = repo
        .parent()
        .map(|p| p.join(format!("{repo_name}-worktrees")));
    let parent = path.parent();
    match (want, parent) {
        (Some(want), Some(parent)) if ident(&want) == ident(parent) => Ok(()),
        _ => Err("not in this repo's worktrees folder".into()),
    }
}

/// `git worktree add --detach -- <path> <commit>` for a scratch path. Refuses a path that exists.
pub fn add_scratch(repo: &Path, path: &Path, commit: &str) -> Result<(), String> {
    scratch_in_place(repo, path)?;
    if !valid_commit(commit) {
        return Err("not a commit id".into());
    }
    if std::fs::symlink_metadata(path).is_ok() {
        return Err("the scratch folder already exists".into());
    }
    let path_text = path.to_string_lossy().to_string();
    git(
        repo,
        &["worktree", "add", "--detach", "--", &path_text, commit],
    )
    .map(|_| ())
}

/// One entry of `git worktree list --porcelain`.
struct Listed {
    path: PathBuf,
    detached: bool,
    branch: bool,
    locked: bool,
}

fn listed_worktrees(repo: &Path) -> Result<Vec<Listed>, String> {
    let out = git(repo, &["worktree", "list", "--porcelain"])?;
    let mut all: Vec<Listed> = Vec::new();
    for line in out.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            all.push(Listed {
                path: PathBuf::from(path),
                detached: false,
                branch: false,
                locked: false,
            });
        } else if let Some(entry) = all.last_mut() {
            if line == "detached" {
                entry.detached = true;
            } else if line.starts_with("branch ") {
                entry.branch = true;
            } else if line == "locked" || line.starts_with("locked ") {
                entry.locked = true;
            }
        }
    }
    Ok(all)
}

/// Removes a scratch worktree. It acts only when the name matches, the parent is the repo's own `-worktrees`
/// folder and git lists the path as `detached` (no branch). A matching entry git lists as `locked` (a crash during
/// `worktree add` leaves `locked initializing`) is unlocked first. A scratch folder git no longer lists is only
/// pruned. No branch is touched.
pub fn remove_scratch(repo: &Path, path: &Path) -> Result<(), String> {
    scratch_in_place(repo, path)?;
    let listed = listed_worktrees(repo)?;
    let mine = listed.iter().find(|l| ident(&l.path) == ident(path));
    let Some(entry) = mine else {
        // Not listed: nothing to remove with git. A folder left behind is not ours to delete.
        let _ = git(repo, &["worktree", "prune"]);
        return if path.exists() {
            Err("git does not list this folder as a worktree".into())
        } else {
            Ok(())
        };
    };
    if entry.branch || !entry.detached {
        return Err("not a detached worktree".into());
    }
    let path_text = path.to_string_lossy().to_string();
    if entry.locked {
        git(repo, &["worktree", "unlock", "--", &path_text])?;
    }
    git(repo, &["worktree", "remove", "--force", "--", &path_text])?;
    let _ = git(repo, &["worktree", "prune"]);
    Ok(())
}

/// `path` relative to `prefix`, comparing the real paths when both exist and the spelled ones otherwise
/// (a removed worktree has no real path), so a run whose worktrees were cleaned up still maps.
pub fn strip_worktree_prefix(path: &Path, prefix: &Path) -> Option<PathBuf> {
    let fold = |p: PathBuf| {
        if cfg!(windows) {
            PathBuf::from(p.to_string_lossy().to_lowercase())
        } else {
            p
        }
    };
    for (a, b) in [
        (canonical(path), canonical(prefix)),
        (path.to_path_buf(), prefix.to_path_buf()),
    ] {
        if let Ok(rel) = fold(a).strip_prefix(fold(b)) {
            return Some(rel.to_path_buf());
        }
    }
    None
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
        // A file state.sh copied in (copyFiles) goes with the worktree too.
        std::fs::write(clean.join(".env"), "K=1").unwrap();
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
            json!({"schema": 1, "worktrees": {"PR1": clean, "PR8": dir.join("ws/foreign"), "PR9": dir.join("gone")},
                   "copied": {"PR1": [".env"], "PR2": [".env"]}}).to_string(),
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

    /// The run's own worktree of `repo` (made where the contract puts it) is accepted.
    fn own_worktree_ok(repo: &Path) {
        let wt = expected_worktree(repo, RUN, "PR1").unwrap();
        add(repo, &format!("dcv2/{RUN}-pr1"), &wt);
        let main = verify_worktree(RUN, "PR1", &wt, Some(repo.to_str().unwrap()))
            .unwrap_or_else(|err| panic!("{} refused: {err}", repo.display()));
        assert_eq!(canonical(&main), canonical(repo));
        // The same worktree is not accepted for a different repo.
        let other = repo.parent().unwrap().join("unrelated");
        repo_at(&other);
        assert_eq!(
            verify_worktree(RUN, "PR1", &wt, Some(other.to_str().unwrap())).unwrap_err(),
            "its repo is not the one status.json names"
        );
        // And the janitor lists it as removable.
        let run_dir = repo.parent().unwrap().join("runs").join(RUN);
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(
            run_dir.join("status.json"),
            json!({"schema": 1, "stage": "done", "prs": [
                {"id": "PR1", "repoPath": repo, "worktree": wt}
            ]})
            .to_string(),
        )
        .unwrap();
        let items = list_worktrees(&run_dir);
        assert!(items[0].removable, "{:?}", items[0]);
    }

    #[test]
    fn a_start_folder_that_is_itself_a_linked_worktree_owns_its_worktrees() {
        let dir = TempDir::new("janitor_linked_start");
        let main = dir.join("main");
        repo_at(&main);
        let start = dir.join("ws/start");
        std::fs::create_dir_all(dir.join("ws")).unwrap();
        add(&main, "feature/start", &start);
        own_worktree_ok(&start);
    }

    #[test]
    fn a_submodule_owns_its_worktrees() {
        let dir = TempDir::new("janitor_submodule");
        let inner = dir.join("inner");
        repo_at(&inner);
        let sup = dir.join("super");
        repo_at(&sup);
        run(
            &sup,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                inner.to_str().unwrap(),
                "sub",
            ],
        );
        let sub = sup.join("sub");
        assert!(sub.join(".git").is_file(), "a submodule's .git is a file");
        own_worktree_ok(&sub);
    }

    #[test]
    fn a_separate_git_dir_repo_owns_its_worktrees() {
        let dir = TempDir::new("janitor_separate");
        let work = dir.join("ws/work");
        let git_dir = dir.join("gitdirs/work.git");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(git_dir.parent().unwrap()).unwrap();
        run(
            &work,
            &[
                "init",
                "-q",
                "-b",
                "main",
                "--separate-git-dir",
                git_dir.to_str().unwrap(),
            ],
        );
        std::fs::write(work.join("README.md"), "hi\n").unwrap();
        run(&work, &["add", "."]);
        run(&work, &["commit", "-q", "-m", "init"]);
        own_worktree_ok(&work);
    }

    #[test]
    fn kept_branches_lists_the_expected_branch_with_its_unmerged_commit_count() {
        use crate::run::git::testing::eval_run;
        let dir = TempDir::new("janitor_kept");
        let ev = eval_run(dir.path(), "ship-approval", true);
        assert!(
            kept_branches(&ev.run_dir).is_empty(),
            "a worktree is on the branch"
        );
        let results = remove_clean(&ev.run_dir, &["PR1".to_string()]);
        assert!(results[0].removed && !results[0].branch_deleted);
        let kept = kept_branches(&ev.run_dir);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(kept[0].branch, ev.branch);
        assert_eq!(kept[0].unmerged_commits, Some(1));
        assert_eq!(kept[0].repo_path, canonical(&ev.repo).display().to_string());

        // A repo the run does not own is never listed.
        let status = std::fs::read_to_string(ev.run_dir.join("status.json")).unwrap();
        let foreign = dir.join("elsewhere");
        repo_at(&foreign);
        let moved = status.replace(
            &ev.repo.display().to_string(),
            &foreign.display().to_string(),
        );
        std::fs::write(ev.run_dir.join("status.json"), moved).unwrap();
        assert!(kept_branches(&ev.run_dir).is_empty());
    }

    #[test]
    fn a_scratch_worktree_is_added_detached_and_removed_leaving_the_run_untouched() {
        let dir = TempDir::new("janitor_scratch");
        let repo = dir.join("ws/mathlib");
        repo_at(&repo);
        let head = git(&repo, &["rev-parse", "HEAD"]).unwrap();
        let contract = expected_worktree(&repo, "r-20261105-130000-cd34", "PR1").unwrap();
        add(&repo, "dcv2/r-20261105-130000-cd34-pr1", &contract);
        let scratch = scratch_worktree(&repo, "r-20261105-130000-cd34", "PR1", 2).unwrap();
        assert!(scratch
            .to_string_lossy()
            .ends_with("mathlib-worktrees/dcv2-replay-r-20261105-130000-cd34-pr1-2"));
        add_scratch(&repo, &scratch, &head).unwrap();
        assert!(scratch.join("README.md").is_file());
        let porcelain = git(&repo, &["worktree", "list", "--porcelain"]).unwrap();
        assert!(porcelain.contains("detached"), "{porcelain}");
        // An existing path is refused; so is a commit that is not hex.
        assert!(add_scratch(&repo, &scratch, &head)
            .unwrap_err()
            .contains("already exists"));
        let other = scratch_worktree(&repo, "r-20261105-130000-cd34", "PR1", 3).unwrap();
        assert!(add_scratch(&repo, &other, "--detach").is_err());
        remove_scratch(&repo, &scratch).unwrap();
        assert!(!scratch.exists());
        // Gone already: only pruned.
        remove_scratch(&repo, &scratch).unwrap();
        // The contract worktree and its branch are untouched.
        assert!(contract.is_dir());
        assert!(git(&repo, &["branch", "--list"])
            .unwrap()
            .contains("dcv2/r-20261105-130000-cd34-pr1"));
    }

    #[test]
    fn remove_scratch_refuses_a_contract_worktree_a_wrong_parent_and_a_non_detached_path() {
        let dir = TempDir::new("janitor_scratch_refuse");
        let repo = dir.join("ws/mathlib");
        repo_at(&repo);
        let head = git(&repo, &["rev-parse", "HEAD"]).unwrap();
        // A contract worktree: the name does not match.
        let contract = expected_worktree(&repo, "r-20261105-130000-cd34", "PR1").unwrap();
        add(&repo, "dcv2/r-20261105-130000-cd34-pr1", &contract);
        assert!(remove_scratch(&repo, &contract)
            .unwrap_err()
            .contains("not a re-run scratch"));
        assert!(contract.is_dir());
        // A scratch-named detached worktree in another folder: the parent is wrong.
        let wrong = dir.join("elsewhere/dcv2-replay-r-20261105-130000-cd34-pr1-1");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                wrong.to_str().unwrap(),
                &head,
            ],
        );
        assert!(remove_scratch(&repo, &wrong)
            .unwrap_err()
            .contains("worktrees folder"));
        assert!(wrong.is_dir());
        // A scratch-named worktree in the right folder but on a branch: not detached.
        let on_branch = scratch_worktree(&repo, "r-20261105-130000-cd34", "PR1", 5).unwrap();
        add(&repo, "feature/x", &on_branch);
        assert!(remove_scratch(&repo, &on_branch)
            .unwrap_err()
            .contains("not a detached"));
        assert!(on_branch.is_dir());
        // A scratch-named folder git does not list is not ours to delete.
        let plain = scratch_worktree(&repo, "r-20261105-130000-cd34", "PR1", 6).unwrap();
        std::fs::create_dir_all(&plain).unwrap();
        assert!(remove_scratch(&repo, &plain).is_err());
        assert!(plain.is_dir());
        // The name rule.
        assert!(scratch_worktree(&repo, "r-1", "PR1; rm", 1).is_none());
        assert!(scratch_worktree(&repo, "x-1", "PR1", 1).is_none());
    }

    #[test]
    fn a_locked_scratch_entry_is_unlocked_and_removed() {
        let dir = TempDir::new("janitor_scratch_locked");
        let repo = dir.join("ws/mathlib");
        repo_at(&repo);
        let head = git(&repo, &["rev-parse", "HEAD"]).unwrap();
        let scratch = scratch_worktree(&repo, "r-20261105-130000-cd34", "PR1", 1).unwrap();
        add_scratch(&repo, &scratch, &head).unwrap();
        git(
            &repo,
            &[
                "worktree",
                "lock",
                "--reason",
                "initializing",
                scratch.to_str().unwrap(),
            ],
        )
        .unwrap();
        assert!(git(&repo, &["worktree", "list", "--porcelain"])
            .unwrap()
            .contains("locked"));
        remove_scratch(&repo, &scratch).unwrap();
        assert!(!scratch.exists());
    }
}
