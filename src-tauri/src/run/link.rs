//! The trace link `dcterminalv2://run/<runId>` (docs/launch.md, "Trace link"): one parser, one resolver and
//! one take-once pending slot, shared by the macOS URL handler and the command palette.
//!
//! The link opens only a run folder this app made (`<app data>/runs/<runId>`, through
//! [`super::record::own_run_dir`]): never an adopted run, never a path from the link. The URL handler
//! stores the newest link text in [`PENDING`]; a window pulls it with `take_run_link` (once), so a link
//! that arrives before any window has loaded, or an event a window missed, loses nothing.

use serde::Serialize;
use std::path::Path;
use std::sync::Mutex;

const PREFIX: &str = "dcterminalv2://run/";
/// A link longer than this is cut before it is kept (a hostile URL cannot grow the slot).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const MAX_LINK_CHARS: usize = 512;

/// `r-YYYYMMDD-HHMMSS-<4 lowercase hex>`.
pub fn is_run_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('-').collect();
    matches!(parts.as_slice(), ["r", date, time, hex]
        if date.len() == 8
            && date.bytes().all(|b| b.is_ascii_digit())
            && time.len() == 6
            && time.bytes().all(|b| b.is_ascii_digit())
            && hex.len() == 4
            && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

/// The run id of `dcterminalv2://run/<runId>` or of a bare run id, after trimming whitespace; nothing else
/// (no query, fragment, extra path, other host, or uppercase hex).
pub fn parse_run_link(text: &str) -> Option<String> {
    let text = text.trim();
    let id = text.strip_prefix(PREFIX).unwrap_or(text);
    is_run_id(id).then(|| id.to_string())
}

/// What a link resolves to: the run folder, or the message to show.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunLinkTarget {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl RunLinkTarget {
    fn found(run_dir: String) -> Self {
        Self {
            run_dir: Some(run_dir),
            error: None,
        }
    }

    fn failed(error: String) -> Self {
        Self {
            run_dir: None,
            error: Some(error),
        }
    }
}

/// Resolves link text against this app's own runs. A malformed link never echoes its text.
pub fn resolve(data: &Path, text: &str) -> RunLinkTarget {
    let Some(id) = parse_run_link(text) else {
        return RunLinkTarget::failed("That run is not on this Mac".into());
    };
    let candidate = super::folder::runs_dir(data).join(&id);
    match super::record::own_run_dir(data, &candidate.display().to_string()) {
        Some(dir) => RunLinkTarget::found(dir.display().to_string()),
        None => RunLinkTarget::failed(format!("Run {id} is not on this Mac")),
    }
}

/// One link waiting for a window; the newest wins.
#[derive(Debug, Default)]
pub struct PendingLink(Mutex<Option<String>>);

impl PendingLink {
    pub const fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// Keeps `text` (cut to a safe length) in place of any older link. Only the macOS URL handler calls it.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn set(&self, text: &str) {
        let cut: String = text.chars().take(MAX_LINK_CHARS).collect();
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(cut);
        }
    }

    /// The waiting link, once.
    pub fn take(&self) -> Option<String> {
        self.0.lock().ok().and_then(|mut slot| slot.take())
    }
}

/// The process-wide slot the URL handler fills.
pub static PENDING: PendingLink = PendingLink::new();

/// The waiting link resolved, or `None` when nothing waits. Take-once.
pub fn take_resolved(pending: &PendingLink, data: &Path) -> Option<RunLinkTarget> {
    pending.take().map(|text| resolve(data, &text))
}

#[tauri::command]
pub fn take_run_link(app: tauri::AppHandle) -> Option<RunLinkTarget> {
    let data = crate::data_dir::app_data_dir(&app).path;
    take_resolved(&PENDING, &data)
}

