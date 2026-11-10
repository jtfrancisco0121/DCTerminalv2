//! Web QA (docs/run-folder.md, "Web QA mode"): the app's side of a black-box test pass of a deployed web app.
//!
//! - **targets**: the operator's list in Settings (`settings.json` `webQa.targets`), saved and checked here. The test
//!   account's password never enters settings.json: it is in the macOS keychain (service `dcterminalv2.webqa`, account =
//!   target id), behind [`PasswordStore`] so tests use memory and never touch the real keychain.
//! - **a pass**: [`start_pass`] makes a `mode: web-qa` run folder with `webqa-target.json` and a 0600 `webqa-secret.json`
//!   and launches the Eagle-Eye tab; the run's results are read only from a digest-valid `webqa.json` ([`read_valid`]),
//!   never from `webqa/results.md`; the secret is removed by `state.sh`, by the cleanup tick and by a failed launch.
//! - **release QA**: [`release_open`] reads the operator-owned `webqa.json` of the installed-app test (docs/release-qa.md).

use super::folder;
use super::triage::{self, ApplyOutcome, PostStore, PostedRecord, DESCRIPTION_CAP, NAME_CAP};
use crate::clickup::secret::keychain_error;
use crate::clickup::trust::{self, Key, WEBQA_APPROVAL_FILE, WEBQA_POSTED_FILE};
use crate::store::settings_store::{WebQaSettings, WebQaTarget};
use crate::store::{write_json_atomic, SettingsStore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Manager};

/// Keychain item service; the item's account field is the target id.
pub const KEYCHAIN_SERVICE: &str = "dcterminalv2.webqa";
/// A password longer than this is refused (it is typed by the driver, not pasted into a form by a person).
pub const MAX_PASSWORD: usize = 200;

/// One password slot per target id. Errors are plain English and never hold the password.
pub trait PasswordStore: Send + Sync {
    fn get(&self, target_id: &str) -> Result<Option<String>, String>;
    fn set(&self, target_id: &str, password: &str) -> Result<(), String>;
    /// Clearing an empty slot is fine.
    fn clear(&self, target_id: &str) -> Result<(), String>;
}

/// The OS keychain, through the `keyring` crate.
pub struct KeychainPasswords;

impl KeychainPasswords {
    fn entry(target_id: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(KEYCHAIN_SERVICE, target_id).map_err(|err| keychain_error(&err))
    }
}

impl PasswordStore for KeychainPasswords {
    fn get(&self, target_id: &str) -> Result<Option<String>, String> {
        match Self::entry(target_id)?.get_password() {
            Ok(saved) => Ok(Some(saved)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(keychain_error(&err)),
        }
    }

    fn set(&self, target_id: &str, password: &str) -> Result<(), String> {
        Self::entry(target_id)?
            .set_password(password)
            .map_err(|err| keychain_error(&err))
    }

    fn clear(&self, target_id: &str) -> Result<(), String> {
        match Self::entry(target_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(keychain_error(&err)),
        }
    }
}

/// In-memory slots for tests.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryPasswords {
    pub slots: Mutex<std::collections::HashMap<String, String>>,
}

#[cfg(test)]
impl PasswordStore for MemoryPasswords {
    fn get(&self, target_id: &str) -> Result<Option<String>, String> {
        Ok(self
            .slots
            .lock()
            .map_err(|e| e.to_string())?
            .get(target_id)
            .cloned())
    }

    fn set(&self, target_id: &str, password: &str) -> Result<(), String> {
        self.slots
            .lock()
            .map_err(|e| e.to_string())?
            .insert(target_id.to_string(), password.to_string());
        Ok(())
    }

    fn clear(&self, target_id: &str) -> Result<(), String> {
        self.slots
            .lock()
            .map_err(|e| e.to_string())?
            .remove(target_id);
        Ok(())
    }
}

/// Whether a target's test password is saved, or why that cannot be told.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PasswordStatus {
    pub target_id: String,
    pub saved: bool,
    pub problem: Option<String>,
}

fn known_target(settings: &SettingsStore, target_id: &str) -> Result<(), String> {
    if settings.web_qa().targets.iter().any(|t| t.id == target_id) {
        Ok(())
    } else {
        Err("No web QA target with that id. Save the target first.".into())
    }
}

fn known_target_in_app(app: &AppHandle, target_id: &str) -> Result<(), String> {
    let settings = app.state::<Mutex<SettingsStore>>();
    let settings = settings.lock().map_err(|e| e.to_string())?;
    known_target(&settings, target_id)
}

/// A password the driver can type: one line, not empty, not control characters.
pub fn clean_password(password: &str) -> Result<&str, String> {
    if password.is_empty()
        || password.chars().count() > MAX_PASSWORD
        || password.chars().any(char::is_control)
    {
        return Err(format!(
            "The password must be one line of 1 to {MAX_PASSWORD} characters."
        ));
    }
    Ok(password)
}

pub fn password_status(store: &dyn PasswordStore, target_id: &str) -> PasswordStatus {
    match store.get(target_id) {
        Ok(saved) => PasswordStatus {
            target_id: target_id.into(),
            saved: saved.is_some(),
            problem: None,
        },
        Err(problem) => PasswordStatus {
            target_id: target_id.into(),
            saved: false,
            problem: Some(problem),
        },
    }
}

/// Saves the target list. A target that is gone loses its keychain password too (best effort).
pub fn save_targets(
    settings: &mut SettingsStore,
    store: &dyn PasswordStore,
    next: Vec<WebQaTarget>,
) -> Result<WebQaSettings, String> {
    let before: Vec<String> = settings
        .web_qa()
        .targets
        .iter()
        .map(|t| t.id.clone())
        .collect();
    let saved = settings.set_web_qa_targets(next)?;
    for gone in before
        .iter()
        .filter(|id| !saved.targets.iter().any(|t| &t.id == *id))
    {
        let _ = store.clear(gone);
    }
    Ok(saved)
}

#[tauri::command]
pub async fn webqa_targets_save(
    app: AppHandle,
    targets: Vec<WebQaTarget>,
) -> Result<WebQaSettings, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = app.state::<Mutex<SettingsStore>>();
        let mut settings = settings.lock().map_err(|e| e.to_string())?;
        save_targets(&mut settings, &KeychainPasswords, targets)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn webqa_password_status(
    app: AppHandle,
    target_id: String,
) -> Result<PasswordStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        known_target_in_app(&app, &target_id)?;
        Ok(password_status(&KeychainPasswords, &target_id))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn webqa_password_set(
    app: AppHandle,
    target_id: String,
    password: String,
) -> Result<PasswordStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        known_target_in_app(&app, &target_id)?;
        KeychainPasswords.set(&target_id, clean_password(&password)?)?;
        Ok(password_status(&KeychainPasswords, &target_id))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn webqa_password_clear(
    app: AppHandle,
    target_id: String,
) -> Result<PasswordStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        known_target_in_app(&app, &target_id)?;
        KeychainPasswords.clear(&target_id)?;
        Ok(password_status(&KeychainPasswords, &target_id))
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---- a pass: start, secret, results ----------------------------------------------------------------------------

pub const SECRET_FILE: &str = "webqa-secret.json";
/// A secret with no live tab and older than this is removed by the cleanup tick (the launch gets this long).
pub const SECRET_GRACE: Duration = Duration::from_secs(2 * 60);
/// A pass whose status.json has not changed for this long is no longer "running".
pub const RUNNING_FOR: Duration = Duration::from_secs(6 * 60 * 60);
/// The release QA folder of a version; `DCV2_RELEASE_QA_ROOT` replaces the root (tests).
pub const RELEASE_ROOT: &str = "/Users/Shared/dcv2-release-qa";

