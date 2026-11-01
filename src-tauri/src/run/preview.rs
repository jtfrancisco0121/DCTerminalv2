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

// ---- clickable file paths in terminal output (AD-18) -----------------------------------------

/// A path the terminal printed is linked only when it exists inside the tab's folder, its run
/// folder or an open repo; a batch is at most this many candidates.
pub const MAX_CANDIDATES: usize = 50;

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PathHit {
    pub candidate: String,
    /// The canonical path when the candidate is an existing file inside the roots.
    pub path: Option<String>,
    /// `preview_file` allows it and it is a Markdown file or a run-folder file: open it in the preview.
    pub previewable: bool,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The existing files among `candidates`: relative paths are taken from `cwd`, `~/` from the home
/// folder, absolute ones as they are; each is canonicalised and kept only inside `roots`.
/// `preview_roots` and `run_roots` decide `previewable`.
pub fn check_candidates(
    cwd: &Path,
    candidates: &[String],
    roots: &[PathBuf],
    preview_roots: &[PathBuf],
    run_roots: &[PathBuf],
) -> Vec<PathHit> {
    candidates
        .iter()
        .take(MAX_CANDIDATES)
        .map(|candidate| {
            let raw = candidate.trim();
            let full = if let Some(rest) = raw.strip_prefix("~/") {
                home_dir().map(|h| h.join(rest))
            } else if Path::new(raw).is_absolute() {
                Some(PathBuf::from(raw))
            } else if raw.is_empty() || raw.chars().any(char::is_control) {
                None
            } else {
                Some(cwd.join(raw))
            };
            let canonical = full
                .and_then(|p| p.canonicalize().ok())
                .filter(|p| p.is_file() && roots.iter().any(|r| p.starts_with(r)));
            let previewable = canonical.as_ref().is_some_and(|p| {
                let md = p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                    e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown")
                });
                resolve(&p.to_string_lossy(), preview_roots).is_ok()
                    && (md || run_roots.iter().any(|r| p.starts_with(r)))
            });
            PathHit {
                candidate: candidate.clone(),
                path: canonical.map(|p| p.display().to_string()),
                previewable,
            }
        })
        .collect()
}

/// The folders a tab's links may point into: its folder, its run folder, and the repo roots of every open tab.
fn link_roots(app: &AppHandle, tab_id: &str) -> Option<(PathBuf, Vec<PathBuf>)> {
    let state = app.try_state::<Mutex<crate::store::StateStore>>()?;
    let (cwd, run_dir, others) = {
        let state = state.lock().ok()?;
        let tab = state.tab_by_id(tab_id)?;
        (
            PathBuf::from(&tab.cwd),
            tab.launch.as_ref().and_then(|l| l.run_dir.clone()),
            state
                .sorted_tabs()
                .iter()
                .map(|t| t.cwd.clone())
                .collect::<Vec<_>>(),
        )
    };
    let cwd = cwd.canonicalize().ok()?;
    let mut roots = vec![cwd.clone()];
    roots.extend(run_dir.and_then(|d| PathBuf::from(d).canonicalize().ok()));
    for other in others {
        roots.extend(
            super::git::work_tree_top(Path::new(&other)).and_then(|p| p.canonicalize().ok()),
        );
    }
    roots.sort();
    roots.dedup();
    Some((cwd, roots))
}

