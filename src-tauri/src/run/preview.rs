//! The preview panel's reads (AD-17): a text file shown rendered beside the
//! terminal. `preview_file` and `preview_stat` take a path the page names, so
//! every path is canonicalised and must sit inside an allowed root: a run
//! folder of this app (or an adopted one), or the repo root of an open tab
//! or of a run that is not finished. A dotfile, a folder, a symlink out of
//! the roots and a binary file are refused; a read stops at 2 MB.

use serde::Serialize;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

/// A preview shows at most this many bytes of a file.
pub const MAX_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    /// The canonical path that was read.
    pub path: String,
    pub text: String,
    pub truncated: bool,
    pub mtime_ms: u64,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewStat {
    pub exists: bool,
    pub mtime_ms: u64,
    pub size: u64,
}

fn mtime_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// `path` canonical when it is a file inside one of `roots` (canonical) with no dotfile
/// component below that root.
pub fn resolve(path: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let raw = Path::new(path.trim());
    if !raw.is_absolute() || path.chars().any(char::is_control) {
        return Err("not a file path".into());
    }
    let file = raw
        .canonicalize()
        .map_err(|_| "file not found".to_string())?;
    let Some(root) = roots.iter().find(|r| file.starts_with(r)) else {
        return Err("outside the folders a preview may read".into());
    };
    let below = file.strip_prefix(root).unwrap_or(&file);
    if below
        .components()
        .any(|c| matches!(c, Component::Normal(n) if n.to_string_lossy().starts_with('.')))
    {
        return Err("a dotfile is not previewed".into());
    }
    if !file.is_file() {
        return Err("not a file".into());
    }
    Ok(file)
}

/// Reads up to [`MAX_BYTES`] of `file`; `(text, truncated)`. A file with a NUL byte is binary and refused.
pub fn read_capped(file: &Path) -> Result<(String, bool), String> {
    use std::io::Read;
    let handle = std::fs::File::open(file).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    handle
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let truncated = bytes.len() as u64 > MAX_BYTES;
    bytes.truncate(MAX_BYTES as usize);
    if bytes.contains(&0) {
        return Err("a binary file is not previewed".into());
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), truncated))
}

pub fn preview(path: &str, roots: &[PathBuf]) -> Result<Preview, String> {
    let file = resolve(path, roots)?;
    let meta = std::fs::metadata(&file).map_err(|e| e.to_string())?;
    let (text, truncated) = read_capped(&file)?;
    Ok(Preview {
        path: file.display().to_string(),
        text,
        truncated,
        mtime_ms: mtime_ms(&meta),
    })
}

/// `exists: false` for a file that is gone (the pane says "file removed"); a path that exists
/// but may not be read is an error, like in [`preview`].
pub fn stat(path: &str, roots: &[PathBuf]) -> Result<PreviewStat, String> {
    if !Path::new(path.trim()).exists() {
        return Ok(PreviewStat {
            exists: false,
            mtime_ms: 0,
            size: 0,
        });
    }
    let file = resolve(path, roots)?;
    let meta = std::fs::metadata(&file).map_err(|e| e.to_string())?;
    Ok(PreviewStat {
        exists: true,
        mtime_ms: mtime_ms(&meta),
        size: meta.len(),
    })
}

