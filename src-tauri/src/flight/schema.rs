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
}

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
