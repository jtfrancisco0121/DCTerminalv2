//! App settings: the terminal, the look, and the Claude accounts a window
//! can use.

use crate::provider::claude_config::ConfigDirInfo;
use crate::store::json_io::{read_json_or_recover, set_aside_other_version, write_json_atomic};
use crate::store::state_types::DEFAULT_ACCOUNT_ID;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SETTINGS_SCHEMA_VERSION: u32 = 1;

fn default_font_size() -> u32 {
    14
}

fn default_option_as_meta() -> bool {
    true
}

/// Shell program and terminal font.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSettings {
    /// Empty: the login shell (`pty::shell`).
    #[serde(default)]
    pub shell: String,
    #[serde(default = "default_font_size")]
    pub font_size: u32,
    /// macOS: the Option key acts as Meta, so Option+P, Option+B and the
    /// like reach Claude Code instead of typing a symbol.
    #[serde(default = "default_option_as_meta")]
    pub option_as_meta: bool,
    /// Copy a finished selection to the clipboard. Off by default.
    #[serde(default)]
    pub copy_on_select: bool,
    /// Offer to compact a big paste (above 200 lines or 16 KB). On by default.
    #[serde(default = "default_true")]
    pub compact_paste: bool,
    /// What a Cmd+click on a file path opens it in: `finder` (default), `vscode`, `cursor`, `zed` or
    /// `sublime`. A fixed list: there is no free-form command.
    #[serde(default = "default_file_link_editor")]
    pub file_link_editor: String,
}

pub const FILE_LINK_EDITORS: &[&str] = &["finder", "vscode", "cursor", "zed", "sublime"];

fn default_file_link_editor() -> String {
    "finder".into()
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            shell: String::new(),
            font_size: default_font_size(),
            option_as_meta: default_option_as_meta(),
            copy_on_select: false,
            compact_paste: true,
            file_link_editor: default_file_link_editor(),
        }
    }
}

const MAX_CLAUDE_ACCOUNTS: usize = 8;
const MAX_ACCOUNT_NAME_CHARS: usize = 40;

/// One named Claude login. `config_dir` is that account's folder
/// (`~/.claude-account2`, `~/.claude`, …). DCTerminalv2 never creates it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeAccount {
    pub id: String,
    pub name: String,
    /// Empty: `~/.claude`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_dir: Option<String>,
    /// What AgentFlight may read and show from this account's session logs.
    /// An old settings file has none: `metadata`.
    #[serde(default)]
    pub capture_policy: crate::flight::schema::CapturePolicy,
    /// "Keep run traces": a finished run of this account gets `<run>/flight/`, a snapshot of its
    /// session events that outlives Claude's transcript cleanup. Off by default; an old file loads off.
    #[serde(default)]
    pub keep_run_traces: bool,
    /// The model of this account's Eagle-Eye orchestrator tabs; None: the run-wide setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator_model: Option<String>,
}

/// Claude Code: where `claude` is (empty: looked up) and the accounts.
/// `DCTV2_CLAUDE_CONFIG_DIR` overrides the first account's folder only, and
/// is not written here.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_path: Option<String>,
    /// Never empty after load: one `default` account is added.
    #[serde(default)]
    pub accounts: Vec<ClaudeAccount>,
}

fn valid_account_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A hand-edited file must not stop startup: drop only the bad accounts,
/// keeping every valid one and its config folder.
fn drop_invalid_claude_accounts(claude: &mut ClaudeSettings) {
    let mut seen = std::collections::HashSet::new();
    claude.accounts.retain(|account| {
        let id = account.id.trim();
        valid_account_id(id)
            && !account.name.trim().is_empty()
            && clean_optional_path(account.config_dir.clone(), "").is_ok()
            && seen.insert(id.to_string())
    });
    claude.accounts.truncate(MAX_CLAUDE_ACCOUNTS);
}

/// Check and tidy the accounts (ids, names, folders), and add the
/// `default` account when there is none.
pub fn normalize_claude_accounts(claude: &mut ClaudeSettings) -> Result<(), String> {
    claude.claude_path = clean_optional_path(claude.claude_path.take(), "claude path")?;
    if claude.accounts.len() > MAX_CLAUDE_ACCOUNTS {
        return Err(format!("at most {MAX_CLAUDE_ACCOUNTS} Claude accounts"));
    }
    let mut seen = std::collections::HashSet::new();
    for account in &mut claude.accounts {
        account.id = account.id.trim().to_string();
        if !valid_account_id(&account.id) {
            return Err("account id is invalid".into());
        }
        if !seen.insert(account.id.clone()) {
            return Err(format!("duplicate account id {}", account.id));
        }
        let name = account.name.trim();
        if name.is_empty() {
            return Err("account name is empty".into());
        }
        account.name = name.chars().take(MAX_ACCOUNT_NAME_CHARS).collect();
        account.config_dir =
            clean_optional_path(account.config_dir.take(), "Claude config folder")?;
    }
    if claude.accounts.is_empty() {
        claude.accounts.push(ClaudeAccount {
            id: DEFAULT_ACCOUNT_ID.to_string(),
            name: "Claude".to_string(),
            config_dir: None,
            capture_policy: Default::default(),
            keep_run_traces: false,
            orchestrator_model: None,
        });
    }
    Ok(())
}

/// Invalid stored orchestrator values become "no flag": an account override to `None`, the run-wide
/// model and mode to "". Never an error.
fn coerce_invalid_orchestrator(claude: &mut ClaudeSettings, run: &mut RunSettings) {
    for account in &mut claude.accounts {
        if account
            .orchestrator_model
            .as_deref()
            .is_some_and(|m| !valid_orchestrator_model(m))
        {
            account.orchestrator_model = None;
        }
    }
    if !valid_orchestrator_model(&run.orchestrator_model) {
        run.orchestrator_model.clear();
    }
    if !valid_orchestrator_mode(&run.orchestrator_permission_mode) {
        run.orchestrator_permission_mode.clear();
    }
}

/// Shown instead of a terminal when a window's account is no longer in
/// Settings: nothing starts under another account in its place.
pub const ACCOUNT_REMOVED: &str = "This window's account was removed. Pick an account.";

/// Whether `account_id` is one of the accounts in Settings.
pub fn account_exists(claude: &ClaudeSettings, account_id: &str) -> bool {
    claude
        .accounts
        .iter()
        .any(|account| account.id == account_id)
}

/// The folder of an account that is in Settings (`None` once it was
/// removed). Anything that starts `claude` or saves a session goes through
/// this, never through the first-account fallback of [`account_config`].
pub fn known_account_config(claude: &ClaudeSettings, account_id: &str) -> Option<ConfigDirInfo> {
    account_exists(claude, account_id).then(|| resolved_account_config(claude, account_id))
}

