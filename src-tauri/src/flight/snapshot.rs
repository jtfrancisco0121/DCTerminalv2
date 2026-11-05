//! The run snapshot (`<run>/flight/`, docs/run-folder.md): the three files of one terminal record, built
//! as text. Pure: the host hands in the parsed sessions, the rendered (already redacted) content and the
//! old snapshot's text, and writes the files itself.
//!
//! - `manifest.json`: what the snapshot holds and which record it belongs to.
//! - `events.jsonl`: one schema-1 Event per line, no content; always complete.
//! - `content.jsonl`: only under `redacted` or `full`: the redacted text behind an Event's source line,
//!   found again by the line's sha256. Stops at the cap and says `truncated`.

use super::schema::{CapturePolicy, Event};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const SCHEMA: u32 = 1;
/// A snapshot's events and content together stop growing the content past this.
pub const MAX_BYTES: usize = 64 * 1024 * 1024;

/// A recorded session the snapshot covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapSession {
    pub id: String,
    pub config_dir: String,
}

/// A transcript file the Events were read from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapSource {
    pub file: String,
    pub size: u64,
    pub mtime: i64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema: u32,
    pub created_at: String,
    pub policy: CapturePolicy,
    pub sessions: Vec<SnapSession>,
    pub sources: Vec<SnapSource>,
    pub events: usize,
    pub content_lines: usize,
    pub truncated: bool,
    /// The run record's outcome this snapshot belongs to: `done`, `failed` or `eval-done`.
    pub outcome: String,
    /// The record's `statusUpdatedAt`; a later change of either makes a new snapshot.
    pub record_status_updated_at: Option<String>,
}

/// One line of `content.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentLine {
    /// The first Event that has this source line.
    pub id: String,
    /// The sha256 of the transcript line: how a read finds it again.
    pub sha256: String,
    pub text: String,
    pub redactions: u32,
}

/// A live session: parsed now.
#[derive(Debug, Clone)]
pub struct SessionIn {
    pub session: SnapSession,
    pub events: Vec<Event>,
    pub sources: Vec<SnapSource>,
}

/// An old snapshot's text, to carry sessions over from.
#[derive(Debug, Clone)]
pub struct Old {
    pub manifest: Manifest,
    pub events_jsonl: String,
    pub content_jsonl: Option<String>,
}

/// What survives from the old snapshot, unchanged: the sessions whose transcripts are gone.
#[derive(Debug, Clone, Default)]
pub struct Carry {
    pub sessions: Vec<SnapSession>,
    pub sources: Vec<SnapSource>,
    pub events: Vec<String>,
    pub content: Vec<String>,
}

/// The old snapshot's Events and content lines of the sessions in `gone`, as they were written.
pub fn carry_over(old: &Old, gone: &[String]) -> Carry {
    let sessions: Vec<SnapSession> = old
        .manifest
        .sessions
        .iter()
        .filter(|s| gone.contains(&s.id))
        .cloned()
        .collect();
    let ids: HashSet<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    let mut events = Vec::new();
    let mut event_ids: HashSet<String> = HashSet::new();
    let mut files: HashSet<String> = HashSet::new();
    for line in old.events_jsonl.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(event) = serde_json::from_str::<Event>(line) else {
            continue;
        };
        if ids.contains(event.session.as_str()) {
            event_ids.insert(event.id.clone());
            files.insert(event.content.source.file.clone());
            events.push(line.to_string());
        }
    }
    let content = old
        .content_jsonl
        .iter()
        .flat_map(|text| text.lines())
        .filter(|line| {
            serde_json::from_str::<ContentLine>(line)
                .map(|c| event_ids.contains(&c.id))
                .unwrap_or(false)
        })
        .map(str::to_string)
        .collect();
    let sources = old
        .manifest
        .sources
        .iter()
        .filter(|s| files.contains(&s.file))
        .cloned()
        .collect();
    Carry {
        sessions,
        sources,
        events,
        content,
    }
}

pub struct Input<'a> {
    pub created_at: String,
    pub policy: CapturePolicy,
    pub outcome: String,
    pub record_status_updated_at: Option<String>,
    /// The sessions parsed now, in record order.
    pub live: &'a [SessionIn],
    pub carry: Carry,
    /// The redacted text of the live sessions' source lines (used only under `redacted`/`full`).
    pub content: Vec<ContentLine>,
    pub cap: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub manifest: Manifest,
    pub events_jsonl: String,
    pub content_jsonl: Option<String>,
}

fn keeps_content(policy: CapturePolicy) -> bool {
    matches!(policy, CapturePolicy::Redacted | CapturePolicy::Full)
}

