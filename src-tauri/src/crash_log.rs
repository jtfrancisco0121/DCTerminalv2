//! Crash log: a panic hook that appends what panicked, where, and a
//! backtrace to `<data dir>/logs/crash.log`. Release builds use
//! `panic = "abort"`; the hook still runs first, so the file is written
//! before the process goes away.

use std::any::Any;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Above this size `crash.log` moves to `crash.log.1` before the next entry.
pub const CRASH_LOG_MAX_BYTES: u64 = 1024 * 1024;

pub fn crash_log_path(data_dir: &Path) -> PathBuf {
    data_dir.join("logs").join("crash.log")
}

/// Installs the hook. The default hook still runs first, so a panic is
/// printed to stderr as before.
pub fn install(data_dir: &Path) {
    let path = crash_log_path(data_dir);
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        let thread = std::thread::current();
        let location = info
            .location()
            .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
            .unwrap_or_else(|| "unknown location".to_string());
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        let entry = format_crash_entry(
            &chrono::Utc::now().to_rfc3339(),
            thread.name().unwrap_or("<unnamed>"),
            &panic_message(info.payload()),
            &location,
            &backtrace,
        );
        // Nothing to do if the log cannot be written; never panic in here.
        let _ = append_crash_entry(&path, &entry, CRASH_LOG_MAX_BYTES);
    }));
}

/// The panic's message: the `&str` or `String` passed to `panic!`.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "<non-text panic payload>".to_string()
    }
}

/// One crash entry, a blank line after it.
pub fn format_crash_entry(
    time: &str,
    thread: &str,
    message: &str,
    location: &str,
    backtrace: &str,
) -> String {
    format!(
        "=== {time} · thread '{thread}' panicked at {location}\n{message}\nbacktrace:\n{}\n\n",
        backtrace.trim_end()
    )
}

/// Appends `entry` to `path`, first moving a log above `max_bytes` to `.1`.
pub fn append_crash_entry(path: &Path, entry: &str, max_bytes: u64) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    rotate_log_if_large(path, max_bytes);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(entry.as_bytes())?;
    file.flush()
}

/// Rename `path` to `path.1` (replacing an older `.1`) once it reaches
/// `max_bytes`.
fn rotate_log_if_large(path: &Path, max_bytes: u64) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() < max_bytes {
        return;
    }
    let mut rolled = path.as_os_str().to_owned();
    rolled.push(".1");
    let rolled = PathBuf::from(rolled);
    let _ = std::fs::remove_file(&rolled);
    let _ = std::fs::rename(path, &rolled);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn an_entry_names_time_thread_message_location_and_backtrace() {
        let entry = format_crash_entry(
            "2026-10-16T00:00:00Z",
            "prompt-turn",
            "index out of bounds",
            "src/lib.rs:10:5",
            "   0: dcterminalv2::run\n",
        );
        assert_eq!(
            entry,
            "=== 2026-10-16T00:00:00Z · thread 'prompt-turn' panicked at src/lib.rs:10:5\n\
             index out of bounds\nbacktrace:\n   0: dcterminalv2::run\n\n"
        );
    }

    #[test]
    fn entries_append_and_a_full_log_rotates_to_dot_one() {
        let dir = TempDir::new("crash_log");
        let path = crash_log_path(dir.path());
        append_crash_entry(&path, "first\n", 32).unwrap();
        append_crash_entry(&path, "second\n", 32).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first\nsecond\n");

        // Past the limit: the old log moves aside and a new one starts.
        append_crash_entry(&path, &"x".repeat(40), 32).unwrap();
        append_crash_entry(&path, "after\n", 32).unwrap();
        let rolled = dir.join("logs").join("crash.log.1");
        assert_eq!(
            std::fs::read_to_string(&rolled).unwrap(),
            format!("first\nsecond\n{}", "x".repeat(40))
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after\n");
    }

    #[test]
    fn the_message_comes_from_str_or_string_payloads() {
        assert_eq!(panic_message(&"boom"), "boom");
        assert_eq!(panic_message(&String::from("bang")), "bang");
        assert_eq!(panic_message(&42u8), "<non-text panic payload>");
    }
}
