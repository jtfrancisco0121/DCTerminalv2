//! A window's tabs: list, add, select, rename, close and reopen. Every
//! command names its window (`window_id`), so two windows never share a
//! sticky focus. Starting a tab's terminal is `pty_open`.

use crate::paths::{folder_status_code, validate_working_folder};
use crate::pty::PtyRegistry;
use crate::store::{StateStore, TabKind, TabRecord};
use serde::Serialize;
use std::sync::Mutex;
use tauri::State;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TabSummary {
    pub id: String,
    pub label: String,
    pub custom_label: bool,
    pub cwd: String,
    pub kind: TabKind,
    /// `ok`, `missing`, `not-a-directory`, … (`paths::FolderError`).
    pub folder_status: String,
    /// A Claude tab that has started a session before (it may be resumable).
    pub has_session: bool,
    /// Eagle-Eye tabs: the run folder the progress panel reads.
    pub run_dir: Option<String>,
    /// dcv2 tabs: `eagle-eye`, `eagle-eye-resume`, or the role.
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClosedTabSummary {
    pub id: String,
    pub label: String,
    pub cwd: String,
    pub kind: TabKind,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppStateSnapshot {
    pub window_id: String,
    pub active_tab_id: Option<String>,
    pub tabs: Vec<TabSummary>,
    /// This window's closed tabs, newest first.
    pub closed_tabs: Vec<ClosedTabSummary>,
}

pub(crate) fn summary(tab: &TabRecord) -> TabSummary {
    TabSummary {
        id: tab.id.clone(),
        label: tab.label.clone(),
        custom_label: tab.custom_label,
        cwd: tab.cwd.clone(),
        kind: tab.kind,
        folder_status: folder_status_code(&tab.cwd),
        has_session: tab.claude_session_id.is_some(),
        run_dir: tab.launch.as_ref().and_then(|l| l.run_dir.clone()),
        role: tab.launch.as_ref().map(|l| l.role.clone()),
    }
}

/// The command's window (`StateStore::bind_window`), as the page sees it.
pub(crate) fn snapshot(store: &StateStore) -> AppStateSnapshot {
    let window = store.focus_window().to_string();
    AppStateSnapshot {
        active_tab_id: store.active_for_window(&window),
        tabs: store
            .tabs_in_window(&window)
            .into_iter()
            .map(summary)
            .collect(),
        closed_tabs: store
            .data
            .closed_tabs
            .iter()
            .filter(|tab| crate::store::window_matches(&tab.window_id, &window))
            .map(|tab| ClosedTabSummary {
                id: tab.id.clone(),
                label: tab.label.clone(),
                cwd: tab.cwd.clone(),
                kind: tab.kind,
            })
            .collect(),
        window_id: window,
    }
}

/// A new tab in `cwd`, checked first, made the window's active tab.
pub(crate) fn add_tab(
    store: &mut StateStore,
    cwd: &str,
    kind: TabKind,
) -> Result<TabSummary, String> {
    let folder = validate_working_folder(cwd).map_err(|err| err.message())?;
    let id = store.create_tab(&folder.display().to_string(), kind)?;
    store
        .tab_by_id(&id)
        .map(summary)
        .ok_or_else(|| "new tab missing".to_string())
}

#[tauri::command]
pub fn get_app_state(
    window_id: Option<String>,
    store: State<Mutex<StateStore>>,
) -> Result<AppStateSnapshot, String> {
    let mut store = store.lock().map_err(|e| e.to_string())?;
    store.bind_window(window_id.as_deref());
    Ok(snapshot(&store))
}

#[tauri::command]
pub fn new_tab(
    window_id: Option<String>,
    cwd: String,
    kind: TabKind,
    store: State<Mutex<StateStore>>,
) -> Result<TabSummary, String> {
    let mut store = store.lock().map_err(|e| e.to_string())?;
    store.bind_window(window_id.as_deref());
    add_tab(&mut store, &cwd, kind)
}

#[tauri::command]
pub fn select_tab(
    app: tauri::AppHandle,
    window_id: Option<String>,
    tab_id: String,
    store: State<Mutex<StateStore>>,
) -> Result<(), String> {
    // The person chose a tab: a pending notification landing is moot.
    crate::observe::attention::clear_landing(&app);
    let mut store = store.lock().map_err(|e| e.to_string())?;
    store.bind_window(window_id.as_deref());
    store.set_active_tab(&tab_id)
}

/// How many recent folders are offered.
pub const RECENT_FOLDERS: usize = 10;

/// The folders to offer, newest first: `(folder, when)` candidates (when an
/// RFC 3339 time; an unreadable one sorts last), folders that no longer
/// exist dropped, the same folder (by canonical path) once, at most
/// [`RECENT_FOLDERS`]. Pure but for `exists`/canonical, which the caller
/// supplies so tests need no disk.
pub(crate) fn recent_from(
    mut candidates: Vec<(String, String)>,
    canonical: impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    let when = |text: &str| {
        chrono::DateTime::parse_from_rfc3339(text)
            .map(|t| t.timestamp_millis())
            .unwrap_or(i64::MIN)
    };
    candidates.sort_by_key(|(_, at)| std::cmp::Reverse(when(at)));
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (folder, _) in candidates {
        if folder.trim().is_empty() {
            continue;
        }
        let Some(real) = canonical(&folder) else {
            continue;
        };
        if seen.insert(real.clone()) {
            out.push(real);
            if out.len() == RECENT_FOLDERS {
                break;
            }
        }
    }
    out
}

/// Up to 10 existing folders for the New tab form and the start screen:
/// from open and closed tabs and from the runs, newest first. The stores
/// are copied out and released before any folder is looked at.
#[tauri::command]
pub async fn recent_folders(
    app: tauri::AppHandle,
    store: State<'_, Mutex<StateStore>>,
) -> Result<Vec<String>, String> {
    let mut candidates: Vec<(String, String)> = {
        let state = store.lock().map_err(|e| e.to_string())?;
        state
            .data
            .tabs
            .iter()
            .map(|tab| (tab.cwd.clone(), tab.created_at.clone()))
            .chain(state.data.closed_tabs.iter().map(|tab| {
                (
                    tab.cwd.clone(),
                    tab.closed_at
                        .clone()
                        .unwrap_or_else(|| tab.created_at.clone()),
                )
            }))
            .collect()
    };
    tauri::async_runtime::spawn_blocking(move || {
        let data = crate::data_dir::app_data_dir(&app).path;
        for run in crate::run::folder::list_runs(&data) {
            if let Some(folder) = run.folder {
                let when = run.updated_at.or(run.created_at).unwrap_or_default();
                candidates.push((folder, when));
            }
        }
        Ok(recent_from(candidates, |folder| {
            let path = std::path::Path::new(folder);
            path.is_dir()
                .then(|| path.canonicalize().ok())
                .flatten()
                .map(|real| real.display().to_string())
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Whether a tab's saved Claude session may follow it to `new_cwd`: only
/// when that session's log is under the new folder's Claude project folder.
pub(crate) fn keeps_session(tab: &TabRecord, new_cwd: &std::path::Path) -> bool {
    match (
        tab.claude_session_id.as_deref(),
        tab.claude_config_dir.as_deref(),
    ) {
        (Some(session), Some(config)) => crate::claude_session_log::session_in_folder(
            std::path::Path::new(config),
            session,
            new_cwd,
        ),
        _ => false,
    }
}

/// Points a tab whose folder is gone at another folder. The tab keeps its
/// id and its name (a name that follows the folder follows the new one); a
/// Claude session is kept only when it belongs to the new folder.
#[tauri::command]
pub fn set_tab_folder(
    tab_id: String,
    cwd: String,
    store: State<Mutex<StateStore>>,
) -> Result<TabSummary, String> {
    let folder = validate_working_folder(&cwd).map_err(|err| err.message())?;
    let folder = folder.canonicalize().unwrap_or(folder);
    // The session log is looked for with no store lock held.
    let tab = store
        .lock()
        .map_err(|e| e.to_string())?
        .tab_by_id(&tab_id)
        .cloned()
        .ok_or_else(|| format!("unknown tab: {tab_id}"))?;
    let keep = keeps_session(&tab, &folder);
    let mut store = store.lock().map_err(|e| e.to_string())?;
    store.set_tab_folder(&tab_id, &folder.display().to_string(), keep)?;
    store
        .tab_by_id(&tab_id)
        .map(summary)
        .ok_or_else(|| format!("unknown tab: {tab_id}"))
}

/// Drags a tab to `index` among the window's tabs; the new order is saved.
#[tauri::command]
pub fn move_tab(
    window_id: Option<String>,
    tab_id: String,
    index: usize,
    store: State<Mutex<StateStore>>,
) -> Result<AppStateSnapshot, String> {
    let mut store = store.lock().map_err(|e| e.to_string())?;
    store.bind_window(window_id.as_deref());
    store.move_tab(&tab_id, index)?;
    Ok(snapshot(&store))
}

#[tauri::command]
pub fn rename_tab(
    tab_id: String,
    label: String,
    store: State<Mutex<StateStore>>,
) -> Result<TabSummary, String> {
    let mut store = store.lock().map_err(|e| e.to_string())?;
    store.set_tab_label(&tab_id, &label)?;
    store
        .tab_by_id(&tab_id)
        .map(summary)
        .ok_or_else(|| format!("unknown tab: {tab_id}"))
}

/// Moves the tab to Reopen Closed Tab, then stops its terminal.
#[tauri::command]
pub fn close_tab(
    app: tauri::AppHandle,
    window_id: Option<String>,
    tab_id: String,
    store: State<Mutex<StateStore>>,
    registry: State<Mutex<PtyRegistry>>,
    usage: State<Mutex<crate::usage::UsageStore>>,
) -> Result<AppStateSnapshot, String> {
    let closed = close_tab_then_kill(&store, &registry, window_id.as_deref(), &tab_id);
    if let Ok(mut usage) = usage.lock() {
        usage.forget_tab(&tab_id);
    }
    crate::observe::forget_tab(&app, &tab_id);
    crate::windows::refresh_recently_closed(&app, window_id.as_deref().unwrap_or("main"));
    // The close may push the oldest closed tab off the list for good.
    crate::usage::remove_gone_status_lines(&app);
    crate::run::spawn_prune_launch_prompts(&app);
    closed
}

/// The store first, the terminal second: an open of the tab in progress
/// checks the store (under the registry lock) right before it spawns, so
/// it either sees the tab gone or has spawned already and is killed here.
/// The store lock is dropped before the registry is taken (lock order:
/// `PtyRegistry` before `StateStore`).
pub(crate) fn close_tab_then_kill(
    store: &Mutex<StateStore>,
    registry: &Mutex<PtyRegistry>,
    window_id: Option<&str>,
    tab_id: &str,
) -> Result<AppStateSnapshot, String> {
    let closed = {
        let mut store = store.lock().map_err(|e| e.to_string())?;
        store.bind_window(window_id);
        store.close_tab(tab_id).map(|()| snapshot(&store))
    };
    crate::pty::kill_tab_pty(registry, tab_id);
    closed
}

/// `tab_id`, or the window's most recently closed tab. Its terminal starts
/// when a view shows it (a Claude tab can resume its session then).
#[tauri::command]
pub fn reopen_closed_tab(
    app: tauri::AppHandle,
    window_id: Option<String>,
    tab_id: Option<String>,
    store: State<Mutex<StateStore>>,
) -> Result<TabSummary, String> {
    let tab = {
        let mut store = store.lock().map_err(|e| e.to_string())?;
        store.bind_window(window_id.as_deref());
        store.reopen_closed_id(tab_id.as_deref())?
    };
    // The state lock is released: the menu is rebuilt from the store.
    crate::windows::refresh_recently_closed(&app, window_id.as_deref().unwrap_or("main"));
    Ok(summary(&tab))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn a_new_tab_needs_a_real_folder_and_shows_in_its_window_only() {
        let dir = TempDir::new("tabs_new");
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let mut store = StateStore::open_path(dir.join("state.json")).unwrap();
        assert!(add_tab(
            &mut store,
            &dir.join("missing").display().to_string(),
            TabKind::Claude
        )
        .unwrap_err()
        .contains("not found"));
        assert!(add_tab(&mut store, "  ", TabKind::Claude).is_err());
        let tab = add_tab(&mut store, &work.display().to_string(), TabKind::Claude).unwrap();
        assert_eq!(tab.label, "Claude · work");
        assert_eq!(tab.folder_status, "ok");
        assert!(!tab.has_session);
        let main = snapshot(&store);
        assert_eq!(main.window_id, "main");
        assert_eq!(main.active_tab_id.as_deref(), Some(tab.id.as_str()));
        assert_eq!(main.tabs, vec![tab.clone()]);
        store.bind_window(Some("win-2"));
        let other = snapshot(&store);
        assert!(other.tabs.is_empty());
        assert_eq!(other.active_tab_id, None);
    }

    #[test]
    fn re_pointing_a_tab_keeps_its_id_and_a_session_only_when_the_new_folder_owns_it() {
        const SID: &str = "685dd464-eabf-4e4c-9f6f-ac02c63aad27";
        let dir = TempDir::new("tabs_repoint");
        let old = dir.join("old");
        let new = dir.join("new");
        std::fs::create_dir_all(&new).unwrap();
        let other = dir.join("other");
        std::fs::create_dir_all(&other).unwrap();
        let config = dir.join("claude");
        let real_new = new.canonicalize().unwrap();
        let project = config
            .join("projects")
            .join(crate::claude_session_log::project_dir_name(
                &real_new.display().to_string(),
            ));
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join(format!("{SID}.jsonl")), "{}\n").unwrap();

        let mut store = StateStore::open_path(dir.join("state.json")).unwrap();
        let mut make = |cwd: &std::path::Path, rename: bool| {
            let id = store
                .create_tab(&cwd.display().to_string(), TabKind::Claude)
                .unwrap();
            store
                .remember_claude_session(&id, SID, &config.display().to_string())
                .unwrap();
            if rename {
                store.set_tab_label(&id, "My name").unwrap();
            }
            id
        };
        let followed = make(&old, false);
        let renamed = make(&old, true);
        let tab = store.tab_by_id(&followed).unwrap().clone();

        // The new folder owns the session: kept. A folder that does not: forgotten.
        assert!(keeps_session(&tab, &new));
        assert!(!keeps_session(&tab, &other));
        let no_session = TabRecord {
            claude_session_id: None,
            ..tab.clone()
        };
        assert!(!keeps_session(&no_session, &new));

        store
            .set_tab_folder(&followed, &new.display().to_string(), true)
            .unwrap();
        let moved = store.tab_by_id(&followed).unwrap();
        assert_eq!(moved.id, followed);
        assert_eq!(moved.cwd, new.display().to_string());
        assert_eq!(moved.label, "Claude · new", "the label follows the folder");
        assert_eq!(moved.claude_session_id.as_deref(), Some(SID));

        store
            .set_tab_folder(&renamed, &other.display().to_string(), false)
            .unwrap();
        let kept_name = store.tab_by_id(&renamed).unwrap();
        assert_eq!(kept_name.label, "My name", "a custom label stays");
        assert_eq!(kept_name.claude_session_id, None);
        assert_eq!(kept_name.claude_config_dir, None);
        assert!(store.set_tab_folder("nope", "/x", true).is_err());
    }

    #[test]
    fn recent_folders_are_newest_first_deduplicated_existing_and_capped() {
        let t = |m: u32| format!("2026-10-27T10:{m:02}:00Z");
        let exists = |folder: &str| {
            // "/gone/..." does not exist; "/link" is another name for "/real".
            if folder.starts_with("/gone") {
                None
            } else if folder == "/link" {
                Some("/real".to_string())
            } else {
                Some(folder.to_string())
            }
        };
        let list = recent_from(
            vec![
                ("/a".into(), t(1)),
                ("/real".into(), t(2)),
                ("/gone/x".into(), t(9)),
                ("/b".into(), t(3)),
                ("/link".into(), t(4)),
                ("/a".into(), t(5)),
                ("  ".into(), t(6)),
                ("/unknown-time".into(), "garbage".into()),
            ],
            exists,
        );
        // /a (10:05), /link = /real (10:04), /b (10:03), then the one with no time.
        assert_eq!(list, ["/a", "/real", "/b", "/unknown-time"]);
        let many: Vec<(String, String)> = (0..40).map(|n| (format!("/f{n}"), t(n % 60))).collect();
        assert_eq!(recent_from(many, exists).len(), RECENT_FOLDERS);
        assert!(recent_from(Vec::new(), exists).is_empty());
    }

    #[test]
    fn a_tab_whose_folder_went_away_says_so_and_closed_tabs_stay_in_their_window() {
        let dir = TempDir::new("tabs_snapshot");
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let mut store = StateStore::open_path(dir.join("state.json")).unwrap();
        let kept = add_tab(&mut store, &work.display().to_string(), TabKind::Shell).unwrap();
        let gone = add_tab(&mut store, &work.display().to_string(), TabKind::Claude).unwrap();
        store.close_tab(&gone.id).unwrap();
        std::fs::remove_dir_all(&work).unwrap();
        let main = snapshot(&store);
        assert_eq!(main.tabs[0].id, kept.id);
        assert_eq!(main.tabs[0].folder_status, "missing");
        assert_eq!(main.closed_tabs.len(), 1);
        assert_eq!(main.closed_tabs[0].id, gone.id);
        store.bind_window(Some("win-2"));
        assert!(snapshot(&store).closed_tabs.is_empty());
    }
}
