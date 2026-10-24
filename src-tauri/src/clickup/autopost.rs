//! ClickUp comments posted on their own after the yes at stop #2 (plan
//! 2026-10-23 "ClickUp comments post automatically"). Statuses are never
//! posted here: they stay ticks in the panel.
//!
//! status.json (which says `shipApproval`) is written by Claude, so this
//! goes no further than what P4 already trusts: only tickets in the signed
//! run-trust snapshot, the run's own account's token (the caller's), each
//! comment once (the signed posted record: a `posted` comment is never sent
//! again and one that may have been posted waits for a confirm), the text
//! secret-scanned (memory.sh's rules; a hit is "blocked") and size-capped.
//! It runs once per run per writeback.json version (its sha-256, kept in
//! the signed `autopost.json` record), and a failed comment can be retried
//! from the panel.

use super::client::ClickUp;
use super::intake::SnapshotTask;
use super::trust::{self, Key};
use super::writeback::{self, PostRequest, MAX_COMMENT_CHARS};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

pub const AUTOPOST_FILE: &str = "autopost.json";
pub const BLOCKED_SECRET: &str = "blocked: looks like a secret";
pub const NOT_IN_SNAPSHOT: &str = "Not one of the run's tickets (signed snapshot), so not posted.";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoPostResult {
    pub ticket_id: String,
    /// `posted`, `failed` (retry), `maybe` (may have been posted: confirm in
    /// the ticks), `skipped` or `blocked` (looks like a secret).
    pub result: String,
    pub message: String,
    /// The ticket in ClickUp (from the app's snapshot).
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub at: String,
}

/// `run-trust/<runId>/autopost.json` (signed).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoPostRecord {
    pub schema: u32,
    /// sha-256 of the writeback.json these results are for.
    pub writeback_sha: String,
    pub at: String,
    pub results: Vec<AutoPostResult>,
}

/// status.json has `shipApproval` (an object: written by `state.sh approve`).
pub fn ship_approved(run_dir: &Path) -> bool {
    crate::run::folder::read_json_value(&run_dir.join("status.json"))
        .and_then(|v| v.get("shipApproval").cloned())
        .is_some_and(|v| v.is_object())
}

/// Cheap file checks before any keychain read: approved and drafts written.
pub fn may_be_due(run_dir: &Path) -> bool {
    run_dir.join(writeback::DRAFTS_FILE).is_file() && ship_approved(run_dir)
}

/// The writeback.json version to auto-post now, if any: approved, drafts
/// written, the run trusted, and not yet done for this version.
pub fn due(key: &Key, data: &Path, run_dir: &Path) -> Result<Option<String>, String> {
    if !may_be_due(run_dir) {
        return Ok(None);
    }
    let Some((_, version)) = writeback::read_drafts(run_dir)? else {
        return Ok(None);
    };
    if trust::load(key, data, run_dir)?.is_none() {
        return Ok(None);
    }
    let done = trust::read_autopost(key, data, run_dir)?;
    Ok((done.map(|r| r.writeback_sha) != Some(version.clone())).then_some(version))
}

/// Why a comment is not sent on its own, before any call.
fn refusal(text: &str, ticket: Option<&SnapshotTask>) -> Option<(&'static str, String)> {
    if ticket.is_none() {
        return Some(("skipped", NOT_IN_SNAPSHOT.into()));
    }
    if let Some(kind) = super::secret_scan::first_hit(text) {
        return Some((
            "blocked",
            format!("{BLOCKED_SECRET} ({kind}). Check it, then post it by hand from the ticks."),
        ));
    }
    if text.chars().count() > MAX_COMMENT_CHARS {
        return Some((
            "skipped",
            format!("Longer than {MAX_COMMENT_CHARS} characters, so not posted on its own."),
        ));
    }
    None
}

