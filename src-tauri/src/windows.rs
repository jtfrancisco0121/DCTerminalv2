//! One Claude account per window, on macOS and Windows.
//!
//! Closing the last window on macOS leaves the process running (a normal Mac
//! app). A Dock click with no windows restores that window. On Windows and
//! other platforms the last window quits the process. Either way the last
//! window's tabs and account stay in `state.json`, so relaunch or the Dock
//! brings them back. Closing a window while another is still open drops that
//! window from the restore list and keeps it, with all its tabs and its
//! account, for File > Reopen Closed Window. Quit (a programmatic exit,
//! including the macOS Quit item) does not drop windows that are still open.
//! A window with terminals still running in it asks before it closes
//! (`close_window_now`), as Quit and a tab's close do.
//!
//! The macOS menu's tab items (New Tab, Close Tab, Reopen Closed Tab,
//! Settings) go to the focused window as a [`MENU_EVENT`] with the item's
//! id; the page acts on them. Their shortcuts work while a terminal has the
//! keyboard, since AppKit handles menu key equivalents first.

use crate::process_tree::STOP_GRACE;
use crate::provider::claude_config::ConfigDirInfo;
use crate::store::settings_store::{account_display_name, account_exists, resolved_account_config};
use crate::store::{SettingsStore, StateStore, MAIN_WINDOW_ID};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const NEW_WINDOW_ACCELERATOR: &str = "CmdOrCtrl+Shift+N";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const NEW_WINDOW_EVENT: &str = "new-window";
/// A menu item for the page (`new-tab`, `close-tab`, `reopen-closed-tab`,
/// `settings`), sent to the focused window with the item's id.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const MENU_EVENT: &str = "menu";
/// The page's menu items: id, title, shortcut.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const PAGE_ITEMS: [(&str, &str, &str); 3] = [
    ("new-tab", "New Tab", "CmdOrCtrl+T"),
    ("close-tab", "Close Tab", "CmdOrCtrl+W"),
    (
        "reopen-closed-tab",
        "Reopen Closed Tab",
        "CmdOrCtrl+Shift+T",
    ),
];
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const SETTINGS_ITEM: &str = "settings";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const REOPEN_WINDOW_ITEM: &str = "reopen-closed-window";

/// File > Reopen Closed Window, kept to enable it only when a closed window
/// is there to reopen.
#[cfg(target_os = "macos")]
struct ReopenWindowItem(tauri::menu::MenuItem<tauri::Wry>);

