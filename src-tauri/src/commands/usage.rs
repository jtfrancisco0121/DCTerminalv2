//! Read the Claude limit snapshot for a window's account, with the latest
//! status line readings of its terminal tabs folded in.

use crate::store::{SettingsStore, StateStore};
use crate::usage::{UsageSnapshot, UsageStore};
use std::sync::Mutex;
use tauri::State;

#[tauri::command]
pub fn get_claude_usage(
    window_id: Option<String>,
    settings: State<Mutex<SettingsStore>>,
    state: State<Mutex<StateStore>>,
    usage: State<Mutex<UsageStore>>,
) -> Result<UsageSnapshot, String> {
    // Lock order: StateStore before SettingsStore (see store/mod.rs).
    let dir = {
        let mut state = state.lock().map_err(|e| e.to_string())?;
        state.bind_window(window_id.as_deref());
        let account = state.account_for_window(state.focus_window());
        let settings = settings.lock().map_err(|e| e.to_string())?;
        crate::store::settings_store::resolved_account_config(settings.claude(), &account)
    };
    let files = usage
        .lock()
        .map_err(|e| e.to_string())?
        .status_line_files(&dir.path);
    // Read without the lock: each file is a tab's last status line reading.
    let readings: Vec<_> = files
        .iter()
        .flat_map(|file| crate::status_line::read_windows(file))
        .collect();
    let mut usage = usage.lock().map_err(|e| e.to_string())?;
    usage.fold(&dir.path, readings);
    Ok(usage.snapshot(&dir.path))
}
