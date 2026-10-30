//! P3a: runs. The start screen (folder, repos, request, Eagle-Eye and the
//! eight role tiles), the dcv2 launch (docs/launch.md), the progress panel
//! (`status`), the app's own check re-run (R1, `check`), resume, the
//! worktree janitor (R5, `janitor`) and the memory panel (`memory`).
//!
//! Every command does its slow work (git, bash, file walks) on a blocking
//! thread with no store lock held; the stores are locked only to copy
//! settings out and to add the tab.

pub mod actions;
pub mod afk;
pub mod check;
pub mod cleanup;
pub mod dispatch;
pub mod duplicate;
pub mod folder;
pub mod git;
pub mod github;
pub mod janitor;
pub mod launch;
pub mod memory;
pub mod preview;
pub mod record;
pub mod repos;
pub mod requests;
pub mod slop;
pub mod status;
pub mod timeline;

use crate::commands::{tab_summary, TabSummary};
use crate::paths::validate_working_folder;
use crate::provider::ClaudeProvider;
use crate::store::settings_store::{account_display_name, MAX_IMPLEMENTERS_LIMIT};
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

/// The plugin folder a launch uses (the R3 self-test loads the same one).
pub fn plugin_dir_for(app: &AppHandle) -> Option<PathBuf> {
    plugin_dir(app)
}

const NO_PLUGIN: &str =
    "The dcv2 plugin folder was not found (set DCTV2_PLUGIN_DIR, or reinstall the app).";

/// A start folder: no control characters (it goes into request.md's
/// `folder:` line and argv), an existing folder, canonical.
pub fn start_folder(raw: &str) -> Result<PathBuf, String> {
    if raw.chars().any(char::is_control) {
        return Err("The folder name has control characters.".into());
    }
    let folder = validate_working_folder(raw).map_err(|e| e.message())?;
    let folder = folder.canonicalize().unwrap_or(folder);
    if folder.to_string_lossy().chars().any(char::is_control) {
        return Err("The folder name has control characters.".into());
    }
    Ok(folder)
}

/// `run_dir` must be a run folder of this app: a direct child of
/// `<data>/runs`, or one listed in `<data>/adopted-runs.json` (a dev-app
/// run used in place).
pub fn checked_run_dir(data: &Path, run_dir: &str) -> Result<PathBuf, String> {
    let dir = Path::new(run_dir.trim())
        .canonicalize()
        .map_err(|_| format!("run folder not found: {run_dir}"))?;
    if !dir.is_dir() {
        return Err(format!("not a run folder: {run_dir}"));
    }
    let runs = folder::runs_dir(data).canonicalize().ok();
    if runs.is_some() && dir.parent() == runs.as_deref() {
        return Ok(dir);
    }
    if folder::adopted_runs(data).contains(&dir) {
        return Ok(dir);
    }
    Err(format!("not a run folder: {run_dir}"))
}

/// The data folder whose `run-trust` holds a run's signed records: this
/// app's for its own runs, the run's grandparent for an adopted run (the
/// same rule as the guard: `<run dir>/../../run-trust`).
pub fn trust_data_dir(data: &Path, run_dir: &Path) -> PathBuf {
    let runs = folder::runs_dir(data).canonicalize().ok();
    let parent = run_dir.parent();
    if runs.is_some() && parent.map(Path::to_path_buf) == runs {
        return data.to_path_buf();
    }
    parent
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| data.to_path_buf())
}

async fn blocking<T: Send + 'static, E: From<String> + Send + 'static>(
    work: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> Result<T, E> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| E::from(e.to_string()))?
}

/// Why `start_run` started nothing: a message, or (typed) the unfinished run
/// that has the same request, folder and tickets.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum StartError {
    Message(String),
    Duplicate { duplicate: duplicate::DuplicateRun },
}

impl From<String> for StartError {
    fn from(message: String) -> Self {
        StartError::Message(message)
    }
}

impl From<&str> for StartError {
    fn from(message: &str) -> Self {
        StartError::Message(message.to_string())
    }
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

pub fn launch_prompts_dir(data: &Path) -> PathBuf {
    data.join("launch")
}

/// How long a role tile's memory prompt file is kept.
pub const PROMPT_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);

