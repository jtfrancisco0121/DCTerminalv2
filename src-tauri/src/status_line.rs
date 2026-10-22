//! Claude usage from terminal tabs, through Claude Code's statusLine (plan
//! D32 as amended by V0 T9, `docs/claude-hooks-observed.md`).
//!
//! Claude Code runs a `statusLine` command on each render with a JSON
//! description of the session on stdin. Since 2.1.287 that JSON carries
//! `rate_limits.five_hour` and `rate_limits.seven_day`, each with
//! `used_percentage` and `resets_at` (unix seconds). It is missing on the
//! first render, before any API response, so it is optional throughout.
//!
//! A Claude terminal tab DCTerminalv2 starts gets `--settings` with a
//! `statusLine` that runs this app's own executable with
//! `--dct-status-line <file>`. In that mode (`run_if_requested`, called first
//! in `main`) the app writes the rate windows it read to `<file>`, under
//! `status-line/` in app data, prints nothing (so the status line stays as
//! blank as Claude Code's default) and exits. `UsageStore` folds those files
//! in when usage is read.
//!
//! Your own status line wins: when the account's `settings.json` or the
//! folder's `.claude/settings.json` / `.claude/settings.local.json` sets
//! `statusLine`, DCTerminalv2 adds none, and usage from that tab stays unknown.
//! Nothing under the config folder is written.

use crate::usage::{window_label, RateWindow};
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};

/// The argument that starts the app as a tab's status line command.
pub const STATUS_LINE_FLAG: &str = "--dct-status-line";
/// Folder in app data holding one reading per terminal tab.
pub const STATUS_LINE_DIR: &str = "status-line";
/// Most stdin read from Claude Code (its JSON is a few kilobytes).
const MAX_INPUT_BYTES: u64 = 1024 * 1024;

/// When the process was started as a status line command, record stdin and
/// return true: the caller exits without starting the app. Never prints.
pub fn run_if_requested() -> bool {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(STATUS_LINE_FLAG)) {
        return false;
    }
    // Only ever a `status-line/<tab>.json` file (`file_for`), never another path.
    let Some(out) = args
        .next()
        .map(PathBuf::from)
        .filter(|out| is_status_line_file(out))
    else {
        return true;
    };
    let mut input = String::new();
    // intentionally silent: anything printed here would show in Claude's status line.
    if std::io::stdin()
        .take(MAX_INPUT_BYTES)
        .read_to_string(&mut input)
        .is_ok()
    {
        let _ = record(&input, &out, chrono::Utc::now().timestamp_millis());
    }
    true
}

/// The rate windows in one statusLine input, seen at `seen_at_ms`. Empty
/// when it has none (the first render) or is not JSON. A window without
/// `used_percentage` keeps its percent unknown, never 0.
pub fn windows_from_input(input: &str, seen_at_ms: i64) -> Vec<RateWindow> {
    let Ok(value) = serde_json::from_str::<Value>(input) else {
        return Vec::new();
    };
    let Some(limits) = value.get("rate_limits").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut windows: Vec<RateWindow> = limits
        .iter()
        .filter_map(|(kind, window)| {
            let window = window.as_object()?;
            let utilization = window
                .get("used_percentage")
                .and_then(Value::as_f64)
                .filter(|used| used.is_finite() && *used >= 0.0)
                .map(|used| (used * 10.0).round() / 10.0);
            Some(RateWindow {
                rate_limit_type: kind.clone(),
                label: window_label(kind),
                utilization,
                resets_at: window.get("resets_at").and_then(Value::as_i64),
                // The input names no status: a full window is a reached limit.
                status: if utilization.is_some_and(|used| used >= 100.0) {
                    "rejected".to_string()
                } else {
                    "allowed".to_string()
                },
                seen_at_ms,
            })
        })
        .collect();
    windows.sort_by(|a, b| a.rate_limit_type.cmp(&b.rate_limit_type));
    windows
}

/// Write the input's rate windows to `out` (atomically). Returns whether
/// anything was written: an input without them keeps the last reading.
pub fn record(input: &str, out: &Path, seen_at_ms: i64) -> Result<bool, String> {
    let windows = windows_from_input(input, seen_at_ms);
    if windows.is_empty() {
        return Ok(false);
    }
    let parent = out
        .parent()
        .ok_or_else(|| "path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    // Written on every render: no backup copy, just a rename over the last one.
    let tmp = out.with_extension(format!("{}.tmp", std::process::id()));
    let data = serde_json::to_vec(&windows).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, out).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(true)
}

