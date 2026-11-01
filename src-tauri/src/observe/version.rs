//! R3: the Claude Code versions this app is tested on, and the one-click
//! haiku self-test after the detected version changes.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Oldest Claude Code the app supports.
pub const CLAUDE_MIN: (u32, u32, u32) = (2, 1, 287);
/// Newest Claude Code the app (and its fixtures) were tested on.
pub const CLAUDE_MAX_TESTED: (u32, u32, u32) = (2, 1, 287);

pub fn version_text(v: (u32, u32, u32)) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

/// `2.1.287 (Claude Code)` → (2, 1, 287).
pub fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let word = text.split_whitespace().find(|w| w.contains('.'))?;
    let mut parts = word.trim_start_matches('v').split('.');
    let mut next = || -> Option<u32> {
        let part = parts.next()?;
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    };
    Some((next()?, next()?, next()?))
}

/// `below`, `ok`, `above` (newer than tested) or `unknown`.
pub fn range_state(version: Option<&str>) -> &'static str {
    match version.and_then(parse_version) {
        None => "unknown",
        Some(v) if v < CLAUDE_MIN => "below",
        Some(v) if v > CLAUDE_MAX_TESTED => "above",
        Some(_) => "ok",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SelfTest {
    pub version: Option<String>,
    pub ok: bool,
    pub plugin_loaded: bool,
    pub agent_spawned: bool,
    pub agent_completed: bool,
    pub finished: bool,
    pub at: String,
    pub detail: Option<String>,
    pub seconds: f64,
}

pub const SELFTEST_FILE: &str = "claude-selftest.json";
pub const SELFTEST_TIMEOUT: Duration = Duration::from_secs(45);
pub const SELFTEST_DONE: &str = "SELFTEST-DONE";
pub const SELFTEST_AGENT: &str = "dcv2:plan-reviewer";

pub fn selftest_prompt() -> String {
    format!(
        "This is a self-test of the dcv2 plugin. Use the Agent tool exactly once, in the foreground, \
         with subagent_type \"{SELFTEST_AGENT}\", model \"haiku\", description \"dcv2 self-test\" and \
         prompt \"Self-test only. Do not read or write any files and do not run commands. Reply with \
         exactly: OK\". After it returns, reply with exactly: {SELFTEST_DONE}"
    )
}

pub fn selftest_args(plugin: &Path) -> Vec<String> {
    vec![
        "-p".into(),
        "--model".into(),
        "haiku".into(),
        "--plugin-dir".into(),
        plugin.display().to_string(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        // The self-test leaves no session behind in the account folder.
        "--no-session-persistence".into(),
        "--max-turns".into(),
        "4".into(),
        selftest_prompt(),
    ]
}

/// Reads `claude -p --output-format stream-json --verbose` output.
pub fn evaluate_stream(text: &str) -> SelfTest {
    let mut out = SelfTest::default();
    let mut agent_tool: Option<String> = None;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match (v["type"].as_str(), v["subtype"].as_str()) {
            (Some("system"), Some("init")) => {
                out.version = v["claude_code_version"].as_str().map(str::to_string);
                let plugin = v["plugins"]
                    .as_array()
                    .is_some_and(|p| p.iter().any(|p| p["name"] == "dcv2"));
                let agents = v["agents"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|a| a.as_str() == Some(SELFTEST_AGENT)));
                out.plugin_loaded = plugin && agents;
            }
            (Some("assistant"), _) => {
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"] == "tool_use"
                        && matches!(block["name"].as_str(), Some("Agent") | Some("Task"))
                        && block["input"]["subagent_type"]
                            .as_str()
                            .is_some_and(|t| t.starts_with("dcv2:"))
                    {
                        out.agent_spawned = true;
                        agent_tool = block["id"].as_str().map(str::to_string);
                    }
                }
            }
            (Some("user"), _) => {
                let done = v["tool_use_result"]["status"] == "completed";
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"] == "tool_result"
                        && agent_tool.as_deref() == block["tool_use_id"].as_str()
                        && block["is_error"].as_bool() != Some(true)
                        && done
                    {
                        out.agent_completed = true;
                    }
                }
            }
            (Some("result"), _) => {
                out.finished = v["subtype"] == "success"
                    && v["result"]
                        .as_str()
                        .is_some_and(|r| r.contains(SELFTEST_DONE));
                if v["is_error"].as_bool() == Some(true) {
                    out.detail = v["result"].as_str().map(|r| r.chars().take(300).collect());
                }
            }
            _ => {}
        }
    }
    out.ok = out.plugin_loaded && out.agent_spawned && out.agent_completed && out.finished;
    if !out.ok && out.detail.is_none() {
        out.detail = Some(
            match (
                out.plugin_loaded,
                out.agent_spawned,
                out.agent_completed,
                out.finished,
            ) {
                (false, ..) => "The dcv2 plugin or its agents did not load.",
                (true, false, ..) => "No dcv2 agent was spawned.",
                (true, true, false, _) => "The dcv2 agent did not complete.",
                _ => "Claude did not finish the self-test.",
            }
            .to_string(),
        );
    }
    out
}

