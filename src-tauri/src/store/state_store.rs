use crate::store::json_io::{read_json_or_recover, set_aside_other_version, write_json_atomic};
use crate::store::state_types::{
    window_matches, AppStateFile, ArchivedWindow, TabKind, TabRecord, WindowRecord,
    DEFAULT_ACCOUNT_ID, MAIN_WINDOW_ID, STATE_SCHEMA_VERSION,
};
use chrono::Utc;
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::AppHandle;

/// Closed tabs kept per open window for Reopen Closed Tab.
const CLOSED_TABS_PER_WINDOW: usize = 15;
/// Closed windows kept for Reopen Closed Window; older ones are dropped
/// with their tabs.
const ARCHIVED_WINDOWS_KEPT: usize = 5;
/// Longest tab name kept.
const MAX_LABEL_CHARS: usize = 80;

pub struct StateStore {
    pub path: PathBuf,
    pub data: AppStateFile,
    /// Window the command in progress is acting for. Not saved. Set at the
    /// start of each command so two windows do not share a sticky focus.
    pub draft_window: String,
}

impl StateStore {
    pub fn load_or_default(app: &AppHandle) -> Result<Self, String> {
        let dir = crate::data_dir::app_data_dir(app).path;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Self::open_path(dir.join("state.json"))
    }

    /// Load `state.json` at `path` (missing file = empty state).
    pub fn open_path(path: PathBuf) -> Result<Self, String> {
        // A damaged (or missing, after an interrupted save) file falls back to
        // state.json.bak instead of blocking startup or starting empty.
        let mut data: AppStateFile = read_json_or_recover(&path)?;
        if data.schema_version != STATE_SCHEMA_VERSION {
            // Another build's file is kept aside, never read or overwritten.
            set_aside_other_version(&path, data.schema_version);
            data = AppStateFile::default();
        }
        Ok(Self {
            path,
            data,
            draft_window: MAIN_WINDOW_ID.to_string(),
        })
    }

    pub fn save(&mut self) -> Result<(), String> {
        write_json_atomic(&self.path, &self.data)
    }