/// Set while a real quit is in progress so closing windows does not archive them.
static QUITTING: AtomicBool = AtomicBool::new(false);
/// Set while the quit question is on screen, so a second quit does not stack another.
static QUIT_ASKING: AtomicBool = AtomicBool::new(false);
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const QUIT_ITEM: &str = "quit";
/// File > Close Window: closes the focused window, asking first like the
/// window's own close button when terminals are running in it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const CLOSE_WINDOW_ITEM: &str = "close-window";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const CLOSE_WINDOW_ACCELERATOR: &str = "CmdOrCtrl+Shift+W";
/// Windows whose close question is on screen, so a second close of the same
/// window does not stack another.
static CLOSE_ASKING: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// How many terminals a quit would stop, across every window. Each count
/// is taken under its own short lock, dropped before anyone is asked
/// anything. A missing registry counts 0.
pub(crate) fn live_terminals(terminals: Option<&Mutex<crate::pty::PtyRegistry>>) -> usize {
    terminals.map_or(0, |registry| match registry.lock() {
        Ok(guard) => guard.live_count(),
        // A panic elsewhere does not make running work disappear.
        Err(poisoned) => poisoned.into_inner().live_count(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuitDecision {
    /// Quit now: nothing would be lost, or the user already said Quit.
    Quit,
    /// Something is running: ask before stopping it.
    Ask,
    /// The question is already on screen; this request does nothing.
    Ignore,
}

/// Whether an explicit quit goes ahead, asks, or is dropped.
pub(crate) fn quit_decision(live: usize, confirmed: bool, dialog_open: bool) -> QuitDecision {
    if confirmed {
        QuitDecision::Quit
    } else if dialog_open {
        QuitDecision::Ignore
    } else if live == 0 {
        QuitDecision::Quit
    } else {
        QuitDecision::Ask
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitStep {
    /// The last window closed on macOS: the process stays up.
    StayUp,
    /// Exit, as today.
    Proceed,
    /// A programmatic exit nobody confirmed: decide with [`quit_decision`].
    AskFirst,
}

/// `ExitRequested`: `code` is `None` when the last window closed and
/// `Some` for a programmatic exit (`app.exit`).
pub(crate) fn exit_step(code: Option<i32>, os: &str, confirmed: bool) -> ExitStep {
    match code {
        None if !quit_when_last_window_closes(os) => ExitStep::StayUp,
        Some(_) if !confirmed => ExitStep::AskFirst,
        _ => ExitStep::Proceed,
    }
}

/// The quit and window-close question, in the tab-close confirm's words.
pub(crate) fn quit_message(terminals: usize) -> String {
    let s = if terminals == 1 { "" } else { "s" };
    format!("{terminals} running terminal{s} will be stopped.")
}

/// An explicit quit (DCTerminalv2 > Quit, Cmd+Q, or an `app.exit`). With
/// nothing running, or once the user said Quit, it marks the quit and
/// returns [`QuitDecision::Quit`]; the caller then exits. With work running
/// it asks once (the answer arrives on another thread, so the main thread
/// never waits); Quit then exits again, which goes straight through.
fn request_quit(app: &AppHandle) -> QuitDecision {
    quit_request(live_work(app), &QUITTING, &QUIT_ASKING, |live| {
        ask_to_quit(app, live)
    })
}

fn live_work(app: &AppHandle) -> usize {
    live_terminals(app.try_state::<Mutex<crate::pty::PtyRegistry>>().as_deref())
}

/// [`request_quit`] over its flags (`QUITTING`, `QUIT_ASKING`) and its
/// question, so it runs in tests without an app.
fn quit_request(
    live: usize,
    quitting: &AtomicBool,
    asking: &AtomicBool,
    ask: impl FnOnce(usize),
) -> QuitDecision {
    let decision = quit_decision(
        live,
        quitting.load(Ordering::SeqCst),
        asking.load(Ordering::SeqCst),
    );
    match decision {
        QuitDecision::Quit => quitting.store(true, Ordering::SeqCst),
        QuitDecision::Ask => {
            // Two quits racing here: only the first one asks.
            if asking.swap(true, Ordering::SeqCst) {
                return QuitDecision::Ignore;
            }
            ask(live);
        }
        QuitDecision::Ignore => {}
    }
    decision
}

/// macOS `applicationShouldTerminate:` (Dock icon Quit, logout, AppleScript
/// `quit`): true lets AppKit terminate now. Otherwise the Cmd+Q question is
/// (or already was) put on screen, AppKit is told to cancel, and the
/// question's Quit exits through `app.exit` like Cmd+Q does.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn terminate_now(
    live: usize,
    quitting: &AtomicBool,
    asking: &AtomicBool,
    ask: impl FnOnce(usize),
) -> bool {
    quit_request(live, quitting, asking, ask) == QuitDecision::Quit
}

/// [`terminate_now`] for the running app.
#[cfg(target_os = "macos")]
pub(crate) fn should_terminate(app: &AppHandle) -> bool {
    terminate_now(live_work(app), &QUITTING, &QUIT_ASKING, |live| {
        ask_to_quit(app, live)
    })
}

fn ask_to_quit(app: &AppHandle, live: usize) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let app_for_answer = app.clone();
    app.dialog()
        .message(quit_message(live))
        .title("Quit DCTerminalv2?")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Quit".to_string(),
            "Cancel".to_string(),
        ))
        .show(move |quit| {
            if quit {
                QUITTING.store(true, Ordering::SeqCst);
                app_for_answer.exit(0);
            }
            QUIT_ASKING.store(false, Ordering::SeqCst);
        });
}

pub fn window_title(account_name: &str) -> String {
    let name = account_name.trim();
    if name.is_empty() {
        "DCTerminalv2".to_string()
    } else {
        format!("DCTerminalv2 — {name}")
    }
}

/// `false` only on macOS: the process stays up with no windows.
pub fn quit_when_last_window_closes(os: &str) -> bool {
    !os.eq_ignore_ascii_case("macos")
}

pub fn host_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

/// Dock click (`applicationShouldHandleReopen`) with nothing visible.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn dock_reopen_creates_window(has_visible_windows: bool) -> bool {
    !has_visible_windows
}

/// The last open window is kept so Dock or the next launch can restore it.
pub fn should_keep_window_for_restore(other_windows: usize) -> bool {
    other_windows == 0
}

pub fn install_macos_menu(app: &tauri::App) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
        let new_window = MenuItem::with_id(
            app,
            "new-window",
            "New Window",
            true,
            Some(NEW_WINDOW_ACCELERATOR),
        )?;
        // No shortcut of its own; enabled while a closed window is kept.
        let reopen_window = MenuItem::with_id(
            app,
            REOPEN_WINDOW_ITEM,
            "Reopen Closed Window",
            has_archived_windows(app.handle()),
            None::<&str>,
        )?;
        let page_items = PAGE_ITEMS
            .iter()
            .map(|(id, title, shortcut)| MenuItem::with_id(app, *id, *title, true, Some(*shortcut)))
            .collect::<tauri::Result<Vec<_>>>()?;
        let separator = PredefinedMenuItem::separator(app)?;
        let mut file_items: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = Vec::new();
        file_items.extend(
            page_items
                .iter()
                .map(|item| item as &dyn tauri::menu::IsMenuItem<tauri::Wry>),
        );
        let close_window = MenuItem::with_id(
            app,
            CLOSE_WINDOW_ITEM,
            "Close Window",
            true,
            Some(CLOSE_WINDOW_ACCELERATOR),
        )?;
        file_items.push(&separator);
        file_items.push(&new_window);
        file_items.push(&reopen_window);
        file_items.push(&close_window);
        let file = Submenu::with_items(app, "File", true, &file_items)?;
        app.manage(ReopenWindowItem(reopen_window));
        let settings =
            MenuItem::with_id(app, SETTINGS_ITEM, "Settings…", true, Some("CmdOrCtrl+,"))?;
        // Not the predefined Quit: that sends `terminate:`, which tao turns
        // straight into `RunEvent::Exit` with no chance to ask first.
        let quit = MenuItem::with_id(
            app,
            QUIT_ITEM,
            "Quit DCTerminalv2",
            true,
            Some("CmdOrCtrl+Q"),
        )?;
        let app_separator = PredefinedMenuItem::separator(app)?;
        let app_menu = Submenu::with_items(
            app,
            "DCTerminalv2",
            true,
            &[&settings, &app_separator, &quit],
        )?;
        // The other `terminate:` senders (Dock icon, logout, AppleScript).
        crate::macos_terminate::install(app.handle());
        // Without these, Cmd+C/V/X/A/Z never reach text fields on macOS.
        let edit = Submenu::with_items(
            app,
            "Edit",
            true,
            &[
                &PredefinedMenuItem::undo(app, None)?,
                &PredefinedMenuItem::redo(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::cut(app, None)?,
                &PredefinedMenuItem::copy(app, None)?,
                &PredefinedMenuItem::paste(app, None)?,
                &PredefinedMenuItem::select_all(app, None)?,
            ],
        )?;
        let menu = Menu::with_items(app, &[&app_menu, &file, &edit])?;
        app.set_menu(menu)?;
        app.on_menu_event(|app, event| match event.id().as_ref() {
            "new-window" => request_new_window(app),
            CLOSE_WINDOW_ITEM => {
                if let Some(window) = focused_label(app).and_then(|l| app.get_webview_window(&l)) {
                    // A close request like the title bar's: it asks first.
                    let _ = window.close();
                }
            }
            id if id == SETTINGS_ITEM || PAGE_ITEMS.iter().any(|(item, _, _)| *item == id) => {
                send_to_focused(app, id);
            }
            QUIT_ITEM => {
                if request_quit(app) == QuitDecision::Quit {
                    app.exit(0);
                }
            }
            REOPEN_WINDOW_ITEM => {
                // Off the main thread: transcripts load from disk, and the
                // window build hands itself to the main thread and waits.
                let app = app.clone();
                std::thread::spawn(move || {
                    if let Err(err) = reopen_last_closed_window(&app) {
                        eprintln!("DCTerminalv2: reopen closed window failed: {err}");
                    }
                });
            }
            _ => {}
        });
    }
    let _ = app;
    Ok(())
}

