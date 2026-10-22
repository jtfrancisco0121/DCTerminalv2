//! The user's login-shell PATH for every child process.
//!
//! An app opened from the Dock or Finder gets launchd's PATH
//! (`/usr/bin:/bin:/usr/sbin:/sbin`), so `brew`, `gh`, `node`, `cargo` and the
//! rest are missing in terminal tabs. At startup the login shell is asked for
//! its PATH once (`$SHELL -l -i -c`, killed after a timeout) and the result is
//! merged with the inherited PATH and the usual install folders. Windows is
//! left alone. `DCTV2_SKIP_SHELL_ENV=1` turns the whole step off.
//!
//! The login shell's answer is cached in the data dir (`login-shell-path.json`).
//! A launch with a cache uses it at once and asks the shell again on a
//! background thread started from `setup`, which only rewrites the cache: a
//! tool installed since shows up one launch later. A shell that prints its
//! PATH and then hangs (a sleep in `.zlogout`, an exit trap) is killed after
//! the timeout, but the PATH it printed is still used. A slow rc file prints
//! nothing in time (`zsh -l -i -c` sources `.zshrc` before the command), so
//! that launch keeps the inherited PATH and the background refresh, with a
//! longer timeout, caches the answer for the next launch.

#[cfg(unix)]
use std::ffi::OsStr;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
const START_MARKER: &str = "__DCT_PATH_START__";
#[cfg(unix)]
const END_MARKER: &str = "__DCT_PATH_END__";
#[cfg(unix)]
const SHELL_TIMEOUT: Duration = Duration::from_secs(3);
/// The background refresh blocks nothing, so it can wait longer.
#[cfg(unix)]
const REFRESH_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(unix)]
const CACHE_FILE: &str = "login-shell-path.json";

/// Set by [`apply_login_path`] on the main thread, read by the refresh
/// thread: the shell and the PATH this process was started with (the refresh
/// must not feed the cached PATH back into the shell). Set only when a
/// refresh is needed (see [`refresh_for`]).
#[cfg(unix)]
static REFRESH: std::sync::OnceLock<Refresh> = std::sync::OnceLock::new();

#[cfg(unix)]
#[derive(Debug)]
struct Refresh {
    shell: PathBuf,
    inherited: String,
    cache: PathBuf,
}

/// The refresh's running shell, so app exit can stop it: see
/// [`stop_login_path_refresh`].
#[cfg(unix)]
static REFRESH_CHILD: std::sync::Mutex<RefreshChild> = std::sync::Mutex::new(RefreshChild::NEW);

#[cfg(unix)]
#[derive(Debug)]
struct RefreshChild {
    /// Set at app exit: a refresh that has not started its shell yet never does.
    stopped: bool,
    /// The shell's pid (it leads its own process group) and its temp file.
    running: Option<(i32, PathBuf)>,
}

#[cfg(unix)]
impl RefreshChild {
    const NEW: Self = Self {
        stopped: false,
        running: None,
    };
}

