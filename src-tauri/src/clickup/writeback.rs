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
//! Claude can write) can make the app post. A request names its ticket, the
//! comment text you saw (edited or not) and whether to set the status; the
//! status sent is always the draft's own.

use super::client::{ClickUp, ClickUpError, TaskRef};
use super::transport::NoReply;
use super::valid_id;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

pub const DRAFTS_FILE: &str = "writeback.json";
pub const POSTED_FILE: &str = "writeback-posted.json";
pub const MAX_COMMENT_CHARS: usize = 10_000;

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
    fn is(&self, result: &str) -> bool {
        self.result == result
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostedDraft {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<PartResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<PartResult>,
}

/// `writeback-posted.json`: ticket id → what was sent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posted {
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub drafts: BTreeMap<String, PostedDraft>,
}

/// One ticked draft, as the page sends it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostRequest {
    pub ticket_id: String,
    /// The comment as you saw it (edited in place or not); `None` sends no comment.
    #[serde(default)]
    pub comment: Option<String>,
    /// Set the draft's suggested status.
    #[serde(default)]
    pub status: bool,
    /// You confirmed sending a comment that may already be posted.
    #[serde(default)]
    pub confirm_resend: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostOutcome {
    pub ticket_id: String,
    /// `posted`, `failed`, `not-sent`.
    pub result: String,
    pub message: String,
}

pub fn read_drafts(run_dir: &Path) -> Result<Option<Drafts>, String> {
    let text = match std::fs::read_to_string(run_dir.join(DRAFTS_FILE)) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("writeback.json: {err}")),
    };
    let drafts: Drafts =
        serde_json::from_str(&text).map_err(|_| "writeback.json could not be read.".to_string())?;
    if drafts.schema != 1 {
        return Err(format!(
            "writeback.json has schema {}, which this app does not know.",
            drafts.schema
        ));
    }
    Ok(Some(drafts))
}

pub fn read_posted(run_dir: &Path) -> Result<Posted, String> {
    match std::fs::read_to_string(run_dir.join(POSTED_FILE)) {
        Ok(text) => serde_json::from_str(&text).map_err(|_| {
            "writeback-posted.json could not be read; nothing is posted until it is fixed or removed.".to_string()
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Posted { schema: 1, ..Posted::default() }),
        Err(err) => Err(format!("writeback-posted.json: {err}")),
    }
}

/// Where a send saves `writeback-posted.json`, so tests can count saves
/// and fail one.
pub trait PostedStore {
    fn save(&mut self, posted: &Posted) -> Result<(), String>;
}

pub struct FileStore<'a>(pub &'a Path);

