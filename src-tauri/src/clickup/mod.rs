//! ClickUp (P4, ported from DCTerminal `src-tauri/src/clickup/`). The app
//! talks to ClickUp itself, never Claude: a token handed to Claude would end
//! up in its prompt, transcript and session log. It reads tickets, and writes
//! exactly two things, each one ticked by you: a comment on a task and a
//! task's status.
//!
//! Each Claude account has its own token (work and personal); a window or a
//! run only ever uses its own account's.
//!
//! The token lives in the macOS keychain (service
//! `com.jtfrancisco.dcterminalv2.clickup`, account field = the Claude account
//! id), is held in a [`ClickUpToken`] whose `Debug` and `Display` print
//! `***`, and is only ever written into the `Authorization` header of a
//! request to ClickUp. It is never in a JSON store, a run file, a log line, an
//! error string, an IPC reply or a child process's env.
//!
//! - `secret`: the keychain behind [`SecretStore`].
//! - `transport`: one HTTPS call with a deadline. Tests feed recorded replies.
//! - `client`: the read calls and the two writes, with at most 4 in flight
//!   and the rate limit.
//! - `types`: ClickUp's JSON, as far as intake needs it.
//! - `intake`: Fetch, the run's `tickets.json` at Start, and R8's change check.
//! - `writeback`: posting the skill's `writeback.json` drafts you tick.
//! - `autopost`: after the yes at stop #2, the comment drafts (never the
//!   statuses) posted on their own (per-account setting, default on).
//! - `secret_scan`: memory.sh's secret rules, for the auto-posted comments.

pub mod autopost;
pub mod backoff;
pub(crate) mod client;
pub mod commands;
pub mod intake;
pub mod secret;
pub mod secret_scan;
pub mod transport;
pub mod trust;
pub mod types;
pub mod writeback;

pub use client::{ClickUp, ClickUpError};
pub use secret::{KeychainStore, SecretStore};
pub use transport::UreqTransport;

use std::fmt;

/// Every request goes here; nothing else is ever sent the token.
pub const API_BASE: &str = "https://api.clickup.com/api/v2";

/// The longest token accepted. ClickUp's personal tokens are about 50 chars.
const MAX_TOKEN_CHARS: usize = 200;

/// A ClickUp token. It has no `Serialize`, and `Debug` / `Display` redact
/// it, so it cannot reach JSON, a log line or an error by accident. Only
/// [`ClickUpToken::expose`] gives the text, for the `Authorization` header
/// and the keychain.
#[derive(Clone, PartialEq, Eq)]
pub struct ClickUpToken(String);

impl ClickUpToken {
    /// Trims what was pasted and checks it looks like a token. The error
    /// never repeats the input.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let token = raw.trim();
        if token.is_empty() {
            return Err("Paste your ClickUp token first.".into());
        }
        if token.chars().count() > MAX_TOKEN_CHARS {
            return Err("That is too long to be a ClickUp token.".into());
        }
        if token
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || !c.is_ascii())
        {
            return Err("A ClickUp token has no spaces or special characters. Copy it again from ClickUp › Settings › Apps.".into());
        }
        Ok(Self(token.to_string()))
    }

    /// The token itself. Callers: the `Authorization` header and the keychain.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ClickUpToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ClickUpToken(***)")
    }
}

impl fmt::Display for ClickUpToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// A ClickUp id that is safe in an endpoint path: task, list and team ids,
/// and custom task ids such as `QA-128`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_never_prints() {
        let token = ClickUpToken::parse("  pk_123_SECRETVALUE \n").unwrap();
        assert_eq!(token.expose(), "pk_123_SECRETVALUE");
        assert_eq!(format!("{token}"), "***");
        assert_eq!(format!("{token:?}"), "ClickUpToken(***)");
        assert_eq!(
            format!("{:?}", Some(token.clone())),
            "Some(ClickUpToken(***))"
        );
    }

    #[test]
    fn bad_tokens_are_refused_without_echoing_them() {
        for bad in ["", "   ", "pk_1 2", "pk_\u{0}x", "pk_é"] {
            let err = ClickUpToken::parse(bad).unwrap_err();
            let pasted = bad.trim();
            if !pasted.is_empty() {
                assert!(!err.contains(pasted), "{err}");
            }
        }
        assert!(ClickUpToken::parse(&"x".repeat(201)).is_err());
    }

    #[test]
    fn ids_that_could_escape_a_path_are_rejected() {
        for good in ["86b0abc12", "QA-128", "901234567", "7xk2epic01"] {
            assert!(valid_id(good), "{good}");
        }
        for bad in ["", "a/b", "../x", "a?b", "a#b", "a b", "a&team_id=1"] {
            assert!(!valid_id(bad), "{bad:?}");
        }
        assert!(!valid_id(&"1".repeat(65)));
    }
}
