//! The ClickUp Tauri commands: the token per account (Settings), Fetch on
//! the start screen, and the run panel's ticket changes (R8) and write-back.
//!
//! Every network call runs on a blocking thread with no store lock held. The
//! token is read from the keychain inside the command and dropped with it;
//! no reply carries it.

use super::autopost;
use super::backoff::Backoff;
use super::intake::{self, ChangedTicket, IntakePreview, IntakeRef};
use super::merged;
use super::trust;
use super::writeback::{self, PostOutcome, PostRequest, PostedDraft};
use super::{ClickUp, ClickUpError, ClickUpToken, KeychainStore, SecretStore, UreqTransport};
use crate::run::checked_run_dir;
use crate::store::settings_store::account_exists;
use crate::store::{SettingsStore, StateStore};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use tauri::{AppHandle, Manager};

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}

/// The auto-post waits (process memory only; see `backoff`).
fn backoff() -> MutexGuard<'static, Backoff> {
    static WAITS: OnceLock<Mutex<Backoff>> = OnceLock::new();
    WAITS
        .get_or_init(|| Mutex::new(Backoff::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn now_text() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn data_dir(app: &AppHandle) -> PathBuf {
    crate::data_dir::app_data_dir(app).path
}

fn known_account(app: &AppHandle, account_id: &str) -> Result<(), String> {
    let settings = app.state::<Mutex<SettingsStore>>();
    let settings = settings.lock().map_err(|e| e.to_string())?;
    if account_exists(settings.claude(), account_id) {
        Ok(())
    } else {
        Err("No Claude account with that id.".into())
    }
}

fn window_account(app: &AppHandle, window_id: Option<&str>) -> Result<String, String> {
    let state = app.state::<Mutex<StateStore>>();
    let mut state = state.lock().map_err(|e| e.to_string())?;
    state.bind_window(window_id);
    let window = state.focus_window().to_string();
    Ok(state.account_for_window(&window))
}

/// The token of `account_id`, or the plain-English "no token" error.
pub fn token_of(store: &dyn SecretStore, account_id: &str) -> Result<ClickUpToken, String> {
    store
        .get(account_id)?
        .ok_or_else(|| ClickUpError::NoToken.to_string())
}

// ---- Settings: the token -----------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenStatus {
    pub account_id: String,
    pub saved: bool,
    pub problem: Option<String>,
}

#[tauri::command]
pub async fn clickup_token_status(
    app: AppHandle,
    account_id: String,
) -> Result<TokenStatus, String> {
    known_account(&app, &account_id)?;
    blocking(move || {
        Ok(match KeychainStore.get(&account_id) {
            Ok(token) => TokenStatus {
                account_id,
                saved: token.is_some(),
                problem: None,
            },
            Err(problem) => TokenStatus {
                account_id,
                saved: false,
                problem: Some(problem),
            },
        })
    })
    .await
}

#[tauri::command]
pub async fn clickup_set_token(
    app: AppHandle,
    account_id: String,
    token: String,
) -> Result<(), String> {
    known_account(&app, &account_id)?;
    let token = ClickUpToken::parse(&token)?;
    blocking(move || {
        let saved = KeychainStore.set(&account_id, &token);
        backoff().clear_all();
        saved
    })
    .await
}

#[tauri::command]
pub async fn clickup_clear_token(app: AppHandle, account_id: String) -> Result<(), String> {
    known_account(&app, &account_id)?;
    blocking(move || {
        let cleared = KeychainStore.clear(&account_id);
        backoff().clear_all();
        cleared
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenTest {
    pub workspaces: Vec<String>,
}

/// Test: `GET /team` with the saved token; the workspace names it sees.
#[tauri::command]
pub async fn clickup_test_token(app: AppHandle, account_id: String) -> Result<TokenTest, String> {
    known_account(&app, &account_id)?;
    blocking(move || {
        let token = token_of(&KeychainStore, &account_id)?;
        let transport = UreqTransport::new();
        let teams = ClickUp::new(&transport, &token)
            .teams()
            .map_err(String::from)?;
        Ok(TokenTest {
            workspaces: teams.into_iter().map(|t| t.name).collect(),
        })
    })
    .await
}

// ---- Start screen: Fetch and Start ---------------------------------------------------------

pub fn fetch_with(client: &ClickUp, refs: &[IntakeRef]) -> Result<IntakePreview, String> {
    let needs_workspace = refs.iter().any(|r| r.custom && r.team_id.is_none());
    let workspace = if needs_workspace {
        intake::single_workspace(client)?
    } else {
        None
    };
    intake::preview(client, refs, workspace.as_deref())
}

/// Fetch with the window's own account's token.
#[tauri::command]
pub async fn clickup_fetch(
    app: AppHandle,
    window_id: Option<String>,
    refs: Vec<IntakeRef>,
) -> Result<IntakePreview, String> {
    let account = window_account(&app, window_id.as_deref())?;
    blocking(move || {
        let token = token_of(&KeychainStore, &account)?;
        let transport = UreqTransport::new();
        fetch_with(&ClickUp::new(&transport, &token), &refs)
    })
    .await
}

/// Start's part: the snapshot of the kept tickets, with `account_id`'s token.
/// Runs before the run folder exists, so a failed read starts nothing.
pub fn start_snapshot(
    account_id: &str,
    refs: &[IntakeRef],
    ids: &[String],
) -> Result<intake::TicketsSnapshot, String> {
    let token = token_of(&KeychainStore, account_id)?;
    let transport = UreqTransport::new();
    let client = ClickUp::new(&transport, &token);
    let workspace = intake::single_workspace(&client)?;
    intake::snapshot(
        &client,
        account_id,
        workspace.as_deref(),
        refs,
        ids,
        &now_text(),
    )
}

// ---- Run panel ---------------------------------------------------------------------------

/// A ticket as the app's own snapshot names it (never writeback.json's words).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedTicket {
    pub custom_id: Option<String>,
    pub name: String,
    pub url: String,
    pub list: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftView {
    #[serde(flatten)]
    pub draft: writeback::Draft,
    pub posted: PostedDraft,
    /// The comment's last send did not finish cleanly: resend needs a confirm.
    pub needs_confirm: bool,
    /// What the secret scan finds in the unedited comment ("a GitHub token"), if anything.
    pub secret_kind: Option<String>,
    /// From the app's snapshot; `None`: not in it, never posted.
    pub trusted: Option<TrustedTicket>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunClickUp {
    pub has_tickets: bool,
    pub ticket_count: usize,
    pub fetched_at: Option<String>,
    pub tickets_problem: Option<String>,
    /// Ticket ids pulled in so far (tickets-update.md).
    pub pulled: Vec<String>,
    pub update_file: Option<String>,
    pub drafts: Option<Vec<DraftView>>,
    /// sha-256 of writeback.json as shown; a post with another is refused.
    pub drafts_version: Option<String>,
    pub writeback_problem: Option<String>,
    /// Comments posted on their own after the ship approval.
    pub auto_post: Option<AutoPostView>,
    /// A status draft per ticket whose PRs all merged (`merged.rs`).
    pub merged: Vec<merged::MergedDraft>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoPostView {
    /// The run's account has "Post ClickUp comments automatically" on.
    pub enabled: bool,
    /// Posting now (in the background).
    pub running: bool,
    pub record: Option<autopost::AutoPostRecord>,
    pub problem: Option<String>,
    /// A dev-app run used in place: never auto-posted here (the app that
    /// owns its data folder does that); ticked posts still work.
    pub adopted: bool,
    /// Waiting for the user's click: the comments that would go, and those that never
    /// go on their own. Nothing is sent until the consent command records the click.
    pub offer: Option<autopost::ConsentOffer>,
    /// The last automatic attempt failed before anything was recorded; the next one waits.
    pub waiting: Option<WaitingView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitingView {
    pub reason: String,
    /// UTC time of the next automatic try; `None`: not before a token is set.
    pub next_at: Option<String>,
    pub no_token: bool,
}

fn waiting_view(dir: &Path) -> Option<WaitingView> {
    backoff().waiting(dir).map(|wait| WaitingView {
        reason: wait.reason.clone(),
        next_at: wait.next_at.and_then(|secs| {
            chrono::DateTime::from_timestamp(i64::try_from(secs).ok()?, 0)
                .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        }),
        no_token: wait.no_token,
    })
}

const UNTRUSTED: &str = "This run's tickets.json was not written by this app's Start, so the app checks and posts nothing for it.";

pub fn run_clickup(key: &Result<trust::Key, String>, data: &Path, run_dir: &Path) -> RunClickUp {
    let mut out = RunClickUp::default();
    let loaded = match key {
        Ok(key) => trust::load(key, data, run_dir),
        Err(problem) if trust::trust_dir(data, run_dir).is_ok_and(|d| d.exists()) => {
            Err(problem.clone())
        }
        Err(_) => Ok(None),
    };
    let trusted = match loaded {
        Ok(Some(t)) => Some(t),
        Ok(None) => {
            if run_dir.join(intake::TICKETS_FILE).exists() {
                out.has_tickets = true;
                out.tickets_problem = Some(UNTRUSTED.into());
            }
            None
        }
        Err(problem) => {
            out.has_tickets = true;
            out.tickets_problem = Some(problem);
            None
        }
    };
    let tickets: HashMap<String, intake::SnapshotTask> = trusted
        .as_ref()
        .map(|t| {
            t.snapshot
                .tasks
                .iter()
                .map(|x| (x.id.clone(), x.clone()))
                .collect()
        })
        .unwrap_or_default();
    if let Some(t) = &trusted {
        out.has_tickets = true;
        out.ticket_count = t.snapshot.tasks.len();
        out.fetched_at = Some(t.snapshot.fetched_at.clone());
        let key = key.as_ref().expect("trusted implies a key");
        let mut pulled: Vec<String> = trust::read_pulls(key, data, run_dir)
            .map(|p| p.tickets.keys().cloned().collect())
            .unwrap_or_default();
        pulled.sort();
        out.pulled = pulled;
    }
    if run_dir.join(intake::UPDATE_FILE).is_file() {
        out.update_file = Some(run_dir.join(intake::UPDATE_FILE).display().to_string());
    }
    match writeback::read_drafts(run_dir) {
        Ok(None) => {}
        Err(problem) => out.writeback_problem = Some(problem),
        Ok(Some((drafts, version))) => {
            let posted = match (key, &trusted) {
                (Ok(key), Some(_)) => trust::read_posted(key, data, run_dir),
                _ => Ok(writeback::Posted::default()),
            };
            if let Err(problem) = &posted {
                out.writeback_problem = Some(problem.clone());
            }
            let posted = posted.unwrap_or_default();
            out.drafts_version = Some(version);
            out.drafts = Some(
                drafts
                    .drafts
                    .into_iter()
                    .map(|draft| {
                        let record = posted
                            .drafts
                            .get(&draft.ticket.id)
                            .cloned()
                            .unwrap_or_default();
                        DraftView {
                            needs_confirm: writeback::needs_confirm(record.comment.as_ref()),
                            secret_kind: draft
                                .comment
                                .as_deref()
                                .and_then(super::secret_scan::first_hit)
                                .map(str::to_string),
                            trusted: tickets.get(&draft.ticket.id).map(|t| TrustedTicket {
                                custom_id: t.custom_id.clone(),
                                name: t.name.clone(),
                                url: t.url.clone(),
                                list: t.list.as_ref().map(|l| l.name.clone()),
                            }),
                            posted: record,
                            draft,
                        }
                    })
                    .collect(),
            );
        }
    }
    out
}

#[tauri::command]
pub async fn clickup_run_state(app: AppHandle, run_dir: String) -> Result<RunClickUp, String> {
    blocking(move || {
        let data = data_dir(&app);
        let dir = checked_run_dir(&data, &run_dir)?;
        let own = data.clone();
        // An adopted run's records are its own data folder's run-trust.
        let data = crate::run::trust_data_dir(&data, &dir);
        let key = trust::install_key();
        let mut view = run_clickup(&key, &data, &dir);
        if let Ok(key) = &key {
            view.merged = merged_drafts(&app, key, &data, &dir);
        }
        if autopost::may_be_due(&dir) {
            if let Ok(key) = &key {
                let adopted = data != own;
                view.auto_post = Some(auto_post_view(&app, key, &data, &dir, adopted));
            }
        }
        Ok(view)
    })
    .await
}

/// The per-list done-status mapping (`settings.json` `clickup.doneStatus`).
fn done_status_map(app: &AppHandle) -> std::collections::BTreeMap<String, String> {
    app.state::<Mutex<SettingsStore>>()
        .lock()
        .map(|s| s.clickup().done_status.clone())
        .unwrap_or_default()
}

/// The merged drafts of the run's view. Local files only, unless a ticket
/// has all its PRs merged and no merged status posted yet: then the list's
/// statuses are read (cached 10 min, a failure 60 s) and a failure is a note
/// on the row, never an error of the view.
fn merged_drafts(
    app: &AppHandle,
    key: &trust::Key,
    data: &Path,
    dir: &Path,
) -> Vec<merged::MergedDraft> {
    let Ok(Some(run)) = trust::load(key, data, dir) else {
        return Vec::new();
    };
    let Ok(Some((drafts, _))) = writeback::read_drafts(dir) else {
        return Vec::new();
    };
    let posted = trust::read_posted(key, data, dir).unwrap_or_default();
    let tickets: HashMap<String, intake::SnapshotTask> = run
        .snapshot
        .tasks
        .into_iter()
        .map(|t| (t.id.clone(), t))
        .collect();
    let account = run.account_id;
    merged::derive(
        &tickets,
        &drafts,
        &crate::run::github::read_github(dir),
        &crate::run::cleanup::read_cleanup(dir),
        &posted,
        &done_status_map(app),
        &mut |list_id| {
            merged::cache().get(list_id, std::time::Instant::now(), || {
                let token = token_of(&KeychainStore, &account)?;
                let transport = UreqTransport::new();
                merged::fetch_statuses(&ClickUp::new(&transport, &token), list_id)
            })
        },
    )
}

fn auto_post_on(app: &AppHandle, account_id: &str) -> bool {
    let settings = app.state::<Mutex<SettingsStore>>();
    settings
        .lock()
        .map(|s| s.clickup().auto_post_for(account_id))
        .unwrap_or(false)
}

/// The auto-post part of the panel; starts the post when it is due.
fn auto_post_view(
    app: &AppHandle,
    key: &trust::Key,
    data: &Path,
    dir: &Path,
    adopted: bool,
) -> AutoPostView {
    let mut view = AutoPostView {
        adopted,
        ..AutoPostView::default()
    };
    let run = match trust::load(key, data, dir) {
        Ok(Some(run)) => run,
        Ok(None) => return view,
        Err(problem) => {
            view.problem = Some(problem);
            return view;
        }
    };
    // Only the app that owns the run's data folder posts on its own.
    view.enabled = !adopted && auto_post_on(app, &run.account_id);
    match trust::read_autopost(key, data, dir) {
        Ok(record) => view.record = record,
        Err(problem) => view.problem = Some(problem),
    }
    view.running = BUSY
        .lock()
        .map(|b| b.as_ref().is_some_and(|set| set.contains(dir)))
        .unwrap_or(false);
    if view.enabled && view.problem.is_none() {
        match autopost::offer(key, data, dir) {
            Ok(offer) => view.offer = offer,
            Err(problem) => view.problem = Some(problem),
        }
    }
    if view.enabled && !view.running && view.problem.is_none() {
        if let Ok(Some(_)) = autopost::due(key, data, dir) {
            view.running = spawn_auto_post(
                key.clone(),
                data.to_path_buf(),
                dir.to_path_buf(),
                run.account_id,
            );
        }
    }
    if view.enabled {
        view.waiting = waiting_view(dir);
    }
    view
}

/// Posts in the background under the run's lock; `false` when busy.
fn spawn_auto_post(key: trust::Key, data: PathBuf, dir: PathBuf, account_id: String) -> bool {
    // A failed attempt waits (1, 2, 4 ... minutes), and no token waits for Settings.
    if !backoff().allowed(&dir, now_secs()) {
        return false;
    }
    let Ok(lock) = RunLock::take(&dir) else {
        return false;
    };
    std::thread::spawn(move || {
        let _lock = lock;
        match run_auto_post(&key, &data, &dir, &account_id) {
            Ok(()) => backoff().succeeded(&dir),
            Err(err) => {
                eprintln!("DCTerminalv2: ClickUp auto-post: {err}");
                if err == ClickUpError::NoToken.to_string() {
                    backoff().missing_token(&dir, &account_id, "no ClickUp token for this account");
                } else {
                    backoff().failed(&dir, &err, now_secs());
                }
            }
        }
    });
    true
}

fn run_auto_post(
    key: &trust::Key,
    data: &Path,
    dir: &Path,
    account_id: &str,
) -> Result<(), String> {
    if autopost::due(key, data, dir)?.is_none() {
        return Ok(());
    }
    // The run's own account's token, from the signed record (never the window's).
    // An account known to have none is not looked up again until Settings change.
    if backoff().account_has_no_token(account_id) {
        return Err(ClickUpError::NoToken.to_string());
    }
    let token = token_of(&KeychainStore, account_id)?;
    let transport = UreqTransport::new();
    autopost::auto_post(&ClickUp::new(&transport, &token), key, data, dir, &now_text).map(|_| ())
}

/// Retries one failed auto-posted comment.
#[tauri::command]
pub async fn clickup_auto_post_retry(
    app: AppHandle,
    run_dir: String,
    ticket_id: String,
) -> Result<autopost::AutoPostRecord, String> {
    blocking(move || {
        let data = data_dir(&app);
        let dir = checked_run_dir(&data, &run_dir)?;
        let data = crate::run::trust_data_dir(&data, &dir);
        let _lock = RunLock::take(&dir)?;
        let key = trust::install_key()?;
        let run = trusted(&key, &data, &dir)?;
        let token = token_of(&KeychainStore, &run.account_id)?;
        let transport = UreqTransport::new();
        autopost::retry(
            &ClickUp::new(&transport, &token),
            &key,
            &data,
            &dir,
            &ticket_id,
            &now_text,
        )
    })
    .await
}

/// The panel's one click: records the user's consent for the writeback.json
/// version they read (signed, app-side), then starts the auto-post. The only
/// writer of the consent.
#[tauri::command]
pub async fn clickup_auto_post_consent(
    app: AppHandle,
    run_dir: String,
    writeback_sha: String,
) -> Result<(), String> {
    blocking(move || {
        let own = data_dir(&app);
        let dir = checked_run_dir(&own, &run_dir)?;
        let data = crate::run::trust_data_dir(&own, &dir);
        if data != own {
            return Err(
                "This run came from the dev app: its comments are not posted automatically here."
                    .into(),
            );
        }
        let key = trust::install_key()?;
        let run = trusted(&key, &data, &dir)?;
        if !auto_post_on(&app, &run.account_id) {
            return Err("Auto-post is off for this account (Settings › ClickUp).".into());
        }
        autopost::give_consent(&key, &data, &dir, &writeback_sha, &now_text)?;
        backoff().clear_run(&dir, &run.account_id);
        spawn_auto_post(key, data, dir, run.account_id);
        Ok(())
    })
    .await
}

/// "Try now": ends this run's wait and makes one attempt through the same path
/// as the sweep (the consent and the secret rules are unchanged).
#[tauri::command]
pub async fn clickup_auto_post_now(app: AppHandle, run_dir: String) -> Result<(), String> {
    blocking(move || {
        let own = data_dir(&app);
        let dir = checked_run_dir(&own, &run_dir)?;
        let data = crate::run::trust_data_dir(&own, &dir);
        if data != own {
            return Err(
                "This run came from the dev app: its comments are not posted automatically here."
                    .into(),
            );
        }
        let key = trust::install_key()?;
        let run = trusted(&key, &data, &dir)?;
        if !auto_post_on(&app, &run.account_id) {
            return Err("Auto-post is off for this account (Settings › ClickUp).".into());
        }
        backoff().clear_run(&dir, &run.account_id);
        spawn_auto_post(key, data, dir, run.account_id);
        Ok(())
    })
    .await
}

#[tauri::command]
pub fn clickup_set_auto_post(app: AppHandle, account_id: String, on: bool) -> Result<(), String> {
    let settings = app.state::<Mutex<SettingsStore>>();
    let mut settings = settings.lock().map_err(|e| e.to_string())?;
    let saved = settings.set_auto_post(&account_id, on);
    backoff().clear_all();
    saved
}

/// How often the app looks for runs whose comments are due.
const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// Auto-post without the panel open: every minute, each run (own and
/// adopted) that is approved and has drafts is looked at; the keychain is
/// read only for those.
pub fn spawn_auto_post_sweep(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(SWEEP_EVERY);
        let data = data_dir(&app);
        for run in crate::run::folder::list_runs(&data) {
            let dir = PathBuf::from(&run.run_dir);
            if !autopost::may_be_due(&dir) || !backoff().allowed(&dir, now_secs()) {
                continue;
            }
            let Ok(key) = trust::install_key() else {
                break;
            };
            let tdata = crate::run::trust_data_dir(&data, &dir);
            if tdata != data {
                continue; // adopted: the owning app posts
            }
            if let Ok(Some(t)) = trust::load(&key, &tdata, &dir) {
                if auto_post_on(&app, &t.account_id)
                    && matches!(autopost::due(&key, &tdata, &dir), Ok(Some(_)))
                {
                    spawn_auto_post(key.clone(), tdata, dir, t.account_id);
                }
            }
        }
    });
}

/// The app's own record of the run, or why the app does nothing for it.
fn trusted(key: &trust::Key, data: &Path, dir: &Path) -> Result<trust::RunTrust, String> {
    trust::load(key, data, dir)?.ok_or_else(|| UNTRUSTED.to_string())
}

/// R8 for one checked run folder: which tickets ClickUp changed since the
/// run's version of them (the button and a typed request share it; the
/// token is read here and dropped).
pub fn changed_tickets(app_data: &Path, dir: &Path) -> Result<Vec<ChangedTicket>, String> {
    // An adopted run's records are its own data folder's run-trust.
    let data = crate::run::trust_data_dir(app_data, dir);
    let key = trust::install_key()?;
    let run = trusted(&key, &data, dir)?;
    let token = token_of(&KeychainStore, &run.account_id)?;
    let transport = UreqTransport::new();
    intake::check_changes(
        &ClickUp::new(&transport, &token),
        &run.snapshot,
        &trust::read_pulls(&key, &data, dir)?,
    )
}

/// R8: which tickets ClickUp changed since the run's version of them.
#[tauri::command]
pub async fn clickup_check_tickets(
    app: AppHandle,
    run_dir: String,
) -> Result<Vec<ChangedTicket>, String> {
    blocking(move || {
        let data = data_dir(&app);
        let dir = checked_run_dir(&data, &run_dir)?;
        changed_tickets(&data, &dir)
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pulled {
    pub pulled: Vec<String>,
    pub update_file: String,
}

/// R8: writes `tickets-update.md`. The user tells Claude in the tab; the app never types.
#[tauri::command]
pub async fn clickup_pull_tickets(
    app: AppHandle,
    run_dir: String,
    ids: Vec<String>,
) -> Result<Pulled, String> {
    blocking(move || {
        let data = data_dir(&app);
        let dir = checked_run_dir(&data, &run_dir)?;
        // An adopted run's records are its own data folder's run-trust.
        let data = crate::run::trust_data_dir(&data, &dir);
        let _lock = RunLock::take(&dir)?;
        let key = trust::install_key()?;
        let run = trusted(&key, &data, &dir)?;
        let token = token_of(&KeychainStore, &run.account_id)?;
        let transport = UreqTransport::new();
        let mut pulls = trust::read_pulls(&key, &data, &dir)?;
        let pulled = intake::pull_in(
            &ClickUp::new(&transport, &token),
            &dir,
            &run.snapshot,
            &mut pulls,
            &ids,
            &now_text(),
        )?;
        trust::save_pulls(&key, &data, &dir, &pulls)?;
        Ok(Pulled {
            pulled,
            update_file: dir.join(intake::UPDATE_FILE).display().to_string(),
        })
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostReply {
    pub outcomes: Vec<PostOutcome>,
    /// A save failed midway: the send stopped here.
    pub error: Option<String>,
}

/// Checks before any call: writeback.json is still the version shown.
pub fn post_checked(
    client: &ClickUp,
    key: &trust::Key,
    data: &Path,
    dir: &Path,
    drafts_version: &str,
    requests: &[PostRequest],
) -> Result<PostReply, String> {
    let run = trusted(key, data, dir)?;
    // One post per run across processes (the dev and the built app).
    let _lock = trust::lock_posts(data, dir)?;
    let (drafts, version) =
        writeback::read_drafts(dir)?.ok_or("This run has no writeback.json yet.")?;
    if version != drafts_version {
        return Err(writeback::DRAFTS_CHANGED.into());
    }
    let tickets: HashMap<String, intake::SnapshotTask> = run
        .snapshot
        .tasks
        .into_iter()
        .map(|t| (t.id.clone(), t))
        .collect();
    let mut posted = trust::read_posted(key, data, dir)?;
    let mut store = trust::TrustStore {
        key,
        data,
        run_dir: dir,
    };
    Ok(
        match writeback::post(
            client,
            &drafts,
            &tickets,
            &mut posted,
            &mut store,
            requests,
            &now_text,
        ) {
            Ok(outcomes) => PostReply {
                outcomes,
                error: None,
            },
            Err((outcomes, error)) => PostReply {
                outcomes,
                error: Some(format!("The posted record could not be saved: {error}")),
            },
        },
    )
}

/// Posts the ticked parts of the drafts. One send per run at a time.
#[tauri::command]
pub async fn clickup_post_writeback(
    app: AppHandle,
    run_dir: String,
    drafts_version: String,
    requests: Vec<PostRequest>,
) -> Result<PostReply, String> {
    blocking(move || {
        let data = data_dir(&app);
        let dir = checked_run_dir(&data, &run_dir)?;
        // An adopted run's records are its own data folder's run-trust.
        let data = crate::run::trust_data_dir(&data, &dir);
        let _lock = RunLock::take(&dir)?;
        let key = trust::install_key()?;
        let run = trusted(&key, &data, &dir)?;
        let token = token_of(&KeychainStore, &run.account_id)?;
        let transport = UreqTransport::new();
        post_checked(
            &ClickUp::new(&transport, &token),
            &key,
            &data,
            &dir,
            &drafts_version,
            &requests,
        )
    })
    .await
}

/// Sets a ticket's "merged" status, once its PRs all merged: only the
/// status (never a comment), only what the app derives itself, one send per
/// run at a time and across processes, recorded in the signed posted record.
/// The list is read fresh here (not from the cache).
#[tauri::command]
pub async fn clickup_post_merged_status(
    app: AppHandle,
    run_dir: String,
    ticket_id: String,
    status: String,
) -> Result<PostOutcome, String> {
    blocking(move || {
        let data = data_dir(&app);
        let dir = checked_run_dir(&data, &run_dir)?;
        // An adopted run's records are its own data folder's run-trust.
        let data = crate::run::trust_data_dir(&data, &dir);
        let _lock = RunLock::take(&dir)?;
        let key = trust::install_key()?;
        let run = trusted(&key, &data, &dir)?;
        let token = token_of(&KeychainStore, &run.account_id)?;
        let transport = UreqTransport::new();
        let _post_lock = trust::lock_posts(&data, &dir)?;
        let (drafts, _) =
            writeback::read_drafts(&dir)?.ok_or("This run has no writeback.json yet.")?;
        let tickets: HashMap<String, intake::SnapshotTask> = run
            .snapshot
            .tasks
            .into_iter()
            .map(|t| (t.id.clone(), t))
            .collect();
        let mut posted = trust::read_posted(&key, &data, &dir)?;
        let mut store = trust::TrustStore {
            key: &key,
            data: &data,
            run_dir: &dir,
        };
        merged::post_merged(
            &ClickUp::new(&transport, &token),
            &tickets,
            &drafts,
            &crate::run::github::read_github(&dir),
            &crate::run::cleanup::read_cleanup(&dir),
            &mut posted,
            &mut store,
            &done_status_map(&app),
            &ticket_id,
            &status,
            &now_text,
        )
    })
    .await
}

static BUSY: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

/// One ClickUp write (post or pull) per run folder at a time.
struct RunLock(PathBuf);

impl RunLock {
    fn take(dir: &Path) -> Result<Self, String> {
        let mut busy = BUSY.lock().map_err(|e| e.to_string())?;
        if !busy
            .get_or_insert_with(HashSet::new)
            .insert(dir.to_path_buf())
        {
            return Err("ClickUp is already being updated for this run.".into());
        }
        Ok(Self(dir.to_path_buf()))
    }
}

impl Drop for RunLock {
    fn drop(&mut self) {
        if let Ok(mut busy) = BUSY.lock() {
            if let Some(set) = busy.as_mut() {
                set.remove(&self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clickup::client::tests::{ok, Recorded};
    use crate::clickup::secret::MemoryStore;
    use crate::clickup::writeback::PostedStore;
    use crate::test_support::TempDir;

    fn k() -> trust::Key {
        trust::tests::key()
    }

    #[test]
    fn no_token_is_a_plain_error_and_tokens_stay_per_account() {
        let store = MemoryStore::default();
        let err = token_of(&store, "work").unwrap_err();
        assert!(err.starts_with("No ClickUp token yet"), "{err}");
        store
            .set("personal", &ClickUpToken::parse("pk_personal").unwrap())
            .unwrap();
        assert!(token_of(&store, "work").is_err());
        assert!(token_of(&store, "personal").is_ok());
    }

    fn snap(id: &str) -> intake::TicketsSnapshot {
        intake::TicketsSnapshot {
            schema: 1,
            fetched_at: "2026-10-23T10:00:00Z".into(),
            source: intake::SnapshotSource {
                kind: "clickup".into(),
                account_id: "a".into(),
                workspace_id: None,
                refs: vec![],
            },
            limits: intake::LIMITS,
            note: String::new(),
            tasks: vec![intake::tests::sample(id, "Export invoices (trusted)")],
            epics: vec![],
        }
    }

    fn run_with_drafts(tag: &str) -> (TempDir, PathBuf, PathBuf) {
        let dir = TempDir::new(tag);
        let data = dir.join("data");
        let run = data.join("runs/r-1");
        std::fs::create_dir_all(&run).unwrap();
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/clickup/writeback-skill.json");
        std::fs::copy(fixture, run.join(writeback::DRAFTS_FILE)).unwrap();
        (dir, data, run)
    }

    #[test]
    fn labels_come_from_the_app_s_snapshot_and_a_run_folder_snapshot_is_not_trusted() {
        let (_dir, data, run) = run_with_drafts("run_clickup");
        // Only the run folder has a tickets.json (Claude could have written it).
        intake::write_snapshot(&run, &snap("86c0abcde")).unwrap();
        let view = run_clickup(&Ok(k()), &data, &run);
        assert_eq!(view.tickets_problem.as_deref(), Some(UNTRUSTED));
        assert!(view.drafts.as_ref().unwrap()[0].trusted.is_none());
        // Written at Start: trusted, with the snapshot's own name.
        trust::write_start(&k(), &data, &run, "personal", &snap("86c0abcde")).unwrap();
        let sending: writeback::Posted = serde_json::from_str(
            r#"{"schema":1,"drafts":{"86c0abcde":{"comment":{"result":"sending","at":"t","value":"x"}}}}"#,
        )
        .unwrap();
        trust::TrustStore {
            key: &k(),
            data: &data,
            run_dir: &run,
        }
        .save(&sending)
        .unwrap();
        let view = run_clickup(&Ok(k()), &data, &run);
        assert!(view.tickets_problem.is_none());
        assert_eq!(view.ticket_count, 1);
        assert_eq!(view.drafts_version.as_ref().unwrap().len(), 64);
        let drafts = view.drafts.unwrap();
        let trusted = drafts[0].trusted.as_ref().unwrap();
        assert_eq!(trusted.name, "Export invoices (trusted)");
        assert!(drafts[0].needs_confirm);
        let json = serde_json::to_value(&drafts[0]).unwrap();
        assert_eq!(json["posted"]["comment"]["result"], "sending");
        // A draft for a ticket outside the snapshot has no trusted label.
        trust::write_start(&k(), &data, &run, "personal", &snap("86other1")).unwrap();
        assert!(run_clickup(&Ok(k()), &data, &run).drafts.unwrap()[0]
            .trusted
            .is_none());
    }

    #[test]
    fn the_draft_view_carries_the_scan_kind_of_the_unedited_comment() {
        let (_dir, data, run) = run_with_drafts("secret_kind");
        std::fs::write(
            run.join(writeback::DRAFTS_FILE),
            serde_json::json!({"schema": 1, "drafts": [
                {"ticket": {"id": "86c0abcde"}, "comment": "key ghp_abcdefghijklmnopqrstuvwxyz0123"},
                {"ticket": {"id": "86c0abcdf"}, "comment": "Done: S1"}
            ]})
            .to_string(),
        )
        .unwrap();
        trust::write_start(&k(), &data, &run, "personal", &snap("86c0abcde")).unwrap();
        let drafts = run_clickup(&Ok(k()), &data, &run).drafts.unwrap();
        assert_eq!(drafts[0].secret_kind.as_deref(), Some("a GitHub token"));
        assert_eq!(drafts[1].secret_kind, None);
        let json = serde_json::to_value(&drafts[0]).unwrap();
        assert_eq!(json["secretKind"], "a GitHub token");
        assert!(
            !json.to_string().contains("secretKind\":\"ghp_"),
            "the value is not in the kind"
        );
    }

    #[test]
    fn a_post_is_refused_when_writeback_json_changed_or_the_run_is_untrusted() {
        let (_dir, data, run) = run_with_drafts("post_checked");
        let token = ClickUpToken::parse("pk_x").unwrap();
        let api = Recorded::default();
        let client = ClickUp::new(&api, &token);
        let req = [PostRequest {
            ticket_id: "86c0abcde".into(),
            comment: Some("x".into()),
            status: None,
            confirm_resend: false,
            confirm_secret: false,
        }];
        assert_eq!(
            post_checked(&client, &k(), &data, &run, "v", &req).unwrap_err(),
            UNTRUSTED
        );
        trust::write_start(&k(), &data, &run, "personal", &snap("86c0abcde")).unwrap();
        let version = run_clickup(&Ok(k()), &data, &run).drafts_version.unwrap();
        assert_eq!(
            post_checked(&client, &k(), &data, &run, "old", &req).unwrap_err(),
            writeback::DRAFTS_CHANGED
        );
        assert!(api.paths().is_empty());
        let api = Recorded::default().on("POST /task/86c0abcde/comment", ok("{}".into()));
        let client = ClickUp::new(&api, &token);
        let reply = post_checked(&client, &k(), &data, &run, &version, &req).unwrap();
        assert_eq!(reply.outcomes[0].result, "posted");
        // Authoritative record app-side; the run folder has a mirror.
        assert!(trust::read_posted(&k(), &data, &run)
            .unwrap()
            .drafts
            .contains_key("86c0abcde"));
        assert!(run.join(writeback::POSTED_FILE).exists());
        // A forged run-folder record does not stop or fake anything.
        std::fs::write(run.join(writeback::POSTED_FILE), "{}").unwrap();
        let again = post_checked(&client, &k(), &data, &run, &version, &req).unwrap();
        assert_eq!(again.outcomes[0].message, "Comment already posted.");
    }

    #[test]
    fn a_tampered_record_turns_clickup_off_with_a_message() {
        let (_dir, data, run) = run_with_drafts("tampered");
        trust::write_start(&k(), &data, &run, "personal", &snap("86c0abcde")).unwrap();
        let path = trust::trust_dir(&data, &run).unwrap().join("trust.json");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("personal", "work")).unwrap();
        let view = run_clickup(&Ok(k()), &data, &run);
        assert_eq!(view.tickets_problem.as_deref(), Some(trust::TAMPERED));
        assert!(view.drafts.unwrap()[0].trusted.is_none());
        let token = ClickUpToken::parse("pk_x").unwrap();
        let api = Recorded::default();
        let client = ClickUp::new(&api, &token);
        let req = [PostRequest {
            ticket_id: "86c0abcde".into(),
            comment: Some("x".into()),
            status: None,
            confirm_resend: false,
            confirm_secret: false,
        }];
        assert_eq!(
            post_checked(&client, &k(), &data, &run, "v", &req).unwrap_err(),
            trust::TAMPERED
        );
        assert!(api.paths().is_empty());
        // No key from the keychain: the panel says so, nothing is trusted.
        let view = run_clickup(&Err("no keychain".into()), &data, &run);
        assert_eq!(view.tickets_problem.as_deref(), Some("no keychain"));
    }

    #[test]
    fn the_panel_reads_the_wait_and_settings_clear_it() {
        let run = Path::new("/runs/r-waiting-view-test");
        assert!(waiting_view(run).is_none());
        backoff().failed(run, "disk full", 1_700_000_000);
        let wait = waiting_view(run).unwrap();
        assert_eq!(wait.reason, "disk full");
        assert_eq!(
            wait.next_at.as_deref(),
            Some("2023-11-14T22:14:20Z"),
            "one minute on"
        );
        assert!(!wait.no_token);
        assert!(!backoff().allowed(run, 1_700_000_059));
        backoff().missing_token(run, "work", "no ClickUp token for this account");
        let wait = waiting_view(run).unwrap();
        assert!(wait.no_token && wait.next_at.is_none());
        assert!(backoff().account_has_no_token("work"));
        // What a token save, a token clear and the auto-post switch all do.
        backoff().clear_all();
        assert!(waiting_view(run).is_none());
        assert!(backoff().allowed(run, 0) && !backoff().account_has_no_token("work"));
    }

    #[test]
    fn one_write_per_run_at_a_time() {
        let dir = TempDir::new("run_lock");
        let first = RunLock::take(dir.path()).unwrap();
        assert!(RunLock::take(dir.path()).is_err());
        drop(first);
        assert!(RunLock::take(dir.path()).is_ok());
    }
}
