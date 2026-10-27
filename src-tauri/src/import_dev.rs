//! Import from the dev app (plan 2026-10-23 "dev → built app"). A release
//! build keeps its files in `<app data>`, a debug build (`tauri dev`) in
//! `<app data>.dev`. The built app picks up the dev app's session:
//!
//! - **Tabs** (open and recently closed) with their Claude session, cwd,
//!   config folder and launch, and the **windows** with their accounts
//!   (the dev `settings.json` accounts, matched to this app's by config
//!   folder, else added). Each dev window comes in as a closed window
//!   (File › Reopen Closed Window); the import reopens them at once.
//! - **Runs are adopted in place**: `<app data>/adopted-runs.json` lists the
//!   dev run folders, whose conversation, `DCV2_RUN_DIR`, status.json and
//!   `.guard.json` all name that path; their signed records stay in the dev
//!   folder's `run-trust` (the guard's `<run>/../../run-trust`).
//! - **Launch args are re-pointed**: `--plugin-dir` → this app's plugin,
//!   `--settings` → this plugin's launch settings, the memory-prompt file →
//!   made again by this app, `DCV2_MEMORY_DIR` → this app's memory;
//!   `DCV2_RUN_DIR` stays the original path.
//! - **Memory** is copied when this app has none yet.
//!
//! Nothing in `.dev` is ever changed or deleted. The keychain items (ClickUp
//! tokens, the trust key) have the same service names in both builds, so
//! they carry over; macOS asks once to let the built app use them.

use crate::provider::claude_config::resolve_with;
use crate::store::settings_store::SettingsFile;
use crate::store::{
    AppStateFile, ArchivedWindow, ClaudeSettings, TabLaunch, TabRecord, WindowRecord,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// `<app data>/dev-import.json`: offered once, and when it was imported.
pub const IMPORT_FILE: &str = "dev-import.json";
const MAX_ACCOUNTS: usize = 8;

/// The dev app's data folder beside `data` (`<name>.dev`).
pub fn dev_dir_for(data: &Path) -> PathBuf {
    let mut name = data.file_name().unwrap_or_default().to_os_string();
    name.push(".dev");
    data.with_file_name(name)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImportRecord {
    #[serde(default)]
    pub offered: bool,
    #[serde(default)]
    pub imported_at: Option<String>,
}

pub fn read_record(data: &Path) -> ImportRecord {
    crate::run::folder::read_json_value(&data.join(IMPORT_FILE))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

pub fn write_record(data: &Path, record: &ImportRecord) -> Result<(), String> {
    crate::store::write_json_atomic(&data.join(IMPORT_FILE), record)
}

/// How a launch is re-pointed at this app.
pub struct Relaunch<'a> {
    pub plugin_dir: PathBuf,
    /// This plugin's `launch/settings.json`, if any.
    pub settings_file: Option<PathBuf>,
    /// This app's `<data>/memory`.
    pub memory_dir: PathBuf,
    /// This app's `<data>/launch` (new memory-prompt files go here, never
    /// into the dev run folder).
    pub launch_dir: PathBuf,
    /// `memory.sh compose <folder> <out>`: the file written, or `None`.
    pub compose: &'a dyn Fn(&Path, &Path) -> Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    /// Labels of the windows made (closed windows, to reopen).
    pub windows: Vec<String>,
    pub tabs: usize,
    pub closed_tabs: usize,
    pub runs: usize,
    pub accounts_added: Vec<String>,
    pub memory_copied: bool,
    pub notes: Vec<String>,
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Whether there is a dev app session to import.
pub fn available(dev: &Path) -> bool {
    dev.join("state.json").is_file()
}

fn same_dir(a: &str, b: &str) -> bool {
    crate::provider::claude::same_config_dir(a, b)
}

/// The launch, pointed at this app (see the module notes).
pub fn repoint(launch: &TabLaunch, folder: &Path, re: &Relaunch) -> TabLaunch {
    let mut args = Vec::new();
    let mut iter = launch.args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--plugin-dir" => {
                iter.next();
                args.push(arg.clone());
                args.push(re.plugin_dir.display().to_string());
            }
            "--append-system-prompt-file" => {
                iter.next();
                let out = re
                    .launch_dir
                    .join(format!("{}-memory-prompt.md", uuid::Uuid::new_v4()));
                if let Some(file) = (re.compose)(folder, &out) {
                    args.push(arg.clone());
                    args.push(file.display().to_string());
                }
            }
            _ => args.push(arg.clone()),
        }
    }
    let env = launch
        .env
        .iter()
        .filter(|(k, _)| k != crate::run::launch::TEST_MODE_ENV)
        .map(|(k, v)| {
            if k == "DCV2_MEMORY_DIR" {
                (k.clone(), re.memory_dir.display().to_string())
            } else {
                (k.clone(), v.clone())
            }
        })
        .collect();
    TabLaunch {
        role: launch.role.clone(),
        prompt: launch.prompt.clone(),
        env,
        args,
        settings_file: launch
            .settings_file
            .as_ref()
            .and_then(|_| re.settings_file.as_ref().map(|f| f.display().to_string())),
        run_dir: launch.run_dir.clone(),
    }
}

/// A free `win-N` label in `state` (and not in `extra`).
fn free_label(state: &AppStateFile, extra: &HashSet<String>) -> String {
    let taken = |label: &str| {
        extra.contains(label)
            || state.windows.iter().any(|w| w.id == label)
            || state.archived_windows.iter().any(|a| a.window.id == label)
            || state.closed_tabs.iter().any(|t| t.window_id == label)
            || state.tabs.iter().any(|t| t.window_id == label)
    };
    (2u32..)
        .map(|n| format!("win-{n}"))
        .find(|l| !taken(l))
        .unwrap_or_else(|| "win-x".into())
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<usize> {
    let mut count = 0;
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)?.flatten() {
        let kind = entry.file_type()?;
        let dest = to.join(entry.file_name());
        if kind.is_dir() {
            count += copy_tree(&entry.path(), &dest)?;
        } else if kind.is_file() && !dest.exists() {
            std::fs::copy(entry.path(), &dest)?;
            count += 1;
        }
    }
    Ok(count)
}