/// The folders a preview may read now: this app's run folders and the adopted ones, the repo
/// root of every open tab, and the repos of every run that is not finished (repos.json).
pub fn allowed_roots(app: &AppHandle) -> Vec<PathBuf> {
    let data = crate::data_dir::app_data_dir(app).path;
    let mut roots: Vec<PathBuf> = Vec::new();
    let runs = super::folder::runs_dir(&data);
    if let Ok(entries) = std::fs::read_dir(&runs) {
        roots.extend(
            entries
                .flatten()
                .filter_map(|e| e.path().canonicalize().ok())
                .filter(|p| p.is_dir()),
        );
    }
    roots.extend(super::folder::adopted_runs(&data));
    for item in super::folder::list_runs(&data) {
        if matches!(item.stage.as_deref(), Some("done") | Some("failed")) {
            continue;
        }
        if let Some(value) = std::fs::read_to_string(Path::new(&item.run_dir).join("repos.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        {
            for repo in value["repos"].as_array().into_iter().flatten() {
                if let Some(p) = repo["path"].as_str().map(PathBuf::from) {
                    roots.extend(p.canonicalize().ok());
                }
            }
        }
    }
    let cwds: Vec<String> = app
        .try_state::<Mutex<crate::store::StateStore>>()
        .and_then(|s| {
            s.lock()
                .ok()
                .map(|s| s.sorted_tabs().iter().map(|t| t.cwd.clone()).collect())
        })
        .unwrap_or_default();
    for cwd in cwds {
        let cwd = PathBuf::from(cwd);
        roots.extend(super::git::work_tree_top(&cwd).and_then(|p| p.canonicalize().ok()));
    }
    roots.sort();
    roots.dedup();
    roots
}

#[tauri::command]
pub async fn preview_file(app: AppHandle, path: String) -> Result<Preview, String> {
    tauri::async_runtime::spawn_blocking(move || preview(&path, &allowed_roots(&app)))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn preview_stat(app: AppHandle, path: String) -> Result<PreviewStat, String> {
    tauri::async_runtime::spawn_blocking(move || stat(&path, &allowed_roots(&app)))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("dcv2-preview-{name}-{}", std::process::id()))
            .canonicalize()
            .unwrap_or_else(|_| std::env::temp_dir());
        let dir = dir.join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    #[test]
    fn a_file_inside_a_root_is_read() {
        let root = temp("ok");
        std::fs::write(root.join("plan.md"), "# Plan\n").unwrap();
        let p = preview(
            root.join("plan.md").to_str().unwrap(),
            std::slice::from_ref(&root),
        )
        .unwrap();
        assert_eq!(p.text, "# Plan\n");
        assert!(!p.truncated);
        assert!(p.mtime_ms > 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn dot_dot_symlink_dotfile_and_outside_paths_are_refused() {
        let root = temp("deny");
        let outside = temp("deny-outside");
        std::fs::write(outside.join("secret.md"), "s").unwrap();
        std::fs::write(root.join(".env"), "KEY=1").unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/config"), "x").unwrap();
        std::os::unix::fs::symlink(outside.join("secret.md"), root.join("link.md")).unwrap();
        let roots = vec![root.clone()];
        let via_dots = root
            .join("..")
            .join(outside.strip_prefix(outside.parent().unwrap()).unwrap())
            .join("secret.md");
        assert!(resolve(via_dots.to_str().unwrap(), &roots).is_err(), "..");
        assert!(
            resolve(root.join("link.md").to_str().unwrap(), &roots).is_err(),
            "symlink out"
        );
        assert_eq!(
            resolve(root.join(".env").to_str().unwrap(), &roots).unwrap_err(),
            "a dotfile is not previewed"
        );
        assert!(
            resolve(root.join(".git/config").to_str().unwrap(), &roots).is_err(),
            "dot folder"
        );
        assert!(
            resolve(outside.join("secret.md").to_str().unwrap(), &roots).is_err(),
            "outside root"
        );
        assert!(resolve("relative/path.md", &roots).is_err());
        assert!(resolve(root.to_str().unwrap(), &roots).is_err(), "a folder");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn a_big_file_is_cut_at_two_megabytes_with_the_flag_and_a_binary_one_is_refused() {
        let root = temp("big");
        std::fs::write(root.join("big.md"), "a".repeat(MAX_BYTES as usize + 10)).unwrap();
        let p = preview(
            root.join("big.md").to_str().unwrap(),
            std::slice::from_ref(&root),
        )
        .unwrap();
        assert!(p.truncated);
        assert_eq!(p.text.len() as u64, MAX_BYTES);
        std::fs::write(root.join("exact.md"), "b".repeat(MAX_BYTES as usize)).unwrap();
        assert!(
            !preview(
                root.join("exact.md").to_str().unwrap(),
                std::slice::from_ref(&root)
            )
            .unwrap()
            .truncated
        );
        std::fs::write(root.join("bin.dat"), [0u8, 1, 2]).unwrap();
        assert!(preview(
            root.join("bin.dat").to_str().unwrap(),
            std::slice::from_ref(&root)
        )
        .is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stat_reports_a_removed_file_and_refuses_one_outside() {
        let root = temp("stat");
        let outside = temp("stat-outside");
        std::fs::write(outside.join("o.md"), "o").unwrap();
        let gone = root.join("gone.md");
        assert!(
            !stat(gone.to_str().unwrap(), std::slice::from_ref(&root))
                .unwrap()
                .exists
        );
        std::fs::write(root.join("a.md"), "x").unwrap();
        assert!(
            stat(
                root.join("a.md").to_str().unwrap(),
                std::slice::from_ref(&root)
            )
            .unwrap()
            .exists
        );
        assert!(stat(
            outside.join("o.md").to_str().unwrap(),
            std::slice::from_ref(&root)
        )
        .is_err());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
