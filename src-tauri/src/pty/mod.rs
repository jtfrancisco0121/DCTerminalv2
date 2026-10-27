//! Embedded terminals. One PTY per tab id, streamed to the webview on a
//! channel.
//!
//! A tab is a folder plus what runs in it: `claude` under its window's
//! account (`CLAUDE_CONFIG_DIR`), or the login shell (with that same
//! `CLAUDE_CONFIG_DIR`, for a `claude` typed there). A window whose account
//! was removed starts no Claude tab. The tab record says
//! which; `pty_open` reads it, so the view only names the tab. A Claude tab
//! is pinned to a session id (`--session-id`) saved on the tab, so after an
//! exit or a restart it can be resumed (`--resume`) while that session's log
//! exists under the same account.

pub(crate) mod session;
mod shell;

use crate::paths::validate_working_folder;
use crate::process_tree::STOP_GRACE;
use crate::provider::claude_config::ConfigDirSource;
use crate::provider::{new_claude_session_id, ClaudeProvider, TerminalKind, TerminalLaunch};
use crate::pty::session::{PtyOutput, PtySession, SpawnSpec};
use crate::pty::shell::resolve_shell_for_host;
use crate::store::settings_store::{known_account_config, ACCOUNT_REMOVED};
use crate::store::{SettingsStore, StateStore, TabKind};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyPacket {
    pub kind: String,
    pub data: String,
    pub code: Option<i32>,
}

pub struct PtyRegistry {
    sessions: HashMap<String, PtySession>,
    /// The latest open per id. A `kill` meanwhile cancels it; a newer open
    /// of the same id supersedes it.
    opening: HashMap<String, u64>,
    next_open: u64,
    /// A replaced or closed program that may still be stopping, so every
    /// open of that id waits for it, not only the one that replaced it.
    stopping: HashMap<String, crate::process_tree::PtyGroup>,
}

fn data_packet(bytes: &[u8]) -> PtyPacket {
    PtyPacket {
        kind: "data".to_string(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        code: None,
    }
}

fn exit_packet(code: Option<i32>) -> PtyPacket {
    PtyPacket {
        kind: "exit".to_string(),
        data: String::new(),
        code,
    }
}

impl Default for PtyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PtyRegistry {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            opening: HashMap::new(),
            next_open: 0,
            stopping: HashMap::new(),
        }
    }

    /// Close the terminal `id`. Its program keeps stopping for up to
    /// [`STOP_GRACE`] (its control thread kills it then), so its group is
    /// kept in `stopping`: an open of the same id right after the close
    /// waits for it, as a replacing open does, and a resumed Claude never
    /// writes `<id>.jsonl` beside the one still finishing. Groups that are
    /// done are dropped here.
    pub fn kill(&mut self, id: &str) {
        self.opening.remove(id);
        if let Some(old) = self.sessions.remove(id) {
            let group = old.group();
            // Dropping the session asks its control thread to stop the program.
            drop(old);
            self.stopping.insert(id.to_string(), group);
        }
        self.prune_stopping();
    }

    /// Forget stopping programs whose group has exited.
    fn prune_stopping(&mut self) {
        self.stopping.retain(|_, group| group.running());
    }

    pub fn shutdown_all(&mut self) {
        self.sessions.clear();
        self.opening.clear();
        self.stopping.clear();
    }

    /// App exit, step 1: every terminal's running process group, so the
    /// caller can SIGTERM them and wait without holding this lock. A
    /// terminal whose program already exited and was reaped is skipped: its
    /// pid may now lead an unrelated group. A replaced program that is still
    /// stopping (a reopen is waiting on it) is included.
    pub fn running_groups(&self) -> Vec<crate::process_tree::PtyGroup> {
        self.sessions
            .values()
            .map(PtySession::group)
            .chain(self.stopping.values().cloned())
            .filter(|group| group.running())
            .collect()
    }

    /// Terminals whose program is still running (what a quit would stop).
    /// Windows cannot tell, so every open terminal counts there.
    pub fn live_count(&self) -> usize {
        self.sessions
            .values()
            .filter(|session| live(session))
            .count()
    }

    /// [`Self::live_count`] over the terminals `ids` only (one window's tabs).
    pub fn live_count_of(&self, ids: &[String]) -> usize {
        ids.iter()
            .filter_map(|id| self.sessions.get(id))
            .filter(|session| live(session))
            .count()
    }
}

fn live(session: &PtySession) -> bool {
    cfg!(not(unix)) || session.group().running()
}

impl Drop for PtyRegistry {
    fn drop(&mut self) {
        self.shutdown_all();
    }
}

/// What runs once when the program has exited (the app resets the tab's
/// hook state there).
type OnExit = Box<dyn FnOnce() + Send>;

/// Streams the program's output to the view. `on_exit` runs when the
/// program exited (its exit packet, or the output ended). It does not run
/// when the view's channel fails first (a webview reload while the program
/// still runs): that program is not known to be gone.
fn forward(output: Receiver<PtyOutput>, channel: Channel<PtyPacket>, on_exit: OnExit) {
    loop {
        match output.recv() {
            Ok(PtyOutput::Data(bytes)) => {
                if channel.send(data_packet(&bytes)).is_err() {
                    return;
                }
            }
            Ok(PtyOutput::Exit { code }) => {
                let _ = channel.send(exit_packet(code));
                on_exit();
                return;
            }
            Err(_) => {
                on_exit();
                return;
            }
        }
    }
}

/// True while the app theme is a light one. Claude Code reads COLORFGBG to pick
/// its own light or dark palette (theme "auto"), so a Claude terminal is told
/// the app theme through that variable. Nothing is written to ~/.claude*.
static LIGHT_SCHEME: AtomicBool = AtomicBool::new(false);

fn claude_scheme_env() -> (String, String) {
    let value = if LIGHT_SCHEME.load(Ordering::Relaxed) {
        "0;15"
    } else {
        "15;0"
    };
    ("COLORFGBG".to_string(), value.to_string())
}