    /// The window a command should read and write. Unknown ids stay on main.
    pub fn bind_window(&mut self, window_id: Option<&str>) {
        let id = window_id.unwrap_or("").trim();
        if id.is_empty() {
            self.draft_window = MAIN_WINDOW_ID.to_string();
            return;
        }
        let safe = id.chars().take(64).collect::<String>();
        if safe
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            self.draft_window = safe;
        }
    }

    pub fn focus_window(&self) -> &str {
        &self.draft_window
    }

    pub fn account_for_window(&self, window_id: &str) -> String {
        self.data
            .windows
            .iter()
            .find(|window| window_matches(&window.id, window_id))
            .map(|window| window.account_id.clone())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| DEFAULT_ACCOUNT_ID.to_string())
    }

    /// Account for a tab's window, or the command's window when the tab is unknown.
    pub fn account_for_tab(&self, tab_id: &str) -> String {
        let window = self
            .tab_by_id(tab_id)
            .map(|tab| tab.window_id.as_str())
            .filter(|id| !id.is_empty())
            .unwrap_or(self.draft_window.as_str());
        self.account_for_window(window)
    }

    pub fn active_for_window(&self, window_id: &str) -> Option<String> {
        self.data
            .windows
            .iter()
            .find(|window| window_matches(&window.id, window_id))
            .and_then(|window| window.active_tab_id.clone())
            .filter(|id| self.tab_by_id(id).is_some())
    }

    fn remember_active(&mut self, window_id: &str, tab_id: Option<String>) {
        if let Some(window) = self
            .data
            .windows
            .iter_mut()
            .find(|window| window_matches(&window.id, window_id))
        {
            window.active_tab_id = tab_id;
        }
    }

    /// Record `window_id` on `account_id` (a new window, or a window moved to
    /// another account).
    pub fn ensure_window(&mut self, window_id: &str, account_id: &str) -> Result<(), String> {
        // A live window owns its label; its closed tabs show in its own list.
        self.data
            .archived_windows
            .retain(|archived| archived.window.id != window_id);
        if let Some(window) = self
            .data
            .windows
            .iter_mut()
            .find(|window| window.id == window_id)
        {
            window.account_id = account_id.to_string();
            return self.save();
        }
        self.data.windows.push(WindowRecord {
            id: window_id.to_string(),
            account_id: account_id.to_string(),
            active_tab_id: None,
        });
        self.save()
    }

    /// Forget a window record that never became a window (its build
    /// failed), so no later launch restores a window nobody opened. A
    /// window with tabs is kept.
    pub fn forget_empty_window(&mut self, window_id: &str) -> Result<(), String> {
        if !self.tabs_in_window(window_id).is_empty() {
            return Ok(());
        }
        let before = self.data.windows.len();
        self.data.windows.retain(|window| window.id != window_id);
        if self.data.windows.len() == before {
            return Ok(());
        }
        self.save()
    }

    /// Drop a window that was closed while another stayed open. Its tabs
    /// all move to the closed list (one save, none trimmed), and the window
    /// is kept with its account for Reopen Closed Window.
    pub fn archive_window(&mut self, window_id: &str) -> Result<(), String> {
        let tab_ids: Vec<String> = self
            .tabs_in_window(window_id)
            .into_iter()
            .map(|tab| tab.id.clone())
            .collect();
        let record = self
            .data
            .windows
            .iter()
            .find(|window| window.id == window_id)
            .cloned();
        for id in &tab_ids {
            if let Some(idx) = self.data.tabs.iter().position(|tab| &tab.id == id) {
                let closed = self.close_tab_record(idx);
                self.push_closed(closed);
            }
        }
        self.data.windows.retain(|window| window.id != window_id);
        // The next launch opens a new main, which would share the label: its
        // closed tabs would show in the new main's list and reopening it
        // would land on that window. Main is kept under a fresh label instead.
        let label = if window_matches(window_id, MAIN_WINDOW_ID) {
            self.next_window_label()
        } else {
            window_id.to_string()
        };
        let mut has_closed = false;
        for tab in &mut self.data.closed_tabs {
            if window_matches(&tab.window_id, window_id) {
                tab.window_id = label.clone();
                has_closed = true;
            }
        }
        if has_closed {
            let mut window = record.unwrap_or_else(|| WindowRecord {
                id: window_id.to_string(),
                account_id: DEFAULT_ACCOUNT_ID.to_string(),
                active_tab_id: None,
            });
            window.id = label.clone();
            self.data
                .archived_windows
                .retain(|archived| archived.window.id != label);
            self.data.archived_windows.insert(
                0,
                ArchivedWindow {
                    window,
                    tab_ids,
                    closed_at: Utc::now().to_rfc3339(),
                },
            );
            if self.data.archived_windows.len() > ARCHIVED_WINDOWS_KEPT {
                let dropped = self.data.archived_windows.split_off(ARCHIVED_WINDOWS_KEPT);
                self.data.closed_tabs.retain(|tab| {
                    !dropped
                        .iter()
                        .any(|archived| window_matches(&tab.window_id, &archived.window.id))
                });
            }
        }
        self.trim_closed_tabs();
        self.save()
    }

    /// Keep each open window's newest closed tabs. Tabs of archived windows
    /// are all kept until their window is reopened or dropped.
    pub fn trim_closed_tabs(&mut self) {
        let archived: Vec<String> = self
            .data
            .archived_windows
            .iter()
            .map(|archived| archived.window.id.clone())
            .collect();
        let mut kept: HashMap<String, usize> = HashMap::new();
        self.data.closed_tabs.retain(|tab| {
            if archived.iter().any(|id| window_matches(&tab.window_id, id)) {
                return true;
            }
            let window = if tab.window_id.trim().is_empty() {
                MAIN_WINDOW_ID
            } else {
                tab.window_id.as_str()
            };
            let count = kept.entry(window.to_string()).or_insert(0);
            *count += 1;
            *count <= CLOSED_TABS_PER_WINDOW
        });
    }

    /// A free `win-N` label for a new window. Labels of archived windows
    /// and of closed tabs are taken too, so a new window never inherits them.
    pub fn next_window_label(&self) -> String {
        let taken = |label: &str| {
            self.data.windows.iter().any(|window| window.id == label)
                || self
                    .data
                    .archived_windows
                    .iter()
                    .any(|archived| archived.window.id == label)
                || self
                    .data
                    .closed_tabs
                    .iter()
                    .any(|tab| tab.window_id == label)
        };
        let mut n = 2u32;
        loop {
            let label = format!("win-{n}");
            if !taken(&label) {
                return label;
            }
            n += 1;
            if n > 10_000 {
                return format!("win-{}", Utc::now().timestamp_millis());
            }
        }
    }

    pub fn has_archived_windows(&self) -> bool {
        !self.data.archived_windows.is_empty()
    }

    /// Reopen the most recently archived window: its record comes back with
    /// its account, and the tabs it had open are reopened in their order.
    /// `None` when there is none.
    pub fn reopen_archived_window(&mut self) -> Result<Option<WindowRecord>, String> {
        if self.data.archived_windows.is_empty() {
            return Ok(None);
        }
        // A failure partway puts everything back as it was.
        let before = self.data.clone();
        let archived = self.data.archived_windows.remove(0);
        let window = archived.window;
        let label = window.id.clone();
        self.data.windows.retain(|open| open.id != label);
        self.data.windows.push(window.clone());
        let caller = std::mem::replace(&mut self.draft_window, label.clone());
        let mut reopened = Ok(());
        for id in &archived.tab_ids {
            if !self.data.closed_tabs.iter().any(|tab| &tab.id == id) {
                continue;
            }
            if let Err(err) = self.reopen_closed_id(Some(id)) {
                reopened = Err(err);
                break;
            }
        }
        if reopened.is_ok() {
            let active = window
                .active_tab_id
                .clone()
                .filter(|id| self.tab_by_id(id).is_some());
            if active.is_some() {
                self.remember_active(&label, active);
            }
        }
        self.draft_window = caller;
        if let Err(err) = reopened {
            self.data = before;
            self.save()?;
            return Err(err);
        }
        self.save()?;
        Ok(Some(window))
    }

    /// Every tab that is open or can still be reopened (closed tabs,
    /// closed windows' tabs). Any other tab id is gone for good.
    pub fn known_tab_ids(&self) -> std::collections::HashSet<String> {
        self.data
            .tabs
            .iter()
            .chain(self.data.closed_tabs.iter())
            .map(|tab| tab.id.clone())
            .collect()
    }

    pub fn tabs_in_window(&self, window_id: &str) -> Vec<&TabRecord> {
        self.sorted_tabs()
            .into_iter()
            .filter(|tab| window_matches(&tab.window_id, window_id))
            .collect()
    }

    pub fn sorted_tabs(&self) -> Vec<&TabRecord> {
        let mut tabs: Vec<&TabRecord> = self.data.tabs.iter().collect();
        tabs.sort_by_key(|t| t.order);
        tabs
    }

    pub fn tab_by_id(&self, tab_id: &str) -> Option<&TabRecord> {
        self.data.tabs.iter().find(|t| t.id == tab_id)
    }

    fn tab_mut(&mut self, tab_id: &str) -> Result<&mut TabRecord, String> {
        self.data
            .tabs
            .iter_mut()
            .find(|t| t.id == tab_id)
            .ok_or_else(|| format!("unknown tab: {tab_id}"))
    }

    /// A new tab in the command's window, made active. Nothing is started:
    /// its terminal opens when a view shows it.
    pub fn create_tab(&mut self, cwd: &str, kind: TabKind) -> Result<String, String> {
        let tab_id = new_tab_id();
        let window_id = self.draft_window.clone();
        self.data.tabs.push(TabRecord {
            id: tab_id.clone(),
            label: tab_label(kind, cwd),
            custom_label: false,
            cwd: cwd.to_string(),
            kind,
            window_id: window_id.clone(),
            order: next_tab_order(&self.data),
            created_at: Utc::now().to_rfc3339(),
            claude_session_id: None,
            claude_config_dir: None,
            closed_at: None,
            launch: None,
        });
        self.remember_active(&window_id, Some(tab_id.clone()));
        self.save()?;
        Ok(tab_id)
    }

    /// A new Claude tab that starts the dcv2 plugin (`launch`), labelled
    /// `label`, made active.
    pub fn create_launch_tab(
        &mut self,
        cwd: &str,
        label: &str,
        launch: super::state_types::TabLaunch,
    ) -> Result<String, String> {
        let tab_id = self.create_tab(cwd, TabKind::Claude)?;
        let tab = self.tab_mut(&tab_id)?;
        tab.label = label.chars().take(MAX_LABEL_CHARS).collect();
        tab.custom_label = true;
        tab.launch = Some(launch);
        self.save()?;
        Ok(tab_id)
    }

    pub fn set_active_tab(&mut self, tab_id: &str) -> Result<(), String> {
        let window_id = self
            .tab_by_id(tab_id)
            .map(|tab| tab.window_id.clone())
            .ok_or_else(|| format!("unknown tab: {tab_id}"))?;
        self.remember_active(&window_id, Some(tab_id.to_string()));
        self.save()
    }

    pub fn set_tab_label(&mut self, tab_id: &str, label: &str) -> Result<(), String> {
        let label = label.trim();
        if label.is_empty() {
            return Err("tab name is empty".to_string());
        }
        let label: String = label.chars().take(MAX_LABEL_CHARS).collect();
        let tab = self.tab_mut(tab_id)?;
        tab.label = label;
        tab.custom_label = true;
        self.save()
    }

    /// The Claude session a terminal tab writes now, and the config folder
    /// it writes it under, so it survives a restart. Saves only on a change.
    pub fn remember_claude_session(
        &mut self,
        tab_id: &str,
        session_id: &str,
        config_dir: &str,
    ) -> Result<(), String> {
        let tab = self.tab_mut(tab_id)?;
        let session = Some(session_id.to_string());
        let config = Some(config_dir.to_string());
        if tab.claude_session_id == session && tab.claude_config_dir == config {
            return Ok(());
        }
        tab.claude_session_id = session;
        tab.claude_config_dir = config;
        self.save()
    }

    pub fn close_tab(&mut self, tab_id: &str) -> Result<(), String> {
        let idx = self
            .data
            .tabs
            .iter()
            .position(|t| t.id == tab_id)
            .ok_or_else(|| format!("unknown tab: {tab_id}"))?;
        let closed = self.close_tab_record(idx);
        self.push_closed(closed);
        self.trim_closed_tabs();
        self.save()
    }

    fn push_closed(&mut self, closed: TabRecord) {
        self.data.closed_tabs.retain(|tab| tab.id != closed.id);
        self.data.closed_tabs.insert(0, closed);
    }

    /// Remove the tab at `idx` from the open tabs and return its closed
    /// record. The window's active tab moves to its newest other tab. No
    /// save and no trim: callers do both.
    fn close_tab_record(&mut self, idx: usize) -> TabRecord {
        let mut tab = self.data.tabs.remove(idx);
        tab.closed_at = Some(Utc::now().to_rfc3339());
        let window_id = tab.window_id.clone();
        let current = self
            .data
            .windows
            .iter()
            .find(|window| window_matches(&window.id, &window_id))
            .and_then(|window| window.active_tab_id.clone());
        if current.as_deref() == Some(tab.id.as_str()) {
            let next = self
                .data
                .tabs
                .iter()
                .filter(|candidate| window_matches(&candidate.window_id, &window_id))
                .max_by_key(|candidate| candidate.order)
                .map(|candidate| candidate.id.clone());
            self.remember_active(&window_id, next);
        }
        tab
    }

    /// Put a closed tab back in the command's window: `tab_id`, or the most
    /// recent one when `None`. Its terminal is not started.
    pub fn reopen_closed_id(&mut self, tab_id: Option<&str>) -> Result<TabRecord, String> {
        let focus = self.draft_window.clone();
        let index = match tab_id {
            Some(id) => {
                let index = self
                    .data
                    .closed_tabs
                    .iter()
                    .position(|t| t.id == id)
                    .ok_or_else(|| "that closed tab is no longer in the list".to_string())?;
                if !window_matches(&self.data.closed_tabs[index].window_id, &focus) {
                    return Err("that closed tab is in another window".to_string());
                }
                index
            }
            None => self
                .data
                .closed_tabs
                .iter()
                .position(|tab| window_matches(&tab.window_id, &focus))
                .ok_or_else(|| "no closed tab to reopen".to_string())?,
        };
        let mut record = self.data.closed_tabs.remove(index);
        if self.tab_by_id(&record.id).is_some() {
            return Err("that tab is already open".to_string());
        }
        record.closed_at = None;
        record.order = next_tab_order(&self.data);
        if record.window_id.trim().is_empty() {
            record.window_id = focus;
        }
        let id = record.id.clone();
        let window_id = record.window_id.clone();
        self.data.tabs.push(record);
        self.remember_active(&window_id, Some(id.clone()));
        self.save()?;
        self.tab_by_id(&id)
            .cloned()
            .ok_or_else(|| "reopened tab missing".to_string())
    }
}

