//! Tauri commands that are not the terminal itself (`pty`) or the windows
//! (`windows`): tabs, settings and accounts, and Claude usage.

mod settings;
mod tabs;
mod usage;

pub use settings::{
    claude_account_logins, claude_status, get_settings, set_claude_settings, set_notify_settings,
    set_observe_settings, set_run_settings, set_terminal_settings, set_ui_settings,
};
#[cfg(test)]
pub(crate) use tabs::close_tab_then_kill;
pub use tabs::{close_tab, get_app_state, new_tab, rename_tab, reopen_closed_tab, select_tab};
pub(crate) use tabs::{summary as tab_summary, TabSummary};
pub use usage::get_claude_usage;