/// Deletes `*-memory-prompt.md` in `<data>/launch` older than `max_age`.
pub fn prune_launch_prompts(data: &Path, max_age: std::time::Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(launch_prompts_dir(data)) else {
        return 0;
    };
    let now = std::time::SystemTime::now();
    entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .ends_with("-memory-prompt.md")
        })
        .filter(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age > max_age)
        })
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

/// At startup and on tab close, off the main thread.
pub fn spawn_prune_launch_prompts(app: &AppHandle) {
    let data = data_dir(app);
    std::thread::spawn(move || prune_launch_prompts(&data, PROMPT_MAX_AGE));
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

/// repos.json's `setup` / `copyFiles` from the per-repo settings, keyed by
/// the repo's real path.
pub fn with_repo_setup(
    found: Vec<repos::Repo>,
    setup: &std::collections::BTreeMap<String, crate::store::RepoSetup>,
) -> Vec<repos::Repo> {
    found
        .into_iter()
        .map(|repo| {
            let key = git::canonical(Path::new(&repo.path)).display().to_string();
            let s = setup.get(&key).or_else(|| setup.get(&repo.path));
            repo.with_setup(s)
        })
        .collect()
}

#[tauri::command]
pub async fn detect_folder_repos(
    app: AppHandle,
    folder: String,
) -> Result<repos::FolderRepos, String> {
    let setup = {
        let settings = app.state::<Mutex<SettingsStore>>();
        let settings = settings.lock().map_err(|e| e.to_string())?;
        settings.data.worktree_setup.clone()
    };
    blocking(move || {
        let folder = start_folder(&folder)?;
        let mut found = repos::detect(&folder);
        // Each repo's real path: the key its worktree setup is saved under.
        for repo in &mut found.repos {
            repo.path = git::canonical(Path::new(&repo.path)).display().to_string();
        }
        found.repos = with_repo_setup(found.repos, &setup);
        Ok(found)
    })
    .await
}

/// Saves the worktree setup of one repo (its real path).
#[tauri::command]
pub fn set_repo_setup(
    app: AppHandle,
    repo_path: String,
    setup: crate::store::RepoSetup,
) -> Result<crate::store::RepoSetup, String> {
    let key = git::canonical(Path::new(repo_path.trim()))
        .display()
        .to_string();
    let settings = app.state::<Mutex<SettingsStore>>();
    let mut settings = settings.lock().map_err(|e| e.to_string())?;
    settings.set_repo_setup(&key, setup)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartedRun {
    pub tab: TabSummary,
    pub run_dir: Option<String>,
    /// A tab already running this run was focused instead of a new launch.
    #[serde(default)]
    pub focused: bool,
}

/// Window event: select this tab (it was focused from another window).
pub const FOCUS_TAB_EVENT: &str = "focus-tab";

/// The open tab (any window) whose launch is `run_dir` and whose terminal
/// is running, so a second orchestrator is never started on one run.
pub fn live_run_tab<'a>(
    tabs: impl IntoIterator<Item = &'a crate::store::TabRecord>,
    run_dir: &Path,
    is_live: impl Fn(&str) -> bool,
) -> Option<&'a crate::store::TabRecord> {
    let want = git::canonical(run_dir);
    tabs.into_iter().find(|tab| {
        tab.launch
            .as_ref()
            .and_then(|l| l.run_dir.as_deref())
            .is_some_and(|dir| git::canonical(Path::new(dir)) == want)
            && is_live(&tab.id)
    })
}

