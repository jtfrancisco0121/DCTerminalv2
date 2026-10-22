//! Find Claude Code (`claude`) and ask it whether it is signed in.
//!
//! Read-only: DCTerminalv2 runs `claude --version` and `claude auth status
//! --json`, both with `CLAUDE_CONFIG_DIR=<configDir>` (via `claude_env`), and
//! never reads Claude's credential store or runs a login flow.
//!
//! `claude auth status` writes `.claude.json` into the config folder (Claude's
//! own bookkeeping) and would create a missing folder, so it only runs when the
//! resolved folder exists. `claude --version` does not touch the folder.

use super::claude_config::{claude_env, ConfigDirInfo};
use crate::cli_detect::{find_on_path, run_cli, strip_ansi, LoginStatus};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Env override for the `claude` executable.
pub const CLAUDE_PATH_ENV: &str = "DCTV2_CLAUDE_PATH";
fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// A CLI after a Windows npm shim has been resolved.
///
/// `prefix_args` is set when the shim launches `node.exe` with a script.
/// A native `claude.exe` has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCli {
    pub program: PathBuf,
    pub prefix_args: Vec<String>,
}

impl ResolvedCli {
    fn direct(program: PathBuf) -> Self {
        Self {
            program,
            prefix_args: Vec::new(),
        }
    }
}

/// `claude.exe`, then `claude.cmd` / `claude.bat`, then the extensionless npm shim.
pub fn windows_cli_names(binary: &str) -> Vec<String> {
    vec![
        format!("{binary}.exe"),
        format!("{binary}.cmd"),
        format!("{binary}.bat"),
        binary.to_string(),
    ]
}

fn host_cli_names(binary: &str) -> Vec<String> {
    if cfg!(windows) {
        return windows_cli_names(binary);
    }
    let mut names = vec![binary.to_string()];
    for name in windows_cli_names(binary) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// Default npm global folders on Windows (`npm prefix -g` is usually `%APPDATA%\npm`).
pub fn windows_npm_bin_dirs(appdata: Option<&Path>, local_appdata: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(dir) = appdata {
        out.push(dir.join("npm"));
    }
    if let Some(dir) = local_appdata {
        out.push(dir.join("npm"));
    }
    out
}

fn windows_npm_dirs_from_env() -> Vec<PathBuf> {
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    windows_npm_bin_dirs(appdata.as_deref(), local.as_deref())
}

pub fn is_windows_batch(path: &Path) -> bool {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"),
        None => false,
    }
}

fn is_direct_exe(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
}

fn should_try_unwrap(path: &Path) -> bool {
    if is_windows_batch(path) {
        return true;
    }
    // npm also writes an extensionless `claude` shell stub next to `claude.cmd`.
    cfg!(windows) && path.extension().is_none()
}

/// When `path` is an npm `.cmd`/`.bat` (or, on Windows, the extensionless stub),
/// return the native executable it launches. A `node` + script shim keeps the
/// script in [`ResolvedCli::prefix_args`]. Anything else is returned unchanged.
pub fn unwrap_windows_shim(path: &Path) -> ResolvedCli {
    if is_direct_exe(path) {
        return ResolvedCli::direct(path.to_path_buf());
    }
    if should_try_unwrap(path) {
        if let Some(resolved) = unwrap_shim_file(path) {
            return resolved;
        }
    }
    ResolvedCli::direct(path.to_path_buf())
}

/// Path to put in `CLAUDE_CODE_EXECUTABLE`: the native binary when the shim
/// points at one. A `node` + script pair cannot be expressed as one path, so
/// the original shim is kept in that case.
pub fn native_executable(path: &Path) -> PathBuf {
    let resolved = unwrap_windows_shim(path);
    if resolved.prefix_args.is_empty() && !is_windows_batch(&resolved.program) {
        resolved.program
    } else {
        path.to_path_buf()
    }
}

fn unwrap_shim_file(path: &Path) -> Option<ResolvedCli> {
    let dir = path.parent()?;
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    for candidate in known_package_exes(dir, &stem) {
        if candidate.is_file() {
            return Some(ResolvedCli::direct(candidate));
        }
    }
    let text = read_shim_text(path)?;
    pick_parsed_target(dir, &stem, &text)
}