/// Ask the focused window to open the account picker. With no window, restore one.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn focused_label(app: &AppHandle) -> Option<String> {
    app.webview_windows()
        .into_iter()
        .find_map(|(label, window)| {
            window
                .is_focused()
                .ok()
                .filter(|focused| *focused)
                .map(|_| label)
        })
}

/// A page menu item: to the focused window, or, with none, a restored one.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn send_to_focused(app: &AppHandle, id: &str) {
    match focused_label(app) {
        Some(label) => {
            let _ = app.emit_to(label, MENU_EVENT, id.to_string());
        }
        None => {
            let _ = restore_a_window(app);
        }
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn request_new_window(app: &AppHandle) {
    if let Some(label) = focused_label(app) {
        let _ = app.emit_to(label, NEW_WINDOW_EVENT, ());
        return;
    }
    let _ = restore_a_window(app);
}

pub fn handle_run_event(app: &AppHandle, event: tauri::RunEvent) {
    match event {
        tauri::RunEvent::Exit => shutdown_processes(app),
        tauri::RunEvent::ExitRequested { code, api, .. } => {
            match exit_step(code, host_os(), QUITTING.load(Ordering::SeqCst)) {
                ExitStep::StayUp => api.prevent_exit(),
                ExitStep::Proceed => QUITTING.store(true, Ordering::SeqCst),
                ExitStep::AskFirst => {
                    if request_quit(app) != QuitDecision::Quit {
                        api.prevent_exit();
                    }
                }
            }
        }
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::CloseRequested { api, .. },
            ..
        } => {
            if !close_window_now(app, &label) {
                api.prevent_close();
            }
        }
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::Destroyed,
            ..
        } => on_window_destroyed(app, &label),
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen {
            has_visible_windows,
            ..
        } if dock_reopen_creates_window(has_visible_windows) => {
            let _ = restore_a_window(app);
        }
        _ => {}
    }
}

