//! Claude Code for terminal tabs: `claude` under one account's config folder.
//!
//! Every Claude process gets `CLAUDE_CONFIG_DIR=<configDir>`, so a GUI
//! launch uses the same account as JT's `claude2` alias.
//!
//! Terminal flags (`claude_terminal_args`):
//! - `--settings` with DCTerminalv2's usage status line (`status_line`; not
//!   when you set a status line of your own), after any `node` script
//! - `--session-id <uuid>` on a fresh launch, so the tab knows its log
//! - `--resume <id>` to reopen a session
//!
//! Never passed: `--dangerously-skip-permissions`, `--allowedTools`,
//! `--disallowedTools`, `--permission-mode`, `--cloud`, `--bg`, `--continue`:
//! a tab is a plain `claude` you drive yourself.

use super::claude_config::{claude_env, ConfigDirInfo};
use super::claude_detect::{read_claude_version, resolve_claude, unwrap_windows_shim, ResolvedCli};
use super::{ProgramArgs, TerminalKind, TerminalLaunch};
use crate::store::settings_store::{resolved_account_config, ClaudeSettings};
use serde::Serialize;

/// Whether two config folders are the same place. Canonical when both exist.
pub fn same_config_dir(left: &str, right: &str) -> bool {
    let left_path = std::path::Path::new(left.trim());
    let right_path = std::path::Path::new(right.trim());
    match (left_path.canonicalize(), right_path.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => left_path == right_path,
    }
}

/// A Claude session id is a UUID; anything else is refused before spawn.
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-')
}

/// argv after `claude` for one terminal launch.
pub fn claude_terminal_args(req: &TerminalLaunch) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    if let Some(settings) = &req.settings {
        args.push("--settings".to_string());
        args.push(settings.to_string());
    }
    match &req.kind {
        TerminalKind::Plain => {
            if let Some(id) = req.session_id.as_deref().map(str::trim) {
                if !valid_session_id(id) {
                    return Err(format!("not a Claude session id: {id}"));
                }
                args.push("--session-id".to_string());
                args.push(id.to_string());
            }
        }
        // `--resume <id>` keeps appending to `<id>.jsonl` (no `--fork-session`),
        // so the terminal's session id is the resumed one.
        TerminalKind::Resume { session_id } => {
            let id = session_id.trim();
            if !valid_session_id(id) {
                return Err(format!("not a Claude session id: {id}"));
            }
            args.push("--resume".to_string());
            args.push(id.to_string());
        }
    }
    Ok(args)
}

/// The program and argv for one terminal launch of the resolved CLI: a
/// `node` + script shim keeps the script first, so Claude's own flags
/// (`--settings` among them) always follow it.
fn claude_terminal_program(
    launch: ResolvedCli,
    req: &TerminalLaunch,
) -> Result<ProgramArgs, String> {
    let mut args = launch.prefix_args;
    args.extend(claude_terminal_args(req)?);
    Ok(ProgramArgs::new(launch.program.display().to_string(), args))
}

/// What `claude` was found to be.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeStatus {
    pub found: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub error: Option<String>,
}

/// `claude` for one account: where the CLI is and the account's folder.
#[derive(Debug, Clone)]
pub struct ClaudeProvider {
    /// Settings `claudePath`; empty means look it up.
    pub claude_path: Option<String>,
    /// Resolved when the provider is built (env override → setting → `~/.claude`).
    pub config: ConfigDirInfo,
}

impl ClaudeProvider {
    /// `account_id`'s folder (an unknown id is the first account).
    pub fn for_account(settings: &ClaudeSettings, account_id: &str) -> Self {
        Self {
            claude_path: settings.claude_path.clone(),
            config: resolved_account_config(settings, account_id),
        }
    }

    /// Env for every Claude process (`CLAUDE_CONFIG_DIR`).
    pub fn env(&self) -> Vec<(String, String)> {
        claude_env(&self.config)
    }

    /// Executable lookup and version (read-only).
    pub fn detect(&self) -> ClaudeStatus {
        let claude = resolve_claude(self.claude_path.as_deref());
        let version = claude
            .as_deref()
            .and_then(|path| read_claude_version(path, &self.config));
        ClaudeStatus {
            found: claude.is_some(),
            path: claude.as_ref().map(|p| p.display().to_string()),
            version,
            error: claude.is_none().then(|| self.missing_message()),
        }
    }

    pub fn terminal_command(&self, req: &TerminalLaunch) -> Result<ProgramArgs, String> {
        let program =
            resolve_claude(self.claude_path.as_deref()).ok_or_else(|| self.missing_message())?;
        let mut command = claude_terminal_program(unwrap_windows_shim(&program), req)?;
        command.env = self.env();
        Ok(command)
    }

