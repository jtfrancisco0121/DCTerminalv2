//! P3a: runs. The start screen (folder, repos, request, Eagle-Eye and the
//! seven role tiles), the dcv2 launch (docs/launch.md), the progress panel
//! (`status`), the app's own check re-run (R1, `check`), resume, the
//! worktree janitor (R5, `janitor`) and the memory panel (`memory`).
//!
//! Every command does its slow work (git, bash, file walks) on a blocking
//! thread with no store lock held; the stores are locked only to copy
//! settings out and to add the tab.

pub mod check;
pub mod folder;
pub mod git;
pub mod janitor;
pub mod launch;
pub mod memory;
pub mod repos;
pub mod status;

use crate::commands::{tab_summary, TabSummary};
use crate::paths::validate_working_folder;
use crate::provider::ClaudeProvider;
use crate::store::settings_store::account_display_name;
use crate::store::{SettingsStore, StateStore};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

fn data_dir(app: &AppHandle) -> PathBuf {
    crate::data_dir::app_data_dir(app).path
}

/// A debug build uses the repo's `plugin/` (live edits) before the copy
/// Tauri puts beside the binary; a release build uses the bundled one.
fn plugin_dir(app: &AppHandle) -> Option<PathBuf> {
    let env = std::env::var(launch::PLUGIN_DIR_ENV).ok();
    let resource = app.path().resource_dir().ok();
    let repo = launch::dev_repo_root();
    if cfg!(debug_assertions) {
        if let Some(dir) = launch::resolve_plugin_dir(env.as_deref(), None, &repo) {
            return Some(dir);
        }
    }
    launch::resolve_plugin_dir(env.as_deref(), resource.as_deref(), &repo)
}

const NO_PLUGIN: &str =
    "The dcv2 plugin folder was not found (set DCTV2_PLUGIN_DIR, or reinstall the app).";