fn known_package_exes(dir: &Path, stem: &str) -> Vec<PathBuf> {
    let spec: Option<(&str, &str)> = match stem {
        "claude" => Some(("@anthropic-ai", "claude-code")),
        _ => None,
    };
    let Some((scope, pkg)) = spec else {
        return Vec::new();
    };
    vec![dir
        .join("node_modules")
        .join(scope)
        .join(pkg)
        .join("bin")
        .join(format!("{stem}.exe"))]
}

fn packaged_executable(binary: &str, dir: &Path) -> Option<PathBuf> {
    known_package_exes(dir, binary)
        .into_iter()
        .find(|path| path.is_file())
}

fn read_shim_text(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 64 * 1024];
    let n = file.read(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf[..n]).into_owned())
}

fn expand_shim_vars(dir: &Path, text: &str) -> String {
    let mut prefix = dir.display().to_string();
    if !prefix.ends_with('\\') && !prefix.ends_with('/') {
        prefix.push(std::path::MAIN_SEPARATOR);
    }
    text.replace("%~dp0%", &prefix)
        .replace("%dp0%", &prefix)
        .replace("%~dp0", &prefix)
}

fn path_from_token(token: &str) -> PathBuf {
    let token = token.trim().trim_matches(|c| c == '"' || c == '\'');
    if token.is_empty() {
        return PathBuf::new();
    }
    let starts_abs = token.starts_with('/') || token.starts_with('\\');
    let mut parts = token.split(['\\', '/']).filter(|part| !part.is_empty());
    let Some(first) = parts.next() else {
        return PathBuf::new();
    };
    let mut path = if starts_abs {
        PathBuf::from(format!("/{first}"))
    } else {
        PathBuf::from(first)
    };
    for part in parts {
        path.push(part);
    }
    path
}

fn token_interesting(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    let ext_ok = lower.ends_with(".exe")
        || lower.ends_with(".js")
        || lower.ends_with(".mjs")
        || lower.ends_with(".cjs");
    ext_ok
        && token.len() > 4
        && (token.contains('\\') || token.contains('/') || token.contains(':'))
}

fn scrape_paths(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let exts = [".exe", ".js", ".mjs", ".cjs"];
    let mut found = Vec::new();
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let mut matched = None;
        for ext in exts {
            if lower[i..].starts_with(ext) {
                let after = i + ext.len();
                let boundary = after >= bytes.len()
                    || matches!(
                        bytes[after],
                        b' ' | b'"' | b'\'' | b'\r' | b'\n' | b'\t' | b'%' | b'&' | b'|' | b')'
                    );
                if boundary {
                    matched = Some(after);
                    break;
                }
            }
        }
        if let Some(end) = matched {
            let begin = text[..i]
                .char_indices()
                .rev()
                .find(|(_, c)| c.is_whitespace() || matches!(*c, '"' | '\'' | '=' | '&' | '|'))
                .map(|(pos, c)| pos + c.len_utf8())
                .unwrap_or(0);
            let token = text[begin..end].trim().trim_matches('"').trim();
            if token_interesting(token) {
                found.push(token.to_string());
            }
            i = end;
        } else {
            i += 1;
        }
    }
    found
}

fn file_name_lower(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_ascii_lowercase())
}

fn is_node_binary(path: &Path) -> bool {
    matches!(
        file_name_lower(path).as_deref(),
        Some("node.exe") | Some("node")
    )
}