/// The windows a tab's status line last wrote; none when the file is
/// missing or damaged.
pub fn read_windows(path: &Path) -> Vec<RateWindow> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// You set a status line of your own, for this account or this folder.
/// A settings file that is missing or does not parse sets none.
pub fn user_has_status_line(config_dir: &Path, cwd: &Path) -> bool {
    [
        config_dir.join("settings.json"),
        cwd.join(".claude").join("settings.json"),
        cwd.join(".claude").join("settings.local.json"),
    ]
    .iter()
    .any(|path| {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|settings| {
                settings
                    .get("statusLine")
                    .is_some_and(|line| !line.is_null())
            })
    })
}

/// Where a tab's status line writes. The tab id is kept to safe characters.
pub fn file_for(data_dir: &Path, tab_id: &str) -> PathBuf {
    let name: String = tab_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    data_dir.join(STATUS_LINE_DIR).join(format!("{name}.json"))
}

/// `path` is a file `file_for` names: `<…>/status-line/<safe name>.json`.
fn is_status_line_file(path: &Path) -> bool {
    let in_folder = path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == STATUS_LINE_DIR);
    let named = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".json"))
        .is_some_and(|stem| {
            !stem.is_empty()
                && stem
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        });
    path.is_absolute() && in_folder && named
}

/// The shell command Claude Code runs: this executable, quoted for the shell
/// Claude Code runs it in (single quotes for a POSIX shell, double quotes on
/// Windows).
pub fn command(exe: &Path, out: &Path) -> String {
    let quote = |path: &Path| {
        let text = path.display().to_string();
        if cfg!(windows) {
            format!("\"{}\"", text.replace('"', ""))
        } else {
            format!("'{}'", text.replace('\'', "'\\''"))
        }
    };
    format!("{} {STATUS_LINE_FLAG} {}", quote(exe), quote(out))
}

