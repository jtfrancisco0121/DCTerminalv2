//! AgentFlight commands: the Session Inspector reads one run's or one
//! session's Claude transcripts. Both run on a worker thread, resolve the
//! capture policy before any file is opened, and hold no store lock while
//! they read.

use crate::flight::host::{self, ContentOut, FlightCache, Opens, Query};
use crate::flight::index::{SearchFilter, SearchOut};
use crate::flight::schema::SourceRef;
use crate::flight::session::{PageRequest, SessionView};
use crate::store::SettingsStore;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, State};

/// Which transcripts: a run's recorded sessions (`runDir`), or one session of
/// one account folder.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub run_dir: Option<String>,
    pub config_dir: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentRequest {
    #[serde(flatten)]
    pub target: Target,
    #[serde(rename = "ref")]
    pub source: SourceRef,
}

/// The run folder, checked, or the plain `{configDir, sessionId}` query.
pub(crate) fn query_for(app: &AppHandle, target: &Target) -> Result<Query, String> {
    let run_dir: Option<PathBuf> = match &target.run_dir {
        Some(dir) => {
            let data = crate::data_dir::app_data_dir(app).path;
            Some(crate::run::checked_run_dir(&data, dir)?)
        }
        None => None,
    };
    if run_dir.is_none() && (target.config_dir.is_none() || target.session_id.is_none()) {
        return Err("name a run folder, or a config folder and a session".into());
    }
    Ok(Query {
        run_dir,
        config_dir: target.config_dir.clone(),
        session_id: target.session_id.clone(),
    })
}