fn has_entries(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut e| e.next().is_some())
}

/// Imports the dev app's session from `dev` into `built` (the stores'
/// data, changed in place; the caller saves them). Reads `dev` only.
pub fn import(
    dev: &Path,
    built: &Path,
    state: &mut AppStateFile,
    claude: &mut ClaudeSettings,
    home: Option<&Path>,
    re: &Relaunch,
    now: &str,
) -> Result<ImportSummary, String> {
    let dev_state: AppStateFile =
        read_json(&dev.join("state.json"))?.ok_or("The dev app has no saved tabs.")?;
    let dev_settings: SettingsFile = read_json(&dev.join("settings.json"))?.unwrap_or_default();
    let mut summary = ImportSummary::default();

    // Accounts: by config folder; one the built app lacks is added.
    let path_of = |dir: Option<&str>| resolve_with(None, dir, home).path;
    let mut accounts: HashMap<String, String> = HashMap::new();
    for account in &dev_settings.claude.accounts {
        let want = path_of(account.config_dir.as_deref());
        if let Some(mine) = claude
            .accounts
            .iter()
            .find(|a| same_dir(&path_of(a.config_dir.as_deref()), &want))
        {
            accounts.insert(account.id.clone(), mine.id.clone());
            continue;
        }
        if claude.accounts.len() >= MAX_ACCOUNTS {
            summary.notes.push(format!(
                "Account {} was not added (at most {MAX_ACCOUNTS}); its windows are skipped.",
                account.name
            ));
            continue;
        }
        let mut id = account.id.clone();
        let mut n = 2;
        while claude.accounts.iter().any(|a| a.id == id) {
            id = format!("{}-{n}", account.id);
            n += 1;
        }
        claude
            .accounts
            .push(crate::store::settings_store::ClaudeAccount {
                id: id.clone(),
                name: account.name.clone(),
                config_dir: account.config_dir.clone(),
            });
        summary.accounts_added.push(account.name.clone());
        accounts.insert(account.id.clone(), id);
    }
    // The accounts must be valid as a whole before anything else is
    // touched: a refusal later would leave adopted runs and copied memory
    // behind for an import that never finished.
    let mut checked = claude.clone();
    crate::store::settings_store::normalize_claude_accounts(&mut checked)
        .map_err(|err| format!("The dev app's accounts cannot be imported: {err}"))?;
    // A dev window that names no known account used the first one; a
    // window whose own account was not brought over is skipped.
    let first_dev = dev_settings.claude.accounts.first().map(|a| a.id.clone());
    let dev_ids: HashSet<&str> = dev_settings
        .claude
        .accounts
        .iter()
        .map(|a| a.id.as_str())
        .collect();
    let account_for = |id: &str| {
        if dev_ids.contains(id) {
            return accounts.get(id).cloned();
        }
        first_dev.as_ref().and_then(|f| accounts.get(f)).cloned()
    };

    // Tabs already here (an earlier import) are not added twice.
    let known_sessions: HashSet<String> = state
        .tabs
        .iter()
        .chain(state.closed_tabs.iter())
        .filter_map(|t| t.claude_session_id.clone())
        .collect();
    let mut known_ids: HashSet<String> = state
        .tabs
        .iter()
        .chain(state.closed_tabs.iter())
        .map(|t| t.id.clone())
        .collect();

    let folder_of = |tab: &TabRecord| -> PathBuf {
        tab.launch
            .as_ref()
            .and_then(|l| l.run_dir.as_deref())
            .and_then(|r| crate::run::folder::run_folder_of(Path::new(r)))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(&tab.cwd))
    };
    let mut bring = |tab: &TabRecord, window: &str| -> Option<TabRecord> {
        if tab
            .claude_session_id
            .as_ref()
            .is_some_and(|s| known_sessions.contains(s))
        {
            return None;
        }
        let mut next = tab.clone();
        if known_ids.contains(&next.id) {
            next.id = format!("tab_{}", uuid::Uuid::new_v4().simple());
        }
        known_ids.insert(next.id.clone());
        next.window_id = window.to_string();
        next.closed_at = Some(tab.closed_at.clone().unwrap_or_else(|| now.to_string()));
        next.launch = tab.launch.as_ref().map(|l| repoint(l, &folder_of(tab), re));
        Some(next)
    };

    let dev_windows: Vec<WindowRecord> = dev_state
        .windows
        .iter()
        .cloned()
        .chain(dev_state.archived_windows.iter().map(|a| a.window.clone()))
        .collect();
    let mut labels: HashMap<String, String> = HashMap::new();
    let mut made: HashSet<String> = HashSet::new();
    let mut archived: Vec<ArchivedWindow> = Vec::new();
    let mut closed: Vec<TabRecord> = Vec::new();
    for window in &dev_windows {
        let Some(account_id) = account_for(&window.account_id) else {
            continue;
        };
        let label = free_label(state, &made);
        made.insert(label.clone());
        labels.insert(window.id.clone(), label.clone());
        let mut open: Vec<&TabRecord> = dev_state
            .tabs
            .iter()
            .filter(|t| crate::store::window_matches(&t.window_id, &window.id))
            .collect();
        open.sort_by_key(|t| t.order);
        let mut tab_ids = Vec::new();
        let mut active = None;
        for tab in open {
            if let Some(next) = bring(tab, &label) {
                if window.active_tab_id.as_deref() == Some(tab.id.as_str()) {
                    active = Some(next.id.clone());
                }
                tab_ids.push(next.id.clone());
                closed.push(next);
                summary.tabs += 1;
            }
        }
        // Tabs of a dev window closed earlier wait in closed_tabs there.
        if let Some(arch) = dev_state
            .archived_windows
            .iter()
            .find(|a| a.window.id == window.id)
        {
            for id in &arch.tab_ids {
                if let Some(tab) = dev_state.closed_tabs.iter().find(|t| &t.id == id) {
                    if let Some(next) = bring(tab, &label) {
                        tab_ids.push(next.id.clone());
                        closed.push(next);
                        summary.tabs += 1;
                    }
                }
            }
        }
        archived.push(ArchivedWindow {
            window: WindowRecord {
                id: label.clone(),
                account_id,
                active_tab_id: active,
            },
            tab_ids,
            closed_at: now.to_string(),
        });
    }
    // Recently closed tabs (not of a closed window): Reopen Closed Tab there.
    let in_archived: HashSet<&String> = dev_state
        .archived_windows
        .iter()
        .flat_map(|a| a.tab_ids.iter())
        .collect();
    for tab in &dev_state.closed_tabs {
        if in_archived.contains(&tab.id) {
            continue;
        }
        let Some(label) = dev_windows
            .iter()
            .find(|w| crate::store::window_matches(&tab.window_id, &w.id))
            .and_then(|w| labels.get(&w.id))
        else {
            continue;
        };
        if let Some(next) = bring(tab, label) {
            closed.push(next);
            summary.closed_tabs += 1;
        }
    }
    // Windows with nothing new (imported before) are not made again.
    archived.retain(|a| !a.tab_ids.is_empty());
    let keep: HashSet<String> = archived.iter().map(|a| a.window.id.clone()).collect();
    closed.retain(|t| keep.contains(&t.window_id));
    summary.windows = archived.iter().map(|a| a.window.id.clone()).collect();
    // Reopen Closed Window takes the first: the import's come first.
    for (at, window) in archived.into_iter().enumerate() {
        state.archived_windows.insert(at, window);
    }
    for (at, tab) in closed.into_iter().enumerate() {
        state.closed_tabs.insert(at, tab);
    }

    // Runs, in place.
    let runs: Vec<PathBuf> = std::fs::read_dir(dev.join("runs"))
        .map(|e| {
            e.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| !n.starts_with('.'))
                })
                .collect()
        })
        .unwrap_or_default();
    summary.runs = crate::run::folder::adopt_runs(built, &runs)?;

    // Memory, when this app has none yet (onboarding is not redone).
    let (from, to) = (dev.join("memory"), built.join("memory"));
    if from.is_dir() && !has_entries(&to) {
        copy_tree(&from, &to).map_err(|e| format!("memory: {e}"))?;
        summary.memory_copied = true;
    }
    Ok(summary)
}

