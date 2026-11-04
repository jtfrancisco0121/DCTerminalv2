//! Correlation: pairs tool starts with their ends into spans and builds the
//! agent tree. With `derive.rs`, the only place that sets a `derived` label.

use super::adapters::AgentMeta;
use super::derive::files_touched;
use super::schema::{kind, AgentNode, Derived, Event, Span};
use regex::Regex;
use std::collections::{BTreeMap, HashMap};

pub const TOOL_USE_ID: &str = "exact:tool_use_id";
pub const ROOT: &str = "exact:session-root";
pub const SUBAGENT_FILE: &str = "heuristic:subagent-without-spawn";
pub const DESCRIPTION_PREFIX: &str = "heuristic:description-prefix";
/// The same prefix in a run from plugin 0.7.0 on, where SKILL.md requires it.
pub const EXACT_PR_DESCRIPTION: &str = "exact:pr-description";

// The panel reads failures and the tree; spans are kept for the run-folder
// adapter and compare (M2, M3) and are covered by the tests below.
#[cfg_attr(not(test), allow(dead_code))]
fn ms(at: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|t| t.timestamp_millis())
}

/// One span per tool call: its `tool.start`, paired by span id with its
/// `tool.end` or `tool.blocked`. A start with no end is open (status `None`).
#[cfg_attr(not(test), allow(dead_code))]
pub fn spans(events: &[Event]) -> Vec<Span> {
    let mut ends: HashMap<(&str, &str), &Event> = HashMap::new();
    for e in events
        .iter()
        .filter(|e| e.kind == kind::TOOL_END || e.kind == kind::TOOL_BLOCKED)
    {
        if let Some(span) = e.span.as_deref() {
            ends.entry((e.agent.id.as_str(), span)).or_insert(e);
        }
    }
    events
        .iter()
        .filter(|e| e.kind == kind::TOOL_START)
        .filter_map(|start| {
            let span = start.span.as_deref()?;
            let end = ends.get(&(start.agent.id.as_str(), span)).copied();
            let mut evidence = vec![start.id.clone()];
            evidence.extend(end.map(|e| e.id.clone()));
            Some(Span {
                span: span.to_string(),
                agent: start.agent.id.clone(),
                name: start.name.clone(),
                start: start.id.clone(),
                end: end.map(|e| e.id.clone()),
                start_at: start.at.clone(),
                end_at: end.map(|e| e.at.clone()),
                status: end.and_then(|e| e.status.clone()),
                duration_ms: end.and_then(|e| Some(ms(&e.at)? - ms(&start.at)?)),
                derived: Derived {
                    label: TOOL_USE_ID.into(),
                    evidence,
                },
            })
        })
        .collect()
}