pub fn read_selftest(data: &Path) -> Option<SelfTest> {
    let text = std::fs::read_to_string(data.join(SELFTEST_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_selftest(data: &Path, result: &SelfTest) -> Result<(), String> {
    std::fs::create_dir_all(data).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(result).map_err(|e| e.to_string())?;
    std::fs::write(data.join(SELFTEST_FILE), text).map_err(|e| e.to_string())
}

/// The self-test is offered when no result exists for the detected version.
pub fn selftest_wanted(detected: Option<&str>, last: Option<&SelfTest>) -> bool {
    let Some(detected) = detected.and_then(parse_version) else {
        return false;
    };
    last.and_then(|l| l.version.as_deref())
        .and_then(parse_version)
        != Some(detected)
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const SIGKILL: i32 = 9;
    if let Ok(pid) = i32::try_from(pid) {
        // Safety: signals only the self-test's own process group.
        unsafe { kill(-pid, SIGKILL) };
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: u32) {}

/// Runs the self-test: `program prefix… args` in a fresh temp folder, killed
/// after `timeout`. Output goes to a file so a full pipe never blocks it.
pub fn run_selftest(
    program: &Path,
    prefix: &[String],
    env: &[(String, String)],
    plugin: &Path,
    work: &Path,
    timeout: Duration,
) -> SelfTest {
    let started = std::time::Instant::now();
    let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let fail = |detail: String| SelfTest {
        at: at.clone(),
        detail: Some(detail),
        seconds: started.elapsed().as_secs_f64(),
        ..Default::default()
    };
    if std::fs::create_dir_all(work).is_err() {
        return fail("Could not make the self-test folder.".into());
    }
    let out_path: PathBuf = work.join("stream.jsonl");
    let Ok(out_file) = std::fs::File::create(&out_path) else {
        return fail("Could not write the self-test output.".into());
    };
    let mut command = crate::platform::quiet_command(program);
    #[cfg(unix)]
    {
        // Its own process group, so a timeout stops Claude's children too.
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .args(prefix)
        .args(selftest_args(plugin))
        .current_dir(work)
        .envs(env.iter().cloned())
        .env("CLAUDE_CODE_SUBAGENT_MODEL", "haiku")
        .env("DCV2_STOP_HOOKS", "off")
        .env_remove("DCV2_TEST_MODE")
        .stdin(std::process::Stdio::null())
        .stdout(out_file)
        .stderr(std::process::Stdio::null());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return fail(format!("claude did not start: {err}")),
    };
    let deadline = started + timeout;
    let timed_out = loop {
        match child.try_wait() {
            Ok(Some(_)) => break false,
            Ok(None) if std::time::Instant::now() >= deadline => {
                kill_group(child.id());
                let _ = child.kill();
                let _ = child.wait();
                break true;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(_) => break false,
        }
    };
    let text = std::fs::read_to_string(&out_path).unwrap_or_default();
    let _ = std::fs::remove_dir_all(work);
    let mut result = evaluate_stream(&text);
    result.at = at;
    result.seconds = (started.elapsed().as_secs_f64() * 10.0).round() / 10.0;
    if timed_out {
        result.ok = false;
        result.detail = Some(format!("Timed out after {} s.", timeout.as_secs()));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn versions_are_checked_against_the_pinned_range() {
        assert_eq!(parse_version("2.1.287 (Claude Code)"), Some((2, 1, 287)));
        assert_eq!(parse_version("v2.10.3-beta"), Some((2, 10, 3)));
        assert_eq!(parse_version("Claude Code"), None);
        assert_eq!(range_state(Some("2.1.287 (Claude Code)")), "ok");
        assert_eq!(range_state(Some("2.1.286")), "below");
        assert_eq!(range_state(Some("2.2.0")), "above");
        assert_eq!(
            range_state(Some("2.1.1000")),
            "above",
            "numeric, not text order"
        );
        assert_eq!(range_state(None), "unknown");
        assert!(CLAUDE_MIN <= CLAUDE_MAX_TESTED);
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/fixtures")
            .join(format!("claude-{}", version_text(CLAUDE_MAX_TESTED)));
        assert!(
            fixtures.is_dir(),
            "the newest tested version has recorded fixtures"
        );
    }

    #[test]
    fn the_recorded_selftest_run_passes_and_broken_ones_say_why() {
        let text = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/fixtures/claude-2.1.287/selftest-stream.jsonl"),
        )
        .unwrap();
        let result = evaluate_stream(&text);
        assert!(result.ok, "{result:?}");
        assert_eq!(result.version.as_deref(), Some("2.1.287"));
        let no_plugin = text.replace("\"name\":\"dcv2\"", "\"name\":\"other\"");
        let failed = evaluate_stream(&no_plugin);
        assert!(!failed.ok);
        assert!(failed.detail.unwrap().contains("did not load"));
        let unfinished = text.replace(SELFTEST_DONE, "nope");
        assert!(!evaluate_stream(&unfinished).ok);
        assert!(!evaluate_stream("").ok);
        assert!(selftest_args(Path::new("/p")).contains(&"--no-session-persistence".to_string()));
    }

    #[test]
    fn the_selftest_is_offered_once_per_detected_version() {
        let dir = TempDir::new("selftest");
        assert!(read_selftest(dir.path()).is_none());
        assert!(selftest_wanted(Some("2.1.287 (Claude Code)"), None));
        let done = SelfTest {
            version: Some("2.1.287".into()),
            ok: true,
            ..Default::default()
        };
        write_selftest(dir.path(), &done).unwrap();
        let last = read_selftest(dir.path()).unwrap();
        assert!(!selftest_wanted(Some("2.1.287 (Claude Code)"), Some(&last)));
        assert!(selftest_wanted(Some("2.1.290 (Claude Code)"), Some(&last)));
        assert!(!selftest_wanted(None, Some(&last)));
    }

    #[cfg(unix)]
    #[test]
    fn a_selftest_that_hangs_is_killed_at_the_timeout() {
        let dir = TempDir::new("selftest_hang");
        let script = dir.join("fake-claude.sh");
        std::fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started = std::time::Instant::now();
        let result = run_selftest(
            &script,
            &[],
            &[],
            dir.path(),
            &dir.join("work"),
            Duration::from_millis(500),
        );
        assert!(!result.ok);
        assert!(result.detail.unwrap().contains("Timed out"));
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(!dir.join("work").exists(), "the temp workspace is removed");
    }
}