fn node_beside(dir: &Path) -> Option<PathBuf> {
    for name in ["node.exe", "node"] {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn pick_parsed_target(dir: &Path, stem: &str, text: &str) -> Option<ResolvedCli> {
    let stem = stem.to_ascii_lowercase();
    let expanded = expand_shim_vars(dir, text);
    let mut node = None;
    let mut script = None;
    let mut exe = None;
    for token in scrape_paths(&expanded) {
        let candidate = path_from_token(&token);
        if !candidate.is_file() {
            continue;
        }
        if is_node_binary(&candidate) {
            node = Some(candidate);
            continue;
        }
        let name = file_name_lower(&candidate).unwrap_or_default();
        if name.ends_with(".exe") {
            let preferred = stem.is_empty() || name.contains(&stem);
            if preferred || exe.is_none() {
                exe = Some(candidate);
            }
            continue;
        }
        if name.ends_with(".js") || name.ends_with(".mjs") || name.ends_with(".cjs") {
            script = Some(candidate);
        }
    }
    if let Some(exe) = exe {
        return Some(ResolvedCli::direct(exe));
    }
    let script = script?;
    let node = node
        .or_else(|| node_beside(dir))
        .or_else(|| find_on_path("node"))?;
    Some(ResolvedCli {
        program: node,
        prefix_args: vec![script.display().to_string()],
    })
}

fn first_cli(binary: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let names = host_cli_names(binary);
    for dir in dirs {
        for name in &names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        if let Some(path) = packaged_executable(binary, dir) {
            return Some(path);
        }
    }
    None
}

/// Places a GUI app should look when `PATH` (which macOS GUI apps do not
/// inherit from the shell) has no `claude`.
pub fn known_claude_locations(home: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    #[cfg(not(windows))]
    {
        out.push(PathBuf::from("/opt/homebrew/bin/claude"));
        out.push(PathBuf::from("/usr/local/bin/claude"));
    }
    if let Some(home) = home {
        out.push(home.join(".local").join("bin").join(exe("claude")));
        // Older Claude Code "local" installs.
        out.push(home.join(".claude").join("local").join(exe("claude")));
    }
    out
}

fn first_file(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().find(|path| path.is_file())
}

fn from_override(value: Option<&str>) -> Option<PathBuf> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_file())
}

/// Inputs to the lookup, passed in so tests can use fake folders.
#[derive(Debug, Clone, Default)]
pub struct Lookup {
    pub env_override: Option<String>,
    pub setting: Option<String>,
    /// Folders to search like `PATH`.
    pub path_dirs: Vec<PathBuf>,
    pub known: Vec<PathBuf>,
}

/// `DCTV2_CLAUDE_PATH`, settings `claudePath`, `PATH`, then known locations.
/// On Windows a `.cmd`/`.bat` npm shim is resolved to `claude.exe` when that
/// file sits next to the shim.
pub fn resolve_claude_with(lookup: &Lookup) -> Option<PathBuf> {
    from_override(lookup.env_override.as_deref())
        .or_else(|| from_override(lookup.setting.as_deref()))
        .or_else(|| first_cli("claude", &lookup.path_dirs))
        .or_else(|| first_file(lookup.known.iter().cloned()))
        .map(|path| native_executable(&path))
}

fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default()
}

pub fn resolve_claude(setting: Option<&str>) -> Option<PathBuf> {
    let mut lookup = Lookup {
        env_override: std::env::var(CLAUDE_PATH_ENV).ok(),
        setting: setting.map(str::to_string),
        path_dirs: path_dirs(),
        known: known_claude_locations(home_dir().as_deref()),
    };
    lookup.path_dirs.extend(windows_npm_dirs_from_env());
    if let Some(found) = resolve_claude_with(&lookup) {
        return Some(found);
    }
    // Custom `npm prefix -g` (not the default `%APPDATA%\npm`).
    lookup.env_override = None;
    lookup.setting = None;
    lookup.path_dirs.clear();
    lookup.known.clear();
    let bin = once_per_process(&NPM_GLOBAL_BIN, npm_global_bin)?;
    lookup.path_dirs.push(bin);
    resolve_claude_with(&lookup)
}

/// `npm prefix -g`'s answer, asked once per process: it can take seconds
/// (20 s cap) and a role start or status check resolves `claude` more than
/// once. A missing or failing npm is remembered too, so a restart is needed
/// after installing npm; a `claude` added to the npm folder later is still
/// found, since the folder is searched on every call.
static NPM_GLOBAL_BIN: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

fn once_per_process(
    cell: &std::sync::OnceLock<Option<PathBuf>>,
    lookup: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    cell.get_or_init(lookup).clone()
}

/// `npm prefix -g` → its bin folder. `npm` itself may only be in Homebrew
/// for a GUI app.
fn npm_global_bin() -> Option<PathBuf> {
    let npm = find_on_path(if cfg!(windows) { "npm.cmd" } else { "npm" })
        .or_else(|| first_file([PathBuf::from("/opt/homebrew/bin/npm")]))?;
    let (out, _err, ok) = run_cli(&npm, &["prefix", "-g"], &[]).ok()?;
    let prefix = out.trim();
    if !ok || prefix.is_empty() {
        return None;
    }
    let prefix = PathBuf::from(prefix);
    Some(if cfg!(windows) {
        prefix
    } else {
        prefix.join("bin")
    })
}

