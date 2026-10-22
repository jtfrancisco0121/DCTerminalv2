mod claude_session_log;
mod cli_detect;
mod commands;
mod crash_log;
mod data_dir;
#[cfg(target_os = "macos")]
mod macos_terminate;
mod paths;
mod process_tree;
mod provider;
mod pty;
mod shell_env;
pub mod status_line;
mod store;
#[cfg(test)]
pub(crate) mod test_support;
mod usage;
mod windows;

use commands::{
    claude_account_logins, claude_status, close_tab, get_app_state, get_claude_usage, get_settings,
    new_tab, rename_tab, reopen_closed_tab, select_tab, set_claude_settings, set_terminal_settings,
    set_ui_settings,
};
use pty::{
    pty_kill, pty_open, pty_resize, pty_set_scheme, pty_write, terminal_resume_id, PtyRegistry,
};
use std::sync::Mutex;
use store::{SettingsStore, StateStore};
use tauri::Manager;

/// The steps that change process env at the start of [`run`], in the order
/// they run. Each is only sound while the main thread is the only thread.
///
/// - The scrub drops a parent Claude Code session's variables, so Claude tabs
///   (and a `claude` typed in a shell tab) don't take themselves for nested
///   sessions and skip their session log. It runs before the login shell, so
///   that shell never sees a parent session's tokens either. The per-command
///   `CLAUDE_CONFIG_DIR` is applied later and is untouched.
/// - The login PATH replaces the inherited one. A cached login PATH is
///   refreshed from `setup`, never from a thread started here.
const STARTUP_ENV_STEPS: [(&str, fn()); 2] = [
    (
        "scrub_parent_claude_session",
        shell_env::scrub_parent_claude_session,
    ),
    ("apply_login_path", shell_env::apply_login_path),
];

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Before anything else, on the main thread with no other thread yet.
    for (_, step) in STARTUP_ENV_STEPS {
        step();
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let resolved = data_dir::app_data_dir(app.handle());
            // First, so a panic anywhere below lands in logs/crash.log.
            crash_log::install(&resolved.path);
            if let Some(warning) = &resolved.warning {
                eprintln!("DCTerminalv2: {warning}");
            }
            let data_dir = resolved.path;
            let state_store = StateStore::load_or_default(app.handle())?;
            let settings_store = SettingsStore::open(&data_dir)?;
            app.manage(Mutex::new(state_store));
            app.manage(Mutex::new(settings_store));
            app.manage(Mutex::new(usage::UsageStore::open(&data_dir)));
            app.manage(Mutex::new(PtyRegistry::new()));
            for note in store::take_recovery_notes() {
                eprintln!("DCTerminalv2: {note}");
            }
            windows::install_macos_menu(app)?;
            windows::restore_saved_windows(app.handle());
            // The env-changing steps at the start of `run()` are done, so a
            // thread may ask the login shell again (it only rewrites the
            // PATH cache, for the next launch).
            shell_env::spawn_login_path_refresh();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            windows::window_context,
            windows::open_account_window,
            windows::reopen_closed_window,
            get_app_state,
            new_tab,
            select_tab,
            rename_tab,
            close_tab,
            reopen_closed_tab,
            pty_open,
            pty_write,
            pty_resize,
            pty_kill,
            pty_set_scheme,
            terminal_resume_id,
            get_settings,
            set_terminal_settings,
            set_ui_settings,
            set_claude_settings,
            claude_account_logins,
            claude_status,
            get_claude_usage,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app_handle, event| {
            windows::handle_run_event(app_handle, event);
        });
}

#[cfg(test)]
mod startup_tests {
    use super::*;

    /// The scrub must run before `apply_login_path` can spawn the login
    /// shell, or that shell inherits a parent session's variables.
    #[test]
    fn the_parent_session_is_scrubbed_before_any_child_can_start() {
        let names: Vec<&str> = STARTUP_ENV_STEPS.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["scrub_parent_claude_session", "apply_login_path"]);
        // The names match the functions, so a reorder cannot hide behind them.
        let scrub: fn() = shell_env::scrub_parent_claude_session;
        assert!(std::ptr::fn_addr_eq(STARTUP_ENV_STEPS[0].1, scrub));
        let shell: fn() = shell_env::apply_login_path;
        assert!(std::ptr::fn_addr_eq(STARTUP_ENV_STEPS[1].1, shell));
    }
}
