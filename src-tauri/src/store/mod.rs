//! Persistent stores behind `tauri::State<Mutex<..>>`.
//!
//! Lock order: when one code path holds more than one store lock, take
//! `StateStore` before `SettingsStore`, or drop one before taking the other.
//! The terminal registry (`pty::PtyRegistry`) comes before both: an open
//! checks the store under the registry lock right before it spawns, so no
//! code takes the registry while holding a store lock.
//!
//! Review rule: no store lock is held across a process spawn or a directory
//! walk. Copy what is needed out of the store, drop the guard, then do the
//! slow work (and lock again only to write the result back).

mod json_io;
pub(crate) mod settings_store;
mod state_store;
mod state_types;

pub(crate) use json_io::{
    append_run_file, create_run_log, read_capped, write_run_file, CappedError,
};
pub use json_io::{read_json_or_recover, take_recovery_notes, write_json_atomic};
pub use settings_store::{
    ClaudeSettings, RepoSetup, RunSettings, SettingsStore, TerminalSettings, UiSettings,
};
pub use state_store::StateStore;
pub use state_types::{
    window_label, window_matches, AppStateFile, ArchivedWindow, TabKind, TabLaunch, TabRecord,
    WindowRecord, DEFAULT_ACCOUNT_ID, MAIN_WINDOW_ID,
};
