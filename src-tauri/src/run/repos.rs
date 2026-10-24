//! The repos of a start folder (run-folder.md, repos.json): the folder
//! itself when it is the top of a git repo, else each direct child folder
//! that is one, named after its folder. Ported from DCTerminal's
//! `workspace_repos.rs` (W1), without Settings › Repos. Read-only.

use super::git::{canonical, is_repo_top, work_tree_top};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The child folders looked at, at most, so a huge folder stays fast.
pub const MAX_CHILDREN: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    pub name: String,
    pub path: String,
    /// `null`: none known.
    pub check: Option<String>,
    /// Worktree setup command (app setting); `null`: the plugin detects it
    /// from the lockfile.
    #[serde(default)]
    pub setup: Option<String>,
    /// Git-ignored files copied from the main checkout into run worktrees
    /// (app setting, relative paths); empty by default.
    #[serde(default)]
    pub copy_files: Vec<String>,
}

impl Repo {
    /// Fills `setup` / `copyFiles` from the app's per-repo settings (keyed
    /// by the repo's real path).
    pub fn with_setup(mut self, setup: Option<&crate::store::RepoSetup>) -> Self {
        if let Some(s) = setup {
            self.setup = s.setup.clone();
            self.copy_files = s.copy_files.clone();
        }
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderRepos {
    /// `repo` (the folder is a repo), `workspace` (a folder of repos),
    /// `inside-repo` (a subfolder of a repo), or `none`.
    pub kind: String,
    pub repos: Vec<Repo>,
}

/// What `folder` holds.
pub fn detect(folder: &Path) -> FolderRepos {
    if !folder.is_dir() {
        return FolderRepos {
            kind: "none".into(),
            repos: Vec::new(),
        };
    }
    if let Some(top) = work_tree_top(folder) {
        if canonical(&top) == canonical(folder) {
            return FolderRepos {
                kind: "repo".into(),
                repos: vec![repo_entry(folder)],
            };
        }
        return FolderRepos {
            kind: "inside-repo".into(),
            repos: Vec::new(),
        };
    }
    let repos = child_repos(folder);
    FolderRepos {
        kind: if repos.is_empty() {
            "none"
        } else {
            "workspace"
        }
        .into(),
        repos,
    }
}

fn repo_entry(path: &Path) -> Repo {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".into());
    Repo {
        name,
        path: path.display().to_string(),
        check: detect_check(path),
        setup: None,
        copy_files: Vec::new(),
    }
}

/// Direct child repos: hidden folders skipped, at most [`MAX_CHILDREN`].
pub fn child_repos(folder: &Path) -> Vec<Repo> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut children: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let path = entry.path();
            (!name.starts_with('.') && path.is_dir()).then_some((name, path))
        })
        .collect();
    children.sort();
    children.truncate(MAX_CHILDREN);
    children
        .into_iter()
        .filter(|(_, path)| is_repo_top(path))
        .map(|(_, path)| repo_entry(&path))
        .collect()
}

/// The check per repo, first match wins (run-folder.md).
pub fn detect_check(folder: &Path) -> Option<String> {
    if folder.join("go.mod").is_file() {
        return Some("go test ./...".into());
    }
    if folder.join("server").join("go.mod").is_file() {
        return Some("cd server && go test ./...".into());
    }
    let scripts = std::fs::read_to_string(folder.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|json| json.get("scripts").cloned());
    let script = |name: &str| {
        scripts
            .as_ref()
            .and_then(|s| s.get(name))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    if script("check").is_some() {
        return Some("npm run check".into());
    }
    if folder.join("Cargo.toml").is_file() {
        return Some("cargo test".into());
    }
    if let Some(test) = script("test") {
        if !test.contains("no test specified") {
            return Some("npm test".into());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::git::testing::repo_at;
    use crate::test_support::TempDir;

    #[test]
    fn a_repo_folder_is_one_repo_and_a_folder_of_repos_lists_its_children() {
        let dir = TempDir::new("repos_detect");
        let ws = dir.join("ws");
        repo_at(&ws.join("backend"));
        std::fs::create_dir_all(ws.join("backend/server")).unwrap();
        std::fs::write(ws.join("backend/server/go.mod"), "module x\n").unwrap();
        repo_at(&ws.join("web"));
        std::fs::write(
            ws.join("web/package.json"),
            r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(ws.join("notes")).unwrap();
        std::fs::create_dir_all(ws.join(".hidden")).unwrap();
        let found = detect(&ws);
        assert_eq!(found.kind, "workspace");
        let names: Vec<_> = found
            .repos
            .iter()
            .map(|r| (r.name.as_str(), r.check.as_deref()))
            .collect();
        assert_eq!(
            names,
            [
                ("backend", Some("cd server && go test ./...")),
                ("web", None)
            ]
        );
        let one = detect(&ws.join("web"));
        assert_eq!(one.kind, "repo");
        assert_eq!(one.repos.len(), 1);
        assert_eq!(detect(&ws.join("backend/server")).kind, "inside-repo");
        assert_eq!(detect(&ws.join("notes")).kind, "none");
        assert_eq!(detect(&dir.join("missing")).kind, "none");
    }

    #[test]
    fn checks_follow_the_documented_order() {
        let dir = TempDir::new("repos_checks");
        let p = dir.path();
        assert_eq!(detect_check(p), None);
        std::fs::write(p.join("package.json"), r#"{"scripts":{"test":"vitest"}}"#).unwrap();
        assert_eq!(detect_check(p).as_deref(), Some("npm test"));
        std::fs::write(p.join("Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(detect_check(p).as_deref(), Some("cargo test"));
        std::fs::write(
            p.join("package.json"),
            r#"{"scripts":{"check":"x","test":"vitest"}}"#,
        )
        .unwrap();
        assert_eq!(detect_check(p).as_deref(), Some("npm run check"));
        std::fs::write(p.join("go.mod"), "module x\n").unwrap();
        assert_eq!(detect_check(p).as_deref(), Some("go test ./..."));
    }
}