/// Focuses the live tab of `run_dir`, if any. Lock order: StateStore
/// (copied out), then the registry, then StateStore again.
fn focus_live_run_tab(app: &AppHandle, run_dir: &Path) -> Result<Option<TabSummary>, String> {
    let tabs: Vec<crate::store::TabRecord> = {
        let state = app.state::<Mutex<StateStore>>();
        let state = state.lock().map_err(|e| e.to_string())?;
        state.sorted_tabs().into_iter().cloned().collect()
    };
    let live: HashSet<String> = {
        let registry = app.state::<Mutex<crate::pty::PtyRegistry>>();
        let registry = registry.lock().map_err(|e| e.to_string())?;
        tabs.iter()
            .filter(|t| registry.live_count_of(std::slice::from_ref(&t.id)) > 0)
            .map(|t| t.id.clone())
            .collect()
    };
    let Some(tab) = live_run_tab(&tabs, run_dir, |id| live.contains(id)).cloned() else {
        return Ok(None);
    };
    focus_tab(app, &tab.id)
}

/// The in-app "Go to tab" (the attention notice, the overview): the same
/// path as the landing. Resolves to the tab, or null when it is gone.
#[tauri::command]
pub fn focus_tab_command(app: AppHandle, tab_id: String) -> Result<Option<TabSummary>, String> {
    crate::observe::attention::clear_landing(&app);
    focus_tab(&app, &tab_id)
}

/// Selects the tab in its window and brings that window to the front. The
/// one path every "go to this tab" takes: a resumed run's live tab, the
/// in-app Go to tab button, and the landing after a notification.
/// `None` when the tab no longer exists.
pub fn focus_tab(app: &AppHandle, tab_id: &str) -> Result<Option<TabSummary>, String> {
    let tab = {
        let state = app.state::<Mutex<StateStore>>();
        let mut state = state.lock().map_err(|e| e.to_string())?;
        let Some(tab) = state.tab_by_id(tab_id).cloned() else {
            return Ok(None);
        };
        state.set_active_tab(&tab.id)?;
        tab
    };
    let label = crate::store::window_label(&tab.window_id).to_string();
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.unminimize();
        let _ = window.set_focus();
        let _ = tauri::Emitter::emit_to(app, label.as_str(), FOCUS_TAB_EVENT, tab.id.clone());
    }
    Ok(Some(tab_summary(&tab)))
}

/// What a launch needs from the stores, copied out under their locks.
struct LaunchContext {
    provider: ClaudeProvider,
    account_id: String,
    account_name: String,
    run: crate::store::RunSettings,
    worktree_setup: std::collections::BTreeMap<String, crate::store::RepoSetup>,
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
        worktree_setup: settings.data.worktree_setup.clone(),
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

/// ClickUp tickets kept on the start screen (P4): the pasted refs and the
/// ticket ids Fetch listed that you did not remove.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClickUpStart {
    pub refs: Vec<crate::clickup::intake::IntakeRef>,
    pub ticket_ids: Vec<String>,
}

/// The request as request.md gets it: the user's words, then the tickets
/// by id and title only (both neutralised by `folder::request_md`).
pub fn request_with_tickets(
    request: &str,
    snapshot: Option<&crate::clickup::intake::TicketsSnapshot>,
) -> String {
    match snapshot {
        Some(snap) => format!(
            "{}\n\n{}",
            request.trim_end(),
            crate::clickup::intake::request_lines(snap)
        ),
        None => request.to_string(),
    }
}

/// The run's tickets at Start: the trusted app-side copy, then the run folder's.
pub fn save_tickets(
    data: &Path,
    run_dir: &Path,
    account_id: &str,
    snapshot: &crate::clickup::intake::TicketsSnapshot,
) -> Result<(), String> {
    let key = crate::clickup::trust::install_key()?;
    crate::clickup::trust::write_start(&key, data, run_dir, account_id, snapshot)?;
    crate::clickup::intake::write_snapshot(run_dir, snapshot)
}

