//! The merged status draft (plan 2026-10-27, CAP-8). For a ticket of the
//! run's signed snapshot whose PRs (the PR ids of its `writeback.json`
//! draft) have all merged, the app offers one status change, "Merged: set
//! status to <s>". It is derived at read time, never written into
//! `writeback.json` (the skill's file), and posted only by a tick through
//! `clickup_post_merged_status`: `set_status` only, never a comment, never
//! twice, recorded as `mergedStatus` in the signed posted record.
//!
//! "Merged" is `prState: MERGED` in `app-github.json` or a `cleaned` / `kept`
//! outcome in `app-cleanup.json` (both app-written). The status is the
//! per-list mapping (`settings.json` `clickup.doneStatus[<list id>]`), else
//! the list's first status of type `closed`, else its first of type `done`.
//!
//! Reading the run's ClickUp view stays local: the list's statuses are read
//! (`GET /list/<id>`, a read) only when some ticket has all its PRs merged
//! and no merged status posted yet, are cached for 10 minutes (a failure for
//! 60 seconds), and a failure becomes a note on the merged row only.

use super::client::{ClickUp, TaskRef};
use super::intake::SnapshotTask;
use super::valid_id;
use super::writeback::{self, Draft, Drafts, PartResult, PostOutcome, Posted, PostedStore};
use crate::run::cleanup::CleanupFile;
use crate::run::github::GithubFile;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A list's statuses are kept this long before they are read again.
pub const LIST_TTL: Duration = Duration::from_secs(10 * 60);
/// A failed read is kept this long, so a broken connection is not retried on every reload.
pub const FAIL_TTL: Duration = Duration::from_secs(60);

/// One status of a ClickUp list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListStatus {
    pub name: String,
    /// `open`, `custom`, `done` or `closed`.
    pub kind: String,
}

/// What the panel shows for one ticket whose PRs all merged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergedDraft {
    pub ticket_id: String,
    /// The status to set; None when `note` says why there is none.
    pub status: Option<String>,
    /// "No done status in <list>." or "Could not read <list>'s statuses: <reason>".
    pub note: Option<String>,
    /// What an earlier post did.
    pub posted: Option<PartResult>,
}

type Fetched = Result<Vec<ListStatus>, String>;

/// Statuses by list id, with the time they were read.
#[derive(Default)]
pub struct ListCache(Mutex<HashMap<String, (Instant, Fetched)>>);

impl ListCache {
    /// The cached answer while it is fresh, else `fetch`'s (cached too). No
    /// lock is held while `fetch` runs.
    pub fn get(&self, list_id: &str, now: Instant, fetch: impl FnOnce() -> Fetched) -> Fetched {
        if let Some((at, answer)) = self.0.lock().ok().and_then(|m| m.get(list_id).cloned()) {
            let ttl = if answer.is_ok() { LIST_TTL } else { FAIL_TTL };
            if now.saturating_duration_since(at) < ttl {
                return answer;
            }
        }
        let answer = fetch();
        if let Ok(mut map) = self.0.lock() {
            map.insert(list_id.to_string(), (now, answer.clone()));
        }
        answer
    }
}

/// The app's one cache.
pub fn cache() -> &'static ListCache {
    static CACHE: std::sync::OnceLock<ListCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(ListCache::default)
}

/// Reads a list's statuses with `client` (a read).
pub fn fetch_statuses(client: &ClickUp, list_id: &str) -> Fetched {
    client
        .list(list_id)
        .map(|l| {
            l.statuses
                .into_iter()
                .map(|s| ListStatus {
                    name: s.status,
                    kind: s.kind,
                })
                .collect()
        })
        .map_err(|e| e.to_string())
}

/// Every PR of the draft has merged: `MERGED` in the GitHub record, or a
/// cleaned / kept outcome in the cleanup record. No PRs, or a PR with no id
/// or no record, is not merged.
pub fn all_merged(draft: &Draft, github: &GithubFile, cleanup: &CleanupFile) -> bool {
    !draft.prs.is_empty()
        && draft.prs.iter().all(|pr| {
            let Some(id) = pr.id.as_deref() else {
                return false;
            };
            github.prs.get(id).is_some_and(|g| g.pr_state == "MERGED")
                || cleanup
                    .prs
                    .get(id)
                    .is_some_and(|c| !c.retry && matches!(c.outcome.as_str(), "cleaned" | "kept"))
        })
}

