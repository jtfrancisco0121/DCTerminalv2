//! Last Claude rate-limit windows, per Claude config folder (one account).
//!
//! Claude terminal tabs report them through the status line DCTerminalv2
//! sets (`status_line`); `fold` takes those readings in, newest first. They
//! are saved to `usage.json` in DCTerminalv2's app data, so the last reading
//! is shown after a restart; nothing is written into the config folder.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const USAGE_FILE: &str = "usage.json";

/// Delete the status line files of tabs closed for good (no longer open or
/// reopenable). Copies the tab ids out under a short state lock, then walks
/// the folder with no lock held.
pub fn remove_gone_status_lines(app: &tauri::AppHandle) {
    use tauri::Manager;
    let Some(state) = app.try_state::<std::sync::Mutex<crate::store::StateStore>>() else {
        return;
    };
    let Ok(keep) = state.lock().map(|state| state.known_tab_ids()) else {
        return;
    };
    let data_dir = crate::data_dir::app_data_dir(app).path;
    crate::status_line::remove_files_of_gone_tabs(&data_dir, &keep);
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RateWindow {
    pub rate_limit_type: String,
    pub label: String,
    /// 0–100 when Claude Code reported it. Missing on its first render, so
    /// the UI says the percent is not reported yet.
    pub utilization: Option<f64>,
    /// Unix seconds, when Claude Code reported it.
    pub resets_at: Option<i64>,
    pub status: String,
    pub seen_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub config_dir: String,
    pub windows: Vec<RateWindow>,
}

/// What `usage.json` holds: the last rate windows per config folder.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageFile {
    #[serde(default)]
    by_config: HashMap<String, HashMap<String, RateWindow>>,
}

#[derive(Debug, Default)]
pub struct UsageStore {
    by_config: HashMap<String, HashMap<String, RateWindow>>,
    /// Terminal tabs' status line files, by tab: (config folder, file).
    status_lines: HashMap<String, (String, PathBuf)>,
    /// `usage.json` in app data. `None` keeps everything in memory (tests).
    path: Option<PathBuf>,
}

impl UsageStore {
    /// Load the last rate windows from app data. A damaged file starts empty.
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join(USAGE_FILE);
        let file: UsageFile = crate::store::read_json_or_recover(&path).unwrap_or_default();
        // Readings of the last run's terminal tabs were folded in already.
        let _ = std::fs::remove_dir_all(data_dir.join(crate::status_line::STATUS_LINE_DIR));
        Self {
            by_config: file.by_config,
            path: Some(path),
            ..Self::default()
        }
    }

    fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let file = UsageFile {
            by_config: self.by_config.clone(),
        };
        if let Err(err) = crate::store::write_json_atomic(path, &file) {
            eprintln!("DCTerminalv2: could not save Claude usage ({err})");
        }
    }

    /// A Claude terminal tab under `config_dir` writes its status line
    /// readings to `file` (`status_line::file_for`).
    pub fn watch_status_line(&mut self, tab_id: &str, config_dir: &str, file: PathBuf) {
        let key = config_dir.trim().to_string();
        if key.is_empty() {
            return;
        }
        self.status_lines.insert(tab_id.to_string(), (key, file));
    }

    /// A closed tab's status line no longer counts. Its file stays while
    /// the tab can be reopened ([`remove_gone_status_lines`] drops it after).
    pub fn forget_tab(&mut self, tab_id: &str) {
        self.status_lines.remove(tab_id);
    }

    /// The status line files of `config_dir`'s terminal tabs, to read
    /// without holding this store's lock.
    pub fn status_line_files(&self, config_dir: &str) -> Vec<PathBuf> {
        let key = config_dir.trim();
        let mut files: Vec<PathBuf> = self
            .status_lines
            .values()
            .filter(|(dir, _)| dir == key)
            .map(|(_, file)| file.clone())
            .collect();
        files.sort();
        files
    }

    /// Fold readings read from status line files into `config_dir`'s windows.
    pub fn fold(&mut self, config_dir: &str, windows: Vec<RateWindow>) {
        let key = config_dir.trim().to_string();
        if key.is_empty() || windows.is_empty() {
            return;
        }
        let stored = self.by_config.entry(key).or_default();
        let mut changed = false;
        for window in windows {
            changed |= merge_window(stored, window);
        }
        if changed {
            self.save();
        }
    }

    pub fn snapshot(&self, config_dir: &str) -> UsageSnapshot {
        let key = config_dir.trim();
        let mut windows: Vec<RateWindow> = self
            .by_config
            .get(key)
            .map(|map| map.values().cloned().collect())
            .unwrap_or_default();
        windows.sort_by(|a, b| a.rate_limit_type.cmp(&b.rate_limit_type));
        UsageSnapshot {
            config_dir: key.to_string(),
            windows,
        }
    }
}