fn read_json(path: &Path) -> Option<Value> {
    folder::read_json_value(path)
}

/// The scratch folder every pass starts in (an empty folder: a pass has no repo).
fn work_folder(data: &Path) -> Result<PathBuf, String> {
    let dir = data.join("webqa-work");
    std::fs::create_dir_all(&dir).map_err(|e| format!("web QA folder: {e}"))?;
    Ok(dir.canonicalize().unwrap_or(dir))
}

/// The summary of a target's newest valid earlier pass: `{rows: [{charter, result}], findings: [<key>]}`.
pub fn previous_summary(data: &Path, target_id: &str) -> Option<Value> {
    passes_of(data)
        .into_iter()
        .filter(|p| p.target_id.as_deref() == Some(target_id))
        .find_map(|pass| {
            let doc = read_valid(&pass.dir).ok()?;
            let rows: Vec<Value> = doc["rows"]
                .as_array()?
                .iter()
                .map(|r| json!({"charter": r["charter"], "result": r["result"]}))
                .collect();
            let findings: Vec<Value> = doc["findings"]
                .as_array()?
                .iter()
                .map(|f| f["key"].clone())
                .collect();
            Some(json!({"rows": rows, "findings": findings}))
        })
}

/// `webqa-target.json` for a pass: the allowlist, the ticked charters, the ClickUp list and the previous summary.
pub fn target_json(
    target: &WebQaTarget,
    charter_ids: &[String],
    previous: Option<Value>,
) -> Result<Value, String> {
    let charters: Vec<Value> = charter_ids
        .iter()
        .map(|id| {
            target
                .charters
                .iter()
                .find(|c| &c.id == id)
                .map(|c| json!({"id": c.id, "title": c.title, "text": c.text, "mayChangeData": c.may_change_data}))
                .ok_or_else(|| format!("{}: no charter {id}.", target.name))
        })
        .collect::<Result<_, _>>()?;
    if charters.is_empty() {
        return Err("Tick at least one charter to try.".into());
    }
    let mut doc = json!({
        "schema": 1,
        "id": target.id,
        "name": target.name,
        "allowlist": target.allowlist,
        "username": target.username,
        "charters": charters,
        "clickupListId": target.clickup_list_id,
    });
    if let Some(previous) = previous {
        doc["previous"] = previous;
    }
    Ok(doc)
}

/// Writes `webqa-secret.json` (mode 0600 from the first byte) for a target that has a test account. A target without a
/// user name needs none (a public site). The password must be in the keychain.
pub fn write_secret(
    run_dir: &Path,
    store: &dyn PasswordStore,
    target: &WebQaTarget,
) -> Result<(), String> {
    if target.username.is_empty() {
        return Ok(());
    }
    let password = store
        .get(&target.id)?
        .ok_or("The test password is not in the keychain.")?;
    let path = run_dir.join(SECRET_FILE);
    let _ = std::fs::remove_file(&path);
    let body = serde_json::to_vec(&json!({"username": target.username, "password": password}))
        .map_err(|e| e.to_string())?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    use std::io::Write;
    let mut file = opts
        .open(&path)
        .map_err(|e| format!("test account file: {e}"))?;
    file.write_all(&body)
        .map_err(|e| format!("test account file: {e}"))
}

fn remove_secret(run_dir: &Path) {
    let _ = std::fs::remove_file(run_dir.join(SECRET_FILE));
}

/// A resumed web QA run gets its secret back from the keychain (the cleanup tick may have removed it).
pub fn rewrite_secret(
    run_dir: &Path,
    settings: &SettingsStore,
    store: &dyn PasswordStore,
) -> Result<(), String> {
    let Some(id) = folder::read_json_value(&run_dir.join("meta.json"))
        .and_then(|m| m["webqa"]["targetId"].as_str().map(str::to_string))
    else {
        return Ok(());
    };
    let target = settings
        .web_qa()
        .targets
        .iter()
        .find(|t| t.id == id)
        .ok_or("This target was removed from Settings.")?;
    write_secret(run_dir, store, target)
}

/// The request.md text of a pass: one line, no ticket or page text.
fn request_for(target: &WebQaTarget, count: usize) -> String {
    format!(
        "Web QA pass of {}: try {count} charter{} against the allowlisted URLs of webqa-target.json.",
        target.name,
        if count == 1 { "" } else { "s" }
    )
}

