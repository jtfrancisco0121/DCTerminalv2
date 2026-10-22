//! Which program a plain Terminal tab runs.
//!
//! Windows prefers `pwsh` when it is on PATH, otherwise `powershell.exe`.
//! Other platforms use `$SHELL`, then `/bin/sh`. On macOS a known shell
//! starts as a login shell (`-l`), like Terminal.app, so `~/.zprofile` runs.
//! A Settings value replaces that program and is not parsed as a shell
//! command line.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOs {
    Windows,
    MacOs,
    Unix,
}

/// Shells that take `-l` to start as a login shell.
const LOGIN_SHELLS: [&str; 5] = ["zsh", "bash", "fish", "sh", "ksh"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellQuery<'a> {
    pub os: HostOs,
    pub configured: &'a str,
    pub env_shell: Option<&'a str>,
    pub pwsh_on_path: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramArgs {
    pub program: String,
    pub args: Vec<String>,
}

pub fn resolve_shell(query: ShellQuery<'_>) -> ProgramArgs {
    let configured = query.configured.trim();
    if !configured.is_empty() {
        return ProgramArgs {
            program: configured.to_string(),
            args: Vec::new(),
        };
    }
    match query.os {
        HostOs::Windows => {
            if query.pwsh_on_path {
                ProgramArgs {
                    program: "pwsh.exe".to_string(),
                    args: vec!["-NoLogo".to_string()],
                }
            } else {
                ProgramArgs {
                    program: "powershell.exe".to_string(),
                    args: vec!["-NoLogo".to_string()],
                }
            }
        }
        HostOs::MacOs | HostOs::Unix => {
            let program = query
                .env_shell
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("/bin/sh");
            let name = Path::new(program)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let login = query.os == HostOs::MacOs && LOGIN_SHELLS.contains(&name);
            ProgramArgs {
                program: program.to_string(),
                args: if login {
                    vec!["-l".to_string()]
                } else {
                    Vec::new()
                },
            }
        }
    }
}

pub fn resolve_shell_for_host(configured: &str) -> ProgramArgs {
    let shell = std::env::var("SHELL").ok();
    resolve_shell(ShellQuery {
        os: if cfg!(windows) {
            HostOs::Windows
        } else if cfg!(target_os = "macos") {
            HostOs::MacOs
        } else {
            HostOs::Unix
        },
        configured,
        env_shell: shell.as_deref(),
        pwsh_on_path: command_on_path("pwsh"),
    })
}

pub fn command_on_path(name: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&path_var) {
        if candidate_exists(&dir, name) {
            return true;
        }
    }
    false
}

fn candidate_exists(dir: &Path, name: &str) -> bool {
    if dir.join(name).is_file() {
        return true;
    }
    #[cfg(windows)]
    {
        for ext in ["exe", "cmd", "bat"] {
            if dir.join(format!("{name}.{ext}")).is_file() {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_prefers_pwsh_then_powershell() {
        let pwsh = resolve_shell(ShellQuery {
            os: HostOs::Windows,
            configured: "",
            env_shell: None,
            pwsh_on_path: true,
        });
        assert_eq!(pwsh.program, "pwsh.exe");
        assert_eq!(pwsh.args, vec!["-NoLogo".to_string()]);
        let powershell = resolve_shell(ShellQuery {
            os: HostOs::Windows,
            configured: "  ",
            env_shell: None,
            pwsh_on_path: false,
        });
        assert_eq!(powershell.program, "powershell.exe");
    }

    #[test]
    fn unix_uses_shell_then_bin_sh() {
        let from_env = resolve_shell(ShellQuery {
            os: HostOs::Unix,
            configured: "",
            env_shell: Some("/bin/zsh"),
            pwsh_on_path: false,
        });
        assert_eq!(from_env.program, "/bin/zsh");
        assert!(from_env.args.is_empty());
        let fallback = resolve_shell(ShellQuery {
            os: HostOs::Unix,
            configured: "",
            env_shell: None,
            pwsh_on_path: true,
        });
        assert_eq!(fallback.program, "/bin/sh");
        assert!(fallback.args.is_empty());
    }

    #[test]
    fn macos_starts_known_shells_as_login_shells() {
        for shell in [
            "/bin/zsh",
            "/bin/bash",
            "/opt/homebrew/bin/fish",
            "/bin/ksh",
        ] {
            let command = resolve_shell(ShellQuery {
                os: HostOs::MacOs,
                configured: "",
                env_shell: Some(shell),
                pwsh_on_path: false,
            });
            assert_eq!(command.program, shell);
            assert_eq!(command.args, vec!["-l".to_string()], "{shell}");
        }
        let fallback = resolve_shell(ShellQuery {
            os: HostOs::MacOs,
            configured: "",
            env_shell: None,
            pwsh_on_path: false,
        });
        assert_eq!(fallback.program, "/bin/sh");
        assert_eq!(fallback.args, vec!["-l".to_string()]);
    }

    #[test]
    fn macos_leaves_unknown_shells_and_configured_programs_without_args() {
        let nu = resolve_shell(ShellQuery {
            os: HostOs::MacOs,
            configured: "",
            env_shell: Some("/opt/homebrew/bin/nu"),
            pwsh_on_path: false,
        });
        assert!(nu.args.is_empty());
        let configured = resolve_shell(ShellQuery {
            os: HostOs::MacOs,
            configured: "/bin/zsh",
            env_shell: Some("/bin/zsh"),
            pwsh_on_path: false,
        });
        assert!(configured.args.is_empty());
    }

    #[test]
    fn configured_shell_replaces_the_default_without_extra_args() {
        let command = resolve_shell(ShellQuery {
            os: HostOs::Windows,
            configured: r"C:\Tools\pwsh.exe",
            env_shell: Some("/bin/bash"),
            pwsh_on_path: true,
        });
        assert_eq!(command.program, r"C:\Tools\pwsh.exe");
        assert!(command.args.is_empty());
    }
}
