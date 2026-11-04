//! Schema 1: the events adapters emit and the derived facts built from them.
//!
//! Events are facts that point at their source (file, byte offset, line
//! hash) and carry no content text; spans, the agent tree, failures and
//! repeats are derived and each says how (`derived`). Serde camelCase.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

pub const SCHEMA: u32 = 1;

/// The core event kinds. A provider may add `x.<provider>.<name>` kinds.
pub mod kind {
    pub const SESSION_START: &str = "session.start";
    pub const TURN_START: &str = "turn.start";
    pub const TURN_END: &str = "turn.end";
    pub const LLM_CALL: &str = "llm.call";
    pub const TOOL_START: &str = "tool.start";
    pub const TOOL_END: &str = "tool.end";
    pub const TOOL_BLOCKED: &str = "tool.blocked";
    pub const AGENT_SPAWN: &str = "agent.spawn";
    pub const FILE_CHANGE: &str = "file.change";
    pub const ERROR: &str = "error";
    pub const UNKNOWN: &str = "x.claude.unknown";
    // The run folder's orchestration kinds (the `dcv2` adapter, `adapters/dcv2_run.rs`).
    pub const STAGE_CHANGE: &str = "stage.change";
    pub const APPROVAL_WAIT: &str = "approval.wait";
    pub const APPROVAL_DECISION: &str = "approval.decision";
    pub const CHECK_END: &str = "check.end";
    pub const REVIEW_ROUND: &str = "review.round";
    pub const CI_STATE: &str = "ci.state";
    pub const RETRY: &str = "retry";
    pub const ATTENTION: &str = "attention";
    pub const PR_STATE: &str = "x.dcv2.pr-state";
    pub const LOG: &str = "x.dcv2.log";
    pub const REQUEST: &str = "x.dcv2.request";
}

/// `status` values of the orchestration events (the transcript adapter adds `ok`, `error`,
/// `blocked` and `cancelled`).
pub mod status {
    pub const PASS: &str = "pass";
    pub const FAIL: &str = "fail";
    pub const SKIP: &str = "skip";
    pub const OK: &str = "ok";
    pub const DECLINED: &str = "declined";
    pub const BLOCKED: &str = "blocked";
}

/// Keys of `Event.attrs` the orchestration events use.
pub mod attr {
    pub const SOURCE: &str = "source";
    pub const PR: &str = "pr";
    pub const ROUND: &str = "round";
    pub const VERDICT: &str = "verdict";
    pub const HEAD: &str = "head";
    pub const EXIT_CODE: &str = "exitCode";
    pub const CI: &str = "ci";
    pub const PR_STATE: &str = "prState";
    pub const RULE: &str = "rule";
    pub const TOOL: &str = "tool";
    pub const FROM: &str = "from";
    pub const TO: &str = "to";
    pub const REQUEST_KIND: &str = "requestKind";
    pub const RESULT: &str = "result";
    pub const AGENT: &str = "agent";
}

/// The lane of the run folder's own events, and the provider that writes them.
pub const ORCHESTRATION: &str = "orchestration";
pub const PROVIDER_DCV2: &str = "dcv2";

/// What an account lets AgentFlight read and show. Ordered from the most to
/// the least strict, so the strictest of a set is `max`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum CapturePolicy {
    Full,
    Redacted,
    #[default]
    Metadata,
    Off,
}

impl CapturePolicy {
    /// The stricter of two policies (`off` > `metadata` > `redacted` > `full`).
    pub fn strictest(self, other: Self) -> Self {
        self.max(other)
    }
}