/// Which folder an account uses. An unknown id is the first account (for
/// display only; launches use [`known_account_config`]).
/// `env_override` applies only to the first account.
pub fn account_config(
    claude: &ClaudeSettings,
    account_id: &str,
    env_override: Option<&str>,
    home: Option<&Path>,
) -> ConfigDirInfo {
    let index = claude
        .accounts
        .iter()
        .position(|account| account.id == account_id)
        .unwrap_or(0);
    let setting = claude
        .accounts
        .get(index)
        .and_then(|account| account.config_dir.as_deref());
    let env = if index == 0 { env_override } else { None };
    crate::provider::claude_config::resolve_with(env, setting, home)
}

/// Account folder for this process (`DCTV2_CLAUDE_CONFIG_DIR` on the first
/// account only).
pub fn resolved_account_config(claude: &ClaudeSettings, account_id: &str) -> ConfigDirInfo {
    let env = std::env::var(crate::provider::claude_config::CONFIG_DIR_ENV).ok();
    account_config(
        claude,
        account_id,
        env.as_deref(),
        crate::provider::claude_config::home_dir().as_deref(),
    )
}

/// The account's name. An unknown id (a window whose account was removed)
/// says so: that window starts no Claude tab until an account is picked.
pub fn account_display_name(claude: &ClaudeSettings, account_id: &str) -> String {
    match claude
        .accounts
        .iter()
        .find(|account| account.id == account_id)
    {
        Some(account) if !account.name.trim().is_empty() => account.name.clone(),
        Some(_) => "Claude".to_string(),
        None => "Account removed".to_string(),
    }
}

const MAX_SETTING_PATH_CHARS: usize = 1024;

/// Trimmed path, `None` when empty. Rejects newlines and NUL.
fn clean_optional_path(value: Option<String>, what: &str) -> Result<Option<String>, String> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.contains(['\n', '\r', '\0']) {
        return Err(format!("{what} cannot contain a newline"));
    }
    if trimmed.chars().count() > MAX_SETTING_PATH_CHARS {
        return Err(format!("{what} is too long"));
    }
    Ok(Some(trimmed.to_string()))
}

/// Look and feel: the colour theme.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UiSettings {
    #[serde(default = "default_theme")]
    pub theme: String,
    /// "Open in editor" on a PR card: a program and its arguments, run
    /// without a shell with the worktree last. Empty: `code`.
    #[serde(default)]
    pub editor_command: String,
}

/// The editor when the setting is empty.
pub const DEFAULT_EDITOR: &str = "code";

pub const UI_THEMES: &[&str] = &[
    "github-dark",
    "github-light",
    "dracula",
    "nord",
    "tokyo-night",
    "catppuccin-mocha",
    "system",
];

fn default_theme() -> String {
    UI_THEMES[0].to_string()
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            editor_command: String::new(),
        }
    }
}

/// Activity tracking (docs/launch.md, "Which tabs get hooks").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ObserveSettings {
    /// "Track activity in every Claude tab": the hook forwarder is attached
    /// to plain Claude tabs too, not only to tabs a run launched.
    #[serde(default = "default_true")]
    pub hooks_all_tabs: bool,
    /// "Eval mode": a finished run can be repeated N times to compare outcomes (it spends usage on
    /// purpose). Off by default; an old file loads off.
    #[serde(default)]
    pub eval_enabled: bool,
    /// Where a Trace Export is sent by default (AgentFlight's OTLP export; Phoenix's local port).
    #[serde(default = "default_export_endpoint")]
    pub export_endpoint: String,
}

/// Phoenix's local OTLP/HTTP traces endpoint.
pub const DEFAULT_EXPORT_ENDPOINT: &str = "http://127.0.0.1:6006/v1/traces";

fn default_export_endpoint() -> String {
    DEFAULT_EXPORT_ENDPOINT.to_string()
}

impl Default for ObserveSettings {
    fn default() -> Self {
        Self {
            hooks_all_tabs: true,
            eval_enabled: false,
            export_endpoint: default_export_endpoint(),
        }
    }
}

/// An export endpoint: only `http://` and `https://` URLs with a host are kept. Returns it trimmed;
/// the error says why a value is refused.
pub fn check_export_endpoint(text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Enter an http:// or https:// address.".into());
    }
    let uri = text
        .parse::<ureq::http::Uri>()
        .map_err(|_| "That is not a valid address.".to_string())?;
    match uri.scheme_str() {
        Some("http") | Some("https") => {}
        _ => return Err("Only http:// and https:// addresses are used.".into()),
    }
    if uri.host().is_none_or(|h| h.is_empty()) {
        return Err("The address has no host.".into());
    }
    Ok(text.to_string())
}

/// Notifications (docs/launch.md, "Attention"): a master switch and one
/// switch per event kind. Missing: all on.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NotifySettings {
    #[serde(default = "default_true")]
    pub master: bool,
    /// Event kind (`needs-you`, `stall`, …) → on. A kind not listed is on.
    #[serde(default)]
    pub events: std::collections::BTreeMap<String, bool>,
}

impl Default for NotifySettings {
    fn default() -> Self {
        Self {
            master: true,
            events: Default::default(),
        }
    }
}

impl NotifySettings {
    /// The master switch and the event's own switch are both on.
    pub fn enabled(&self, kind: &str) -> bool {
        self.master && self.events.get(kind).copied().unwrap_or(true)
    }
}

/// Where phone pushes go (`observe/push.rs`). The ntfy topic, the Pushover
/// token and user key and the webhook URL are secrets: they live in the
/// keychain, never here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PushSettings {
    /// `none`, `ntfy`, `pushover` or `webhook`.
    #[serde(default = "default_push_target")]
    pub target: String,
    /// The ntfy server (the topic is the secret).
    #[serde(default = "default_ntfy_server")]
    pub ntfy_server: String,
    /// App-only event kinds → on (missing: on): stall, check-failed,
    /// ci-failed, merged-cleaned, autopost-failed.
    #[serde(default)]
    pub events: std::collections::BTreeMap<String, bool>,
}

fn default_push_target() -> String {
    "none".to_string()
}

fn default_ntfy_server() -> String {
    "https://ntfy.sh".to_string()
}

