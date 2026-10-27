//! Test-only helpers shared by the coverage tests. Nothing here touches the
//! real home folder, `~/.claude*` or DCTerminalv2's app data.
//!
//! Every test temp folder lives under one per-process run folder,
//! `temp_dir()/dcterminalv2-tests/run-<pid>-<start-nanos>/`. On first use, run
//! folders left by test processes that have exited are deleted, so a test run
//! leaves at most its own run folder behind.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

/// `ok()` within about 2 s. A lock dropped by one test can look held for a
/// moment: a child another test thread is forking shares the open file
/// (and its `flock`) until its `exec` closes it (O_CLOEXEC). Production
/// never re-takes a lock that fast; tests that do wait for that window.
pub fn eventually(mut ok: impl FnMut() -> bool) -> bool {
    for _ in 0..100 {
        if ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Folder under `temp_dir()` that holds every test run folder.
const TESTS_DIR: &str = "dcterminalv2-tests";
/// Run folders whose owner cannot be checked are swept after this long.
const STALE_UNKNOWN: Duration = Duration::from_secs(60 * 60);
/// Run folders this old are swept even when their pid looks alive (pid reuse).
const STALE_ALWAYS: Duration = Duration::from_secs(24 * 60 * 60);

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

/// This test process's run folder. Created, and stale sibling runs swept, on
/// first use.
pub fn test_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let base = std::env::temp_dir().join(TESTS_DIR);
        let own = format!("run-{}-{}", std::process::id(), now_nanos());
        sweep_stale_runs(&base, &own);
        let root = base.join(own);
        std::fs::create_dir_all(&root).expect("create test run dir");
        root
    })
}

/// A fresh, unique, created folder under the run folder. Not removed on drop;
/// a later test run sweeps it with the rest of this run.
pub fn temp_path(prefix: &str) -> PathBuf {
    let path = test_root().join(format!(
        "{prefix}_{}_{}",
        NEXT.fetch_add(1, Ordering::SeqCst),
        now_nanos()
    ));
    std::fs::create_dir_all(&path).expect("create temp path");
    path
}

/// Delete `run-<pid>-<nanos>` folders directly inside `base` whose process has
/// exited (or, when that cannot be told, that are older than an hour). Never
/// follows symlinks and never touches anything outside `base`. Returns the
/// folders it removed.
pub fn sweep_stale_runs(base: &Path, own: &str) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let Ok(entries) = std::fs::read_dir(base) else {
        return removed;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == own {
            continue;
        }
        let Some(pid) = run_pid(&name) else {
            continue;
        };
        let path = base.join(&name);
        // Only real folders directly inside `base`, never a symlink.
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.is_dir() || path.parent() != Some(base) {
            continue;
        }
        let age = meta
            .modified()
            .ok()
            .and_then(|time| SystemTime::now().duration_since(time).ok())
            .unwrap_or_default();
        let stale = match pid_alive(pid) {
            Some(true) => age > STALE_ALWAYS,
            Some(false) => true,
            None => age > STALE_UNKNOWN,
        };
        if stale && std::fs::remove_dir_all(&path).is_ok() {
            removed.push(path);
        }
    }
    removed
}

/// The pid in a `run-<pid>-<nanos>` folder name.
fn run_pid(name: &str) -> Option<u32> {
    let (pid, nanos) = name.strip_prefix("run-")?.split_once('-')?;
    nanos.parse::<u128>().ok()?;
    pid.parse().ok()
}

/// Whether `pid` is a running process; `None` when that cannot be told.
#[cfg(unix)]
pub fn pid_alive(pid: u32) -> Option<bool> {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const EPERM: i32 = 1;
    const ESRCH: i32 = 3;
    let pid = i32::try_from(pid).ok().filter(|pid| *pid > 0)?;
    // SAFETY: signal 0 only checks that the process exists; nothing is sent.
    if unsafe { kill(pid, 0) } == 0 {
        return Some(true);
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(ESRCH) => Some(false),
        Some(EPERM) => Some(true),
        _ => None,
    }
}

#[cfg(not(unix))]
pub fn pid_alive(_pid: u32) -> Option<bool> {
    None
}

/// A fresh folder under the test run folder, removed on drop.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let path = test_root().join(format!(
            "dct_cov_{tag}_{}_{}",
            NEXT.fetch_add(1, Ordering::SeqCst),
            now_nanos()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, part: impl AsRef<Path>) -> PathBuf {
        self.path.join(part)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Names of the entries directly inside `dir`, sorted.
pub fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_names_parse_only_the_expected_shape() {
        assert_eq!(run_pid("run-42-1700000000"), Some(42));
        assert_eq!(run_pid("run-42"), None);
        assert_eq!(run_pid("run-x-1"), None);
        assert_eq!(run_pid("run-42-abc"), None);
        assert_eq!(run_pid("dct_cov_42_1"), None);
    }

    #[cfg(unix)]
    #[test]
    fn sweep_removes_dead_runs_only_inside_its_root() {
        let outer = TempDir::new("sweep");
        let base = outer.join("dcterminalv2-tests");
        // A pid that has exited: spawn a process and reap it.
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead_pid = child.id();
        child.wait().unwrap();
        assert_eq!(pid_alive(dead_pid), Some(false));
        assert_eq!(pid_alive(std::process::id()), Some(true));

        let me = std::process::id();
        let own = format!("run-{me}-1");
        let dead = base.join(format!("run-{dead_pid}-5"));
        let live = base.join(format!("run-{me}-2"));
        let stranger = base.join("not-a-run");
        let odd = base.join(format!("run-{dead_pid}-oops"));
        for dir in [&base.join(&own), &dead, &live, &stranger, &odd] {
            std::fs::create_dir_all(dir.join("inner")).unwrap();
        }
        // A run-named file, and a run-named symlink to a folder outside `base`.
        let file = base.join(format!("run-{dead_pid}-6"));
        std::fs::write(&file, "x").unwrap();
        let outside = outer.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep.txt"), "keep").unwrap();
        let link = base.join(format!("run-{dead_pid}-7"));
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        let removed = sweep_stale_runs(&base, &own);
        assert_eq!(removed, vec![dead.clone()]);
        assert!(!dead.exists());
        assert!(base.join(&own).join("inner").is_dir());
        assert!(live.is_dir());
        assert!(stranger.is_dir());
        assert!(odd.is_dir());
        assert!(file.is_file());
        assert!(outside.join("keep.txt").is_file());
        assert!(std::fs::symlink_metadata(&link).is_ok());
        assert_eq!(
            names_in(outer.path()),
            vec!["dcterminalv2-tests", "outside"]
        );
    }

    #[test]
    fn a_missing_base_sweeps_nothing() {
        let outer = TempDir::new("sweep_missing");
        assert!(sweep_stale_runs(&outer.join("nope"), "run-1-1").is_empty());
    }

    #[test]
    fn temp_folders_live_under_this_runs_folder() {
        let root = test_root();
        let base = std::env::temp_dir().join(TESTS_DIR);
        assert_eq!(root.parent(), Some(base.as_path()));
        let name = root.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(run_pid(&name), Some(std::process::id()));
        let a = temp_path("tp");
        let b = temp_path("tp");
        assert_ne!(a, b);
        assert!(a.is_dir() && a.starts_with(root));
        let dir = TempDir::new("tp");
        assert!(dir.path().starts_with(root));
        let _ = std::fs::remove_dir_all(a);
        let _ = std::fs::remove_dir_all(b);
    }
}
