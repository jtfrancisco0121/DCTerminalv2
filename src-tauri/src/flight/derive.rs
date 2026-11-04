//! Derived facts over events: failures, the events before a failure, files
//! touched and repeats. With `correlate.rs`, the only place that sets a
//! `derived` label (AD-7). Every fact says how it was derived and which events
//! it rests on.

use super::schema::{attr, kind, Derived, Event, Failure, Phase, PROVIDER_DCV2};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// How many earlier events the context band shows.
pub const BEFORE: usize = 5;

pub const IS_ERROR: &str = "exact:is_error";
pub const HOOK_DENIAL: &str = "exact:hook-denial";
pub const HOOK_DENIAL_GUARD: &str = "exact:hook-denial:dcv2-guard";
pub const STOP_FAILURE: &str = "exact:stop-failure";
pub const CHECK_FAIL: &str = "exact:check-fail";
/// A phase's PR state came from the run's own `pr` lines, or (a run before them) from status.json now.
pub const PR_STATE_LOG: &str = "exact:log-line";
pub const PR_STATE_STATUS_NOW: &str = "heuristic:status-now";
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
            // The guard's own log line (`logs/guard.log`): merged into the transcript's block by tool_use id,
            // and a row of its own when none matches; never a Failure.
            (kind::TOOL_BLOCKED, _) if event.provider.name == PROVIDER_DCV2 => continue,
            (kind::CHECK_END, Some("fail")) => CHECK_FAIL,
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