#[tauri::command]
pub fn resolve_run_link(app: tauri::AppHandle, text: String) -> RunLinkTarget {
    let data = crate::data_dir::app_data_dir(&app).path;
    resolve(&data, &text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    const ID: &str = "r-20261105-230341-7b3c";

    #[test]
    fn a_link_and_a_bare_id_parse_and_nothing_else_does() {
        assert_eq!(
            parse_run_link(&format!("dcterminalv2://run/{ID}")).as_deref(),
            Some(ID)
        );
        assert_eq!(parse_run_link(ID).as_deref(), Some(ID));
        assert_eq!(
            parse_run_link(&format!("  \n dcterminalv2://run/{ID}\t ")).as_deref(),
            Some(ID),
            "whitespace is trimmed"
        );
        for bad in [
            "",
            "r-123",
            "r-20261105-230341-7B3C",
            "r-20261105-230341-7b3",
            "r-20261105-230341-7b3cc",
            "r-2026110-2303410-7b3c",
            "x-20261105-230341-7b3c",
            "dcterminalv2://run/",
            "dcterminalv2://runs/r-20261105-230341-7b3c",
            "dcterminalv2://other/r-20261105-230341-7b3c",
            "DCTERMINALV2://run/r-20261105-230341-7b3c",
            "https://run/r-20261105-230341-7b3c",
            "dcterminalv2://run/r-20261105-230341-7b3c?x=1",
            "dcterminalv2://run/r-20261105-230341-7b3c#top",
            "dcterminalv2://run/r-20261105-230341-7b3c/",
            "dcterminalv2://run/r-20261105-230341-7b3c/extra",
            "dcterminalv2://run/../r-20261105-230341-7b3c",
            "dcterminalv2://run/../../etc/passwd",
            "../r-20261105-230341-7b3c",
            "r-20261105-230341-7b3c/..",
            "r-20261105-230341-7b3c r-20261105-230341-7b3c",
        ] {
            assert_eq!(parse_run_link(bad), None, "{bad:?}");
        }
    }

    fn own_run(data: &Path) -> String {
        let dir = super::super::folder::runs_dir(data).join(ID);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap().display().to_string()
    }

    #[test]
    fn an_own_run_resolves_and_an_unknown_adopted_or_malformed_one_gives_the_message() {
        let dir = TempDir::new("run_link");
        let data = dir.join("data");
        let own = own_run(&data);
        let found = resolve(&data, &format!("dcterminalv2://run/{ID}"));
        assert_eq!(found.run_dir.as_deref(), Some(own.as_str()));
        assert_eq!(found.error, None);

        let unknown = resolve(&data, "r-20261105-230341-0000");
        assert_eq!(unknown.run_dir, None);
        assert_eq!(
            unknown.error.as_deref(),
            Some("Run r-20261105-230341-0000 is not on this Mac")
        );

        // An adopted run folder with a valid id is refused: own runs only.
        let dev = dir.join("dev/runs/r-20261105-230341-ad0e");
        std::fs::create_dir_all(&dev).unwrap();
        crate::run::folder::adopt_runs(&data, std::slice::from_ref(&dev)).unwrap();
        let adopted = resolve(&data, "r-20261105-230341-ad0e");
        assert_eq!(adopted.run_dir, None);
        assert_eq!(
            adopted.error.as_deref(),
            Some("Run r-20261105-230341-ad0e is not on this Mac")
        );

        // A malformed link never echoes its text.
        let bad = resolve(&data, "r-123 <script>");
        assert_eq!(bad.run_dir, None);
        assert_eq!(bad.error.as_deref(), Some("That run is not on this Mac"));
    }

    #[test]
    fn a_link_in_the_slot_is_taken_once_and_the_newest_wins() {
        let dir = TempDir::new("run_link_slot");
        let data = dir.join("data");
        let own = own_run(&data);
        let slot = PendingLink::new();
        assert_eq!(take_resolved(&slot, &data), None);
        slot.set("r-20261105-230341-0000");
        slot.set(&format!("dcterminalv2://run/{ID}"));
        let first = take_resolved(&slot, &data).expect("a link waits");
        assert_eq!(first.run_dir.as_deref(), Some(own.as_str()));
        assert_eq!(
            take_resolved(&slot, &data),
            None,
            "the second take is empty"
        );
        // An invalid link is kept too, so the window can say so.
        slot.set("nonsense");
        let bad = take_resolved(&slot, &data).expect("kept");
        assert!(bad.error.is_some());
    }

    #[test]
    fn a_hostile_link_is_cut_before_it_is_kept() {
        let slot = PendingLink::new();
        slot.set(&"x".repeat(100_000));
        assert_eq!(slot.take().map(|t| t.len()), Some(MAX_LINK_CHARS));
    }
}
