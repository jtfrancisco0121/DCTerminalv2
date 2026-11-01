//! Write-back (P4): posting the drafts the skill wrote to `writeback.json`
//! (`state.sh writeback`, at shipping), only the ones you tick, and recording
//! what was sent in `writeback-posted.json`. The skill never rewrites
//! `writeback.json` after it exists and never reads `writeback-posted.json`.
//!
//! Ported from DCTerminal `clickup/writeback.rs` (`send_requests`): the
//! status is set first, then the comment; each step is saved before the next
//! call. Before a comment POST its record says `sending`; a POST that did not
//! finish cleanly (a quit or crash mid-call, a timeout, a broken connection,
//! a server error) stays `maybe`, and sending it again needs your
//! confirmation. A part already `posted` is never sent again.
//!
//! Ticks live in the page, not in a file: nothing in the run folder (which
//! Claude can write) can make the app post. Status and comment have their own
//! ticks. A request names its ticket, the comment text you saw (edited or
//! not) and the status you saw, which must still be the draft's; the whole
//! send is refused when writeback.json changed since the panel read it. Only
//! tickets of the app's own snapshot (`trust.rs`) are posted, and a status
//! must be one of the ticket's list. The authoritative posted record lives
//! app-side; `writeback-posted.json` in the run folder is a mirror.

use super::client::{ClickUp, ClickUpError, TaskRef};
use super::intake::{SnapshotTask, MAX_TICKETS};
use super::transport::NoReply;
use super::valid_id;
use crate::store::CappedError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

pub const DRAFTS_FILE: &str = "writeback.json";
pub const POSTED_FILE: &str = "writeback-posted.json";
pub const MAX_COMMENT_CHARS: usize = 10_000;
/// The largest writeback.json the app reads.
pub const MAX_DRAFTS_BYTES: usize = 2 * 1024 * 1024;
pub const DRAFTS_CHANGED: &str =
    "writeback.json changed since the panel showed it. Look at the drafts again, then tick and post.";

pub const MAYBE_POSTED: &str =
    "ClickUp may have posted the comment anyway: check the ticket in ClickUp before sending it again.";