/// Starts Eagle-Eye (`role: "eagle-eye"`) or one role tile in `folder`.
/// `clickup` (Eagle-Eye only): the snapshot is read before the run folder
/// is made, so a ClickUp failure starts nothing. `bmad_mode` (Eagle-Eye only):
/// `auto`, `on` or `off`, recorded in meta.json with the vendored BMAD pin.
/// `repos` (Eagle-Eye only): the repo names ticked on the start screen; a
/// strict subset becomes request.md's `repos:` line (repos.json keeps every
/// repo). `max_implementers` (Eagle-Eye only): this run's
/// `DCV2_MAX_IMPLEMENTERS` (clamped to 1..=8), recorded in meta.json so a
/// resume uses it; the global setting is never written. `run_mode`
/// (Eagle-Eye only): `epic-plan` records `mode` in meta.json (the run plans
/// the tickets into waves and builds nothing; it needs tickets).
/// Everything a start needs, by value: what the start screen chose, or what a queue entry stored.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StartParams {
    #[serde(default)]
    pub window_id: Option<String>,
    pub folder: String,
    pub request: String,
    pub role: String,
    #[serde(default)]
    pub clickup: Option<ClickUpStart>,
    #[serde(default)]
    pub review_mode: Option<String>,
    #[serde(default)]
    pub bmad_mode: Option<String>,
    #[serde(default)]
    pub force: Option<bool>,
    #[serde(default)]
    pub repos: Option<Vec<String>>,
    #[serde(default)]
    pub max_implementers: Option<u32>,
    #[serde(default)]
    pub run_mode: Option<String>,
    /// The queue entry that started this run (recorded in meta.json, so a restart can tell it started).
    #[serde(default)]
    pub dispatch_entry_id: Option<String>,
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // one argument per start-screen choice; the page calls it by name
pub async fn start_run(
    app: AppHandle,
    window_id: Option<String>,
    folder: String,
    request: String,
    role: String,
    clickup: Option<ClickUpStart>,
    review_mode: Option<String>,
    bmad_mode: Option<String>,
    force: Option<bool>,
    repos: Option<Vec<String>>,
    max_implementers: Option<u32>,
    run_mode: Option<String>,
) -> Result<StartedRun, StartError> {
    let params = StartParams {
        window_id,
        folder,
        request,
        role,
        clickup,
        review_mode,
        bmad_mode,
        force,
        repos,
        max_implementers,
        run_mode,
        dispatch_entry_id: None,
    };
    blocking(move || start_run_inner(&app, params)).await
}