#[tauri::command]
pub async fn paths_exist(
    app: AppHandle,
    tab_id: String,
    candidates: Vec<String>,
) -> Result<Vec<PathHit>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (cwd, roots) = link_roots(&app, &tab_id).ok_or("the tab is gone")?;
        let preview_roots = allowed_roots(&app);
        let data = crate::data_dir::app_data_dir(&app).path;
        let runs = super::folder::runs_dir(&data).canonicalize().ok();
        let run_roots: Vec<PathBuf> = preview_roots
            .iter()
            .filter(|r| {
                runs.as_ref()
                    .is_some_and(|runs| r.parent() == Some(runs.as_path()))
            })
            .cloned()
            .collect();
        Ok(check_candidates(
            &cwd,
            &candidates,
            &roots,
            &preview_roots,
            &run_roots,
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The argument vector (program first) that opens `file` in the editor `id` at `line`/`col`;
/// `finder` reveals the file instead. Never a shell string. `has_cli` says whether the editor's CLI is on the PATH.
/// The choice is `platform::editor_argv` (macOS `open`, Windows Explorer).
pub fn editor_argv(
    id: &str,
    file: &Path,
    line: Option<u32>,
    col: Option<u32>,
    has_cli: impl Fn(&str) -> bool,
) -> Vec<String> {
    let os = if cfg!(windows) { "windows" } else { "macos" };
    crate::platform::editor_argv(os, id, file, line, col, has_cli)
}

fn on_path(program: &str) -> bool {
    crate::platform::which(program).is_some()
}

/// Opens a linked file in the chosen editor (or reveals it in Finder). The path must be one `paths_exist` would link.
#[tauri::command]
pub async fn open_file_link(
    app: AppHandle,
    tab_id: String,
    path: String,
    line: Option<u32>,
    col: Option<u32>,
    editor: Option<String>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (cwd, roots) = link_roots(&app, &tab_id).ok_or("the tab is gone")?;
        let hit = check_candidates(&cwd, &[path], &roots, &[], &[]).remove(0);
        let file = hit
            .path
            .ok_or("that file is not inside the tab's folder, its run folder or an open repo")?;
        let argv = editor_argv(
            editor.as_deref().unwrap_or("finder"),
            Path::new(&file),
            line,
            col,
            on_path,
        );
        // A Windows editor CLI is usually a batch shim (`code.cmd`): spawn the resolved full path
        // (std runs .cmd/.bat through cmd with its argument escaping).
        let program = crate::platform::which(&argv[0])
            .map(|p| p.into_os_string())
            .unwrap_or_else(|| argv[0].clone().into());
        crate::platform::quiet_command(program)
            .args(&argv[1..])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("could not start {}: {e}", argv[0]))
    })
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

    #[cfg(unix)]
    #[test]
    fn only_existing_files_inside_the_roots_are_linked() {
        let root = temp("links");
        let outside = temp("links-outside");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "x").unwrap();
        std::fs::write(root.join("notes.md"), "x").unwrap();
        std::fs::write(outside.join("o.md"), "x").unwrap();
        std::os::unix::fs::symlink(outside.join("o.md"), root.join("escape.md")).unwrap();
        let roots = vec![root.clone()];
        let cands: Vec<String> = [
            "src/a.rs",
            "./src/a.rs",
            "missing.rs",
            "src/../../outside.md",
            "escape.md",
            "notes.md",
            "src",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let hits = check_candidates(&root, &cands, &roots, &roots, &[]);
        let linked: Vec<&str> = hits
            .iter()
            .filter(|h| h.path.is_some())
            .map(|h| h.candidate.as_str())
            .collect();
        assert_eq!(linked, ["src/a.rs", "./src/a.rs", "notes.md"]);
        assert!(hits[5].previewable, "a markdown file inside a preview root");
        assert!(
            !hits[0].previewable,
            "a source file is opened in the editor"
        );
        let abs = outside.join("o.md").display().to_string();
        assert!(check_candidates(&root, &[abs], &roots, &roots, &[])[0]
            .path
            .is_none());
        let many: Vec<String> = (0..80).map(|i| format!("f{i}.rs")).collect();
        assert_eq!(
            check_candidates(&root, &many, &roots, &roots, &[]).len(),
            MAX_CANDIDATES
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn a_markdown_file_outside_the_preview_roots_opens_in_the_editor() {
        let root = temp("links-md");
        std::fs::write(root.join("README.md"), "x").unwrap();
        let hits = check_candidates(
            &root,
            &["README.md".to_string()],
            std::slice::from_ref(&root),
            &[],
            &[],
        );
        assert!(hits[0].path.is_some());
        assert!(!hits[0].previewable);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_editor_command_is_an_argument_vector_with_the_line() {
        let f = Path::new("/w/src/a b.rs");
        assert_eq!(
            editor_argv("vscode", f, Some(12), Some(3), |_| true),
            ["code", "-g", "/w/src/a b.rs:12:3"]
        );
        assert_eq!(
            editor_argv("cursor", f, Some(7), None, |_| true),
            ["cursor", "-g", "/w/src/a b.rs:7"]
        );
        assert_eq!(
            editor_argv("zed", f, Some(7), Some(1), |_| true),
            ["zed", "/w/src/a b.rs:7:1"]
        );
        assert_eq!(
            editor_argv("sublime", f, None, None, |_| true),
            ["subl", "-g", "/w/src/a b.rs"]
        );
        assert_eq!(
            editor_argv("vscode", f, Some(1), None, |_| false),
            ["open", "-a", "Visual Studio Code", "/w/src/a b.rs"]
        );
        assert_eq!(
            editor_argv("finder", f, Some(1), None, |_| true),
            ["open", "-R", "/w/src/a b.rs"]
        );
        assert_eq!(
            editor_argv("rm -rf /", f, None, None, |_| true),
            ["open", "-R", "/w/src/a b.rs"],
            "an unknown editor is Finder"
        );
    }
}