#[cfg(unix)]
extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}
#[cfg(unix)]
const SIGKILL: i32 = 9;

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct CachedPath {
    shell: String,
    path: String,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    LoginShell,
    Cache,
    Inherited,
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct Resolved {
    path: String,
    source: Source,
    /// The login shell's own PATH, before merging (what the cache holds).
    login: Option<String>,
}

/// Replace this process's PATH with the merged login-shell PATH.
///
/// Must run on the main thread before any other thread exists (the start of
/// `run()`): `set_var` is not safe while another thread may read the
/// environment. The shell is polled, not read on a helper thread, so this
/// function itself starts no threads. With a cached PATH the shell is not
/// run at all; [`spawn_login_path_refresh`] asks it later.
pub fn apply_login_path() {
    #[cfg(unix)]
    {
        if skip_requested(std::env::var_os("DCTV2_SKIP_SHELL_ENV").as_deref()) {
            eprintln!("DCTerminalv2: PATH left as inherited (DCTV2_SKIP_SHELL_ENV)");
            return;
        }
        let started = Instant::now();
        let inherited = std::env::var("PATH").unwrap_or_default();
        let shell = login_shell();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let cache = crate::data_dir::data_dir_before_tauri().map(|dir| dir.join(CACHE_FILE));
        let resolved = startup_path(
            &shell,
            &inherited,
            home.as_deref(),
            cache.as_deref(),
            SHELL_TIMEOUT,
        );
        let source = match resolved.source {
            Source::LoginShell => format!("login shell {}", shell.display()),
            Source::Cache => format!("cached login shell {} (refreshing)", shell.display()),
            Source::Inherited => format!("inherited (login shell {} failed)", shell.display()),
        };
        eprintln!(
            "DCTerminalv2: PATH from {source} in {} ms",
            started.elapsed().as_millis()
        );
        if let Some(refresh) = refresh_for(resolved.source, shell, inherited, cache) {
            let _ = REFRESH.set(refresh);
        }
        // Safety: called first thing in `run()` on the main thread, before
        // the Tauri builder or any other thread starts, so nothing can read
        // the environment concurrently.
        std::env::set_var("PATH", &resolved.path);
    }
}

/// Ask the login shell again, on a background thread, when this launch used
/// the cached PATH or the shell gave no answer in time. Only the cache file
/// is rewritten; this process's env is never touched. Call it after the
/// env-changing steps at the start of `run()` (from `setup`).
pub fn spawn_login_path_refresh() {
    #[cfg(unix)]
    {
        let Some(refresh) = REFRESH.get() else {
            return;
        };
        let spawned = std::thread::Builder::new()
            .name("login-path-refresh".into())
            .spawn(move || refresh_cache(refresh, REFRESH_TIMEOUT, &REFRESH_CHILD));
        if let Err(err) = spawned {
            eprintln!("DCTerminalv2: login PATH refresh not started: {err}");
        }
    }
}

/// App exit: kill the background refresh's shell (its whole process group)
/// and remove its temp file, so neither outlives the app. A refresh that has
/// not started its shell yet will not start one.
pub fn stop_login_path_refresh() {
    #[cfg(unix)]
    stop_refresh(&REFRESH_CHILD);
}

#[cfg(unix)]
fn stop_refresh(slot: &std::sync::Mutex<RefreshChild>) {
    let Ok(mut child) = slot.lock() else {
        return;
    };
    child.stopped = true;
    if let Some((pid, out_path)) = child.running.take() {
        // Safety: signals only the shell's own group; no Rust state. The
        // poll loop reaps it.
        unsafe { kill(-pid, SIGKILL) };
        let _ = std::fs::remove_file(out_path);
    }
}

/// The PATH for this launch: from the cache when it was written for this
/// shell, else from the shell (and then cached). Touches no env.
#[cfg(unix)]
fn startup_path(
    shell: &Path,
    inherited: &str,
    home: Option<&Path>,
    cache: Option<&Path>,
    timeout: Duration,
) -> Resolved {
    if let Some(login) = cache.and_then(|cache| read_cache(cache, shell)) {
        return Resolved {
            path: merge_path(Some(&login), inherited, &fallback_dirs(home), |dir| {
                dir.is_dir()
            }),
            source: Source::Cache,
            login: Some(login),
        };
    }
    let resolved = resolve(shell, inherited, home, timeout);
    if let (Some(cache), Some(login)) = (cache, resolved.login.as_deref()) {
        write_cache(cache, shell, login);
    }
    resolved
}

/// Whether this launch's PATH source calls for a background refresh, and
/// with what: a cache in use may be stale, and a shell that gave no answer
/// in time (a slow rc file) gets the longer refresh timeout, so the next
/// launch has a cache. A shell that answered already wrote the cache.
#[cfg(unix)]
fn refresh_for(
    source: Source,
    shell: PathBuf,
    inherited: String,
    cache: Option<PathBuf>,
) -> Option<Refresh> {
    match (source, cache) {
        (Source::Cache | Source::Inherited, Some(cache)) => Some(Refresh {
            shell,
            inherited,
            cache,
        }),
        _ => None,
    }
}

#[cfg(unix)]
fn refresh_cache(refresh: &Refresh, timeout: Duration, slot: &std::sync::Mutex<RefreshChild>) {
    match shell_path(
        &refresh.shell,
        Some(&refresh.inherited),
        timeout,
        Some(slot),
    ) {
        Some(login) => write_cache(&refresh.cache, &refresh.shell, &login),
        // Keep the old cache: one slow or failed run says little.
        None => eprintln!(
            "DCTerminalv2: login shell {} did not report a PATH; cache kept",
            refresh.shell.display()
        ),
    }
}

/// The cached login PATH, when it was written for `shell`.
#[cfg(unix)]
fn read_cache(cache: &Path, shell: &Path) -> Option<String> {
    let cached: CachedPath = serde_json::from_slice(&std::fs::read(cache).ok()?).ok()?;
    (Path::new(&cached.shell) == shell && !cached.path.trim().is_empty()).then_some(cached.path)
}

/// Best effort: a failed write only means the next launch asks the shell.
#[cfg(unix)]
fn write_cache(cache: &Path, shell: &Path, login: &str) {
    let cached = CachedPath {
        shell: shell.display().to_string(),
        path: login.to_string(),
    };
    let Ok(body) = serde_json::to_vec_pretty(&cached) else {
        return;
    };
    let Some(dir) = cache.parent() else {
        return;
    };
    let tmp = cache.with_extension("json.tmp");
    let written = std::fs::create_dir_all(dir)
        .and_then(|_| std::fs::write(&tmp, body))
        .and_then(|_| std::fs::rename(&tmp, cache));
    if let Err(err) = written {
        let _ = std::fs::remove_file(&tmp);
        eprintln!("DCTerminalv2: login PATH not cached: {err}");
    }
}

/// Any of these means we were started inside a Claude Code session.
const SESSION_MARKERS: [&str; 4] = [
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_ENTRYPOINT",
];

/// `CLAUDE*` settings that belong to the user, not to the parent session.
/// Every terminal sets or drops `CLAUDE_CONFIG_DIR` itself (its window's
/// account), so a kept inherited value never picks a tab's account.
const KEEP_CLAUDE_VARS: [&str; 2] = ["CLAUDE_CONFIG_DIR", "CLAUDE_CODE_GIT_BASH_PATH"];

/// Whether the parent Claude Code session set this variable for its own
/// tools: any `CLAUDE*` name the user does not own, `AI_AGENT`, and
/// `GIT_EDITOR=true` (it makes a plain `git commit` abort). Any other
/// `GIT_EDITOR` is the user's own editor and stays.
fn set_by_parent_session(name: &str, value: &str) -> bool {
    match name {
        "AI_AGENT" => true,
        "GIT_EDITOR" => value == "true",
        _ => name.starts_with("CLAUDE") && !KEEP_CLAUDE_VARS.contains(&name),
    }
}

/// Names of the variables a parent Claude Code session put in our env.
///
/// Started from a Claude Code terminal (`npm run dev:app` run by Claude),
/// DCTerminalv2 inherits that session's `CLAUDECODE`, `CLAUDE_CODE_*`,
/// `AI_AGENT`, `GIT_EDITOR=true` and friends, and every child gets them. A
/// `claude` started under them takes itself for a nested session and writes
/// no session log, and `git commit` in a shell tab aborts. Takes
/// `(name, value)` pairs. Returns nothing unless a marker shows we really are
/// inside a session, and never `CLAUDE_CONFIG_DIR` (the user's account
/// choice), `CLAUDE_CODE_GIT_BASH_PATH` (install setup), a `GIT_EDITOR` other
/// than exactly `true`, or any `ANTHROPIC_*`.
pub(crate) fn parent_claude_session_vars<'a>(
    vars: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Vec<String> {
    let vars: Vec<(&str, &str)> = vars.into_iter().collect();
    if !vars.iter().any(|(name, _)| SESSION_MARKERS.contains(name)) {
        return Vec::new();
    }
    vars.into_iter()
        .filter(|(name, value)| set_by_parent_session(name, value))
        .map(|(name, _)| name.to_string())
        .collect()
}

/// Remove the parent Claude Code session's variables from this process, so
/// no child (PTY shell, Claude, CLI probe) inherits them. Logs the count,
/// never the values: some are secrets.
///
/// Must run on the main thread before any other thread exists, like
/// [`apply_login_path`].
pub fn scrub_parent_claude_session() {
    let vars: Vec<(String, String)> = std::env::vars_os()
        .filter_map(|(name, value)| {
            let name = name.into_string().ok()?;
            Some((name, value.to_string_lossy().into_owned()))
        })
        .collect();
    let scrubbed = parent_claude_session_vars(
        vars.iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
    );
    if scrubbed.is_empty() {
        return;
    }
    remove_vars(&scrubbed);
    eprintln!(
        "DCTerminalv2: removed {} variables inherited from a parent Claude Code session",
        scrubbed.len()
    );
}

fn remove_vars(names: &[String]) {
    for name in names {
        std::env::remove_var(name);
    }
}

#[cfg(unix)]
fn skip_requested(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| {
        let value = value.to_string_lossy();
        let value = value.trim();
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

#[cfg(unix)]
fn login_shell() -> PathBuf {
    std::env::var_os("SHELL")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(if cfg!(target_os = "macos") {
                "/bin/zsh"
            } else {
                "/bin/sh"
            })
        })
}

#[cfg(unix)]
fn resolve(shell: &Path, inherited: &str, home: Option<&Path>, timeout: Duration) -> Resolved {
    let login = shell_path(shell, None, timeout, None);
    let source = if login.is_some() {
        Source::LoginShell
    } else {
        Source::Inherited
    };
    let path = merge_path(login.as_deref(), inherited, &fallback_dirs(home), |dir| {
        dir.is_dir()
    });
    Resolved {
        path,
        source,
        login,
    }
}

/// Folders tools are commonly installed to, added when they exist.
#[cfg(unix)]
fn fallback_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = ["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin"]
        .into_iter()
        .map(PathBuf::from)
        .collect();
    if let Some(home) = home {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".cargo/bin"));
    }
    dirs
}