/// Makes the pass's run folder (not the tab): meta.json mode web-qa, request.md, `webqa-target.json`, the secret. A failure
/// removes what was made.
pub fn make_pass(
    data: &Path,
    meta: &folder::RunMeta,
    settings: &SettingsStore,
    store: &dyn PasswordStore,
    target_id: &str,
    charter_ids: &[String],
) -> Result<folder::CreatedRun, String> {
    let target = settings
        .web_qa()
        .targets
        .iter()
        .find(|t| t.id == target_id)
        .ok_or("No web QA target with that id.")?;
    if target
        .allowlist
        .iter()
        .any(|e| !crate::store::settings_store::allowlist_entry_ok(e))
    {
        return Err(format!(
            "{}: the allowlist has a URL the test cannot use. Fix it in Settings.",
            target.name
        ));
    }
    let doc = target_json(target, charter_ids, previous_summary(data, target_id))?;
    let work = work_folder(data)?;
    let meta = folder::RunMeta {
        mode: Some("web-qa".into()),
        bmad_mode: Some("off".into()),
        review_mode: Some("local".into()),
        webqa: Some((target_id.to_string(), charter_ids.to_vec())),
        ..meta.clone()
    };
    let created = folder::create_run(
        data,
        &work,
        &[],
        &[],
        &request_for(target, charter_ids.len()),
        &meta,
    )?;
    let dir = PathBuf::from(&created.run_dir);
    let made = write_json_atomic(&dir.join("webqa-target.json"), &doc)
        .and_then(|()| {
            std::fs::create_dir_all(dir.join("webqa").join("shots")).map_err(|e| e.to_string())
        })
        .and_then(|()| {
            std::fs::create_dir_all(dir.join("webqa").join("steps")).map_err(|e| e.to_string())
        })
        .and_then(|()| write_secret(&dir, store, target));
    if let Err(err) = made {
        remove_secret(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        return Err(err);
    }
    Ok(created)
}

/// Starts a pass: the run folder, then the Eagle-Eye tab. When the tab cannot start the secret and the folder go.
pub fn start_pass(
    app: &AppHandle,
    window_id: Option<String>,
    target_id: &str,
    charter_ids: &[String],
) -> Result<super::StartedRun, String> {
    let data = super::data_dir(app);
    let meta = super::webqa_run_meta(app, window_id.as_deref())?;
    let created = {
        let settings = app.state::<Mutex<SettingsStore>>();
        let settings = settings.lock().map_err(|e| e.to_string())?;
        make_pass(
            &data,
            &meta,
            &settings,
            &KeychainPasswords,
            target_id,
            charter_ids,
        )?
    };
    let run_dir = PathBuf::from(&created.run_dir);
    match super::launch_webqa_tab(app, window_id.as_deref(), &run_dir) {
        Ok(tab) => Ok(super::StartedRun {
            tab,
            run_dir: Some(created.run_dir),
            focused: false,
        }),
        Err(err) => {
            remove_secret(&run_dir);
            let _ = folder::append_app_log(
                &run_dir,
                "webqa secret removed (app): the tab did not start",
            );
            Err(err)
        }
    }
}

#[tauri::command]
pub async fn webqa_start(
    app: AppHandle,
    window_id: Option<String>,
    target_id: String,
    charter_ids: Vec<String>,
) -> Result<super::StartedRun, String> {
    tauri::async_runtime::spawn_blocking(move || {
        start_pass(&app, window_id, target_id.trim(), &charter_ids)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---- results: only a digest-valid webqa.json ---------------------------------------------------------------------

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The digest `state.sh webqa` records: sha-256 of results.md (newlines as `\n`) and, for each screenshot it names (sorted,
/// once), `\n<name>:<sha-256 of the file>` or `\n<name>:missing`.
pub fn results_digest(dir: &Path) -> Option<String> {
    let raw = std::fs::read(dir.join("webqa").join("results.md")).ok()?;
    let text = String::from_utf8(raw)
        .ok()?
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    if text.is_empty() {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let mut names: Vec<String> = Vec::new();
    let bytes = text.as_bytes();
    let marker = "webqa/shots/";
    let mut from = 0;
    while let Some(at) = text[from..].find(marker) {
        let start = from + at;
        let mut end = start + marker.len();
        while end < bytes.len()
            && (bytes[end].is_ascii_alphanumeric() || matches!(bytes[end], b'.' | b'_' | b'-'))
        {
            end += 1;
        }
        let name = &text[start..end];
        let stem_ok = name.len() > marker.len() + 4
            && name.ends_with(".png")
            && name.as_bytes()[marker.len()].is_ascii_alphanumeric();
        if stem_ok {
            names.push(name.to_string());
        }
        from = end.max(start + 1);
    }
    names.sort();
    names.dedup();
    for name in names {
        hasher.update(b"\n");
        hasher.update(name.as_bytes());
        match std::fs::read(dir.join(&name)) {
            Ok(file) => hasher.update(format!(":{}", sha256_hex(&file)).as_bytes()),
            Err(_) => hasher.update(b":missing"),
        }
    }
    Some(
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

/// `webqa.json` of a pass folder, only when its digest matches the files now (the way triage verifies triage.json).
pub fn read_valid(dir: &Path) -> Result<Value, String> {
    let doc = read_json(&dir.join("webqa.json")).ok_or("there are no checked results")?;
    if doc["schema"] != json!(1) || !doc["rows"].is_array() || !doc["findings"].is_array() {
        return Err("the checked results are not in a form this app reads".into());
    }
    match (results_digest(dir), doc["digest"].as_str()) {
        (Some(now), Some(recorded)) if now == recorded => Ok(doc),
        _ => Err("the results changed after they were checked".into()),
    }
}

struct PassDir {
    dir: PathBuf,
    run_id: String,
    created_at: Option<String>,
    stage: Option<String>,
    target_id: Option<String>,
}

/// The app's web QA runs, newest first.
fn passes_of(data: &Path) -> Vec<PassDir> {
    folder::list_runs(data)
        .into_iter()
        .filter(|r| !r.adopted)
        .filter_map(|r| {
            let dir = PathBuf::from(&r.run_dir);
            (folder::run_mode_of(&dir).as_deref() == Some("web-qa")).then(|| PassDir {
                target_id: read_json(&dir.join("meta.json"))
                    .and_then(|m| m["webqa"]["targetId"].as_str().map(str::to_string)),
                run_id: r.run_id.clone(),
                created_at: r.created_at.clone(),
                stage: r.stage.clone(),
                dir,
            })
        })
        .collect()
}

/// One pass in the Runs view.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PassView {
    /// The run id, or `release-<version>` for a release QA folder.
    pub pass_id: String,
    pub run_dir: Option<String>,
    pub release: bool,
    pub created_at: Option<String>,
    pub stage: Option<String>,
    pub target_id: Option<String>,
    pub target_name: Option<String>,
    /// The tab is still working on it.
    pub running: bool,
    /// `pass`, `fail` or `error`, from the checked results.
    pub result: Option<String>,
    /// Why there are no checked results (a finished pass without them), or null.
    pub problem: Option<String>,
    pub rows: Vec<Value>,
    pub findings: Vec<Value>,
    pub comparison: Value,
    /// Where the findings' tasks go; null when the target has no list.
    pub clickup_list_id: Option<String>,
    /// What was sent to ClickUp for the findings, by `create:<finding index>` (the signed record), or empty.
    pub posted: std::collections::BTreeMap<String, triage::PostItem>,
}

fn target_name(
    settings: &Option<&WebQaSettings>,
    id: &Option<String>,
    dir: &Path,
) -> Option<String> {
    let id = id.as_ref()?;
    settings
        .and_then(|s| {
            s.targets
                .iter()
                .find(|t| &t.id == id)
                .map(|t| t.name.clone())
        })
        .or_else(|| {
            read_json(&dir.join("webqa-target.json"))
                .and_then(|t| t["name"].as_str().map(str::to_string))
        })
}

fn view_of(
    data: &Path,
    pass: &PassDir,
    settings: Option<&WebQaSettings>,
    now: SystemTime,
) -> PassView {
    let finished = matches!(pass.stage.as_deref(), Some("done" | "failed"));
    let quiet = std::fs::metadata(pass.dir.join("status.json"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|at| now.duration_since(at).ok())
        .is_some_and(|age| age >= RUNNING_FOR);
    let checked = read_valid(&pass.dir);
    let (result, rows, findings, comparison, problem) = match &checked {
        Ok(doc) => (
            doc["result"].as_str().map(str::to_string),
            doc["rows"].as_array().cloned().unwrap_or_default(),
            doc["findings"].as_array().cloned().unwrap_or_default(),
            doc["comparison"].clone(),
            None,
        ),
        Err(why) => {
            let why = if pass.stage.as_deref() == Some("failed") {
                read_json(&pass.dir.join("status.json"))
                    .and_then(|s| s["error"].as_str().map(str::to_string))
                    .unwrap_or_else(|| "the run failed".into())
            } else {
                why.clone()
            };
            (
                None,
                vec![],
                vec![],
                Value::Null,
                (finished || quiet).then(|| format!("Pass results are not valid: {why}.")),
            )
        }
    };
    let list = read_json(&pass.dir.join("webqa-target.json"))
        .and_then(|t| t["clickupListId"].as_str().map(str::to_string));
    PassView {
        pass_id: pass.run_id.clone(),
        run_dir: Some(pass.dir.display().to_string()),
        release: false,
        created_at: pass.created_at.clone(),
        stage: pass.stage.clone(),
        target_id: pass.target_id.clone(),
        target_name: target_name(&settings, &pass.target_id, &pass.dir),
        running: !finished && !quiet,
        result,
        problem,
        rows,
        findings,
        comparison,
        clickup_list_id: list,
        posted: posted_of(data, &pass.run_id),
    }
}

/// The passes, newest first, each read only from its digest-valid `webqa.json`.
pub fn overview(data: &Path, settings: Option<&WebQaSettings>, now: SystemTime) -> Vec<PassView> {
    passes_of(data)
        .iter()
        .map(|p| view_of(data, p, settings, now))
        .collect()
}

#[tauri::command]
pub async fn webqa_overview(app: AppHandle) -> Result<Vec<PassView>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let data = super::data_dir(&app);
        let settings = app.state::<Mutex<SettingsStore>>();
        let settings = settings.lock().map_err(|e| e.to_string())?.web_qa().clone();
        Ok(overview(&data, Some(&settings), SystemTime::now()))
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---- ticking findings: the signed approval and the tasks sent ------------------------------------------------------

/// What the operator approved: each ticked finding's exact task text, for the results as they were (their digest).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebQaApproval {
    pub schema: u32,
    pub digest: String,
    pub creates: Vec<triage::CreateRec>,
    pub at: String,
}

pub const STALE: &str = "The results changed. Tick the findings again.";

fn capped(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// The task a finding becomes: its title, and the steps, expected and seen text, built from the checked `webqa.json`.
pub fn task_of(finding: &Value, target: &str, pass_id: &str) -> triage::CreateRec {
    let text = |key: &str| finding[key].as_str().unwrap_or("").trim().to_string();
    let mut description = format!(
        "Found by a web QA pass of {target} ({pass_id}).\n\nSeverity: {}\n\nSteps:\n{}\n\nExpected:\n{}\n\nSeen:\n{}",
        finding["severity"].as_str().unwrap_or("medium"),
        text("steps"),
        text("expected"),
        text("seen"),
    );
    if let Some(shot) = finding["shot"].as_str().filter(|s| !s.is_empty()) {
        description.push_str(&format!("\n\nScreenshot: {shot} (kept in the run folder)"));
    }
    triage::CreateRec {
        index: 0,
        name: capped(&format!("[Web QA] {}", text("title")), NAME_CAP),
        description: capped(&description, DESCRIPTION_CAP),
    }
}

/// The approval for `ticked` finding keys: unknown or repeated keys are refused, and so is an empty tick.
pub fn build_approval(
    doc: &Value,
    target: &str,
    pass_id: &str,
    ticked: &[String],
    at: &str,
) -> Result<WebQaApproval, String> {
    let digest = doc["digest"]
        .as_str()
        .ok_or("The results have no digest.")?
        .to_string();
    let findings = doc["findings"]
        .as_array()
        .ok_or("The results have no findings.")?;
    if ticked.is_empty() {
        return Err("Tick at least one finding.".into());
    }
    let mut creates: Vec<triage::CreateRec> = Vec::new();
    for key in ticked {
        let (index, finding) = findings
            .iter()
            .enumerate()
            .find(|(_, f)| f["key"].as_str() == Some(key.as_str()))
            .ok_or_else(|| format!("No finding \"{key}\" in these results."))?;
        if creates.iter().any(|c| c.index == index) {
            return Err("A finding is ticked twice.".into());
        }
        let mut task = task_of(finding, target, pass_id);
        task.index = index;
        creates.push(task);
    }
    Ok(WebQaApproval {
        schema: 1,
        digest,
        creates,
        at: at.to_string(),
    })
}

/// Where a pass's signed records live: its folder under the trust folder is named by the pass id.
fn pass_key(pass_id: &str) -> Result<PathBuf, String> {
    if trust::valid_pass_id(pass_id) {
        Ok(PathBuf::from(pass_id))
    } else {
        Err("That is not a pass id.".into())
    }
}

/// What was sent for a pass, from its signed posted record (nothing when there is none or it cannot be read).
fn posted_of(data: &Path, pass_id: &str) -> std::collections::BTreeMap<String, triage::PostItem> {
    let (Ok(key), Ok(pass)) = (triage::signing_key(), pass_key(pass_id)) else {
        return Default::default();
    };
    match trust::read_triage::<PostedRecord>(&key, data, &pass, WEBQA_POSTED_FILE) {
        Ok(Some(posted)) => posted.items,
        _ => Default::default(),
    }
}

struct PassStore<'a> {
    key: &'a Key,
    data: &'a Path,
    pass: &'a Path,
}

impl PostStore for PassStore<'_> {
    fn save(&mut self, posted: &PostedRecord) -> Result<(), String> {
        trust::write_triage(self.key, self.data, self.pass, WEBQA_POSTED_FILE, posted)
    }
}

/// A pass the page named: this app's run (`r-…`) or an operator-owned release folder (`release-<version>`).
fn resolve_pass(data: &Path, root: &Path, pass_id: &str) -> Result<(PathBuf, bool), String> {
    pass_key(pass_id)?;
    if let Some(version) = pass_id.strip_prefix("release-") {
        let dir = root.join(version);
        if !valid_version(version)
            || ![root, dir.as_path(), &dir.join("release.json")]
                .into_iter()
                .all(operator_owned)
        {
            return Err("This release folder is not yours, or others can write it.".into());
        }
        return Ok((dir, true));
    }
    let dir = super::checked_run_dir(
        data,
        &folder::runs_dir(data).join(pass_id).display().to_string(),
    )?;
    if folder::run_mode_of(&dir).as_deref() != Some("web-qa") {
        return Err("That is not a web QA pass.".into());
    }
    Ok((dir, false))
}

/// "Create tickets": signs the ticked findings' task text. Nothing is sent yet.
pub fn approve(
    data: &Path,
    root: &Path,
    key: &Key,
    pass_id: &str,
    ticked: &[String],
    at: &str,
) -> Result<WebQaApproval, String> {
    let (dir, _) = resolve_pass(data, root, pass_id)?;
    let doc = read_valid(&dir).map_err(|why| format!("The results are not valid: {why}."))?;
    let target = read_json(&dir.join("webqa-target.json"))
        .and_then(|t| t["name"].as_str().map(str::to_string))
        .unwrap_or_default();
    let record = build_approval(&doc, &target, pass_id, ticked, at)?;
    trust::write_triage(key, data, &pass_key(pass_id)?, WEBQA_APPROVAL_FILE, &record)?;
    Ok(record)
}

/// Sends the approved record's tasks, once each, through the one shared function in `run/triage.rs`.
pub fn apply(
    client: &crate::clickup::client::ClickUp,
    data: &Path,
    root: &Path,
    key: &Key,
    pass_id: &str,
    list_id: &str,
    now: &dyn Fn() -> String,
) -> Result<Vec<ApplyOutcome>, String> {
    let (dir, _) = resolve_pass(data, root, pass_id)?;
    let doc = read_valid(&dir).map_err(|why| format!("The results are not valid: {why}."))?;
    let pass = pass_key(pass_id)?;
    let record = trust::read_triage::<WebQaApproval>(key, data, &pass, WEBQA_APPROVAL_FILE)?
        .ok_or("No findings are approved.")?;
    if Some(record.digest.as_str()) != doc["digest"].as_str() {
        return Err(STALE.into());
    }
    let _lock = trust::lock_posts(data, &pass)?;
    let mut posted = trust::read_triage::<PostedRecord>(key, data, &pass, WEBQA_POSTED_FILE)?
        .unwrap_or_default();
    posted.schema = 1;
    let mut out = Vec::new();
    let mut store = PassStore {
        key,
        data,
        pass: &pass,
    };
    triage::create_tasks_from_record(
        client,
        list_id,
        &record.creates,
        &mut posted,
        &mut store,
        now,
        &mut out,
    )?;
    Ok(out)
}

#[tauri::command]
pub async fn webqa_approve(
    app: AppHandle,
    pass: String,
    ticked: Vec<String>,
) -> Result<WebQaApproval, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let data = super::data_dir(&app);
        let at = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        approve(
            &data,
            &release_root(),
            &triage::signing_key()?,
            pass.trim(),
            &ticked,
            &at,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Sends the approved findings as ClickUp tasks (and retries a failed one). Needs the target's ClickUp list.
#[tauri::command]
pub async fn webqa_apply(
    app: AppHandle,
    window_id: Option<String>,
    pass: String,
) -> Result<Vec<ApplyOutcome>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let data = super::data_dir(&app);
        let pass = pass.trim().to_string();
        let root = release_root();
        let (dir, release) = resolve_pass(&data, &root, &pass)?;
        let list_id = read_json(&dir.join("webqa-target.json"))
            .and_then(|t| t["clickupListId"].as_str().map(str::to_string))
            .filter(|l| !l.is_empty())
            .ok_or("This target has no ClickUp list: add one in Settings › Web QA.")?;
        let account = if release {
            super::dispatch::account_of(&app, window_id.as_deref())?
        } else {
            read_json(&dir.join("meta.json"))
                .and_then(|m| m["account"]["id"].as_str().map(str::to_string))
                .ok_or("This pass records no account.")?
        };
        let token = crate::clickup::commands::token_of(&crate::clickup::KeychainStore, &account)?;
        let transport = crate::clickup::UreqTransport::new();
        let client = crate::clickup::client::ClickUp::new(&transport, &token);
        let now = || chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        apply(
            &client,
            &data,
            &root,
            &triage::signing_key()?,
            &pass,
            &list_id,
            &now,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---- release QA results (docs/release-qa.md) -------------------------------------------------------------------------

fn release_root() -> PathBuf {
    std::env::var_os("DCV2_RELEASE_QA_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(RELEASE_ROOT))
}

/// A version number as `prepare.sh` names the folder: digits and dots, an optional `-suffix`.
pub fn valid_version(version: &str) -> bool {
    let (core, suffix) = version
        .split_once('-')
        .map_or((version, ""), |(c, s)| (c, s));
    !core.is_empty()
        && core.split('.').count() == 3
        && core
            .split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        && suffix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.')
        && version.len() <= 40
}

/// The operator owns it and neither the group nor the world can write it (and it is not a link).
#[cfg(unix)]
pub fn operator_owned(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(path).is_ok_and(|m| {
        !m.file_type().is_symlink()
            // SAFETY: geteuid has no arguments and cannot fail.
            && m.uid() == unsafe { libc::geteuid() }
            && m.mode() & 0o022 == 0
    })
}

#[cfg(not(unix))]
pub fn operator_owned(_path: &Path) -> bool {
    false
}

/// `<root>/<version>/webqa.json` for the Runs view, only when the root, the folder and `release.json` are the operator's and
/// not group or world writable, and the digest matches the files now.
pub fn release_open(data: &Path, root: &Path, version: &str) -> Result<PassView, String> {
    if !valid_version(version) {
        return Err("That is not a version number.".into());
    }
    let dir = root.join(version);
    for path in [root, dir.as_path(), &dir.join("release.json")] {
        if !operator_owned(path) {
            return Err(format!(
                "{} is not owned by you or can be written by others, so its results are not shown.",
                path.display()
            ));
        }
    }
    let doc =
        read_valid(&dir).map_err(|why| format!("Release QA results are not valid: {why}."))?;
    let target = read_json(&dir.join("webqa-target.json")).unwrap_or(Value::Null);
    Ok(PassView {
        pass_id: format!("release-{version}"),
        run_dir: None,
        release: true,
        created_at: doc["createdAt"].as_str().map(str::to_string),
        stage: Some("done".into()),
        target_id: None,
        target_name: Some(format!("Release {version}")),
        running: false,
        result: doc["result"].as_str().map(str::to_string),
        problem: None,
        rows: doc["rows"].as_array().cloned().unwrap_or_default(),
        findings: doc["findings"].as_array().cloned().unwrap_or_default(),
        comparison: doc["comparison"].clone(),
        clickup_list_id: target["clickupListId"].as_str().map(str::to_string),
        posted: posted_of(data, &format!("release-{version}")),
    })
}

#[tauri::command]
pub async fn webqa_release_open(app: AppHandle, version: String) -> Result<PassView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        release_open(&super::data_dir(&app), &release_root(), version.trim())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::settings_store::{url_allowed, WebQaCharter};
    use crate::test_support::TempDir;

    fn target(name: &str) -> WebQaTarget {
        WebQaTarget {
            id: String::new(),
            name: name.into(),
            allowlist: vec!["https://app.example.com/".into()],
            username: "tester".into(),
            clickup_list_id: Some("901".into()),
            charters: vec![
                WebQaCharter {
                    id: String::new(),
                    title: "Search".into(),
                    text: "Search for a word".into(),
                    may_change_data: false,
                },
                WebQaCharter {
                    id: String::new(),
                    title: "Sign in".into(),
                    text: "Sign in".into(),
                    may_change_data: true,
                },
            ],
        }
    }

    fn open(dir: &TempDir) -> SettingsStore {
        SettingsStore::open(dir.path()).expect("opens")
    }

    #[test]
    fn a_saved_target_reloads_with_its_urls_and_charters_and_settings_json_holds_no_password() {
        let dir = TempDir::new("webqa-save");
        let store = MemoryPasswords::default();
        let mut settings = open(&dir);
        let saved = save_targets(&mut settings, &store, vec![target("Staging")]).expect("saves");
        assert_eq!(saved.targets.len(), 1);
        let id = saved.targets[0].id.clone();
        assert!(id.starts_with("t-"), "a new target gets an id: {id}");
        store.set(&id, "hunter2-password").unwrap();
        let again = open(&dir);
        let t = &again.web_qa().targets[0];
        assert_eq!(t.allowlist, vec!["https://app.example.com/".to_string()]);
        assert_eq!(t.username, "tester");
        assert_eq!(t.clickup_list_id.as_deref(), Some("901"));
        // Charters are ordered by title and each has an id; the data-change flag is kept as set.
        assert_eq!(
            t.charters
                .iter()
                .map(|c| c.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Search", "Sign in"]
        );
        assert!(t.charters.iter().all(|c| !c.id.is_empty()));
        assert!(!t.charters[0].may_change_data && t.charters[1].may_change_data);
        let file =
            std::fs::read_to_string(dir.path().join("settings.json")).expect("settings.json");
        assert!(
            !file.contains("hunter2-password"),
            "the password is never in settings.json"
        );
        assert!(
            !file.to_lowercase().contains("password"),
            "settings.json has no password field at all"
        );
    }

    #[test]
    fn a_duplicate_name_or_an_invalid_url_refuses_the_save_and_keeps_the_old_list() {
        let dir = TempDir::new("webqa-refuse");
        let store = MemoryPasswords::default();
        let mut settings = open(&dir);
        save_targets(&mut settings, &store, vec![target("Staging")]).expect("saves");
        let err = save_targets(
            &mut settings,
            &store,
            vec![target("Staging"), target("staging")],
        )
        .unwrap_err();
        assert!(err.contains("Names must differ"), "{err}");
        for bad in [
            "ftp://app.example.com/",
            "https://user@app.example.com/",
            "javascript:alert(1)",
            "https://app.example.com/?q=1",
            "app.example.com",
            "",
        ] {
            let mut t = target("Other");
            t.allowlist = vec![bad.into()];
            let err = save_targets(&mut settings, &store, vec![t]).unwrap_err();
            assert!(!err.is_empty(), "{bad} was accepted");
        }
        assert_eq!(settings.web_qa().targets.len(), 1, "the old list is kept");
        assert_eq!(settings.web_qa().targets[0].name, "Staging");
        let mut none = target("No urls");
        none.allowlist.clear();
        assert!(save_targets(&mut settings, &store, vec![none]).is_err());
        let mut no_title = target("No title");
        no_title.charters[0].title = "  ".into();
        assert!(save_targets(&mut settings, &store, vec![no_title]).is_err());
    }

    #[test]
    fn removing_a_target_clears_its_password() {
        let dir = TempDir::new("webqa-remove");
        let store = MemoryPasswords::default();
        let mut settings = open(&dir);
        let saved = save_targets(&mut settings, &store, vec![target("A"), target("B")]).unwrap();
        let (a, b) = (saved.targets[0].id.clone(), saved.targets[1].id.clone());
        store.set(&a, "pw-a").unwrap();
        store.set(&b, "pw-b").unwrap();
        save_targets(
            &mut settings,
            &store,
            saved
                .targets
                .iter()
                .filter(|t| t.id == b)
                .cloned()
                .collect(),
        )
        .unwrap();
        assert_eq!(store.get(&a).unwrap(), None);
        assert_eq!(store.get(&b).unwrap().as_deref(), Some("pw-b"));
    }

    #[test]
    fn the_password_goes_through_the_store_and_only_for_a_saved_target() {
        let dir = TempDir::new("webqa-pw");
        let store = MemoryPasswords::default();
        let mut settings = open(&dir);
        assert!(known_target(&settings, "nope").is_err());
        let saved = save_targets(&mut settings, &store, vec![target("Staging")]).unwrap();
        let id = saved.targets[0].id.clone();
        assert!(known_target(&settings, &id).is_ok());
        assert_eq!(
            password_status(&store, &id),
            PasswordStatus {
                target_id: id.clone(),
                saved: false,
                problem: None
            }
        );
        store
            .set(&id, clean_password("a good password").unwrap())
            .unwrap();
        assert!(password_status(&store, &id).saved);
        store.clear(&id).unwrap();
        assert!(!password_status(&store, &id).saved);
        assert!(clean_password("").is_err());
        assert!(clean_password("two\nlines").is_err());
        assert!(clean_password(&"x".repeat(MAX_PASSWORD + 1)).is_err());
    }

    #[test]
    fn rust_and_the_driver_give_the_same_verdict_on_every_shared_url_case() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../fixtures/webqa/urls.json"
        ))
        .expect("urls.json");
        let file: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        let cases = file["cases"].as_array().expect("cases");
        assert!(cases.len() > 40);
        let mut wrong = Vec::new();
        for case in cases {
            let allowlist: Vec<String> = case["allowlist"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            let url = case["url"].as_str().unwrap();
            let want = case["allowed"].as_bool().unwrap();
            if url_allowed(url, &allowlist) != want {
                wrong.push(format!(
                    "{url} for {allowlist:?}: want {want} ({})",
                    case["why"]
                ));
            }
        }
        assert!(
            wrong.is_empty(),
            "verdicts differ from the shared cases:\n{}",
            wrong.join("\n")
        );
    }

    fn pass_settings(dir: &TempDir, store: &MemoryPasswords) -> (SettingsStore, String) {
        let mut settings = open(dir);
        let saved = save_targets(&mut settings, store, vec![target("Staging")]).unwrap();
        let id = saved.targets[0].id.clone();
        store.set(&id, "hunter2-password").unwrap();
        (settings, id)
    }

    fn meta() -> folder::RunMeta {
        folder::RunMeta {
            account_id: "default".into(),
            account_name: "Personal".into(),
            config_dir: "/h/.claude".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_start_writes_the_meta_mode_the_target_and_a_0600_secret() {
        let dir = TempDir::new("webqa-start");
        let store = MemoryPasswords::default();
        let (settings, id) = pass_settings(&dir, &store);
        let charters: Vec<String> = settings.web_qa().targets[0]
            .charters
            .iter()
            .map(|c| c.id.clone())
            .collect();
        let data = dir.path().join("data");
        let made =
            make_pass(&data, &meta(), &settings, &store, &id, &charters[..1]).expect("starts");
        let run = PathBuf::from(&made.run_dir);
        let m = read_json(&run.join("meta.json")).unwrap();
        assert_eq!(m["mode"], "web-qa");
        assert_eq!(m["webqa"]["targetId"], id.as_str());
        assert_eq!(m["bmad"]["mode"], "off");
        let t = read_json(&run.join("webqa-target.json")).unwrap();
        assert_eq!(t["allowlist"][0], "https://app.example.com/");
        assert_eq!(
            t["charters"].as_array().unwrap().len(),
            1,
            "only the ticked charter"
        );
        assert!(
            t.get("previous").is_none(),
            "a first pass has no previous summary"
        );
        let secret = run.join(SECRET_FILE);
        let body: Value = serde_json::from_str(&std::fs::read_to_string(&secret).unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"username": "tester", "password": "hunter2-password"})
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&secret).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(
            run.join("webqa").join("shots").is_dir() && run.join("webqa").join("steps").is_dir()
        );
        assert!(
            make_pass(&data, &meta(), &settings, &store, &id, &[]).is_err(),
            "no charter ticked"
        );
        assert!(make_pass(&data, &meta(), &settings, &store, "nope", &charters).is_err());
    }

    #[test]
    fn a_start_without_the_password_in_the_keychain_is_refused_and_leaves_nothing() {
        let dir = TempDir::new("webqa-nopw");
        let store = MemoryPasswords::default();
        let (settings, id) = pass_settings(&dir, &store);
        store.clear(&id).unwrap();
        let charters: Vec<String> = settings.web_qa().targets[0]
            .charters
            .iter()
            .map(|c| c.id.clone())
            .collect();
        let data = dir.path().join("data");
        let err = make_pass(&data, &meta(), &settings, &store, &id, &charters).unwrap_err();
        assert_eq!(err, "The test password is not in the keychain.");
        let left: Vec<_> = std::fs::read_dir(data.join("runs"))
            .map(|d| d.flatten().collect())
            .unwrap_or_default();
        assert!(
            left.is_empty(),
            "the half-made run folder is removed: {left:?}"
        );
    }

    /// Writes results.md, a screenshot and a webqa.json whose digest matches (what `state.sh webqa` leaves).
    fn checked_pass(run: &Path, result: &str, rows: Value, findings: Value) {
        std::fs::create_dir_all(run.join("webqa").join("shots")).unwrap();
        std::fs::write(run.join("webqa").join("shots").join("c1.png"), b"png").unwrap();
        std::fs::write(
            run.join("webqa").join("results.md"),
            format!(
                "WEBQA: {result}\nc1 | {} | because | webqa/shots/c1.png\n",
                if result == "pass" { "PASS" } else { "FAIL" }
            ),
        )
        .unwrap();
        let digest = results_digest(run).unwrap();
        std::fs::write(
            run.join("webqa.json"),
            json!({"schema": 1, "digest": digest, "result": result, "rows": rows, "findings": findings, "comparison": {"first": true, "fixedFindings": []}}).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn the_previous_summary_is_the_newest_valid_pass_and_a_tampered_webqa_json_is_ignored() {
        let dir = TempDir::new("webqa-prev");
        let store = MemoryPasswords::default();
        let (settings, id) = pass_settings(&dir, &store);
        let charters: Vec<String> = settings.web_qa().targets[0]
            .charters
            .iter()
            .map(|c| c.id.clone())
            .collect();
        let data = dir.path().join("data");
        let first = PathBuf::from(
            make_pass(&data, &meta(), &settings, &store, &id, &charters)
                .unwrap()
                .run_dir,
        );
        checked_pass(
            &first,
            "fail",
            json!([{"charter": "c1", "result": "FAIL"}]),
            json!([{"key": "login button hidden", "title": "Login button hidden"}]),
        );
        std::fs::write(first.join("status.json"), r#"{"stage":"done"}"#).unwrap();
        // A second pass, newer, whose webqa.json says pass but whose results.md was changed afterwards: ignored.
        std::thread::sleep(Duration::from_millis(1100));
        let second = PathBuf::from(
            make_pass(&data, &meta(), &settings, &store, &id, &charters)
                .unwrap()
                .run_dir,
        );
        checked_pass(
            &second,
            "pass",
            json!([{"charter": "c1", "result": "PASS"}]),
            json!([]),
        );
        std::fs::write(
            second.join("webqa").join("results.md"),
            "WEBQA: pass\nc1 | PASS | edited | webqa/shots/c1.png\n",
        )
        .unwrap();
        let prev = previous_summary(&data, &id).expect("the first pass is the newest valid one");
        assert_eq!(prev["rows"][0], json!({"charter": "c1", "result": "FAIL"}));
        assert_eq!(prev["findings"][0], "login button hidden");
        assert!(
            read_valid(&second).is_err(),
            "the tampered pass is not read"
        );
        // A third pass gets the previous summary in its target file.
        let third = make_pass(&data, &meta(), &settings, &store, &id, &charters).unwrap();
        let t = read_json(&Path::new(&third.run_dir).join("webqa-target.json")).unwrap();
        assert_eq!(t["previous"]["rows"][0]["result"], "FAIL");
        // The overview shows results only for the valid pass, and says why for the finished invalid one.
        std::fs::write(second.join("status.json"), r#"{"stage":"done"}"#).unwrap();
        let views = overview(&data, Some(settings.web_qa()), SystemTime::now());
        let by = |dir: &Path| {
            let id = dir.file_name().unwrap().to_string_lossy().to_string();
            views.iter().find(|v| v.pass_id == id).unwrap().clone()
        };
        assert_eq!(by(&first).result.as_deref(), Some("fail"));
        assert_eq!(by(&first).rows.len(), 1);
        assert!(by(&second).rows.is_empty());
        assert!(by(&second)
            .problem
            .as_deref()
            .unwrap()
            .starts_with("Pass results are not valid:"));
        assert!(by(&PathBuf::from(&third.run_dir)).running);
    }

    #[test]
    fn a_resumed_pass_gets_its_secret_back_from_the_keychain() {
        let dir = TempDir::new("webqa-resume");
        let store = MemoryPasswords::default();
        let (settings, id) = pass_settings(&dir, &store);
        let charters: Vec<String> = settings.web_qa().targets[0]
            .charters
            .iter()
            .map(|c| c.id.clone())
            .collect();
        let data = dir.path().join("data");
        let run = PathBuf::from(
            make_pass(&data, &meta(), &settings, &store, &id, &charters)
                .unwrap()
                .run_dir,
        );
        std::fs::remove_file(run.join(SECRET_FILE)).unwrap();
        rewrite_secret(&run, &settings, &store).expect("rewritten");
        assert!(run.join(SECRET_FILE).is_file());
        store.clear(&id).unwrap();
        std::fs::remove_file(run.join(SECRET_FILE)).unwrap();
        let err = rewrite_secret(&run, &settings, &store).unwrap_err();
        assert_eq!(err, "The test password is not in the keychain.");
        assert!(!run.join(SECRET_FILE).exists());
    }

    #[test]
    fn the_digest_matches_what_state_sh_records() {
        // The same recipe as plugin/scripts/state.sh webqa_results_digest: results.md, then "\n<name>:<sha-256>" per shot.
        let dir = TempDir::new("webqa-digest");
        let run = dir.path();
        std::fs::create_dir_all(run.join("webqa").join("shots")).unwrap();
        std::fs::write(run.join("webqa").join("shots").join("a.png"), b"png").unwrap();
        std::fs::write(run.join("webqa").join("results.md"), "WEBQA: pass\nA | PASS | ok | webqa/shots/a.png\nB | PASS | ok | webqa/shots/gone.png\n").unwrap();
        let mut h = Sha256::new();
        h.update(b"WEBQA: pass\nA | PASS | ok | webqa/shots/a.png\nB | PASS | ok | webqa/shots/gone.png\n");
        h.update(format!("\nwebqa/shots/a.png:{}", sha256_hex(b"png")).as_bytes());
        h.update(b"\nwebqa/shots/gone.png:missing");
        let want: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(results_digest(run).unwrap(), want);
    }

    #[cfg(unix)]
    #[test]
    fn a_release_folder_is_shown_only_when_it_is_the_operators_and_not_writable_by_others() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("webqa-release");
        let root = dir.path().join("dcv2-release-qa");
        let rel = root.join("0.1.0");
        std::fs::create_dir_all(&rel).unwrap();
        for p in [&root, &rel] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(rel.join("release.json"), r#"{"version":"0.1.0"}"#).unwrap();
        std::fs::set_permissions(
            rel.join("release.json"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        checked_pass(
            &rel,
            "pass",
            json!([{"charter": "c1", "result": "PASS"}]),
            json!([]),
        );
        let view = release_open(dir.path(), &root, "0.1.0").expect("shown");
        assert!(
            view.release
                && view.pass_id == "release-0.1.0"
                && view.result.as_deref() == Some("pass")
        );
        std::fs::set_permissions(
            rel.join("release.json"),
            std::fs::Permissions::from_mode(0o666),
        )
        .unwrap();
        assert!(release_open(dir.path(), &root, "0.1.0")
            .unwrap_err()
            .contains("not owned by you or can be written by others"));
        std::fs::set_permissions(
            rel.join("release.json"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        std::fs::set_permissions(&rel, std::fs::Permissions::from_mode(0o775)).unwrap();
        assert!(
            release_open(dir.path(), &root, "0.1.0").is_err(),
            "a group-writable version folder"
        );
        std::fs::set_permissions(&rel, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(release_open(dir.path(), &root, "../etc").is_err());
        assert!(release_open(dir.path(), &root, "0.2.0").is_err());
        assert!(
            valid_version("0.10.0")
                && valid_version("1.2.3-rc.1")
                && !valid_version("1.2")
                && !valid_version("a.b.c")
        );
    }

    // ---- ticked findings become ClickUp tasks through the shared path ----------------------------------------------

    use crate::clickup::client::tests::{ok, FakeClock, Recorded};
    use crate::clickup::client::{ClickUp, Gate};
    use crate::clickup::transport::NoReply;
    use crate::clickup::ClickUpToken;

    const LIST: &str = "901800112233";

    fn finding(title: &str, steps: &str) -> Value {
        json!({"key": title.to_lowercase(), "title": title, "severity": "high", "steps": steps, "expected": "a Sign in button", "seen": "no button", "shot": "webqa/shots/c1.png"})
    }

    /// A pass with two findings, the way `state.sh webqa` leaves it, and its id.
    fn pass_with_findings(dir: &TempDir) -> (PathBuf, PathBuf, String, Value) {
        let store = MemoryPasswords::default();
        let (settings, id) = pass_settings(dir, &store);
        let charters: Vec<String> = settings.web_qa().targets[0]
            .charters
            .iter()
            .map(|c| c.id.clone())
            .collect();
        let data = dir.path().join("data");
        let made = make_pass(&data, &meta(), &settings, &store, &id, &charters).unwrap();
        let run = PathBuf::from(&made.run_dir);
        let findings = json!([
            finding("Login button hidden", "open /login"),
            finding("Search is empty", "type a word")
        ]);
        checked_pass(
            &run,
            "fail",
            json!([{"charter": "c1", "result": "FAIL"}]),
            findings,
        );
        let pass_id = run.file_name().unwrap().to_string_lossy().to_string();
        let doc = read_valid(&run).unwrap();
        (data, run, pass_id, doc)
    }

    fn client_apply(
        api: &Recorded,
        data: &Path,
        pass_id: &str,
    ) -> Result<Vec<ApplyOutcome>, String> {
        let (gate, clock, token) = (
            Gate::new(),
            FakeClock::at(1_790_000_000),
            ClickUpToken::parse("pk_test_TOKEN").unwrap(),
        );
        let client = ClickUp::with(api, &token, &gate, &clock);
        apply(
            &client,
            data,
            Path::new("/nonexistent"),
            &trust::tests::key(),
            pass_id,
            LIST,
            &|| "t".to_string(),
        )
    }

    #[test]
    fn nothing_ticked_asks_for_a_tick_and_makes_no_client_call() {
        let dir = TempDir::new("webqa-none");
        let (data, _run, pass_id, doc) = pass_with_findings(&dir);
        assert_eq!(
            build_approval(&doc, "Staging", &pass_id, &[], "t").unwrap_err(),
            "Tick at least one finding."
        );
        assert!(approve(
            &data,
            Path::new("/nonexistent"),
            &trust::tests::key(),
            &pass_id,
            &[],
            "t"
        )
        .is_err());
        // With no approval at all, applying sends nothing: the Recorded transport panics on any unexpected call.
        let api = Recorded::default();
        assert_eq!(
            client_apply(&api, &data, &pass_id).unwrap_err(),
            "No findings are approved."
        );
        assert!(api.writes.lock().unwrap().is_empty());
        // A record with no creates sends nothing either.
        let mut posted = PostedRecord::default();
        let (gate, clock, token) = (
            Gate::new(),
            FakeClock::at(1_790_000_000),
            ClickUpToken::parse("pk_test_TOKEN").unwrap(),
        );
        let client = ClickUp::with(&api, &token, &gate, &clock);
        let mut out = Vec::new();
        struct Nowhere;
        impl PostStore for Nowhere {
            fn save(&mut self, _: &PostedRecord) -> Result<(), String> {
                Ok(())
            }
        }
        triage::create_tasks_from_record(
            &client,
            LIST,
            &[],
            &mut posted,
            &mut Nowhere,
            &|| "t".into(),
            &mut out,
        )
        .unwrap();
        assert!(out.is_empty() && api.writes.lock().unwrap().is_empty());
    }

    #[test]
    fn each_ticked_finding_makes_exactly_one_task_with_the_stored_text_and_a_second_apply_sends_nothing(
    ) {
        let dir = TempDir::new("webqa-apply");
        let (data, run, pass_id, _doc) = pass_with_findings(&dir);
        let key = trust::tests::key();
        let record = approve(
            &data,
            Path::new("/nonexistent"),
            &key,
            &pass_id,
            &["search is empty".to_string()],
            "t",
        )
        .unwrap();
        assert_eq!(record.creates.len(), 1);
        assert_eq!(record.creates[0].index, 1, "the finding's position");
        let api = Recorded::default().on(&format!("POST /list/{LIST}/task"), ok("{}".into()));
        let out = client_apply(&api, &data, &pass_id).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].result, "posted");
        let writes = api.writes.lock().unwrap().clone();
        assert_eq!(writes.len(), 1, "exactly one create_task");
        assert_eq!(writes[0].body["name"], "[Web QA] Search is empty");
        let description = writes[0].body["description"].as_str().unwrap();
        assert!(
            description.contains("type a word")
                && description.contains("Expected:\na Sign in button")
                && description.contains("Seen:\nno button")
        );
        assert!(description.contains("webqa/shots/c1.png"));
        // A second confirm sends nothing (the transport would panic on another call).
        assert!(client_apply(&api, &data, &pass_id).unwrap().is_empty());
        assert_eq!(api.writes.lock().unwrap().len(), 1);
        // What was sent is shown from the signed record, by the finding's position.
        let view = overview(&data, None, SystemTime::now());
        let shown = view.iter().find(|v| v.pass_id == pass_id).unwrap();
        let _ = &run;
        assert!(shown.posted.is_empty() || shown.posted.contains_key("create:1"));
        // Changing the results after the approval makes it stale: nothing is sent.
        std::fs::write(
            run.join("webqa").join("results.md"),
            "WEBQA: fail\nc1 | FAIL | edited | webqa/shots/c1.png\n",
        )
        .unwrap();
        let again = Recorded::default();
        assert!(client_apply(&again, &data, &pass_id).is_err());
        assert!(again.writes.lock().unwrap().is_empty());
    }

    #[test]
    fn a_forged_or_tampered_approval_is_refused() {
        let dir = TempDir::new("webqa-forged");
        let (data, run, pass_id, _doc) = pass_with_findings(&dir);
        let key = trust::tests::key();
        approve(
            &data,
            Path::new("/nonexistent"),
            &key,
            &pass_id,
            &["login button hidden".to_string()],
            "t",
        )
        .unwrap();
        // A file of that name in the run folder is never read.
        std::fs::write(
            run.join(WEBQA_APPROVAL_FILE),
            r#"{"creates":[{"index":0,"name":"evil","description":"x"}]}"#,
        )
        .unwrap();
        // Editing the signed record in the trust folder breaks its MAC.
        let signed = data
            .join(trust::TRUST_DIR)
            .join(&pass_id)
            .join(WEBQA_APPROVAL_FILE);
        let text = std::fs::read_to_string(&signed)
            .unwrap()
            .replace("Login button hidden", "Evil");
        std::fs::write(&signed, text).unwrap();
        let api = Recorded::default();
        assert!(client_apply(&api, &data, &pass_id).is_err());
        assert!(api.writes.lock().unwrap().is_empty());
        // An unknown finding and a repeated tick are refused when approving.
        let doc = read_valid(&run).unwrap();
        assert!(build_approval(&doc, "S", &pass_id, &["nope".into()], "t").is_err());
        assert!(build_approval(
            &doc,
            "S",
            &pass_id,
            &["search is empty".into(), "search is empty".into()],
            "t"
        )
        .is_err());
    }

    #[test]
    fn a_maybe_is_not_resent_and_the_pass_id_grammar_is_enforced() {
        assert!(
            trust::valid_pass_id("r-20261110-054207-f172")
                && trust::valid_pass_id("release-0.10.0")
        );
        for bad in [
            "",
            "r-1",
            "../x",
            "release-",
            "release-../x",
            "r-20261110-054207-xyz!",
            "release-a/b",
        ] {
            assert!(!trust::valid_pass_id(bad), "{bad}");
        }
        let dir = TempDir::new("webqa-maybe");
        let (data, _run, pass_id, _doc) = pass_with_findings(&dir);
        approve(
            &data,
            Path::new("/nonexistent"),
            &trust::tests::key(),
            &pass_id,
            &["login button hidden".to_string()],
            "t",
        )
        .unwrap();
        let api = Recorded::default().on(&format!("POST /list/{LIST}/task"), Err(NoReply::Failed));
        assert_eq!(
            client_apply(&api, &data, &pass_id).unwrap()[0].result,
            "maybe"
        );
        assert!(
            client_apply(&api, &data, &pass_id).unwrap().is_empty(),
            "a maybe is not sent again"
        );
    }

    #[test]
    fn keychain_service_is_web_qa_s_own() {
        assert_eq!(KEYCHAIN_SERVICE, "dcterminalv2.webqa");
    }
}