    /// Text shown when the CLI is not installed.
    pub fn missing_message(&self) -> String {
        "Claude Code (claude) was not found. Install it from \
         https://code.claude.com/docs/en/setup. On Windows: \
         `npm install -g @anthropic-ai/claude-code` (npm puts `claude.cmd` in `%APPDATA%\\npm`) \
         or the native installer on that page. On a Mac: `brew install --cask claude-code`. \
         Or set DCTV2_CLAUDE_PATH to `claude.exe` or the `.cmd` shim."
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::claude_config::resolve_with;
    use crate::test_support::TempDir;
    use std::path::{Path, PathBuf};

    const ID: &str = "0b6c1d2e-aaaa-4bbb-8ccc-1234567890ab";

    fn plain(session_id: Option<&str>) -> TerminalLaunch {
        TerminalLaunch {
            kind: TerminalKind::Plain,
            session_id: session_id.map(str::to_string),
            settings: None,
        }
    }

    fn resume(id: &str) -> TerminalLaunch {
        TerminalLaunch {
            kind: TerminalKind::Resume {
                session_id: id.to_string(),
            },
            session_id: None,
            settings: None,
        }
    }

    #[test]
    fn usage_settings_go_with_the_claude_flags_after_a_node_script() {
        let script = r"C:\npm\node_modules\@anthropic-ai\claude-code\cli.js".to_string();
        let settings = crate::status_line::settings("'dct' --dct-status-line 'x'");
        let launch = TerminalLaunch {
            settings: Some(settings.clone()),
            ..plain(Some(ID))
        };
        let node = ResolvedCli {
            program: PathBuf::from(r"C:\nodejs\node.exe"),
            prefix_args: vec![script.clone()],
        };
        let args = claude_terminal_program(node, &launch).unwrap().args;
        assert_eq!(args[0], script, "node must get the script first: {args:?}");
        assert_eq!(
            args[1..],
            [
                "--settings".to_string(),
                settings.to_string(),
                "--session-id".to_string(),
                ID.to_string()
            ]
        );

        // A native claude: the flag comes first.
        let direct = ResolvedCli {
            program: PathBuf::from("/usr/local/bin/claude"),
            prefix_args: Vec::new(),
        };
        let args = claude_terminal_program(direct, &launch).unwrap().args;
        assert_eq!(args[0], "--settings");
        // Resume keeps it too; no settings means no flag.
        let resumed = TerminalLaunch {
            settings: Some(settings),
            ..resume(ID)
        };
        assert_eq!(claude_terminal_args(&resumed).unwrap()[0], "--settings");
        assert!(!claude_terminal_args(&resume(ID))
            .unwrap()
            .contains(&"--settings".to_string()));
    }

    #[test]
    fn a_fresh_launch_pins_its_session_id_and_a_resume_reopens_it() {
        let id = crate::provider::new_claude_session_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id.as_bytes()[14], b'4');
        assert!(valid_session_id(&id));
        assert_eq!(
            claude_terminal_args(&plain(Some(&id))).unwrap(),
            vec!["--session-id", id.as_str()]
        );
        assert!(claude_terminal_args(&plain(None)).unwrap().is_empty());
        assert_eq!(
            claude_terminal_args(&resume(ID)).unwrap(),
            vec!["--resume", ID]
        );
        // A resume never adds a second id.
        let both = TerminalLaunch {
            session_id: Some("11111111-2222-4333-8444-555555555555".into()),
            ..resume(ID)
        };
        let args = claude_terminal_args(&both).unwrap();
        assert!(!args
            .iter()
            .any(|a| a == "--session-id" || a == "--fork-session"));
    }