/// Where an event came from: the provider's own line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRef {
    pub file: String,
    pub offset: u64,
    pub len: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Content {
    #[serde(rename = "ref")]
    pub source: SourceRef,
    pub policy: CapturePolicy,
    pub redactions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRef {
    pub id: String,
    pub parent: Option<String>,
    /// `main` or `subagent`.
    pub kind: String,
    #[serde(rename = "type")]
    pub agent_type: Option<String>,
    pub role: Option<String>,
    pub pr: Option<String>,
}

impl AgentRef {
    pub fn main() -> Self {
        Self {
            id: "main".into(),
            parent: None,
            kind: "main".into(),
            agent_type: None,
            role: None,
            pr: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub name: String,
    pub version: Option<String>,
    /// The provider's own name for the line, e.g. `assistant.tool_use`.
    pub native: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_write5m: u64,
    pub cache_write1h: u64,
    pub cache_read: u64,
    pub model: Option<String>,
    pub usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub schema: u32,
    pub id: String,
    pub trace: String,
    pub session: String,
    pub agent: AgentRef,
    pub provider: Provider,
    pub kind: String,
    /// ISO-8601 UTC, as the provider wrote it.
    pub at: String,
    pub span: Option<String>,
    /// `ok`, `error`, `blocked` or `cancelled` on `*.end`.
    pub status: Option<String>,
    pub name: Option<String>,
    /// Small and safe: paths, exit codes, error kinds, statuses, counts.
    pub attrs: Map<String, Value>,
    pub usage: Option<Usage>,
    pub content: Content,
    pub derived: Option<String>,
}

/// How a derived fact came about, and the events it rests on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Derived {
    /// `exact:<rule>` or `heuristic:<rule>`; never empty.
    pub label: String,
    pub evidence: Vec<String>,
}

/// A tool call: its start and, when it came, its end.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub span: String,
    pub agent: String,
    pub name: Option<String>,
    pub start: String,
    pub end: Option<String>,
    pub start_at: String,
    pub end_at: Option<String>,
    /// `null` while the span is open.
    pub status: Option<String>,
    pub duration_ms: Option<i64>,
    pub derived: Derived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentNode {
    pub id: String,
    /// The session this agent belongs to (a run can hold several).
    pub session: String,
    pub kind: String,
    #[serde(rename = "type")]
    pub agent_type: Option<String>,
    pub role: Option<String>,
    pub pr: Option<String>,
    /// How `pr` was found (set only with `pr`).
    pub pr_derived: Option<Derived>,
    pub description: Option<String>,
    /// The spawn tool_use id that links this agent to its parent.
    pub spawn_span: Option<String>,
    /// Sorted, unique paths the agent's edit tools touched.
    pub files: Vec<String>,
    pub children: Vec<AgentNode>,
    pub derived: Derived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    /// The id of the failing event.
    pub id: String,
    pub agent: String,
    pub at: String,
    pub name: Option<String>,
    /// `tool.end`, `tool.blocked` or `error`.
    pub kind: String,
    pub derived: Derived,
    /// The previous events of the same agent, oldest first.
    pub before: Vec<String>,
    /// Set when a heuristic says this repeats an earlier failure.
    pub repeat_of: Option<Derived>,
    /// Later failures that repeat this one (set by the host).
    pub repeats: Vec<Failure>,
    /// The guard rule id, when the dcv2 guard's own log (`logs/guard.log`) names this block.
    pub rule: Option<String>,
}

/// Where in the run an event happened: the stage and, for an event of a PR, the PR's state. One per
/// event, from `derive::phases`; `label` says how `prState` was found (`exact:log-line` from the
/// run's own `pr` lines, `heuristic:status-now` from status.json for runs before them).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Phase {
    pub stage: Option<String>,
    pub pr_state: Option<String>,
    pub label: Option<String>,
}

/// The first 16 hex digits of a sha256 are enough for an id.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// `ev_` + 16 hex of sha256(file ":" offset ":" line sha256): the position,
/// plus the line hash so a rewritten file does not reuse an id.
pub fn event_id(file: &str, offset: u64, line_sha256: &str) -> String {
    let digest = sha256_hex(format!("{file}:{offset}:{line_sha256}").as_bytes());
    format!("ev_{}", &digest[..16])
}

/// The id of the `seq`-th event of one source line: the first one is
/// [`event_id`], later ones (a line can hold several tool_use blocks) add
/// their index so ids stay distinct.
pub fn event_id_seq(file: &str, offset: u64, line_sha256: &str, seq: usize) -> String {
    if seq == 0 {
        return event_id(file, offset, line_sha256);
    }
    let digest = sha256_hex(format!("{file}:{offset}:{line_sha256}:{seq}").as_bytes());
    format!("ev_{}", &digest[..16])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn source(file: &str, offset: u64, line: &[u8]) -> SourceRef {
        SourceRef {
            file: file.into(),
            offset,
            len: line.len() as u64,
            sha256: sha256_hex(line),
        }
    }

    /// The sample events of `fixtures/flight/events-sample.json`, built from
    /// a fixed path so the file is the same on every machine.
    fn sample() -> Vec<Event> {
        let file = "/config/projects/-work-toy/s1.jsonl";
        let line_a = br#"{"type":"user"}"#;
        let line_b = br#"{"type":"assistant"}"#;
        let a = source(file, 0, line_a);
        let b = source(file, 16, line_b);
        let mut attrs = Map::new();
        attrs.insert("files".into(), json!(["/work/toy/src/add.js"]));
        vec![
            Event {
                schema: SCHEMA,
                id: event_id(file, 0, &a.sha256),
                trace: "session:s1".into(),
                session: "s1".into(),
                agent: AgentRef::main(),
                provider: Provider {
                    name: "claude-code".into(),
                    version: Some("2.1.287".into()),
                    native: "user.text".into(),
                },
                kind: kind::TURN_START.into(),
                at: "2026-10-22T15:32:16.271Z".into(),
                span: None,
                status: None,
                name: None,
                attrs: Map::new(),
                usage: None,
                content: Content {
                    source: a,
                    policy: CapturePolicy::Metadata,
                    redactions: 0,
                },
                derived: None,
            },
            Event {
                schema: SCHEMA,
                id: event_id(file, 16, &b.sha256),
                trace: "session:s1".into(),
                session: "s1".into(),
                agent: AgentRef {
                    id: "a1".into(),
                    parent: Some("main".into()),
                    kind: "subagent".into(),
                    agent_type: Some("dcv2:implementer".into()),
                    role: Some("implementer".into()),
                    pr: Some("PR1".into()),
                },
                provider: Provider {
                    name: "claude-code".into(),
                    version: Some("2.1.287".into()),
                    native: "assistant.tool_use".into(),
                },
                kind: kind::TOOL_START.into(),
                at: "2026-10-22T15:32:41.758Z".into(),
                span: Some("toolu_01".into()),
                status: None,
                name: Some("Edit".into()),
                attrs,
                usage: Some(Usage {
                    input: 8,
                    output: 408,
                    cache_write5m: 0,
                    cache_write1h: 290,
                    cache_read: 31611,
                    model: Some("claude-haiku-4-5-20251001".into()),
                    usd: None,
                }),
                content: Content {
                    source: b,
                    policy: CapturePolicy::Metadata,
                    redactions: 0,
                },
                derived: None,
            },
        ]
    }

    #[test]
    fn the_sample_events_equal_the_checked_in_json() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/fixtures/flight/events-sample.json");
        let ours = serde_json::to_value(sample()).unwrap();
        if std::env::var("FLIGHT_WRITE_SAMPLE").is_ok() {
            std::fs::write(&path, serde_json::to_string_pretty(&ours).unwrap() + "\n").unwrap();
        }
        let theirs: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(ours, theirs, "regenerate with FLIGHT_WRITE_SAMPLE=1");
        let back: Vec<Event> = serde_json::from_value(theirs).unwrap();
        assert_eq!(back, sample());
    }

    /// One orchestration event of each new kind (and the statuses and attribute keys they use), from a
    /// fixed run path so the file is the same on every machine.
    fn run_sample() -> Vec<Event> {
        let file = "/runs/r-20261105-100000-fa17/log.md";
        let mut n = 0u64;
        let mut make = |k: &str,
                        at: &str,
                        status: Option<&str>,
                        name: Option<&str>,
                        span: Option<&str>,
                        attrs: Vec<(&str, Value)>,
                        derived: Option<&str>| {
            n += 1;
            let line = format!("line {n}");
            let src = source(file, n * 100, line.as_bytes());
            Event {
                schema: SCHEMA,
                id: event_id(file, n * 100, &src.sha256),
                trace: "r-20261105-100000-fa17".into(),
                session: "r-20261105-100000-fa17".into(),
                agent: AgentRef {
                    id: ORCHESTRATION.into(),
                    parent: None,
                    kind: ORCHESTRATION.into(),
                    agent_type: None,
                    role: None,
                    pr: None,
                },
                provider: Provider {
                    name: PROVIDER_DCV2.into(),
                    version: None,
                    native: "log.md".into(),
                },
                kind: k.into(),
                at: at.into(),
                span: span.map(str::to_string),
                status: status.map(str::to_string),
                name: name.map(str::to_string),
                attrs: attrs.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
                usage: None,
                content: Content {
                    source: src,
                    policy: CapturePolicy::Metadata,
                    redactions: 0,
                },
                derived: derived.map(str::to_string),
            }
        };
        let cargo = "cargo test --manifest-path src-tauri/Cargo.toml";
        vec![
            make(
                kind::STAGE_CHANGE,
                "2026-11-05T10:07:00.000Z",
                None,
                Some("building"),
                None,
                vec![
                    (attr::FROM, json!("plan-approval")),
                    (attr::TO, json!("building")),
                ],
                None,
            ),
            make(
                kind::APPROVAL_WAIT,
                "2026-11-05T10:03:00.000Z",
                None,
                Some("plan"),
                None,
                vec![],
                None,
            ),
            make(
                kind::APPROVAL_DECISION,
                "2026-11-05T10:06:00.000Z",
                Some(status::OK),
                Some("plan"),
                None,
                vec![],
                None,
            ),
            make(
                kind::APPROVAL_DECISION,
                "2026-11-05T10:56:00.000Z",
                Some(status::DECLINED),
                Some("ship"),
                None,
                vec![],
                None,
            ),
            make(
                kind::CHECK_END,
                "2026-11-05T10:22:00.000Z",
                Some(status::FAIL),
                Some(cargo),
                None,
                vec![
                    (attr::SOURCE, json!("app")),
                    (attr::PR, json!("PR2")),
                    (attr::HEAD, json!("abc1234")),
                    (attr::EXIT_CODE, json!(101)),
                ],
                None,
            ),
            make(
                kind::CHECK_END,
                "2026-11-05T10:21:00.000Z",
                Some(status::PASS),
                Some(cargo),
                None,
                vec![(attr::SOURCE, json!("skill")), (attr::PR, json!("PR2"))],
                None,
            ),
            make(
                kind::CHECK_END,
                "2026-11-05T10:50:00.000Z",
                Some(status::SKIP),
                Some("stop-check"),
                None,
                vec![(attr::SOURCE, json!("stop-check"))],
                None,
            ),
            make(
                kind::REVIEW_ROUND,
                "2026-11-05T10:25:30.000Z",
                None,
                Some("PR2-round1"),
                None,
                vec![
                    (attr::PR, json!("PR2")),
                    (attr::ROUND, json!(1)),
                    (attr::VERDICT, json!("FAIL")),
                ],
                None,
            ),
            make(
                kind::CI_STATE,
                "2026-11-05T10:40:00.000Z",
                Some(status::FAIL),
                Some("fail"),
                None,
                vec![
                    (attr::PR, json!("PR2")),
                    (attr::CI, json!("fail")),
                    (attr::PR_STATE, json!("OPEN")),
                ],
                None,
            ),
            make(
                kind::RETRY,
                "2026-11-05T10:30:00.000Z",
                None,
                Some("PR2"),
                None,
                vec![(attr::PR, json!("PR2"))],
                Some("exact:log-line"),
            ),
            make(
                kind::ATTENTION,
                "2026-11-05T10:26:00.000Z",
                None,
                Some("approval-wait"),
                None,
                vec![],
                None,
            ),
            make(
                kind::PR_STATE,
                "2026-11-05T10:30:00.000Z",
                None,
                Some("fixing"),
                None,
                vec![
                    (attr::PR, json!("PR2")),
                    (attr::FROM, json!("reviewing")),
                    (attr::TO, json!("fixing")),
                ],
                None,
            ),
            make(
                kind::LOG,
                "2026-11-05T10:08:00.000Z",
                None,
                None,
                None,
                vec![],
                None,
            ),
            make(
                kind::REQUEST,
                "2026-11-05T10:45:05.000Z",
                Some(status::OK),
                Some("recheck"),
                None,
                vec![
                    (attr::REQUEST_KIND, json!("recheck")),
                    (attr::RESULT, json!("done")),
                ],
                None,
            ),
            make(
                kind::TOOL_BLOCKED,
                "2026-11-05T10:12:00.000Z",
                Some(status::BLOCKED),
                Some("Bash"),
                Some("toolu_guard01"),
                vec![
                    (attr::SOURCE, json!("guard.log")),
                    (attr::RULE, json!("run-file-write")),
                    (attr::TOOL, json!("Bash")),
                    (attr::AGENT, json!("main")),
                ],
                None,
            ),
        ]
    }

    #[test]
    fn the_run_sample_equals_the_checked_in_json_and_covers_every_orchestration_kind() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/fixtures/flight/run-events-sample.json");
        let ours = serde_json::to_value(run_sample()).unwrap();
        if std::env::var("FLIGHT_WRITE_SAMPLE").is_ok() {
            std::fs::write(&path, serde_json::to_string_pretty(&ours).unwrap() + "\n").unwrap();
        }
        let theirs: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(ours, theirs, "regenerate with FLIGHT_WRITE_SAMPLE=1");
        let back: Vec<Event> = serde_json::from_value(theirs).unwrap();
        assert_eq!(back, run_sample());
        let kinds: std::collections::BTreeSet<&str> =
            back.iter().map(|e| e.kind.as_str()).collect();
        for k in [
            kind::STAGE_CHANGE,
            kind::APPROVAL_WAIT,
            kind::APPROVAL_DECISION,
            kind::CHECK_END,
            kind::REVIEW_ROUND,
            kind::CI_STATE,
            kind::RETRY,
            kind::ATTENTION,
            kind::PR_STATE,
            kind::LOG,
            kind::REQUEST,
            kind::TOOL_BLOCKED,
        ] {
            assert!(kinds.contains(k), "{k} is not in the sample");
        }
        assert!(
            kind::PR_STATE.starts_with("x.dcv2.")
                && kind::LOG.starts_with("x.dcv2.")
                && kind::REQUEST.starts_with("x.dcv2.")
        );
    }

    #[test]
    fn ids_follow_the_position_and_the_line_hash() {
        let a = event_id("/f", 10, "aa");
        assert!(a.starts_with("ev_") && a.len() == 19);
        assert_eq!(a, event_id("/f", 10, "aa"));
        assert_ne!(a, event_id("/f", 10, "bb"), "rewritten bytes, new id");
        assert_ne!(a, event_id("/f", 11, "aa"));
    }

    #[test]
    fn policies_order_by_strictness_and_default_to_metadata() {
        use CapturePolicy::*;
        assert_eq!(Full.strictest(Redacted), Redacted);
        assert_eq!(Metadata.strictest(Off), Off);
        assert_eq!(Metadata.strictest(Full), Metadata);
        assert_eq!(CapturePolicy::default(), Metadata);
        assert_eq!(serde_json::to_string(&Off).unwrap(), "\"off\"");
    }
}
