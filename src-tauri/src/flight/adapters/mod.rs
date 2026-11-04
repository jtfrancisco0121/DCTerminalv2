//! Provider adapters: a provider's own records to schema-1 events. Each one
//! gets its files from the host and only reads them through `flight::tail`.

pub mod claude;
pub mod dcv2_run;

// The sibling modules, reachable as `super::schema` / `super::tail` from the
// adapters (the flight boundary scan bans `crate::` and `super::super`).
use super::{schema, tail};

use std::path::PathBuf;

/// What `agent-<id>.meta.json` says about a sub-agent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentMeta {
    pub agent_type: Option<String>,
    pub description: Option<String>,
    /// The `Agent` tool_use that started it.
    pub tool_use_id: Option<String>,
}

impl AgentMeta {
    pub fn from_json(value: &serde_json::Value) -> Self {
        let text = |key: &str| value[key].as_str().map(str::to_string);
        Self {
            agent_type: text("agentType"),
            description: text("description"),
            tool_use_id: text("toolUseId"),
        }
    }
}

/// One sub-agent transcript the host found beside the main log.
#[derive(Debug, Clone)]
pub struct SubagentFile {
    pub agent_id: String,
    pub path: PathBuf,
    pub meta: Option<AgentMeta>,
}
