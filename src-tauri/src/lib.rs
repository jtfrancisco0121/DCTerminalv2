mod claude_session_log;
mod cli_detect;
mod clickup;
mod commands;
mod crash_log;
mod data_dir;
mod import_dev;
#[cfg(target_os = "macos")]
mod macos_terminate;
pub(crate) mod observe;
mod paths;
mod process_tree;
mod provider;
mod pty;
pub(crate) mod run;
mod shell_env;
pub mod status_line;
mod store;
#[cfg(test)]
pub(crate) mod test_support;
mod usage;
mod windows;

use commands::{
    claude_account_logins, claude_status, close_tab, get_app_state, get_claude_usage, get_settings,
    new_tab, rename_tab, reopen_closed_tab, select_tab, set_claude_settings, set_run_settings,
    set_terminal_settings, set_ui_settings,
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

/// `<app data>/app.lock`, flock'd for the app's whole life.
static APP_LOCK: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Before anything else, on the main thread with no other thread yet.
    for (_, step) in STARTUP_ENV_STEPS {
        step();
    }
    let mut builder = tauri::Builder::default();
    if data_dir::single_instance_here() {
        // First: a second copy hands over to this one and exits before it
        // reads or writes state.json or settings.json.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let _ = windows::restore_a_window(app);
        }));
    }
    builder
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
            // Held until exit: another build can tell this one is running.
            if let Some(lock) = import_dev::hold_app_lock(&data_dir) {
                let _ = APP_LOCK.set(lock);
            }
            let state_store = StateStore::load_or_default(app.handle())?;
            let settings_store = SettingsStore::open(&data_dir)?;
            app.manage(Mutex::new(state_store));
            app.manage(Mutex::new(settings_store));
            app.manage(Mutex::new(usage::UsageStore::open(&data_dir)));
            app.manage(Mutex::new(PtyRegistry::new()));
            app.manage(observe::ObserveState::start());
            for note in store::take_recovery_notes() {
                eprintln!("DCTerminalv2: {note}");
            }
            usage::remove_gone_status_lines(app.handle());
            windows::install_macos_menu(app)?;
            windows::restore_saved_windows(app.handle());
            // The env-changing steps at the start of `run()` are done, so a
            // thread may ask the login shell again (it only rewrites the
            // PATH cache, for the next launch).
            shell_env::spawn_login_path_refresh();
            run::spawn_prune_launch_prompts(app.handle());
            clickup::commands::spawn_auto_post_sweep(app.handle());
            run::afk::spawn_afk_sweeps(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            windows::window_context,
            windows::set_window_account,
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
            set_run_settings,
            run::run_overview,
            run::detect_folder_repos,
            run::set_repo_setup,
            run::start_run,
            run::resume_run,
            run::read_run,
            run::rerun_checks,
            run::run_worktrees,
            run::remove_run_worktrees,
            run::read_run_file,
            run::memory_status,
            run::read_memory_file,
            clickup::commands::clickup_token_status,
            clickup::commands::clickup_set_token,
            clickup::commands::clickup_clear_token,
            clickup::commands::clickup_test_token,
            clickup::commands::clickup_fetch,
            clickup::commands::clickup_run_state,
            clickup::commands::clickup_check_tickets,
            clickup::commands::clickup_pull_tickets,
            clickup::commands::clickup_post_writeback,
            clickup::commands::clickup_auto_post_retry,
            clickup::commands::clickup_auto_post_consent,
            clickup::commands::clickup_auto_post_now,
            clickup::commands::clickup_set_auto_post,
            import_dev::dev_import_status,
            import_dev::dev_import_dismiss,
            import_dev::import_from_dev_app,
            observe::observe_tab,
            observe::observe_tabs,
            observe::nudge_tab,
            observe::claude_version_status,
            observe::run_claude_selftest,
            observe::read_check_log,
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
