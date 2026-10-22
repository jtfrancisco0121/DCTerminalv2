//! Where DCTerminalv2 stores its own files.
//!
//! `DCTV2_DATA_DIR` is the explicit override used by end-to-end tests. On Windows
//! Tauri's known-folder lookup ignores `APPDATA` and can fail with
//! `unknown path` when `USERPROFILE` is not a real profile. That failure must
//! not abort startup: the setup hook panics if it returns an error.

use std::ffi::OsStr;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataDirOrigin {
    Override,
    KnownFolder,
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDataDir {
    pub path: PathBuf,
    pub origin: DataDirOrigin,
    /// Set when the operating system could not name the profile folder.
    pub warning: Option<String>,
}

pub fn app_data_dir(app: &AppHandle) -> ResolvedDataDir {
    let known = app
        .path()
        .app_data_dir()
        .map(|dir| build_data_dir(dir, DEV_BUILD))
        .map_err(|err| err.to_string());
    resolve_data_dir(
        std::env::var_os(DATA_DIR_ENV).as_deref(),
        known,
        std::env::temp_dir().join("dcterminalv2"),
    )
}

/// The explicit data folder override (end-to-end tests). Not DCTerminal's
/// `DCT_DATA_DIR`, so a shell set up for DCTerminal's tests leaves v2 alone.
pub const DATA_DIR_ENV: &str = "DCTV2_DATA_DIR";

/// A debug build (`tauri dev`) has the installed app's identifier, so it
/// keeps its files in a folder of its own (`<identifier>.dev`): the two
/// never write the same `state.json` or `settings.json`.
const DEV_BUILD: bool = cfg!(debug_assertions);

/// The platform data folder named after the identifier, with `.dev` added
/// for a debug build.
fn build_data_dir(known: PathBuf, dev: bool) -> PathBuf {
    if !dev {
        return known;
    }
    let mut name = known.file_name().unwrap_or_default().to_os_string();
    name.push(".dev");
    known.with_file_name(name)
}

/// Whether this process lets only one copy of itself run (a second launch
/// focuses the first). Only the installed app on its own data folder: a
/// debug build or a `DCTV2_DATA_DIR` run (end-to-end tests) has a folder of
/// its own and runs beside it.
pub fn single_instance(dev: bool, override_dir: Option<&OsStr>) -> bool {
    !dev && override_path(override_dir).is_none()
}

/// [`single_instance`] for this process.
pub fn single_instance_here() -> bool {
    single_instance(DEV_BUILD, std::env::var_os(DATA_DIR_ENV).as_deref())
}

/// `identifier` in `tauri.conf.json`; Tauri names the app data folder after it.
const IDENTIFIER: &str = "com.jtfrancisco.dcterminalv2";

/// The data dir before Tauri exists (no AppHandle yet), by the same rule as
/// [`app_data_dir`]: `DCTV2_DATA_DIR`, else the platform data folder joined
/// with the identifier. `None` when the platform folder cannot be named
/// (Tauri would then use its temp fallback) and on Windows, where nothing
/// needs it yet.
#[cfg_attr(not(unix), allow(dead_code))]
pub fn data_dir_before_tauri() -> Option<PathBuf> {
    let known = platform_data_dir(
        std::env::var_os("HOME").as_deref(),
        std::env::var_os("XDG_DATA_HOME").as_deref(),
    )
    .map(|dir| build_data_dir(dir.join(IDENTIFIER), DEV_BUILD))
    .ok_or_else(|| "no home folder".to_string());
    let resolved = resolve_data_dir(
        std::env::var_os(DATA_DIR_ENV).as_deref(),
        known,
        std::env::temp_dir().join("dcterminalv2"),
    );
    (resolved.origin != DataDirOrigin::Fallback).then_some(resolved.path)
}

/// The folder Tauri's `app_data_dir` starts from (the `dirs` crate's
/// `data_dir`): `~/Library/Application Support` on macOS, `$XDG_DATA_HOME`
/// (when absolute) or `~/.local/share` on other Unix systems.
#[cfg_attr(not(unix), allow(dead_code))]
fn platform_data_dir(home: Option<&OsStr>, xdg_data_home: Option<&OsStr>) -> Option<PathBuf> {
    let home = home.filter(|home| !home.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        return None;
    }
    if cfg!(target_os = "macos") {
        return home.map(|home| home.join("Library").join("Application Support"));
    }
    xdg_data_home
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| home.map(|home| home.join(".local").join("share")))
}