impl Default for PushSettings {
    fn default() -> Self {
        Self {
            target: default_push_target(),
            ntfy_server: default_ntfy_server(),
            events: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SettingsFile {
    pub schema_version: u32,
    #[serde(default)]
    pub terminal: TerminalSettings,
    #[serde(default)]
    pub claude: ClaudeSettings,
    #[serde(default)]
    pub ui: UiSettings,
    #[serde(default)]
    pub run: RunSettings,
    #[serde(default)]
    pub observe: ObserveSettings,
    #[serde(default)]
    pub notify: NotifySettings,
    #[serde(default)]
    pub push: PushSettings,
    /// ClickUp: per-account switches (auto-post after ship approval).
    #[serde(default)]
    pub clickup: ClickUpSettings,
    /// Worktree setup per repo, keyed by the repo's real path (repos.json
    /// `setup` / `copyFiles`). Missing: auto-detect, copy nothing.
    #[serde(default)]
    pub worktree_setup: std::collections::BTreeMap<String, RepoSetup>,
}

/// Per-account ClickUp settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClickUpSettings {
    /// "Post ClickUp comments after ship approval, after one click in the
    /// run panel", per account id. Missing: off (a stored `true` is kept).
    #[serde(default)]
    pub auto_post: std::collections::BTreeMap<String, bool>,
    /// The status a ticket moves to when its PRs merge, per ClickUp list id.
    /// Missing: the list's first `closed`-type status. No editor yet.
    #[serde(default)]
    pub done_status: std::collections::BTreeMap<String, String>,
}

impl ClickUpSettings {
    pub fn auto_post_for(&self, account_id: &str) -> bool {
        self.auto_post.get(account_id).copied().unwrap_or(false)
    }
}

/// One repo's worktree setup (repos.json `setup` and `copyFiles`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepoSetup {
    /// The setup command; `None`: the plugin detects it from the lockfile.
    #[serde(default)]
    pub setup: Option<String>,
    /// Git-ignored files copied from the main checkout, relative paths.
    #[serde(default)]
    pub copy_files: Vec<String>,
    /// "Allow live Claude tests in checks" (repos.json `liveTests`): the
    /// Implementer and PR Reviewer may run scripts/skill-e2e/golden.sh.
    #[serde(default)]
    pub live_tests: bool,
    /// "Receipts in PR bodies" (repos.json `receipt`): Eagle-Eye adds a
    /// collapsed Receipt to this repo's PR bodies. Off by default.
    #[serde(default)]
    pub receipt: bool,
    /// "Try PRs in the app (QA)" (repos.json `qa`): off when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qa: Option<QaSetup>,
}

/// The command that serves a PR's build for QA and the loopback URL it answers on. `%PORT%` in either is the
/// free port `state.sh qa-serve` chose (docs/run-folder.md, QA).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QaSetup {
    pub command: String,
    pub url: String,
}

/// Whether `url` is `http://` with a loopback host (`127.0.0.1`, `localhost`, `[::1]`), by parsing the authority
/// (a prefix match would accept `http://localhost.evil.com`). The port may be `%PORT%`.
pub fn loopback_http_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') || authority.contains(char::is_whitespace) {
        return false;
    }
    let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
        let Some((host, tail)) = inner.split_once(']') else {
            return false;
        };
        let port = if tail.is_empty() {
            ""
        } else if let Some(port) = tail.strip_prefix(':') {
            port
        } else {
            return false;
        };
        (format!("[{host}]"), port)
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host.to_string(), port),
            None => (authority.to_string(), ""),
        }
    };
    let port_ok = port.is_empty()
        || port == "%PORT%"
        || (port.len() <= 5 && port.chars().all(|c| c.is_ascii_digit()));
    port_ok
        && matches!(
            host.to_ascii_lowercase().as_str(),
            "127.0.0.1" | "localhost" | "[::1]"
        )
}

pub const MAX_COPY_FILES: usize = 20;
const MAX_SETUP_CHARS: usize = 400;

/// A relative path inside the repo: no `..`, not absolute, no control
/// characters, no `.git` component anywhere.
pub fn valid_copy_path(path: &str) -> bool {
    let p = std::path::Path::new(path);
    !path.is_empty()
        && path.len() <= 300
        && !path.chars().any(char::is_control)
        && !p.is_absolute()
        && !path.starts_with('~')
        && p.components().next().is_some()
        && p.components()
            .all(|c| matches!(c, std::path::Component::Normal(n) if n != ".git"))
}

/// Trimmed and checked; an empty setup and no files is the default.
pub fn clean_repo_setup(next: RepoSetup) -> Result<RepoSetup, String> {
    let setup = next
        .setup
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(cmd) = &setup {
        if cmd.contains(['\n', '\r', '\0']) || cmd.chars().count() > MAX_SETUP_CHARS {
            return Err("The setup command must be one line (at most 400 characters).".into());
        }
    }
    let mut files: Vec<String> = Vec::new();
    for raw in next.copy_files {
        let path = raw.trim().trim_start_matches("./").to_string();
        if path.is_empty() {
            continue;
        }
        if !valid_copy_path(&path) {
            return Err(format!(
                "{path}: a file to copy must be a relative path inside the repo."
            ));
        }
        if !files.contains(&path) {
            files.push(path);
        }
    }
    if files.len() > MAX_COPY_FILES {
        return Err(format!("At most {MAX_COPY_FILES} files to copy."));
    }
    let qa = match next.qa {
        None => None,
        Some(qa) => {
            let command = qa.command.trim().to_string();
            let url = qa.url.trim().to_string();
            if command.is_empty()
                || command.contains(['\n', '\r', '\0'])
                || command.chars().count() > MAX_SETUP_CHARS
            {
                return Err(
                    "QA command: enter one line (at most 400 characters), for example npm run dev:fake."
                        .into(),
                );
            }
            if !loopback_http_url(&url) {
                return Err(
                    "QA URL: use http:// with 127.0.0.1, localhost or [::1], for example http://127.0.0.1:%PORT%/."
                        .into(),
                );
            }
            Some(QaSetup { command, url })
        }
    };
    Ok(RepoSetup {
        setup,
        copy_files: files,
        live_tests: next.live_tests,
        receipt: next.receipt,
        qa,
    })
}

/// Eagle-Eye launch settings (docs/launch.md).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunSettings {
    /// `DCV2_MAX_IMPLEMENTERS` (R6), 1..=8.
    #[serde(default = "default_max_implementers")]
    pub max_implementers: u32,
    /// `DCV2_STOP_HOOKS` (A3 kill switch).
    #[serde(default = "default_true")]
    pub stop_hooks: bool,
    /// `DCV2_SLOP_CHECK`: the Stop hook also reports slop in the lines a PR added.
    #[serde(default = "default_true")]
    pub slop_check: bool,
    /// R2 watchdog: minutes with no progress before the panel says so; 0 is off.
    #[serde(default = "default_watchdog_minutes")]
    pub watchdog_minutes: u32,
    /// How many runs may be live at once (dispatcher slots), 1..=8.
    #[serde(default = "default_max_active_runs")]
    pub max_active_runs: u32,
    /// Hold queued starts while the entry's account is at or past this percent of its five-hour usage (None: off).
    #[serde(default)]
    pub usage_hold_percent: Option<u32>,
    /// Phone pushes for the batch and wave milestones (`batch-planned`, `batch-ready`, `wave-ready`); the
    /// desktop notification for each is always raised.
    #[serde(default = "default_true")]
    pub batch_pushes: bool,
    /// A ready wave starts on its own (through the cap) instead of waiting for "Start wave".
    #[serde(default)]
    pub auto_start_waves: bool,
    /// "Clean up merged runs automatically" (plan 2026-10-27): finished
    /// runs' merged PRs lose their worktree, local and remote branch.
    #[serde(default = "default_true")]
    pub auto_cleanup: bool,
    /// `--model` of an Eagle-Eye orchestrator tab: `opus` by default, empty: the account's own model.
    #[serde(default = "default_orchestrator_model")]
    pub orchestrator_model: String,
    /// `--permission-mode` of an Eagle-Eye orchestrator tab: `auto` by default, empty: the account's own mode.
    #[serde(default = "default_orchestrator_permission_mode")]
    pub orchestrator_permission_mode: String,
}