/// The `--settings` value setting the status line to `command`. It goes in
/// `TerminalLaunch::settings`, so Claude's argument builder puts it with
/// the other Claude flags.
pub fn settings(command: &str) -> Value {
    json!({ "statusLine": { "type": "command", "command": command } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// The statusLine input captured in V0 (T9), trimmed to what matters.
    fn input(rate_limits: Option<Value>) -> String {
        let mut value = json!({
            "session_id": "685dd464-eabf-4e4c-9f6f-ac02c63aad27",
            "model": { "id": "claude-haiku-4-5-20251001", "display_name": "Haiku 4.5" },
            "version": "2.1.287",
            "context_window": { "used_percentage": 19, "remaining_percentage": 81 },
        });
        if let Some(limits) = rate_limits {
            value["rate_limits"] = limits;
        }
        value.to_string()
    }

    fn limits() -> Value {
        json!({
            "five_hour": { "used_percentage": 42.25, "resets_at": 1_791_538_800 },
            "seven_day": { "used_percentage": 100, "resets_at": 1_792_000_000 },
        })
    }

    #[test]
    fn rate_limits_become_windows_and_the_first_render_has_none() {
        let windows = windows_from_input(&input(Some(limits())), 7);
        assert_eq!(windows.len(), 2);
        let five = windows
            .iter()
            .find(|w| w.rate_limit_type == "five_hour")
            .unwrap();
        assert_eq!(five.label, "5h");
        assert_eq!(five.utilization, Some(42.3));
        assert_eq!(five.resets_at, Some(1_791_538_800));
        assert_eq!(five.status, "allowed");
        assert_eq!(five.seen_at_ms, 7);
        let seven = windows
            .iter()
            .find(|w| w.rate_limit_type == "seven_day")
            .unwrap();
        assert_eq!(
            (seven.label.as_str(), seven.status.as_str()),
            ("7d", "rejected")
        );

        // First render: no rate_limits. Unknown, never 0%.
        assert!(windows_from_input(&input(None), 7).is_empty());
        assert!(windows_from_input("not json", 7).is_empty());
        // A window without a percent keeps it unknown, not 0.
        let partial = windows_from_input(
            &input(Some(
                json!({ "five_hour": { "resets_at": 5 }, "seven_day": "junk" }),
            )),
            7,
        );
        assert_eq!(partial.len(), 1);
        assert_eq!(
            (partial[0].utilization, partial[0].resets_at),
            (None, Some(5))
        );
    }

    #[test]
    fn a_reading_is_written_and_the_first_render_keeps_the_last_one() {
        let dir = TempDir::new("status_line_record");
        let out = file_for(&dir.join("data"), "tab_1");
        assert!(out.starts_with(dir.join("data").join(STATUS_LINE_DIR)));
        assert_eq!(record(&input(Some(limits())), &out, 9), Ok(true));
        assert_eq!(read_windows(&out).len(), 2);
        assert_eq!(record(&input(None), &out, 10), Ok(false));
        let kept = read_windows(&out);
        assert_eq!(kept.len(), 2, "the last reading stays");
        assert!(kept.iter().all(|w| w.seen_at_ms == 9));
        std::fs::write(&out, "{oops").unwrap();
        assert!(read_windows(&out).is_empty());
        assert!(read_windows(&dir.join("missing.json")).is_empty());
    }

    #[test]
    fn a_tab_id_cannot_leave_the_status_line_folder() {
        let dir = TempDir::new("status_line_name");
        let out = file_for(dir.path(), "../../etc/x");
        assert_eq!(out.parent().unwrap(), dir.join(STATUS_LINE_DIR));
        assert!(!out.to_string_lossy().contains(".."));
        // The status line mode writes only such a file, never another path.
        assert!(is_status_line_file(&out));
        assert!(is_status_line_file(&file_for(dir.path(), "tab_1")));
        assert!(!is_status_line_file(&dir.join("settings.json")));
        assert!(!is_status_line_file(
            &dir.join(STATUS_LINE_DIR).join("x.txt")
        ));
        assert!(
            !is_status_line_file(Path::new("status-line/tab_1.json")),
            "relative"
        );
        assert!(!is_status_line_file(
            &dir.join(STATUS_LINE_DIR).join("..json")
        ));
    }

    #[test]
    fn your_own_status_line_in_the_account_or_the_folder_wins() {
        let config = TempDir::new("status_line_config");
        let cwd = TempDir::new("status_line_cwd");
        assert!(config.path().starts_with(crate::test_support::test_root()));
        assert!(!user_has_status_line(config.path(), cwd.path()));
        std::fs::write(config.join("settings.json"), r#"{"model":"opus"}"#).unwrap();
        assert!(!user_has_status_line(config.path(), cwd.path()));
        std::fs::write(config.join("settings.json"), "{broken").unwrap();
        assert!(!user_has_status_line(config.path(), cwd.path()));
        std::fs::write(
            config.join("settings.json"),
            r#"{"statusLine":{"type":"command","command":"~/bin/line"}}"#,
        )
        .unwrap();
        assert!(user_has_status_line(config.path(), cwd.path()));
        std::fs::remove_file(config.join("settings.json")).unwrap();
        std::fs::create_dir_all(cwd.join(".claude")).unwrap();
        std::fs::write(
            cwd.join(".claude/settings.local.json"),
            r#"{"statusLine":{"type":"command","command":"x"}}"#,
        )
        .unwrap();
        assert!(user_has_status_line(config.path(), cwd.path()));
    }

    #[test]
    fn the_command_quotes_paths_and_the_settings_name_it() {
        let cmd = command(
            Path::new("/Applications/DC Terminal.app/Contents/MacOS/dcterminal"),
            Path::new("/Users/me/Library/it's/status-line/tab_1.json"),
        );
        if cfg!(windows) {
            assert!(cmd.starts_with('"'), "{cmd}");
        } else {
            assert_eq!(
                cmd,
                "'/Applications/DC Terminal.app/Contents/MacOS/dcterminal' --dct-status-line \
                 '/Users/me/Library/it'\\''s/status-line/tab_1.json'"
            );
        }
        let value: Value = serde_json::from_str(&settings(&cmd).to_string()).unwrap();
        assert_eq!(
            value,
            json!({ "statusLine": { "type": "command", "command": cmd } })
        );
    }
}