/// The status to set in a list: the mapping if it is one of the list's,
/// else the first `closed`-type status, else the first `done`-type one.
/// `Err` is the note for the row.
pub fn pick_status(
    list_name: &str,
    mapped: Option<&str>,
    statuses: &[ListStatus],
) -> Result<String, String> {
    if let Some(wanted) = mapped.map(str::trim).filter(|m| !m.is_empty()) {
        return statuses
            .iter()
            .find(|s| s.name.trim().eq_ignore_ascii_case(wanted))
            .map(|s| s.name.clone())
            .ok_or_else(|| {
                format!("\"{wanted}\" (set for this list) is not a status in {list_name}.")
            });
    }
    ["closed", "done"]
        .iter()
        .find_map(|kind| statuses.iter().find(|s| s.kind == *kind))
        .map(|s| s.name.clone())
        .ok_or_else(|| format!("No done status in {list_name}."))
}

fn list_label(ticket: &SnapshotTask) -> String {
    ticket
        .list
        .as_ref()
        .map(|l| l.name.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "this ticket's list".into())
}

/// The merged drafts of a run. `statuses` reads one list's statuses (the
/// cache and the network); it is called only for a ticket with all PRs
/// merged and no merged status posted yet.
pub fn derive(
    tickets: &HashMap<String, SnapshotTask>,
    drafts: &Drafts,
    github: &GithubFile,
    cleanup: &CleanupFile,
    posted: &Posted,
    done_status: &BTreeMap<String, String>,
    statuses: &mut dyn FnMut(&str) -> Fetched,
) -> Vec<MergedDraft> {
    let mut out = Vec::new();
    for draft in &drafts.drafts {
        let id = &draft.ticket.id;
        let Some(ticket) = tickets.get(id) else {
            continue;
        };
        if !all_merged(draft, github, cleanup) {
            continue;
        }
        let made = |status: Option<String>, note: Option<String>, posted: Option<PartResult>| {
            MergedDraft {
                ticket_id: id.clone(),
                status,
                note,
                posted,
            }
        };
        let recorded = posted.drafts.get(id).and_then(|d| d.merged_status.clone());
        if let Some(part) = recorded.filter(|p| p.is("posted")) {
            out.push(made(Some(part.value.clone()), None, Some(part)));
            continue;
        }
        let name = list_label(ticket);
        let list_id = ticket
            .list
            .as_ref()
            .map(|l| l.id.as_str())
            .filter(|l| valid_id(l));
        let Some(list_id) = list_id else {
            out.push(made(
                None,
                Some(format!("No done status in {name}: its list is not known.")),
                None,
            ));
            continue;
        };
        let draft = match statuses(list_id) {
            Err(why) => made(
                None,
                Some(format!("Could not read {name}'s statuses: {why}")),
                None,
            ),
            Ok(list) => {
                match pick_status(&name, done_status.get(list_id).map(String::as_str), &list) {
                    Ok(status) => made(Some(status), None, None),
                    Err(note) => made(None, Some(note), None),
                }
            }
        };
        out.push(draft);
    }
    out
}

