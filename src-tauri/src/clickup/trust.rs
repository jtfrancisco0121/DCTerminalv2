//! App-owned ClickUp state of a run, kept OUTSIDE the run folder (which
//! Claude can write): `<app data>/run-trust/<runId>/`.
//!
//! - `trust.json`: the run folder and the Claude account chosen at Start
//!   (whose token posts, checks and pulls use).
//! - `tickets.json`: the snapshot as written at Start. The run folder's
//!   `tickets.json` is the copy Claude reads; posts, R8 and labels use this one.
//! - `pulls.json`: R8 baselines (the version of each ticket pulled in last).
//! - `posted.json`: the authoritative record of what was posted; the run
//!   folder's `writeback-posted.json` is an informational mirror.

use super::intake::{Pulls, TicketsSnapshot};
use super::writeback::{Posted, PostedStore, POSTED_FILE};
use crate::store::write_json_atomic;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const TRUST_DIR: &str = "run-trust";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustFile {
    pub schema: u32,
    pub run_id: String,
    pub run_dir: String,
    pub account_id: String,
}

/// What the app trusts about one run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunTrust {
    pub account_id: String,
    pub snapshot: TicketsSnapshot,
}

fn run_id_of(run_dir: &Path) -> Result<String, String> {
    run_dir
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty() && !n.starts_with('.'))
        .map(str::to_string)
        .ok_or_else(|| "not a run folder".to_string())
}

pub fn trust_dir(data: &Path, run_dir: &Path) -> Result<PathBuf, String> {
    Ok(data.join(TRUST_DIR).join(run_id_of(run_dir)?))
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|_| format!("{} could not be read.", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

/// At Start: the account and the snapshot, before Claude is launched.
pub fn write_start(
    data: &Path,
    run_dir: &Path,
    account_id: &str,
    snapshot: &TicketsSnapshot,
) -> Result<(), String> {
    let dir = trust_dir(data, run_dir)?;
    write_json_atomic(
        &dir.join("trust.json"),
        &TrustFile {
            schema: 1,
            run_id: run_id_of(run_dir)?,
            run_dir: run_dir.display().to_string(),
            account_id: account_id.to_string(),
        },
    )?;
    write_json_atomic(&dir.join("tickets.json"), snapshot)
}

/// The run's trusted state; `None` when the app took no snapshot at Start.
pub fn load(data: &Path, run_dir: &Path) -> Result<Option<RunTrust>, String> {
    let dir = trust_dir(data, run_dir)?;
    let Some(file) = read::<TrustFile>(&dir.join("trust.json"))? else {
        return Ok(None);
    };
    let same = |a: &str| {
        let a = Path::new(a);
        a.canonicalize().unwrap_or_else(|_| a.to_path_buf())
            == run_dir
                .canonicalize()
                .unwrap_or_else(|_| run_dir.to_path_buf())
    };
    if file.schema != 1 || !same(&file.run_dir) {
        return Err("The app's record of this run's tickets does not match the run folder.".into());
    }
    let snapshot = read::<TicketsSnapshot>(&dir.join("tickets.json"))?
        .ok_or("The app's copy of this run's tickets is missing.")?;
    Ok(Some(RunTrust {
        account_id: file.account_id,
        snapshot,
    }))
}

pub fn read_pulls(data: &Path, run_dir: &Path) -> Result<Pulls, String> {
    Ok(read(&trust_dir(data, run_dir)?.join("pulls.json"))?.unwrap_or_default())
}

pub fn save_pulls(data: &Path, run_dir: &Path, pulls: &Pulls) -> Result<(), String> {
    write_json_atomic(&trust_dir(data, run_dir)?.join("pulls.json"), pulls)
}

/// A damaged record never reads as "nothing posted".
pub fn read_posted(data: &Path, run_dir: &Path) -> Result<Posted, String> {
    Ok(read(&trust_dir(data, run_dir)?.join("posted.json"))
        .map_err(|_| {
            "The app's record of what was posted could not be read; nothing is posted until it is fixed."
                .to_string()
        })?
        .unwrap_or(Posted {
            schema: 1,
            ..Posted::default()
        }))
}

/// Saves the posted record app-side (must succeed), then mirrors it into
/// the run folder for people to read (best effort).
pub struct TrustStore<'a> {
    pub data: &'a Path,
    pub run_dir: &'a Path,
}

impl PostedStore for TrustStore<'_> {
    fn save(&mut self, posted: &Posted) -> Result<(), String> {
        write_json_atomic(
            &trust_dir(self.data, self.run_dir)?.join("posted.json"),
            posted,
        )?;
        let _ = write_json_atomic(&self.run_dir.join(POSTED_FILE), posted);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clickup::intake::{tests::sample, SnapshotSource, LIMITS};
    use crate::test_support::TempDir;

    pub(crate) fn snap() -> TicketsSnapshot {
        TicketsSnapshot {
            schema: 1,
            fetched_at: "t".into(),
            source: SnapshotSource {
                kind: "clickup".into(),
                account_id: "personal".into(),
                workspace_id: None,
                refs: vec![],
            },
            limits: LIMITS,
            note: String::new(),
            tasks: vec![sample("86x1", "Export")],
        }
    }

    #[test]
    fn the_trusted_copy_lives_outside_the_run_folder_and_ignores_run_folder_edits() {
        let dir = TempDir::new("trust");
        let run = dir.join("data/runs/r-1");
        std::fs::create_dir_all(&run).unwrap();
        let data = dir.join("data");
        assert_eq!(load(&data, &run).unwrap(), None);
        write_start(&data, &run, "personal", &snap()).unwrap();
        assert!(!trust_dir(&data, &run).unwrap().starts_with(&run));
        // Claude rewrites the run folder's copies: the trusted state is unchanged.
        std::fs::write(run.join("tickets.json"), r#"{"tasks":[{"id":"evil1"}]}"#).unwrap();
        std::fs::write(run.join("meta.json"), r#"{"account":{"id":"work"}}"#).unwrap();
        std::fs::write(run.join(POSTED_FILE), r#"{"schema":1,"drafts":{"86x1":{"comment":{"result":"posted","at":"t","value":"x"}}}}"#).unwrap();
        let trust = load(&data, &run).unwrap().unwrap();
        assert_eq!(trust.account_id, "personal");
        assert_eq!(trust.snapshot.tasks[0].id, "86x1");
        assert!(read_posted(&data, &run).unwrap().drafts.is_empty());
        // Saving mirrors into the run folder; the record read back is the app's.
        let mut posted = Posted {
            schema: 1,
            ..Posted::default()
        };
        posted.drafts.insert("86x1".into(), Default::default());
        TrustStore {
            data: &data,
            run_dir: &run,
        }
        .save(&posted)
        .unwrap();
        assert_eq!(read_posted(&data, &run).unwrap(), posted);
        assert!(std::fs::read_to_string(run.join(POSTED_FILE))
            .unwrap()
            .contains("86x1"));
        // Another run folder with the same name elsewhere does not match.
        let other = dir.join("elsewhere/r-1");
        std::fs::create_dir_all(&other).unwrap();
        assert!(load(&data, &other).is_err());
        std::fs::write(trust_dir(&data, &run).unwrap().join("posted.json"), "{bad").unwrap();
        assert!(read_posted(&data, &run).is_err());
    }
}
