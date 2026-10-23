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
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            shell: String::new(),
            font_size: default_font_size(),
            option_as_meta: default_option_as_meta(),
            copy_on_select: false,
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
        });
    }
    Ok(())
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
}

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
    /// R2 watchdog: minutes with no progress before the panel says so; 0 is off.
    #[serde(default = "default_watchdog_minutes")]
    pub watchdog_minutes: u32,
}

pub const WATCHDOG_MINUTES_LIMIT: u32 = 240;

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
            watchdog_minutes: default_watchdog_minutes(),
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
        self.data.terminal = next;
        self.save()
    }

    pub fn claude(&self) -> &ClaudeSettings {
        &self.data.claude
    }

    /// Paths are trimmed; an empty path clears the setting.
    pub fn set_claude(&mut self, mut next: ClaudeSettings) -> Result<(), String> {
        normalize_claude_accounts(&mut next)?;
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
        self.data.run = RunSettings {
            max_implementers: next.max_implementers.clamp(1, MAX_IMPLEMENTERS_LIMIT),
            stop_hooks: next.stop_hooks,
            watchdog_minutes: next.watchdog_minutes.min(WATCHDOG_MINUTES_LIMIT),
        };
        self.save()
    }

    /// Unknown themes fall back to GitHub Dark.
    pub fn set_ui(&mut self, next: UiSettings) -> Result<(), String> {
        let theme = if UI_THEMES.contains(&next.theme.as_str()) {
            next.theme
        } else {
            default_theme()
        };
        self.data.ui = UiSettings { theme };
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn account(id: &str, name: &str, dir: Option<&str>) -> ClaudeAccount {
        ClaudeAccount {
            id: id.into(),
            name: name.into(),
            config_dir: dir.map(str::to_string),
        }
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