/// `run_dir` must be a run folder of this app: a direct child of `<data>/runs`.
pub fn checked_run_dir(data: &Path, run_dir: &str) -> Result<PathBuf, String> {
    let runs = folder::runs_dir(data)
        .canonicalize()
        .map_err(|_| "no runs yet".to_string())?;
    let dir = Path::new(run_dir.trim())
        .canonicalize()
        .map_err(|_| format!("run folder not found: {run_dir}"))?;
    if dir.parent() != Some(runs.as_path()) || !dir.is_dir() {
        return Err(format!("not a run folder: {run_dir}"));
    }
    Ok(dir)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleTile {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOverview {
    pub plugin_dir: Option<String>,
    pub plugin_version: Option<String>,
    pub roles: Vec<RoleTile>,
    pub runs: Vec<folder::RunListItem>,
}

#[tauri::command]
pub async fn run_overview(app: AppHandle) -> Result<RunOverview, String> {
    blocking(move || {
        let plugin = plugin_dir(&app);
        Ok(RunOverview {
            plugin_version: plugin.as_deref().and_then(launch::plugin_version),
            plugin_dir: plugin.map(|p| p.display().to_string()),
            roles: launch::ROLES
                .iter()
                .map(|(id, label)| RoleTile {
                    id: id.to_string(),
                    label: label.to_string(),
                })
                .collect(),
            runs: folder::list_runs(&data_dir(&app)),
        })
    })
    .await
}

#[tauri::command]
pub async fn detect_folder_repos(folder: String) -> Result<repos::FolderRepos, String> {
    blocking(move || {
        let folder = validate_working_folder(&folder).map_err(|e| e.message())?;
        Ok(repos::detect(&folder))
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartedRun {
    pub tab: TabSummary,
    pub run_dir: Option<String>,
}

/// What a launch needs from the stores, copied out under their locks.
struct LaunchContext {
    provider: ClaudeProvider,
    account_id: String,
    account_name: String,
    run: crate::store::RunSettings,
}

fn launch_context(app: &AppHandle, window_id: Option<&str>) -> Result<LaunchContext, String> {
    let account_id = {
        let state = app.state::<Mutex<StateStore>>();
        let mut state = state.lock().map_err(|e| e.to_string())?;
        state.bind_window(window_id);
        let window = state.focus_window().to_string();
        state.account_for_window(&window)
    };
    let settings = app.state::<Mutex<SettingsStore>>();
    let settings = settings.lock().map_err(|e| e.to_string())?;
    let claude = settings.claude();
    Ok(LaunchContext {
        provider: ClaudeProvider::for_account(claude, &account_id),
        account_name: account_display_name(claude, &account_id),
        run: settings.run().clone(),
        account_id,
    })
}

fn add_launch_tab(
    app: &AppHandle,
    window_id: Option<&str>,
    cwd: &Path,
    label: &str,
    spec: crate::store::TabLaunch,
) -> Result<TabSummary, String> {
    let state = app.state::<Mutex<StateStore>>();
    let mut state = state.lock().map_err(|e| e.to_string())?;
    state.bind_window(window_id);
    let id = state.create_launch_tab(&cwd.display().to_string(), label, spec)?;
    state
        .tab_by_id(&id)
        .map(tab_summary)
        .ok_or_else(|| "new tab missing".into())
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// Starts Eagle-Eye (`role: "eagle-eye"`) or one role tile in `folder`.
#[tauri::command]
pub async fn start_run(
    app: AppHandle,
    window_id: Option<String>,
    folder: String,
    request: String,
    role: String,
) -> Result<StartedRun, String> {
    blocking(move || {
        let folder = validate_working_folder(&folder).map_err(|e| e.message())?;
        let folder = folder.canonicalize().unwrap_or(folder);
        let ctx = launch_context(&app, window_id.as_deref())?;
        let plugin = plugin_dir(&app).ok_or(NO_PLUGIN)?;
        let data = data_dir(&app);
        let memory_dir = memory::memory_dir(&data);
        let base = launch::LaunchInput {
            role: role.clone(),
            prompt: String::new(),
            plugin_dir: plugin.clone(),
            memory_dir: memory_dir.clone(),
            run_dir: None,
            max_implementers: ctx.run.max_implementers,
            stop_hooks: ctx.run.stop_hooks,
            settings_file: launch::launch_settings_file(&plugin),
            memory_prompt: None,
        };
        if role == "eagle-eye" {
            let found = repos::detect(&folder);
            if found.repos.is_empty() {
                return Err(format!(
                    "{} is not a git repo or a folder of git repos.",
                    folder.display()
                ));
            }
            let meta = folder::RunMeta {
                plugin_version: launch::plugin_version(&plugin),
                claude_version: ctx.provider.detect().version,
                account_id: ctx.account_id.clone(),
                account_name: ctx.account_name.clone(),
                config_dir: ctx.provider.config.path.clone(),
            };
            let created = folder::create_run(&data, &folder, &found.repos, &[], &request, &meta)?;
            let run_dir = PathBuf::from(&created.run_dir);
            let prompt_file = memory::compose(
                &plugin,
                &memory_dir,
                &folder,
                &run_dir.join("memory-prompt.md"),
            );
            let spec = launch::launch_spec(&launch::LaunchInput {
                prompt: format!("/dcv2:eagle-eye run={}", run_dir.display()),
                run_dir: Some(run_dir.clone()),
                memory_prompt: prompt_file,
                ..base
            });
            let label = format!("Eagle-Eye · {}", folder_name(&folder));
            let tab = add_launch_tab(&app, window_id.as_deref(), &folder, &label, spec)?;
            return Ok(StartedRun {
                tab,
                run_dir: Some(created.run_dir),
            });
        }
        let label = launch::role_label(&role).ok_or_else(|| format!("unknown role: {role}"))?;
        let out = data
            .join("launch")
            .join(format!("{}-memory-prompt.md", uuid::Uuid::new_v4()));
        let prompt_file = memory::compose(&plugin, &memory_dir, &folder, &out);
        let spec = launch::launch_spec(&launch::LaunchInput {
            prompt: launch::role_prompt(&role, &request),
            memory_prompt: prompt_file,
            ..base
        });
        let label = format!("{label} · {}", folder_name(&folder));
        let tab = add_launch_tab(&app, window_id.as_deref(), &folder, &label, spec)?;
        Ok(StartedRun { tab, run_dir: None })
    })
    .await
}

/// `/dcv2:eagle-eye resume run=<dir>` in a new tab, with the same env.
#[tauri::command]
pub async fn resume_run(
    app: AppHandle,
    window_id: Option<String>,
    run_dir: String,
) -> Result<StartedRun, String> {
    blocking(move || {
        let data = data_dir(&app);
        let run_dir = checked_run_dir(&data, &run_dir)?;
        let folder =
            folder::run_folder_of(&run_dir).ok_or("This run has no folder: line in request.md.")?;
        let folder = validate_working_folder(&folder).map_err(|e| e.message())?;
        let ctx = launch_context(&app, window_id.as_deref())?;
        let plugin = plugin_dir(&app).ok_or(NO_PLUGIN)?;
        let memory_dir = memory::memory_dir(&data);
        let prompt_file = memory::compose(
            &plugin,
            &memory_dir,
            &folder,
            &run_dir.join("memory-prompt.md"),
        );
        let spec = launch::launch_spec(&launch::LaunchInput {
            role: "eagle-eye-resume".into(),
            prompt: format!("/dcv2:eagle-eye resume run={}", run_dir.display()),
            plugin_dir: plugin.clone(),
            memory_dir,
            run_dir: Some(run_dir.clone()),
            max_implementers: ctx.run.max_implementers,
            stop_hooks: ctx.run.stop_hooks,
            settings_file: launch::launch_settings_file(&plugin),
            memory_prompt: prompt_file,
        });
        let label = format!("Eagle-Eye · {}", folder_name(&folder));
        let tab = add_launch_tab(&app, window_id.as_deref(), &folder, &label, spec)?;
        Ok(StartedRun {
            tab,
            run_dir: Some(run_dir.display().to_string()),
        })
    })
    .await
}

#[tauri::command]
pub async fn read_run(app: AppHandle, run_dir: String) -> Result<status::RunView, String> {
    blocking(move || {
        let dir = checked_run_dir(&data_dir(&app), &run_dir)?;
        Ok(status::read_run(&dir))
    })
    .await
}

static CHECKS_RUNNING: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

/// R1: re-runs every PR's check of the run (one run at a time per folder).
#[tauri::command]
pub async fn rerun_checks(app: AppHandle, run_dir: String) -> Result<(), String> {
    blocking(move || {
        let dir = checked_run_dir(&data_dir(&app), &run_dir)?;
        {
            let mut running = CHECKS_RUNNING.lock().map_err(|e| e.to_string())?;
            if !running.get_or_insert_with(HashSet::new).insert(dir.clone()) {
                return Err("The checks of this run are already running.".into());
            }
        }
        let result = check::rerun_checks(&dir, check::CHECK_TIMEOUT);
        if let Ok(mut running) = CHECKS_RUNNING.lock() {
            if let Some(set) = running.as_mut() {
                set.remove(&dir);
            }
        }
        result
    })
    .await
}

#[tauri::command]
pub async fn run_worktrees(
    app: AppHandle,
    run_dir: String,
) -> Result<Vec<janitor::WorktreeItem>, String> {
    blocking(move || {
        let dir = checked_run_dir(&data_dir(&app), &run_dir)?;
        Ok(janitor::list_worktrees(&dir))
    })
    .await
}

#[tauri::command]
pub async fn remove_run_worktrees(
    app: AppHandle,
    run_dir: String,
    pr_ids: Vec<String>,
) -> Result<Vec<janitor::RemoveResult>, String> {
    blocking(move || {
        let dir = checked_run_dir(&data_dir(&app), &run_dir)?;
        Ok(janitor::remove_clean(&dir, &pr_ids))
    })
    .await
}

/// A text file of the run (`report.md`, `logs/…`), read-only.
#[tauri::command]
pub async fn read_run_file(
    app: AppHandle,
    run_dir: String,
    path: String,
) -> Result<String, String> {
    blocking(move || {
        let dir = checked_run_dir(&data_dir(&app), &run_dir)?;
        let name = Path::new(&path);
        if name
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
            || name
                .to_string_lossy()
                .split('/')
                .any(|p| p.starts_with('.'))
        {
            return Err("not a run file".into());
        }
        let file = dir
            .join(name)
            .canonicalize()
            .map_err(|_| "file not found".to_string())?;
        if !file.starts_with(&dir) || !file.is_file() {
            return Err("not a run file".into());
        }
        let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
        Ok(text.chars().take(400_000).collect())
    })
    .await
}

#[tauri::command]
pub async fn memory_status(app: AppHandle, folder: String) -> Result<serde_json::Value, String> {
    blocking(move || {
        let folder = validate_working_folder(&folder).map_err(|e| e.message())?;
        let Some(plugin) = plugin_dir(&app) else {
            return Ok(serde_json::json!({"available": false, "problem": NO_PLUGIN}));
        };
        Ok(memory::status(
            &plugin,
            &memory::memory_dir(&data_dir(&app)),
            &folder,
        ))
    })
    .await
}

#[tauri::command]
pub async fn read_memory_file(app: AppHandle, path: String) -> Result<String, String> {
    blocking(move || memory::read_file(&memory::memory_dir(&data_dir(&app)), &path)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn only_run_folders_of_this_app_are_accepted() {
        let dir = TempDir::new("checked_run");
        assert!(checked_run_dir(dir.path(), "/tmp").is_err());
        let run = dir.join("runs/r-1");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::create_dir_all(dir.join("runs/r-1/deeper")).unwrap();
        assert!(checked_run_dir(dir.path(), run.to_str().unwrap()).is_ok());
        assert!(
            checked_run_dir(dir.path(), dir.join("runs/r-1/deeper").to_str().unwrap()).is_err()
        );
        assert!(checked_run_dir(dir.path(), dir.join("runs/r-1/../..").to_str().unwrap()).is_err());
        assert!(checked_run_dir(dir.path(), dir.join("runs").to_str().unwrap()).is_err());
    }
}
