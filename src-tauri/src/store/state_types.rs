use serde::{Deserialize, Serialize};

pub const STATE_SCHEMA_VERSION: u32 = 1;
pub const MAIN_WINDOW_ID: &str = "main";
/// The account a window uses when it names none (the first one in Settings).
pub const DEFAULT_ACCOUNT_ID: &str = "default";

pub fn default_window_id() -> String {
    MAIN_WINDOW_ID.to_string()
}

/// The window label a tab's `window_id` names. Blank is the original window.
pub fn window_label(window_id: &str) -> &str {
    if window_id.trim().is_empty() {
        MAIN_WINDOW_ID
    } else {
        window_id
    }
}

/// A tab belongs to `window_id`. Blank ids are the original window.
pub fn window_matches(tab_window: &str, window_id: &str) -> bool {
    let tab_window = if tab_window.trim().is_empty() {
        MAIN_WINDOW_ID
    } else {
        tab_window
    };
    let window_id = if window_id.trim().is_empty() {
        MAIN_WINDOW_ID
    } else {
        window_id
    };
    tab_window == window_id
}

/// `state.json`: the windows, each one's account, and their tabs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppStateFile {
    pub schema_version: u32,
    #[serde(default)]
    pub tabs: Vec<TabRecord>,
    /// Newest first. Per window, see `CLOSED_TABS_PER_WINDOW`.
    #[serde(default)]
    pub closed_tabs: Vec<TabRecord>,
    /// Windows and the Claude account each one uses.
    #[serde(default)]
    pub windows: Vec<WindowRecord>,
    /// Windows closed while another stayed open, newest first. Their tabs
    /// wait in `closed_tabs` for File > Reopen Closed Window.
    #[serde(default)]
    pub archived_windows: Vec<ArchivedWindow>,
}

/// One top-level window bound to a Claude account.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WindowRecord {
    pub id: String,
    /// `claude.accounts[].id` in `settings.json`.
    pub account_id: String,
    #[serde(default)]
    pub active_tab_id: Option<String>,
}

/// A window closed while another stayed open, kept with its account so it
/// can be reopened as it was.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedWindow {
    #[serde(flatten)]
    pub window: WindowRecord,
    /// The tabs open when it closed, in tab order.
    #[serde(default)]
    pub tab_ids: Vec<String>,
    pub closed_at: String,
}

/// What a terminal tab runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabKind {
    /// `claude` under the window's account.
    #[default]
    Claude,
    /// The login shell.
    Shell,
}

impl TabKind {
    pub fn label(self) -> &'static str {
        match self {
            TabKind::Claude => "Claude",
            TabKind::Shell => "Shell",
        }
    }
}

/// One terminal tab: a folder, what runs in it, and (for Claude) the
/// session it writes, so it can be resumed after an exit or a restart.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TabRecord {
    pub id: String,
    pub label: String,
    /// You renamed the tab: its label no longer follows its folder.
    #[serde(default)]
    pub custom_label: bool,
    pub cwd: String,
    #[serde(default)]
    pub kind: TabKind,
    /// Window this tab belongs to. Blank is the original window.
    #[serde(default = "default_window_id")]
    pub window_id: String,
    pub order: u32,
    pub created_at: String,
    /// Claude: the session the terminal writes (`--session-id` / `--resume`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_session_id: Option<String>,
    /// Claude: the config folder that session belongs to. A session is only
    /// resumed under the same folder (another account cannot see its log).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_config_dir: Option<String>,
    /// When it was closed (closed tabs only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
    /// Claude tabs started from the start screen: the dcv2 plugin launch
    /// (docs/launch.md). Plain Claude tabs have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<TabLaunch>,
    /// A second pane (a Shell) beside or below the tab's own terminal. At
    /// most one; absent in a file from before split panes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<SplitPane>,
}

/// A terminal id's second-pane suffix: the tab's own terminal is the tab id,
/// the second pane's is `<tab id>:p2`.
pub const SECOND_PANE_SUFFIX: &str = ":p2";

/// The terminal id of the second pane of `tab_id`.
pub fn second_pane_id(tab_id: &str) -> String {
    format!("{tab_id}{SECOND_PANE_SUFFIX}")
}

/// A terminal id split into its tab id and whether it is the second pane.
pub fn pane_parts(terminal_id: &str) -> (&str, bool) {
    match terminal_id.strip_suffix(SECOND_PANE_SUFFIX) {
        Some(tab) => (tab, true),
        None => (terminal_id, false),
    }
}

/// Where the second pane sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    /// Side by side.
    Right,
    /// One above the other.
    Down,
}

fn half() -> f32 {
    0.5
}

/// The second pane of a tab: always a Shell, with its own terminal and
/// folder. `ratio` is the first pane's share of the tab (0.2 to 0.8).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SplitPane {
    pub direction: SplitDirection,
    #[serde(default = "half")]
    pub ratio: f32,
    /// `<tab id>:p2`.
    pub id: String,
    #[serde(default = "shell_kind")]
    pub kind: TabKind,
    pub cwd: String,
}

fn shell_kind() -> TabKind {
    TabKind::Shell
}

/// The first pane's share is kept within this.
pub const MIN_SPLIT_RATIO: f32 = 0.2;
pub const MAX_SPLIT_RATIO: f32 = 0.8;

/// What a dcv2 Claude tab adds to `claude` (docs/launch.md): env, flags and
/// the first prompt. The prompt is sent on a fresh start only; a resumed
/// session gets the env and flags again but not the prompt.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TabLaunch {
    /// `eagle-eye`, `eagle-eye-resume`, or a role skill name (`planner`, …).
    pub role: String,
    /// `/dcv2:eagle-eye run=<dir>`, `/dcv2:<role> <request>`, …
    pub prompt: String,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// `--plugin-dir <p>`, `--append-system-prompt-file <f>`.
    #[serde(default)]
    pub args: Vec<String>,
    /// `plugin/launch/settings.json`, merged with the app's own settings
    /// (the usage status line) and passed as one `--settings`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_file: Option<String>,
    /// Eagle-Eye: the run folder the progress panel reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_dir: Option<String>,
}

impl Default for AppStateFile {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            tabs: Vec::new(),
            closed_tabs: Vec::new(),
            windows: vec![WindowRecord {
                id: MAIN_WINDOW_ID.to_string(),
                account_id: DEFAULT_ACCOUNT_ID.to_string(),
                active_tab_id: None,
            }],
            archived_windows: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_window_ids_are_the_main_window() {
        assert!(window_matches("", MAIN_WINDOW_ID));
        assert!(window_matches("main", " "));
        assert!(!window_matches("win-2", MAIN_WINDOW_ID));
    }

    #[test]
    fn a_minimal_file_loads_with_defaults() {
        let raw = r#"{"schemaVersion":1,"tabs":[{"id":"tab_1","label":"Claude · app","cwd":"/w/app","order":1,"createdAt":"2026-10-22T00:00:00Z"}]}"#;
        let parsed: AppStateFile = serde_json::from_str(raw).unwrap();
        let tab = &parsed.tabs[0];
        assert_eq!(tab.kind, TabKind::Claude);
        assert_eq!(tab.window_id, MAIN_WINDOW_ID);
        assert!(!tab.custom_label);
        assert!(
            parsed.windows.is_empty(),
            "windows come from the file, not the default"
        );
        let json = serde_json::to_value(tab).unwrap();
        assert!(
            json.get("claudeSessionId").is_none(),
            "unset ids are not written"
        );
        assert_eq!(json["kind"], "claude");
    }
}