/// `Claude · app`: what runs, then the folder's name.
pub fn tab_label(kind: TabKind, cwd: &str) -> String {
    format!("{} · {}", kind.label(), folder_display_name(cwd))
}

/// Millisecond ids, bumped so two tabs made in the same millisecond still get
/// distinct ids.
fn new_tab_id() -> String {
    use std::sync::atomic::{AtomicI64, Ordering};
    static LAST: AtomicI64 = AtomicI64::new(0);
    let now = Utc::now().timestamp_millis();
    let mut prev = LAST.load(Ordering::Relaxed);
    loop {
        let next = now.max(prev + 1);
        match LAST.compare_exchange_weak(prev, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return format!("tab_{next:x}"),
            Err(actual) => prev = actual,
        }
    }
}

fn next_tab_order(data: &AppStateFile) -> u32 {
    data.tabs.iter().map(|t| t.order).max().unwrap_or(0) + 1
}

/// Last path segment, treating both `/` and `\` as separators so Windows
/// paths still label tabs when the host (or a test) is not Windows.
fn folder_display_name(cwd: &str) -> &str {
    let trimmed = cwd.trim().trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn store_in(dir: &TempDir) -> StateStore {
        StateStore::open_path(dir.join("state.json")).unwrap()
    }

    #[test]
    fn tab_labels_name_the_kind_and_the_folder() {
        assert_eq!(tab_label(TabKind::Claude, "/w/app/"), "Claude · app");
        assert_eq!(
            tab_label(TabKind::Shell, r"C:\Projects\DCTerminal"),
            "Shell · DCTerminal"
        );
    }

    #[test]
    fn tab_ids_made_back_to_back_are_unique() {
        let ids: std::collections::HashSet<String> = (0..1000).map(|_| new_tab_id()).collect();
        assert_eq!(ids.len(), 1000);
    }

    #[test]
    fn tabs_and_their_claude_session_survive_a_reload() {
        let dir = TempDir::new("state_reload");
        let mut store = store_in(&dir);
        let id = store.create_tab("/w/app", TabKind::Claude).unwrap();
        store
            .remember_claude_session(&id, "0b6c1d2e-aaaa-4bbb-8ccc-1234567890ab", "/cfg/a")
            .unwrap();
        let shell = store.create_tab("/w/api", TabKind::Shell).unwrap();
        let again = store_in(&dir);
        let tabs = again.tabs_in_window(MAIN_WINDOW_ID);
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0].id, id);
        assert_eq!(
            tabs[0].claude_session_id.as_deref(),
            Some("0b6c1d2e-aaaa-4bbb-8ccc-1234567890ab")
        );
        assert_eq!(tabs[0].claude_config_dir.as_deref(), Some("/cfg/a"));
        assert_eq!(tabs[1].kind, TabKind::Shell);
        assert_eq!(
            again.active_for_window(MAIN_WINDOW_ID).as_deref(),
            Some(shell.as_str())
        );
    }

    #[test]
    fn renamed_tab_keeps_its_name_through_close_reopen_and_reload() {
        let dir = TempDir::new("state_rename");
        let mut store = store_in(&dir);
        let id = store.create_tab("/tmp/app", TabKind::Claude).unwrap();
        store.set_tab_label(&id, "  Auth bug  ").unwrap();
        store.close_tab(&id).unwrap();
        let reopened = store.reopen_closed_id(None).unwrap();
        assert_eq!(reopened.label, "Auth bug");
        assert!(reopened.custom_label);
        assert!(reopened.closed_at.is_none());
        assert_eq!(store_in(&dir).data.tabs[0].label, "Auth bug");
        assert!(store.set_tab_label(&id, "   ").is_err());
        store.set_tab_label(&id, &"x".repeat(200)).unwrap();
        assert_eq!(
            store.tab_by_id(&id).unwrap().label.chars().count(),
            MAX_LABEL_CHARS
        );
    }

    #[test]
    fn closing_the_active_tab_activates_the_newest_other_one() {
        let dir = TempDir::new("state_close_active");
        let mut store = store_in(&dir);
        let a = store.create_tab("/w/a", TabKind::Claude).unwrap();
        let b = store.create_tab("/w/b", TabKind::Claude).unwrap();
        let c = store.create_tab("/w/c", TabKind::Claude).unwrap();
        store.set_active_tab(&b).unwrap();
        store.close_tab(&b).unwrap();
        assert_eq!(
            store.active_for_window(MAIN_WINDOW_ID).as_deref(),
            Some(c.as_str())
        );
        store.close_tab(&a).unwrap();
        assert_eq!(
            store.active_for_window(MAIN_WINDOW_ID).as_deref(),
            Some(c.as_str())
        );
        store.close_tab(&c).unwrap();
        assert_eq!(store.active_for_window(MAIN_WINDOW_ID), None);
        assert_eq!(store.data.closed_tabs.len(), 3);
        assert_eq!(store.data.closed_tabs[0].id, c, "newest first");
    }

    #[test]
    fn a_specific_closed_tab_can_be_reopened_but_only_in_its_window() {
        let dir = TempDir::new("state_reopen_id");
        let mut store = store_in(&dir);
        let older = store.create_tab("/w/a", TabKind::Claude).unwrap();
        let newer = store.create_tab("/w/b", TabKind::Shell).unwrap();
        store.close_tab(&older).unwrap();
        store.close_tab(&newer).unwrap();
        store.bind_window(Some("win-2"));
        assert!(
            store.reopen_closed_id(Some(&older)).is_err(),
            "another window"
        );
        assert!(
            store.reopen_closed_id(None).is_err(),
            "nothing closed in win-2"
        );
        store.bind_window(None);
        let restored = store.reopen_closed_id(Some(&older)).unwrap();
        assert_eq!(restored.id, older);
        assert_eq!(store.data.closed_tabs.len(), 1);
        assert_eq!(store.data.closed_tabs[0].id, newer);
        assert!(store.reopen_closed_id(Some("tab_gone")).is_err());
    }

    #[test]
    fn closed_tabs_are_capped_per_window() {
        let dir = TempDir::new("state_closed_cap");
        let mut store = store_in(&dir);
        for n in 0..(CLOSED_TABS_PER_WINDOW + 3) {
            let id = store
                .create_tab(&format!("/w/{n}"), TabKind::Shell)
                .unwrap();
            store.close_tab(&id).unwrap();
        }
        assert_eq!(store.data.closed_tabs.len(), CLOSED_TABS_PER_WINDOW);
    }

    #[test]
    fn each_window_has_its_own_account_and_tabs() {
        let dir = TempDir::new("state_windows");
        let mut store = store_in(&dir);
        assert_eq!(store.account_for_window(MAIN_WINDOW_ID), DEFAULT_ACCOUNT_ID);
        let label = store.next_window_label();
        assert_eq!(label, "win-2");
        store.ensure_window(&label, "work").unwrap();
        assert_eq!(store.next_window_label(), "win-3");
        store.bind_window(Some(&label));
        let work_tab = store.create_tab("/w/koneksi", TabKind::Claude).unwrap();
        store.bind_window(None);
        let main_tab = store.create_tab("/w/home", TabKind::Claude).unwrap();
        assert_eq!(store.account_for_tab(&work_tab), "work");
        assert_eq!(store.account_for_tab(&main_tab), DEFAULT_ACCOUNT_ID);
        assert_eq!(store.tabs_in_window(&label).len(), 1);
        assert_eq!(store.tabs_in_window(MAIN_WINDOW_ID).len(), 1);
        assert_eq!(
            store.active_for_window(&label).as_deref(),
            Some(work_tab.as_str())
        );
        // A window id that could leave the data folder is not taken.
        store.bind_window(Some("../evil"));
        assert_eq!(store.focus_window(), MAIN_WINDOW_ID);
        let again = store_in(&dir);
        assert_eq!(again.account_for_window(&label), "work");
    }

    #[test]
    fn a_closed_window_comes_back_with_its_account_tabs_and_active_tab() {
        let dir = TempDir::new("state_archive");
        let mut store = store_in(&dir);
        store.ensure_window("win-2", "work").unwrap();
        store.bind_window(Some("win-2"));
        let a = store.create_tab("/w/a", TabKind::Claude).unwrap();
        let b = store.create_tab("/w/b", TabKind::Shell).unwrap();
        store.set_active_tab(&a).unwrap();
        store.bind_window(None);
        store.archive_window("win-2").unwrap();
        assert!(store.has_archived_windows());
        assert!(store.tabs_in_window("win-2").is_empty());
        assert!(!store.data.windows.iter().any(|w| w.id == "win-2"));
        assert_eq!(
            store.next_window_label(),
            "win-3",
            "an archived label stays taken"
        );

        let window = store.reopen_archived_window().unwrap().unwrap();
        assert_eq!(window.id, "win-2");
        assert_eq!(window.account_id, "work");
        let ids: Vec<_> = store
            .tabs_in_window("win-2")
            .iter()
            .map(|t| t.id.clone())
            .collect();
        assert_eq!(ids, vec![a.clone(), b]);
        assert_eq!(
            store.active_for_window("win-2").as_deref(),
            Some(a.as_str())
        );
        assert_eq!(
            store.focus_window(),
            MAIN_WINDOW_ID,
            "the caller's window is kept"
        );
        assert!(!store.has_archived_windows());
        assert_eq!(store.reopen_archived_window().unwrap(), None);
    }

    #[test]
    fn an_archived_main_is_kept_under_a_fresh_label() {
        let dir = TempDir::new("state_archive_main");
        let mut store = store_in(&dir);
        store.create_tab("/w/a", TabKind::Claude).unwrap();
        store.archive_window(MAIN_WINDOW_ID).unwrap();
        let archived = &store.data.archived_windows[0];
        assert_eq!(archived.window.id, "win-2");
        assert!(store
            .data
            .closed_tabs
            .iter()
            .all(|tab| tab.window_id == "win-2"));
    }

    #[test]
    fn a_window_that_was_never_built_is_forgotten_but_one_with_tabs_is_kept() {
        let dir = TempDir::new("state_forget_window");
        let mut store = store_in(&dir);
        store.ensure_window("win-2", "work").unwrap();
        store.ensure_window("win-3", "work").unwrap();
        store.bind_window(Some("win-3"));
        store.create_tab("/w/a", TabKind::Shell).unwrap();
        store.forget_empty_window("win-2").unwrap();
        store.forget_empty_window("win-3").unwrap();
        let again = StateStore::open_path(dir.join("state.json")).unwrap();
        let ids: Vec<&str> = again.data.windows.iter().map(|w| w.id.as_str()).collect();
        assert!(
            !ids.contains(&"win-2"),
            "no ghost window on the next launch"
        );
        assert!(ids.contains(&"win-3"));
    }

    #[test]
    fn a_window_with_no_tabs_is_not_kept_for_reopen() {
        let dir = TempDir::new("state_archive_empty");
        let mut store = store_in(&dir);
        store.ensure_window("win-2", "work").unwrap();
        store.archive_window("win-2").unwrap();
        assert!(!store.has_archived_windows());
    }

    #[test]
    fn only_five_closed_windows_are_kept_with_their_tabs() {
        let dir = TempDir::new("state_archive_cap");
        let mut store = store_in(&dir);
        for n in 2..(2 + ARCHIVED_WINDOWS_KEPT + 2) {
            let label = format!("win-{n}");
            store.ensure_window(&label, "default").unwrap();
            store.bind_window(Some(&label));
            store.create_tab("/w/x", TabKind::Shell).unwrap();
            store.archive_window(&label).unwrap();
        }
        assert_eq!(store.data.archived_windows.len(), ARCHIVED_WINDOWS_KEPT);
        assert_eq!(store.data.closed_tabs.len(), ARCHIVED_WINDOWS_KEPT);
    }

    #[test]
    fn a_file_from_another_version_is_set_aside_not_read() {
        let dir = TempDir::new("state_other_version");
        let path = dir.join("state.json");
        std::fs::write(&path, r#"{"schemaVersion":99,"tabs":[]}"#).unwrap();
        let store = StateStore::open_path(path.clone()).unwrap();
        assert_eq!(store.data, AppStateFile::default());
        assert!(!path.exists(), "moved aside, so no save overwrites it");
        assert!(crate::test_support::names_in(dir.path())
            .iter()
            .any(|name| name.starts_with("state.json.from-v99")));
    }
}