/// App exit: SIGTERM every terminal at once, wait for all of them together
/// for at most [`STOP_GRACE`] (no registry lock held), then kill what is
/// left. Claude gets the same chance to finish its session log as on a tab
/// close. Windows has no SIGTERM, so nothing waits there. The login PATH
/// refresh's shell is killed first; it holds nothing worth waiting for.
fn shutdown_processes(app: &AppHandle) {
    crate::shell_env::stop_login_path_refresh();
    let terminals = app.try_state::<Mutex<crate::pty::PtyRegistry>>();
    let ptys = terminals
        .as_ref()
        .and_then(|state| state.lock().ok().map(|guard| guard.running_groups()))
        .unwrap_or_default();
    // Signalled with no registry lock held: each walks its descendants.
    ptys.iter()
        .for_each(crate::process_tree::PtyGroup::terminate);
    if cfg!(unix) {
        let deadline = Instant::now() + STOP_GRACE;
        while Instant::now() < deadline && ptys.iter().any(crate::process_tree::PtyGroup::running) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    for group in ptys {
        group.kill();
    }
    if let Some(state) = terminals {
        if let Ok(mut guard) = state.lock() {
            guard.shutdown_all();
        }
    }
}

/// Terminals still running in window `label`, each lock taken and dropped
/// in turn (StateStore, then the registry).
fn live_work_in(app: &AppHandle, label: &str) -> usize {
    let tab_ids = window_tab_ids(app, label);
    app.try_state::<Mutex<crate::pty::PtyRegistry>>()
        .map_or(0, |registry| match registry.lock() {
            Ok(guard) => guard.live_count_of(&tab_ids),
            Err(poisoned) => poisoned.into_inner().live_count_of(&tab_ids),
        })
}

/// The ids of the open tabs in window `label`.
fn window_tab_ids(app: &AppHandle, label: &str) -> Vec<String> {
    let Some(state) = app.try_state::<Mutex<StateStore>>() else {
        return Vec::new();
    };
    let Ok(state) = state.lock() else {
        return Vec::new();
    };
    state
        .data
        .tabs
        .iter()
        .filter(|tab| crate::store::window_matches(&tab.window_id, label))
        .map(|tab| tab.id.clone())
        .collect()
}

/// `CloseRequested` (the title bar, Cmd+W on the window, File > Close
/// Window): true lets the window close now. With terminals running in it
/// the close is held and the question goes on screen once; Close then
/// destroys the window (no second question), which stops its terminals.
/// During a confirmed quit every window closes without asking.
fn close_window_now(app: &AppHandle, label: &str) -> bool {
    let quitting = QUITTING.load(Ordering::SeqCst);
    let live = if quitting {
        0
    } else {
        live_work_in(app, label)
    };
    window_close_request(live, quitting, &CLOSE_ASKING, label, |live| {
        ask_to_close_window(app, label, live)
    })
}

/// [`close_window_now`] over its question list and its question, so it
/// runs in tests without an app.
fn window_close_request(
    live: usize,
    quitting: bool,
    asking: &Mutex<Vec<String>>,
    label: &str,
    ask: impl FnOnce(usize),
) -> bool {
    let mut asking = asking
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match quit_decision(live, quitting, asking.iter().any(|open| open == label)) {
        QuitDecision::Quit => true,
        QuitDecision::Ignore => false,
        QuitDecision::Ask => {
            asking.push(label.to_string());
            drop(asking);
            ask(live);
            false
        }
    }
}

fn ask_to_close_window(app: &AppHandle, label: &str, live: usize) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let app_for_answer = app.clone();
    let label = label.to_string();
    app.dialog()
        .message(quit_message(live))
        .title("Close window?")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Close window".to_string(),
            "Cancel".to_string(),
        ))
        .show(move |close| {
            if let Ok(mut asking) = CLOSE_ASKING.lock() {
                asking.retain(|open| open != &label);
            }
            if close {
                if let Some(window) = app_for_answer.get_webview_window(&label) {
                    let _ = window.destroy();
                }
            }
        });
}

fn on_window_destroyed(app: &AppHandle, label: &str) {
    // An open still starting in this window sees the window gone (it is
    // checked right before the spawn) or is killed here.
    let tab_ids = window_tab_ids(app, label);
    if let Some(terminals) = app.try_state::<Mutex<crate::pty::PtyRegistry>>() {
        for id in &tab_ids {
            crate::pty::kill_tab_pty(&terminals, id);
        }
    }
    if let Some(usage) = app.try_state::<Mutex<crate::usage::UsageStore>>() {
        if let Ok(mut usage) = usage.lock() {
            for id in &tab_ids {
                usage.forget_tab(id);
            }
        }
    }
    let Some(state) = app.try_state::<Mutex<StateStore>>() else {
        return;
    };
    let Ok(mut state) = state.lock() else {
        return;
    };
    if QUITTING.load(Ordering::SeqCst) {
        return;
    }
    let others = app
        .webview_windows()
        .keys()
        .filter(|key| key.as_str() != label)
        .count();
    if should_keep_window_for_restore(others) {
        return;
    }
    let _ = state.archive_window(label);
    drop(state);
    refresh_reopen_window_item(app);
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn has_archived_windows(app: &AppHandle) -> bool {
    let Some(state) = app.try_state::<Mutex<StateStore>>() else {
        return false;
    };
    let archived = state.lock().map(|state| state.has_archived_windows());
    archived.unwrap_or(false)
}

/// Enable File > Reopen Closed Window only while a closed window is kept.
/// Takes the state lock: never call it while holding that lock.
fn refresh_reopen_window_item(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    if let Some(item) = app.try_state::<ReopenWindowItem>() {
        let _ = item.0.set_enabled(has_archived_windows(app));
    }
    let _ = app;
}

/// Build the most recently closed window again, with its tabs and account.
/// `None` when no closed window is kept.
pub fn reopen_last_closed_window(app: &AppHandle) -> Result<Option<String>, String> {
    let state = app
        .try_state::<Mutex<StateStore>>()
        .ok_or_else(|| "state is not ready".to_string())?;
    let result = reopen_archived_window_with(&state, |window| {
        let title = title_for(app, &window.id);
        build_window(app, &window.id, &title)
    });
    refresh_reopen_window_item(app);
    result
}

/// The store side of [`reopen_last_closed_window`]: `build` opens the
/// window. If the window cannot be built it is archived again, so it stays
/// in File > Reopen Closed Window.
pub(crate) fn reopen_archived_window_with(
    state: &Mutex<StateStore>,
    build: impl FnOnce(&crate::store::WindowRecord) -> Result<(), String>,
) -> Result<Option<String>, String> {
    let reopened = {
        let mut state = state.lock().map_err(|err| err.to_string())?;
        state.reopen_archived_window()?
    };
    let Some(window) = reopened else {
        return Ok(None);
    };
    if let Err(err) = build(&window) {
        // The build error is the cause; a failed put-back is only added to it.
        let archived = state
            .lock()
            .map_err(|err| err.to_string())
            .and_then(|mut state| state.archive_window(&window.id));
        return Err(match archived {
            Ok(()) => err,
            Err(archive_err) => {
                format!("{err} (and it could not be kept for Reopen Closed Window: {archive_err})")
            }
        });
    }
    Ok(Some(window.id))
}

/// File > Reopen Closed Window from the page. Returns the reopened label.
#[tauri::command]
pub async fn reopen_closed_window(app: AppHandle) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || reopen_last_closed_window(&app))
        .await
        .map_err(|err| err.to_string())?
}