/// The three files' text. Events are always all written; content stops at `cap` bytes (events and content
/// together) and sets `truncated`. Under `metadata` there is no content at all.
pub fn build(input: Input<'_>) -> Result<Built, String> {
    let Input {
        created_at,
        policy,
        outcome,
        record_status_updated_at,
        live,
        carry,
        content,
        cap,
    } = input;
    let mut events_jsonl = String::new();
    let mut events = 0usize;
    for line in &carry.events {
        events_jsonl.push_str(line);
        events_jsonl.push('\n');
        events += 1;
    }
    for session in live {
        for event in &session.events {
            let line = serde_json::to_string(event).map_err(|e| format!("event: {e}"))?;
            events_jsonl.push_str(&line);
            events_jsonl.push('\n');
            events += 1;
        }
    }
    let mut sessions = carry.sessions.clone();
    sessions.extend(live.iter().map(|s| s.session.clone()));
    let mut sources = carry.sources.clone();
    sources.extend(live.iter().flat_map(|s| s.sources.clone()));

    let mut truncated = false;
    let mut content_lines = 0usize;
    let content_jsonl = if keeps_content(policy) {
        let mut text = String::new();
        let mut used = events_jsonl.len();
        let mut seen: HashSet<String> = HashSet::new();
        let carried = carry.content.iter().cloned();
        let fresh = content.iter().filter_map(|c| {
            // One line per source line: the sha256 is the key.
            seen.insert(c.sha256.clone())
                .then(|| serde_json::to_string(c).ok())
                .flatten()
        });
        for line in carried.chain(fresh.collect::<Vec<_>>()) {
            if used + line.len() + 1 > cap {
                truncated = true;
                break;
            }
            used += line.len() + 1;
            text.push_str(&line);
            text.push('\n');
            content_lines += 1;
        }
        Some(text)
    } else {
        None
    };
    Ok(Built {
        manifest: Manifest {
            schema: SCHEMA,
            created_at,
            policy,
            sessions,
            sources,
            events,
            content_lines,
            truncated,
            outcome,
            record_status_updated_at,
        },
        events_jsonl,
        content_jsonl,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flight::schema::{AgentRef, Content, Provider, SourceRef};
    use serde_json::Map;

    fn event(session: &str, id: &str, line: &str) -> Event {
        Event {
            schema: 1,
            id: id.into(),
            trace: "t".into(),
            session: session.into(),
            agent: AgentRef::main(),
            provider: Provider {
                name: "claude".into(),
                version: None,
                native: "user".into(),
            },
            kind: "message".into(),
            at: "2026-11-05T10:00:00Z".into(),
            span: None,
            status: None,
            name: None,
            attrs: Map::new(),
            usage: None,
            content: Content {
                source: SourceRef {
                    file: format!("/c/{session}.jsonl"),
                    offset: 0,
                    len: line.len() as u64,
                    sha256: format!("sha-{line}"),
                },
                policy: CapturePolicy::Metadata,
                redactions: 0,
            },
            derived: None,
        }
    }

    fn live(session: &str, ids: &[&str]) -> SessionIn {
        SessionIn {
            session: SnapSession {
                id: session.into(),
                config_dir: "/c".into(),
            },
            events: ids.iter().map(|id| event(session, id, id)).collect(),
            sources: vec![SnapSource {
                file: format!("/c/{session}.jsonl"),
                size: 10,
                mtime: 1,
                sha256: "f".into(),
            }],
        }
    }

    fn lines(live: &[SessionIn]) -> Vec<ContentLine> {
        live.iter()
            .flat_map(|s| s.events.iter())
            .map(|e| ContentLine {
                id: e.id.clone(),
                sha256: e.content.source.sha256.clone(),
                text: format!("text of {}", e.id),
                redactions: 0,
            })
            .collect()
    }

    fn input(policy: CapturePolicy, sessions: &[SessionIn], carry: Carry, cap: usize) -> Input<'_> {
        Input {
            created_at: "2026-11-05T10:00:00Z".into(),
            policy,
            outcome: "done".into(),
            record_status_updated_at: Some("t1".into()),
            live: sessions,
            carry,
            content: lines(sessions),
            cap,
        }
    }

    #[test]
    fn the_metadata_policy_writes_events_and_refs_but_no_content() {
        let sessions = [live("s1", &["e1", "e2"])];
        let built = build(input(
            CapturePolicy::Metadata,
            &sessions,
            Carry::default(),
            MAX_BYTES,
        ))
        .unwrap();
        assert!(built.content_jsonl.is_none());
        assert_eq!(built.manifest.events, 2);
        assert_eq!(built.manifest.content_lines, 0);
        assert!(!built.manifest.truncated);
        assert_eq!(built.manifest.outcome, "done");
        assert_eq!(
            built.manifest.record_status_updated_at.as_deref(),
            Some("t1")
        );
        assert_eq!(built.manifest.sessions[0].id, "s1");
        assert_eq!(built.events_jsonl.lines().count(), 2);
        assert!(
            !built.events_jsonl.contains("text of"),
            "events hold no content"
        );
    }

    #[test]
    fn redacted_and_full_keep_one_content_line_per_source_line() {
        for policy in [CapturePolicy::Redacted, CapturePolicy::Full] {
            let mut sessions = [live("s1", &["e1", "e2"])];
            // Two Events from the same transcript line share one content line.
            sessions[0].events[1].content.source.sha256 =
                sessions[0].events[0].content.source.sha256.clone();
            let built = build(input(policy, &sessions, Carry::default(), MAX_BYTES)).unwrap();
            let content = built.content_jsonl.unwrap();
            assert_eq!(content.lines().count(), 1);
            assert_eq!(built.manifest.content_lines, 1);
            let line: ContentLine = serde_json::from_str(content.lines().next().unwrap()).unwrap();
            assert_eq!((line.id.as_str(), line.text.as_str()), ("e1", "text of e1"));
        }
    }

    #[test]
    fn the_cap_stops_content_and_sets_truncated_while_events_stay_complete() {
        let sessions = [live("s1", &["e1", "e2", "e3", "e4"])];
        let uncapped = build(input(
            CapturePolicy::Full,
            &sessions,
            Carry::default(),
            MAX_BYTES,
        ))
        .unwrap();
        let cap = uncapped.events_jsonl.len() + 100;
        let built = build(input(CapturePolicy::Full, &sessions, Carry::default(), cap)).unwrap();
        assert!(built.manifest.truncated);
        assert!(built.manifest.content_lines < uncapped.manifest.content_lines);
        assert_eq!(built.manifest.events, 4, "events are always complete");
        assert_eq!(built.events_jsonl, uncapped.events_jsonl);
        assert!(!uncapped.manifest.truncated);
        // A cap below the events alone: all events, no content.
        let tiny = build(input(CapturePolicy::Full, &sessions, Carry::default(), 10)).unwrap();
        assert_eq!((tiny.manifest.events, tiny.manifest.content_lines), (4, 0));
        assert!(tiny.manifest.truncated);
    }

    #[test]
    fn carry_over_keeps_a_gone_sessions_events_and_content_with_their_ids() {
        let first = [live("s1", &["a1", "a2"])];
        let old_built = build(input(
            CapturePolicy::Redacted,
            &first,
            Carry::default(),
            MAX_BYTES,
        ))
        .unwrap();
        let old = Old {
            manifest: old_built.manifest.clone(),
            events_jsonl: old_built.events_jsonl.clone(),
            content_jsonl: old_built.content_jsonl.clone(),
        };
        // s1's transcript is now gone; s2 is new.
        let carry = carry_over(&old, &["s1".to_string()]);
        assert_eq!(carry.sessions.len(), 1);
        assert_eq!(carry.events.len(), 2);
        assert_eq!(carry.content.len(), 2);
        assert_eq!(carry.sources.len(), 1);
        let second = [live("s2", &["b1"])];
        let built = build(input(CapturePolicy::Redacted, &second, carry, MAX_BYTES)).unwrap();
        let ids: Vec<String> = built
            .events_jsonl
            .lines()
            .map(|l| serde_json::from_str::<Event>(l).unwrap().id)
            .collect();
        assert_eq!(
            ids,
            ["a1", "a2", "b1"],
            "the same ids as before, then the new session's"
        );
        assert_eq!(
            built
                .manifest
                .sessions
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            ["s1", "s2"]
        );
        assert_eq!(built.manifest.content_lines, 3);
        assert_eq!(built.manifest.events, 3);
        // The carried lines are the old text, byte for byte.
        assert!(built
            .events_jsonl
            .starts_with(old_built.events_jsonl.as_str()));
        // A session that is not gone is not carried (it is parsed live).
        assert!(carry_over(&old, &[]).events.is_empty());
        // Under metadata the old content is dropped.
        let meta = build(input(
            CapturePolicy::Metadata,
            &second,
            carry_over(&old, &["s1".to_string()]),
            MAX_BYTES,
        ))
        .unwrap();
        assert!(meta.content_jsonl.is_none());
        assert_eq!(meta.manifest.events, 3);
    }
}