// ---- Is the dev app running? ---------------------------------------------------------------

/// `<app data>/app.lock`: flock'd for the app's whole life (pid inside), so
/// another build can tell this one is running.
pub const APP_LOCK: &str = "app.lock";
/// A session log written this recently counts as live.
pub const LIVE_WINDOW: std::time::Duration = std::time::Duration::from_secs(120);
pub const QUIT_DEV_FIRST: &str = "Quit the dev app first";

/// What taking this app's lock came to.
#[derive(Debug)]
pub enum AppLock {
    /// Ours; keep the file open until exit.
    Acquired(std::fs::File),
    /// Another process runs on this data folder.
    HeldElsewhere,
    /// The lock file could not be created, opened or locked (not a running copy).
    Unavailable(String),
}

/// Takes this app's lock at startup.
pub fn hold_app_lock(data: &Path) -> AppLock {
    use std::io::Write;
    if let Err(err) = std::fs::create_dir_all(data) {
        return AppLock::Unavailable(format!("{}: {err}", data.display()));
    }
    let path = data.join(APP_LOCK);
    let mut file = match std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(err) => return AppLock::Unavailable(format!("{}: {err}", path.display())),
    };
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return AppLock::HeldElsewhere,
        Err(std::fs::TryLockError::Error(err)) => {
            return AppLock::Unavailable(format!("{}: {err}", path.display()))
        }
    }
    let _ = file.set_len(0);
    let _ = writeln!(file, "{}", std::process::id());
    AppLock::Acquired(file)
}

/// Another process holds `<dir>/app.lock`. Opened read-only: nothing in
/// `dir` is changed.
pub fn lock_held(dir: &Path) -> bool {
    let Ok(file) = std::fs::File::open(dir.join(APP_LOCK)) else {
        return false;
    };
    matches!(
        file.try_lock_shared(),
        Err(std::fs::TryLockError::WouldBlock)
    )
}

/// Every process as `pid ppid command` (`ps`), or empty when ps fails.
pub fn process_lines() -> String {
    std::process::Command::new("ps")
        .args(["-axww", "-o", "pid=,ppid=,command="])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// `pid ppid command` lines.
fn parse_ps(ps: &str) -> Vec<(u32, u32, &str)> {
    ps.lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, rest) = line.split_once(char::is_whitespace)?;
            let rest = rest.trim_start();
            let (ppid, command) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            Some((pid.parse().ok()?, ppid.parse().ok()?, command))
        })
        .collect()
}

/// A `claude` process outside `own_pid`'s process tree runs session `id`
/// (`--resume <id>` / `--session-id <id>`). This app's own terminals (its
/// children) never count.
pub fn session_in_use(ps: &str, id: &str, own_pid: u32) -> bool {
    let id = id.trim();
    if id.is_empty() {
        return false;
    }
    let procs = parse_ps(ps);
    let parent: HashMap<u32, u32> = procs.iter().map(|(p, pp, _)| (*p, *pp)).collect();
    let ours = |mut pid: u32| {
        for _ in 0..64 {
            if pid == own_pid {
                return true;
            }
            match parent.get(&pid) {
                Some(&pp) if pp != pid && pp != 0 => pid = pp,
                _ => return false,
            }
        }
        false
    };
    procs.iter().any(|(pid, _, command)| {
        command.contains("claude")
            && command
                .split_whitespace()
                .collect::<Vec<_>>()
                .windows(2)
                .any(|w| (w[0] == "--resume" || w[0] == "--session-id") && w[1] == id)
            && !ours(*pid)
    })
}