/// A saved window label that can name a window: what `next_window_label`
/// and `main` look like. A hand-edited `state.json` may hold anything else.
pub(crate) fn valid_window_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 64
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Recreate saved windows other than the one Tauri already opened, and
/// title them. Labels that cannot name a window are skipped, and when no
/// window came up at all (none saved, or none could be built) `main` is
/// built, so a launch never leaves the app with no window.
pub fn restore_saved_windows(app: &AppHandle) {
    let labels = {
        let Some(state) = app.try_state::<Mutex<StateStore>>() else {
            return;
        };
        let Ok(state) = state.lock() else {
            return;
        };
        state
            .data
            .windows
            .iter()
            .map(|window| window.id.clone())
            .collect::<Vec<_>>()
    };
    let mut shown = 0;
    for label in labels {
        if !valid_window_label(&label) {
            eprintln!("DCTerminalv2: skipped a saved window with a bad label: {label:?}");
            continue;
        }
        let title = title_for(app, &label);
        match build_window(app, &label, &title) {
            Ok(()) => shown += 1,
            Err(err) => eprintln!("DCTerminalv2: could not restore window {label}: {err}"),
        }
    }
    if shown == 0 {
        if !window_is_saved(app, MAIN_WINDOW_ID) {
            if let Err(err) = remember_window(app, MAIN_WINDOW_ID, &first_account(app)) {
                eprintln!("DCTerminalv2: could not save the main window: {err}");
            }
        }
        let title = title_for(app, MAIN_WINDOW_ID);
        if let Err(err) = build_window(app, MAIN_WINDOW_ID, &title) {
            eprintln!("DCTerminalv2: could not open a window: {err}");
        }
    }
}

fn title_for(app: &AppHandle, label: &str) -> String {
    let Some(state) = app.try_state::<Mutex<StateStore>>() else {
        return window_title("");
    };
    let Some(settings) = app.try_state::<Mutex<SettingsStore>>() else {
        return window_title("");
    };
    let Ok(state) = state.lock() else {
        return window_title("");
    };
    let Ok(settings) = settings.lock() else {
        return window_title("");
    };
    let account = state.account_for_window(label);
    window_title(&account_display_name(settings.claude(), &account))
}

fn retitle(app: &AppHandle, label: &str) -> Result<(), String> {
    let title = title_for(app, label);
    if let Some(window) = app.get_webview_window(label) {
        window.set_title(&title).map_err(|err| err.to_string())?;
    }
    Ok(())
}

/// Show an existing window, or recreate the saved one (main, else the last
/// record). Also what a second launch of the app does to this one.
pub fn restore_a_window(app: &AppHandle) -> Result<(), String> {
    let label = {
        let state = app
            .try_state::<Mutex<StateStore>>()
            .ok_or_else(|| "state is not ready".to_string())?;
        let state = state.lock().map_err(|err| err.to_string())?;
        if state
            .data
            .windows
            .iter()
            .any(|window| window.id == MAIN_WINDOW_ID)
        {
            MAIN_WINDOW_ID.to_string()
        } else {
            state
                .data
                .windows
                .last()
                .map(|window| window.id.clone())
                .unwrap_or_else(|| MAIN_WINDOW_ID.to_string())
        }
    };
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        retitle(app, &label)?;
        return Ok(());
    }
    if !window_is_saved(app, &label) {
        remember_window(app, &label, &first_account(app))?;
    }
    let title = title_for(app, &label);
    build_window(app, &label, &title)
}

fn window_is_saved(app: &AppHandle, label: &str) -> bool {
    let Some(state) = app.try_state::<Mutex<StateStore>>() else {
        return false;
    };
    let Ok(state) = state.lock() else {
        return false;
    };
    state.data.windows.iter().any(|window| window.id == label)
}