pub const RESEND_NEEDS_CONFIRM: &str =
    "ClickUp may already have this comment (its last send did not finish cleanly). Check the ticket in ClickUp, then confirm to send it again.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DraftTicket {
    pub id: String,
    #[serde(default)]
    pub custom_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DraftPr {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

/// One draft of `writeback.json` (docs/run-folder.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub ticket: DraftTicket,
    #[serde(default)]
    pub prs: Vec<DraftPr>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub status_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Drafts {
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub drafts: Vec<Draft>,
}

/// What happened to one part (the comment, or the status) of a draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartResult {
    /// `posted`, `failed` (certainly not posted), `maybe` (may have been
    /// posted) or `sending` (the app stopped mid-call).
    pub result: String,
    pub at: String,
    /// The comment text or the status sent.
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl PartResult {
    pub(crate) fn is(&self, result: &str) -> bool {
        self.result == result
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostedDraft {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<PartResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<PartResult>,
    /// The "merged" status set from the merged draft (`merged.rs`), apart
    /// from the draft's own status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_status: Option<PartResult>,
}

/// `writeback-posted.json`: ticket id → what was sent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posted {
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub drafts: BTreeMap<String, PostedDraft>,
}

/// One ticked draft, as the page sends it. Status and comment are ticked apart.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostRequest {
    pub ticket_id: String,
    /// The comment as you saw it (edited in place or not); `None`: not ticked.
    #[serde(default)]
    pub comment: Option<String>,
    /// The status you saw (the draft's); `None`: not ticked.
    #[serde(default)]
    pub status: Option<String>,
    /// You confirmed sending a comment that may already be posted.
    #[serde(default)]
    pub confirm_resend: bool,
    /// You checked a comment the secret scan flagged and want it sent anyway.
    #[serde(default)]
    pub confirm_secret: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostOutcome {
    pub ticket_id: String,
    /// `posted`, `failed`, `not-sent`.
    pub result: String,
    pub message: String,
}

/// writeback.json and its version (sha-256 of the bytes), checked: schema
/// 1, at most [`MAX_DRAFTS_BYTES`] and [`MAX_TICKETS`] drafts, one draft per ticket.
pub fn read_drafts(run_dir: &Path) -> Result<Option<(Drafts, String)>, String> {
    // Never more than the cap is read, and a link or special file is refused.
    let bytes = match crate::store::read_capped(&run_dir.join(DRAFTS_FILE), MAX_DRAFTS_BYTES as u64)
    {
        Ok(bytes) => bytes,
        Err(CappedError::Missing) => return Ok(None),
        Err(CappedError::TooLarge) => {
            return Err("writeback.json is larger than 2 MB; the app does not read it.".into())
        }
        Err(CappedError::NotRegular) => {
            return Err("writeback.json is not a regular file; the app does not read it.".into())
        }
        Err(CappedError::Io(err)) => return Err(format!("writeback.json: {err}")),
    };
    let drafts: Drafts = serde_json::from_slice(&bytes)
        .map_err(|_| "writeback.json could not be read.".to_string())?;
    if drafts.schema != 1 {
        return Err(format!(
            "writeback.json has schema {}, which this app does not know.",
            drafts.schema
        ));
    }
    if drafts.drafts.len() > MAX_TICKETS {
        return Err(format!(
            "writeback.json has more than {MAX_TICKETS} drafts."
        ));
    }
    let mut seen = HashSet::new();
    if let Some(dup) = drafts
        .drafts
        .iter()
        .find(|d| !seen.insert(d.ticket.id.as_str()))
    {
        return Err(format!(
            "writeback.json has two drafts for ticket {}; nothing is posted from it.",
            dup.ticket.id
        ));
    }
    let version = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(Some((drafts, version)))
}

/// Where a send saves the posted record, so tests can count saves and fail one.
pub trait PostedStore {
    fn save(&mut self, posted: &Posted) -> Result<(), String>;

    /// Saves one ticket's entry. A store shared with other processes
    /// merges it into the record on disk instead of overwriting others'.
    fn save_ticket(&mut self, posted: &Posted, _ticket_id: &str) -> Result<(), String> {
        self.save(posted)
    }

    /// The record as it is on disk now (before each ticket), if the store
    /// can tell; `None` keeps the one in hand.
    fn reload(&mut self) -> Result<Option<Posted>, String> {
        Ok(None)
    }
}

/// An error after which ClickUp may still have acted on the call.
fn may_have_posted(err: &ClickUpError) -> bool {
    matches!(
        err,
        ClickUpError::NoReply(NoReply::Timeout | NoReply::Failed)
            | ClickUpError::Server(_)
            | ClickUpError::Unexpected(_)
    )
}

/// The comment needs your confirmation before it is sent again.
pub fn needs_confirm(part: Option<&PartResult>) -> bool {
    part.is_some_and(|p| p.is("maybe") || p.is("sending"))
}

fn check_comment(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("The comment is empty.".into());
    }
    if text.chars().count() > MAX_COMMENT_CHARS {
        return Err(format!(
            "The comment is longer than {MAX_COMMENT_CHARS} characters."
        ));
    }
    Ok(())
}

/// Whether `status` is one of the statuses of the list the ticket is in
/// NOW (read fresh: it may have moved since the snapshot), each list read
/// once per send. `Err` is the reason it is not sent.
pub(crate) fn check_status(
    client: &ClickUp,
    lists: &mut HashMap<String, Result<Vec<String>, String>>,
    ticket: &SnapshotTask,
    status: &str,
) -> Result<(), String> {
    let now = client
        .task(&TaskRef::Id(ticket.id.clone()))
        .map_err(|e| format!("Could not read the ticket's list: {e}"))?;
    let (list_id, list_name) = (now.list.id.clone(), now.list.name.trim().to_string());
    if !valid_id(&list_id) {
        return Err("The ticket's list is not known, so its statuses cannot be checked.".into());
    }
    let statuses = lists.entry(list_id.clone()).or_insert_with(|| {
        client
            .list(&list_id)
            .map(|l| l.statuses.into_iter().map(|s| s.status).collect())
            .map_err(|e| format!("Could not read the statuses of {list_name}: {e}"))
    });
    let statuses = statuses.as_ref().map_err(String::clone)?;
    if statuses
        .iter()
        .any(|s| s.trim().eq_ignore_ascii_case(status.trim()))
    {
        Ok(())
    } else {
        let name = if list_name.is_empty() {
            &list_id
        } else {
            &list_name
        };
        Err(format!("\"{status}\" is not a status in {name}."))
    }
}

/// Sends each request the files allow, one at a time, saving each step.
/// `tickets`: the app's own snapshot (a draft for any other ticket is never
/// sent). The status and the comment are independent: a refused status
/// does not stop the comment. Stops at a save failure.
pub fn post(
    client: &ClickUp,
    drafts: &Drafts,
    tickets: &HashMap<String, SnapshotTask>,
    posted: &mut Posted,
    store: &mut dyn PostedStore,
    requests: &[PostRequest],
    now: &dyn Fn() -> String,
) -> Result<Vec<PostOutcome>, (Vec<PostOutcome>, String)> {
    let mut out: Vec<PostOutcome> = Vec::new();
    let mut seen = HashSet::new();
    let mut lists = HashMap::new();
    posted.schema = 1;
    macro_rules! save {
        ($id:expr) => {
            if let Err(err) = store.save_ticket(posted, $id) {
                return Err((out, err));
            }
        };
    }
    for request in requests {
        let id = request.ticket_id.clone();
        if !seen.insert(id.clone()) {
            continue;
        }
        // Another window or app may have posted since: read it again.
        match store.reload() {
            Ok(Some(fresh)) => {
                *posted = fresh;
                posted.schema = 1;
            }
            Ok(None) => {}
            Err(err) => return Err((out, err)),
        }
        let outcome = |result: &str, message: String| PostOutcome {
            ticket_id: id.clone(),
            result: result.into(),
            message,
        };
        let Some(draft) = drafts.drafts.iter().find(|d| d.ticket.id == id) else {
            out.push(outcome(
                "not-sent",
                "No draft for this ticket in writeback.json.".into(),
            ));
            continue;
        };
        let Some(ticket) = tickets.get(&id).filter(|_| valid_id(&id)) else {
            out.push(outcome(
                "not-sent",
                "This ticket is not in the run's snapshot, so nothing was sent.".into(),
            ));
            continue;
        };
        let record = posted.drafts.get(&id).cloned().unwrap_or_default();
        let task = TaskRef::Id(id.clone());
        let mut done: Vec<String> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();
        if let Some(status) = request.status.as_deref() {
            if draft.status.as_deref() != Some(status) {
                skipped.push("Status not set: it is not the draft's status.".into());
            } else if record
                .status
                .as_ref()
                .is_some_and(|p| p.is("posted") && p.value == status)
            {
                skipped.push("Status already set.".into());
            } else {
                let result = check_status(client, &mut lists, ticket, status)
                    .and_then(|()| client.set_status(&task, status).map_err(|e| e.to_string()));
                let entry = posted.drafts.entry(id.clone()).or_default();
                match result {
                    Ok(()) => {
                        entry.status = Some(PartResult {
                            result: "posted".into(),
                            at: now(),
                            value: status.into(),
                            error: None,
                        });
                        done.push(format!("status set to {status}"));
                    }
                    Err(message) => {
                        entry.status = Some(PartResult {
                            result: "failed".into(),
                            at: now(),
                            value: status.into(),
                            error: Some(message.clone()),
                        });
                        failed.push(format!("Status not set: {message}"));
                    }
                }
                save!(&id);
            }
        }
        if let Some(text) = request.comment.as_deref() {
            if draft.comment.is_none() {
                skipped.push("This draft has no comment.".into());
            } else if record.comment.as_ref().is_some_and(|p| p.is("posted")) {
                skipped.push("Comment already posted.".into());
            } else if let Err(why) = check_comment(text) {
                skipped.push(why);
            } else if let Some(kind) =
                super::secret_scan::first_hit(text).filter(|_| !request.confirm_secret)
            {
                // The same scan auto-post applies: a hit goes out only with the user's confirm.
                skipped.push(format!(
                    "Comment not sent: looks like it contains {kind}. Check the comment, tick the confirm box, then post again."
                ));
            } else if needs_confirm(record.comment.as_ref()) && !request.confirm_resend {
                skipped.push(RESEND_NEEDS_CONFIRM.into());
            } else {
                // Until ClickUp answers cleanly, the comment may be posted.
                posted.drafts.entry(id.clone()).or_default().comment = Some(PartResult {
                    result: "sending".into(),
                    at: now(),
                    value: text.into(),
                    error: None,
                });
                save!(&id);
                let result = client.post_comment(&task, text);
                let entry = posted.drafts.entry(id.clone()).or_default();
                let at = now();
                match result {
                    Ok(()) => {
                        entry.comment = Some(PartResult {
                            result: "posted".into(),
                            at,
                            value: text.into(),
                            error: None,
                        });
                        done.push("comment posted".into());
                    }
                    Err(err) => {
                        let maybe = may_have_posted(&err);
                        let message = if maybe {
                            format!("{err} {MAYBE_POSTED}")
                        } else {
                            err.to_string()
                        };
                        entry.comment = Some(PartResult {
                            result: if maybe { "maybe" } else { "failed" }.into(),
                            at,
                            value: text.into(),
                            error: Some(message.clone()),
                        });
                        failed.push(format!("Comment not posted: {message}"));
                    }
                }
                save!(&id);
            }
        }
        let mut message: Vec<String> = Vec::new();
        if !done.is_empty() {
            message.push(done.join(", "));
        }
        message.extend(failed.iter().cloned());
        message.extend(skipped.iter().cloned());
        let result = if !failed.is_empty() {
            "failed"
        } else if !done.is_empty() {
            "posted"
        } else {
            "not-sent"
        };
        if message.is_empty() {
            message.push("Nothing ticked.".into());
        }
        out.push(outcome(result, message.join(" ")));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clickup::client::tests::{ok, status, Recorded};
    use crate::clickup::intake::tests::sample;
    use crate::clickup::ClickUpToken;

    #[derive(Default)]
    struct Saves {
        saved: Vec<Posted>,
        fail_at: Option<usize>,
    }

    impl PostedStore for Saves {
        fn save(&mut self, posted: &Posted) -> Result<(), String> {
            if self.fail_at == Some(self.saved.len()) {
                return Err("disk full".into());
            }
            self.saved.push(posted.clone());
            Ok(())
        }
    }

    fn drafts() -> Drafts {
        serde_json::from_value(serde_json::json!({
            "schema": 1,
            "runId": "r-1",
            "drafts": [
                {"ticket": {"id": "86b1aa001", "customId": "QA-129", "title": "Sync", "url": "u"},
                 "prs": [{"id": "PR1", "repo": "r", "url": "https://github.com/o/r/pull/1"}],
                 "comment": "Done: S1", "status": "in review", "statusNote": null},
                {"ticket": {"id": "86b1aa002", "customId": null, "title": "Logs", "url": "u"},
                 "prs": [], "comment": "No PR", "status": null, "statusNote": "No PR covers this ticket."},
                {"ticket": {"id": "86zzzz999", "title": "Not ours"}, "comment": "x", "status": "done"}
            ]
        }))
        .unwrap()
    }

    /// The app's snapshot: both tickets in list "1" ("Sprint").
    fn tickets() -> HashMap<String, SnapshotTask> {
        ["86b1aa001", "86b1aa002"]
            .into_iter()
            .map(|id| (id.to_string(), sample(id, "x")))
            .collect()
    }

    const LIST: &str = "/list/1";
    const TASK1: &str = "/task/86b1aa001?include_subtasks=true&include_markdown_description=true";
    /// The ticket as ClickUp has it now: in list 1, unless `list` says otherwise.
    fn task_now(
        list: &str,
    ) -> Result<crate::clickup::transport::Reply, crate::clickup::transport::NoReply> {
        ok(format!(
            r#"{{"id":"86b1aa001","name":"x","list":{{"id":"{list}","name":"List {list}"}}}}"#
        ))
    }
    fn statuses() -> Result<crate::clickup::transport::Reply, crate::clickup::transport::NoReply> {
        ok(
            r#"{"id":"1","name":"Sprint","statuses":[{"status":"open"},{"status":"In Review"}]}"#
                .into(),
        )
    }

    fn req(id: &str, comment: Option<&str>, status: Option<&str>) -> PostRequest {
        PostRequest {
            ticket_id: id.into(),
            comment: comment.map(str::to_string),
            status: status.map(str::to_string),
            confirm_resend: false,
            confirm_secret: false,
        }
    }

    fn token() -> ClickUpToken {
        ClickUpToken::parse("pk_test_writeback").unwrap()
    }

    const COMMENT1: &str = "POST /task/86b1aa001/comment";
    const STATUS1: &str = "PUT /task/86b1aa001";

    fn send(
        api: &Recorded,
        posted: &mut Posted,
        saves: &mut Saves,
        r: &[PostRequest],
    ) -> Vec<PostOutcome> {
        let token = token();
        let client = ClickUp::new(api, &token);
        post(&client, &drafts(), &tickets(), posted, saves, r, &|| {
            "t".into()
        })
        .unwrap()
    }

    #[test]
    fn only_ticked_parts_are_posted_status_first_and_each_step_saved() {
        let api = Recorded::default()
            .on(TASK1, task_now("1"))
            .on(LIST, statuses())
            .on(STATUS1, ok("{}".into()))
            .on(COMMENT1, ok("{}".into()));
        let mut posted = Posted::default();
        let mut saves = Saves::default();
        let out = send(
            &api,
            &mut posted,
            &mut saves,
            &[req("86b1aa001", Some("Edited: S1"), Some("in review"))],
        );
        assert_eq!(out[0].result, "posted", "{out:?}");
        assert_eq!(api.paths(), [TASK1, LIST, STATUS1, COMMENT1]);
        let writes = api.writes.lock().unwrap().clone();
        assert_eq!(writes[0].body, serde_json::json!({"status": "in review"}));
        assert_eq!(
            writes[1].body["comment_text"], "Edited: S1",
            "the edited text is what is sent"
        );
        // Saved after the status, before the POST (sending) and after it.
        assert_eq!(saves.saved.len(), 3);
        assert_eq!(
            saves.saved[1].drafts["86b1aa001"]
                .comment
                .as_ref()
                .unwrap()
                .result,
            "sending"
        );
        let rec = &posted.drafts["86b1aa001"];
        assert_eq!(rec.comment.as_ref().unwrap().result, "posted");
        assert_eq!(rec.status.as_ref().unwrap().value, "in review");
        assert!(!serde_json::to_string(&posted).unwrap().contains("pk_test"));
        // The comment alone: no status call.
        let api = Recorded::default().on("POST /task/86b1aa002/comment", ok("{}".into()));
        let out = send(
            &api,
            &mut posted,
            &mut saves,
            &[req("86b1aa002", Some("No PR"), None)],
        );
        assert_eq!(out[0].result, "posted");
        assert_eq!(api.paths(), ["POST /task/86b1aa002/comment"]);
    }

    #[test]
    fn a_ticked_comment_the_secret_scan_flags_is_sent_only_with_the_confirm() {
        const TOKEN_TEXT: &str = "Fixed, key ghp_abcdefghijklmnopqrstuvwxyz0123 was used";
        let api = Recorded::default().on("POST /task/86b1aa002/comment", ok("{}".into()));
        let mut posted = Posted::default();
        let mut saves = Saves::default();
        // Without the confirm: this ticket is not sent (and says why), the other ticked one is.
        let out = send(
            &api,
            &mut posted,
            &mut saves,
            &[
                req("86b1aa001", Some(TOKEN_TEXT), None),
                req("86b1aa002", Some("No PR"), None),
            ],
        );
        assert_eq!(out[0].result, "not-sent", "{out:?}");
        assert!(
            out[0]
                .message
                .contains("looks like it contains a GitHub token"),
            "{}",
            out[0].message
        );
        assert!(!out[0].message.contains("ghp_"), "the value is never shown");
        assert_eq!(out[1].result, "posted", "{out:?}");
        assert_eq!(api.paths(), ["POST /task/86b1aa002/comment"]);
        assert!(
            !posted.drafts.contains_key("86b1aa001"),
            "nothing recorded for the refused one"
        );
        // With the confirm for that ticket: sent.
        let api = Recorded::default().on(COMMENT1, ok("{}".into()));
        let mut confirmed = req("86b1aa001", Some(TOKEN_TEXT), None);
        confirmed.confirm_secret = true;
        let out = send(&api, &mut posted, &mut saves, &[confirmed]);
        assert_eq!(out[0].result, "posted", "{out:?}");
        assert_eq!(api.paths(), [COMMENT1]);
        assert_eq!(
            api.writes.lock().unwrap()[0].body["comment_text"],
            TOKEN_TEXT
        );
        // The confirm covers a comment that was edited clean too: no flag, no confirm needed.
        let api = Recorded::default().on("POST /task/86b1aa002/comment", ok("{}".into()));
        let again = req("86b1aa002", Some("No PR"), None);
        assert_eq!(
            send(&api, &mut posted, &mut saves, &[again])[0].message,
            "Comment already posted."
        );
    }

    #[test]
    fn a_posted_part_is_never_posted_twice() {
        let api = Recorded::default()
            .on(TASK1, task_now("1"))
            .on(LIST, statuses())
            .on(STATUS1, ok("{}".into()))
            .on(COMMENT1, ok("{}".into()));
        let mut posted = Posted::default();
        let mut saves = Saves::default();
        let r = [req("86b1aa001", Some("Done: S1"), Some("in review"))];
        send(&api, &mut posted, &mut saves, &r);
        let again = send(&api, &mut posted, &mut saves, &r);
        assert_eq!(again[0].result, "not-sent");
        assert_eq!(
            again[0].message,
            "Status already set. Comment already posted."
        );
        assert_eq!(api.paths().len(), 4, "no second call");
        // A duplicate request in one send is sent once.
        let mut fresh = Posted::default();
        let api = Recorded::default().on("POST /task/86b1aa002/comment", ok("{}".into()));
        let r = [
            req("86b1aa002", Some("No PR"), None),
            req("86b1aa002", Some("No PR"), None),
        ];
        let out = send(&api, &mut fresh, &mut saves, &r);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn drafts_outside_the_snapshot_a_foreign_status_or_an_empty_comment_are_never_sent() {
        let api = Recorded::default();
        let mut posted = Posted::default();
        let mut saves = Saves::default();
        let out = send(
            &api,
            &mut posted,
            &mut saves,
            &[
                req("86zzzz999", Some("x"), Some("done")),
                req("nothere1", Some("x"), None),
                req("86b1aa001", Some("  "), Some("closed")),
                req("86b1aa002", None, Some("done")),
            ],
        );
        assert!(out.iter().all(|o| o.result == "not-sent"), "{out:?}");
        assert!(out[2].message.contains("not the draft's status"));
        assert!(out[3].message.contains("not the draft's status"));
        assert!(api.paths().is_empty());
        assert!(saves.saved.is_empty());
    }

    #[test]
    fn a_status_not_in_the_list_or_refused_does_not_block_the_comment() {
        let api = Recorded::default()
            .on(TASK1, task_now("1"))
            .on(
                LIST,
                ok(r#"{"id":"1","name":"Sprint","statuses":[{"status":"open"}]}"#.into()),
            )
            .on(COMMENT1, ok("{}".into()));
        let mut posted = Posted::default();
        let mut saves = Saves::default();
        let out = send(
            &api,
            &mut posted,
            &mut saves,
            &[req("86b1aa001", Some("Done: S1"), Some("in review"))],
        );
        assert_eq!(out[0].result, "failed");
        assert!(
            out[0].message.contains("comment posted"),
            "{}",
            out[0].message
        );
        assert!(out[0]
            .message
            .contains("\"in review\" is not a status in List 1"));
        assert_eq!(
            api.paths(),
            [TASK1, LIST, COMMENT1],
            "no PUT for a status the list lacks"
        );
        assert_eq!(
            posted.drafts["86b1aa001"].status.as_ref().unwrap().result,
            "failed"
        );
        // ClickUp refusing the PUT: the comment still goes.
        let api = Recorded::default()
            .on(TASK1, task_now("1"))
            .on(LIST, statuses())
            .on(STATUS1, status(422))
            .on("POST /task/86b1aa001/comment", ok("{}".into()));
        let mut posted = Posted::default();
        let out = send(
            &api,
            &mut posted,
            &mut saves,
            &[req("86b1aa001", Some("Done: S1"), Some("in review"))],
        );
        assert!(
            out[0].message.contains("Status not set"),
            "{}",
            out[0].message
        );
        assert_eq!(
            posted.drafts["86b1aa001"].comment.as_ref().unwrap().result,
            "posted"
        );
    }

    #[test]
    fn the_status_is_checked_against_the_list_the_ticket_is_in_now() {
        // The snapshot says list 1; the ticket moved to list 2, which lacks the status.
        let api = Recorded::default().on(TASK1, task_now("2")).on(
            "/list/2",
            ok(r#"{"id":"2","name":"Done list","statuses":[{"status":"closed"}]}"#.into()),
        );
        let mut posted = Posted::default();
        let mut saves = Saves::default();
        let out = send(
            &api,
            &mut posted,
            &mut saves,
            &[req("86b1aa001", None, Some("in review"))],
        );
        assert!(
            out[0].message.contains("not a status in List 2"),
            "{}",
            out[0].message
        );
        assert_eq!(api.paths(), [TASK1, "/list/2"]);
    }

    #[test]
    fn a_comment_that_may_have_posted_needs_confirmation_and_a_refusal_can_retry() {
        let api = Recorded::default().on(COMMENT1, Err(NoReply::Timeout));
        let mut posted = Posted::default();
        let mut saves = Saves::default();
        let r = [req("86b1aa001", Some("Done: S1"), None)];
        let out = send(&api, &mut posted, &mut saves, &r);
        assert_eq!(out[0].result, "failed");
        assert!(
            out[0].message.contains("may have posted"),
            "{}",
            out[0].message
        );
        assert_eq!(
            posted.drafts["86b1aa001"].comment.as_ref().unwrap().result,
            "maybe"
        );
        let out = send(&api, &mut posted, &mut saves, &r);
        assert_eq!(out[0].message, RESEND_NEEDS_CONFIRM);
        let api = Recorded::default().on(COMMENT1, ok("{}".into()));
        let mut confirmed = r[0].clone();
        confirmed.confirm_resend = true;
        let out = send(&api, &mut posted, &mut saves, &[confirmed]);
        assert_eq!(out[0].result, "posted");
        // A 400 is certainly not posted: failed, and a plain retry is allowed.
        let api = Recorded::default()
            .on("POST /task/86b1aa002/comment", status(400))
            .on("POST /task/86b1aa002/comment", ok("{}".into()));
        let r = [req("86b1aa002", Some("No PR"), None)];
        send(&api, &mut posted, &mut saves, &r);
        assert_eq!(
            posted.drafts["86b1aa002"].comment.as_ref().unwrap().result,
            "failed"
        );
        let out = send(&api, &mut posted, &mut saves, &r);
        assert_eq!(out[0].result, "posted");
    }

    #[test]
    fn a_crash_mid_post_reads_as_sending() {
        let api = Recorded::default()
            .on(TASK1, task_now("1"))
            .on(LIST, statuses())
            .on(STATUS1, ok("{}".into()))
            .on(COMMENT1, ok("{}".into()));
        let token = token();
        let client = ClickUp::new(&api, &token);
        let mut posted = Posted::default();
        let mut saves = Saves {
            fail_at: Some(2),
            ..Saves::default()
        };
        let r = [req("86b1aa001", Some("Done: S1"), Some("in review"))];
        let err = post(
            &client,
            &drafts(),
            &tickets(),
            &mut posted,
            &mut saves,
            &r,
            &|| "t".into(),
        )
        .unwrap_err();
        assert_eq!(err.1, "disk full");
        let on_disk = saves.saved.last().unwrap();
        assert_eq!(
            on_disk.drafts["86b1aa001"].comment.as_ref().unwrap().result,
            "sending"
        );
        assert!(needs_confirm(on_disk.drafts["86b1aa001"].comment.as_ref()));
    }

    #[test]
    fn writeback_json_is_read_with_a_version_and_checked() {
        let dir = crate::test_support::TempDir::new("wb_read");
        assert!(read_drafts(dir.path()).unwrap().is_none());
        std::fs::write(dir.join(DRAFTS_FILE), r#"{"schema":2,"drafts":[]}"#).unwrap();
        assert!(read_drafts(dir.path()).is_err());
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures/clickup/writeback-skill.json");
        std::fs::copy(&fixture, dir.join(DRAFTS_FILE)).unwrap();
        let (d, version) = read_drafts(dir.path()).unwrap().unwrap();
        assert_eq!(d.drafts[0].ticket.custom_id.as_deref(), Some("KON-123"));
        assert_eq!(d.drafts[0].status.as_deref(), Some("in review"));
        assert_eq!(version.len(), 64);
        std::fs::write(
            dir.join(DRAFTS_FILE),
            std::fs::read_to_string(&fixture)
                .unwrap()
                .replace("in review", "done"),
        )
        .unwrap();
        assert_ne!(
            read_drafts(dir.path()).unwrap().unwrap().1,
            version,
            "a changed file has a new version"
        );
        let dup = r#"{"schema":1,"drafts":[{"ticket":{"id":"a1"}},{"ticket":{"id":"a1"}}]}"#;
        std::fs::write(dir.join(DRAFTS_FILE), dup).unwrap();
        assert!(read_drafts(dir.path()).unwrap_err().contains("two drafts"));
        let many: Vec<_> = (0..=MAX_TICKETS)
            .map(|n| serde_json::json!({"ticket": {"id": format!("t{n}")}}))
            .collect();
        std::fs::write(
            dir.join(DRAFTS_FILE),
            serde_json::json!({"schema": 1, "drafts": many}).to_string(),
        )
        .unwrap();
        assert!(read_drafts(dir.path()).is_err());
        std::fs::write(dir.join(DRAFTS_FILE), vec![b' '; MAX_DRAFTS_BYTES + 1]).unwrap();
        assert!(read_drafts(dir.path()).unwrap_err().contains("2 MB"));
    }

    /// LOW 11: the app's tickets.json → `state.sh writeback` → read_drafts. Needs bash and python3
    /// (the Windows gate runs the plugin suites instead).
    #[cfg(unix)]
    #[test]
    fn the_skill_writes_drafts_from_the_app_s_tickets_json() {
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let dir = crate::test_support::TempDir::new("wb_roundtrip");
        let run = dir.join("r-1");
        std::fs::create_dir_all(&run).unwrap();
        let mut task = sample("86b1aa001", "Export invoices");
        task.custom_id = Some("QA-129".into());
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
            tasks: vec![task, sample("86b1aa002", "Logs")],
        };
        crate::clickup::intake::write_snapshot(&run, &snap).unwrap();
        std::fs::write(
            run.join("plan.md"),
            "# Plan\n\n### S1: Export\n- Tickets: QA-129\n\n## Tickets\n- 86b1aa001 | QA-129 | Export invoices | u | stories: S1 | ship status: in review\n- 86b1aa002 | - | Logs | u | stories: - | ship status: -\n",
        )
        .unwrap();
        std::fs::write(
            run.join("status.json"),
            r#"{"schema":1,"runId":"r-1","stage":"shipping","prs":[{"id":"PR1","repo":"r","branch":"dcv2/r-1-pr1","state":"opened","prUrl":"https://github.com/o/r/pull/1","stories":{"ids":["S1"]},"check":{"command":"x","result":"pass"},"review":{"verdict":"APPROVED","round":1}}]}"#,
        )
        .unwrap();
        let state =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugin/scripts/state.sh");
        let out = std::process::Command::new("bash")
            .arg(&state)
            .args(["writeback", run.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let (d, _) = read_drafts(&run).unwrap().unwrap();
        assert_eq!(d.drafts.len(), 2);
        assert_eq!(d.drafts[0].ticket.id, "86b1aa001");
        assert_eq!(d.drafts[0].ticket.custom_id.as_deref(), Some("QA-129"));
        assert_eq!(d.drafts[0].ticket.title.as_deref(), Some("Export invoices"));
        assert_eq!(d.drafts[0].status.as_deref(), Some("in review"));
        assert_eq!(
            d.drafts[0].prs[0].url.as_deref(),
            Some("https://github.com/o/r/pull/1")
        );
        assert!(d.drafts[1].status.is_none());
    }

    #[test]
    fn writeback_json_is_read_capped_and_never_through_a_link() {
        let dir = crate::test_support::TempDir::new("wb_read_cap");
        // 3 MB sparse: refused from its length, nothing past the cap is read.
        std::fs::File::create(dir.join(DRAFTS_FILE))
            .unwrap()
            .set_len(3 * 1024 * 1024)
            .unwrap();
        assert!(read_drafts(dir.path()).unwrap_err().contains("2 MB"));
        #[cfg(unix)]
        {
            let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../fixtures/clickup/writeback-skill.json");
            let real = dir.join("real.json");
            std::fs::copy(&fixture, &real).unwrap();
            std::fs::remove_file(dir.join(DRAFTS_FILE)).unwrap();
            std::os::unix::fs::symlink(&real, dir.join(DRAFTS_FILE)).unwrap();
            assert!(read_drafts(dir.path())
                .unwrap_err()
                .contains("not a regular file"));
        }
    }
}
