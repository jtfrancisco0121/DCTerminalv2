//! Small read-mostly `git` helpers for the run screen. Every call names its
//! folder with `-C`, never inherits a repo from the app's own cwd, and is
//! made with no store lock held (it spawns a process).

use std::path::{Path, PathBuf};
use std::process::Stdio;

/// `git -C <dir> <args>`: stdout (trimmed) on success, else the first stderr line.
pub fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    // Paths come from LLM-written files: never run a repo's fsmonitor hook.
    let output = crate::platform::quiet_command("git")
        .args(["-c", "core.fsmonitor=false"])
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|err| format!("git: {err}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(err
            .lines()
            .next()
            .unwrap_or("git failed")
            .trim()
            .to_string())
    }
}

/// `git -C <dir> <args>`: stdout byte for byte on success (a file's content keeps its last newline), else the
/// first stderr line. Reads at most `max` bytes of it: more is an error.
pub fn git_bytes(dir: &Path, args: &[&str], max: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut child = crate::platform::quiet_command("git")
        .args(["-c", "core.fsmonitor=false"])
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .spawn()
        .map_err(|err| format!("git: {err}"))?;
    let mut out = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        // One byte past the cap says "too large" without reading a huge object.
        let _ = stdout.take(max as u64 + 1).read_to_end(&mut out);
    }
    if out.len() > max {
        let _ = child.kill();
        let _ = child.wait();
        return Err("too large".into());
    }
    let mut err = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut err);
    }
    let status = child.wait().map_err(|e| format!("git: {e}"))?;
    if status.success() {
        Ok(out)
    } else {
        Err(err
            .lines()
            .next()
            .unwrap_or("git failed")
            .trim()
            .to_string())
    }
}

/// The top of the work tree `folder` is in, if any.
pub fn work_tree_top(folder: &Path) -> Option<PathBuf> {
    git(folder, &["rev-parse", "--show-toplevel"])
        .ok()
        .filter(|top| !top.is_empty())
        .map(PathBuf::from)
}

pub fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// `folder` is the top of its own git work tree (a repo or a linked worktree).
pub fn is_repo_top(folder: &Path) -> bool {
    if std::fs::symlink_metadata(folder.join(".git")).is_err() {
        return false;
    }
    work_tree_top(folder).is_some_and(|top| canonical(&top) == canonical(folder))
}

#[cfg(test)]
pub(crate) mod testing {
    use std::path::Path;
    use std::process::Command;

    /// Runs git in `dir` with a fixed identity; panics on failure.
    pub fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} in {}", dir.display());
    }

    /// A repo at `path` with one commit on `main`.
    pub fn repo_at(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        run(path, &["init", "-q", "-b", "main"]);
        std::fs::write(path.join("README.md"), "hi\n").unwrap();
        run(path, &["add", "."]);
        run(path, &["commit", "-q", "-m", "init"]);
    }

    /// An eval run on disk: repo `ws/mathlib`, PR1's worktree on the run's branch with one commit past the
    /// base, and the run folder (`data/runs/<id>`) with status.json at `stage`, request.md, repos.json and
    /// (when `eval`) a meta.json `eval` naming a group of 2.
    pub struct EvalRun {
        pub run_dir: std::path::PathBuf,
        pub repo: std::path::PathBuf,
        pub worktree: std::path::PathBuf,
        pub branch: String,
    }

    pub const EVAL_RUN_ID: &str = "r-20261105-0900-abcd";

    pub fn eval_run(root: &Path, stage: &str, eval: bool) -> EvalRun {
        let repo = root.join("ws/mathlib");
        repo_at(&repo);
        let base = super::git(&repo, &["rev-parse", "HEAD"]).unwrap();
        let branch = format!("dcv2/{EVAL_RUN_ID}-pr1");
        let worktree = root.join(format!("ws/mathlib-worktrees/dcv2-{EVAL_RUN_ID}-pr1"));
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &branch,
                worktree.to_str().unwrap(),
            ],
        );
        std::fs::write(worktree.join("feature.txt"), "x\n").unwrap();
        run(&worktree, &["add", "feature.txt"]);
        run(&worktree, &["commit", "-q", "-m", "implement"]);
        let run_dir = root.join("data/runs").join(EVAL_RUN_ID);
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(
            run_dir.join("request.md"),
            format!(
                "# Request\n\nfolder: {}\n\n## What to do\n\nDo it\n",
                root.join("ws").display()
            ),
        )
        .unwrap();
        std::fs::write(
            run_dir.join("repos.json"),
            serde_json::json!({"schema":1,"repos":[{"name":"mathlib","path":repo}]}).to_string(),
        )
        .unwrap();
        std::fs::write(
            run_dir.join("status.json"),
            serde_json::json!({"schema":1,"stage":stage,"waitingFor":null,"prs":[
                {"id":"PR1","repoPath":repo,"worktree":worktree,"baseCommit":base,"state":"reviewed"}]})
            .to_string(),
        )
        .unwrap();
        let meta = if eval {
            serde_json::json!({"schema":1,"eval":{"group":"b-1","index":1,"of":2,"source":"r-0"}})
        } else {
            serde_json::json!({"schema":1})
        };
        std::fs::write(run_dir.join("meta.json"), meta.to_string()).unwrap();
        EvalRun {
            run_dir,
            repo,
            worktree,
            branch,
        }
    }
}