/// Why the dev app looks live, if it does: its app lock is held, or a tab's
/// session log was written in the last 2 min, or a `claude` runs its session.
pub fn dev_live_reason(dev: &Path, ps: &str, now: std::time::SystemTime) -> Option<String> {
    if lock_held(dev) {
        return Some(format!("{QUIT_DEV_FIRST}: it is running."));
    }
    let state: AppStateFile = read_json(&dev.join("state.json")).ok().flatten()?;
    for tab in state.tabs.iter().chain(state.closed_tabs.iter()) {
        let Some(id) = tab.claude_session_id.as_deref() else {
            continue;
        };
        if session_in_use(ps, id, std::process::id()) {
            return Some(format!(
                "{QUIT_DEV_FIRST}: a Claude session of tab \"{}\" is still running.",
                tab.label
            ));
        }
        let fresh = tab
            .claude_config_dir
            .as_deref()
            .and_then(|c| crate::claude_session_log::find_session_log(Path::new(c), id))
            .and_then(|log| std::fs::metadata(log).and_then(|m| m.modified()).ok())
            .and_then(|at| now.duration_since(at).ok())
            .is_some_and(|age| age < LIVE_WINDOW);
        if fresh {
            return Some(format!(
                "{QUIT_DEV_FIRST}: tab \"{}\" wrote to its conversation in the last 2 minutes.",
                tab.label
            ));
        }
    }
    None
}

/// Before resuming an adopted run's tab: refused while another process runs
/// that session (the dev app still has it open).
pub fn refuse_live_session(session_id: &str) -> Result<(), String> {
    if session_in_use(&process_lines(), session_id, std::process::id()) {
        return Err("Another Claude process is using this conversation (is the dev app still running it?). Quit it first, then reopen the tab.".into());
    }
    Ok(())
}

// ---- Commands ------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportStatus {
    /// A release build with a dev app session beside it.
    pub available: bool,
    /// The first-launch offer was answered (or the import done).
    pub offered: bool,
    pub imported_at: Option<String>,
    pub dev_dir: String,
}

fn data_dir(app: &tauri::AppHandle) -> PathBuf {
    crate::data_dir::app_data_dir(app).path
}

#[tauri::command]
pub fn dev_import_status(app: tauri::AppHandle) -> ImportStatus {
    let data = data_dir(&app);
    let dev = dev_dir_for(&data);
    let record = read_record(&data);
    ImportStatus {
        available: !cfg!(debug_assertions) && available(&dev),
        offered: record.offered,
        imported_at: record.imported_at,
        dev_dir: dev.display().to_string(),
    }
}

/// "Not now" on the first-launch offer: it is not shown again (File ›
/// Import from dev app… still works).
#[tauri::command]
pub fn dev_import_dismiss(app: tauri::AppHandle) -> Result<(), String> {
    let data = data_dir(&app);
    let mut record = read_record(&data);
    record.offered = true;
    write_record(&data, &record)
}

