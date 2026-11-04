//! One inspector view: the parsed sessions of a run (or one session) and
//! the page of events the panel asks for. Pure: the host hands in the files
//! and the policy.

use super::adapters::dcv2_run::RunEvents;
use super::adapters::{claude, AgentMeta, SubagentFile};
use super::correlate;
use super::derive;
use super::schema::{attr, kind, AgentNode, CapturePolicy, Event, Failure, Phase, PROVIDER_DCV2};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
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

/// `exact` says the run's agents were launched with the exact `PR<n> ` description rule
/// (plugin 0.7.0 on), so a PR tied by that prefix is labelled exact.
impl Parsed {
    /// A session rebuilt from events alone (the index keeps them when a transcript is gone): no
    /// metas, so the tree is the bare root, but the failures and PRs the events carry stay.
    pub fn from_events(events: Vec<Event>) -> Self {
        let failures = derive::failures_with_repeats(&events);
        let tree = correlate::agent_tree(&events, &BTreeMap::new(), false);
        Self {
            events,
            tree,
            failures,
        }
    }
}

pub fn parse(main: &Path, subagents: &[SubagentFile], exact: bool) -> Parsed {
    let mut events = claude::events_for(main, subagents);
    let metas: BTreeMap<String, AgentMeta> = subagents
        .iter()
        .filter_map(|s| Some((s.agent_id.clone(), s.meta.clone()?)))
        .collect();
    let tree = correlate::agent_tree(&events, &metas, exact);
    // Each event carries its agent's PR (the index and the phases read it from there).
    let prs = agent_prs(&tree);
    for event in events.iter_mut() {
        if let Some(pr) = prs.get(&event.agent.id) {
            event.agent.pr = Some(pr.clone());
        }
    }
    let failures = derive::failures_with_repeats(&events);
    Parsed {
        events,
        tree,
        failures,
    }
}

/// agent id -> PR id, for every agent of the tree that has one.
pub fn agent_prs(tree: &AgentNode) -> std::collections::HashMap<String, String> {
    fn walk(node: &AgentNode, out: &mut std::collections::HashMap<String, String>) {
        if let Some(pr) = &node.pr {
            out.insert(node.id.clone(), pr.clone());
        }
        node.children.iter().for_each(|c| walk(c, out));
    }
    let mut out = std::collections::HashMap::new();
    walk(tree, &mut out);
    out
}

/// The position after which the next page starts: the last returned event's
/// (time, file, offset) and its index among events at the same position, so
/// lines appended later never shift it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Cursor {
    pub at: String,
    pub file: String,
    pub offset: u64,
    pub seq: usize,
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
    /// An event id: with no cursor, the first page is the one holding this event.
    pub focus: Option<String>,
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
    /// The phase (stage and PR state) of each event of the page, by event id.
    pub phases: BTreeMap<String, Phase>,
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
            phases: BTreeMap::new(),
        }
    }
}

/// Sessions and a run's own events as one list: in (time, file, offset) order, each with its
/// cursor key, the guard's log merged into the transcript, the failures and one phase per event.
/// The view pages it and the index stores it, so both agree.
pub struct Merged<'a> {
    pub ordered: Vec<(Cursor, Cow<'a, Event>)>,
    pub failures: Vec<Failure>,
    pub phases: HashMap<String, Phase>,
}

fn is_guard_log(e: &Event) -> bool {
    e.kind == kind::TOOL_BLOCKED && e.provider.name == PROVIDER_DCV2
}

fn rule_of(e: &Event) -> Option<&str> {
    e.attrs.get(attr::RULE).and_then(|v| v.as_str())
}

fn apply_rule(failure: &mut Failure, rules: &HashMap<String, String>) {
    if let Some(rule) = rules.get(&failure.id) {
        failure.rule = Some(rule.clone());
    }
    failure
        .repeats
        .iter_mut()
        .for_each(|f| apply_rule(f, rules));
}