/// Login PATH first, then the inherited PATH, then fallback folders that
/// exist. Empty entries and duplicates are dropped; first position wins.
#[cfg(unix)]
fn merge_path(
    login: Option<&str>,
    inherited: &str,
    fallbacks: &[PathBuf],
    exists: impl Fn(&Path) -> bool,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut push = |dir: &str| {
        if !dir.is_empty() && !parts.iter().any(|part| part == dir) {
            parts.push(dir.to_string());
        }
    };
    for dir in login.unwrap_or_default().split(':') {
        push(dir);
    }
    for dir in inherited.split(':') {
        push(dir);
    }
    for dir in fallbacks {
        if exists(dir) {
            if let Some(dir) = dir.to_str() {
                push(dir);
            }
        }
    }
    parts.join(":")
}

/// The text between the markers, trimmed. rc files may print around it.
#[cfg(unix)]
fn parse_marked(output: &str) -> Option<String> {
    let start = output.find(START_MARKER)? + START_MARKER.len();
    let rest = &output[start..];
    let end = rest.find(END_MARKER)?;
    let path = rest[..end].trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// Run the login shell and read its PATH; `None` on failure, or on a timeout
/// before the shell printed both markers. `path_env` replaces the PATH the
/// shell starts with (the refresh passes the PATH this process inherited).
/// With `slot`, the running shell is recorded there so [`stop_refresh`] can
/// kill it, and no shell starts once the slot is stopped.
///
/// stdout goes to a temp file rather than a pipe: no reader thread is needed,
/// a chatty rc file cannot fill a pipe and stall, and a background job the
/// rc file starts cannot hold the read end open. The shell runs in its own
/// session (no controlling terminal), like a Dock launch, so an interactive
/// shell started from a dev terminal cannot take over that terminal.
#[cfg(unix)]
fn shell_path(
    shell: &Path,
    path_env: Option<&str>,
    timeout: Duration,
    slot: Option<&std::sync::Mutex<RefreshChild>>,
) -> Option<String> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    extern "C" {
        fn setsid() -> i32;
    }

    let out_path = std::env::temp_dir().join(format!(
        "dct-shell-env-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&out_path)
        .ok()?;
    // `printenv` prints the exported, colon-joined PATH in every shell,
    // fish included; `;` separates commands in all of them.
    let script = format!("echo {START_MARKER}; /usr/bin/env printenv PATH; echo {END_MARKER}");
    let mut command = Command::new(shell);
    command
        .args(["-l", "-i", "-c", &script])
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::null());
    if let Some(path) = path_env {
        command.env("PATH", path);
    }
    // Safety: `setsid` is async-signal-safe and touches no Rust state.
    unsafe {
        command.pre_exec(|| {
            setsid();
            Ok(())
        });
    }
    // The slot stays locked from the stopped check to the record, and while
    // each poll reaps, so a stop never signals a reaped (reusable) pid.
    let lock = || slot.and_then(|slot| slot.lock().ok());
    let mut guard = lock();
    if guard.as_ref().is_some_and(|child| child.stopped) {
        let _ = std::fs::remove_file(&out_path);
        return None;
    }
    let spawned = command.spawn();
    if let (Some(child), Ok(spawned)) = (guard.as_mut(), spawned.as_ref()) {
        if let Ok(pid) = i32::try_from(spawned.id()) {
            child.running = Some((pid, out_path.clone()));
        }
    }
    drop(guard);
    // Whether the shell started; after a timeout the file still holds
    // whatever it printed, often the whole PATH (a shell that hangs after
    // the command ran: a sleep in `.zlogout`, an exit trap). A slow rc file
    // runs before the command, so it prints nothing in time.
    let started = match spawned {
        Ok(mut child) => {
            let deadline = Instant::now() + timeout;
            loop {
                let mut guard = lock();
                match child.try_wait() {
                    Ok(None) if Instant::now() < deadline => {
                        drop(guard);
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Ok(Some(_)) => {}
                    _ => {
                        // The shell leads its own process group (setsid), so
                        // this also stops anything the rc file started.
                        if let Ok(pid) = i32::try_from(child.id()) {
                            // Safety: signals only that group; no Rust state.
                            unsafe { kill(-pid, SIGKILL) };
                        }
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                }
                if let Some(slot) = guard.as_mut() {
                    slot.running = None;
                }
                break true;
            }
        }
        Err(_) => false,
    };
    let text = started
        .then(|| std::fs::read(&out_path).ok())
        .flatten()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    let _ = std::fs::remove_file(&out_path);
    parse_marked(&text?)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::os::unix::fs::PermissionsExt;

    fn fake_shell(dir: &TempDir, body: &str) -> PathBuf {
        let path = dir.join("fake-shell");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn parses_only_the_text_between_markers() {
        let output = format!(
            "Welcome to zsh!\nlast login: today\n{START_MARKER}\n/opt/homebrew/bin:/usr/bin\n{END_MARKER}\nbye\n"
        );
        assert_eq!(
            parse_marked(&output).as_deref(),
            Some("/opt/homebrew/bin:/usr/bin")
        );
    }

    #[test]
    fn missing_or_empty_markers_give_none() {
        assert_eq!(parse_marked("/usr/bin:/bin\n"), None);
        assert_eq!(parse_marked(&format!("{START_MARKER}\n/usr/bin\n")), None);
        assert_eq!(parse_marked(&format!("/usr/bin\n{END_MARKER}\n")), None);
        assert_eq!(
            parse_marked(&format!("{START_MARKER}\n\n{END_MARKER}")),
            None
        );
    }

    #[test]
    fn merge_puts_login_first_then_inherited_then_existing_fallbacks() {
        let fallbacks = fallback_dirs(Some(Path::new("/home/me")));
        let exists = |dir: &Path| {
            dir == Path::new("/opt/homebrew/bin")
                || dir == Path::new("/home/me/.cargo/bin")
                || dir == Path::new("/usr/local/bin")
        };
        let merged = merge_path(
            Some("/Users/me/.nvm/bin:/opt/homebrew/bin:/usr/bin"),
            "/usr/bin:/bin::/usr/sbin:/sbin",
            &fallbacks,
            exists,
        );
        assert_eq!(
            merged,
            "/Users/me/.nvm/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin:/home/me/.cargo/bin"
        );
    }

    #[test]
    fn merge_without_login_keeps_inherited_and_adds_fallbacks() {
        let fallbacks = fallback_dirs(Some(Path::new("/home/me")));
        let merged = merge_path(None, "/usr/bin:/bin", &fallbacks, |dir: &Path| {
            dir == Path::new("/home/me/.local/bin")
        });
        assert_eq!(merged, "/usr/bin:/bin:/home/me/.local/bin");
    }

    #[test]
    fn fake_shell_path_comes_first() {
        let dir = TempDir::new("shell_env_ok");
        let shell = fake_shell(
            &dir,
            &format!("echo 'rc noise'\necho {START_MARKER}\necho /fake/login/bin:/usr/bin\necho {END_MARKER}\necho 'more noise'"),
        );
        let resolved = resolve(&shell, "/usr/bin:/bin", None, Duration::from_secs(5));
        assert_eq!(resolved.source, Source::LoginShell);
        assert!(
            resolved.path.starts_with("/fake/login/bin:/usr/bin:/bin"),
            "{}",
            resolved.path
        );
    }

    #[test]
    fn real_sh_reports_its_path_through_the_script() {
        let resolved = shell_path(Path::new("/bin/sh"), None, Duration::from_secs(5), None);
        assert!(resolved.is_some_and(|path| !path.is_empty()));
    }

    #[test]
    fn slow_shell_is_killed_and_inherited_is_used() {
        let dir = TempDir::new("shell_env_slow");
        let shell = fake_shell(
            &dir,
            &format!("echo {START_MARKER}\nsleep 30\necho /never:{END_MARKER}"),
        );
        let started = Instant::now();
        let resolved = resolve(&shell, "/usr/bin:/bin", None, Duration::from_millis(300));
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(resolved.source, Source::Inherited);
        assert!(
            resolved.path.starts_with("/usr/bin:/bin"),
            "{}",
            resolved.path
        );
        assert!(!resolved.path.contains("/never"));
    }

    #[test]
    fn a_timed_out_shell_still_yields_its_printed_path() {
        let dir = TempDir::new("shell_env_printed_then_hung");
        let shell = fake_shell(
            &dir,
            &format!(
                "echo {START_MARKER}\necho /printed/bin:/usr/bin\necho {END_MARKER}\nsleep 30"
            ),
        );
        // Long enough for a freshly written script's first launch (macOS
        // checks a new executable before it runs), far below the sleep.
        let timeout = Duration::from_secs(2);
        let started = Instant::now();
        let resolved = resolve(&shell, "/usr/bin:/bin", None, timeout);
        assert!(
            started.elapsed() < timeout + Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(resolved.source, Source::LoginShell);
        assert!(
            resolved.path.starts_with("/printed/bin:/usr/bin:/bin"),
            "{}",
            resolved.path
        );
    }

    #[test]
    fn a_cached_path_is_used_without_running_the_shell() {
        let dir = TempDir::new("shell_env_cached");
        let shell = fake_shell(
            &dir,
            &format!("sleep 30\necho {START_MARKER}\necho /never\necho {END_MARKER}"),
        );
        let cache = dir.join("data").join(CACHE_FILE);
        write_cache(&cache, &shell, "/cached/bin:/usr/bin");
        let started = Instant::now();
        let resolved = startup_path(
            &shell,
            "/usr/bin:/bin",
            None,
            Some(&cache),
            Duration::from_secs(5),
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(resolved.source, Source::Cache);
        assert!(
            resolved.path.starts_with("/cached/bin:/usr/bin:/bin"),
            "{}",
            resolved.path
        );
    }

    #[test]
    fn without_a_cache_for_this_shell_the_shell_runs_and_its_answer_is_cached() {
        let dir = TempDir::new("shell_env_cache_miss");
        let shell = fake_shell(
            &dir,
            &format!("echo {START_MARKER}\necho /fresh/bin\necho {END_MARKER}"),
        );
        let cache = dir.join("data").join(CACHE_FILE);
        write_cache(&cache, Path::new("/bin/other-shell"), "/stale/bin");
        let resolved = startup_path(
            &shell,
            "/usr/bin",
            None,
            Some(&cache),
            Duration::from_secs(5),
        );
        assert_eq!(resolved.source, Source::LoginShell);
        assert!(
            resolved.path.starts_with("/fresh/bin:/usr/bin"),
            "{}",
            resolved.path
        );
        assert_eq!(read_cache(&cache, &shell).as_deref(), Some("/fresh/bin"));
        assert_eq!(read_cache(&cache, Path::new("/bin/other-shell")), None);
    }

    #[test]
    fn the_refresh_asks_the_shell_with_the_inherited_path_and_keeps_the_cache_on_failure() {
        let dir = TempDir::new("shell_env_refresh");
        // Prints the PATH it was started with, so the test can see it.
        let shell = fake_shell(
            &dir,
            &format!("echo {START_MARKER}\necho \"$PATH\"\necho {END_MARKER}"),
        );
        let cache = dir.join("data").join(CACHE_FILE);
        write_cache(&cache, &shell, "/old/bin");
        let refresh = Refresh {
            shell: shell.clone(),
            inherited: "/inherited/bin:/usr/bin:/bin".into(),
            cache: cache.clone(),
        };
        refresh_cache(
            &refresh,
            Duration::from_secs(5),
            &std::sync::Mutex::new(RefreshChild::NEW),
        );
        assert_eq!(
            read_cache(&cache, &shell).as_deref(),
            Some("/inherited/bin:/usr/bin:/bin")
        );

        let broken = Refresh {
            shell: dir.join("no-such-shell"),
            inherited: "/usr/bin".into(),
            cache: cache.clone(),
        };
        write_cache(&cache, &broken.shell, "/kept/bin");
        refresh_cache(
            &broken,
            Duration::from_secs(1),
            &std::sync::Mutex::new(RefreshChild::NEW),
        );
        assert_eq!(
            read_cache(&cache, &broken.shell).as_deref(),
            Some("/kept/bin")
        );
    }

    #[test]
    fn a_shell_too_slow_to_answer_is_asked_again_and_cached_for_the_next_launch() {
        let dir = TempDir::new("shell_env_slow_rc");
        // A slow rc file: `zsh -l -i -c` sources .zshrc before the command,
        // so nothing is printed before the startup timeout.
        let shell = fake_shell(
            &dir,
            &format!("sleep 1\necho {START_MARKER}\necho /rc/bin:/usr/bin\necho {END_MARKER}"),
        );
        let cache = dir.join("data").join(CACHE_FILE);
        let first = startup_path(
            &shell,
            "/usr/bin:/bin",
            None,
            Some(&cache),
            Duration::from_millis(300),
        );
        assert_eq!(first.source, Source::Inherited);
        assert_eq!(read_cache(&cache, &shell), None);

        let refresh = refresh_for(
            first.source,
            shell.clone(),
            "/usr/bin:/bin".into(),
            Some(cache.clone()),
        )
        .expect("an unanswered startup shell arms the refresh");
        refresh_cache(
            &refresh,
            Duration::from_secs(10),
            &std::sync::Mutex::new(RefreshChild::NEW),
        );

        let next = startup_path(
            &shell,
            "/usr/bin:/bin",
            None,
            Some(&cache),
            Duration::from_millis(300),
        );
        assert_eq!(next.source, Source::Cache);
        assert!(
            next.path.starts_with("/rc/bin:/usr/bin:/bin"),
            "{}",
            next.path
        );

        // A shell that answered at startup already wrote the cache; no data
        // dir means nowhere to cache.
        assert!(refresh_for(
            Source::LoginShell,
            shell.clone(),
            String::new(),
            Some(cache.clone())
        )
        .is_none());
        assert!(refresh_for(Source::Inherited, shell, String::new(), None).is_none());
    }

    #[test]
    fn stopping_the_refresh_kills_its_shell_and_removes_its_temp_file() {
        let dir = TempDir::new("shell_env_refresh_stop");
        let finished = dir.join("finished");
        let shell = fake_shell(
            &dir,
            &format!(
                "sleep 1\ntouch '{}'\necho {START_MARKER}\necho /late/bin\necho {END_MARKER}",
                finished.display()
            ),
        );
        let cache = dir.join("data").join(CACHE_FILE);
        let refresh = Refresh {
            shell: shell.clone(),
            inherited: "/usr/bin:/bin".into(),
            cache: cache.clone(),
        };
        let slot = std::sync::Mutex::new(RefreshChild::NEW);
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| refresh_cache(&refresh, Duration::from_secs(15), &slot));
            let deadline = Instant::now() + Duration::from_secs(5);
            let (pid, tmp) = loop {
                if let Some(running) = slot.lock().unwrap().running.clone() {
                    break running;
                }
                assert!(
                    Instant::now() < deadline,
                    "the refresh never started its shell"
                );
                std::thread::sleep(Duration::from_millis(10));
            };
            assert!(tmp.exists());
            let stopped = Instant::now();
            stop_refresh(&slot);
            // Safety: signal 0 only checks that the pid exists.
            while unsafe { kill(pid, 0) } == 0 && stopped.elapsed() < Duration::from_secs(1) {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(
                stopped.elapsed() < Duration::from_secs(1),
                "the refresh shell still runs after stop"
            );
            assert!(!tmp.exists(), "the refresh left its temp file behind");
            worker.join().unwrap();
        });
        std::thread::sleep(Duration::from_millis(1500));
        assert!(!finished.exists(), "the killed shell's group kept running");
        assert_eq!(read_cache(&cache, &shell), None);

        // A refresh that starts after the stop runs no shell.
        let started = Instant::now();
        refresh_cache(&refresh, Duration::from_secs(15), &slot);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(read_cache(&cache, &shell), None);
    }

    #[test]
    fn missing_shell_falls_back_to_inherited() {
        let resolved = resolve(
            Path::new("/no/such/shell"),
            "/usr/bin:/bin",
            None,
            Duration::from_secs(1),
        );
        assert_eq!(resolved.source, Source::Inherited);
        assert!(resolved.path.starts_with("/usr/bin:/bin"));
    }

    #[test]
    fn a_scrubbed_name_does_not_reach_the_child() {
        use std::process::Command;

        // The parent env is built here, not read from or written to this
        // process, so parallel tests and a surrounding Claude session see
        // nothing. Values are never printed.
        const PROBE: &str = "CLAUDE_DCT_SCRUB_TEST_PROBE";
        let parent = [
            ("PATH", "/usr/bin:/bin"),
            ("CLAUDECODE", "1"),
            (PROBE, "from the parent session"),
            ("CLAUDE_CONFIG_DIR", "/tmp/claude-config"),
            ("AI_AGENT", "claude-code"),
            ("GIT_EDITOR", "true"),
        ];
        let mut command = Command::new("/bin/sh");
        command.env_clear().envs(parent);
        for name in parent_claude_session_vars(parent) {
            command.env_remove(name);
        }
        let output = command
            .args([
                "-c",
                &format!(
                    "printf '%s|%s|%s|%s|%s' \"${{{PROBE}-unset}}\" \"${{CLAUDECODE-unset}}\" \"${{AI_AGENT-unset}}\" \"${{GIT_EDITOR-unset}}\" \"${{CLAUDE_CONFIG_DIR-unset}}\""
                ),
            ])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "unset|unset|unset|unset|/tmp/claude-config"
        );
    }

    #[test]
    fn opt_out_values() {
        assert!(!skip_requested(None));
        assert!(!skip_requested(Some(OsStr::new(""))));
        assert!(!skip_requested(Some(OsStr::new("0"))));
        assert!(!skip_requested(Some(OsStr::new("false"))));
        assert!(skip_requested(Some(OsStr::new("1"))));
        assert!(skip_requested(Some(OsStr::new("yes"))));
    }
}

#[cfg(test)]
mod scrub_tests {
    use super::parent_claude_session_vars;

    fn listed(vars: &[(&str, &str)]) -> Vec<String> {
        let mut listed = parent_claude_session_vars(vars.iter().copied());
        listed.sort();
        listed
    }

    #[test]
    fn parent_session_vars_are_listed_only_inside_a_claude_session() {
        let outside = [
            ("CLAUDE_CONFIG_DIR", "/me/.claude"),
            ("CLAUDE_EFFORT", "high"),
            ("AI_AGENT", "claude-code"),
            ("GIT_EDITOR", "true"),
            ("PATH", "/usr/bin"),
            ("HOME", "/me"),
        ];
        assert!(listed(&outside).is_empty());

        for marker in [
            "CLAUDECODE",
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_ENTRYPOINT",
        ] {
            assert_eq!(
                listed(&[(marker, "1"), ("PATH", "/usr/bin")]),
                vec![marker.to_string()],
                "marker {marker}"
            );
        }

        let inside = [
            ("PATH", "/usr/bin"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_CHILD_SESSION", "1"),
            ("CLAUDE_CODE_SESSION_ID", "id"),
            ("CLAUDE_CODE_ENTRYPOINT", "cli"),
            ("CLAUDE_CODE_EXECPATH", "/bin/claude"),
            ("CLAUDE_CODE_MESSAGING_SOCKET", "/tmp/sock"),
            ("CLAUDE_CODE_MESSAGING_TOKEN", "secret"),
            ("CLAUDE_CODE_SESSION_ATTENDED", "1"),
            ("CLAUDE_EFFORT", "high"),
            ("CLAUDE_PID", "1"),
            ("CLAUDE_CONFIG_DIR", "/me/.claude"),
            ("CLAUDE_CODE_GIT_BASH_PATH", "C:/git/bash.exe"),
            ("ANTHROPIC_API_KEY", "key"),
            ("ANTHROPIC_BASE_URL", "https://example.test"),
            ("AI_AGENT", "claude-code"),
            ("GIT_EDITOR", "true"),
            ("NoDefaultCurrentDirectoryInExePath", "1"),
            ("HOME", "/me"),
        ];
        assert_eq!(
            listed(&inside),
            [
                "AI_AGENT",
                "CLAUDECODE",
                "CLAUDE_CODE_CHILD_SESSION",
                "CLAUDE_CODE_ENTRYPOINT",
                "CLAUDE_CODE_EXECPATH",
                "CLAUDE_CODE_MESSAGING_SOCKET",
                "CLAUDE_CODE_MESSAGING_TOKEN",
                "CLAUDE_CODE_SESSION_ATTENDED",
                "CLAUDE_CODE_SESSION_ID",
                "CLAUDE_EFFORT",
                "CLAUDE_PID",
                "GIT_EDITOR",
            ]
        );
    }

    #[test]
    fn a_real_git_editor_is_kept_inside_a_claude_session() {
        for editor in ["vim", "code --wait", "TRUE", " true", "true "] {
            assert_eq!(
                listed(&[("CLAUDECODE", "1"), ("GIT_EDITOR", editor)]),
                ["CLAUDECODE"],
                "GIT_EDITOR {editor:?}"
            );
        }
    }
}