impl PostedStore for FileStore<'_> {
    fn save(&mut self, posted: &Posted) -> Result<(), String> {
        crate::store::write_json_atomic(&self.0.join(POSTED_FILE), posted)
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

/// Sends each request the files allow, one at a time, saving each step.
/// `tickets`: the ids in the run's `tickets.json` (a draft for any other
/// ticket is never sent). Stops at a save failure.
pub fn post(
    client: &ClickUp,
    drafts: &Drafts,
    tickets: &HashSet<String>,
    posted: &mut Posted,
    store: &mut dyn PostedStore,
    requests: &[PostRequest],
    now: &dyn Fn() -> String,
) -> Result<Vec<PostOutcome>, (Vec<PostOutcome>, String)> {
    let mut out: Vec<PostOutcome> = Vec::new();
    let mut seen = HashSet::new();
    posted.schema = 1;
    macro_rules! save {
        () => {
            if let Err(err) = store.save(posted) {
                return Err((out, err));
            }
        };
    }
    for request in requests {
        let id = request.ticket_id.clone();
        if !seen.insert(id.clone()) {
            continue;
        }
        let not_sent = |message: &str| PostOutcome {
            ticket_id: id.clone(),
            result: "not-sent".into(),
            message: message.into(),
        };
        let Some(draft) = drafts.drafts.iter().find(|d| d.ticket.id == id) else {
            out.push(not_sent("No draft for this ticket in writeback.json."));
            continue;
        };
        if !tickets.contains(&id) || !valid_id(&id) {
            out.push(not_sent(
                "This ticket is not in the run's tickets.json, so nothing was sent.",
            ));
            continue;
        }
        let record = posted.drafts.get(&id).cloned().unwrap_or_default();
        let want_status = request
            .status
            .then_some(draft.status.as_deref())
            .flatten()
            .filter(|s| !s.trim().is_empty())
            .filter(|s| {
                !record
                    .status
                    .as_ref()
                    .is_some_and(|p| p.is("posted") && p.value == *s)
            });
        let want_comment = request
            .comment
            .as_deref()
            .filter(|_| !record.comment.as_ref().is_some_and(|p| p.is("posted")));
        if let Some(text) = want_comment {
            if let Err(why) = check_comment(text) {
                out.push(not_sent(&why));
                continue;
            }
            if needs_confirm(record.comment.as_ref()) && !request.confirm_resend {
                out.push(not_sent(RESEND_NEEDS_CONFIRM));
                continue;
            }
        }
        if want_status.is_none() && want_comment.is_none() {
            out.push(not_sent("Already posted."));
            continue;
        }
        let task = TaskRef::Id(id.clone());
        let mut done: Vec<String> = Vec::new();
        if let Some(status) = want_status {
            let result = client.set_status(&task, status);
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
                    save!();
                }
                Err(err) => {
                    let message = err.to_string();
                    entry.status = Some(PartResult {
                        result: "failed".into(),
                        at: now(),
                        value: status.into(),
                        error: Some(message.clone()),
                    });
                    save!();
                    out.push(PostOutcome {
                        ticket_id: id.clone(),
                        result: "failed".into(),
                        message: format!("Status not set: {message}"),
                    });
                    continue;
                }
            }
        }
        if let Some(text) = want_comment {
            // Until ClickUp answers cleanly, the comment may be posted.
            posted.drafts.entry(id.clone()).or_default().comment = Some(PartResult {
                result: "sending".into(),
                at: now(),
                value: text.into(),
                error: None,
            });
            save!();
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
                    save!();
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
                    save!();
                    let before = if done.is_empty() {
                        String::new()
                    } else {
                        format!(" ({}.)", done.join(", "))
                    };
                    out.push(PostOutcome {
                        ticket_id: id.clone(),
                        result: "failed".into(),
                        message: format!("Comment not posted: {message}{before}"),
                    });
                    continue;
                }
            }
        }
        out.push(PostOutcome {
            ticket_id: id.clone(),
            result: "posted".into(),
            message: done.join(", "),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clickup::client::tests::{ok, status, Recorded};
    use crate::clickup::transport::Reply;
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

    fn tickets() -> HashSet<String> {
        ["86b1aa001".to_string(), "86b1aa002".to_string()].into()
    }

    fn req(id: &str, comment: Option<&str>, status: bool) -> PostRequest {
        PostRequest {
            ticket_id: id.into(),
            comment: comment.map(str::to_string),
            status,
            confirm_resend: false,
        }
    }

    fn token() -> ClickUpToken {
        ClickUpToken::parse("pk_test_writeback").unwrap()
    }

    const COMMENT1: &str = "POST /task/86b1aa001/comment";
    const STATUS1: &str = "PUT /task/86b1aa001";

    #[test]
    fn only_ticked_drafts_are_posted_status_first_and_each_step_saved() {
        let api = Recorded::default()
            .on(STATUS1, ok("{}".into()))
            .on(COMMENT1, ok("{}".into()));
        let token = token();
        let client = ClickUp::new(&api, &token);
        let mut posted = Posted::default();
        let mut saves = Saves::default();
        let out = post(
            &client,
            &drafts(),
            &tickets(),
            &mut posted,
            &mut saves,
            &[req("86b1aa001", Some("Edited: S1"), true)],
            &|| "t".into(),
        )
        .unwrap();
        assert_eq!(out[0].result, "posted", "{out:?}");
        assert_eq!(api.paths(), [STATUS1, COMMENT1]);
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
    }

    #[test]
    fn a_posted_draft_is_never_posted_twice() {
        let api = Recorded::default()
            .on(STATUS1, ok("{}".into()))
            .on(COMMENT1, ok("{}".into()));
        let token = token();
        let client = ClickUp::new(&api, &token);
        let mut posted = Posted::default();
        let mut saves = Saves::default();
        let r = [req("86b1aa001", Some("Done: S1"), true)];
        post(
            &client,
            &drafts(),
            &tickets(),
            &mut posted,
            &mut saves,
            &r,
            &|| "t".into(),
        )
        .unwrap();
        let again = post(
            &client,
            &drafts(),
            &tickets(),
            &mut posted,
            &mut saves,
            &r,
            &|| "t2".into(),
        )
        .unwrap();
        assert_eq!(again[0].result, "not-sent");
        assert_eq!(again[0].message, "Already posted.");
        assert_eq!(api.paths().len(), 2, "no second call");
        // A duplicate request in one send is sent once.
        let mut fresh = Posted::default();
        let api = Recorded::default().on("POST /task/86b1aa002/comment", ok("{}".into()));
        let client = ClickUp::new(&api, &token);
        let r = [
            req("86b1aa002", Some("No PR"), true),
            req("86b1aa002", Some("No PR"), true),
        ];
        let out = post(
            &client,
            &drafts(),
            &tickets(),
            &mut fresh,
            &mut saves,
            &r,
            &|| "t".into(),
        )
        .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(
            api.paths(),
            ["POST /task/86b1aa002/comment"],
            "no status: the draft has none"
        );
    }

    #[test]
    fn drafts_for_tickets_outside_tickets_json_or_unknown_are_never_sent() {
        let api = Recorded::default();
        let token = token();
        let client = ClickUp::new(&api, &token);
        let mut posted = Posted::default();
        let mut saves = Saves::default();
        let out = post(
            &client,
            &drafts(),
            &tickets(),
            &mut posted,
            &mut saves,
            &[
                req("86zzzz999", Some("x"), true),
                req("nothere1", Some("x"), false),
                req("86b1aa001", Some("  "), false),
            ],
            &|| "t".into(),
        )
        .unwrap();
        assert!(out.iter().all(|o| o.result == "not-sent"), "{out:?}");
        assert!(api.paths().is_empty());
        assert!(saves.saved.is_empty());
    }

    #[test]
    fn a_comment_that_may_have_posted_needs_confirmation_and_a_refusal_can_retry() {
        let token = token();
        let api = Recorded::default().on(COMMENT1, Err(NoReply::Timeout));
        let client = ClickUp::new(&api, &token);
        let mut posted = Posted::default();
        let mut saves = Saves::default();
        let r = [req("86b1aa001", Some("Done: S1"), false)];
        let out = post(
            &client,
            &drafts(),
            &tickets(),
            &mut posted,
            &mut saves,
            &r,
            &|| "t".into(),
        )
        .unwrap();
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
        // Without confirmation: nothing sent.
        let out = post(
            &client,
            &drafts(),
            &tickets(),
            &mut posted,
            &mut saves,
            &r,
            &|| "t".into(),
        )
        .unwrap();
        assert_eq!(out[0].message, RESEND_NEEDS_CONFIRM);
        // With it: sent.
        let api = Recorded::default().on(COMMENT1, ok("{}".into()));
        let client = ClickUp::new(&api, &token);
        let mut confirmed = r[0].clone();
        confirmed.confirm_resend = true;
        let out = post(
            &client,
            &drafts(),
            &tickets(),
            &mut posted,
            &mut saves,
            &[confirmed],
            &|| "t".into(),
        )
        .unwrap();
        assert_eq!(out[0].result, "posted");
        // A 400 is certainly not posted: failed, and a plain retry is allowed.
        let api = Recorded::default()
            .on("POST /task/86b1aa002/comment", status(400))
            .on("POST /task/86b1aa002/comment", ok("{}".into()));
        let client = ClickUp::new(&api, &token);
        let r = [req("86b1aa002", Some("No PR"), false)];
        let out = post(
            &client,
            &drafts(),
            &tickets(),
            &mut posted,
            &mut saves,
            &r,
            &|| "t".into(),
        )
        .unwrap();
        assert_eq!(
            posted.drafts["86b1aa002"].comment.as_ref().unwrap().result,
            "failed",
            "{out:?}"
        );
        let out = post(
            &client,
            &drafts(),
            &tickets(),
            &mut posted,
            &mut saves,
            &r,
            &|| "t".into(),
        )
        .unwrap();
        assert_eq!(out[0].result, "posted");
    }

    #[test]
    fn a_status_failure_skips_the_comment_and_a_crash_mid_post_reads_as_sending() {
        let token = token();
        let api = Recorded::default().on(STATUS1, status(422));
        let client = ClickUp::new(&api, &token);
        let mut posted = Posted::default();
        let mut saves = Saves::default();
        let r = [req("86b1aa001", Some("Done: S1"), true)];
        let out = post(
            &client,
            &drafts(),
            &tickets(),
            &mut posted,
            &mut saves,
            &r,
            &|| "t".into(),
        )
        .unwrap();
        assert!(
            out[0].message.starts_with("Status not set"),
            "{}",
            out[0].message
        );
        assert_eq!(api.paths(), [STATUS1]);
        assert!(posted.drafts["86b1aa001"].comment.is_none());
        // The save after the POST fails (a stand-in for a crash): the file says sending.
        let api = Recorded::default()
            .on(STATUS1, ok("{}".into()))
            .on(COMMENT1, ok("{}".into()));
        let client = ClickUp::new(&api, &token);
        let mut posted = Posted::default();
        let mut saves = Saves {
            fail_at: Some(2),
            ..Saves::default()
        };
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
        // The status, posted, is not set again on the next send.
        let mut reread = on_disk.clone();
        let api = Recorded::default();
        let client = ClickUp::new(&api, &token);
        let out = post(
            &client,
            &drafts(),
            &tickets(),
            &mut reread,
            &mut Saves::default(),
            &[req("86b1aa001", None, true)],
            &|| "t".into(),
        )
        .unwrap();
        assert_eq!(out[0].message, "Already posted.");
        assert!(api.paths().is_empty());
        let _ = Reply::default();
    }

    #[test]
    fn the_skill_s_writeback_json_reads_and_an_unknown_schema_is_refused() {
        let dir = crate::test_support::TempDir::new("wb_read");
        assert!(read_drafts(dir.path()).unwrap().is_none());
        std::fs::write(dir.join(DRAFTS_FILE), r#"{"schema":2,"drafts":[]}"#).unwrap();
        assert!(read_drafts(dir.path()).is_err());
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures/clickup/writeback-skill.json");
        std::fs::copy(fixture, dir.join(DRAFTS_FILE)).unwrap();
        let d = read_drafts(dir.path()).unwrap().unwrap();
        assert_eq!(d.drafts[0].ticket.custom_id.as_deref(), Some("KON-123"));
        assert_eq!(d.drafts[0].status.as_deref(), Some("in review"));
        assert_eq!(
            read_posted(dir.path()).unwrap(),
            Posted {
                schema: 1,
                ..Posted::default()
            }
        );
        std::fs::write(dir.join(POSTED_FILE), "{bad").unwrap();
        assert!(
            read_posted(dir.path()).is_err(),
            "a damaged record never reads as nothing posted"
        );
    }
}
