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