/// The text behind one event, secret-scanned (always, whatever the policy)
/// with the number of redacted lines.
#[tauri::command]
pub async fn flight_content(
    app: AppHandle,
    settings: State<'_, Mutex<SettingsStore>>,
    request: ContentRequest,
) -> Result<ContentOut, String> {
    let claude = settings.lock().map_err(|e| e.to_string())?.claude().clone();
    let query = query_for(&app, &request.target)?;
    tauri::async_runtime::spawn_blocking(move || {
        host::read_content(&claude, &query, &request.source, &Opens::default())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRequest {
    #[serde(flatten)]
    pub target: Target,
    pub agent: Option<String>,
    pub cursor: Option<String>,
    pub page_size: Option<usize>,
    /// An event id: with no cursor, the first page is the one holding it (a search result opens there).
    pub focus: Option<String>,
}

/// One run's (or one session's) trace: the agent tree, the failures and one
/// page of events. Parses on a worker thread through the cache.
#[tauri::command]
pub async fn flight_session(
    app: AppHandle,
    settings: State<'_, Mutex<SettingsStore>>,
    cache: State<'_, FlightCache>,
    request: SessionRequest,
) -> Result<SessionView, String> {
    let claude = settings.lock().map_err(|e| e.to_string())?.claude().clone();
    let query = query_for(&app, &request.target)?;
    let cache = cache.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let page = PageRequest {
            agent: request.agent,
            cursor: request.cursor,
            page_size: request.page_size,
            focus: request.focus,
        };
        host::session_view(&claude, &query, &page, &cache, &Opens::default())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Searches the trace index with a structured filter (no SQL text from the caller). It runs on a worker
/// thread, holds no store lock, and opens `flight.db` read-only; the index thread is its only writer.
#[tauri::command]
pub async fn flight_search(app: AppHandle, filter: SearchFilter) -> Result<SearchOut, String> {
    let data = crate::data_dir::app_data_dir(&app).path;
    tauri::async_runtime::spawn_blocking(move || host::search_index(&data, &filter))
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompareRequest {
    pub a: String,
    pub b: String,
}

/// Compares two finished runs from the trace index and their records: the rows in one fixed group order and
/// the first difference. Runs on a worker thread outside the store locks, reads `flight.db` read-only and
/// never indexes on demand. The same run twice is refused.
#[tauri::command]
pub async fn flight_compare(
    app: AppHandle,
    settings: State<'_, Mutex<SettingsStore>>,
    usage: State<'_, Mutex<crate::usage::UsageStore>>,
    request: CompareRequest,
) -> Result<host::CompareOut, String> {
    let data = crate::data_dir::app_data_dir(&app).path;
    let a = crate::run::checked_run_dir(&data, &request.a)?;
    let b = crate::run::checked_run_dir(&data, &request.b)?;
    let claude = settings.lock().map_err(|e| e.to_string())?.claude().clone();
    // The usage readings of the accounts of both runs, copied under a short lock.
    let mut histories: std::collections::HashMap<String, host::UsageHistory> =
        std::collections::HashMap::new();
    {
        let usage = usage.lock().map_err(|e| e.to_string())?;
        for dir in [&a, &b] {
            let account = crate::run::folder::read_json_value(&dir.join("meta.json"))
                .and_then(|m| m["account"]["id"].as_str().map(str::to_string));
            if let Some(account) = account {
                let config =
                    crate::store::settings_store::resolved_account_config(&claude, &account).path;
                histories
                    .entry(config.clone())
                    .or_insert_with(|| usage.snapshot(&config).history);
            }
        }
    }
    tauri::async_runtime::spawn_blocking(move || {
        host::compare_runs(&data, &claude, &a, &b, &|config| {
            histories.get(config).cloned().unwrap_or_default()
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The runs of one eval group with their measures and the min / median / max over the finished ones. Read-only,
/// on a worker thread, outside the store locks.
#[tauri::command]
pub async fn flight_eval_results(
    app: AppHandle,
    group: String,
) -> Result<host::EvalResults, String> {
    let data = crate::data_dir::app_data_dir(&app).path;
    tauri::async_runtime::spawn_blocking(move || {
        host::eval_results(&data, &group, &crate::run::checks_running)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// What one finished run used of the account's five-hour window (an estimate), or null when not known.
#[tauri::command]
pub async fn flight_run_share(
    app: AppHandle,
    settings: State<'_, Mutex<SettingsStore>>,
    usage: State<'_, Mutex<crate::usage::UsageStore>>,
    run_dir: String,
) -> Result<Option<f64>, String> {
    let data = crate::data_dir::app_data_dir(&app).path;
    let dir = crate::run::checked_run_dir(&data, &run_dir)?;
    let claude = settings.lock().map_err(|e| e.to_string())?.claude().clone();
    let account = crate::run::folder::read_json_value(&dir.join("meta.json"))
        .and_then(|m| m["account"]["id"].as_str().map(str::to_string));
    let history = match account {
        Some(account) => {
            let config =
                crate::store::settings_store::resolved_account_config(&claude, &account).path;
            let snapshot = usage.lock().map_err(|e| e.to_string())?.snapshot(&config);
            Some((config, snapshot.history))
        }
        None => None,
    };
    tauri::async_runtime::spawn_blocking(move || {
        host::run_five_hour_share(&claude, &dir, &|config| match &history {
            Some((known, history)) if known == config => history.clone(),
            _ => Vec::new(),
        })
    })
    .await
    .map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPreviewRequest {
    #[serde(flatten)]
    pub target: Target,
    #[serde(flatten)]
    pub export: host::ExportRequest,
    /// Empty or missing: the endpoint of Settings.
    pub endpoint: Option<String>,
}

/// Builds the Export payload for a Trace and keeps its bytes. Reads on a worker thread, holds no store lock.
#[tauri::command]
pub async fn flight_export_preview(
    app: AppHandle,
    settings: State<'_, Mutex<SettingsStore>>,
    cache: State<'_, FlightCache>,
    export: State<'_, host::ExportState>,
    request: ExportPreviewRequest,
) -> Result<host::ExportPreview, String> {
    let (claude, default_endpoint) = {
        let guard = settings.lock().map_err(|e| e.to_string())?;
        (
            guard.claude().clone(),
            guard.observe().export_endpoint.clone(),
        )
    };
    let query = query_for(&app, &request.target)?;
    let cache = cache.inner().clone();
    let state = export.inner().clone();
    let endpoint = request
        .endpoint
        .filter(|e| !e.trim().is_empty())
        .unwrap_or(default_endpoint);
    tauri::async_runtime::spawn_blocking(move || {
        host::export_preview(
            &claude,
            &query,
            &cache,
            &Opens::default(),
            &state,
            &request.export,
            &endpoint,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSendRequest {
    pub digest: String,
    pub endpoint: String,
}

/// Posts the previewed bytes. An unknown digest is refused.
#[tauri::command]
pub async fn flight_export_send(
    export: State<'_, host::ExportState>,
    request: ExportSendRequest,
) -> Result<host::ExportSent, String> {
    let state = export.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        host::export_send(
            &state,
            &host::export_agent(),
            &request.digest,
            &request.endpoint,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSaveRequest {
    pub digest: String,
    pub path: String,
}

/// Writes the previewed bytes to the picked file.
#[tauri::command]
pub async fn flight_export_save(
    export: State<'_, host::ExportState>,
    request: ExportSaveRequest,
) -> Result<(), String> {
    let state = export.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        host::export_save(&state, &request.digest, &PathBuf::from(request.path))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The replay track of a Trace: one tick per Event, no path. Read-only, on a worker thread.
#[tauri::command]
pub async fn flight_replay_track(
    app: AppHandle,
    settings: State<'_, Mutex<SettingsStore>>,
    cache: State<'_, FlightCache>,
    target: Target,
) -> Result<host::ReplayTrack, String> {
    let claude = settings.lock().map_err(|e| e.to_string())?.claude().clone();
    let query = query_for(&app, &target)?;
    let cache = cache.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        host::replay_track(&claude, &query, &cache, &Opens::default())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayFileRequest {
    #[serde(flatten)]
    pub target: Target,
    pub event_id: String,
}

/// The file a `file.change` Event changed, rebuilt as it was after that edit (read-only; nothing recorded runs).
#[tauri::command]
pub async fn flight_replay_file(
    app: AppHandle,
    settings: State<'_, Mutex<SettingsStore>>,
    cache: State<'_, FlightCache>,
    request: ReplayFileRequest,
) -> Result<host::ReplayFile, String> {
    let claude = settings.lock().map_err(|e| e.to_string())?.claude().clone();
    let query = query_for(&app, &request.target)?;
    let cache = cache.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        host::replay_file(
            &claude,
            &query,
            &cache,
            &Opens::default(),
            &request.event_id,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RerunTarget {
    pub run_dir: String,
    pub pr: String,
}

/// A run's folder, checked, and its account's policy (not `off`).
fn rerun_run_dir(
    app: &AppHandle,
    settings: &State<'_, Mutex<SettingsStore>>,
    run_dir: &str,
) -> Result<(PathBuf, PathBuf), String> {
    let data = crate::data_dir::app_data_dir(app).path;
    let dir = crate::run::checked_run_dir(&data, run_dir)?;
    let claude = settings.lock().map_err(|e| e.to_string())?.claude().clone();
    host::replay_gate(&claude, &dir)?;
    Ok((data, dir))
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RerunInfoOut {
    #[serde(flatten)]
    pub info: crate::run::rerun::Info,
    /// A re-run runs somewhere in the app.
    pub running: bool,
}

/// What the re-run confirmation shows, or why there is none (read-only).
#[tauri::command]
pub async fn flight_replay_check_info(
    app: AppHandle,
    settings: State<'_, Mutex<SettingsStore>>,
    request: RerunTarget,
) -> Result<RerunInfoOut, String> {
    let (_, dir) = rerun_run_dir(&app, &settings, &request.run_dir)?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut info = crate::run::rerun::info(&dir, &request.pr);
        let running = crate::run::rerun::running();
        if info.reason.is_none() && running {
            info.reason = Some(crate::run::rerun::ALREADY_RUNNING.to_string());
        }
        RerunInfoOut { info, running }
    })
    .await
    .map_err(|e| e.to_string())
}

/// Starts a check re-run at the recorded commit in a scratch worktree (one at a time in the app).
#[tauri::command]
pub async fn flight_replay_check(
    app: AppHandle,
    settings: State<'_, Mutex<SettingsStore>>,
    request: RerunTarget,
) -> Result<crate::run::rerun::RerunRecord, String> {
    let (data, dir) = rerun_run_dir(&app, &settings, &request.run_dir)?;
    tauri::async_runtime::spawn_blocking(move || crate::run::rerun::start(&data, &dir, &request.pr))
        .await
        .map_err(|e| e.to_string())?
}

/// Stops the running re-run; its result is `stopped` and its scratch worktree is removed.
#[tauri::command]
pub fn flight_replay_check_stop() -> Result<(), String> {
    crate::run::rerun::stop()
}

/// The re-run records of a run (newest first per PR).
#[tauri::command]
pub async fn flight_replay_checks(
    app: AppHandle,
    run_dir: String,
) -> Result<Vec<crate::run::rerun::RerunRecord>, String> {
    let data = crate::data_dir::app_data_dir(&app).path;
    let dir = crate::run::checked_run_dir(&data, &run_dir)?;
    let run_id = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || crate::run::rerun::list(&data, &run_id))
        .await
        .map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RerunLogRequest {
    pub run_dir: String,
    pub pr: String,
    pub n: u32,
}

/// A re-run's log (the last 256 KiB of it).
#[tauri::command]
pub async fn flight_replay_check_log(
    app: AppHandle,
    request: RerunLogRequest,
) -> Result<String, String> {
    let data = crate::data_dir::app_data_dir(&app).path;
    let dir = crate::run::checked_run_dir(&data, &request.run_dir)?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::run::rerun::read_log(&data, &dir, &request.pr, request.n)
    })
    .await
    .map_err(|e| e.to_string())?
}