pub fn merge<'a>(sessions: &'a [std::sync::Arc<Parsed>], run: Option<&'a RunEvents>) -> Merged<'a> {
    let run_events: &[Event] = run.map_or(&[], |r| r.events.as_slice());
    // The guard's log names the rule of a block by its tool_use id. A transcript block with the same
    // id carries it (one row, one Failure); a line with no match stays a row of its own.
    let by_span: HashMap<&str, &str> = run_events
        .iter()
        .filter(|e| is_guard_log(e))
        .filter_map(|e| Some((e.span.as_deref()?, rule_of(e)?)))
        .collect();
    let mut blocked: HashSet<&str> = HashSet::new();
    let mut rules: HashMap<String, String> = HashMap::new();
    let mut all: Vec<Cow<'a, Event>> = Vec::new();
    for e in sessions.iter().flat_map(|s| s.events.iter()) {
        let rule = (e.kind == kind::TOOL_BLOCKED)
            .then(|| {
                e.span
                    .as_deref()
                    .and_then(|s| by_span.get(s).map(|r| (s, *r)))
            })
            .flatten();
        match rule {
            Some((span, rule)) => {
                blocked.insert(span);
                rules.insert(e.id.clone(), rule.to_string());
                let mut shown = e.clone();
                shown
                    .attrs
                    .insert(attr::RULE.into(), serde_json::Value::String(rule.into()));
                all.push(Cow::Owned(shown));
            }
            None => all.push(Cow::Borrowed(e)),
        }
    }
    for e in run_events {
        let merged = is_guard_log(e) && e.span.as_deref().is_some_and(|s| blocked.contains(s));
        if !merged {
            all.push(Cow::Borrowed(e));
        }
    }
    if sessions.len() + usize::from(!run_events.is_empty()) > 1 {
        all.sort_by(|a, b| {
            (&a.at, &a.content.source.file, a.content.source.offset).cmp(&(
                &b.at,
                &b.content.source.file,
                b.content.source.offset,
            ))
        });
    }
    let mut ordered: Vec<(Cursor, Cow<'a, Event>)> = Vec::with_capacity(all.len());
    for event in all {
        let r = &event.content.source;
        let seq = match ordered.last() {
            Some((k, _)) if k.at == event.at && k.file == r.file && k.offset == r.offset => {
                k.seq + 1
            }
            _ => 0,
        };
        let key = Cursor {
            at: event.at.clone(),
            file: r.file.clone(),
            offset: r.offset,
            seq,
        };
        ordered.push((key, event));
    }
    let mut agent_prs: HashMap<String, String> = HashMap::new();
    for s in sessions {
        agent_prs.extend(self::agent_prs(&s.tree));
    }
    let refs: Vec<&Event> = ordered.iter().map(|(_, e)| e.as_ref()).collect();
    let empty = BTreeMap::new();
    let phases = derive::phases(&refs, &agent_prs, run.map_or(&empty, |r| &r.status_now));
    let mut failures: Vec<Failure> = sessions
        .iter()
        .flat_map(|s| s.failures.iter().cloned())
        .chain(derive::failures_with_repeats(run_events))
        .collect();
    failures.iter_mut().for_each(|f| apply_rule(f, &rules));
    failures.sort_by(|a, b| a.at.cmp(&b.at));
    Merged {
        ordered,
        failures,
        phases,
    }
}

/// The view of `sessions` and a run's own events: every tree and failure, and one page of events in
/// (time, file, offset) order. `trace` is the run id when there is one.
pub fn view(
    sessions: &[std::sync::Arc<Parsed>],
    run: Option<&RunEvents>,
    trace: Option<&str>,
    policy: CapturePolicy,
    request: &PageRequest,
    notices: Vec<String>,
) -> Result<SessionView, String> {
    let size = request.page_size.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);
    let after = request.cursor.as_deref().map(decode).transpose()?;
    let merged = merge(sessions, run);
    let agent = request.agent.as_deref();
    let keyed: Vec<&(Cursor, Cow<Event>)> = merged
        .ordered
        .iter()
        .filter(|(_, e)| agent.is_none_or(|a| e.agent.id == a))
        .collect();
    // Keys ascend, so the page starts at the first key after the cursor, or at the page holding the
    // focused event when there is no cursor.
    let start = match (&after, &request.focus) {
        (Some(after), _) => keyed.partition_point(|(key, _)| key <= after),
        (None, Some(focus)) => keyed
            .iter()
            .position(|(_, e)| &e.id == focus)
            .map_or(0, |at| at / size * size),
        (None, None) => 0,
    };
    let end = (start + size).min(keyed.len());
    let mut phases = BTreeMap::new();
    let events: Vec<Event> = keyed[start..end]
        .iter()
        .map(|(_, event)| {
            let mut shown = event.as_ref().clone();
            shown.content.policy = policy;
            if let Some(trace) = trace {
                shown.trace = trace.to_string();
            }
            if let Some(phase) = merged.phases.get(&shown.id) {
                phases.insert(shown.id.clone(), phase.clone());
            }
            shown
        })
        .collect();
    let next_cursor = (end < keyed.len() && end > start).then(|| encode(&keyed[end - 1].0));
    let mut notices = notices;
    notices.extend(run.into_iter().flat_map(|r| r.notices.iter().cloned()));
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
        failures: merged.failures,
        page: Page {
            events,
            next_cursor,
        },
        notices,
        phases,
    })
}
