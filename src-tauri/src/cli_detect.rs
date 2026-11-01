//! Helpers for asking a CLI something: find it on `PATH`, run it with a
//! timeout and no window, and read its signed-in state.

use serde::Serialize;
use std::path::{Path, PathBuf};

pub(crate) fn find_on_path(binary: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for candidate in candidate_names(binary) {
            let full = dir.join(&candidate);
            if full.is_file() {
                return Some(full);
            }
        }
    }
    None
}

fn candidate_names(binary: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        let mut names = Vec::new();
        if let Ok(pathext) = std::env::var("PATHEXT") {
            for ext in pathext.split(';') {
                let ext = ext.trim();
                if ext.is_empty() {
                    continue;
                }
                let ext = if ext.starts_with('.') {
                    ext.to_string()
                } else {
                    format!(".{}", ext)
                };
                names.push(format!("{}{}", binary, ext));
            }
        }
        names.push(format!("{}.exe", binary));
        names.push(format!("{}.cmd", binary));
        names.push(binary.to_string());
        names
    }
    #[cfg(not(windows))]
    {
        vec![binary.to_string()]
    }
}

/// Whether the Claude CLI is signed in (`claude auth status --json`).
/// Display only; nothing is stored.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LoginStatus {
    /// `loggedIn`, `loggedOut`, `unknown`, `noCli`, or (Claude)
    /// `noConfigDir` when the Claude config folder does not exist.
    pub state: String,
    pub account: Option<String>,
    /// Short text to show when the state is not clear.
    pub detail: Option<String>,
    /// `ANTHROPIC_API_KEY` is set (its value is never read back).
    pub api_key_env: bool,
    /// Claude: login method and plan, e.g. `claude.ai · team`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Claude: organization name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
}

pub(crate) fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

const STATUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Why [`run_cli_within`] returned no output.
#[derive(Debug)]
pub(crate) enum CliRunError {
    /// Killed after the timeout.
    TimedOut(String),
    /// Could not be started or waited on.
    Failed(String),
}

impl CliRunError {
    fn into_message(self) -> String {
        match self {
            CliRunError::TimedOut(message) | CliRunError::Failed(message) => message,
        }
    }
}

/// Run `<program> <args>` with extra env, no window, stdin closed, and a
/// timeout. Returns stdout, stderr, success.
pub(crate) fn run_cli(
    program: &Path,
    args: &[&str],
    env: &[(String, String)],
) -> Result<(String, String, bool), String> {
    run_cli_within(program, args, env, STATUS_TIMEOUT).map_err(CliRunError::into_message)
}

/// [`run_cli`] with its own timeout.
pub(crate) fn run_cli_within(
    program: &Path,
    args: &[&str],
    env: &[(String, String)],
    timeout: std::time::Duration,
) -> Result<(String, String, bool), CliRunError> {
    use std::io::Read;
    use std::process::Stdio;
    let mut command = crate::platform::quiet_command(program);
    for (key, value) in env {
        command.env(key, value);
    }
    command
        .args(args)
        .env("NO_COLOR", "1")
        .env("NO_OPEN_BROWSER", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|err| {
        CliRunError::Failed(format!(
            "could not run {} {}: {err}",
            program.display(),
            args.join(" ")
        ))
    })?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| CliRunError::Failed("no stdout".into()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| CliRunError::Failed("no stderr".into()))?;
    let out_thread = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let err_thread = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CliRunError::TimedOut(format!(
                    "{} {} timed out",
                    program.display(),
                    args.join(" ")
                )));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(err) => return Err(CliRunError::Failed(err.to_string())),
        }
    };
    Ok((
        out_thread.join().unwrap_or_default(),
        err_thread.join().unwrap_or_default(),
        status.success(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_colour_codes_are_stripped() {
        assert_eq!(
            strip_ansi("\u{1b}[32m ✓ Logged in\u{1b}[0m"),
            " ✓ Logged in"
        );
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[cfg(unix)]
    #[test]
    fn a_cli_that_hangs_is_killed_after_the_timeout() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::test_support::TempDir::new("cli_detect_hang");
        let cli = dir.join("hangs");
        std::fs::write(&cli, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        let timeout = std::time::Duration::from_millis(300);
        let started = std::time::Instant::now();
        let result = run_cli_within(&cli, &["--version"], &[], timeout);
        assert!(
            started.elapsed() < timeout + std::time::Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert!(matches!(result, Err(CliRunError::TimedOut(_))));
    }

    #[cfg(unix)]
    #[test]
    fn a_cli_gets_its_env_and_reports_its_output() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::test_support::TempDir::new("cli_detect_env");
        let cli = dir.join("echoes");
        std::fs::write(
            &cli,
            "#!/bin/sh\nprintf '%s' \"$PROBE\"; echo oops >&2; exit 3\n",
        )
        .unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (out, err, ok) = run_cli(&cli, &[], &[("PROBE".into(), "hi".into())]).unwrap();
        assert_eq!((out.as_str(), err.trim(), ok), ("hi", "oops", false));
        assert!(run_cli(&dir.join("missing"), &[], &[]).is_err());
    }
}