/// Arguments of the auth-status call (env comes from [`auth_status_env`]).
pub const AUTH_STATUS_ARGS: [&str; 3] = ["auth", "status", "--json"];

/// Env for `claude auth status` / `claude --version`: `CLAUDE_CONFIG_DIR`
/// from the resolver, plus no colour.
pub fn auth_status_env(config: &ConfigDirInfo) -> Vec<(String, String)> {
    let mut env = claude_env(config);
    env.push(("NO_COLOR".to_string(), "1".to_string()));
    env
}

/// `claude --version` → `2.1.236 (Claude Code)`.
pub fn read_claude_version(claude: &Path, config: &ConfigDirInfo) -> Option<String> {
    let (out, err, ok) = run_cli(claude, &["--version"], &auth_status_env(config)).ok()?;
    let text = if ok { out } else { err };
    let text = strip_ansi(text.trim());
    (!text.is_empty()).then(|| text.lines().next().unwrap_or("").trim().to_string())
}

/// Parse `claude auth status --json` (CLI 2.1.236 shape, see
/// `fixtures/claude/`). Falls back to the `--text` wording.
pub fn parse_claude_auth_status(stdout: &str, stderr: &str) -> LoginStatus {
    if let Ok(value) = serde_json::from_str::<Value>(stdout.trim()) {
        if let Some(logged_in) = value.get("loggedIn").and_then(Value::as_bool) {
            let text = |key: &str| {
                value
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|v| !v.is_empty() && *v != "none")
                    .map(str::to_string)
            };
            if !logged_in {
                return LoginStatus {
                    state: "loggedOut".into(),
                    ..Default::default()
                };
            }
            let method = match (text("authMethod"), text("subscriptionType")) {
                (Some(method), Some(plan)) => Some(format!("{method} · {plan}")),
                (Some(method), None) => Some(method),
                (None, Some(plan)) => Some(plan),
                (None, None) => None,
            };
            let organization = text("orgName");
            let account = text("email").or_else(|| organization.clone());
            return LoginStatus {
                state: "loggedIn".into(),
                account,
                method,
                organization,
                ..Default::default()
            };
        }
    }
    let text = strip_ansi(&format!("{stdout}\n{stderr}"));
    let lower = text.to_ascii_lowercase();
    if lower.contains("not logged in") || lower.contains("not authenticated") {
        return LoginStatus {
            state: "loggedOut".into(),
            ..Default::default()
        };
    }
    let detail: String = text.trim().chars().take(200).collect();
    LoginStatus {
        state: "unknown".into(),
        detail: (!detail.is_empty()).then_some(detail),
        ..Default::default()
    }
}

/// Signed-in state for the account in `config`. Never runs when the folder
/// is missing (Claude would create it).
pub fn claude_login_status(claude: Option<&Path>, config: &ConfigDirInfo) -> LoginStatus {
    let api_key_env = std::env::var_os("ANTHROPIC_API_KEY").is_some_and(|v| !v.is_empty());
    let mut status = match claude {
        None => LoginStatus {
            state: "noCli".into(),
            ..Default::default()
        },
        Some(_) if !config.exists => LoginStatus {
            state: "noConfigDir".into(),
            detail: config.missing_message(),
            ..Default::default()
        },
        Some(claude) => match run_cli(claude, &AUTH_STATUS_ARGS, &auth_status_env(config)) {
            // Exit 1 when signed out; the JSON is still on stdout.
            Ok((out, err, _ok)) => parse_claude_auth_status(&out, &err),
            Err(err) => LoginStatus {
                state: "unknown".into(),
                detail: Some(err.chars().take(200).collect()),
                ..Default::default()
            },
        },
    };
    status.api_key_env = api_key_env;
    status
}

#[cfg(test)]
mod tests {

    #[test]
    fn npms_global_folder_is_asked_once_per_process() {
        let cell = std::sync::OnceLock::new();
        let calls = std::cell::Cell::new(0);
        let lookup = || {
            calls.set(calls.get() + 1);
            Some(PathBuf::from("/npm/global/bin"))
        };
        assert_eq!(
            once_per_process(&cell, lookup),
            Some(PathBuf::from("/npm/global/bin"))
        );
        assert_eq!(
            once_per_process(&cell, lookup),
            Some(PathBuf::from("/npm/global/bin"))
        );
        assert_eq!(calls.get(), 1);
        // A failed lookup is remembered too: a hung npm costs one timeout.
        let missing = std::sync::OnceLock::new();
        let misses = std::cell::Cell::new(0);
        let fail = || {
            misses.set(misses.get() + 1);
            None
        };
        assert_eq!(once_per_process(&missing, fail), None);
        assert_eq!(once_per_process(&missing, fail), None);
        assert_eq!(misses.get(), 1);
    }
    use super::*;
    use crate::provider::claude_config::{resolve_with, CLAUDE_CONFIG_DIR};
    use std::time::{SystemTime, UNIX_EPOCH};