fn default_orchestrator_model() -> String {
    "opus".to_string()
}

fn default_orchestrator_permission_mode() -> String {
    "auto".to_string()
}

/// Message for a refused orchestrator model.
pub const ORCHESTRATOR_MODEL_REFUSED: &str =
    "Not a model id: use opus, sonnet, haiku or a claude-… id.";
/// Message for a refused orchestrator permission mode.
pub const ORCHESTRATOR_MODE_REFUSED: &str =
    "Not a permission mode: use auto, acceptEdits or default.";

/// An orchestrator model: empty (no flag), an alias (`opus`, `sonnet`, `haiku`) or a `claude-…` id,
/// each with an optional `[1m]` suffix.
pub fn valid_orchestrator_model(model: &str) -> bool {
    if model.is_empty() {
        return true;
    }
    let base = model.strip_suffix("[1m]").unwrap_or(model);
    if ["opus", "sonnet", "haiku"].contains(&base) {
        return true;
    }
    base.len() > "claude-".len()
        && base.len() <= 80
        && base.starts_with("claude-")
        && base
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-')
}

/// An orchestrator permission mode: empty (no flag), `auto`, `acceptEdits` or `default`. Never
/// `bypassPermissions`, `dontAsk` or `plan`.
pub fn valid_orchestrator_mode(mode: &str) -> bool {
    mode.is_empty() || ["auto", "acceptEdits", "default"].contains(&mode)
}

/// The `--model` and `--permission-mode` values of an orchestrator tab of `account_id`: the account's
/// override when set, else the run-wide value. Empty or invalid stored values give `None` (no flag).
pub fn orchestrator_for(
    settings: &SettingsFile,
    account_id: &str,
) -> (Option<String>, Option<String>) {
    let own = settings
        .claude
        .accounts
        .iter()
        .find(|a| a.id == account_id)
        .and_then(|a| a.orchestrator_model.as_deref())
        .filter(|m| !m.is_empty() && valid_orchestrator_model(m));
    let model = own
        .or(Some(settings.run.orchestrator_model.as_str()))
        .filter(|m| !m.is_empty() && valid_orchestrator_model(m))
        .map(str::to_string);
    let mode = Some(settings.run.orchestrator_permission_mode.as_str())
        .filter(|m| !m.is_empty() && valid_orchestrator_mode(m))
        .map(str::to_string);
    (model, mode)
}

pub const WATCHDOG_MINUTES_LIMIT: u32 = 240;

fn default_max_active_runs() -> u32 {
    2
}

fn default_watchdog_minutes() -> u32 {
    crate::observe::watchdog::DEFAULT_MINUTES
}

pub const MAX_IMPLEMENTERS_LIMIT: u32 = 8;

fn default_max_implementers() -> u32 {
    3
}

fn default_true() -> bool {
    true
}

impl Default for RunSettings {
    fn default() -> Self {
        Self {
            max_implementers: default_max_implementers(),
            stop_hooks: true,
            slop_check: true,
            watchdog_minutes: default_watchdog_minutes(),
            max_active_runs: default_max_active_runs(),
            usage_hold_percent: None,
            batch_pushes: true,
            auto_start_waves: false,
            auto_cleanup: true,
            orchestrator_model: default_orchestrator_model(),
            orchestrator_permission_mode: default_orchestrator_permission_mode(),
        }
    }
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            terminal: TerminalSettings::default(),
            claude: ClaudeSettings::default(),
            ui: UiSettings::default(),
            run: RunSettings::default(),
            observe: ObserveSettings::default(),
            notify: NotifySettings::default(),
            push: PushSettings::default(),
            clickup: ClickUpSettings::default(),
            worktree_setup: Default::default(),
        }
    }
}

pub struct SettingsStore {
    pub path: PathBuf,
    pub data: SettingsFile,
}

