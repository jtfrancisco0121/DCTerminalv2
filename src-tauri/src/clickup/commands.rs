//! The ClickUp Tauri commands: the token per account (Settings), Fetch on
//! the start screen, and the run panel's ticket changes (R8) and write-back.
//!
//! Every network call runs on a blocking thread with no store lock held. The
//! token is read from the keychain inside the command and dropped with it;
//! no reply carries it.

use super::intake::{self, ChangedTicket, IntakePreview, IntakeRef};
use super::writeback::{self, PostOutcome, PostRequest, PostedDraft};
use super::{ClickUp, ClickUpError, ClickUpToken, KeychainStore, SecretStore, UreqTransport};
use crate::run::checked_run_dir;
use crate::store::settings_store::account_exists;
use crate::store::{SettingsStore, StateStore};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
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

/// The run's Claude account (meta.json), whose token the run uses.
fn run_account(run_dir: &Path) -> Result<String, String> {
    crate::run::folder::read_json_value(&run_dir.join("meta.json"))
        .and_then(|m| m.get("account")?.get("id")?.as_str().map(str::to_string))
        .ok_or_else(|| "This run's meta.json names no Claude account.".into())
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
    blocking(move || KeychainStore.set(&account_id, &token)).await
}

#[tauri::command]
pub async fn clickup_clear_token(app: AppHandle, account_id: String) -> Result<(), String> {
    known_account(&app, &account_id)?;
    blocking(move || KeychainStore.clear(&account_id)).await
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftView {
    #[serde(flatten)]
    pub draft: writeback::Draft,
    pub posted: PostedDraft,
    /// The comment's last send did not finish cleanly: resend needs a confirm.
    pub needs_confirm: bool,
    /// The ticket is in tickets.json (else it is never posted).
    pub in_tickets: bool,
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
    pub writeback_problem: Option<String>,
}

pub fn run_clickup(run_dir: &Path) -> RunClickUp {
    let mut out = RunClickUp::default();
    let tickets: HashSet<String> = match intake::read_snapshot(run_dir) {
        Ok(Some(snap)) => {
            out.has_tickets = true;
            out.ticket_count = snap.tasks.len();
            out.fetched_at = Some(snap.fetched_at.clone());
            snap.tasks.iter().map(|t| t.id.clone()).collect()
        }
        Ok(None) => HashSet::new(),
        Err(problem) => {
            out.has_tickets = true;
            out.tickets_problem = Some(problem);
            HashSet::new()
        }
    };
    let pulls = intake::read_pulls(run_dir);
    let mut pulled: Vec<String> = pulls.tickets.keys().cloned().collect();
    pulled.sort();
    out.pulled = pulled;
    if run_dir.join(intake::UPDATE_FILE).is_file() {
        out.update_file = Some(run_dir.join(intake::UPDATE_FILE).display().to_string());
    }
    match writeback::read_drafts(run_dir) {
        Ok(None) => {}
        Err(problem) => out.writeback_problem = Some(problem),
        Ok(Some(drafts)) => {
            let posted = writeback::read_posted(run_dir);
            if let Err(problem) = &posted {
                out.writeback_problem = Some(problem.clone());
            }
            let posted = posted.unwrap_or_default();
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
                            in_tickets: tickets.contains(&draft.ticket.id),
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
        let dir = checked_run_dir(&data_dir(&app), &run_dir)?;
        Ok(run_clickup(&dir))
    })
    .await
}

fn snapshot_of(dir: &Path) -> Result<intake::TicketsSnapshot, String> {
    intake::read_snapshot(dir)?.ok_or_else(|| "This run has no tickets.json.".to_string())
}

/// R8: which tickets ClickUp changed since the run's version of them.
#[tauri::command]
pub async fn clickup_check_tickets(
    app: AppHandle,
    run_dir: String,
) -> Result<Vec<ChangedTicket>, String> {
    blocking(move || {
        let dir = checked_run_dir(&data_dir(&app), &run_dir)?;
        let snap = snapshot_of(&dir)?;
        let token = token_of(&KeychainStore, &run_account(&dir)?)?;
        let transport = UreqTransport::new();
        intake::check_changes(
            &ClickUp::new(&transport, &token),
            &snap,
            &intake::read_pulls(&dir),
        )
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
        let dir = checked_run_dir(&data_dir(&app), &run_dir)?;
        let _lock = RunLock::take(&dir, "pull")?;
        let snap = snapshot_of(&dir)?;
        let token = token_of(&KeychainStore, &run_account(&dir)?)?;
        let transport = UreqTransport::new();
        let pulled = intake::pull_in(
            &ClickUp::new(&transport, &token),
            &dir,
            &snap,
            &ids,
            &now_text(),
        )?;
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

/// Posts the ticked drafts. One send per run at a time.
#[tauri::command]
pub async fn clickup_post_writeback(
    app: AppHandle,
    run_dir: String,
    requests: Vec<PostRequest>,
) -> Result<PostReply, String> {
    blocking(move || {
        let dir = checked_run_dir(&data_dir(&app), &run_dir)?;
        let _lock = RunLock::take(&dir, "post")?;
        let drafts = writeback::read_drafts(&dir)?.ok_or("This run has no writeback.json yet.")?;
        let tickets: HashSet<String> = snapshot_of(&dir)?.tasks.into_iter().map(|t| t.id).collect();
        let mut posted = writeback::read_posted(&dir)?;
        let token = token_of(&KeychainStore, &run_account(&dir)?)?;
        let transport = UreqTransport::new();
        let client = ClickUp::new(&transport, &token);
        let mut store = writeback::FileStore(&dir);
        Ok(
            match writeback::post(
                &client,
                &drafts,
                &tickets,
                &mut posted,
                &mut store,
                &requests,
                &now_text,
            ) {
                Ok(outcomes) => PostReply {
                    outcomes,
                    error: None,
                },
                Err((outcomes, error)) => PostReply {
                    outcomes,
                    error: Some(format!("writeback-posted.json: {error}")),
                },
            },
        )
    })
    .await
}

static BUSY: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

/// One ClickUp write (post or pull) per run folder at a time.
struct RunLock(PathBuf);

impl RunLock {
    fn take(dir: &Path, _what: &str) -> Result<Self, String> {
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
    use crate::clickup::secret::MemoryStore;
    use crate::test_support::TempDir;

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

    #[test]
    fn the_run_panel_view_joins_drafts_with_what_was_posted() {
        let dir = TempDir::new("run_clickup");
        assert!(!run_clickup(dir.path()).has_tickets);
        let snap = intake::TicketsSnapshot {
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
            tasks: vec![intake::tests::sample("86c0abcde", "Export invoices")],
        };
        intake::write_snapshot(dir.path(), &snap).unwrap();
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/clickup/writeback-skill.json");
        std::fs::copy(fixture, dir.join(writeback::DRAFTS_FILE)).unwrap();
        std::fs::write(
            dir.join(writeback::POSTED_FILE),
            r#"{"schema":1,"drafts":{"86c0abcde":{"comment":{"result":"sending","at":"t","value":"x"}}}}"#,
        )
        .unwrap();
        let view = run_clickup(dir.path());
        assert_eq!(view.ticket_count, 1);
        assert_eq!(view.fetched_at.as_deref(), Some("2026-10-23T10:00:00Z"));
        let drafts = view.drafts.unwrap();
        assert!(drafts[0].in_tickets);
        assert!(drafts[0].needs_confirm);
        let json = serde_json::to_value(&drafts[0]).unwrap();
        assert_eq!(json["ticket"]["customId"], "KON-123");
        assert_eq!(json["posted"]["comment"]["result"], "sending");
    }

    #[test]
    fn one_write_per_run_at_a_time() {
        let dir = TempDir::new("run_lock");
        let first = RunLock::take(dir.path(), "post").unwrap();
        assert!(RunLock::take(dir.path(), "pull").is_err());
        drop(first);
        assert!(RunLock::take(dir.path(), "post").is_ok());
    }
}