/// Imports the dev app's tabs, windows, runs and memory. The windows come
/// in as closed windows (File › Reopen Closed Window); nothing reopens on
/// its own. Refused while the dev app looks live.
#[tauri::command]
pub async fn import_from_dev_app(app: tauri::AppHandle) -> Result<ImportSummary, String> {
    use std::sync::Mutex;
    use tauri::Manager;
    if cfg!(debug_assertions) {
        return Err("Import from the dev app is for the built app only.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let data = data_dir(&app);
        let dev = dev_dir_for(&data);
        if !available(&dev) {
            return Err(format!("No dev app data at {}.", dev.display()));
        }
        if let Some(reason) = dev_live_reason(&dev, &process_lines(), std::time::SystemTime::now())
        {
            return Err(reason);
        }
        let plugin = crate::run::plugin_dir_for(&app)
            .ok_or("The dcv2 plugin folder was not found (reinstall the app).")?;
        let memory_dir = crate::run::memory::memory_dir(&data);
        let compose = |folder: &Path, out: &Path| {
            crate::run::memory::compose(&plugin, &memory_dir, folder, out)
        };
        let re = Relaunch {
            plugin_dir: plugin.clone(),
            settings_file: crate::run::launch::launch_settings_file(&plugin),
            memory_dir: memory_dir.clone(),
            launch_dir: crate::run::launch_prompts_dir(&data),
            compose: &compose,
        };
        let mut claude = {
            let settings = app.state::<Mutex<crate::store::SettingsStore>>();
            let settings = settings.lock().map_err(|e| e.to_string())?;
            settings.claude().clone()
        };
        let accounts_before = claude.accounts.clone();
        let before = {
            let state = app.state::<Mutex<crate::store::StateStore>>();
            let state = state.lock().map_err(|e| e.to_string())?;
            state.data.clone()
        };
        let home = crate::provider::claude_config::home_dir();
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        // No lock held while this runs (memory.sh compose is slow).
        let mut after = before.clone();
        let summary = import(
            &dev,
            &data,
            &mut after,
            &mut claude,
            home.as_deref(),
            &re,
            &now,
        )?;
        // Settings first: the imported windows name its accounts. The
        // accounts the import added go into the settings as they are now
        // (not the copy taken before the slow part), so a change made
        // meanwhile survives. A clash is refused before the state changes.
        {
            let added: Vec<_> = claude
                .accounts
                .iter()
                .filter(|a| !accounts_before.iter().any(|b| b.id == a.id))
                .cloned()
                .collect();
            let settings = app.state::<Mutex<crate::store::SettingsStore>>();
            let mut settings = settings.lock().map_err(|e| e.to_string())?;
            let merged = merge_added_accounts(settings.claude(), &added, home.as_deref())?;
            settings.set_claude(merged)?;
        }
        {
            let state = app.state::<Mutex<crate::store::StateStore>>();
            let mut state = state.lock().map_err(|e| e.to_string())?;
            apply_delta(&before, &after, &summary, &mut state.data)?;
            state.save()?;
        }
        write_record(
            &data,
            &ImportRecord {
                offered: true,
                imported_at: Some(now),
            },
        )?;
        crate::windows::refresh_reopen_window_item(&app);
        Ok(summary)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// `current` Claude settings plus the accounts the import `added`. One
/// already there (same id and folder) is kept as is. An id now taken by
/// another folder, or a folder now under another id, means the accounts
/// changed during the import: refused, so nothing is overwritten.
pub fn merge_added_accounts(
    current: &ClaudeSettings,
    added: &[crate::store::settings_store::ClaudeAccount],
    home: Option<&Path>,
) -> Result<ClaudeSettings, String> {
    let path_of = |dir: Option<&str>| resolve_with(None, dir, home).path;
    let mut merged = current.clone();
    for account in added {
        let folder = path_of(account.config_dir.as_deref());
        let same_id = merged.accounts.iter().find(|a| a.id == account.id);
        let same_folder = merged
            .accounts
            .iter()
            .find(|a| same_dir(&path_of(a.config_dir.as_deref()), &folder));
        match (same_id, same_folder) {
            (Some(a), Some(b)) if a.id == b.id => {}
            (None, None) => merged.accounts.push(account.clone()),
            _ => return Err(
                "Settings changed during the import (an account was added or changed); try again."
                    .into(),
            ),
        }
    }
    Ok(merged)
}

/// Adds what the import added to `before` (its windows and closed tabs) to
/// the state as it is now, which may have moved on meanwhile.
pub fn apply_delta(
    before: &AppStateFile,
    after: &AppStateFile,
    summary: &ImportSummary,
    now: &mut AppStateFile,
) -> Result<(), String> {
    let old_tabs: HashSet<&String> = before
        .tabs
        .iter()
        .chain(before.closed_tabs.iter())
        .map(|t| &t.id)
        .collect();
    let windows: Vec<&ArchivedWindow> = after
        .archived_windows
        .iter()
        .filter(|a| summary.windows.contains(&a.window.id))
        .collect();
    let tabs: Vec<&TabRecord> = after
        .closed_tabs
        .iter()
        .filter(|t| !old_tabs.contains(&t.id))
        .collect();
    let clash = windows.iter().any(|a| {
        now.windows.iter().any(|w| w.id == a.window.id)
            || now
                .archived_windows
                .iter()
                .any(|w| w.window.id == a.window.id)
    }) || tabs.iter().any(|t| {
        now.tabs
            .iter()
            .chain(now.closed_tabs.iter())
            .any(|n| n.id == t.id)
    });
    if clash {
        return Err("Windows or tabs changed during the import. Import again.".into());
    }
    for (at, window) in windows.into_iter().enumerate() {
        now.archived_windows.insert(at, window.clone());
    }
    for (at, tab) in tabs.into_iter().enumerate() {
        now.closed_tabs.insert(at, tab.clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clickup::trust;
    use crate::provider::{TerminalKind, TerminalLaunch};
    use crate::store::{TabKind, TabLaunch};
    use crate::test_support::TempDir;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    const SID: &str = "685dd464-eabf-4e4c-9f6f-ac02c63aad27";
    const SID2: &str = "11111111-2222-4333-8444-555555555555";

    /// Every file under `dir`, with its sha-256 and modified time.
    fn snapshot(dir: &Path) -> Vec<(PathBuf, String, std::time::SystemTime)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let bytes = std::fs::read(&p).unwrap();
                    let sha = Sha256::digest(&bytes)
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    out.push((p.clone(), sha, e.metadata().unwrap().modified().unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    fn tab(
        id: &str,
        window: &str,
        cwd: &str,
        sid: &str,
        config: &Path,
        launch: Option<TabLaunch>,
    ) -> serde_json::Value {
        let mut t = serde_json::to_value(TabRecord {
            id: id.into(),
            label: "Eagle-Eye · ws".into(),
            custom_label: true,
            cwd: cwd.into(),
            kind: TabKind::Claude,
            window_id: window.into(),
            order: 1,
            created_at: "2026-10-23T09:00:00Z".into(),
            claude_session_id: Some(sid.into()),
            claude_config_dir: Some(config.display().to_string()),
            closed_at: None,
            launch,
            split: None,
        })
        .unwrap();
        t["extraFieldFromNewerBuild"] = json!(1);
        t
    }

    /// A sanitised `.dev` folder: a window on a work account with a
    /// mid-run Eagle-Eye tab, a recently closed plain tab, the run folder
    /// (status.json, .guard.json, run-trust signed with the test key) and memory.
    fn fixture(root: &Path) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let dev = root.join("app.dev");
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("mathlib/.git")).unwrap();
        let config = root.join("claude-work");
        std::fs::create_dir_all(config.join("projects/-ws")).unwrap();
        std::fs::write(
            config.join(format!("projects/-ws/{SID}.jsonl")),
            "{\"type\":\"user\"}\n",
        )
        .unwrap();
        let run = dev.join("runs/r-20261023-090000-abcd");
        std::fs::create_dir_all(run.join("gates")).unwrap();
        std::fs::write(
            run.join("request.md"),
            format!(
                "# Request\n\nfolder: {}\n\n## What to do\n\nAdd multiply()\n",
                ws.display()
            ),
        )
        .unwrap();
        std::fs::write(
            run.join("meta.json"),
            json!({"schema": 1, "folder": ws.display().to_string(), "account": {"id": "work"}})
                .to_string(),
        )
        .unwrap();
        std::fs::write(
            run.join("status.json"),
            json!({"schema": 1, "stage": "building", "prs": [{"id": "PR1", "state": "building"}]})
                .to_string(),
        )
        .unwrap();
        std::fs::write(run.join(".guard.json"), json!({"schema": 1, "worktrees": [], "repos": [ws.join("mathlib").display().to_string()]}).to_string()).unwrap();
        std::fs::write(run.join("memory-prompt.md"), "old prompt").unwrap();
        trust::write_start(
            &trust::tests::key(),
            &dev,
            &run,
            "work",
            &trust::tests::snap(),
        )
        .unwrap();
        std::fs::create_dir_all(dev.join("memory/github.com__o__mathlib")).unwrap();
        std::fs::write(
            dev.join("memory/github.com__o__mathlib/index.md"),
            "# mathlib\n",
        )
        .unwrap();
        let old_plugin = "/old/repo/plugin";
        let launch = TabLaunch {
            role: "eagle-eye".into(),
            prompt: format!("/dcv2:eagle-eye run={}", run.display()),
            env: vec![
                (
                    "DCV2_MEMORY_DIR".into(),
                    dev.join("memory").display().to_string(),
                ),
                ("DCV2_RUN_DIR".into(), run.display().to_string()),
                ("DCV2_MAX_IMPLEMENTERS".into(), "3".into()),
                ("DCV2_STOP_HOOKS".into(), "on".into()),
            ],
            args: vec![
                "--plugin-dir".into(),
                old_plugin.into(),
                "--append-system-prompt-file".into(),
                run.join("memory-prompt.md").display().to_string(),
            ],
            settings_file: Some(format!("{old_plugin}/launch/settings.json")),
            run_dir: Some(run.display().to_string()),
        };
        let mut closed = tab(
            "tab_old",
            "main",
            &ws.display().to_string(),
            SID2,
            &config,
            None,
        );
        closed["closedAt"] = json!("2026-10-23T08:00:00Z");
        std::fs::write(
            dev.join("state.json"),
            json!({
                "schemaVersion": 1,
                "tabs": [tab("tab_1", "main", &ws.display().to_string(), SID, &config, Some(launch))],
                "closedTabs": [closed],
                "windows": [{"id": "main", "accountId": "work", "activeTabId": "tab_1"}],
                "archivedWindows": []
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            dev.join("settings.json"),
            json!({"schemaVersion": 1, "claude": {"accounts": [
                {"id": "default", "name": "Personal", "configDir": root.join("claude-personal").display().to_string()},
                {"id": "work", "name": "Work", "configDir": config.display().to_string()}
            ]}})
            .to_string(),
        )
        .unwrap();
        (dev, run, config, ws)
    }

    #[test]
    fn the_dev_session_comes_back_with_the_exact_resume_command_and_an_adopted_trusted_run() {
        let root = TempDir::new("import_dev");
        let (dev, run, config, ws) = fixture(root.path());
        let before = snapshot(&dev);
        let built = root.join("app");
        std::fs::create_dir_all(&built).unwrap();
        let plugin = root.join("bundle/plugin");
        let mut state = AppStateFile::default();
        // The built app already has the personal account, under another id.
        let mut claude = ClaudeSettings {
            claude_path: None,
            accounts: vec![crate::store::settings_store::ClaudeAccount {
                id: "me".into(),
                name: "Me".into(),
                config_dir: Some(root.join("claude-personal").display().to_string()),
            }],
        };
        let composed = std::cell::RefCell::new(Vec::new());
        let compose = |folder: &Path, out: &Path| {
            composed.borrow_mut().push(folder.to_path_buf());
            std::fs::create_dir_all(out.parent().unwrap()).unwrap();
            std::fs::write(out, "new prompt").unwrap();
            Some(out.to_path_buf())
        };
        let re = Relaunch {
            plugin_dir: plugin.clone(),
            settings_file: Some(plugin.join("launch/settings.json")),
            memory_dir: built.join("memory"),
            launch_dir: built.join("launch"),
            compose: &compose,
        };
        let summary = import(
            &dev,
            &built,
            &mut state,
            &mut claude,
            None,
            &re,
            "2026-10-24T10:00:00Z",
        )
        .unwrap();
        assert_eq!(summary.tabs, 1);
        assert_eq!(summary.closed_tabs, 1);
        assert_eq!(summary.runs, 1);
        assert!(summary.memory_copied);
        assert_eq!(summary.accounts_added, ["Work"]);
        assert_eq!(
            composed.borrow().as_slice(),
            std::slice::from_ref(&ws),
            "the run's folder"
        );

        // The window comes back on the work account, first in line to reopen.
        let window = &state.archived_windows[0];
        assert_eq!(summary.windows, std::slice::from_ref(&window.window.id));
        assert_eq!(window.window.account_id, "work");
        assert_eq!(window.tab_ids, ["tab_1"]);
        let mut store = crate::store::StateStore::open_path(built.join("state.json")).unwrap();
        store.data = state.clone();
        let reopened = store.reopen_archived_window().unwrap().unwrap();
        assert_eq!(reopened.account_id, "work");
        let tab = store.tab_by_id("tab_1").unwrap().clone();
        assert_eq!(tab.cwd, ws.display().to_string());
        assert_eq!(
            tab.claude_config_dir.as_deref(),
            Some(config.to_str().unwrap())
        );

        // The resume command line and env, exactly.
        let account = crate::store::settings_store::account_config(&claude, "work", None, None);
        let resumable = crate::claude_session_log::resumable_session(
            tab.claude_session_id.as_deref(),
            tab.claude_config_dir.as_deref(),
            &account.path,
        );
        assert_eq!(resumable.as_deref(), Some(SID));
        let launch = tab.launch.as_ref().unwrap();
        let mut command = crate::provider::ProgramArgs::new(
            "claude",
            crate::provider::claude::claude_terminal_args(&TerminalLaunch {
                kind: TerminalKind::Resume {
                    session_id: SID.into(),
                },
                session_id: Some(SID.into()),
                settings: None,
            })
            .unwrap(),
        );
        let drop = crate::run::launch::apply_launch(&mut command, Some(launch), true);
        let prompt = built.join("launch");
        let prompt_file = std::fs::read_dir(&prompt)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            command.args,
            [
                "--resume".to_string(),
                SID.into(),
                "--plugin-dir".into(),
                plugin.display().to_string(),
                "--append-system-prompt-file".into(),
                prompt_file.display().to_string(),
            ]
        );
        assert_eq!(
            command.env,
            [
                (
                    "DCV2_MEMORY_DIR".to_string(),
                    built.join("memory").display().to_string()
                ),
                ("DCV2_RUN_DIR".into(), run.display().to_string()),
                ("DCV2_MAX_IMPLEMENTERS".into(), "3".into()),
                ("DCV2_STOP_HOOKS".into(), "on".into()),
            ]
        );
        assert_eq!(drop, ["DCV2_TEST_MODE"]);
        assert_eq!(
            launch.settings_file.as_deref(),
            Some(plugin.join("launch/settings.json").to_str().unwrap())
        );
        assert_eq!(launch.run_dir.as_deref(), Some(run.to_str().unwrap()));
        // A fresh start of the tab resumes the run (status.json exists).
        assert_eq!(
            crate::run::launch::fresh_prompt(launch),
            format!("/dcv2:eagle-eye resume run={}", run.display())
        );

        // The adopted run is a run of this app, and its trust check passes.
        let dir = crate::run::checked_run_dir(&built, run.to_str().unwrap()).unwrap();
        let tdata = crate::run::trust_data_dir(&built, &dir);
        assert_eq!(tdata, dev.canonicalize().unwrap());
        let trusted = trust::load(&trust::tests::key(), &tdata, &dir)
            .unwrap()
            .unwrap();
        assert_eq!(trusted.account_id, "work");
        assert!(crate::run::folder::list_runs(&built)
            .iter()
            .any(|r| r.adopted));
        assert_eq!(
            crate::run::status::read_run(&dir, false).stage.as_deref(),
            Some("building")
        );

        // Memory copied; nothing in .dev changed.
        assert!(built
            .join("memory/github.com__o__mathlib/index.md")
            .is_file());
        assert_eq!(snapshot(&dev), before);

        // Importing again adds nothing twice.
        let again = import(&dev, &built, &mut state, &mut claude, None, &re, "t").unwrap();
        assert_eq!((again.tabs, again.closed_tabs, again.runs), (0, 0, 0));
        assert!(again.windows.is_empty() && again.accounts_added.is_empty());
        assert!(!again.memory_copied);
        assert_eq!(snapshot(&dev), before);
    }

    #[test]
    fn a_live_dev_app_is_detected_and_nothing_in_it_changes() {
        let root = TempDir::new("import_live");
        let (dev, _run, config, _ws) = fixture(root.path());
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(600);
        // Nothing live: logs old (as seen 10 min later), no process, no lock.
        assert_eq!(dev_live_reason(&dev, "", later), None);
        // The session log was just written.
        let fresh = dev_live_reason(&dev, "", std::time::SystemTime::now()).unwrap();
        assert!(fresh.starts_with(QUIT_DEV_FIRST), "{fresh}");
        // A claude process runs that session.
        let ps = format!("  1 0 /sbin/launchd\n 500 1 /usr/bin/zsh\n 501 500 node /opt/claude/cli.js --settings {{}} --resume {SID} --plugin-dir /p\n");
        let me = std::process::id();
        assert!(session_in_use(&ps, SID, me));
        assert!(!session_in_use(&ps, SID2, me));
        assert!(!session_in_use(&format!(" 9 1 vim {SID}.jsonl"), SID, me));
        assert!(dev_live_reason(&dev, &ps, later)
            .unwrap()
            .contains("still running"));
        // The same claude in this app's own tree (its terminal) never counts:
        // a restart of the tab, or a re-import after resuming it here.
        let own = format!("  1 0 /sbin/launchd\n {me} 1 dcterminalv2\n 700 {me} /bin/zsh -l\n 701 700 claude --resume {SID}\n");
        assert!(!session_in_use(&own, SID, me));
        assert_eq!(dev_live_reason(&dev, &own, later), None);
        assert!(session_in_use(&own, SID, 12345), "another app's view: live");
        assert!(refuse_live_session("00000000-0000-4000-8000-000000000000").is_ok());
        let before = snapshot(&dev);
        // The app lock of a running build (another open file holds it).
        let AppLock::Acquired(held) = hold_app_lock(&root.join("other-app")) else {
            panic!("the first lock is ours");
        };
        assert!(lock_held(&root.join("other-app")));
        drop(held);
        assert!(crate::test_support::eventually(|| !lock_held(
            &root.join("other-app")
        )));
        assert!(!lock_held(&dev), "no lock file in an old dev build");
        assert_eq!(snapshot(&dev), before);
        let _ = config;
    }

    #[test]
    fn the_lock_tells_a_running_copy_from_a_folder_that_cannot_be_used() {
        let root = crate::test_support::TempDir::new("app_lock_outcomes");
        let data = root.join("data");
        let AppLock::Acquired(first) = hold_app_lock(&data) else {
            panic!("a free folder is locked");
        };
        // A second hold while the first file is alive: another copy runs.
        assert!(matches!(hold_app_lock(&data), AppLock::HeldElsewhere));
        drop(first);
        assert!(crate::test_support::eventually(|| matches!(
            hold_app_lock(&data),
            AppLock::Acquired(_)
        )));
        // A folder that cannot be created (a path under a regular file).
        let file = root.join("plain-file");
        std::fs::write(&file, "x").unwrap();
        match hold_app_lock(&file.join("data")) {
            AppLock::Unavailable(why) => assert!(why.contains("plain-file"), "{why}"),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn the_delta_lands_on_a_state_that_moved_on_and_a_clash_is_refused() {
        let mut before = AppStateFile::default();
        let mut after = before.clone();
        after.archived_windows.push(ArchivedWindow {
            window: WindowRecord {
                id: "win-2".into(),
                account_id: "work".into(),
                active_tab_id: None,
            },
            tab_ids: vec!["tab_9".into()],
            closed_at: "t".into(),
        });
        let mut t: TabRecord = serde_json::from_value(json!({"id": "tab_9", "label": "x", "cwd": "/w", "order": 1, "createdAt": "t", "windowId": "win-2"})).unwrap();
        t.closed_at = Some("t".into());
        after.closed_tabs.push(t.clone());
        let summary = ImportSummary {
            windows: vec!["win-2".into()],
            ..ImportSummary::default()
        };
        let mut now = before.clone();
        let mut other = t.clone();
        other.id = "tab_new".into();
        now.closed_tabs.push(other);
        apply_delta(&before, &after, &summary, &mut now).unwrap();
        assert_eq!(now.archived_windows[0].window.id, "win-2");
        assert_eq!(now.closed_tabs.len(), 2);
        before.windows.clear();
        let mut clash = AppStateFile::default();
        clash.windows.push(WindowRecord {
            id: "win-2".into(),
            account_id: "a".into(),
            active_tab_id: None,
        });
        assert!(apply_delta(&before, &after, &summary, &mut clash).is_err());
    }

    fn account(id: &str, dir: &str) -> crate::store::settings_store::ClaudeAccount {
        crate::store::settings_store::ClaudeAccount {
            id: id.into(),
            name: id.to_uppercase(),
            config_dir: Some(dir.into()),
        }
    }

    // Ticket 14yq6g30jeu: a dev account the settings refuse was found only
    // when the import saved them, after runs were adopted and memory copied.
    #[test]
    fn a_bad_dev_account_fails_before_runs_are_adopted_or_memory_is_copied() {
        let root = TempDir::new("import_bad_account");
        let (dev, _run, _config, _ws) = fixture(root.path());
        std::fs::write(
            dev.join("settings.json"),
            json!({"schemaVersion": 1, "claude": {"accounts": [
                {"id": "", "name": "Nameless id", "configDir": root.join("claude-x").display().to_string()}
            ]}})
            .to_string(),
        )
        .unwrap();
        let built = root.join("app");
        std::fs::create_dir_all(&built).unwrap();
        let mut state = AppStateFile::default();
        let mut claude = ClaudeSettings::default();
        let compose = |_: &Path, _: &Path| None;
        let re = Relaunch {
            plugin_dir: "/p".into(),
            settings_file: None,
            memory_dir: built.join("memory"),
            launch_dir: built.join("launch"),
            compose: &compose,
        };
        let err = import(&dev, &built, &mut state, &mut claude, None, &re, "t").unwrap_err();
        assert!(err.contains("cannot be imported"), "{err}");
        assert!(!built.join("runs").exists(), "no run adopted");
        assert!(!built.join("memory").exists(), "no memory copied");
        assert!(state.archived_windows.is_empty() && state.closed_tabs.is_empty());
    }

    #[test]
    fn added_accounts_are_merged_into_the_settings_as_they_are_now() {
        let current = ClaudeSettings {
            claude_path: None,
            accounts: vec![account("me", "/c/me"), account("meanwhile", "/c/meanwhile")],
        };
        let merged = merge_added_accounts(&current, &[account("work", "/c/work")], None).unwrap();
        let ids: Vec<&str> = merged.accounts.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(
            ids,
            ["me", "meanwhile", "work"],
            "the account added meanwhile stays"
        );
        // Added twice (a retry) is a no-op.
        let again = merge_added_accounts(&merged, &[account("work", "/c/work")], None).unwrap();
        assert_eq!(again.accounts.len(), 3);
        // The id now belongs to another folder.
        let taken = merge_added_accounts(&merged, &[account("meanwhile", "/c/else")], None);
        assert!(taken.unwrap_err().contains("try again"));
        // The folder now belongs to another id.
        let folder = merge_added_accounts(&merged, &[account("other", "/c/meanwhile")], None);
        assert!(folder.unwrap_err().contains("try again"));
    }

    #[test]
    fn a_window_whose_account_did_not_come_over_is_skipped() {
        let root = TempDir::new("import_full");
        let (dev, _run, _config, _ws) = fixture(root.path());
        let built = root.join("app");
        let mut state = AppStateFile::default();
        let mut claude = ClaudeSettings {
            claude_path: None,
            accounts: (0..8)
                .map(|n| crate::store::settings_store::ClaudeAccount {
                    id: format!("a{n}"),
                    name: format!("A{n}"),
                    config_dir: Some(format!("/nowhere/{n}")),
                })
                .collect(),
        };
        let compose = |_: &Path, _: &Path| None;
        let re = Relaunch {
            plugin_dir: "/p".into(),
            settings_file: None,
            memory_dir: built.join("memory"),
            launch_dir: built.join("launch"),
            compose: &compose,
        };
        let summary = import(&dev, &built, &mut state, &mut claude, None, &re, "t").unwrap();
        assert!(summary.windows.is_empty(), "never on another account");
        assert_eq!(summary.tabs, 0);
        assert_eq!(summary.notes.len(), 2);
        assert!(state.archived_windows.is_empty() && state.closed_tabs.is_empty());
    }

    #[test]
    fn the_dev_folder_is_named_after_the_built_one() {
        assert_eq!(
            dev_dir_for(Path::new("/s/com.jtfrancisco.dcterminalv2")),
            PathBuf::from("/s/com.jtfrancisco.dcterminalv2.dev")
        );
        let root = TempDir::new("import_none");
        assert!(!available(&root.join("x.dev")));
        let mut state = AppStateFile::default();
        let mut claude = ClaudeSettings::default();
        let compose = |_: &Path, _: &Path| None;
        let re = Relaunch {
            plugin_dir: "/p".into(),
            settings_file: None,
            memory_dir: "/m".into(),
            launch_dir: "/l".into(),
            compose: &compose,
        };
        assert!(import(
            &root.join("x.dev"),
            root.path(),
            &mut state,
            &mut claude,
            None,
            &re,
            "t"
        )
        .is_err());
        let record = ImportRecord {
            offered: true,
            imported_at: None,
        };
        write_record(root.path(), &record).unwrap();
        assert_eq!(read_record(root.path()), record);
    }
}