impl SettingsStore {
    pub fn open(dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("settings dir: {e}"))?;
        let path = dir.join("settings.json");
        let mut data = read_json_or_recover::<SettingsFile>(&path)?;
        if data.schema_version > SETTINGS_SCHEMA_VERSION {
            // Moved aside, so no save here can overwrite a newer build's file.
            set_aside_other_version(&path, data.schema_version);
            data = SettingsFile::default();
        }
        data.schema_version = SETTINGS_SCHEMA_VERSION;
        if normalize_claude_accounts(&mut data.claude).is_err() {
            drop_invalid_claude_accounts(&mut data.claude);
            data.claude.claude_path = None;
            normalize_claude_accounts(&mut data.claude)?;
        }
        // A hand-edited orchestrator value must not stop startup or drop an account: no flag instead.
        coerce_invalid_orchestrator(&mut data.claude, &mut data.run);
        Ok(Self { path, data })
    }

    pub fn save(&self) -> Result<(), String> {
        write_json_atomic(&self.path, &self.data)
    }

    pub fn terminal(&self) -> &TerminalSettings {
        &self.data.terminal
    }

    pub fn set_terminal(&mut self, mut next: TerminalSettings) -> Result<(), String> {
        next.shell = next.shell.trim().chars().take(400).collect();
        if next.shell.contains(['\n', '\r']) {
            return Err("shell path cannot contain a newline".to_string());
        }
        next.font_size = next.font_size.clamp(8, 32);
        if !FILE_LINK_EDITORS.contains(&next.file_link_editor.as_str()) {
            next.file_link_editor = default_file_link_editor();
        }
        self.data.terminal = next;
        self.save()
    }

    pub fn claude(&self) -> &ClaudeSettings {
        &self.data.claude
    }

    /// Paths are trimmed; an empty path clears the setting.
    pub fn set_claude(&mut self, mut next: ClaudeSettings) -> Result<(), String> {
        normalize_claude_accounts(&mut next)?;
        // Only a changed override is refused; an unchanged bad stored one is coerced, so it never
        // blocks an unrelated save.
        for account in &mut next.accounts {
            let model = account
                .orchestrator_model
                .take()
                .map(|m| m.trim().to_string());
            let model = model.filter(|m| !m.is_empty());
            let stored = self
                .data
                .claude
                .accounts
                .iter()
                .find(|a| a.id == account.id)
                .and_then(|a| a.orchestrator_model.clone());
            account.orchestrator_model = match model {
                Some(m) if valid_orchestrator_model(&m) => Some(m),
                Some(m) if stored.as_deref() == Some(m.as_str()) => None,
                Some(_) => return Err(ORCHESTRATOR_MODEL_REFUSED.to_string()),
                None => None,
            };
        }
        self.data.claude = next;
        self.save()
    }

    pub fn ui(&self) -> &UiSettings {
        &self.data.ui
    }

    pub fn run(&self) -> &RunSettings {
        &self.data.run
    }

    /// The implementer cap is kept within 1..=8.
    pub fn set_run(&mut self, next: RunSettings) -> Result<(), String> {
        // Only a changed value is refused; an unchanged bad stored one is coerced to "" (no flag).
        let model = next.orchestrator_model.trim().to_string();
        let model = if valid_orchestrator_model(&model) {
            model
        } else if model == self.data.run.orchestrator_model {
            String::new()
        } else {
            return Err(ORCHESTRATOR_MODEL_REFUSED.to_string());
        };
        let mode = next.orchestrator_permission_mode.trim().to_string();
        let mode = if valid_orchestrator_mode(&mode) {
            mode
        } else if mode == self.data.run.orchestrator_permission_mode {
            String::new()
        } else {
            return Err(ORCHESTRATOR_MODE_REFUSED.to_string());
        };
        self.data.run = RunSettings {
            max_implementers: next.max_implementers.clamp(1, MAX_IMPLEMENTERS_LIMIT),
            stop_hooks: next.stop_hooks,
            slop_check: next.slop_check,
            watchdog_minutes: next.watchdog_minutes.min(WATCHDOG_MINUTES_LIMIT),
            max_active_runs: next.max_active_runs.clamp(1, MAX_IMPLEMENTERS_LIMIT),
            usage_hold_percent: next.usage_hold_percent.map(|p| p.clamp(1, 100)),
            batch_pushes: next.batch_pushes,
            auto_start_waves: next.auto_start_waves,
            auto_cleanup: next.auto_cleanup,
            orchestrator_model: model,
            orchestrator_permission_mode: mode,
        };
        self.save()
    }

    pub fn observe(&self) -> &ObserveSettings {
        &self.data.observe
    }

    pub fn set_observe(&mut self, mut next: ObserveSettings) -> Result<(), String> {
        // An invalid endpoint is refused and the old settings stay.
        next.export_endpoint = check_export_endpoint(&next.export_endpoint)?;
        self.data.observe = next;
        self.save()
    }

    pub fn notify(&self) -> &NotifySettings {
        &self.data.notify
    }

    /// Only known event kinds are kept.
    pub fn set_notify(&mut self, mut next: NotifySettings) -> Result<(), String> {
        next.events
            .retain(|kind, _| crate::observe::attention::Kind::from_key(kind).is_some());
        self.data.notify = next;
        self.save()
    }

    pub fn push(&self) -> &PushSettings {
        &self.data.push
    }

    /// The target must be known, the ntfy server a clean URL, and only the
    /// four app-only kinds are kept.
    pub fn set_push(&mut self, mut next: PushSettings) -> Result<(), String> {
        if !["none", "ntfy", "pushover", "webhook"].contains(&next.target.as_str()) {
            return Err("unknown push target".into());
        }
        next.ntfy_server = next.ntfy_server.trim().trim_end_matches('/').to_string();
        if next.ntfy_server.is_empty() {
            next.ntfy_server = default_ntfy_server();
        }
        crate::observe::push::check_url(&next.ntfy_server)?;
        next.events
            .retain(|kind, _| crate::observe::push::PUSH_KINDS.contains(&kind.as_str()));
        self.data.push = next;
        self.save()
    }

    pub fn clickup(&self) -> &ClickUpSettings {
        &self.data.clickup
    }

    pub fn set_auto_post(&mut self, account_id: &str, on: bool) -> Result<(), String> {
        if !account_exists(&self.data.claude, account_id) {
            return Err("No Claude account with that id.".into());
        }
        self.data
            .clickup
            .auto_post
            .insert(account_id.to_string(), on);
        self.save()
    }

    /// The setup of the repo at `repo` (its real path), if any.
    #[cfg(test)]
    pub fn repo_setup(&self, repo: &str) -> Option<&RepoSetup> {
        self.data.worktree_setup.get(repo)
    }

    /// Saves (or, when empty, forgets) the setup of the repo at `repo`.
    pub fn set_repo_setup(&mut self, repo: &str, next: RepoSetup) -> Result<RepoSetup, String> {
        if repo.trim().is_empty() || repo.chars().any(char::is_control) {
            return Err("not a repo path".into());
        }
        let clean = clean_repo_setup(next)?;
        if clean == RepoSetup::default() {
            self.data.worktree_setup.remove(repo);
        } else {
            self.data
                .worktree_setup
                .insert(repo.to_string(), clean.clone());
        }
        self.save()?;
        Ok(clean)
    }

    /// Unknown themes fall back to GitHub Dark.
    pub fn set_ui(&mut self, next: UiSettings) -> Result<(), String> {
        let theme = if UI_THEMES.contains(&next.theme.as_str()) {
            next.theme
        } else {
            default_theme()
        };
        let editor_command = next.editor_command.trim().to_string();
        if editor_command.chars().count() > 300 || editor_command.contains(['\n', '\r', '\0']) {
            return Err("The editor command must be one line (at most 300 characters).".into());
        }
        self.data.ui = UiSettings {
            theme,
            editor_command,
        };
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn an_old_settings_file_loads_with_hooks_on_and_the_same_schema() {
        let dir = TempDir::new("settings_observe_default");
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"schemaVersion":1,"terminal":{"fontSize":15}}"#,
        )
        .unwrap();
        let mut store = SettingsStore::open(dir.path()).unwrap();
        assert!(store.observe().hooks_all_tabs);
        assert_eq!(store.data.schema_version, SETTINGS_SCHEMA_VERSION);
        store
            .set_observe(ObserveSettings {
                hooks_all_tabs: false,
                ..Default::default()
            })
            .unwrap();
        let again = SettingsStore::open(dir.path()).unwrap();
        assert!(!again.observe().hooks_all_tabs);
    }

    #[test]
    fn the_export_endpoint_defaults_for_old_files_and_an_invalid_one_keeps_the_old_value() {
        let dir = TempDir::new("settings_export_endpoint");
        std::fs::write(
            dir.join("settings.json"),
            r#"{"observe":{"hooksAllTabs":true,"evalEnabled":false}}"#,
        )
        .unwrap();
        let mut store = SettingsStore::open(dir.path()).unwrap();
        assert_eq!(
            store.observe().export_endpoint,
            "http://127.0.0.1:6006/v1/traces"
        );
        store
            .set_observe(ObserveSettings {
                export_endpoint: " http://localhost:4318/v1/traces ".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            store.observe().export_endpoint,
            "http://localhost:4318/v1/traces"
        );
        for bad in ["ftp://example.com", "http://", "not a url", ""] {
            let err = store
                .set_observe(ObserveSettings {
                    export_endpoint: bad.into(),
                    ..Default::default()
                })
                .unwrap_err();
            assert!(!err.is_empty());
            assert_eq!(
                store.observe().export_endpoint,
                "http://localhost:4318/v1/traces",
                "{bad}"
            );
        }
        let again = SettingsStore::open(dir.path()).unwrap();
        assert_eq!(
            again.observe().export_endpoint,
            "http://localhost:4318/v1/traces"
        );
    }

    #[test]
    fn the_editor_command_defaults_empty_and_is_kept_with_the_theme() {
        let dir = TempDir::new("settings_editor");
        let mut store = SettingsStore::open(dir.path()).unwrap();
        assert_eq!(store.ui().editor_command, "");
        store
            .set_ui(UiSettings {
                theme: "nord".into(),
                editor_command: "  cursor --new-window ".into(),
            })
            .unwrap();
        assert_eq!(store.ui().editor_command, "cursor --new-window");
        assert!(store
            .set_ui(UiSettings {
                theme: "nord".into(),
                editor_command: "code\nrm -rf".into(),
            })
            .is_err());
        let again = SettingsStore::open(dir.path()).unwrap();
        assert_eq!(again.ui().editor_command, "cursor --new-window");
    }

    #[test]
    fn notifications_default_on_and_an_old_file_keeps_its_schema() {
        let dir = TempDir::new("settings_notify_default");
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"schemaVersion":1,"terminal":{"fontSize":15}}"#,
        )
        .unwrap();
        let mut store = SettingsStore::open(dir.path()).unwrap();
        assert_eq!(store.data.schema_version, SETTINGS_SCHEMA_VERSION);
        assert!(store.notify().master);
        assert!(store.notify().enabled("needs-you"));
        let mut next = store.notify().clone();
        next.events.insert("stall".into(), false);
        next.events.insert("made-up".into(), false);
        store.set_notify(next).unwrap();
        assert!(!store.notify().enabled("stall"));
        assert!(store.notify().enabled("needs-you"));
        assert!(!store.notify().events.contains_key("made-up"));
        let mut off = store.notify().clone();
        off.master = false;
        store.set_notify(off).unwrap();
        assert!(
            !store.notify().enabled("needs-you"),
            "the master switch wins"
        );
    }

    #[test]
    fn an_old_repo_setup_without_receipt_loads_as_off() {
        let old: RepoSetup =
            serde_json::from_str(r#"{"setup":"npm ci","copyFiles":[],"liveTests":true}"#).unwrap();
        assert!(!old.receipt);
        assert!(old.live_tests);
    }

    #[test]
    fn an_old_repo_setup_without_qa_loads_as_off() {
        let old: RepoSetup =
            serde_json::from_str(r#"{"setup":"npm ci","copyFiles":[],"receipt":true}"#).unwrap();
        assert_eq!(old.qa, None);
        let text = serde_json::to_string(&old).unwrap();
        assert!(!text.contains("qa"), "off writes no qa key: {text}");
    }

    #[test]
    fn a_qa_setting_needs_a_command_and_a_loopback_http_url() {
        let with = |command: &str, url: &str| RepoSetup {
            qa: Some(QaSetup {
                command: command.into(),
                url: url.into(),
            }),
            ..Default::default()
        };
        let ok =
            clean_repo_setup(with(" npm run dev:fake ", " http://127.0.0.1:%PORT%/ ")).unwrap();
        assert_eq!(ok.qa.unwrap().command, "npm run dev:fake");
        for url in [
            "http://localhost:3000/",
            "http://[::1]:%PORT%/x",
            "http://127.0.0.1/",
            "http://LOCALHOST:8080",
        ] {
            assert!(clean_repo_setup(with("npm run dev", url)).is_ok(), "{url}");
        }
        for url in [
            "http://localhost.evil.com/",
            "http://localhost.evil.com:3000/",
            "http://example.com/",
            "https://127.0.0.1:3000/",
            "http://127.0.0.1@evil.com/",
            "http://evil.com/@127.0.0.1",
            "http://127.0.0.1:abc/",
            "127.0.0.1:3000",
            "",
        ] {
            let err = clean_repo_setup(with("npm run dev", url)).unwrap_err();
            assert!(err.contains("QA URL"), "{url}: {err}");
        }
        let err = clean_repo_setup(with("  ", "http://127.0.0.1:%PORT%/")).unwrap_err();
        assert!(err.contains("QA command"), "{err}");
    }

    #[test]
    fn a_refused_qa_save_keeps_the_previous_values() {
        let dir = TempDir::new("settings_qa");
        let mut store = SettingsStore::open(dir.path()).unwrap();
        let good = RepoSetup {
            qa: Some(QaSetup {
                command: "npm run dev:fake".into(),
                url: "http://127.0.0.1:%PORT%/".into(),
            }),
            ..Default::default()
        };
        let saved = store.set_repo_setup("/w/a", good).unwrap();
        let bad = RepoSetup {
            qa: Some(QaSetup {
                command: "npm run dev:fake".into(),
                url: "http://localhost.evil.com/".into(),
            }),
            ..Default::default()
        };
        assert!(store.set_repo_setup("/w/a", bad).is_err());
        assert_eq!(store.repo_setup("/w/a"), Some(&saved));
    }

    #[test]
    fn worktree_setup_is_checked_and_auto_post_defaults_off() {
        let dir = TempDir::new("settings_setup");
        let mut store = SettingsStore::open(dir.path()).unwrap();
        assert!(!store.clickup().auto_post_for(DEFAULT_ACCOUNT_ID));
        store.set_auto_post(DEFAULT_ACCOUNT_ID, true).unwrap();
        assert!(store.clickup().auto_post_for(DEFAULT_ACCOUNT_ID));
        store.set_auto_post(DEFAULT_ACCOUNT_ID, false).unwrap();
        assert!(!store.clickup().auto_post_for(DEFAULT_ACCOUNT_ID));
        assert!(store.set_auto_post("nope", false).is_err());
        for bad in [
            "/etc/passwd",
            "../x",
            "a/../../b",
            ".git/config",
            "sub/.git/hooks/x",
            "~/x",
            "a\nb",
        ] {
            let next = RepoSetup {
                setup: None,
                copy_files: vec![bad.into()],
                live_tests: false,
                receipt: false,
                qa: None,
            };
            assert!(store.set_repo_setup("/w/a", next).is_err(), "{bad}");
        }
        let saved = store
            .set_repo_setup(
                "/w/a",
                RepoSetup {
                    setup: Some("  pnpm i  ".into()),
                    copy_files: vec![
                        "./.env".into(),
                        ".env".into(),
                        " ".into(),
                        "cfg/.env.local".into(),
                    ],
                    live_tests: true,
                    receipt: true,
                    qa: None,
                },
            )
            .unwrap();
        assert_eq!(saved.setup.as_deref(), Some("pnpm i"));
        assert_eq!(saved.copy_files, [".env", "cfg/.env.local"]);
        assert!(saved.live_tests);
        assert!(saved.receipt);
        assert!(store
            .set_repo_setup(
                "/w/a",
                RepoSetup {
                    setup: Some("a\nb".into()),
                    copy_files: vec![],
                    live_tests: false,
                    receipt: false,
                    qa: None,
                }
            )
            .is_err());
        let again = SettingsStore::open(dir.path()).unwrap();
        assert_eq!(again.repo_setup("/w/a"), Some(&saved));
        assert!(!again.clickup().auto_post_for(DEFAULT_ACCOUNT_ID));
        let mut again = again;
        again.set_repo_setup("/w/a", RepoSetup::default()).unwrap();
        assert_eq!(again.repo_setup("/w/a"), None);
    }

    fn account(id: &str, name: &str, dir: Option<&str>) -> ClaudeAccount {
        ClaudeAccount {
            id: id.into(),
            name: name.into(),
            config_dir: dir.map(str::to_string),
            capture_policy: Default::default(),
            keep_run_traces: false,
            orchestrator_model: None,
        }
    }

    #[test]
    fn an_account_saved_before_capture_policy_existed_loads_as_metadata() {
        use crate::flight::schema::CapturePolicy;
        let old: ClaudeAccount =
            serde_json::from_str(r#"{"id":"work","name":"Work","configDir":"/c"}"#).unwrap();
        assert_eq!(old.capture_policy, CapturePolicy::Metadata);
        let set: ClaudeAccount =
            serde_json::from_str(r#"{"id":"work","name":"Work","capturePolicy":"off"}"#).unwrap();
        assert_eq!(set.capture_policy, CapturePolicy::Off);
        let json = serde_json::to_value(&set).unwrap();
        assert_eq!(json["capturePolicy"], "off");
        // The whole file, as the store loads it.
        let dir = TempDir::new("settings_old_policy");
        std::fs::write(
            dir.join("settings.json"),
            r#"{"claude":{"accounts":[{"id":"default","name":"Claude"}]}}"#,
        )
        .unwrap();
        let store = SettingsStore::open(dir.path()).unwrap();
        assert_eq!(
            store.claude().accounts[0].capture_policy,
            CapturePolicy::Metadata
        );
    }

    #[test]
    fn orchestrator_defaults_validation_and_resolution() {
        let dir = TempDir::new("settings_orchestrator");
        std::fs::write(
            dir.join("settings.json"),
            r#"{"schemaVersion":1,"claude":{"accounts":[{"id":"default","name":"Claude"},{"id":"b","name":"B"}]}}"#,
        )
        .unwrap();
        let mut store = SettingsStore::open(dir.path()).unwrap();
        assert_eq!(store.run().orchestrator_model, "opus");
        assert_eq!(store.run().orchestrator_permission_mode, "auto");
        assert_eq!(
            orchestrator_for(&store.data, "default"),
            (Some("opus".into()), Some("auto".into()))
        );

        for bad in ["gpt-4", "claude-", "Opus", "opus 1m"] {
            let mut run = store.run().clone();
            run.orchestrator_model = bad.into();
            assert!(store.set_run(run).is_err(), "{bad}");
        }
        for bad in ["bypassPermissions", "dontAsk", "plan"] {
            let mut run = store.run().clone();
            run.orchestrator_permission_mode = bad.into();
            assert!(store.set_run(run).is_err(), "{bad}");
        }
        assert_eq!(store.run().orchestrator_model, "opus", "kept");
        assert_eq!(store.run().orchestrator_permission_mode, "auto", "kept");

        let mut run = store.run().clone();
        run.orchestrator_model = "opus[1m]".into();
        store.set_run(run).unwrap();
        assert_eq!(store.run().orchestrator_model, "opus[1m]");

        let mut claude = store.claude().clone();
        claude.accounts[1].orchestrator_model = Some("claude-sonnet-4-5".into());
        store.set_claude(claude).unwrap();
        assert_eq!(
            orchestrator_for(&store.data, "b").0.as_deref(),
            Some("claude-sonnet-4-5")
        );
        assert_eq!(
            orchestrator_for(&store.data, "default").0.as_deref(),
            Some("opus[1m]")
        );
        let mut claude = store.claude().clone();
        claude.accounts[1].orchestrator_model = Some("gpt-4".into());
        assert!(store.set_claude(claude).is_err());
        assert_eq!(
            store.claude().accounts[1].orchestrator_model.as_deref(),
            Some("claude-sonnet-4-5")
        );

        let mut run = store.run().clone();
        run.orchestrator_model = String::new();
        run.orchestrator_permission_mode = String::new();
        store.set_run(run).unwrap();
        assert_eq!(orchestrator_for(&store.data, "default"), (None, None));
    }

    #[test]
    fn invalid_stored_orchestrator_values_never_fail_open_or_block_saves() {
        let dir = TempDir::new("settings_orchestrator_bad");
        std::fs::write(
            dir.join("settings.json"),
            r#"{"schemaVersion":1,"claude":{"accounts":[{"id":"default","name":"Claude","orchestratorModel":"gpt-4"}]},"run":{"orchestratorModel":"gpt-4","orchestratorPermissionMode":"bypassPermissions"}}"#,
        )
        .unwrap();
        let mut store = SettingsStore::open(dir.path()).unwrap();
        assert_eq!(store.claude().accounts.len(), 1);
        assert_eq!(orchestrator_for(&store.data, "default"), (None, None));

        // The raw stored values, as a UI that did not touch them would send them back.
        store.data.claude.accounts[0].orchestrator_model = Some("gpt-4".into());
        store.data.run.orchestrator_model = "gpt-4".into();
        let mut claude = store.claude().clone();
        claude.accounts[0].name = "Renamed".into();
        store.set_claude(claude).unwrap();
        assert_eq!(store.claude().accounts[0].name, "Renamed");
        assert_eq!(store.claude().accounts[0].orchestrator_model, None);
        store.data.run.orchestrator_model = "gpt-4".into();
        let mut run = store.run().clone();
        run.max_implementers = 5;
        store.set_run(run).unwrap();
        assert_eq!(store.run().max_implementers, 5);
        assert_eq!(store.run().orchestrator_model, "");
    }

    #[test]
    fn keep_run_traces_and_eval_mode_default_off_for_old_files_and_round_trip() {
        let dir = TempDir::new("settings_wave2_optins");
        std::fs::write(
            dir.join("settings.json"),
            r#"{"claude":{"accounts":[{"id":"default","name":"Claude"}]},"observe":{"hooksAllTabs":true}}"#,
        )
        .unwrap();
        let mut store = SettingsStore::open(dir.path()).unwrap();
        assert!(!store.claude().accounts[0].keep_run_traces);
        assert!(!store.observe().eval_enabled);
        assert!(
            store.observe().hooks_all_tabs,
            "the other setting is untouched"
        );

        let mut claude = store.claude().clone();
        claude.accounts[0].keep_run_traces = true;
        store.set_claude(claude).unwrap();
        store
            .set_observe(ObserveSettings {
                eval_enabled: true,
                ..Default::default()
            })
            .unwrap();
        let again = SettingsStore::open(dir.path()).unwrap();
        assert!(again.claude().accounts[0].keep_run_traces);
        assert!(again.observe().eval_enabled);
        let json = serde_json::to_value(&again.claude().accounts[0]).unwrap();
        assert_eq!(json["keepRunTraces"], true);
        assert!(!ObserveSettings::default().eval_enabled);
    }

    #[test]
    fn a_fresh_profile_has_one_default_account_on_dot_claude() {
        let dir = TempDir::new("settings_fresh");
        let store = SettingsStore::open(dir.path()).unwrap();
        assert_eq!(
            store.claude().accounts,
            vec![account(DEFAULT_ACCOUNT_ID, "Claude", None)]
        );
        let info = account_config(
            store.claude(),
            DEFAULT_ACCOUNT_ID,
            None,
            Some(Path::new("/Users/jt")),
        );
        assert_eq!(info.path, "/Users/jt/.claude");
        assert_eq!(store.terminal(), &TerminalSettings::default());
        assert_eq!(store.ui().theme, "github-dark");
    }

    #[test]
    fn accounts_are_checked_tidied_and_saved() {
        let dir = TempDir::new("settings_accounts");
        let mut store = SettingsStore::open(dir.path()).unwrap();
        store
            .set_claude(ClaudeSettings {
                claude_path: Some("   ".into()),
                accounts: vec![
                    account("default", " Personal ", Some(" ~/.claude ")),
                    account("work", &"W".repeat(60), Some("~/.claude-account2")),
                ],
            })
            .unwrap();
        let again = SettingsStore::open(dir.path()).unwrap();
        let claude = again.claude();
        assert_eq!(claude.claude_path, None);
        assert_eq!(claude.accounts[0].name, "Personal");
        assert_eq!(claude.accounts[0].config_dir.as_deref(), Some("~/.claude"));
        assert_eq!(
            claude.accounts[1].name.chars().count(),
            MAX_ACCOUNT_NAME_CHARS
        );
        assert_eq!(
            account_display_name(claude, "work").chars().count(),
            MAX_ACCOUNT_NAME_CHARS
        );
        assert_eq!(
            account_display_name(claude, "gone"),
            "Account removed",
            "never another account's name"
        );
        assert_eq!(
            account_display_name(&ClaudeSettings::default(), "gone"),
            "Account removed"
        );

        let bad = |accounts: Vec<ClaudeAccount>| {
            let mut store = SettingsStore::open(dir.path()).unwrap();
            store.set_claude(ClaudeSettings {
                claude_path: None,
                accounts,
            })
        };
        assert!(
            bad(vec![account("a", "A", None), account("a", "B", None)]).is_err(),
            "duplicate id"
        );
        assert!(bad(vec![account("../x", "A", None)]).is_err(), "unsafe id");
        assert!(bad(vec![account("a", "  ", None)]).is_err(), "empty name");
        assert!(
            bad(vec![account("a", "A", Some("x\ny"))]).is_err(),
            "newline in folder"
        );
        assert!(bad((0..9)
            .map(|n| account(&format!("a{n}"), "A", None))
            .collect())
        .is_err());
        // Removing every account leaves the default one.
        let mut store = SettingsStore::open(dir.path()).unwrap();
        store.set_claude(ClaudeSettings::default()).unwrap();
        assert_eq!(store.claude().accounts[0].id, DEFAULT_ACCOUNT_ID);
    }

    #[test]
    fn a_hand_edited_file_keeps_its_good_accounts() {
        let dir = TempDir::new("settings_hand_edit");
        std::fs::write(
            dir.join("settings.json"),
            r#"{"schemaVersion":1,"claude":{"accounts":[
                {"id":"work","name":"Work","configDir":"~/.claude-account2"},
                {"id":"bad id","name":"Bad"},
                {"id":"work","name":"Again"}
            ]}}"#,
        )
        .unwrap();
        let store = SettingsStore::open(dir.path()).unwrap();
        assert_eq!(
            store.claude().accounts,
            vec![account("work", "Work", Some("~/.claude-account2"))]
        );
    }

    #[test]
    fn each_account_resolves_its_own_folder_and_only_the_first_takes_the_env() {
        let claude = ClaudeSettings {
            claude_path: None,
            accounts: vec![
                account("default", "Personal", Some("~/.claude")),
                account("work", "Work", Some("~/.claude-account2")),
            ],
        };
        let home = Some(Path::new("/Users/jt"));
        assert_eq!(
            account_config(&claude, "work", None, home).path,
            "/Users/jt/.claude-account2"
        );
        assert_eq!(
            account_config(&claude, "default", None, home).path,
            "/Users/jt/.claude"
        );
        assert_eq!(
            account_config(&claude, "gone", None, home).path,
            "/Users/jt/.claude",
            "unknown is the first"
        );
        assert_eq!(
            account_config(&claude, "default", Some("/env/cfg"), home).path,
            "/env/cfg"
        );
        assert_eq!(
            account_config(&claude, "work", Some("/env/cfg"), home).path,
            "/Users/jt/.claude-account2",
            "the env override is the first account's only"
        );
    }

    // Review: a window whose account was removed silently ran Claude on
    // the first account and saved that account's session over its own.
    #[test]
    fn a_removed_account_has_no_folder_to_launch_under() {
        let claude = ClaudeSettings {
            claude_path: None,
            accounts: vec![account("work", "Work", Some("/w/.claude-account2"))],
        };
        assert!(account_exists(&claude, "work"));
        assert!(!account_exists(&claude, "gone"));
        assert_eq!(
            known_account_config(&claude, "work").map(|info| info.path),
            Some("/w/.claude-account2".to_string())
        );
        assert_eq!(known_account_config(&claude, "gone"), None);
    }

    #[test]
    fn terminal_and_ui_settings_are_clamped_and_saved() {
        let dir = TempDir::new("settings_terminal");
        let mut store = SettingsStore::open(dir.path()).unwrap();
        store
            .set_terminal(TerminalSettings {
                shell: "  /bin/zsh  ".into(),
                font_size: 99,
                option_as_meta: false,
                copy_on_select: true,
                compact_paste: true,
                file_link_editor: "finder".into(),
            })
            .unwrap();
        assert!(store
            .set_terminal(TerminalSettings {
                shell: "a\nb".into(),
                ..TerminalSettings::default()
            })
            .is_err());
        store
            .set_ui(UiSettings {
                theme: "no-such-theme".into(),
                ..UiSettings::default()
            })
            .unwrap();
        let again = SettingsStore::open(dir.path()).unwrap();
        assert_eq!(again.terminal().shell, "/bin/zsh");
        assert_eq!(again.terminal().font_size, 32);
        assert!(!again.terminal().option_as_meta);
        assert!(again.terminal().copy_on_select);
        assert_eq!(again.ui().theme, "github-dark");
        let old: SettingsFile =
            serde_json::from_str(r#"{"schemaVersion":1,"terminal":{"fontSize":12}}"#).unwrap();
        assert!(
            old.terminal.option_as_meta,
            "a missing key keeps its default"
        );
    }

    #[test]
    fn a_newer_settings_file_is_set_aside() {
        let dir = TempDir::new("settings_newer");
        std::fs::write(dir.join("settings.json"), r#"{"schemaVersion":7}"#).unwrap();
        let store = SettingsStore::open(dir.path()).unwrap();
        assert_eq!(store.data.schema_version, SETTINGS_SCHEMA_VERSION);
        assert!(!dir.join("settings.json").exists());
    }
}
