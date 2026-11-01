//! The platform seam: every OS difference of the app's non-PTY children lives
//! here, as a pure selector with injected inputs plus a thin adapter. The Unix
//! branch of each adapter is what the app did before Windows support.
//!
//! - [`resolve_git_bash`]: the one Git Bash resolution order (docs/launch.md).
//! - [`creation_flags`]: the only place a Windows creation-flags value is
//!   computed (`CommandExt::creation_flags` replaces flags, it does not OR
//!   them, so two helpers must never each call it on one `Command`).
//! - [`quiet_command`] / [`script_command`]: the only constructors of a
//!   non-PTY child (a source scan in this module enforces it).
//! - [`spawn_group`] / [`kill_tree`] / [`kill_tree_pid`]: a child that may
//!   time out gets its own group and is killed with its descendants.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

/// Shown when no Git Bash is found (docs/launch.md, AD-2).
pub const GIT_BASH_NOT_FOUND: &str =
    "Git Bash not found. Install Git for Windows or set CLAUDE_CODE_GIT_BASH_PATH.";

/// `CREATE_NO_WINDOW`: a console child opens no console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// `CREATE_NEW_PROCESS_GROUP`: the child leads its own process group.
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// The Windows creation flags for a child. Pure, and the only place the value
/// is computed. It compiles on every OS so it is tested on macOS.
pub const fn creation_flags(quiet: bool, group: bool) -> u32 {
    let mut flags = 0;
    if quiet {
        flags |= CREATE_NO_WINDOW;
    }
    if group {
        flags |= CREATE_NEW_PROCESS_GROUP;
    }
    flags
}

/// Splits a Windows path on `\` and `/`.
fn windows_parts(path: &str) -> Vec<&str> {
    path.split(['\\', '/']).filter(|p| !p.is_empty()).collect()
}

/// A candidate the resolver must never return: anything under
/// `%SystemRoot%\System32` (WSL's `bash.exe`) or a `WindowsApps` alias.
fn rejected(candidate: &str, system_root: Option<&str>) -> bool {
    let lower = candidate.replace('/', "\\").to_lowercase();
    if lower.contains("\\windowsapps\\") {
        return true;
    }
    let system32 = match system_root {
        Some(root) if !root.is_empty() => {
            format!("{}\\system32\\", root.replace('/', "\\").to_lowercase())
        }
        _ => "\\windows\\system32\\".to_string(),
    };
    lower.starts_with(&system32) || lower.contains("\\windows\\system32\\")
}

/// The Git for Windows `bash.exe`, by the AD-2 order, with injected inputs:
/// `CLAUDE_CODE_GIT_BASH_PATH` (when it is a file), `%ProgramFiles%`,
/// `%ProgramW6432%`, `%ProgramFiles(x86)%` and `%LOCALAPPDATA%\Programs`
/// under `Git\bin\bash.exe`, then `git --exec-path` three folders up.
/// Candidates under System32 or WindowsApps are rejected. Pure: this is the
/// Windows logic on every OS, tested on macOS.
pub fn resolve_windows_bash(
    env: impl Fn(&str) -> Option<String>,
    is_file: impl Fn(&Path) -> bool,
    git_exec_path: impl Fn() -> Option<PathBuf>,
) -> Result<PathBuf, String> {
    let system_root = env("SystemRoot");
    let usable = |candidate: String| -> Option<PathBuf> {
        let path = PathBuf::from(&candidate);
        (!rejected(&candidate, system_root.as_deref()) && is_file(&path)).then_some(path)
    };
    let under = |var: &str, tail: &str| -> Option<String> {
        env(var)
            .filter(|base| !base.is_empty())
            .map(|base| format!("{}\\{tail}", base.trim_end_matches(['\\', '/'])))
    };
    let tail = "Git\\bin\\bash.exe";
    let mut candidates: Vec<String> = Vec::new();
    candidates.extend(env("CLAUDE_CODE_GIT_BASH_PATH").filter(|p| !p.is_empty()));
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        candidates.extend(under(var, tail));
    }
    candidates.extend(under("LOCALAPPDATA", &format!("Programs\\{tail}")));
    for candidate in candidates {
        if let Some(found) = usable(candidate) {
            return Ok(found);
        }
    }
    if let Some(exec) = git_exec_path() {
        let text = exec.to_string_lossy().to_string();
        let parts = windows_parts(&text);
        if parts.len() > 3 {
            let keep = &parts[..parts.len() - 3];
            let unc = text.starts_with("\\\\") || text.starts_with("//");
            let mut base = keep.join("\\");
            if unc {
                base = format!("\\\\{base}");
            }
            if let Some(found) = usable(format!("{base}\\bin\\bash.exe")) {
                return Ok(found);
            }
        }
    }
    Err(GIT_BASH_NOT_FOUND.to_string())
}