/// The root (`main`) with every sub-agent below its parent. A sub-agent is
/// linked by its spawn: the `Agent` tool_use id that `meta.json` names, or the
/// `agentId` its spawn result returned. `role` is the type without `dcv2:`;
/// `pr` is set only from a description starting `PR<n> `: exact when the run
/// is from a plugin that requires that form (`exact`), a heuristic otherwise.
pub fn agent_tree(events: &[Event], metas: &BTreeMap<String, AgentMeta>, exact: bool) -> AgentNode {
    let pr_prefix = Regex::new(r"^(PR[0-9]+) ").unwrap();
    let mut first: BTreeMap<&str, &Event> = BTreeMap::new();
    for e in events {
        first.entry(e.agent.id.as_str()).or_insert(e);
    }
    let spawn_start: HashMap<&str, &Event> = events
        .iter()
        .filter(|e| e.kind == kind::AGENT_SPAWN)
        .filter_map(|e| Some((e.span.as_deref()?, e)))
        .collect();
    // sub-agent id → the spawn result (tool.end) that named it.
    let result_of: HashMap<&str, &Event> = events
        .iter()
        .filter(|e| e.kind == kind::TOOL_END)
        .filter_map(|e| Some((e.attrs.get("agentId")?.as_str()?, e)))
        .collect();

    let session = events
        .first()
        .map(|e| e.session.clone())
        .unwrap_or_default();
    let mut ids: Vec<&str> = first.keys().copied().filter(|id| *id != "main").collect();
    for id in metas.keys() {
        if !ids.contains(&id.as_str()) {
            ids.push(id);
        }
    }
    ids.sort_by_key(|id| (first.get(id).map(|e| e.at.clone()), *id));
    let mut nodes: Vec<(String, AgentNode)> = ids
        .iter()
        .map(|id| {
            let meta = metas.get(*id);
            let sample = first.get(id).copied();
            let agent_type = meta
                .and_then(|m| m.agent_type.clone())
                .or_else(|| sample.and_then(|e| e.agent.agent_type.clone()));
            let description = meta.and_then(|m| m.description.clone());
            let result = result_of.get(id).copied();
            let spawn_span = meta
                .and_then(|m| m.tool_use_id.clone())
                .or_else(|| result.and_then(|e| e.span.clone()));
            let mut evidence: Vec<String> = Vec::new();
            if let Some(span) = spawn_span.as_deref() {
                evidence.extend(spawn_start.get(span).map(|e| e.id.clone()));
            }
            evidence.extend(result.map(|e| e.id.clone()));
            let linked = spawn_span.is_some() && !evidence.is_empty();
            if !linked {
                evidence = sample.map(|e| e.id.clone()).into_iter().collect();
            }
            let parent = sample
                .and_then(|e| e.agent.parent.clone())
                .or_else(|| result.map(|e| e.agent.id.clone()))
                .unwrap_or_else(|| "main".into());
            let (pr, pr_derived) = match description.as_deref().and_then(|d| pr_prefix.captures(d))
            {
                Some(cap) => (
                    Some(cap[1].to_string()),
                    Some(Derived {
                        label: if exact {
                            EXACT_PR_DESCRIPTION
                        } else {
                            DESCRIPTION_PREFIX
                        }
                        .into(),
                        evidence: sample.map(|e| e.id.clone()).into_iter().collect(),
                    }),
                ),
                None => (None, None),
            };
            (
                parent,
                AgentNode {
                    id: id.to_string(),
                    session: session.clone(),
                    kind: "subagent".into(),
                    role: agent_type
                        .as_deref()
                        .map(|t| t.strip_prefix("dcv2:").unwrap_or(t).to_string()),
                    agent_type,
                    pr,
                    pr_derived,
                    description,
                    spawn_span,
                    files: files_touched(events, id),
                    children: Vec::new(),
                    derived: Derived {
                        label: if linked { TOOL_USE_ID } else { SUBAGENT_FILE }.into(),
                        evidence,
                    },
                },
            )
        })
        .collect();

    // Attach deepest first is not needed: build by parent id, cycles fall back
    // to the root.
    let known: Vec<String> = nodes.iter().map(|(_, n)| n.id.clone()).collect();
    for (parent, node) in nodes.iter_mut() {
        if *parent == node.id || (*parent != "main" && !known.contains(parent)) {
            *parent = "main".into();
        }
    }
    fn attach(
        parent: &str,
        nodes: &mut Vec<(String, AgentNode)>,
        seen: &mut Vec<String>,
    ) -> Vec<AgentNode> {
        let mut mine = Vec::new();
        let mut at = 0;
        while at < nodes.len() {
            if nodes[at].0 == parent && !seen.contains(&nodes[at].1.id) {
                let (_, mut node) = nodes.remove(at);
                seen.push(node.id.clone());
                node.children = attach(&node.id.clone(), nodes, seen);
                mine.push(node);
            } else {
                at += 1;
            }
        }
        mine
    }
    let mut seen = Vec::new();
    let mut children = attach("main", &mut nodes, &mut seen);
    // A cycle or an orphan still shows up, under the root.
    while let Some((_, node)) = nodes.pop() {
        children.push(node);
    }
    AgentNode {
        id: "main".into(),
        session,
        kind: "main".into(),
        agent_type: None,
        role: None,
        pr: None,
        pr_derived: None,
        description: None,
        spawn_span: None,
        files: files_touched(events, "main"),
        children,
        derived: Derived {
            label: ROOT.into(),
            evidence: events.first().map(|e| e.id.clone()).into_iter().collect(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flight::derive;
    use crate::flight::testkit::*;
    use crate::test_support::TempDir;
    use serde_json::json;

    const SELFTEST: &str = "de6a6329-de0f-49da-960b-f50fed9236e4";
    const FAILING: &str = "5b0d6e0e-3a52-4c61-9d1f-7a2c4e8b9f10";

    #[test]
    fn the_recorded_session_is_main_with_one_plan_reviewer_and_no_failures() {
        let (events, metas) = recorded(SELFTEST);
        let tree = agent_tree(&events, &metas, false);
        assert_eq!(tree.id, "main");
        assert_eq!(tree.children.len(), 1);
        let child = &tree.children[0];
        assert_eq!(child.role.as_deref(), Some("plan-reviewer"));
        assert_eq!(child.agent_type.as_deref(), Some("dcv2:plan-reviewer"));
        assert_eq!(child.derived.label, TOOL_USE_ID);
        assert_eq!(
            child.spawn_span.as_deref(),
            Some("toolu_014z93kPSuie6cXVZCWDsh32")
        );
        assert!(child.pr.is_none() && child.pr_derived.is_none());
        assert!(derive::failures_with_repeats(&events).is_empty());
        let spans = spans(&events);
        let agent = spans
            .iter()
            .find(|s| s.name.as_deref() == Some("Agent"))
            .unwrap();
        assert_eq!(agent.status.as_deref(), Some("ok"));
        assert!(agent.duration_ms.unwrap() > 0);
    }

    #[test]
    fn the_failing_session_lists_the_guard_block_then_the_exit_code_failure() {
        let (events, metas) = recorded(FAILING);
        let failures = derive::failures_with_repeats(&events);
        let labels: Vec<&str> = failures.iter().map(|f| f.derived.label.as_str()).collect();
        assert_eq!(labels, [derive::HOOK_DENIAL_GUARD, derive::IS_ERROR]);
        assert!(failures[0].at <= failures[1].at);
        assert_eq!(failures[0].kind, kind::TOOL_BLOCKED);
        assert_eq!(failures[1].kind, kind::TOOL_END);
        let kind_of = |id: &String| events.iter().find(|e| &e.id == id).unwrap().kind.as_str();
        let before: Vec<&str> = failures[0].before.iter().map(kind_of).collect();
        assert_eq!(
            before,
            ["session.start", "turn.start", "llm.call", "tool.start"]
        );
        let before: Vec<&str> = failures[1].before.iter().map(kind_of).collect();
        assert_eq!(
            before,
            [
                "llm.call",
                "tool.start",
                "tool.blocked",
                "llm.call",
                "tool.start"
            ],
            "the five events before it"
        );
        for f in &failures {
            assert!(f.derived.evidence.contains(&f.id));
            assert!(f.repeat_of.is_none(), "different commands are not repeats");
        }
        let tree = agent_tree(&events, &metas, false);
        assert_eq!(tree.files, ["/work/toy/src/add.js"]);
        let statuses: Vec<_> = spans(&events).iter().map(|s| s.status.clone()).collect();
        assert_eq!(
            statuses,
            [
                Some("blocked".into()),
                Some("error".into()),
                Some("ok".into())
            ]
        );
    }

    #[test]
    fn a_start_with_no_end_is_an_open_span() {
        let dir = TempDir::new("flight_open_span");
        let events = synthetic(
            &dir,
            "s.jsonl",
            &[
                prompt("2026-10-22T10:00:00Z"),
                tool_use(
                    "2026-10-22T10:00:01Z",
                    "m1",
                    "t1",
                    "Bash",
                    json!({"command": "sleep 9"}),
                ),
            ],
        );
        let s = spans(&events);
        assert_eq!(s.len(), 1);
        assert_eq!(
            (s[0].status.clone(), s[0].end.clone(), s[0].duration_ms),
            (None, None, None)
        );
        assert_eq!(s[0].derived.label, "exact:tool_use_id");
    }

    #[test]
    fn the_same_failing_command_twice_is_a_labelled_repeat_that_links_the_first() {
        let dir = TempDir::new("flight_repeat");
        let bash = |at, msg, id| {
            tool_use(
                at,
                msg,
                id,
                "Bash",
                json!({"command": "cd /a/b/proj  && make test"}),
            )
        };
        let events = synthetic(
            &dir,
            "s.jsonl",
            &[
                prompt("2026-10-22T10:00:00Z"),
                bash("2026-10-22T10:00:01Z", "m1", "t1"),
                tool_result("2026-10-22T10:00:02Z", "t1", "Exit code 2\nboom", true),
                bash("2026-10-22T10:00:03Z", "m2", "t2"),
                tool_result("2026-10-22T10:00:04Z", "t2", "Exit code 2\nboom", true),
                tool_use(
                    "2026-10-22T10:00:05Z",
                    "m3",
                    "t3",
                    "Bash",
                    json!({"command": "make other"}),
                ),
                tool_result("2026-10-22T10:00:06Z", "t3", "Exit code 1", true),
            ],
        );
        let flat = derive::failures(&events);
        assert_eq!(flat.len(), 3);
        let top = derive::failures_with_repeats(&events);
        assert_eq!(top.len(), 2, "the repeat is nested, not listed on its own");
        assert_eq!(top[0].repeats.len(), 1);
        let repeat = &top[0].repeats[0];
        let link = repeat.repeat_of.as_ref().unwrap();
        assert_eq!(link.label, "heuristic:same-tool-same-input-after-error");
        assert_eq!(link.evidence, [top[0].id.clone(), repeat.id.clone()]);
        assert!(top[1].repeat_of.is_none());
    }

    #[test]
    fn an_api_error_between_two_turns_is_one_stop_failure_with_its_evidence() {
        let dir = TempDir::new("flight_api_error");
        let events = synthetic(
            &dir,
            "s.jsonl",
            &[
                prompt("2026-10-22T10:00:00Z"),
                tool_use(
                    "2026-10-22T10:00:01Z",
                    "m1",
                    "t1",
                    "Bash",
                    json!({"command": "ls"}),
                ),
                tool_result("2026-10-22T10:00:02Z", "t1", "ok", false),
                json!({"type": "assistant", "isApiErrorMessage": true, "error": "rate_limit",
                    "apiErrorStatus": 429, "timestamp": "2026-10-22T10:00:03Z",
                    "message": {"id": "e1", "model": "<synthetic>", "content": [{"type": "text", "text": "limit"}],
                        "usage": {"input_tokens": 0, "output_tokens": 0}}}),
                prompt("2026-10-22T10:00:04Z"),
            ],
        );
        let failures = derive::failures_with_repeats(&events);
        assert_eq!(failures.len(), 1);
        let error = events.iter().find(|e| e.kind == kind::ERROR).unwrap();
        assert_eq!(failures[0].derived.label, "exact:stop-failure");
        assert_eq!(failures[0].derived.evidence, vec![error.id.clone()]);
        let kinds: Vec<&str> = failures[0]
            .before
            .iter()
            .map(|id| events.iter().find(|e| &e.id == id).unwrap().kind.as_str())
            .collect();
        assert_eq!(
            kinds,
            [
                "session.start",
                "turn.start",
                "llm.call",
                "tool.start",
                "tool.end"
            ]
        );
    }

    #[test]
    fn edit_tools_list_their_files_sorted_and_unique() {
        let dir = TempDir::new("flight_files");
        let events = synthetic(
            &dir,
            "s.jsonl",
            &[
                prompt("2026-10-22T10:00:00Z"),
                tool_use(
                    "2026-10-22T10:00:01Z",
                    "m1",
                    "t1",
                    "Write",
                    json!({"file_path": "/w/b.rs", "content": "x"}),
                ),
                tool_use(
                    "2026-10-22T10:00:02Z",
                    "m2",
                    "t2",
                    "Edit",
                    json!({"file_path": "/w/a.rs"}),
                ),
                tool_use(
                    "2026-10-22T10:00:03Z",
                    "m3",
                    "t3",
                    "MultiEdit",
                    json!({"file_path": "/w/b.rs"}),
                ),
                tool_use(
                    "2026-10-22T10:00:04Z",
                    "m4",
                    "t4",
                    "NotebookEdit",
                    json!({"notebook_path": "/w/n.ipynb"}),
                ),
                tool_use(
                    "2026-10-22T10:00:05Z",
                    "m5",
                    "t5",
                    "Read",
                    json!({"file_path": "/w/never.rs"}),
                ),
            ],
        );
        assert_eq!(
            derive::files_touched(&events, "main"),
            ["/w/a.rs", "/w/b.rs", "/w/n.ipynb"]
        );
        assert!(derive::files_touched(&events, "other").is_empty());
    }

    #[test]
    fn a_pr_id_comes_only_from_a_description_prefix_and_says_it_is_a_heuristic() {
        let (events, mut metas) = recorded(SELFTEST);
        let id = metas.keys().next().unwrap().clone();
        metas.get_mut(&id).unwrap().description = Some("PR2 build the thing".into());
        let node = &agent_tree(&events, &metas, false).children[0];
        assert_eq!(node.pr.as_deref(), Some("PR2"));
        assert_eq!(
            node.pr_derived.as_ref().unwrap().label,
            "heuristic:description-prefix"
        );
        metas.get_mut(&id).unwrap().description = Some("review PR2".into());
        assert!(agent_tree(&events, &metas, false).children[0].pr.is_none());
        assert!(agent_tree(&events, &metas, true).children[0].pr.is_none());
    }

    #[test]
    fn the_exact_flag_changes_only_the_label_of_a_prefixed_description() {
        let (events, mut metas) = recorded(SELFTEST);
        let id = metas.keys().next().unwrap().clone();
        metas.get_mut(&id).unwrap().description = Some("PR2 review round 1".into());
        let exact = &agent_tree(&events, &metas, true).children[0];
        assert_eq!(exact.pr.as_deref(), Some("PR2"));
        let derived = exact.pr_derived.as_ref().unwrap();
        assert_eq!(derived.label, EXACT_PR_DESCRIPTION);
        assert!(!derived.evidence.is_empty());
        let heuristic = &agent_tree(&events, &metas, false).children[0];
        assert_eq!(heuristic.pr.as_deref(), Some("PR2"));
        assert_eq!(
            heuristic.pr_derived.as_ref().unwrap().label,
            DESCRIPTION_PREFIX
        );
    }

    #[test]
    fn every_derived_fact_has_a_label_and_evidence() {
        for session in [SELFTEST, FAILING] {
            let (events, metas) = recorded(session);
            let mut all: Vec<Derived> = spans(&events).into_iter().map(|s| s.derived).collect();
            for f in derive::failures_with_repeats(&events) {
                all.push(f.derived);
            }
            fn walk(node: &AgentNode, all: &mut Vec<Derived>) {
                all.push(node.derived.clone());
                all.extend(node.pr_derived.clone());
                node.children.iter().for_each(|c| walk(c, all));
            }
            walk(&agent_tree(&events, &metas, false), &mut all);
            assert!(all.len() >= 3);
            for d in all {
                assert!(!d.label.is_empty() && !d.evidence.is_empty(), "{d:?}");
            }
        }
    }
}