/// Put `window` in `windows` unless the stored reading is newer. A reading
/// without a percent keeps the stored one's while the window is the same
/// (same reset time).
/// Returns whether what is saved changed.
fn merge_window(windows: &mut HashMap<String, RateWindow>, mut window: RateWindow) -> bool {
    let Some(old) = windows.get(&window.rate_limit_type) else {
        windows.insert(window.rate_limit_type.clone(), window);
        return true;
    };
    if window.seen_at_ms < old.seen_at_ms {
        return false;
    }
    if window.utilization.is_none() && window.resets_at == old.resets_at {
        window.utilization = old.utilization;
    }
    let changed = old.utilization != window.utilization
        || old.resets_at != window.resets_at
        || old.status != window.status;
    windows.insert(window.rate_limit_type.clone(), window);
    changed
}

pub fn window_label(rate_type: &str) -> String {
    match rate_type {
        "five_hour" => "5h".to_string(),
        "seven_day" => "7d".to_string(),
        other => other.replace('_', " "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(kind: &str, utilization: Option<f64>, resets_at: i64, seen_at_ms: i64) -> RateWindow {
        RateWindow {
            rate_limit_type: kind.into(),
            label: window_label(kind),
            utilization,
            resets_at: Some(resets_at),
            status: "allowed".into(),
            seen_at_ms,
        }
    }

    #[test]
    fn terminal_status_line_readings_fold_in_for_their_account_only() {
        let dir = crate::test_support::TempDir::new("usage_status_line");
        let mut store = UsageStore::open(dir.path());
        let file = crate::status_line::file_for(dir.path(), "tab_t");
        store.watch_status_line("tab_t", "/cfg", file.clone());
        assert_eq!(store.status_line_files("/cfg"), vec![file.clone()]);
        assert!(store.status_line_files("/other").is_empty());

        // Nothing written yet (the first render): usage stays unknown.
        store.fold("/cfg", Vec::new());
        assert!(store.snapshot("/cfg").windows.is_empty());

        store.fold(
            "/cfg",
            vec![
                window("five_hour", Some(42.0), 100, 10),
                window("seven_day", Some(7.0), 900, 10),
            ],
        );
        let snap = store.snapshot("/cfg");
        assert_eq!(snap.windows.len(), 2);
        assert_eq!(snap.windows[0].utilization, Some(42.0));
        assert!(store.snapshot("/other").windows.is_empty());
        // Saved, so it is shown after a restart.
        assert_eq!(
            UsageStore::open(dir.path()).snapshot("/cfg").windows.len(),
            2
        );

        // An older reading never replaces a newer one.
        store.fold("/cfg", vec![window("five_hour", Some(10.0), 100, 5)]);
        assert_eq!(store.snapshot("/cfg").windows[0].utilization, Some(42.0));
        store.fold("/cfg", vec![window("five_hour", Some(55.0), 100, 20)]);
        assert_eq!(store.snapshot("/cfg").windows[0].utilization, Some(55.0));
    }

    #[test]
    fn opening_clears_status_line_files_from_the_last_run() {
        let dir = crate::test_support::TempDir::new("usage_status_line_clear");
        let old = crate::status_line::file_for(dir.path(), "tab_gone");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, "[]").unwrap();
        let _store = UsageStore::open(dir.path());
        assert!(!old.exists());
    }

    #[test]
    fn a_newer_reading_without_a_percent_keeps_the_same_windows_percent() {
        let mut store = UsageStore::default();
        store.fold("/cfg", vec![window("five_hour", Some(42.0), 100, 10)]);
        store.fold("/cfg", vec![window("five_hour", None, 100, 20)]);
        let w = &store.snapshot("/cfg").windows[0];
        assert_eq!((w.utilization, w.seen_at_ms), (Some(42.0), 20));
        // Once the window resets, the old percent no longer applies.
        store.fold("/cfg", vec![window("five_hour", None, 200, 30)]);
        assert_eq!(store.snapshot("/cfg").windows[0].utilization, None);
    }

    #[test]
    fn a_closed_tab_stops_counting() {
        let dir = crate::test_support::TempDir::new("usage_forget");
        let mut store = UsageStore::open(dir.path());
        store.watch_status_line(
            "tab_t",
            "/cfg",
            crate::status_line::file_for(dir.path(), "tab_t"),
        );
        store.forget_tab("tab_t");
        assert!(store.status_line_files("/cfg").is_empty());
    }
}
