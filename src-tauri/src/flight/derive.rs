//! Derived facts over events: failures, the events before a failure, files
//! touched and repeats. With `correlate.rs`, the only place that sets a
//! `derived` label (AD-7). Every fact says how it was derived and which events
//! it rests on.

use super::schema::{kind, Derived, Event, Failure};
use std::collections::{BTreeSet, HashMap};

/// How many earlier events the context band shows.
pub const BEFORE: usize = 5;

pub const IS_ERROR: &str = "exact:is_error";
pub const HOOK_DENIAL: &str = "exact:hook-denial";
pub const HOOK_DENIAL_GUARD: &str = "exact:hook-denial:dcv2-guard";
pub const STOP_FAILURE: &str = "exact:stop-failure";
pub const SAME_TOOL_SAME_INPUT: &str = "heuristic:same-tool-same-input-after-error";

/// The ids of the `n` events of `agent` before `events[at]`, oldest first.
/// `x.claude.unknown` lines (attachments, latches) are noise and are skipped.
pub fn before(events: &[Event], at: usize, n: usize) -> Vec<String> {
    let agent = &events[at].agent.id;
    let mut ids: Vec<String> = events[..at]
        .iter()
        .rev()
        .filter(|e| &e.agent.id == agent && e.kind != kind::UNKNOWN)
        .take(n)
        .map(|e| e.id.clone())
        .collect();
    ids.reverse();
    ids
}

/// Sorted, unique paths the edit tools of `agent` touched.
pub fn files_touched(events: &[Event], agent: &str) -> Vec<String> {
    let mut files = BTreeSet::new();
    for event in events
        .iter()
        .filter(|e| e.agent.id == agent && e.kind == kind::FILE_CHANGE)
    {
        for path in event
            .attrs
            .get("files")
            .and_then(|f| f.as_array())
            .into_iter()
            .flatten()
        {
            if let Some(path) = path.as_str() {
                files.insert(path.to_string());
            }
        }
    }
    files.into_iter().collect()
}

/// Every failed, blocked or API-error event, in time order, flat.
pub fn failures(events: &[Event]) -> Vec<Failure> {
    let starts: HashMap<(&str, &str), &Event> = events
        .iter()
        .filter(|e| e.kind == kind::TOOL_START)
        .filter_map(|e| Some(((e.agent.id.as_str(), e.span.as_deref()?), e)))
        .collect();
    let mut out = Vec::new();
    for (at, event) in events.iter().enumerate() {
        let label = match (event.kind.as_str(), event.status.as_deref()) {
            (kind::TOOL_END, Some("error")) => IS_ERROR,
            (kind::TOOL_BLOCKED, _) => {
                if event.attrs.get("dcv2Guard").and_then(|v| v.as_bool()) == Some(true) {
                    HOOK_DENIAL_GUARD
                } else {
                    HOOK_DENIAL
                }
            }
            (kind::ERROR, _) => STOP_FAILURE,
            _ => continue,
        };
        let mut evidence = Vec::new();
        if let Some(span) = event.span.as_deref() {
            if let Some(start) = starts.get(&(event.agent.id.as_str(), span)) {
                evidence.push(start.id.clone());
            }
        }
        evidence.push(event.id.clone());
        out.push(Failure {
            id: event.id.clone(),
            agent: event.agent.id.clone(),
            at: event.at.clone(),
            name: event.name.clone(),
            kind: event.kind.clone(),
            derived: Derived {
                label: label.into(),
                evidence,
            },
            before: before(events, at, BEFORE),
            repeat_of: None,
            repeats: Vec::new(),
            rule: None,
        });
    }
    out
}

/// `(later failure id, earlier failure id)` for each failure that repeats an
/// earlier one of the same agent with the same tool and the same normalized
/// input (the adapter's `inputHash`). Only tool failures can repeat.
pub fn repeats(events: &[Event], failures: &[Failure]) -> Vec<(String, Derived)> {
    let by_id: HashMap<&str, &Event> = events.iter().map(|e| (e.id.as_str(), e)).collect();
    // The tool.start of a failure carries its input hash.
    let key = |failure: &Failure| -> Option<(String, String, String)> {
        let start = failure
            .derived
            .evidence
            .first()
            .and_then(|id| by_id.get(id.as_str()))
            .filter(|e| e.kind == kind::TOOL_START)?;
        let hash = start.attrs.get("inputHash")?.as_str()?;
        Some((failure.agent.clone(), start.name.clone()?, hash.to_string()))
    };
    let mut last: HashMap<(String, String, String), &Failure> = HashMap::new();
    let mut out = Vec::new();
    for failure in failures {
        let Some(k) = key(failure) else { continue };
        if let Some(earlier) = last.get(&k) {
            out.push((
                failure.id.clone(),
                Derived {
                    label: SAME_TOOL_SAME_INPUT.into(),
                    evidence: vec![earlier.id.clone(), failure.id.clone()],
                },
            ));
        }
        last.insert(k, failure);
    }
    out
}

/// The failures the panel lists: repeats are nested under the first failure
/// of their chain, and each repeat carries its heuristic label and evidence.
pub fn failures_with_repeats(events: &[Event]) -> Vec<Failure> {
    let flat = failures(events);
    let links: HashMap<String, Derived> = repeats(events, &flat).into_iter().collect();
    let mut roots: Vec<Failure> = Vec::new();
    let mut root_of: HashMap<String, usize> = HashMap::new();
    for mut failure in flat {
        match links.get(&failure.id) {
            Some(link) => {
                let earlier = &link.evidence[0];
                let root = root_of.get(earlier).copied().unwrap_or(0);
                root_of.insert(failure.id.clone(), root);
                failure.repeat_of = Some(link.clone());
                roots[root].repeats.push(failure);
            }
            None => {
                root_of.insert(failure.id.clone(), roots.len());
                roots.push(failure);
            }
        }
    }
    roots
}