/// Posts the merged status of one ticket: recomputes the draft from a fresh
/// read of the list (no cache), refuses a different status, re-checks the
/// status against the ticket's list now (`writeback::check_status`), then
/// calls `set_status` and nothing else, and records `mergedStatus`. Never
/// sends twice once posted. `posted` and `store` are the signed record.
#[allow(clippy::too_many_arguments)]
pub fn post_merged(
    client: &ClickUp,
    tickets: &HashMap<String, SnapshotTask>,
    drafts: &Drafts,
    github: &GithubFile,
    cleanup: &CleanupFile,
    posted: &mut Posted,
    store: &mut dyn PostedStore,
    done_status: &BTreeMap<String, String>,
    ticket_id: &str,
    status: &str,
    now: &dyn Fn() -> String,
) -> Result<PostOutcome, String> {
    if let Some(fresh) = store.reload()? {
        *posted = fresh;
    }
    posted.schema = 1;
    let outcome = |result: &str, message: String| PostOutcome {
        ticket_id: ticket_id.into(),
        result: result.into(),
        message,
    };
    let ticket = tickets
        .get(ticket_id)
        .filter(|_| valid_id(ticket_id))
        .ok_or("This ticket is not in the run's snapshot, so nothing was sent.")?;
    let draft = drafts
        .drafts
        .iter()
        .find(|d| d.ticket.id == ticket_id)
        .ok_or("No draft for this ticket in writeback.json.")?;
    if !all_merged(draft, github, cleanup) {
        return Err("Not every PR of this ticket has merged, so no status was set.".into());
    }
    if let Some(part) = posted
        .drafts
        .get(ticket_id)
        .and_then(|d| d.merged_status.as_ref())
        .filter(|p| p.is("posted"))
    {
        return Ok(outcome(
            "not-sent",
            format!("Status already set to {}.", part.value),
        ));
    }
    let name = list_label(ticket);
    let list_id = ticket
        .list
        .as_ref()
        .map(|l| l.id.as_str())
        .filter(|l| valid_id(l))
        .ok_or_else(|| format!("No done status in {name}: its list is not known."))?;
    let wanted = pick_status(
        &name,
        done_status.get(list_id).map(String::as_str),
        &fetch_statuses(client, list_id)
            .map_err(|why| format!("Could not read {name}'s statuses: {why}"))?,
    )?;
    if !wanted.eq_ignore_ascii_case(status.trim()) {
        return Err(format!(
            "The merged status is now \"{wanted}\", not \"{status}\". Look at the row again."
        ));
    }
    let mut lists = HashMap::new();
    let result = writeback::check_status(client, &mut lists, ticket, &wanted).and_then(|()| {
        client
            .set_status(&TaskRef::Id(ticket_id.into()), &wanted)
            .map_err(|e| e.to_string())
    });
    let entry = posted.drafts.entry(ticket_id.into()).or_default();
    let text = match result {
        Ok(()) => {
            entry.merged_status = Some(PartResult {
                result: "posted".into(),
                at: now(),
                value: wanted.clone(),
                error: None,
            });
            outcome("posted", format!("status set to {wanted}"))
        }
        Err(message) => {
            entry.merged_status = Some(PartResult {
                result: "failed".into(),
                at: now(),
                value: wanted.clone(),
                error: Some(message.clone()),
            });
            outcome("failed", format!("Status not set: {message}"))
        }
    };
    store
        .save_ticket(posted, ticket_id)
        .map_err(|err| format!("The posted record could not be saved: {err}"))?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clickup::client::tests::{ok, status as http_status, Recorded};
    use crate::clickup::intake::tests::sample;
    use crate::clickup::ClickUpToken;
    use crate::run::cleanup::CleanupRecord;
    use crate::run::github::PrGithub;
    use serde_json::json;
    use std::cell::Cell;

    fn drafts() -> Drafts {
        serde_json::from_value(json!({
            "schema": 1,
            "drafts": [
                {"ticket": {"id": "86b1aa001"}, "prs": [{"id": "PR1"}, {"id": "PR2"}], "comment": "x"},
                {"ticket": {"id": "86b1aa002"}, "prs": [], "comment": "No PR"},
                {"ticket": {"id": "86b1aa003"}, "prs": [{"id": "PR9"}]},
                {"ticket": {"id": "86zzzz999"}, "prs": [{"id": "PR1"}]}
            ]
        }))
        .unwrap()
    }

    fn tickets() -> HashMap<String, SnapshotTask> {
        ["86b1aa001", "86b1aa002", "86b1aa003"]
            .into_iter()
            .map(|id| (id.to_string(), sample(id, "x")))
            .collect()
    }

    fn github(states: &[(&str, &str)]) -> GithubFile {
        GithubFile {
            schema: 1,
            prs: states
                .iter()
                .map(|(id, state)| {
                    (
                        id.to_string(),
                        PrGithub {
                            ci: "pass".into(),
                            pr_state: state.to_string(),
                            at: "t".into(),
                            error: None,
                        },
                    )
                })
                .collect(),
        }
    }

    fn cleaned(id: &str, outcome: &str, retry: bool) -> CleanupFile {
        let mut file = CleanupFile::default();
        file.prs.insert(
            id.into(),
            CleanupRecord {
                pr_id: id.into(),
                outcome: outcome.into(),
                retry,
                ..Default::default()
            },
        );
        file
    }

    fn list(kinds: &[(&str, &str)]) -> Fetched {
        Ok(kinds
            .iter()
            .map(|(n, k)| ListStatus {
                name: n.to_string(),
                kind: k.to_string(),
            })
            .collect())
    }

    fn standard() -> Fetched {
        list(&[
            ("to do", "open"),
            ("in progress", "custom"),
            ("shipped", "done"),
            ("complete", "closed"),
        ])
    }

    fn run_derive(
        gh: &GithubFile,
        cl: &CleanupFile,
        posted: &Posted,
        map: &BTreeMap<String, String>,
        statuses: &mut dyn FnMut(&str) -> Fetched,
    ) -> Vec<MergedDraft> {
        derive(&tickets(), &drafts(), gh, cl, posted, map, statuses)
    }

    #[test]
    fn a_ticket_whose_prs_all_merged_gets_a_draft_and_others_none() {
        let gh = github(&[("PR1", "MERGED"), ("PR2", "MERGED"), ("PR9", "OPEN")]);
        let out = run_derive(
            &gh,
            &CleanupFile::default(),
            &Posted::default(),
            &BTreeMap::new(),
            &mut |_| standard(),
        );
        // 86b1aa001 merged (both PRs); 002 has no PRs; 003's PR is open; 86zzzz999 is not in the snapshot.
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].ticket_id, "86b1aa001");
        assert_eq!(
            out[0].status.as_deref(),
            Some("complete"),
            "closed type first"
        );
        // One PR open: none. An unknown PR id (no record): none.
        let one_open = github(&[("PR1", "MERGED"), ("PR2", "OPEN")]);
        assert!(run_derive(
            &one_open,
            &CleanupFile::default(),
            &Posted::default(),
            &BTreeMap::new(),
            &mut |_| standard()
        )
        .is_empty());
        let unknown = github(&[("PR1", "MERGED")]);
        assert!(run_derive(
            &unknown,
            &CleanupFile::default(),
            &Posted::default(),
            &BTreeMap::new(),
            &mut |_| standard()
        )
        .is_empty());
    }

    #[test]
    fn a_cleanup_record_counts_as_merged_unless_it_is_a_retry() {
        let mut both = cleaned("PR1", "cleaned", false);
        both.prs.extend(cleaned("PR2", "kept", false).prs);
        let out = run_derive(
            &GithubFile::default(),
            &both,
            &Posted::default(),
            &BTreeMap::new(),
            &mut |_| standard(),
        );
        assert_eq!(out.len(), 1);
        let mut retry = cleaned("PR1", "cleaned", false);
        retry.prs.extend(cleaned("PR2", "kept", true).prs);
        assert!(run_derive(
            &GithubFile::default(),
            &retry,
            &Posted::default(),
            &BTreeMap::new(),
            &mut |_| standard()
        )
        .is_empty());
        let untouched = cleaned("PR1", "untouched", false);
        assert!(run_derive(
            &GithubFile::default(),
            &untouched,
            &Posted::default(),
            &BTreeMap::new(),
            &mut |_| standard()
        )
        .is_empty());
    }

    #[test]
    fn the_mapping_beats_the_type_and_a_missing_done_status_is_a_note() {
        let gh = github(&[("PR1", "MERGED"), ("PR2", "MERGED")]);
        let map: BTreeMap<String, String> = [("1".to_string(), "Shipped".to_string())].into();
        let out = run_derive(
            &gh,
            &CleanupFile::default(),
            &Posted::default(),
            &map,
            &mut |_| standard(),
        );
        assert_eq!(
            out[0].status.as_deref(),
            Some("shipped"),
            "the list's own spelling"
        );
        // A mapping that is not in the list is a note, never a guess.
        let bad: BTreeMap<String, String> = [("1".to_string(), "nope".to_string())].into();
        let out = run_derive(
            &gh,
            &CleanupFile::default(),
            &Posted::default(),
            &bad,
            &mut |_| standard(),
        );
        assert!(out[0].status.is_none());
        assert!(out[0].note.as_deref().unwrap().contains("not a status in"));
        // No closed or done type at all.
        let none = |_: &str| list(&[("to do", "open"), ("doing", "custom")]);
        let out = run_derive(
            &gh,
            &CleanupFile::default(),
            &Posted::default(),
            &BTreeMap::new(),
            &mut |l| none(l),
        );
        assert!(out[0].status.is_none());
        assert!(out[0]
            .note
            .as_deref()
            .unwrap()
            .starts_with("No done status in "));
        // `done` type when there is no `closed`.
        let done = |_: &str| list(&[("to do", "open"), ("finished", "done")]);
        let out = run_derive(
            &gh,
            &CleanupFile::default(),
            &Posted::default(),
            &BTreeMap::new(),
            &mut |l| done(l),
        );
        assert_eq!(out[0].status.as_deref(), Some("finished"));
    }

    #[test]
    fn a_read_failure_is_a_note_on_the_row_and_a_posted_ticket_needs_no_read() {
        let gh = github(&[("PR1", "MERGED"), ("PR2", "MERGED")]);
        let out = run_derive(
            &gh,
            &CleanupFile::default(),
            &Posted::default(),
            &BTreeMap::new(),
            &mut |_| Err("No ClickUp token yet.".into()),
        );
        assert_eq!(out.len(), 1);
        assert!(out[0].status.is_none());
        assert!(out[0].note.as_deref().unwrap().contains("Could not read"));
        assert!(out[0]
            .note
            .as_deref()
            .unwrap()
            .contains("No ClickUp token yet."));
        // Already posted: shown from the record, with no read at all.
        let mut posted = Posted::default();
        posted
            .drafts
            .entry("86b1aa001".into())
            .or_default()
            .merged_status = Some(PartResult {
            result: "posted".into(),
            at: "t".into(),
            value: "complete".into(),
            error: None,
        });
        let reads = Cell::new(0);
        let out = run_derive(
            &gh,
            &CleanupFile::default(),
            &posted,
            &BTreeMap::new(),
            &mut |_| {
                reads.set(reads.get() + 1);
                standard()
            },
        );
        assert_eq!(reads.get(), 0);
        assert_eq!(out[0].posted.as_ref().unwrap().value, "complete");
        // No merged ticket at all: no read.
        let open = github(&[("PR1", "OPEN")]);
        run_derive(
            &open,
            &CleanupFile::default(),
            &Posted::default(),
            &BTreeMap::new(),
            &mut |_| {
                reads.set(reads.get() + 1);
                standard()
            },
        );
        assert_eq!(reads.get(), 0, "a run with no merged ticket makes no GET");
    }

    #[test]
    fn the_list_cache_reads_once_within_the_ttl_and_a_failure_is_retried_after_a_minute() {
        let cache = ListCache::default();
        let calls = Cell::new(0);
        let t0 = Instant::now();
        let fetch = || {
            calls.set(calls.get() + 1);
            standard()
        };
        assert!(cache.get("1", t0, fetch).is_ok());
        assert!(cache.get("1", t0 + Duration::from_secs(3), fetch).is_ok());
        assert!(cache.get("1", t0 + Duration::from_secs(599), fetch).is_ok());
        assert_eq!(
            calls.get(),
            1,
            "two reloads and a third inside the TTL: one GET"
        );
        assert!(cache.get("1", t0 + Duration::from_secs(601), fetch).is_ok());
        assert_eq!(calls.get(), 2);
        // Another list is its own entry.
        assert!(cache.get("2", t0, fetch).is_ok());
        assert_eq!(calls.get(), 3);
        // A failure is kept 60 s, then retried.
        let fails = Cell::new(0);
        let bad = || {
            fails.set(fails.get() + 1);
            Err::<Vec<ListStatus>, String>("offline".into())
        };
        assert!(cache.get("9", t0, bad).is_err());
        assert!(cache.get("9", t0 + Duration::from_secs(30), bad).is_err());
        assert_eq!(fails.get(), 1);
        assert!(cache.get("9", t0 + Duration::from_secs(61), bad).is_err());
        assert_eq!(fails.get(), 2);
    }

    // ---- posting ----

    #[derive(Default)]
    struct Saves {
        saved: Vec<Posted>,
    }

    impl PostedStore for Saves {
        fn save(&mut self, posted: &Posted) -> Result<(), String> {
            self.saved.push(posted.clone());
            Ok(())
        }
    }

    const LIST: &str = "/list/1";
    const TASK1: &str = "/task/86b1aa001?include_subtasks=true&include_markdown_description=true";
    const SET1: &str = "PUT /task/86b1aa001";

    fn task_now() -> Result<crate::clickup::transport::Reply, crate::clickup::transport::NoReply> {
        ok(r#"{"id":"86b1aa001","name":"x","list":{"id":"1","name":"List 1"}}"#.into())
    }

    fn statuses() -> Result<crate::clickup::transport::Reply, crate::clickup::transport::NoReply> {
        ok(r#"{"id":"1","name":"Sprint","statuses":[{"status":"to do","type":"open"},{"status":"complete","type":"closed"}]}"#.into())
    }

    fn post(
        api: &Recorded,
        posted: &mut Posted,
        store: &mut Saves,
        status: &str,
    ) -> Result<PostOutcome, String> {
        let token = ClickUpToken::parse("pk_test_merged").unwrap();
        let client = ClickUp::new(api, &token);
        post_merged(
            &client,
            &tickets(),
            &drafts(),
            &github(&[("PR1", "MERGED"), ("PR2", "MERGED")]),
            &CleanupFile::default(),
            posted,
            store,
            &BTreeMap::new(),
            "86b1aa001",
            status,
            &|| "2026-10-27T14:00:00Z".into(),
        )
    }

    #[test]
    fn posting_sets_the_status_only_records_it_and_never_sends_twice() {
        let api = Recorded::default()
            .on(LIST, statuses())
            .on(TASK1, task_now())
            .on(LIST, statuses())
            .on(SET1, ok("{}".into()));
        let (mut posted, mut store) = (Posted::default(), Saves::default());
        let out = post(&api, &mut posted, &mut store, "complete").unwrap();
        assert_eq!(
            (out.result.as_str(), out.message.as_str()),
            ("posted", "status set to complete")
        );
        // A fresh list read, the ticket's list now, its statuses, and one PUT: no comment, nothing else.
        assert_eq!(api.paths(), [LIST, TASK1, LIST, SET1]);
        let writes = api.writes.lock().unwrap().clone();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].body, json!({"status": "complete"}));
        let part = posted.drafts["86b1aa001"].merged_status.clone().unwrap();
        assert_eq!(
            (part.result.as_str(), part.value.as_str()),
            ("posted", "complete")
        );
        assert!(
            posted.drafts["86b1aa001"].status.is_none()
                && posted.drafts["86b1aa001"].comment.is_none()
        );
        assert_eq!(store.saved.len(), 1);
        // A second post is a no-op: no call at all.
        let quiet = Recorded::default();
        let again = post(&quiet, &mut posted, &mut store, "complete").unwrap();
        assert_eq!(again.result, "not-sent");
        assert!(quiet.paths().is_empty());
    }

    #[test]
    fn a_different_status_is_refused_and_a_failed_set_is_recorded_and_retryable() {
        let api = Recorded::default().on(LIST, statuses());
        let (mut posted, mut store) = (Posted::default(), Saves::default());
        let err = post(&api, &mut posted, &mut store, "to do").unwrap_err();
        assert!(err.contains("is now \"complete\""), "{err}");
        assert!(api.writes.lock().unwrap().is_empty());
        assert!(store.saved.is_empty());
        // The server refuses the PUT: recorded as failed, not posted.
        let api = Recorded::default()
            .on(LIST, statuses())
            .on(TASK1, task_now())
            .on(LIST, statuses())
            .on(SET1, http_status(400));
        let out = post(&api, &mut posted, &mut store, "complete").unwrap();
        assert_eq!(out.result, "failed", "{out:?}");
        assert_eq!(
            posted.drafts["86b1aa001"]
                .merged_status
                .as_ref()
                .unwrap()
                .result,
            "failed"
        );
        // A failed one can be sent again.
        let api = Recorded::default()
            .on(LIST, statuses())
            .on(TASK1, task_now())
            .on(LIST, statuses())
            .on(SET1, ok("{}".into()));
        assert_eq!(
            post(&api, &mut posted, &mut store, "complete")
                .unwrap()
                .result,
            "posted"
        );
    }

    #[test]
    fn nothing_is_sent_when_a_pr_has_not_merged_or_the_ticket_is_foreign() {
        let token = ClickUpToken::parse("pk_test_merged").unwrap();
        let api = Recorded::default();
        let client = ClickUp::new(&api, &token);
        let (mut posted, mut store) = (Posted::default(), Saves::default());
        let run = |gh: GithubFile, id: &str, posted: &mut Posted, store: &mut Saves| {
            post_merged(
                &client,
                &tickets(),
                &drafts(),
                &gh,
                &CleanupFile::default(),
                posted,
                store,
                &BTreeMap::new(),
                id,
                "complete",
                &|| "t".into(),
            )
        };
        assert!(run(
            github(&[("PR1", "MERGED"), ("PR2", "OPEN")]),
            "86b1aa001",
            &mut posted,
            &mut store
        )
        .is_err());
        assert!(run(
            github(&[("PR1", "MERGED"), ("PR2", "MERGED")]),
            "86zzzz999",
            &mut posted,
            &mut store
        )
        .is_err());
        assert!(api.paths().is_empty());
    }
}