/// One phase per event (AD-33), walking `events` in their keyed order (time, file, offset, seq):
/// - `stage` is the `to` of the last `stage.change` at or before the event, and before the first one the
///   first logged stage. A run with no stage lines has none.
/// - The event's PR is `attrs.pr` for the run's own events, the agent's PR (`agent_prs`, from the agent
///   tree, exact or heuristic) for transcript events, and none otherwise.
/// - `pr_state` is the `to` of the last `x.dcv2.pr-state` of that PR at or before the event (the pr-state
///   event itself takes its own `to`), labelled `exact:log-line`. Before the PR's first line it is that line's
///   `from`. A PR the run has no `pr` line for (a run from before the lines) gets its status.json state now,
///   labelled `heuristic:status-now`.
pub fn phases(
    events: &[&Event],
    agent_prs: &HashMap<String, String>,
    status_now: &BTreeMap<String, String>,
) -> HashMap<String, Phase> {
    let text = |e: &Event, key: &str| {
        e.attrs
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let first_stage = events
        .iter()
        .find(|e| e.kind == kind::STAGE_CHANGE)
        .and_then(|e| text(e, attr::TO));
    let mut with_lines: HashSet<String> = HashSet::new();
    let mut first_from: HashMap<String, String> = HashMap::new();
    for e in events.iter().filter(|e| e.kind == kind::PR_STATE) {
        if let Some(pr) = text(e, attr::PR) {
            with_lines.insert(pr.clone());
            if let Some(from) = text(e, attr::FROM) {
                first_from.entry(pr).or_insert(from);
            }
        }
    }
    let mut stage = first_stage;
    let mut state: HashMap<String, String> = HashMap::new();
    let mut out = HashMap::with_capacity(events.len());
    for e in events {
        if e.kind == kind::STAGE_CHANGE {
            stage = text(e, attr::TO).or(stage);
        }
        if e.kind == kind::PR_STATE {
            if let (Some(pr), Some(to)) = (text(e, attr::PR), text(e, attr::TO)) {
                state.insert(pr, to);
            }
        }
        let pr = if e.provider.name == PROVIDER_DCV2 {
            text(e, attr::PR)
        } else {
            e.agent
                .pr
                .clone()
                .or_else(|| agent_prs.get(&e.agent.id).cloned())
        };
        let (pr_state, label) = match pr {
            None => (None, None),
            Some(pr) if with_lines.contains(&pr) => {
                let known = state
                    .get(&pr)
                    .cloned()
                    .or_else(|| first_from.get(&pr).filter(|f| f.as_str() != "-").cloned());
                let label = known.is_some().then(|| PR_STATE_LOG.to_string());
                (known, label)
            }
            Some(pr) => match status_now.get(&pr) {
                Some(now) => (Some(now.clone()), Some(PR_STATE_STATUS_NOW.to_string())),
                None => (None, None),
            },
        };
        out.insert(
            e.id.clone(),
            Phase {
                stage: stage.clone(),
                pr_state,
                label,
            },
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flight::schema::{
        AgentRef, CapturePolicy, Content, Provider, SourceRef, ORCHESTRATION, SCHEMA,
    };
    use serde_json::{json, Map, Value};

    fn ev(
        id: &str,
        k: &str,
        dcv2: bool,
        agent: &str,
        pr: Option<&str>,
        attrs: &[(&str, Value)],
    ) -> Event {
        Event {
            schema: SCHEMA,
            id: id.into(),
            trace: "t".into(),
            session: "s".into(),
            agent: AgentRef {
                id: if dcv2 {
                    ORCHESTRATION.into()
                } else {
                    agent.into()
                },
                parent: None,
                kind: "main".into(),
                agent_type: None,
                role: None,
                pr: pr.map(str::to_string),
            },
            provider: Provider {
                name: if dcv2 { PROVIDER_DCV2 } else { "claude-code" }.into(),
                version: None,
                native: "n".into(),
            },
            kind: k.into(),
            at: "2026-11-05T10:00:00.000Z".into(),
            span: None,
            status: None,
            name: None,
            attrs: attrs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect::<Map<_, _>>(),
            usage: None,
            content: Content {
                source: SourceRef {
                    file: "f".into(),
                    offset: 0,
                    len: 0,
                    sha256: "x".into(),
                },
                policy: CapturePolicy::Metadata,
                redactions: 0,
            },
            derived: None,
        }
    }

    fn stage(id: &str, to: &str) -> Event {
        ev(
            id,
            kind::STAGE_CHANGE,
            true,
            "",
            None,
            &[(attr::TO, json!(to))],
        )
    }
    fn pr_line(id: &str, pr: &str, from: &str, to: &str) -> Event {
        ev(
            id,
            kind::PR_STATE,
            true,
            "",
            None,
            &[
                (attr::PR, json!(pr)),
                (attr::FROM, json!(from)),
                (attr::TO, json!(to)),
            ],
        )
    }
    fn check(id: &str, pr: &str) -> Event {
        ev(
            id,
            kind::CHECK_END,
            true,
            "",
            None,
            &[(attr::PR, json!(pr))],
        )
    }
    fn phase_of(
        map: &HashMap<String, Phase>,
        id: &str,
    ) -> (Option<String>, Option<String>, Option<String>) {
        let p = &map[id];
        (p.stage.clone(), p.pr_state.clone(), p.label.clone())
    }
    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    #[test]
    fn a_phase_has_the_last_stage_and_pr_state_at_or_before_the_event() {
        let events = vec![
            ev("early", kind::TOOL_START, false, "main", None, &[]),
            stage("st1", "planning"),
            stage("st2", "building"),
            pr_line("l1", "PR2", "-", "building"),
            check("c0", "PR2"),
            pr_line("l2", "PR2", "building", "reviewing"),
            check("c1", "PR2"),
            pr_line("l3", "PR2", "reviewing", "fixing"),
            check("c2", "PR2"),
            ev("sub", kind::TOOL_START, false, "a1", None, &[]),
            ev("other", kind::TOOL_START, false, "a9", None, &[]),
        ];
        let refs: Vec<&Event> = events.iter().collect();
        let agent_prs = HashMap::from([("a1".to_string(), "PR2".to_string())]);
        let got = phases(&refs, &agent_prs, &BTreeMap::new());
        assert_eq!(got.len(), events.len(), "one phase per event");
        assert_eq!(
            phase_of(&got, "early"),
            (s("planning"), None, None),
            "before the first stage: the first stage"
        );
        assert_eq!(
            phase_of(&got, "st1").1,
            None,
            "a stage change has no PR state"
        );
        assert_eq!(
            phase_of(&got, "st2").0,
            s("building"),
            "the change event is in its own stage"
        );
        assert_eq!(
            phase_of(&got, "l1"),
            (s("building"), s("building"), s("exact:log-line")),
            "a pr line takes its own `to`"
        );
        assert_eq!(phase_of(&got, "c0").1, s("building"));
        assert_eq!(phase_of(&got, "c1").1, s("reviewing"));
        assert_eq!(phase_of(&got, "c2").1, s("fixing"));
        assert_eq!(
            phase_of(&got, "sub"),
            (s("building"), s("fixing"), s("exact:log-line")),
            "a transcript event takes its agent's PR"
        );
        assert_eq!(
            phase_of(&got, "other").1,
            None,
            "an agent with no PR has no state"
        );
    }

    #[test]
    fn before_a_prs_first_line_the_state_is_that_lines_from_unless_it_is_new() {
        let events = vec![
            check("a", "PR1"),
            pr_line("l", "PR1", "building", "reviewing"),
            check("b", "PR3"),
            pr_line("n", "PR3", "-", "building"),
        ];
        let refs: Vec<&Event> = events.iter().collect();
        let got = phases(&refs, &HashMap::new(), &BTreeMap::new());
        assert_eq!(phase_of(&got, "a").1, s("building"));
        assert_eq!(
            phase_of(&got, "b"),
            (None, None, None),
            "new PR, no state yet"
        );
    }

    #[test]
    fn a_run_with_no_pr_lines_falls_back_to_status_json_and_says_so() {
        let events = vec![
            stage("st", "building"),
            check("c", "PR2"),
            check("d", "PR7"),
        ];
        let refs: Vec<&Event> = events.iter().collect();
        let now = BTreeMap::from([("PR2".to_string(), "fixing".to_string())]);
        let got = phases(&refs, &HashMap::new(), &now);
        assert_eq!(
            phase_of(&got, "c"),
            (s("building"), s("fixing"), s("heuristic:status-now"))
        );
        assert_eq!(
            phase_of(&got, "d").1,
            None,
            "a PR status.json does not know"
        );
    }

    #[test]
    fn a_run_with_no_stage_lines_has_no_stage() {
        let events = vec![check("c", "PR2")];
        let refs: Vec<&Event> = events.iter().collect();
        let got = phases(&refs, &HashMap::new(), &BTreeMap::new());
        assert_eq!(phase_of(&got, "c").0, None);
    }

    #[test]
    fn a_failed_check_is_a_failure_and_a_guard_log_line_never_is() {
        let mut fail = check("c", "PR2");
        fail.status = Some("fail".into());
        let mut pass = check("p", "PR2");
        pass.status = Some("pass".into());
        let mut guard = ev("g", kind::TOOL_BLOCKED, true, "", None, &[]);
        guard.status = Some("blocked".into());
        let mut transcript = ev("t", kind::TOOL_BLOCKED, false, "main", None, &[]);
        transcript.status = Some("blocked".into());
        let all = [fail, pass, guard, transcript];
        let f = failures(&all);
        let got: Vec<(&str, &str)> = f
            .iter()
            .map(|f| (f.id.as_str(), f.derived.label.as_str()))
            .collect();
        assert_eq!(got, [("c", CHECK_FAIL), ("t", HOOK_DENIAL)]);
    }
}