/// What the signed posted record says about a ticket's comment now.
fn result_of(
    posted: &writeback::Posted,
    id: &str,
    outcome: Option<&writeback::PostOutcome>,
) -> (String, String) {
    let part = posted.drafts.get(id).and_then(|d| d.comment.as_ref());
    let message = outcome.map(|o| o.message.clone()).unwrap_or_default();
    match part.map(|p| p.result.as_str()) {
        Some("posted") => ("posted".into(), message),
        Some("maybe") | Some("sending") => (
            "maybe".into(),
            if message.is_empty() {
                writeback::MAYBE_POSTED.into()
            } else {
                message
            },
        ),
        Some(_) if outcome.is_some_and(|o| o.result == "failed") => ("failed".into(), message),
        _ => ("skipped".into(), message),
    }
}

/// Posts every comment draft the rules allow (one ticket at a time), then
/// saves the signed record for this writeback.json version.
pub fn auto_post(
    client: &ClickUp,
    key: &Key,
    data: &Path,
    run_dir: &Path,
    now: &dyn Fn() -> String,
) -> Result<AutoPostRecord, String> {
    if !ship_approved(run_dir) {
        return Err("The ship approval is not in status.json.".into());
    }
    let run = trust::load(key, data, run_dir)?.ok_or("This run has no app-side ticket record.")?;
    let (drafts, version) =
        writeback::read_drafts(run_dir)?.ok_or("This run has no writeback.json yet.")?;
    let tickets: HashMap<String, SnapshotTask> = run
        .snapshot
        .tasks
        .into_iter()
        .map(|t| (t.id.clone(), t))
        .collect();
    let mut results: Vec<AutoPostResult> = Vec::new();
    let mut requests: Vec<PostRequest> = Vec::new();
    for draft in &drafts.drafts {
        let Some(text) = draft.comment.as_deref().filter(|t| !t.trim().is_empty()) else {
            continue;
        };
        let ticket = tickets.get(&draft.ticket.id);
        let url = ticket.map(|t| t.url.clone());
        if let Some((result, message)) = refusal(text, ticket) {
            results.push(AutoPostResult {
                ticket_id: draft.ticket.id.clone(),
                result: result.into(),
                message,
                url,
                at: now(),
            });
            continue;
        }
        requests.push(PostRequest {
            ticket_id: draft.ticket.id.clone(),
            comment: Some(text.to_string()),
            status: None,
            confirm_resend: false,
        });
    }
    let mut posted = trust::read_posted(key, data, run_dir)?;
    let mut store = trust::TrustStore { key, data, run_dir };
    let outcomes = writeback::post(
        client,
        &drafts,
        &tickets,
        &mut posted,
        &mut store,
        &requests,
        now,
    )
    // A save failed midway: no record, so it is looked at again (the
    // posted record keeps a comment that was sent from going twice).
    .map_err(|(_, err)| format!("The posted record could not be saved: {err}"))?;
    for request in &requests {
        let outcome = outcomes.iter().find(|o| o.ticket_id == request.ticket_id);
        let (result, message) = result_of(&posted, &request.ticket_id, outcome);
        results.push(AutoPostResult {
            url: tickets.get(&request.ticket_id).map(|t| t.url.clone()),
            ticket_id: request.ticket_id.clone(),
            result,
            message,
            at: now(),
        });
    }
    let record = AutoPostRecord {
        schema: 1,
        writeback_sha: version,
        at: now(),
        results,
    };
    trust::save_autopost(key, data, run_dir, &record)?;
    Ok(record)
}

