//! AgentFlight commands: the Session Inspector reads one run's or one
//! session's Claude transcripts. Both run on a worker thread, resolve the
//! capture policy before any file is opened, and hold no store lock while
//! they read.

use crate::flight::host::{self, ContentOut, Opens, Query};
use crate::flight::schema::SourceRef;
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
