//! Small read-mostly `git` helpers for the run screen. Every call names its
//! folder with `-C`, never inherits a repo from the app's own cwd, and is
//! made with no store lock held (it spawns a process).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// `git -C <dir> <args>`: stdout (trimmed) on success, else the first stderr line.
pub fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
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
}
