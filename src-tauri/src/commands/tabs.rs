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
    window_id: Option<String>,
    tab_id: Option<String>,
    store: State<Mutex<StateStore>>,
) -> Result<TabSummary, String> {
    let mut store = store.lock().map_err(|e| e.to_string())?;
    store.bind_window(window_id.as_deref());
    let tab = store.reopen_closed_id(tab_id.as_deref())?;
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