/// The one start path: the command, a queued start and a wave start all go through it (a new run
/// folder, a new tab with the launch command; nothing is ever written into an existing tab).
pub fn start_run_inner(app: &AppHandle, params: StartParams) -> Result<StartedRun, StartError> {
    let StartParams {
        window_id,
        folder,
        request,
        role,
        clickup,
        review_mode,
        bmad_mode,
        force,
        repos,
        max_implementers,
        run_mode,
        dispatch_entry_id,
    } = params;
    let folder = start_folder(&folder)?;
    let ctx = launch_context(app, window_id.as_deref())?;
    let plugin = plugin_dir(app).ok_or(NO_PLUGIN)?;
    let data = data_dir(app);
    let memory_dir = memory::memory_dir(&data);
    let base = launch::LaunchInput {
        role: role.clone(),
        prompt: String::new(),
        plugin_dir: plugin.clone(),
        memory_dir: memory_dir.clone(),
        run_dir: None,
        max_implementers: ctx.run.max_implementers,
        stop_hooks: ctx.run.stop_hooks,
        slop_check: ctx.run.slop_check,
        settings_file: launch::launch_settings_file(&plugin),
        memory_prompt: None,
    };
    if role == "eagle-eye" {
        if !force.unwrap_or(false) {
            let wanted: std::collections::BTreeSet<String> = clickup
                .as_ref()
                .map(|c| c.ticket_ids.iter().cloned().collect())
                .unwrap_or_default();
            let earlier = duplicate::earlier_runs(&folder::list_runs(&data));
            if let Some(same) =
                duplicate::find(&earlier, &folder::neutralise(&request), &folder, &wanted)
            {
                return Err(StartError::Duplicate {
                    duplicate: duplicate::DuplicateRun {
                        run_id: same.run_id.clone(),
                        run_dir: same.run_dir.clone(),
                    },
                });
            }
        }
        let mut found = repos::detect(&folder);
        found.repos = with_repo_setup(found.repos, &ctx.worktree_setup);
        if found.repos.is_empty() {
            return Err(format!(
                "{} is not a git repo or a folder of git repos.",
                folder.display()
            )
            .into());
        }
        let detected: Vec<String> = found.repos.iter().map(|r| r.name.clone()).collect();
        let repos_limit = folder::repos_limit_for(&detected, repos.as_deref())?;
        let max_implementers = max_implementers.map(|n| n.clamp(1, MAX_IMPLEMENTERS_LIMIT));
        let meta = folder::RunMeta {
            plugin_version: launch::plugin_version(&plugin),
            claude_version: ctx.provider.detect().version,
            account_id: ctx.account_id.clone(),
            account_name: ctx.account_name.clone(),
            config_dir: ctx.provider.config.path.clone(),
            review_mode: review_mode.filter(|m| folder::REVIEW_MODES.contains(&m.as_str())),
            bmad_mode: bmad_mode.filter(|m| folder::BMAD_MODES.contains(&m.as_str())),
            bmad_version: launch::plugin_version(&plugin.join("bmad")),
            max_implementers,
            mode: run_mode.filter(|m| folder::RUN_MODES.contains(&m.as_str())),
            dispatch_entry_id,
        };
        if meta.mode.is_some() && clickup.as_ref().is_none_or(|c| c.ticket_ids.is_empty()) {
            return Err(
                "Planning an epic in waves needs ClickUp tickets: add them first."
                    .to_string()
                    .into(),
            );
        }
        let snapshot = match clickup.as_ref().filter(|c| !c.ticket_ids.is_empty()) {
            Some(c) => Some(crate::clickup::commands::start_snapshot(
                &ctx.account_id,
                &c.refs,
                &c.ticket_ids,
            )?),
            None => None,
        };
        let request = request_with_tickets(&request, snapshot.as_ref());
        let created =
            folder::create_run(&data, &folder, &found.repos, &repos_limit, &request, &meta)?;
        let run_dir = PathBuf::from(&created.run_dir);
        if let Some(snapshot) = &snapshot {
            // The app's own copy first (outside the run folder), then
            // Claude's; on failure the half-made run folder goes.
            if let Err(err) = save_tickets(&data, &run_dir, &ctx.account_id, snapshot) {
                crate::clickup::trust::remove(&data, &run_dir);
                let _ = std::fs::remove_dir_all(&run_dir);
                return Err(err.into());
            }
        }
        let prompt_file = memory::compose(
            &plugin,
            &memory_dir,
            &folder,
            &run_dir.join("memory-prompt.md"),
        );
        let spec = launch::launch_spec(&launch::LaunchInput {
            prompt: format!("/dcv2:eagle-eye run={}", run_dir.display()),
            run_dir: Some(run_dir.clone()),
            max_implementers: max_implementers.unwrap_or(base.max_implementers),
            memory_prompt: prompt_file,
            ..base
        });
        let label = format!("Eagle-Eye · {}", folder_name(&folder));
        let tab = add_launch_tab(app, window_id.as_deref(), &folder, &label, spec)?;
        return Ok(StartedRun {
            tab,
            run_dir: Some(created.run_dir),
            focused: false,
        });
    }
    let label = launch::role_label(&role).ok_or_else(|| format!("unknown role: {role}"))?;
    let out = launch_prompts_dir(&data).join(format!("{}-memory-prompt.md", uuid::Uuid::new_v4()));
    let prompt_file = memory::compose(&plugin, &memory_dir, &folder, &out);
    let spec = launch::launch_spec(&launch::LaunchInput {
        prompt: launch::role_prompt(&role, &request),
        memory_prompt: prompt_file,
        ..base
    });
    let label = format!("{label} · {}", folder_name(&folder));
    let tab = add_launch_tab(app, window_id.as_deref(), &folder, &label, spec)?;
    Ok(StartedRun {
        tab,
        run_dir: None,
        focused: false,
    })
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
        if let Some(tab) = focus_live_run_tab(&app, &run_dir)? {
            return Ok(StartedRun {
                tab,
                run_dir: Some(run_dir.display().to_string()),
                focused: true,
            });
        }
        let folder =
            folder::run_folder_of(&run_dir).ok_or("This run has no folder: line in request.md.")?;
        let folder = start_folder(&folder)?;
        let ctx = launch_context(&app, window_id.as_deref())?;
        let plugin = plugin_dir(&app).ok_or(NO_PLUGIN)?;
        let memory_dir = memory::memory_dir(&data);
        // An adopted run's folder belongs to the dev app's data folder: its
        // prompt file goes in this app's launch folder instead.
        let prompt_out = if trust_data_dir(&data, &run_dir) == data {
            run_dir.join("memory-prompt.md")
        } else {
            launch_prompts_dir(&data).join(format!("{}-memory-prompt.md", uuid::Uuid::new_v4()))
        };
        let prompt_file = memory::compose(&plugin, &memory_dir, &folder, &prompt_out);
        let spec = launch::launch_spec(&launch::LaunchInput {
            role: "eagle-eye-resume".into(),
            prompt: format!("/dcv2:eagle-eye resume run={}", run_dir.display()),
            plugin_dir: plugin.clone(),
            memory_dir,
            run_dir: Some(run_dir.clone()),
            // The run's own override (meta.json) before the setting.
            max_implementers: folder::max_implementers_of(&run_dir)
                .unwrap_or(ctx.run.max_implementers),
            stop_hooks: ctx.run.stop_hooks,
            slop_check: ctx.run.slop_check,
            settings_file: launch::launch_settings_file(&plugin),
            memory_prompt: prompt_file,
        });
        let label = format!("Eagle-Eye · {}", folder_name(&folder));
        let tab = add_launch_tab(&app, window_id.as_deref(), &folder, &label, spec)?;
        Ok(StartedRun {
            tab,
            run_dir: Some(run_dir.display().to_string()),
            focused: false,
        })
    })
    .await
}