    const LOGGED_IN: &str = include_str!("../../../fixtures/claude/auth-status-logged-in.json");
    const LOGGED_OUT: &str = include_str!("../../../fixtures/claude/auth-status-logged-out.json");

    fn temp(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = crate::test_support::test_root().join(format!("dct_claude_detect_{tag}_{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "#!/bin/sh\n").unwrap();
    }

    #[test]
    fn claude_lookup_order_is_env_setting_path_known() {
        let root = temp("order");
        let env_bin = root.join("env").join("claude");
        let setting_bin = root.join("setting").join("claude");
        let path_dir = root.join("path");
        let known_bin = root.join("known").join(exe("claude"));
        touch(&env_bin);
        touch(&setting_bin);
        touch(&path_dir.join(exe("claude")));
        touch(&known_bin);
        let mut lookup = Lookup {
            env_override: Some(env_bin.display().to_string()),
            setting: Some(setting_bin.display().to_string()),
            path_dirs: vec![root.join("empty"), path_dir.clone()],
            known: vec![root.join("nope").join("claude"), known_bin.clone()],
        };
        assert_eq!(resolve_claude_with(&lookup), Some(env_bin.clone()));
        lookup.env_override = Some(root.join("missing").display().to_string());
        assert_eq!(resolve_claude_with(&lookup), Some(setting_bin));
        lookup.setting = None;
        assert_eq!(
            resolve_claude_with(&lookup),
            Some(path_dir.join(exe("claude")))
        );
        lookup.path_dirs.clear();
        assert_eq!(resolve_claude_with(&lookup), Some(known_bin));
        lookup.known.clear();
        assert_eq!(resolve_claude_with(&lookup), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn known_locations_include_homebrew_and_local_bin() {
        let known = known_claude_locations(Some(Path::new("/Users/jt")));
        #[cfg(not(windows))]
        assert_eq!(known[0], PathBuf::from("/opt/homebrew/bin/claude"));
        assert!(known.contains(&PathBuf::from("/Users/jt/.local/bin").join(exe("claude"))));
    }

    fn npm_claude_exe(npm_bin: &Path) -> PathBuf {
        npm_bin
            .join("node_modules")
            .join("@anthropic-ai")
            .join("claude-code")
            .join("bin")
            .join("claude.exe")
    }

    #[test]
    fn windows_cmd_on_path_resolves_to_the_npm_package_exe() {
        let root = temp("winpath");
        let npm_bin = root.join("Roaming").join("npm");
        let exe = npm_claude_exe(&npm_bin);
        touch(&exe);
        let cmd = npm_bin.join("claude.cmd");
        std::fs::write(
            &cmd,
            "@ECHO off\r\n\"%dp0%\\node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe\" %*\r\n",
        )
        .unwrap();
        let lookup = Lookup {
            path_dirs: vec![root.join("empty"), npm_bin],
            ..Lookup::default()
        };
        assert_eq!(resolve_claude_with(&lookup), Some(exe));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn windows_env_override_cmd_resolves_to_the_package_exe() {
        let root = temp("winenv");
        let npm_bin = root.join("npm");
        let exe = npm_claude_exe(&npm_bin);
        touch(&exe);
        let cmd = npm_bin.join("claude.CMD");
        std::fs::write(&cmd, "shim\r\n").unwrap();
        let lookup = Lookup {
            env_override: Some(cmd.display().to_string()),
            ..Lookup::default()
        };
        assert_eq!(resolve_claude_with(&lookup), Some(exe));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn windows_shim_text_resolves_when_the_exe_is_not_in_the_default_layout() {
        let root = temp("winparse");
        let npm_bin = root.join("npm");
        std::fs::create_dir_all(&npm_bin).unwrap();
        let exe = root.join("tools").join("claude.exe");
        touch(&exe);
        let cmd = npm_bin.join("claude.cmd");
        std::fs::write(
            &cmd,
            format!(
                "@ECHO off\r\n\"{}\" %*\r\n",
                exe.display().to_string().replace('/', "\\")
            ),
        )
        .unwrap();
        let resolved = unwrap_windows_shim(&cmd);
        assert_eq!(resolved.program, exe);
        assert!(resolved.prefix_args.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn windows_npm_bin_dirs_follow_appdata() {
        let dirs = windows_npm_bin_dirs(
            Some(Path::new(r"C:\Users\user\AppData\Roaming")),
            Some(Path::new(r"C:\Users\user\AppData\Local")),
        );
        assert_eq!(
            dirs[0],
            PathBuf::from(r"C:\Users\user\AppData\Roaming").join("npm")
        );
        assert_eq!(
            dirs[1],
            PathBuf::from(r"C:\Users\user\AppData\Local").join("npm")
        );
        assert_eq!(
            windows_cli_names("claude"),
            vec!["claude.exe", "claude.cmd", "claude.bat", "claude"]
        );
    }

    #[test]
    fn extensionless_npm_stub_resolves_to_the_package_exe() {
        let root = temp("winstub");
        let npm_bin = root.join("npm");
        let exe = npm_claude_exe(&npm_bin);
        touch(&exe);
        let stub = npm_bin.join("claude");
        std::fs::write(&stub, "#!/bin/sh\nexec \"$basedir/node_modules/@anthropic-ai/claude-code/bin/claude.exe\" \"$@\"\n").unwrap();
        let resolved = unwrap_shim_file(&stub).expect("stub");
        assert_eq!(resolved.program, exe);
        assert!(resolved.prefix_args.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn parses_logged_in_fixture() {
        let status = parse_claude_auth_status(LOGGED_IN, "");
        assert_eq!(status.state, "loggedIn");
        assert_eq!(status.account.as_deref(), Some("jt@example.com"));
        assert_eq!(status.organization.as_deref(), Some("Example Org"));
        assert_eq!(status.method.as_deref(), Some("claude.ai · team"));
    }

    #[test]
    fn parses_logged_out_fixture_and_text() {
        let status = parse_claude_auth_status(LOGGED_OUT, "");
        assert_eq!(status.state, "loggedOut");
        assert_eq!(status.account, None);
        assert_eq!(status.method, None);
        let text =
            parse_claude_auth_status("Not logged in. Run claude auth login to authenticate.", "");
        assert_eq!(text.state, "loggedOut");
        assert_eq!(parse_claude_auth_status("", "").state, "unknown");
    }

    #[test]
    fn auth_status_env_carries_the_resolved_config_dir() {
        let config = resolve_with(None, Some("/Users/jt/.claude-account2"), None);
        let env = auth_status_env(&config);
        assert!(env.contains(&(
            CLAUDE_CONFIG_DIR.to_string(),
            "/Users/jt/.claude-account2".to_string()
        )));
        assert_eq!(AUTH_STATUS_ARGS, ["auth", "status", "--json"]);
    }

    #[test]
    fn missing_config_dir_skips_the_auth_call() {
        let config = resolve_with(None, Some("/definitely/not/here/.claude-x"), None);
        // A program that would fail loudly if it were run.
        let status = claude_login_status(Some(Path::new("/definitely/not/a/claude")), &config);
        assert_eq!(status.state, "noConfigDir");
        assert!(status
            .detail
            .unwrap()
            .contains("Claude config folder not found"));
        assert!(!Path::new("/definitely/not/here/.claude-x").exists());
        assert_eq!(claude_login_status(None, &config).state, "noCli");
    }

    #[cfg(unix)]
    #[test]
    fn auth_status_runs_with_claude_config_dir_from_the_resolver() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp("fake");
        let config_dir = root.join("account2");
        std::fs::create_dir_all(&config_dir).unwrap();
        // Fake `claude`: echo the env it got as JSON.
        let fake = root.join("claude");
        std::fs::write(
            &fake,
            "#!/bin/sh\nprintf '{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"email\":\"%s\"}' \"$CLAUDE_CONFIG_DIR\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = resolve_with(None, Some(&config_dir.display().to_string()), None);
        let status = claude_login_status(Some(&fake), &config);
        assert_eq!(status.state, "loggedIn");
        assert_eq!(status.account.as_deref(), Some(config.path.as_str()));
        let _ = std::fs::remove_dir_all(root);
    }
}