pub fn resolve_data_dir(
    override_dir: Option<&OsStr>,
    known_folder: Result<PathBuf, String>,
    fallback: PathBuf,
) -> ResolvedDataDir {
    if let Some(path) = override_path(override_dir) {
        return ResolvedDataDir {
            path,
            origin: DataDirOrigin::Override,
            warning: None,
        };
    }
    match known_folder {
        Ok(path) => ResolvedDataDir {
            path,
            origin: DataDirOrigin::KnownFolder,
            warning: None,
        },
        Err(err) => {
            let warning = format!(
                "known folder lookup failed ({err}); using {}",
                fallback.display()
            );
            ResolvedDataDir {
                path: fallback,
                origin: DataDirOrigin::Fallback,
                warning: Some(warning),
            }
        }
    }
}

fn override_path(value: Option<&OsStr>) -> Option<PathBuf> {
    let raw = value?;
    if raw.is_empty() {
        return None;
    }
    if let Some(text) = raw.to_str() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        return Some(PathBuf::from(trimmed));
    }
    Some(PathBuf::from(raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_data_dir_wins_over_the_known_folder() {
        let resolved = resolve_data_dir(
            Some(OsStr::new("/tmp/dcterminal-e2e")),
            Ok(PathBuf::from("/real/profile")),
            PathBuf::from("/fallback"),
        );
        assert_eq!(resolved.origin, DataDirOrigin::Override);
        assert_eq!(resolved.path, PathBuf::from("/tmp/dcterminal-e2e"));
        assert!(resolved.warning.is_none());
    }

    #[test]
    fn an_unknown_path_falls_back_instead_of_failing_startup() {
        let resolved = resolve_data_dir(
            None,
            Err("unknown path".to_string()),
            PathBuf::from("/fallback"),
        );
        assert_eq!(resolved.origin, DataDirOrigin::Fallback);
        assert_eq!(resolved.path, PathBuf::from("/fallback"));
        let warning = resolved.warning.expect("fallback explains itself");
        assert!(warning.contains("unknown path"));
    }

    #[test]
    fn the_identifier_matches_tauri_conf() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"], IDENTIFIER);
    }

    /// DCTerminal stays installed beside v2 (plan, answer 1): v2 must not
    /// share its bundle id, its app data folder, or its env overrides.
    #[test]
    fn v2_installs_and_stores_apart_from_dcterminal() {
        assert_ne!(IDENTIFIER, "com.jtfrancisco.dcterminal");
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["productName"], "DCTerminalv2");
        let dir =
            platform_data_dir(Some(OsStr::new("/home/me")), None).map(|dir| dir.join(IDENTIFIER));
        if let Some(dir) = dir {
            assert!(dir.ends_with("com.jtfrancisco.dcterminalv2"));
        }
        assert_eq!(
            DATA_DIR_ENV, "DCTV2_DATA_DIR",
            "not DCTerminal's DCT_DATA_DIR"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_platform_data_folder_matches_tauris() {
        let dir = platform_data_dir(Some(OsStr::new("/home/me")), None).unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(dir, PathBuf::from("/home/me/Library/Application Support"));
        } else {
            assert_eq!(dir, PathBuf::from("/home/me/.local/share"));
            let xdg = platform_data_dir(Some(OsStr::new("/home/me")), Some(OsStr::new("/xdg")));
            assert_eq!(xdg.unwrap(), PathBuf::from("/xdg"));
            let relative = platform_data_dir(Some(OsStr::new("/home/me")), Some(OsStr::new("rel")));
            assert_eq!(relative.unwrap(), PathBuf::from("/home/me/.local/share"));
        }
        assert_eq!(platform_data_dir(None, None), None);
    }

    // Review: `tauri dev` and the installed app share the identifier, so
    // both read and wrote the same state.json and settings.json.
    #[test]
    fn a_debug_build_keeps_its_files_apart_from_the_installed_app() {
        let known = PathBuf::from("/s/com.jtfrancisco.dcterminalv2");
        assert_eq!(build_data_dir(known.clone(), false), known);
        assert_eq!(
            build_data_dir(known, true),
            PathBuf::from("/s/com.jtfrancisco.dcterminalv2.dev")
        );
    }

    #[test]
    fn only_the_installed_app_on_its_own_folder_is_single_instance() {
        assert!(single_instance(false, None));
        assert!(single_instance(false, Some(OsStr::new("  "))));
        assert!(!single_instance(true, None), "a debug build runs beside it");
        assert!(
            !single_instance(false, Some(OsStr::new("/tmp/e2e"))),
            "a test data folder runs beside it"
        );
    }

    #[test]
    fn a_blank_override_does_not_hide_the_known_folder() {
        let resolved = resolve_data_dir(
            Some(OsStr::new("   ")),
            Ok(PathBuf::from("/real/profile")),
            PathBuf::from("/fallback"),
        );
        assert_eq!(resolved.origin, DataDirOrigin::KnownFolder);
        assert_eq!(resolved.path, PathBuf::from("/real/profile"));
    }
}