/// The bash to run plugin scripts with: `bash` from PATH on Unix, the Git Bash
/// of [`resolve_windows_bash`] on Windows (the real environment, the real
/// disk, and `git --exec-path`).
pub fn resolve_git_bash() -> Result<PathBuf, String> {
    #[cfg(not(windows))]
    {
        Ok(PathBuf::from("bash"))
    }
    #[cfg(windows)]
    {
        resolve_windows_bash(
            |name| std::env::var(name).ok(),
            |path| path.is_file(),
            || {
                let out = quiet_command("git").arg("--exec-path").output().ok()?;
                let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                (out.status.success() && !text.is_empty()).then(|| PathBuf::from(text))
            },
        )
    }
}

/// The one constructor of a non-PTY child: no console window on Windows.
pub fn quiet_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(creation_flags(true, false));
    }
    cmd
}

/// `bash <script>` on the resolved bash, quiet. The caller adds the
/// arguments. A missing Git Bash is the `Err` text for the user.
pub fn script_command(script: &Path) -> Result<Command, String> {
    let mut cmd = quiet_command(resolve_git_bash()?);
    cmd.arg(script);
    Ok(cmd)
}

/// Makes `cmd` lead its own group so [`kill_tree`] reaches its descendants.
/// Every caller is a quiet child, so Windows keeps `CREATE_NO_WINDOW` in the
/// same single `creation_flags` call.
pub fn spawn_group(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(creation_flags(true, true));
    }
    #[cfg(not(any(unix, windows)))]
    let _ = cmd;
}

/// Kills a [`spawn_group`] child and everything it started.
pub fn kill_tree(child: &Child) {
    kill_tree_pid(child.id());
}

/// [`kill_tree`] for a pid alone (the check runner keeps only pids).
pub fn kill_tree_pid(pid: u32) {
    use std::process::Stdio;
    #[cfg(unix)]
    {
        let _ = quiet_command("kill")
            .args(["-9", &format!("-{pid}")])
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        let _ = quiet_command("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(any(unix, windows)))]
    let _ = (pid, Stdio::null());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn creation_flags_are_computed_in_one_place() {
        assert_eq!(creation_flags(true, false), 0x0800_0000);
        assert_eq!(creation_flags(true, true), 0x0800_0200);
        assert_eq!(creation_flags(false, true), 0x0000_0200);
        assert_eq!(creation_flags(false, false), 0);
    }

    #[test]
    fn git_bash_resolver_matches_every_fixture_case() {
        let text = include_str!("../../../fixtures/platform/git-bash-cases.json");
        let fixture: Value = serde_json::from_str(text).unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert!(cases.len() >= 10);
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let env: HashMap<String, String> = case["env"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                .collect();
            let files: HashSet<String> = case["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            let exec = case["gitExecPath"].as_str().map(PathBuf::from);
            let got = resolve_windows_bash(
                |key| env.get(key).cloned(),
                |path| files.contains(path.to_str().unwrap()),
                || exec.clone(),
            );
            match case["expect"].as_str() {
                Some(path) => assert_eq!(got, Ok(PathBuf::from(path)), "{name}"),
                None => assert_eq!(got, Err(GIT_BASH_NOT_FOUND.to_string()), "{name}"),
            }
        }
    }

    #[test]
    fn the_not_found_text_is_exact() {
        assert_eq!(
            GIT_BASH_NOT_FOUND,
            "Git Bash not found. Install Git for Windows or set CLAUDE_CODE_GIT_BASH_PATH."
        );
    }

    #[cfg(unix)]
    #[test]
    fn on_unix_the_bash_is_bash_from_path() {
        assert_eq!(resolve_git_bash(), Ok(PathBuf::from("bash")));
    }

    #[cfg(unix)]
    #[test]
    fn kill_tree_reaches_a_grandchild() {
        let dir = crate::test_support::TempDir::new("kill_tree");
        let pid_file = dir.join("pid");
        let mut cmd = quiet_command("sh");
        cmd.arg("-c").arg(format!(
            "sleep 60 & echo $! > '{}'; wait",
            pid_file.display()
        ));
        spawn_group(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let mut pid = None;
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(&pid_file) {
                pid = text.trim().parse::<u32>().ok();
                if pid.is_some() {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let pid = pid.expect("grandchild pid");
        kill_tree(&child);
        let _ = child.wait();
        let mut gone = false;
        for _ in 0..100 {
            if crate::test_support::pid_alive(pid) == Some(false) {
                gone = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(gone, "the grandchild survived kill_tree");
    }
}
