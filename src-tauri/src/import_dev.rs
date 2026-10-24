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
    // A dev window that names no account used the first one.
    let first_dev = dev_settings.claude.accounts.first().map(|a| a.id.clone());
    let account_for = |id: &str| {
        accounts
            .get(id)
            .or_else(|| first_dev.as_ref().and_then(|f| accounts.get(f)))
            .cloned()
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

/// Imports the dev app's tabs, windows, runs and memory, then reopens the
/// imported windows.
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
        let home = crate::provider::claude_config::home_dir();
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let summary = {
            let state = app.state::<Mutex<crate::store::StateStore>>();
            let mut state = state.lock().map_err(|e| e.to_string())?;
            let mut data_copy = state.data.clone();
            let summary = import(
                &dev,
                &data,
                &mut data_copy,
                &mut claude,
                home.as_deref(),
                &re,
                &now,
            )?;
            state.data = data_copy;
            state.save()?;
            summary
        };
        {
            let settings = app.state::<Mutex<crate::store::SettingsStore>>();
            let mut settings = settings.lock().map_err(|e| e.to_string())?;
            settings.set_claude(claude)?;
        }
        write_record(
            &data,
            &ImportRecord {
                offered: true,
                imported_at: Some(now),
            },
        )?;
        for _ in &summary.windows {
            if let Err(err) = crate::windows::reopen_last_closed_window(&app) {
                eprintln!("DCTerminalv2: reopen imported window: {err}");
                break;
            }
        }
        crate::windows::retitle_open_windows(&app);
        Ok(summary)
    })
    .await
    .map_err(|e| e.to_string())?
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