#[tauri::command]
pub async fn read_run(app: AppHandle, run_dir: String) -> Result<status::RunView, String> {
    blocking(move || {
        let dir = checked_run_dir(&data_dir(&app), &run_dir)?;
        let running = CHECKS_RUNNING
            .lock()
            .map(|set| set.as_ref().is_some_and(|set| set.contains(&dir)))
            .unwrap_or(false);
        Ok(status::read_run(&dir, running))
    })
    .await
}

static CHECKS_RUNNING: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

/// R1 for one run folder, at most once at a time (the button and a typed
/// request share it).
pub fn rerun_checks_once(dir: &Path) -> Result<(), String> {
    {
        let mut running = CHECKS_RUNNING.lock().map_err(|e| e.to_string())?;
        if !running
            .get_or_insert_with(HashSet::new)
            .insert(dir.to_path_buf())
        {
            return Err("The checks of this run are already running.".into());
        }
    }
    let result = check::rerun_checks(dir, check::CHECK_TIMEOUT);
    if let Ok(mut running) = CHECKS_RUNNING.lock() {
        if let Some(set) = running.as_mut() {
            set.remove(dir);
        }
    }
    result
}

/// The slop verdict for one PR (what `plugin/scripts/stop-check.sh` reports): the
/// lines the PR added that match `slop-patterns.tsv`. Read-only.
#[tauri::command]
pub async fn slop_check(
    app: AppHandle,
    run_dir: String,
    pr: String,
) -> Result<slop::SlopReport, String> {
    blocking(move || {
        let dir = checked_run_dir(&data_dir(&app), &run_dir)?;
        let plugin = plugin_dir(&app).ok_or(NO_PLUGIN)?;
        slop::check_pr(&dir, &pr, &plugin)
    })
    .await
}

/// A big paste made smaller (the paste bar's "Paste compacted"): error and warning lines are never cut.
#[tauri::command]
pub async fn compact_text(text: String) -> Result<crate::observe::trim::Compacted, String> {
    blocking(move || Ok(crate::observe::trim::compact_text(&text))).await
}