    #[test]
    fn ids_that_could_be_flags_or_paths_are_refused() {
        for bad in ["--continue", "../etc", "", "a b"] {
            assert!(claude_terminal_args(&resume(bad)).is_err(), "{bad:?}");
        }
        for bad in ["--continue", "../etc", "a b"] {
            assert!(claude_terminal_args(&plain(Some(bad))).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn never_passes_forbidden_flags() {
        for launch in [plain(Some(ID)), plain(None), resume(ID)] {
            let args = claude_terminal_args(&launch).unwrap();
            for bad in [
                "--dangerously-skip-permissions",
                "--allow-dangerously-skip-permissions",
                "--permission-mode",
                "--allowedTools",
                "--disallowedTools",
                "--cloud",
                "--bg",
                "--settings",
                "--continue",
            ] {
                assert!(!args.iter().any(|a| a == bad), "{bad} in {args:?}");
            }
        }
    }

    fn provider_with(root: &Path, config: ConfigDirInfo) -> ClaudeProvider {
        let bin = root.join("bin").join("claude");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "#!/bin/sh\n").unwrap();
        ClaudeProvider {
            claude_path: Some(bin.display().to_string()),
            config,
        }
    }

    #[test]
    fn terminal_env_carries_the_resolved_config_dir() {
        use crate::provider::claude_config::{ConfigDirSource, CLAUDE_CONFIG_DIR};
        let root = TempDir::new("claude_env");
        let home = root.join("home");
        let from_env = home.join(".claude-env");
        std::fs::create_dir_all(home.join(".claude-account2")).unwrap();
        std::fs::create_dir_all(&from_env).unwrap();
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        let env_value = from_env.display().to_string();
        let cases = [
            (
                resolve_with(None, Some("~/.claude-account2"), Some(&home)),
                ConfigDirSource::Setting,
            ),
            (
                resolve_with(Some(&env_value), Some("~/.claude-account2"), Some(&home)),
                ConfigDirSource::Env,
            ),
            (
                resolve_with(None, None, Some(&home)),
                ConfigDirSource::Default,
            ),
        ];
        let pinned = std::fs::canonicalize(root.path()).unwrap();
        for (config, source) in cases {
            assert_eq!(config.source, source);
            assert!(
                std::path::Path::new(&config.path).starts_with(&pinned),
                "{} is not under the temp dir",
                config.path
            );
            let expected = config.path.clone();
            let p = provider_with(root.path(), config);
            let command = p.terminal_command(&plain(Some(ID))).unwrap();
            assert!(command.program.ends_with("claude"));
            assert_eq!(
                command
                    .env
                    .iter()
                    .find(|(k, _)| k == CLAUDE_CONFIG_DIR)
                    .map(|(_, v)| v.clone()),
                Some(expected),
                "{source:?}"
            );
        }
    }

    #[test]
    fn each_account_launches_under_its_own_folder() {
        use crate::provider::claude_config::{CLAUDE_CONFIG_DIR, CONFIG_DIR_ENV};
        use crate::store::settings_store::ClaudeAccount;
        if std::env::var_os(CONFIG_DIR_ENV).is_some() {
            eprintln!("ignored: DCTV2_CLAUDE_CONFIG_DIR is set");
            return;
        }
        let root = TempDir::new("claude_accounts");
        let folder = |name: &str| {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            dir.display().to_string()
        };
        let fake = provider_with(root.path(), resolve_with(None, Some("/nowhere"), None));
        let settings = ClaudeSettings {
            claude_path: fake.claude_path,
            accounts: vec![
                ClaudeAccount {
                    id: "default".into(),
                    name: "Personal".into(),
                    config_dir: Some(folder("personal")),
                },
                ClaudeAccount {
                    id: "work".into(),
                    name: "Work".into(),
                    config_dir: Some(folder("work")),
                },
            ],
        };
        let pinned = std::fs::canonicalize(root.path()).unwrap();
        for (account, name) in [("default", "personal"), ("work", "work")] {
            let provider = ClaudeProvider::for_account(&settings, account);
            assert_eq!(
                std::path::Path::new(&provider.config.path),
                pinned.join(name)
            );
            let command = provider.terminal_command(&plain(None)).unwrap();
            let dir = command
                .env
                .iter()
                .find(|(k, _)| k == CLAUDE_CONFIG_DIR)
                .map(|(_, v)| v.clone());
            assert_eq!(dir, Some(provider.config.path.clone()));
        }
    }

    #[test]
    fn missing_claude_message_names_the_installs() {
        let p = ClaudeProvider {
            claude_path: None,
            config: resolve_with(None, Some("/nowhere"), None),
        };
        let message = p.missing_message();
        assert!(message.contains("npm install -g @anthropic-ai/claude-code"));
        assert!(message.contains("%APPDATA%\\npm"));
        assert!(message.contains("brew install --cask claude-code"));
        assert!(message.contains("DCTV2_CLAUDE_PATH"));
    }

    #[test]
    fn windows_npm_shims_spawn_the_exe() {
        let root = TempDir::new("claude_winspawn");
        let npm = root.join("npm");
        let exe = npm
            .join("node_modules")
            .join("@anthropic-ai")
            .join("claude-code")
            .join("bin")
            .join("claude.exe");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "").unwrap();
        let claude_cmd = npm.join("claude.cmd");
        std::fs::write(&claude_cmd, "@echo off\r\n").unwrap();
        let config = root.join("account");
        std::fs::create_dir_all(&config).unwrap();
        let provider = ClaudeProvider {
            claude_path: Some(claude_cmd.display().to_string()),
            config: resolve_with(None, Some(&config.display().to_string()), None),
        };
        let terminal = provider.terminal_command(&plain(None)).unwrap();
        assert_eq!(terminal.program, exe.display().to_string());
    }

    #[test]
    fn config_dirs_compare_by_place() {
        let root = TempDir::new("claude_same_dir");
        let a = root.join("a");
        std::fs::create_dir_all(&a).unwrap();
        let a = a.display().to_string();
        assert!(same_config_dir(&a, &format!("{a}/")));
        assert!(same_config_dir(" /x/missing ", "/x/missing"));
        assert!(!same_config_dir(&a, &root.join("b").display().to_string()));
    }
}