/// Retry of one `failed` comment of the current record (same version).
pub fn retry(
    client: &ClickUp,
    key: &Key,
    data: &Path,
    run_dir: &Path,
    ticket_id: &str,
    now: &dyn Fn() -> String,
) -> Result<AutoPostRecord, String> {
    let run = trust::load(key, data, run_dir)?.ok_or("This run has no app-side ticket record.")?;
    let (drafts, version) =
        writeback::read_drafts(run_dir)?.ok_or("This run has no writeback.json yet.")?;
    let mut record =
        trust::read_autopost(key, data, run_dir)?.ok_or("Nothing was posted on its own yet.")?;
    if record.writeback_sha != version {
        return Err(writeback::DRAFTS_CHANGED.into());
    }
    let at = record
        .results
        .iter()
        .position(|r| r.ticket_id == ticket_id && r.result == "failed")
        .ok_or("Only a failed comment can be retried.")?;
    let tickets: HashMap<String, SnapshotTask> = run
        .snapshot
        .tasks
        .into_iter()
        .map(|t| (t.id.clone(), t))
        .collect();
    let draft = drafts
        .drafts
        .iter()
        .find(|d| d.ticket.id == ticket_id)
        .ok_or("No draft for this ticket in writeback.json.")?;
    let text = draft.comment.clone().unwrap_or_default();
    if let Some((result, message)) = refusal(&text, tickets.get(ticket_id)) {
        record.results[at].result = result.into();
        record.results[at].message = message;
    } else {
        let mut posted = trust::read_posted(key, data, run_dir)?;
        let mut store = trust::TrustStore { key, data, run_dir };
        let request = PostRequest {
            ticket_id: ticket_id.to_string(),
            comment: Some(text),
            status: None,
            confirm_resend: false,
        };
        let outcomes = writeback::post(
            client,
            &drafts,
            &tickets,
            &mut posted,
            &mut store,
            std::slice::from_ref(&request),
            now,
        )
        .map_err(|(_, err)| format!("The posted record could not be saved: {err}"))?;
        let (result, message) = result_of(&posted, ticket_id, outcomes.first());
        record.results[at].result = result;
        record.results[at].message = message;
    }
    record.results[at].at = now();
    trust::save_autopost(key, data, run_dir, &record)?;
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clickup::client::tests::{ok, status, Recorded};
    use crate::clickup::intake::{self, tests::sample};
    use crate::clickup::ClickUpToken;
    use crate::test_support::TempDir;
    use serde_json::json;
    use std::path::PathBuf;

    fn k() -> Key {
        trust::tests::key()
    }

    fn snap(ids: &[&str]) -> intake::TicketsSnapshot {
        let mut s = trust::tests::snap();
        s.tasks = ids.iter().map(|id| sample(id, "t")).collect();
        s
    }

    fn setup(tag: &str, approved: bool) -> (TempDir, PathBuf, PathBuf) {
        let dir = TempDir::new(tag);
        let data = dir.join("data");
        let run = data.join("runs/r-1");
        std::fs::create_dir_all(&run).unwrap();
        trust::write_start(
            &k(),
            &data,
            &run,
            "personal",
            &snap(&["86a1", "86a2", "86a3", "86a4"]),
        )
        .unwrap();
        std::fs::write(
            run.join("writeback.json"),
            json!({"schema": 1, "drafts": [
                {"ticket": {"id": "86a1"}, "comment": "Done: S1", "status": "in review"},
                {"ticket": {"id": "86a2"}, "comment": "Set DB_PASSWORD=s3cr3tPass! in prod", "status": null},
                {"ticket": {"id": "86a3"}, "comment": "Done: S3"},
                {"ticket": {"id": "86a4"}, "comment": null, "status": "done"},
                {"ticket": {"id": "86zz"}, "comment": "Not ours"}
            ]})
            .to_string(),
        )
        .unwrap();
        let status = if approved {
            json!({"schema": 1, "shipApproval": {"by": "user", "at": "t"}})
        } else {
            json!({"schema": 1, "shipApproval": null})
        };
        std::fs::write(run.join("status.json"), status.to_string()).unwrap();
        (dir, data, run)
    }

    fn token() -> ClickUpToken {
        ClickUpToken::parse("pk_test_autopost").unwrap()
    }

    #[test]
    fn after_ship_approval_only_trusted_clean_comments_post_once_and_never_statuses() {
        let (_dir, data, run) = setup("autopost", false);
        assert_eq!(due(&k(), &data, &run).unwrap(), None, "no approval yet");
        let (_d2, data, run) = {
            drop(_dir);
            setup("autopost2", true)
        };
        let version = due(&k(), &data, &run).unwrap().expect("due");
        assert_eq!(version.len(), 64);
        let api = Recorded::default()
            .on("POST /task/86a1/comment", ok("{}".into()))
            .on("POST /task/86a3/comment", status(500));
        let token = token();
        let client = ClickUp::new(&api, &token);
        let record = auto_post(&client, &k(), &data, &run, &|| "t".into()).unwrap();
        let paths = api.paths();
        assert!(
            paths
                .iter()
                .all(|p| p.starts_with("POST /task/") && p.ends_with("/comment")),
            "{paths:?}"
        );
        assert!(!paths
            .iter()
            .any(|p| p.contains("86a2") || p.contains("86zz") || p.contains("86a4")));
        let by = |id: &str| {
            record
                .results
                .iter()
                .find(|r| r.ticket_id == id)
                .unwrap()
                .clone()
        };
        assert_eq!(by("86a1").result, "posted");
        assert!(by("86a1").url.is_some());
        assert_eq!(by("86a2").result, "blocked");
        assert!(by("86a2").message.starts_with(BLOCKED_SECRET));
        assert!(
            !by("86a2").message.contains("s3cr3t"),
            "the value is never shown"
        );
        assert_eq!(
            by("86a3").result,
            "maybe",
            "a 500 may have posted: confirm in the ticks"
        );
        assert_eq!(by("86zz").result, "skipped");
        assert!(
            record.results.iter().all(|r| r.ticket_id != "86a4"),
            "no comment, nothing"
        );
        // Once per version: not due again, and nothing is sent twice.
        assert_eq!(due(&k(), &data, &run).unwrap(), None);
        let again = auto_post(&client, &k(), &data, &run, &|| "t".into()).unwrap();
        assert_eq!(api.paths().len(), paths.len(), "nothing sent again");
        assert_eq!(
            again
                .results
                .iter()
                .find(|r| r.ticket_id == "86a1")
                .unwrap()
                .result,
            "posted"
        );
        // Statuses stay ticks.
        let posted = trust::read_posted(&k(), &data, &run).unwrap();
        assert!(posted.drafts.values().all(|d| d.status.is_none()));
        // A new writeback.json version is due again.
        std::fs::write(
            run.join("writeback.json"),
            json!({"schema": 1, "drafts": [{"ticket": {"id": "86a1"}, "comment": "Done: S1 (v2)"}]}).to_string(),
        )
        .unwrap();
        assert!(due(&k(), &data, &run).unwrap().is_some());
    }

    #[test]
    fn a_failed_comment_can_be_retried_and_a_tampered_record_stops_everything() {
        let (_dir, data, run) = setup("autopost_retry", true);
        let api = Recorded::default()
            .on("POST /task/86a1/comment", status(401))
            .on("POST /task/86a3/comment", ok("{}".into()));
        let token = token();
        let client = ClickUp::new(&api, &token);
        let record = auto_post(&client, &k(), &data, &run, &|| "t".into()).unwrap();
        let one = record
            .results
            .iter()
            .find(|r| r.ticket_id == "86a1")
            .unwrap();
        assert_eq!(one.result, "failed", "{one:?}");
        assert!(
            retry(&client, &k(), &data, &run, "86a3", &|| "t".into()).is_err(),
            "posted: no retry"
        );
        let api = Recorded::default().on("POST /task/86a1/comment", ok("{}".into()));
        let client = ClickUp::new(&api, &token);
        let record = retry(&client, &k(), &data, &run, "86a1", &|| "t".into()).unwrap();
        assert_eq!(
            record
                .results
                .iter()
                .find(|r| r.ticket_id == "86a1")
                .unwrap()
                .result,
            "posted"
        );
        // A forged record is refused, never read as "not done yet".
        let path = trust::trust_dir(&data, &run).unwrap().join(AUTOPOST_FILE);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("posted", "failed")).unwrap();
        assert_eq!(due(&k(), &data, &run).unwrap_err(), trust::TAMPERED);
        // An untrusted run (no app record) is never due.
        trust::remove(&data, &run);
        assert_eq!(due(&k(), &data, &run).unwrap(), None);
    }
}