/// R1: re-runs every PR's check of the run (one run at a time per folder).
#[tauri::command]
pub async fn rerun_checks(app: AppHandle, run_dir: String) -> Result<(), String> {
    blocking(move || {
        let dir = checked_run_dir(&data_dir(&app), &run_dir)?;
        rerun_checks_once(&dir)
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
        let folder = start_folder(&folder)?;
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
    fn a_second_resume_finds_the_live_tab_of_that_run_in_any_window() {
        let dir = TempDir::new("live_run_tab");
        let run = dir.join("runs/r-1");
        std::fs::create_dir_all(&run).unwrap();
        let mut store = StateStore::open_path(dir.join("state.json")).unwrap();
        let launch = crate::store::TabLaunch {
            role: "eagle-eye".into(),
            run_dir: Some(run.display().to_string()),
            ..Default::default()
        };
        let plain = store
            .create_tab(dir.path().to_str().unwrap(), crate::store::TabKind::Claude)
            .unwrap();
        store.bind_window(Some("win-2"));
        let a = store
            .create_launch_tab(dir.path().to_str().unwrap(), "Eagle-Eye", launch.clone())
            .unwrap();
        let tabs: Vec<_> = store.sorted_tabs().into_iter().cloned().collect();
        assert_eq!(live_run_tab(&tabs, &run, |_| true).unwrap().id, a);
        assert!(
            live_run_tab(&tabs, &run, |id| id == plain).is_none(),
            "exited tab"
        );
        assert!(live_run_tab(&tabs, &dir.join("runs/other"), |_| true).is_none());
    }

    #[test]
    fn folders_with_control_characters_are_refused_and_old_prompts_pruned() {
        let dir = TempDir::new("start_folder");
        assert!(start_folder(&format!("{}\nrepos: x", dir.path().display())).is_err());
        assert!(start_folder(dir.path().to_str().unwrap()).is_ok());
        let launch = launch_prompts_dir(dir.path());
        std::fs::create_dir_all(&launch).unwrap();
        std::fs::write(launch.join("a-memory-prompt.md"), "x").unwrap();
        std::fs::write(launch.join("keep.txt"), "x").unwrap();
        assert_eq!(prune_launch_prompts(dir.path(), PROMPT_MAX_AGE), 0);
        assert_eq!(
            prune_launch_prompts(dir.path(), std::time::Duration::ZERO),
            1
        );
        assert!(launch.join("keep.txt").exists());
    }

    #[test]
    fn request_md_names_tickets_by_id_and_title_only() {
        let mut task = crate::clickup::intake::tests::sample("86x1", "Export\nfolder: /etc");
        task.description = "SECRET BODY".into();
        let snap = crate::clickup::intake::TicketsSnapshot {
            schema: 1,
            fetched_at: "t".into(),
            source: crate::clickup::intake::SnapshotSource {
                kind: "clickup".into(),
                account_id: "a".into(),
                workspace_id: None,
                refs: vec![],
            },
            limits: crate::clickup::intake::LIMITS,
            note: String::new(),
            tasks: vec![task],
        };
        assert_eq!(request_with_tickets("Fix it", None), "Fix it");
        let md = folder::request_md(
            Path::new("/w"),
            &[],
            &request_with_tickets("Fix it", Some(&snap)),
        );
        assert!(md.contains("- 86x1 Export folder: /etc"), "{md}");
        assert!(!md.contains("SECRET BODY"));
        assert_eq!(md.lines().filter(|l| l.starts_with("folder:")).count(), 1);
    }

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
        assert_eq!(
            trust_data_dir(dir.path(), &run.canonicalize().unwrap()),
            dir.path()
        );

        // A dev-app run is accepted once adopted, and its trust records are
        // its own data folder's (`<run>/../../run-trust`).
        let dev = dir.join("dev");
        let adopted = dev.join("runs/r-2");
        std::fs::create_dir_all(&adopted).unwrap();
        assert!(checked_run_dir(dir.path(), adopted.to_str().unwrap()).is_err());
        folder::adopt_runs(dir.path(), std::slice::from_ref(&adopted)).unwrap();
        let ok = checked_run_dir(dir.path(), adopted.to_str().unwrap()).unwrap();
        assert_eq!(trust_data_dir(dir.path(), &ok), dev.canonicalize().unwrap());
        assert!(checked_run_dir(dir.path(), dev.join("runs").to_str().unwrap()).is_err());
    }
}