/// The first account in Settings, for a window saved with none.
fn first_account(app: &AppHandle) -> String {
    app.try_state::<Mutex<SettingsStore>>()
        .and_then(|settings| {
            let settings = settings.lock().ok()?;
            settings
                .claude()
                .accounts
                .first()
                .map(|account| account.id.clone())
        })
        .unwrap_or_else(|| crate::store::DEFAULT_ACCOUNT_ID.to_string())
}

fn remember_window(app: &AppHandle, label: &str, account_id: &str) -> Result<(), String> {
    let state = app
        .try_state::<Mutex<StateStore>>()
        .ok_or_else(|| "state is not ready".to_string())?;
    let mut state = state.lock().map_err(|err| err.to_string())?;
    state.ensure_window(label, account_id)
}

fn build_window(app: &AppHandle, label: &str, title: &str) -> Result<(), String> {
    if app.get_webview_window(label).is_some() {
        return retitle(app, label);
    }
    tauri::WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::App("index.html".into()))
        .title(title)
        .inner_size(1400.0, 900.0)
        .build()
        .map_err(|err| err.to_string())?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowContext {
    pub id: String,
    pub account_id: String,
    pub account_name: String,
    pub title: String,
    pub config: ConfigDirInfo,
    /// The window's account is no longer in Settings: its Claude tabs do
    /// not start until another account is picked (`set_window_account`).
    pub account_missing: bool,
}

/// `label`'s account as the page shows it. Takes both store locks
/// (StateStore first).
pub(crate) fn context_for(
    state: &StateStore,
    settings: &SettingsStore,
    label: &str,
) -> WindowContext {
    let account_id = state.account_for_window(label);
    let account_name = account_display_name(settings.claude(), &account_id);
    let config = resolved_account_config(settings.claude(), &account_id);
    let title = window_title(&account_name);
    WindowContext {
        id: label.to_string(),
        account_missing: !account_exists(settings.claude(), &account_id),
        account_id,
        account_name,
        title,
        config,
    }
}

#[tauri::command]
pub fn window_context(
    window: tauri::WebviewWindow,
    state: State<'_, Mutex<StateStore>>,
    settings: State<'_, Mutex<SettingsStore>>,
) -> Result<WindowContext, String> {
    let ctx = {
        let state = state.lock().map_err(|err| err.to_string())?;
        let settings = settings.lock().map_err(|err| err.to_string())?;
        context_for(&state, &settings, window.label())
    };
    let _ = window.set_title(&ctx.title);
    Ok(ctx)
}

/// Move this window to `account_id` (after its own account was removed).
/// Its tabs keep their saved sessions; one saved under another account
/// starts fresh when it is next shown.
#[tauri::command]
pub fn set_window_account(
    window: tauri::WebviewWindow,
    account_id: String,
    state: State<'_, Mutex<StateStore>>,
    settings: State<'_, Mutex<SettingsStore>>,
) -> Result<WindowContext, String> {
    let ctx = {
        let mut state = state.lock().map_err(|err| err.to_string())?;
        let settings = settings.lock().map_err(|err| err.to_string())?;
        move_window_to_account(&mut state, &settings, window.label(), account_id.trim())?
    };
    let _ = window.set_title(&ctx.title);
    Ok(ctx)
}

pub(crate) fn move_window_to_account(
    state: &mut StateStore,
    settings: &SettingsStore,
    label: &str,
    account_id: &str,
) -> Result<WindowContext, String> {
    if !account_exists(settings.claude(), account_id) {
        return Err("That Claude account is not in Settings.".into());
    }
    state.ensure_window(label, account_id)?;
    Ok(context_for(state, settings, label))
}

/// Retitle every open window after an account rename.
pub fn retitle_open_windows(app: &AppHandle) {
    let labels: Vec<String> = app
        .webview_windows()
        .keys()
        .map(|label| label.to_string())
        .collect();
    for label in labels {
        let _ = retitle(app, &label);
    }
}

