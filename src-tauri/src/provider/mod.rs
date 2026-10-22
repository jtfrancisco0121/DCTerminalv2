//! Claude Code, the one CLI behind a terminal tab (D3: Claude only).
//!
//! `claude` finds and runs the CLI, `claude_config` resolves an account's
//! config folder, and `claude_detect` looks the CLI up and asks whether it
//! is signed in.

pub mod claude;
pub mod claude_config;
pub mod claude_detect;

pub use claude::ClaudeProvider;

/// A program to run: argv plus extra environment. `env` is applied on top of
/// the inherited environment (an entry overwrites an inherited value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramArgs {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl ProgramArgs {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            env: Vec::new(),
        }
    }
}

/// What kind of Claude terminal to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalKind {
    /// `claude` with no policy flags, pinned to a new session id.
    Plain,
    /// Reopen a Claude session by id (`--resume`).
    Resume { session_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalLaunch {
    pub kind: TerminalKind,
    /// `Plain`: pin the new session to this id (`--session-id`).
    pub session_id: Option<String>,
    /// One `--settings` value, put with the other Claude flags (after any
    /// `node` script): the usage status line.
    pub settings: Option<serde_json::Value>,
}

/// A fresh v4 UUID for `claude --session-id`.
pub fn new_claude_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