/// The `--settings` value with DCTerminalv2's status line for a Claude
/// terminal, so the tab reports Claude usage (`status_line`), and has
/// `UsageStore` read what it writes. None when you set a status line of
/// your own (it wins; usage from the tab stays unknown) or the app's own
/// path is unknown.
fn usage_status_line(
    app: &AppHandle,
    tab_id: &str,
    config_dir: &str,
    cwd: &std::path::Path,
) -> Option<serde_json::Value> {
    let config = config_dir.trim();
    if config.is_empty()
        || crate::status_line::user_has_status_line(std::path::Path::new(config), cwd)
    {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let file = crate::status_line::file_for(&crate::data_dir::app_data_dir(app).path, tab_id);
    let usage = app.try_state::<Mutex<crate::usage::UsageStore>>()?;
    let mut usage = usage.lock().ok()?;
    usage.watch_status_line(tab_id, config, file.clone());
    drop(usage);
    Some(crate::status_line::settings(&crate::status_line::command(
        &exe, &file,
    )))
}

#[tauri::command]
pub fn pty_set_scheme(light: bool) {
    LIGHT_SCHEME.store(light, Ordering::Relaxed);
}

/// Register an open of the terminal `id` and return its token. From here
/// on a `kill` of `id` cancels the open, so it is called before anything
/// else of the open is read or checked.
fn begin_open(registry: &Mutex<PtyRegistry>, id: &str) -> Result<u64, String> {
    let mut registry = registry.lock().map_err(|err| err.to_string())?;
    registry.next_open += 1;
    let token = registry.next_open;
    registry.opening.insert(id.to_string(), token);
    Ok(token)
}

/// An open that stops before [`open_session_as`] (a bad folder, say)
/// takes its token back, unless a newer open or a kill already did.
fn end_open(registry: &Mutex<PtyRegistry>, id: &str, token: u64) {
    if let Ok(mut registry) = registry.lock() {
        if registry.opening.get(id) == Some(&token) {
            registry.opening.remove(id);
        }
    }
}

/// Whether the open `token` of `id` is still the one wanted: `Ok(true)`
/// to go on, `Ok(false)` when a newer open took over, an error when the
/// terminal was closed meanwhile.
fn open_is_current(registry: &PtyRegistry, id: &str, token: u64) -> Result<bool, String> {
    match registry.opening.get(id) {
        Some(latest) if *latest == token => Ok(true),
        // A newer open of this id took over (it may have finished already).
        Some(_) => Ok(false),
        None if registry.sessions.contains_key(id) => Ok(false),
        None => Err("The terminal was closed while it was starting.".to_string()),
    }
}

/// Start the terminal `id`, replacing the one running under that id
/// ([`begin_open`] then [`open_session_as`], wanted until spawned).
#[cfg(test)]
fn open_session(
    registry: &Mutex<PtyRegistry>,
    id: &str,
    spec: SpawnSpec,
    on_output: Channel<PtyPacket>,
) -> Result<Option<u32>, String> {
    let token = begin_open(registry, id)?;
    open_session_as(registry, id, token, spec, on_output, || true, || Ok(()))
}

/// [`open_session_with`] with nothing to save after the spawn.
#[cfg(test)]
fn open_session_as(
    registry: &Mutex<PtyRegistry>,
    id: &str,
    token: u64,
    spec: SpawnSpec,
    on_output: Channel<PtyPacket>,
    still_wanted: impl FnOnce() -> bool,
    before_spawn: impl FnOnce() -> Result<(), String>,
) -> Result<Option<u32>, String> {
    open_session_with(
        registry,
        id,
        token,
        spec,
        on_output,
        still_wanted,
        before_spawn,
        || Ok(()),
        Box::new(|| {}),
    )
}

/// Start the terminal `id` for the open `token` ([`begin_open`]),
/// replacing the one running under that id. `after_spawn` runs under the
/// registry lock right after the program started for the current open (and
/// never for a superseded, closed or failed one): it saves what the open
/// changes in the tab record (lock order: `PtyRegistry` before
/// `StateStore`). Its failure is logged and does not stop the terminal.
/// `on_exit` runs once when the started program has exited (see `forward`).
///
/// Blocking: runs on a `spawn_blocking` thread, never the UI thread or a
/// Tokio worker. A replaced (or just closed) program is stopped first, and
/// the new one starts only once its group has exited (at most
/// [`STOP_GRACE`] plus a margin), so a Claude resumed with the same session
/// id never writes `<id>.jsonl` alongside the old one. The registry lock is
/// not held during that wait. Closing the terminal meanwhile cancels the
/// open (an error); a newer open of the same id supersedes it (`Ok(None)`,
/// nothing started). `still_wanted` is asked under the registry lock right
/// before the spawn (the tab and its window still exist); false cancels
/// the open. It may take the `StateStore` lock (lock order: `PtyRegistry`
/// before `StateStore`), so nobody takes the registry while holding that.
#[allow(clippy::too_many_arguments)]
fn open_session_with(
    registry: &Mutex<PtyRegistry>,
    id: &str,
    token: u64,
    spec: SpawnSpec,
    on_output: Channel<PtyPacket>,
    still_wanted: impl FnOnce() -> bool,
    before_spawn: impl FnOnce() -> Result<(), String>,
    after_spawn: impl FnOnce() -> Result<(), String>,
    on_exit: OnExit,
) -> Result<Option<u32>, String> {
    let stopping = {
        let mut registry = registry.lock().map_err(|err| err.to_string())?;
        if !open_is_current(&registry, id, token)? {
            return Ok(None);
        }
        registry.prune_stopping();
        match registry.sessions.remove(id) {
            Some(old) => {
                let group = old.group();
                // Dropping the session asks its control thread to stop the program.
                drop(old);
                registry.stopping.insert(id.to_string(), group.clone());
                Some(group)
            }
            None => registry.stopping.get(id).cloned(),
        }
    };
    if let Some(group) = stopping {
        wait_until_stopped(&group);
    }
    // After the old program is gone (so it is never mistaken for another).
    if let Err(err) = before_spawn() {
        if let Ok(mut registry) = registry.lock() {
            if open_is_current(&registry, id, token).unwrap_or(false) {
                registry.opening.remove(id);
            }
        }
        return Err(err);
    }
    let mut registry = registry.lock().map_err(|err| err.to_string())?;
    if !open_is_current(&registry, id, token)? {
        return Ok(None);
    }
    registry.opening.remove(id);
    if !still_wanted() {
        return Err("The tab was closed while its terminal was starting.".to_string());
    }
    registry.stopping.remove(id);
    let mut session = PtySession::spawn(spec).map_err(|err| err.to_string())?;
    let pid = session.pid();
    if let Err(err) = after_spawn() {
        eprintln!("DCTerminalv2: could not save the terminal's session: {err}");
    }
    if let Some(rx) = session.take_output() {
        thread::Builder::new()
            .name(format!("pty-fwd-{id}"))
            .spawn(move || forward(rx, on_output, on_exit))
            .map_err(|err| err.to_string())?;
    }
    registry.sessions.insert(id.to_string(), session);
    Ok(Some(pid))
}

/// Wait until a replaced terminal's program group has exited. Its control
/// thread kills it after [`STOP_GRACE`]; the margin covers that last step.
fn wait_until_stopped(group: &crate::process_tree::PtyGroup) {
    let deadline = std::time::Instant::now() + STOP_GRACE + Duration::from_millis(500);
    while group.running() && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyOpenInput {
    /// The tab id; the terminal runs what the tab record says.
    pub id: String,
    pub cols: u16,
    pub rows: u16,
    /// Claude tabs: resume the tab's saved session instead of starting a
    /// new one. Refused when that session cannot be resumed.
    #[serde(default)]
    pub resume: bool,
}

/// What a Claude tab launches: the session it resumes or a fresh one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaudeStart {
    pub session_id: String,
    pub resumed: bool,
}

/// The session a Claude tab starts with. `resumable` is the tab's saved
/// session when it can be resumed now (`claude_session_log`).
pub(crate) fn claude_start(resume: bool, resumable: Option<String>) -> Result<ClaudeStart, String> {
    match (resume, resumable) {
        (true, Some(id)) => Ok(ClaudeStart {
            session_id: id,
            resumed: true,
        }),
        (true, None) => Err(
            "This session can no longer be resumed (its log is gone, or the window uses another account). Start fresh instead."
                .to_string(),
        ),
        (false, _) => Ok(ClaudeStart {
            session_id: new_claude_session_id(),
            resumed: false,
        }),
    }
}

/// An account whose folder you set but that does not exist would have
/// `claude` create it, under a name you may have mistyped. The default
/// `~/.claude` is Claude Code's own first-run folder and may be created.
pub(crate) fn check_account_folder(
    config: &crate::provider::claude_config::ConfigDirInfo,
) -> Result<(), String> {
    if config.exists || config.source == ConfigDirSource::Default {
        return Ok(());
    }
    Err(format!(
        "Claude config folder not found: {}. Pick an existing folder for this account in Settings.",
        config.path
    ))
}

/// What a shell tab adds to its environment: `CLAUDE_CONFIG_DIR` set to
/// the window's account folder, so a `claude` typed there uses the
/// window's account and not one inherited from where the app started. With
/// the account removed, an inherited value is dropped instead (`claude`
/// then uses `~/.claude`, as in any plain terminal).
pub(crate) fn shell_account_env(
    config: Option<&crate::provider::claude_config::ConfigDirInfo>,
) -> (Vec<(String, String)>, Vec<String>) {
    match config {
        Some(config) => (
            crate::provider::claude_config::claude_env(config),
            Vec::new(),
        ),
        None => (
            Vec::new(),
            vec![crate::provider::claude_config::CLAUDE_CONFIG_DIR.to_string()],
        ),
    }
}

/// Async with the work on a blocking thread: replacing a running terminal
/// waits for its program to exit (see `open_session`).
#[tauri::command]
pub async fn pty_open(
    app: AppHandle,
    input: PtyOpenInput,
    on_output: Channel<PtyPacket>,
) -> Result<u32, String> {
    tauri::async_runtime::spawn_blocking(move || {
        pty_open_blocking(
            &app,
            input,
            on_output,
            app.state(),
            app.state(),
            app.state(),
        )
    })
    .await
    .map_err(|err| err.to_string())?
}

fn pty_open_blocking(
    app: &AppHandle,
    input: PtyOpenInput,
    on_output: Channel<PtyPacket>,
    settings: State<Mutex<SettingsStore>>,
    store: State<Mutex<StateStore>>,
    registry: State<Mutex<PtyRegistry>>,
) -> Result<u32, String> {
    // First, so a close of the tab (or its window) from here on cancels it.
    let token = begin_open(&registry, &input.id)?;
    let SpawnPlan {
        tab,
        spec,
        remember,
        hook_token,
    } = match spawn_spec(app, &input, &settings, &store) {
        Ok(found) => found,
        Err(err) => {
            end_open(&registry, &input.id, token);
            return Err(err);
        }
    };
    let window = crate::store::window_label(&tab.window_id).to_string();
    let still_wanted = || {
        let tab_open = store
            .lock()
            .map(|state| state.tab_by_id(&tab.id).is_some())
            .unwrap_or(false);
        tab_open && app.get_webview_window(&window).is_some()
    };
    // An adopted (dev-app) run's conversation is never resumed while a
    // process outside this app (the dev app) still runs it.
    let resumes = adopted_run_tab(app, &tab)
        .then(|| resumed_session(&spec.args))
        .flatten();
    let before_spawn = || match resumes.as_deref() {
        Some(id) => crate::import_dev::refuse_live_session(id),
        None => Ok(()),
    };
    let after_spawn = || match remember {
        Some((session_id, config_dir)) => {
            let mut state = store.lock().map_err(|err| err.to_string())?;
            if state.tab_by_id(&tab.id).is_some() {
                state.remember_claude_session(&tab.id, &session_id, &config_dir)?;
            }
            Ok(())
        }
        None => Ok(()),
    };
    let on_exit: OnExit = match hook_token {
        Some(token) => {
            let app = app.clone();
            let tab_id = tab.id.clone();
            Box::new(move || {
                if let Some(observe) = app.try_state::<crate::observe::ObserveState>() {
                    if let Some(hooks) = &observe.hooks {
                        hooks.launch_exited(&tab_id, &token);
                    }
                }
            })
        }
        None => Box::new(|| {}),
    };
    let pid = open_session_with(
        &registry,
        &tab.id,
        token,
        spec,
        on_output,
        still_wanted,
        before_spawn,
        after_spawn,
        on_exit,
    )?;
    // The frontend does not use the pid; 0 means a newer open replaced this one.
    Ok(pid.unwrap_or(0))
}

/// A tab and what its terminal runs.
struct SpawnPlan {
    tab: crate::store::TabRecord,
    spec: SpawnSpec,
    /// Claude tabs: the `(session id, account folder)` to save once the open wins.
    remember: Option<(String, String)>,
    /// The hook token of this launch (dcv2 tabs with the forwarder).
    hook_token: Option<String>,
}

/// The tab `input.id`, what its terminal runs, and for a Claude tab the
/// `(session id, account folder)` to save once the open wins (it is not
/// saved here: a superseded or failed open must not change the tab).
/// Takes the store locks one at a time, briefly (StateStore before
/// SettingsStore).
fn spawn_spec(
    app: &AppHandle,
    input: &PtyOpenInput,
    settings: &Mutex<SettingsStore>,
    store: &Mutex<StateStore>,
) -> Result<SpawnPlan, String> {
    let (tab, account) = {
        let state = store.lock().map_err(|err| err.to_string())?;
        let tab = state
            .tab_by_id(&input.id)
            .cloned()
            .ok_or_else(|| format!("unknown tab: {}", input.id))?;
        let account = state.account_for_tab(&tab.id);
        (tab, account)
    };
    let (shell, claude, hooks_all_tabs) = {
        let settings = settings.lock().map_err(|err| err.to_string())?;
        (
            settings.terminal().shell.clone(),
            settings.claude().clone(),
            settings.observe().hooks_all_tabs,
        )
    };
    let cwd = validate_working_folder(&tab.cwd).map_err(|err| err.message())?;
    // `None` once the window's account was removed from Settings.
    let account_config = known_account_config(&claude, &account);
    let mut remember = None;
    let mut hook_token = None;
    let (program, args, env, env_remove) = match tab.kind {
        TabKind::Shell => {
            let resolved = resolve_shell_for_host(&shell);
            let (env, env_remove) = shell_account_env(account_config.as_ref());
            (resolved.program, resolved.args, env, env_remove)
        }
        TabKind::Claude => {
            // Never another account in its place: that would also save the
            // other account's session over the tab's own.
            if account_config.is_none() {
                return Err(ACCOUNT_REMOVED.to_string());
            }
            let provider = ClaudeProvider::for_account(&claude, &account);
            check_account_folder(&provider.config)?;
            let current = provider.config.path.clone();
            let resumable = crate::claude_session_log::resumable_session(
                tab.claude_session_id.as_deref(),
                tab.claude_config_dir.as_deref(),
                &current,
            );
            let start = claude_start(input.resume, resumable)?;
            let kind = if start.resumed {
                TerminalKind::Resume {
                    session_id: start.session_id.clone(),
                }
            } else {
                TerminalKind::Plain
            };
            let usage = usage_status_line(app, &tab.id, &current, &cwd);
            let launch = tab.launch.as_ref();
            // A2: dcv2 tabs also get the hook forwarder (per-launch token);
            // so does every Claude tab while `observe.hooksAllTabs` is on.
            let hooks = wants_hooks(tab.kind, launch.is_some(), hooks_all_tabs)
                .then(|| crate::observe::launch_hooks(app, &tab.id))
                .flatten();
            hook_token = hooks.as_ref().map(|h| h.2.clone());
            let app_settings = crate::observe::with_hooks(usage, hooks.as_ref().map(|h| &h.0));
            let settings = crate::run::launch::merged_settings(
                launch.and_then(|l| l.settings_file.as_deref()),
                app_settings,
            );
            let mut command = provider.terminal_command(&TerminalLaunch {
                kind,
                session_id: Some(start.session_id.clone()),
                settings,
            })?;
            command.env.push(claude_scheme_env());
            let env_remove = crate::run::launch::apply_launch(&mut command, launch, start.resumed);
            if let Some((_, env, _)) = hooks {
                command.env.extend(env);
            }
            remember = Some((start.session_id.clone(), current));
            (command.program, command.args, command.env, env_remove)
        }
    };
    let spec = SpawnSpec {
        program,
        args,
        env,
        env_remove,
        cwd,
        cols: input.cols,
        rows: input.rows,
    };
    Ok(SpawnPlan {
        tab,
        spec,
        remember,
        hook_token,
    })
}

/// Whether a Claude tab gets the hook forwarder: a run's launched tab always,
/// any other Claude tab while the setting is on. A Shell tab never does.
fn wants_hooks(kind: TabKind, has_launch: bool, hooks_all_tabs: bool) -> bool {
    kind == TabKind::Claude && (has_launch || hooks_all_tabs)
}

/// The id after `--resume` in a Claude command line.
fn resumed_session(args: &[String]) -> Option<String> {
    args.windows(2)
        .find(|w| w[0] == "--resume")
        .map(|w| w[1].clone())
}

/// The tab's launch names a run folder outside this app's own runs (adopted).
fn adopted_run_tab(app: &AppHandle, tab: &crate::store::TabRecord) -> bool {
    let Some(run) = tab.launch.as_ref().and_then(|l| l.run_dir.as_deref()) else {
        return false;
    };
    let data = crate::data_dir::app_data_dir(app).path;
    let run = crate::run::git::canonical(std::path::Path::new(run));
    crate::run::trust_data_dir(&data, &run) != data
}

/// The session the Claude tab `tab_id` could resume now, if any: its saved
/// session, while that log exists under the window's account. Reads one
/// folder listing, with no lock held.
#[tauri::command]
pub fn terminal_resume_id(
    tab_id: String,
    settings: State<Mutex<SettingsStore>>,
    store: State<Mutex<StateStore>>,
) -> Result<Option<String>, String> {
    let (tab, account) = {
        let state = store.lock().map_err(|err| err.to_string())?;
        let Some(tab) = state.tab_by_id(&tab_id).cloned() else {
            return Ok(None);
        };
        let account = state.account_for_tab(&tab.id);
        (tab, account)
    };
    if tab.kind != TabKind::Claude {
        return Ok(None);
    }
    let current = {
        let settings = settings.lock().map_err(|err| err.to_string())?;
        // A removed account resumes nothing (and its tabs do not start).
        match known_account_config(settings.claude(), &account) {
            Some(config) => config.path,
            None => return Ok(None),
        }
    };
    Ok(crate::claude_session_log::resumable_session(
        tab.claude_session_id.as_deref(),
        tab.claude_config_dir.as_deref(),
        &current,
    ))
}

/// Writes `data` to the terminal of `tab_id` (R2 Nudge, on your click).
pub fn write_to_tab(
    registry: &Mutex<PtyRegistry>,
    tab_id: &str,
    data: &[u8],
) -> Result<(), String> {
    let registry = registry.lock().map_err(|err| err.to_string())?;
    let session = registry
        .sessions
        .get(tab_id)
        .ok_or_else(|| "The tab's terminal is not running.".to_string())?;
    session.write(data).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn pty_write(
    id: String,
    data: String,
    registry: State<Mutex<PtyRegistry>>,
) -> Result<(), String> {
    let registry = registry.lock().map_err(|err| err.to_string())?;
    let session = registry
        .sessions
        .get(&id)
        .ok_or_else(|| format!("no terminal: {id}"))?;
    session
        .write(data.as_bytes())
        .map_err(|err| err.to_string())
}

/// Stays sync: async commands run on a pool and could apply resizes out of
/// order. It only queues the resize, so it never waits on the terminal.
#[tauri::command]
pub fn pty_resize(
    id: String,
    cols: u16,
    rows: u16,
    registry: State<Mutex<PtyRegistry>>,
) -> Result<(), String> {
    resize_terminal(&registry, &id, cols, rows)
}

fn resize_terminal(
    registry: &Mutex<PtyRegistry>,
    id: &str,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    // Clone the handle and drop the lock before sending, so a busy terminal
    // never holds up another terminal's input or resize.
    let handle = {
        let registry = registry.lock().map_err(|err| err.to_string())?;
        registry
            .sessions
            .get(id)
            .map(PtySession::handle)
            .ok_or_else(|| format!("no terminal: {id}"))?
    };
    handle.resize(cols, rows).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn pty_kill(id: String, registry: State<Mutex<PtyRegistry>>) -> Result<(), String> {
    kill_tab_pty(&registry, &id);
    Ok(())
}

pub fn kill_tab_pty(registry: &Mutex<PtyRegistry>, tab_id: &str) {
    if let Ok(mut registry) = registry.lock() {
        registry.kill(tab_id);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn hooks_follow_the_launch_the_setting_and_the_kind() {
        use super::wants_hooks;
        use crate::store::TabKind::{Claude, Shell};
        // (kind, launched, setting on) -> hooks
        assert!(wants_hooks(Claude, true, false));
        assert!(wants_hooks(Claude, true, true));
        assert!(wants_hooks(Claude, false, true));
        assert!(!wants_hooks(Claude, false, false));
        assert!(!wants_hooks(Shell, true, true));
        assert!(!wants_hooks(Shell, false, true));
    }

    use super::*;

    #[test]
    fn a_fresh_start_gets_a_new_session_and_a_resume_needs_one_to_resume() {
        let fresh = claude_start(false, Some("old".into())).unwrap();
        assert!(!fresh.resumed);
        assert_ne!(fresh.session_id, "old", "Start fresh drops the old session");
        assert!(crate::provider::claude::valid_session_id(&fresh.session_id));
        let resumed =
            claude_start(true, Some("685dd464-eabf-4e4c-9f6f-ac02c63aad27".into())).unwrap();
        assert_eq!(
            resumed,
            ClaudeStart {
                session_id: "685dd464-eabf-4e4c-9f6f-ac02c63aad27".into(),
                resumed: true
            }
        );
        assert!(claude_start(true, None)
            .unwrap_err()
            .contains("Start fresh"));
    }

    #[test]
    fn a_missing_account_folder_is_refused_unless_it_is_the_default() {
        use crate::provider::claude_config::resolve_with;
        let dir = crate::test_support::TempDir::new("pty_account_folder");
        let set_missing = resolve_with(None, Some(&dir.join("nope").display().to_string()), None);
        let err = check_account_folder(&set_missing).unwrap_err();
        assert!(err.contains("Claude config folder not found"), "{err}");
        assert!(!dir.join("nope").exists(), "never created");
        let set_present = resolve_with(None, Some(&dir.path().display().to_string()), None);
        assert!(check_account_folder(&set_present).is_ok());
        let default = resolve_with(None, None, Some(&dir.join("home")));
        assert!(!default.exists);
        assert!(
            check_account_folder(&default).is_ok(),
            "claude makes its own ~/.claude"
        );
    }

    // Review: shell tabs got no account at all and kept an inherited
    // CLAUDE_CONFIG_DIR, so `claude` typed there ran on another account.
    #[cfg(unix)]
    #[test]
    fn a_shell_tab_runs_claude_under_its_windows_account() {
        use crate::provider::claude_config::resolve_with;
        let dir = crate::test_support::TempDir::new("pty_shell_account");
        let account = resolve_with(None, Some("/w/.claude-account2"), None);
        let printed = |config: Option<&crate::provider::claude_config::ConfigDirInfo>| {
            let (env, env_remove) = shell_account_env(config);
            let mut spec = sh_spec(dir.path(), "printf '<%s>' \"${CLAUDE_CONFIG_DIR-unset}\"");
            if config.is_some() {
                // Set earlier, as an inherited value is: the account wins.
                spec.env
                    .push(("CLAUDE_CONFIG_DIR".into(), "/inherited".into()));
            }
            // Without an account, whatever this test process inherited goes.
            spec.env.extend(env);
            spec.env_remove = env_remove;
            let session = PtySession::spawn(spec).expect("spawn sh");
            let mut text = String::new();
            for _ in 0..100 {
                match session.recv_timeout(Duration::from_millis(50)) {
                    Ok(Some(session::PtyOutput::Data(bytes))) => {
                        text.push_str(&String::from_utf8_lossy(&bytes))
                    }
                    Ok(Some(session::PtyOutput::Exit { .. })) => break,
                    _ => {}
                }
            }
            text
        };
        assert!(
            printed(Some(&account)).contains("</w/.claude-account2>"),
            "the window's account wins over an inherited one"
        );
        assert!(
            printed(None).contains("<unset>"),
            "a removed account leaves no inherited folder behind"
        );
    }

    // Review: Quit SIGTERMed (and then waited on and SIGKILLed) the pid of a
    // terminal whose program had already exited and been reaped. That pid
    // may since lead an unrelated process group.
    #[cfg(unix)]
    #[test]
    fn quitting_skips_a_terminal_whose_program_already_exited() {
        let dir = crate::test_support::TempDir::new("pty_exited");
        let session = PtySession::spawn(sh_spec(dir.path(), "exit 0")).expect("spawn sh");
        let mut exited = false;
        for _ in 0..100 {
            if matches!(
                session.recv_timeout(Duration::from_millis(50)),
                Ok(Some(session::PtyOutput::Exit { .. }))
            ) {
                exited = true;
                break;
            }
        }
        assert!(exited, "the program exited on its own");
        let mut registry = PtyRegistry::new();
        registry.sessions.insert("done".into(), session);
        // The control thread finishes with the group right after reporting
        // the exit; give it a moment.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut left = registry.running_groups();
        while !left.is_empty() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
            left = registry.running_groups();
        }
        assert!(
            left.is_empty(),
            "an exited, reaped terminal is not signalled or waited on"
        );
    }

    /// `sh -c script` in a PTY under `dir`.
    #[cfg(unix)]
    fn sh_spec(dir: &std::path::Path, script: &str) -> SpawnSpec {
        SpawnSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            env: Vec::new(),
            env_remove: Vec::new(),
            cwd: dir.to_path_buf(),
            cols: 80,
            rows: 24,
        }
    }

    /// Every packet a view got, decoded: (kind, text, code).
    #[cfg(unix)]
    type Seen = std::sync::Arc<Mutex<Vec<(String, String, Option<i32>)>>>;

    #[cfg(unix)]
    fn recording_view() -> (Channel<PtyPacket>, Seen) {
        let seen: Seen = Default::default();
        let into = seen.clone();
        let channel = Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(json) = body {
                let packet: serde_json::Value = serde_json::from_str(&json).unwrap();
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(packet["data"].as_str().unwrap_or_default())
                    .unwrap_or_default();
                into.lock().unwrap().push((
                    packet["kind"].as_str().unwrap_or_default().to_string(),
                    String::from_utf8_lossy(&bytes).into_owned(),
                    packet["code"].as_i64().map(|c| c as i32),
                ));
            }
            Ok(())
        });
        (channel, seen)
    }

    #[cfg(unix)]
    #[test]
    fn a_view_streams_the_output_then_the_exit_code() {
        let dir = crate::test_support::TempDir::new("pty_view");
        let registry = Mutex::new(PtyRegistry::new());
        let (view, seen) = recording_view();
        open_session(
            &registry,
            "t",
            sh_spec(dir.path(), "printf 'hello'; exit 3"),
            view,
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !seen.lock().unwrap().iter().any(|p| p.0 == "exit") {
            assert!(
                std::time::Instant::now() < deadline,
                "no exit: {:?}",
                seen.lock().unwrap()
            );
            thread::sleep(Duration::from_millis(20));
        }
        let seen = seen.lock().unwrap();
        let text: String = seen
            .iter()
            .filter(|p| p.0 == "data")
            .map(|p| p.1.clone())
            .collect();
        assert!(text.contains("hello"), "{text:?}");
        assert_eq!(seen.last().unwrap().2, Some(3));
    }

    /// Opens a terminal under `id` that, like Claude finishing its session
    /// log, takes 0.4 s to exit on SIGTERM; returns its group once the trap
    /// is set.
    #[cfg(unix)]
    fn open_slow_stopper(
        registry: &Mutex<PtyRegistry>,
        dir: &std::path::Path,
        id: &str,
    ) -> crate::process_tree::PtyGroup {
        let ready = dir.join(format!("{id}.ready"));
        let script = format!(
            "trap 'sleep 0.4; exit 0' TERM\ntouch '{}'\nwhile :; do sleep 0.05; done",
            ready.display()
        );
        open_session(
            registry,
            id,
            sh_spec(dir, &script),
            Channel::new(|_| Ok(())),
        )
        .expect("open old terminal");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(std::time::Instant::now() < deadline, "old terminal ready");
            thread::sleep(Duration::from_millis(20));
        }
        registry.lock().unwrap().sessions[id].group()
    }

    // Review: restarting a Claude terminal stopped the old one in the
    // background and at once spawned `claude --resume <same id>`, so both
    // could append to <id>.jsonl while the old one finished on SIGTERM.
    #[cfg(unix)]
    #[test]
    fn reopening_a_terminal_waits_until_the_old_program_has_exited() {
        let dir = crate::test_support::TempDir::new("pty_reopen");
        let registry = Mutex::new(PtyRegistry::new());
        let old = open_slow_stopper(&registry, dir.path(), "t");
        open_session(
            &registry,
            "t",
            sh_spec(dir.path(), "sleep 5"),
            Channel::new(|_| Ok(())),
        )
        .expect("reopen");
        assert!(
            !old.running(),
            "the old program exited before the new one started"
        );
        assert!(registry.lock().unwrap().sessions.contains_key("t"));
        registry.lock().unwrap().shutdown_all();
    }

    // Review: closing a tab forgot its program at once, though it may take
    // up to STOP_GRACE to exit, so reopening the tab right away (resume)
    // started `claude --resume <id>` while the old one still wrote the log.
    #[cfg(unix)]
    #[test]
    fn reopening_right_after_a_close_waits_for_the_closed_program() {
        let dir = crate::test_support::TempDir::new("pty_close_reopen");
        let registry = Mutex::new(PtyRegistry::new());
        let old = open_slow_stopper(&registry, dir.path(), "t");
        registry.lock().unwrap().kill("t");
        assert!(old.running(), "the closed program is still finishing");
        assert_eq!(registry.lock().unwrap().running_groups().len(), 1);
        open_session(
            &registry,
            "t",
            sh_spec(dir.path(), "sleep 5"),
            Channel::new(|_| Ok(())),
        )
        .expect("reopen");
        assert!(
            !old.running(),
            "the closed program exited before the new one started"
        );
        registry.lock().unwrap().shutdown_all();
    }

    // The wait above holds no lock, so the tab can be closed during it; the
    // open must then not leave a terminal running for a closed tab.
    #[cfg(unix)]
    #[test]
    fn closing_a_terminal_while_it_reopens_cancels_the_open() {
        let dir = crate::test_support::TempDir::new("pty_reopen_closed");
        let registry = Mutex::new(PtyRegistry::new());
        open_slow_stopper(&registry, dir.path(), "t");
        let started = dir.join("started");
        thread::scope(|scope| {
            let reopen = scope.spawn(|| {
                open_session(
                    &registry,
                    "t",
                    sh_spec(
                        dir.path(),
                        &format!("touch '{}'; sleep 5", started.display()),
                    ),
                    Channel::new(|_| Ok(())),
                )
            });
            thread::sleep(Duration::from_millis(100));
            registry.lock().unwrap().kill("t");
            assert!(reopen.join().unwrap().is_err(), "the open was cancelled");
        });
        assert!(!registry.lock().unwrap().sessions.contains_key("t"));
        thread::sleep(Duration::from_millis(200));
        assert!(!started.exists(), "the new program never started");
    }

    // Review: an open registered itself only once it reached the spawn
    // step, so a tab closed while the open read the tab and its account was
    // not cancelled, and the terminal started for a closed tab.
    #[cfg(unix)]
    #[test]
    fn a_close_before_the_open_reaches_the_spawn_cancels_it() {
        let dir = crate::test_support::TempDir::new("pty_close_early");
        let registry = Mutex::new(PtyRegistry::new());
        let started = dir.join("started");
        let token = begin_open(&registry, "t").unwrap();
        registry.lock().unwrap().kill("t");
        let opened = open_session_as(
            &registry,
            "t",
            token,
            sh_spec(dir.path(), &format!("touch '{}'", started.display())),
            Channel::new(|_| Ok(())),
            || true,
            || Ok(()),
        );
        assert!(opened.is_err(), "the open was cancelled: {opened:?}");
        assert!(registry.lock().unwrap().sessions.is_empty());
        thread::sleep(Duration::from_millis(200));
        assert!(!started.exists(), "nothing started");
        // An open that gives up before the spawn step takes its token back.
        let token = begin_open(&registry, "u").unwrap();
        end_open(&registry, "u", token);
        assert!(registry.lock().unwrap().opening.is_empty());
    }

    // Ticket 14yq6g30jek: `on_exit` runs once when the program exited, and
    // not when the view's channel fails while the program still runs.
    #[cfg(unix)]
    #[test]
    fn forward_runs_on_exit_once_when_the_program_exits() {
        let dir = crate::test_support::TempDir::new("pty_on_exit");
        let registry = Mutex::new(PtyRegistry::new());
        let (view, seen) = recording_view();
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = ran.clone();
        let token = begin_open(&registry, "t").unwrap();
        open_session_with(
            &registry,
            "t",
            token,
            sh_spec(dir.path(), "printf 'bye'; exit 2"),
            view,
            || true,
            || Ok(()),
            || Ok(()),
            Box::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while ran.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        thread::sleep(Duration::from_millis(200));
        assert_eq!(ran.load(Ordering::SeqCst), 1);
        assert!(seen.lock().unwrap().iter().any(|p| p.0 == "exit"));
    }

    #[cfg(unix)]
    #[test]
    fn forward_skips_on_exit_when_the_view_fails_while_the_program_runs() {
        let dir = crate::test_support::TempDir::new("pty_on_exit_view");
        let registry = Mutex::new(PtyRegistry::new());
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = ran.clone();
        let token = begin_open(&registry, "t").unwrap();
        // A view whose channel always fails, like a reloaded webview.
        let broken: Channel<PtyPacket> =
            Channel::new(|_| Err(std::io::Error::other("gone").into()));
        open_session_with(
            &registry,
            "t",
            token,
            sh_spec(dir.path(), "printf 'out'; sleep 30"),
            broken,
            || true,
            || Ok(()),
            || Ok(()),
            Box::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(800));
        assert_eq!(ran.load(Ordering::SeqCst), 0, "the program still runs");
        registry.lock().unwrap().kill("t");
    }

    // Ticket 14yq6g30jet: the saved session changed before the open knew it
    // had won, so a superseded, closed or failed open still changed the tab.
    #[cfg(unix)]
    #[test]
    fn the_session_is_saved_only_after_the_current_open_spawned() {
        let dir = crate::test_support::TempDir::new("pty_after_spawn");
        let registry = Mutex::new(PtyRegistry::new());
        let calls = std::cell::Cell::new(0);
        let saved = || {
            calls.set(calls.get() + 1);
            Ok(())
        };
        let idle = |dir: &std::path::Path| sh_spec(dir, "sleep 30");
        let go = |token, spec, saved: &dyn Fn() -> Result<(), String>| {
            open_session_with(
                &registry,
                "t",
                token,
                spec,
                Channel::new(|_| Ok(())),
                || true,
                || Ok(()),
                saved,
                Box::new(|| {}),
            )
        };
        // Superseded by a newer open of the same id.
        let old = begin_open(&registry, "t").unwrap();
        let _newer = begin_open(&registry, "t").unwrap();
        assert_eq!(go(old, idle(dir.path()), &saved), Ok(None));
        // Closed before the spawn.
        let token = begin_open(&registry, "t").unwrap();
        registry.lock().unwrap().kill("t");
        assert!(go(token, idle(dir.path()), &saved).is_err());
        // The program does not exist.
        let token = begin_open(&registry, "t").unwrap();
        let mut missing = idle(dir.path());
        missing.program = "/nonexistent/dcv2-no-such-program".into();
        assert!(go(token, missing, &saved).is_err());
        assert_eq!(calls.get(), 0, "no losing open saved its session");
        // The tab went away (still_wanted false): nothing started, nothing saved.
        let token = begin_open(&registry, "t").unwrap();
        let gone = open_session_with(
            &registry,
            "t",
            token,
            idle(dir.path()),
            Channel::new(|_| Ok(())),
            || false,
            || Ok(()),
            saved,
            Box::new(|| {}),
        );
        assert!(gone.is_err());
        assert_eq!(calls.get(), 0);
        // The winning open saves once, and a failed save does not stop it.
        let token = begin_open(&registry, "t").unwrap();
        let won = go(token, idle(dir.path()), &saved);
        assert!(matches!(won, Ok(Some(_))), "{won:?}");
        assert_eq!(calls.get(), 1);
        let token = begin_open(&registry, "t").unwrap();
        let failing = go(token, idle(dir.path()), &|| Err("disk full".into()));
        assert!(matches!(failing, Ok(Some(_))), "{failing:?}");
        registry.lock().unwrap().kill("t");
    }

    // Review: the live-session check of an adopted run's tab ran before the
    // old program was stopped, so a Restart always saw its own `claude`.
    // It runs after the wait now, and a refusal starts nothing.
    #[cfg(unix)]
    #[test]
    fn the_check_before_the_spawn_runs_once_the_old_program_is_gone() {
        let dir = crate::test_support::TempDir::new("pty_before_spawn");
        let registry = Mutex::new(PtyRegistry::new());
        let pid_file = dir.join("old.pid");
        let token = begin_open(&registry, "t").unwrap();
        open_session_as(
            &registry,
            "t",
            token,
            sh_spec(
                dir.path(),
                &format!("echo $$ > '{}'; sleep 30", pid_file.display()),
            ),
            Channel::new(|_| Ok(())),
            || true,
            || Ok(()),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !pid_file.exists() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        let old = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .to_string();
        let started = dir.join("started");
        let token = begin_open(&registry, "t").unwrap();
        let old_alive = std::cell::Cell::new(true);
        let opened = open_session_as(
            &registry,
            "t",
            token,
            sh_spec(dir.path(), &format!("touch '{}'", started.display())),
            Channel::new(|_| Ok(())),
            || true,
            || {
                // Gone, or a zombie waiting to be reaped (not running).
                let alive = std::process::Command::new("ps")
                    .args(["-o", "stat=", "-p", &old])
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .is_ok_and(|stat| !stat.is_empty() && !stat.starts_with('Z'));
                old_alive.set(alive);
                Err("refused".into())
            },
        );
        assert_eq!(opened.unwrap_err(), "refused");
        assert!(
            !old_alive.get(),
            "the old program was stopped before the check"
        );
        let reg = registry.lock().unwrap();
        assert!(reg.sessions.is_empty() && reg.opening.is_empty());
        drop(reg);
        thread::sleep(Duration::from_millis(200));
        assert!(!started.exists(), "nothing started");
    }

    // The tab (or its window) is checked under the registry lock right
    // before the spawn; gone means nothing starts.
    #[cfg(unix)]
    #[test]
    fn an_open_whose_tab_is_gone_by_the_spawn_starts_nothing() {
        let dir = crate::test_support::TempDir::new("pty_tab_gone");
        let registry = Mutex::new(PtyRegistry::new());
        let started = dir.join("started");
        let token = begin_open(&registry, "t").unwrap();
        let opened = open_session_as(
            &registry,
            "t",
            token,
            sh_spec(dir.path(), &format!("touch '{}'", started.display())),
            Channel::new(|_| Ok(())),
            || false,
            || Ok(()),
        );
        assert!(opened.unwrap_err().contains("closed"));
        let registry = registry.lock().unwrap();
        assert!(registry.sessions.is_empty() && registry.opening.is_empty());
        thread::sleep(Duration::from_millis(200));
        assert!(!started.exists(), "nothing started");
    }

    // Review: close_tab killed the terminal first and edited the store
    // second, so an open between the two saw the tab still there and
    // spawned a terminal nobody would close.
    #[cfg(unix)]
    #[test]
    fn closing_a_tab_while_its_terminal_starts_leaves_nothing_running() {
        use crate::store::{StateStore, TabKind};
        let dir = crate::test_support::TempDir::new("pty_close_tab_race");
        let mut state = StateStore::open_path(dir.join("state.json")).unwrap();
        let tab = state
            .create_tab(&dir.path().display().to_string(), TabKind::Shell)
            .unwrap();
        let store = Mutex::new(state);
        let registry = Mutex::new(PtyRegistry::new());
        // An old program that takes a while to stop holds the open in its wait.
        open_slow_stopper(&registry, dir.path(), &tab);
        let started = dir.join("started");
        thread::scope(|scope| {
            let open = scope.spawn(|| {
                let token = begin_open(&registry, &tab).unwrap();
                open_session_as(
                    &registry,
                    &tab,
                    token,
                    sh_spec(
                        dir.path(),
                        &format!("touch '{}'; sleep 5", started.display()),
                    ),
                    Channel::new(|_| Ok(())),
                    || store.lock().unwrap().tab_by_id(&tab).is_some(),
                    || Ok(()),
                )
            });
            thread::sleep(Duration::from_millis(100));
            crate::commands::close_tab_then_kill(&store, &registry, None, &tab).unwrap();
            assert!(open.join().unwrap().is_err(), "the open was cancelled");
        });
        assert!(store.lock().unwrap().tab_by_id(&tab).is_none());
        assert!(registry.lock().unwrap().sessions.is_empty());
        thread::sleep(Duration::from_millis(200));
        assert!(!started.exists(), "the new program never started");
    }

    // Review: two opens of the same id. The first stopped the old program
    // and waited; the second saw no old session and spawned while the old
    // program still ran, and the first then failed as "closed".
    #[cfg(unix)]
    #[test]
    fn a_second_open_of_the_same_terminal_also_waits_and_the_first_yields() {
        let dir = crate::test_support::TempDir::new("pty_reopen_twice");
        let registry = Mutex::new(PtyRegistry::new());
        let old = open_slow_stopper(&registry, dir.path(), "t");
        let (first, second) = thread::scope(|scope| {
            let first = scope.spawn(|| {
                open_session(
                    &registry,
                    "t",
                    sh_spec(dir.path(), "sleep 5"),
                    Channel::new(|_| Ok(())),
                )
            });
            thread::sleep(Duration::from_millis(100));
            let second = scope.spawn(|| {
                let opened = open_session(
                    &registry,
                    "t",
                    sh_spec(dir.path(), "sleep 5"),
                    Channel::new(|_| Ok(())),
                );
                (opened, old.running())
            });
            (first.join().unwrap(), second.join().unwrap())
        });
        let (second, old_running) = second;
        assert!(
            !old_running,
            "the second open also waited for the old program"
        );
        assert!(
            matches!(second, Ok(Some(_))),
            "the newer open runs: {second:?}"
        );
        assert_eq!(first, Ok(None), "the older open yields quietly");
        let registry = registry.lock().unwrap();
        assert_eq!(
            registry.sessions["t"].pid(),
            second.unwrap().unwrap(),
            "the newer open's terminal is the one kept"
        );
    }

    // Review: an open left its `opening` entry behind, and a kill left the
    // replaced program in `stopping`.
    #[cfg(unix)]
    #[test]
    fn opens_and_kills_leave_no_bookkeeping_behind() {
        let dir = crate::test_support::TempDir::new("pty_bookkeeping");
        let registry = Mutex::new(PtyRegistry::new());
        let empty = |registry: &Mutex<PtyRegistry>| {
            let registry = registry.lock().unwrap();
            registry.opening.is_empty() && registry.stopping.is_empty()
        };
        let old = open_slow_stopper(&registry, dir.path(), "t");
        assert!(empty(&registry), "nothing left after an open");
        thread::scope(|scope| {
            let reopen = scope.spawn(|| {
                open_session(
                    &registry,
                    "t",
                    sh_spec(dir.path(), "sleep 5"),
                    Channel::new(|_| Ok(())),
                )
            });
            thread::sleep(Duration::from_millis(100));
            assert!(
                !registry.lock().unwrap().stopping.is_empty(),
                "old program is stopping"
            );
            registry.lock().unwrap().kill("t");
            assert!(
                registry.lock().unwrap().opening.is_empty(),
                "no open left after a kill"
            );
            assert!(reopen.join().unwrap().is_err());
        });
        // The closed program is kept only while it is still stopping.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while old.running() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        registry.lock().unwrap().kill("nothing");
        assert!(empty(&registry), "nothing left once it has stopped");
        open_slow_stopper(&registry, dir.path(), "u");
        open_session(
            &registry,
            "u",
            sh_spec(dir.path(), "sleep 5"),
            Channel::new(|_| Ok(())),
        )
        .expect("reopen");
        assert!(empty(&registry), "nothing left after a reopen");
    }

    // Quit asks first when a terminal is still running; one whose program
    // ended, or that was closed, is not counted.
    #[cfg(unix)]
    #[test]
    fn running_terminals_count_as_live_work_for_quit() {
        let dir = crate::test_support::TempDir::new("pty_live_count");
        let registry = Mutex::new(PtyRegistry::new());
        assert_eq!(registry.lock().unwrap().live_count(), 0);
        open_slow_stopper(&registry, dir.path(), "a");
        open_slow_stopper(&registry, dir.path(), "b");
        assert_eq!(registry.lock().unwrap().live_count(), 2);
        assert_eq!(
            registry
                .lock()
                .unwrap()
                .live_count_of(&["a".into(), "gone".into()]),
            1,
            "one window's terminals only"
        );
        registry.lock().unwrap().kill("a");
        assert_eq!(
            registry.lock().unwrap().live_count(),
            1,
            "a closed terminal is not live"
        );
        assert_eq!(registry.lock().unwrap().live_count_of(&["a".into()]), 0);
        open_session(
            &registry,
            "c",
            sh_spec(dir.path(), "exit 0"),
            Channel::new(|_| Ok(())),
        )
        .expect("open short terminal");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while registry.lock().unwrap().live_count() != 1 && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            registry.lock().unwrap().live_count(),
            1,
            "an ended program is not live"
        );
        registry.lock().unwrap().kill("b");
    }

    // Review: quit walked only live sessions, so a replaced program still
    // stopping was neither waited on nor killed.
    #[cfg(unix)]
    #[test]
    fn quitting_also_stops_a_replaced_program_that_is_still_stopping() {
        let dir = crate::test_support::TempDir::new("pty_quit_stopping");
        let registry = Mutex::new(PtyRegistry::new());
        let old = open_slow_stopper(&registry, dir.path(), "t");
        thread::scope(|scope| {
            let reopen = scope.spawn(|| {
                open_session(
                    &registry,
                    "t",
                    sh_spec(dir.path(), "sleep 5"),
                    Channel::new(|_| Ok(())),
                )
            });
            thread::sleep(Duration::from_millis(100));
            assert!(old.running(), "the old program is still finishing");
            let groups = registry.lock().unwrap().running_groups();
            assert_eq!(groups.len(), 1, "quit waits on the stopping program");
            registry.lock().unwrap().kill("t");
            let _ = reopen.join();
        });
    }

    // Regression: a resize waited up to 2 s for a busy terminal's ack while
    // holding the registry lock, so every other terminal's input and resize
    // (and pty_open, pty_kill) waited too.
    #[test]
    fn a_resize_on_a_stuck_terminal_does_not_hold_the_registry() {
        let registry = Mutex::new(PtyRegistry::new());
        let (stuck, resizes) = PtySession::stalled_for_test();
        registry
            .lock()
            .unwrap()
            .sessions
            .insert("stuck".into(), stuck);
        thread::scope(|scope| {
            let resize = scope.spawn(|| {
                let start = std::time::Instant::now();
                let result = resize_terminal(&registry, "stuck", 100, 30);
                (result, start.elapsed())
            });
            let arrived = resizes.recv_timeout(Duration::from_secs(1));
            assert_eq!(arrived, Ok((100, 30)), "the resize reached the terminal");
            let free = registry.try_lock().is_ok();
            let (result, took) = resize.join().unwrap();
            assert!(free, "registry is free while the terminal is busy");
            assert_eq!(result, Ok(()));
            assert!(took < Duration::from_millis(200), "resize took {took:?}");
        });
    }
}
