//! One inspector view: the parsed sessions of a run (or one session) and
//! the page of events the panel asks for. Pure: the host hands in the files
//! and the policy.

use super::adapters::{claude, AgentMeta, SubagentFile};
use super::correlate;
use super::derive;
use super::schema::{AgentNode, CapturePolicy, Event, Failure};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const DEFAULT_PAGE: usize = 200;
pub const MAX_PAGE: usize = 1000;

/// One session parsed once: events, tree and failures.
#[derive(Debug)]
pub struct Parsed {
    pub events: Vec<Event>,
    pub tree: AgentNode,
    pub failures: Vec<Failure>,
}

pub fn parse(main: &Path, subagents: &[SubagentFile]) -> Parsed {
    let events = claude::events_for(main, subagents);
    let metas: BTreeMap<String, AgentMeta> = subagents
        .iter()
        .filter_map(|s| Some((s.agent_id.clone(), s.meta.clone()?)))
        .collect();
    let tree = correlate::agent_tree(&events, &metas);
    let failures = derive::failures_with_repeats(&events);
    Parsed {
        events,
        tree,
        failures,
    }
}

/// The position after which the next page starts: the last returned event's
/// (time, file, offset) and its index among events at the same position, so
/// lines appended later never shift it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Cursor {
    at: String,
    file: String,
    offset: u64,
    seq: usize,
}

fn encode(cursor: &Cursor) -> String {
    let json = serde_json::to_vec(cursor).unwrap_or_default();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
}

fn decode(text: &str) -> Result<Cursor, String> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text.trim())
        .map_err(|_| "bad cursor".to_string())?;
    serde_json::from_slice(&bytes).map_err(|_| "bad cursor".to_string())
}

#[derive(Debug, Clone, Default)]
pub struct PageRequest {
    pub agent: Option<String>,
    pub cursor: Option<String>,
    pub page_size: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub events: Vec<Event>,
    pub next_cursor: Option<String>,
}

/// `ok`, or `off` when the policy lets nothing be read.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub status: String,
    pub trace: String,
    pub policy: CapturePolicy,
    pub agents: Vec<AgentNode>,
    pub failures: Vec<Failure>,
    pub page: Page,
    pub notices: Vec<String>,
}

impl SessionView {
    /// What `off` returns: no trace, no events.
    pub fn off(trace: &str) -> Self {
        Self {
            status: "off".into(),
            trace: trace.into(),
            policy: CapturePolicy::Off,
            agents: Vec::new(),
            failures: Vec::new(),
            page: Page {
                events: Vec::new(),
                next_cursor: None,
            },
            notices: Vec::new(),
        }
    }
}

/// Events in (time, file, offset) order, each with its cursor key.
fn keyed<'a>(
    sessions: &'a [std::sync::Arc<Parsed>],
    agent: Option<&str>,
) -> Vec<(Cursor, &'a Event)> {
    let mut all: Vec<&Event> = sessions
        .iter()
        .flat_map(|s| s.events.iter())
        .filter(|e| agent.is_none_or(|a| e.agent.id == a))
        .collect();
    if sessions.len() > 1 {
        all.sort_by(|a, b| {
            (&a.at, &a.content.source.file, a.content.source.offset).cmp(&(
                &b.at,
                &b.content.source.file,
                b.content.source.offset,
            ))
        });
    }
    let mut out: Vec<(Cursor, &Event)> = Vec::with_capacity(all.len());
    for event in all {
        let r = &event.content.source;
        let seq = match out.last() {
            Some((k, _)) if k.at == event.at && k.file == r.file && k.offset == r.offset => {
                k.seq + 1
            }
            _ => 0,
        };
        out.push((
            Cursor {
                at: event.at.clone(),
                file: r.file.clone(),
                offset: r.offset,
                seq,
            },
            event,
        ));
    }
    out
}

/// The view of `sessions`: every tree and failure, and one page of events in
/// (time, file, offset) order. `trace` is the run id when there is one.
pub fn view(
    sessions: &[std::sync::Arc<Parsed>],
    trace: Option<&str>,
    policy: CapturePolicy,
    request: &PageRequest,
    notices: Vec<String>,
) -> Result<SessionView, String> {
    let size = request.page_size.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);
    let after = request.cursor.as_deref().map(decode).transpose()?;
    let keyed = keyed(sessions, request.agent.as_deref());
    // Keys ascend, so the page starts at the first key after the cursor.
    let start = match &after {
        Some(after) => keyed.partition_point(|(key, _)| key <= after),
        None => 0,
    };
    let end = (start + size).min(keyed.len());
    let events: Vec<Event> = keyed[start..end]
        .iter()
        .map(|(_, event)| {
            let mut shown = (*event).clone();
            shown.content.policy = policy;
            if let Some(trace) = trace {
                shown.trace = trace.to_string();
            }
            shown
        })
        .collect();
    let next_cursor = (end < keyed.len() && end > start).then(|| encode(&keyed[end - 1].0));
    let mut failures: Vec<Failure> = sessions
        .iter()
        .flat_map(|s| s.failures.iter().cloned())
        .collect();
    failures.sort_by(|a, b| a.at.cmp(&b.at));
    Ok(SessionView {
        status: "ok".into(),
        trace: trace
            .map(str::to_string)
            .or_else(|| {
                sessions
                    .first()
                    .and_then(|s| s.events.first())
                    .map(|e| e.trace.clone())
            })
            .unwrap_or_default(),
        policy,
        agents: sessions.iter().map(|s| s.tree.clone()).collect(),
        failures,
        page: Page {
            events,
            next_cursor,
        },
        notices,
    })
}