/// Open another window on `account_id`. Two windows may share an account.
#[tauri::command]
pub async fn open_account_window(app: AppHandle, account_id: String) -> Result<String, String> {
    let account_id = account_id.trim().to_string();
    if account_id.is_empty() {
        return Err("Pick an account for the new window.".into());
    }
    let (label, title) = {
        let settings = app
            .try_state::<Mutex<SettingsStore>>()
            .ok_or_else(|| "settings are not ready".to_string())?;
        let settings = settings.lock().map_err(|err| err.to_string())?;
        if !settings
            .claude()
            .accounts
            .iter()
            .any(|account| account.id == account_id)
        {
            return Err("That Claude account is not in Settings.".into());
        }
        let title = window_title(&account_display_name(settings.claude(), &account_id));
        drop(settings);
        let state = app
            .try_state::<Mutex<StateStore>>()
            .ok_or_else(|| "state is not ready".to_string())?;
        let mut state = state.lock().map_err(|err| err.to_string())?;
        let label = state.next_window_label();
        state.ensure_window(&label, &account_id)?;
        (label, title)
    };
    let app_for_build = app.clone();
    let label_for_build = label.clone();
    let built = tauri::async_runtime::spawn_blocking(move || {
        build_window(&app_for_build, &label_for_build, &title)
    })
    .await
    .map_err(|err| err.to_string())
    .and_then(|built| built);
    if let Err(err) = built {
        // Saved first so the new page finds its account; not kept when no
        // window came of it, or every launch would restore one.
        if let Some(state) = app.try_state::<Mutex<StateStore>>() {
            if let Ok(mut state) = state.lock() {
                let _ = state.forget_empty_window(&label);
            }
        }
        return Err(err);
    }
    Ok(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_include_the_account_on_every_platform() {
        assert_eq!(window_title("Personal"), "DCTerminalv2 — Personal");
        assert_eq!(window_title(" Company "), "DCTerminalv2 — Company");
        assert_eq!(window_title("  "), "DCTerminalv2");
    }

    #[test]
    fn last_window_quits_except_on_macos() {
        assert!(!quit_when_last_window_closes("macos"));
        assert!(!quit_when_last_window_closes("MacOS"));
        assert!(quit_when_last_window_closes("windows"));
        assert!(quit_when_last_window_closes("linux"));
    }

    #[test]
    fn dock_click_reopens_only_when_nothing_is_visible() {
        assert!(dock_reopen_creates_window(false));
        assert!(!dock_reopen_creates_window(true));
    }

    #[test]
    fn only_the_last_window_is_kept_for_restore() {
        assert!(should_keep_window_for_restore(0));
        assert!(!should_keep_window_for_restore(1));
    }

    #[test]
    fn quitting_with_nothing_live_quits_at_once() {
        assert_eq!(quit_decision(0, false, false), QuitDecision::Quit);
    }

    #[test]
    fn quitting_with_live_work_asks_first() {
        assert_eq!(quit_decision(1, false, false), QuitDecision::Ask);
        assert_eq!(quit_decision(3, false, false), QuitDecision::Ask);
    }

    #[test]
    fn a_confirmed_quit_goes_through_without_asking_again() {
        assert_eq!(quit_decision(2, true, false), QuitDecision::Quit);
        assert_eq!(quit_decision(2, true, true), QuitDecision::Quit);
    }

    #[test]
    fn a_quit_while_the_question_is_open_is_ignored() {
        assert_eq!(quit_decision(2, false, true), QuitDecision::Ignore);
        assert_eq!(quit_decision(0, false, true), QuitDecision::Ignore);
    }

    const TWO_LIVE: usize = 2;

    #[test]
    fn a_dock_quit_with_live_work_asks_and_does_not_terminate_yet() {
        let (quitting, asking) = (AtomicBool::new(false), AtomicBool::new(false));
        let mut asked = None;
        let now = terminate_now(TWO_LIVE, &quitting, &asking, |live| asked = Some(live));
        assert!(!now);
        assert_eq!(asked, Some(TWO_LIVE));
        assert!(asking.load(Ordering::SeqCst));
        assert!(!quitting.load(Ordering::SeqCst));
    }

    #[test]
    fn a_dock_quit_with_nothing_live_terminates_now_without_asking() {
        let (quitting, asking) = (AtomicBool::new(false), AtomicBool::new(false));
        let now = terminate_now(0, &quitting, &asking, |_| {
            panic!("nothing is running: no question")
        });
        assert!(now);
        assert!(
            quitting.load(Ordering::SeqCst),
            "windows are not archived on this quit"
        );
    }

    #[test]
    fn a_dock_quit_after_the_user_said_quit_terminates_now() {
        let (quitting, asking) = (AtomicBool::new(true), AtomicBool::new(false));
        let now = terminate_now(TWO_LIVE, &quitting, &asking, |_| {
            panic!("already confirmed")
        });
        assert!(now);
    }

    #[test]
    fn a_second_dock_quit_while_the_question_is_open_neither_asks_nor_terminates() {
        let (quitting, asking) = (AtomicBool::new(false), AtomicBool::new(true));
        let now = terminate_now(TWO_LIVE, &quitting, &asking, |_| {
            panic!("one question at a time")
        });
        assert!(!now);
    }

    #[test]
    fn closing_the_last_window_on_macos_still_keeps_the_process_up() {
        assert_eq!(exit_step(None, "macos", false), ExitStep::StayUp);
        assert_eq!(exit_step(None, "macos", true), ExitStep::StayUp);
        // Elsewhere the last window quits, as before (its close asked first
        // when terminals were running in it: `close_window_now`).
        assert_eq!(exit_step(None, "windows", false), ExitStep::Proceed);
        assert_eq!(exit_step(None, "linux", false), ExitStep::Proceed);
    }

    #[test]
    fn a_programmatic_exit_asks_unless_already_confirmed() {
        assert_eq!(exit_step(Some(0), "macos", false), ExitStep::AskFirst);
        assert_eq!(exit_step(Some(0), "windows", false), ExitStep::AskFirst);
        // Our own `app.exit(0)` after Quit must not loop back into a question.
        assert_eq!(exit_step(Some(0), "macos", true), ExitStep::Proceed);
    }

    #[test]
    fn the_quit_question_names_what_stops() {
        assert_eq!(quit_message(1), "1 running terminal will be stopped.");
        assert_eq!(quit_message(3), "3 running terminals will be stopped.");
    }

    #[test]
    fn live_terminals_are_counted_without_keeping_the_lock() {
        let terminals = Mutex::new(crate::pty::PtyRegistry::new());
        assert_eq!(live_terminals(Some(&terminals)), 0);
        assert!(
            terminals.try_lock().is_ok(),
            "free again before anyone is asked"
        );
        // A registry not set up yet counts nothing.
        assert_eq!(live_terminals(None), 0);
    }

    // Review: a window closed from its title bar stopped every terminal in
    // it without asking (Quit and tab close both ask).
    #[test]
    fn closing_a_window_with_running_terminals_asks_once_first() {
        let asking = Mutex::new(Vec::new());
        let mut asked = None;
        assert!(!window_close_request(2, false, &asking, "win-2", |live| {
            asked = Some(live)
        }));
        assert_eq!(asked, Some(2), "the question names what stops");
        assert!(
            !window_close_request(2, false, &asking, "win-2", |_| panic!("one question")),
            "a second close while it is asked is held too"
        );
        let mut other = false;
        assert!(!window_close_request(1, false, &asking, "main", |_| {
            other = true
        }));
        assert!(other, "another window asks for itself");
    }

    #[test]
    fn a_window_with_nothing_running_or_during_a_quit_closes_at_once() {
        let asking = Mutex::new(Vec::new());
        assert!(window_close_request(
            0,
            false,
            &asking,
            "win-2",
            |_| panic!("idle")
        ));
        assert!(window_close_request(3, true, &asking, "win-2", |_| panic!(
            "quitting"
        )));
        assert!(asking.lock().unwrap().is_empty());
        assert_eq!(CLOSE_WINDOW_ACCELERATOR, "CmdOrCtrl+Shift+W");
    }

    #[test]
    fn page_menu_items_have_their_own_ids_and_shortcuts() {
        let ids: Vec<&str> = PAGE_ITEMS.iter().map(|(id, _, _)| *id).collect();
        assert_eq!(ids, ["new-tab", "close-tab", "reopen-closed-tab"]);
        assert!(PAGE_ITEMS
            .iter()
            .all(|(_, _, shortcut)| shortcut.starts_with("CmdOrCtrl+")));
        assert!(!ids.contains(&SETTINGS_ITEM) && !ids.contains(&NEW_WINDOW_EVENT));
    }

    #[test]
    fn a_window_that_cannot_be_built_stays_closed_for_reopen() {
        let dir = crate::test_support::TempDir::new("windows_reopen_fail");
        let mut store = StateStore::open_path(dir.join("state.json")).unwrap();
        store.ensure_window("win-2", "work").unwrap();
        store.bind_window(Some("win-2"));
        store
            .create_tab("/w/a", crate::store::TabKind::Claude)
            .unwrap();
        store.bind_window(None);
        store.archive_window("win-2").unwrap();
        let state = Mutex::new(store);
        let err =
            reopen_archived_window_with(&state, |_| Err("no display".to_string())).unwrap_err();
        assert_eq!(err, "no display");
        assert!(
            state.lock().unwrap().has_archived_windows(),
            "kept for another try"
        );
        let reopened = reopen_archived_window_with(&state, |window| {
            assert_eq!(window.account_id, "work");
            Ok(())
        })
        .unwrap();
        assert_eq!(reopened.as_deref(), Some("win-2"));
        assert_eq!(state.lock().unwrap().tabs_in_window("win-2").len(), 1);
    }

    #[test]
    fn a_window_whose_account_was_removed_says_so_until_another_is_picked() {
        let dir = crate::test_support::TempDir::new("windows_account_removed");
        let mut state = StateStore::open_path(dir.join("state.json")).unwrap();
        state.ensure_window("win-2", "gone").unwrap();
        let mut settings = SettingsStore::open(dir.path()).unwrap();
        settings
            .set_claude(crate::store::ClaudeSettings {
                claude_path: None,
                accounts: vec![crate::store::settings_store::ClaudeAccount {
                    id: "work".into(),
                    name: "Work".into(),
                    config_dir: None,
                }],
            })
            .unwrap();
        let ctx = context_for(&state, &settings, "win-2");
        assert!(ctx.account_missing);
        assert_eq!(ctx.title, "DCTerminalv2 — Account removed");
        assert!(move_window_to_account(&mut state, &settings, "win-2", "nope").is_err());
        let ctx = move_window_to_account(&mut state, &settings, "win-2", "work").unwrap();
        assert!(!ctx.account_missing);
        assert_eq!(ctx.account_id, "work");
        assert_eq!(state.account_for_window("win-2"), "work");
    }

    #[test]
    fn only_labels_that_can_name_a_window_are_restored() {
        assert!(valid_window_label("main"));
        assert!(valid_window_label("win-2"));
        assert!(!valid_window_label(""));
        assert!(!valid_window_label("../x"));
        assert!(!valid_window_label("a b"));
        assert!(!valid_window_label(&"w".repeat(65)));
    }

    #[test]
    fn shortcut_is_command_on_mac_and_control_on_windows() {
        assert_eq!(NEW_WINDOW_ACCELERATOR, "CmdOrCtrl+Shift+N");
        assert!(NEW_WINDOW_ACCELERATOR.contains("Shift+N"));
        assert!(NEW_WINDOW_ACCELERATOR.starts_with("CmdOrCtrl+"));
    }
}
