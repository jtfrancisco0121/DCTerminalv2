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
//!
//! Every file is signed: `{"mac": <hex>, "body": <document>}`, the MAC an
//! HMAC-SHA256 over the run id, the file name and the body, with a key made
//! once per install and kept in the macOS keychain (service
//! `com.jtfrancisco.dcterminalv2.trust`). A file with a bad or missing MAC
//! turns ClickUp off for the run, with a message in the panel. All four
//! files are written at Start, so a deleted one is refused too, never read
//! as "nothing posted". The guard also blocks Claude's writes into
//! `run-trust` (guard.sh).

use super::intake::{Pulls, TicketsSnapshot};
use super::writeback::{Posted, PostedStore, POSTED_FILE};
use crate::store::write_json_atomic;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const TRUST_DIR: &str = "run-trust";
pub const KEY_SERVICE: &str = "com.jtfrancisco.dcterminalv2.trust";
const KEY_ACCOUNT: &str = "install";

pub const TAMPERED: &str = "The app's signed record of this run's ClickUp tickets is missing or was changed outside the app. ClickUp checks and posts are off for this run.";

/// The per-install signing key. No `Debug` of its bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct Key([u8; 32]);

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(***)")
    }
}

impl Key {
    #[cfg(test)]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    fn random() -> Result<Self, String> {
        use std::io::Read;
        let mut bytes = [0u8; 32];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut bytes))
            .map_err(|_| "Could not make the app's signing key.".to_string())?;
        Ok(Self(bytes))
    }

    fn hex(&self) -> String {
        hex(&self.0)
    }

    fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
        }
        Some(Self(bytes))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// HMAC-SHA256 (RFC 2104).
pub fn hmac(key: &Key, message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    k[..32].copy_from_slice(&key.0);
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// The key, from the keychain; made and saved on first use, then cached.
pub fn install_key() -> Result<Key, String> {
    static KEY: OnceLock<Key> = OnceLock::new();
    if let Some(key) = KEY.get() {
        return Ok(key.clone());
    }
    let entry = keyring::Entry::new(KEY_SERVICE, KEY_ACCOUNT)
        .map_err(|_| "Could not use the system keychain for the app's signing key.".to_string())?;
    let key = match entry.get_password() {
        Ok(saved) => {
            Key::parse(&saved).ok_or("The app's signing key in the keychain is unreadable.")?
        }
        Err(keyring::Error::NoEntry) => {
            let key = Key::random()?;
            entry
                .set_password(&key.hex())
                .map_err(|_| "Could not save the app's signing key in the keychain.".to_string())?;
            key
        }
        Err(_) => return Err("Could not read the app's signing key from the keychain.".into()),
    };
    Ok(KEY.get_or_init(|| key).clone())
}

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

#[derive(Serialize, Deserialize)]
struct Signed {
    mac: String,
    body: serde_json::Value,
}

fn mac_of(key: &Key, run_id: &str, name: &str, body: &serde_json::Value) -> String {
    let message = format!("dcv2-trust/1\n{run_id}\n{name}\n{body}");
    hex(&hmac(key, message.as_bytes()))
}

/// Constant-time compare of two hex MACs.
fn same_mac(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn write_signed<T: Serialize>(
    key: &Key,
    data: &Path,
    run_dir: &Path,
    name: &str,
    value: &T,
) -> Result<(), String> {
    let run_id = run_id_of(run_dir)?;
    let body = serde_json::to_value(value).map_err(|e| e.to_string())?;
    let mac = mac_of(key, &run_id, name, &body);
    write_json_atomic(&trust_dir(data, run_dir)?.join(name), &Signed { mac, body })
}

/// `Ok(None)` when the file is not there; `Err(TAMPERED)` on a bad MAC.
fn read_signed<T: DeserializeOwned>(
    key: &Key,
    data: &Path,
    run_dir: &Path,
    name: &str,
) -> Result<Option<T>, String> {
    let path = trust_dir(data, run_dir)?.join(name);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("{}: {err}", path.display())),
    };
    let signed: Signed = serde_json::from_str(&text).map_err(|_| TAMPERED.to_string())?;
    if !same_mac(
        &signed.mac,
        &mac_of(key, &run_id_of(run_dir)?, name, &signed.body),
    ) {
        return Err(TAMPERED.into());
    }
    serde_json::from_value(signed.body)
        .map(Some)
        .map_err(|_| TAMPERED.to_string())
}

/// Removes a run's records (a Start that failed).
pub fn remove(data: &Path, run_dir: &Path) {
    if let Ok(dir) = trust_dir(data, run_dir) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// At Start: the account, the snapshot, and empty pulls and posted records.
pub fn write_start(
    key: &Key,
    data: &Path,
    run_dir: &Path,
    account_id: &str,
    snapshot: &TicketsSnapshot,
) -> Result<(), String> {
    remove(data, run_dir);
    let file = TrustFile {
        schema: 1,
        run_id: run_id_of(run_dir)?,
        run_dir: run_dir.display().to_string(),
        account_id: account_id.to_string(),
    };
    write_signed(key, data, run_dir, "tickets.json", snapshot)?;
    write_signed(
        key,
        data,
        run_dir,
        "pulls.json",
        &Pulls {
            schema: 1,
            ..Pulls::default()
        },
    )?;
    write_signed(
        key,
        data,
        run_dir,
        "posted.json",
        &Posted {
            schema: 1,
            ..Posted::default()
        },
    )?;
    // Last: a run counts as having app records only once all are written.
    write_signed(key, data, run_dir, "trust.json", &file)
}

/// The run's trusted state; `None` when the app took no snapshot at Start.
pub fn load(key: &Key, data: &Path, run_dir: &Path) -> Result<Option<RunTrust>, String> {
    let dir = trust_dir(data, run_dir)?;
    if !dir.exists() {
        return Ok(None);
    }
    let file: TrustFile = read_signed(key, data, run_dir, "trust.json")?.ok_or(TAMPERED)?;
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    if file.schema != 1 || canon(Path::new(&file.run_dir)) != canon(run_dir) {
        return Err(TAMPERED.into());
    }
    let snapshot =
        read_signed::<TicketsSnapshot>(key, data, run_dir, "tickets.json")?.ok_or(TAMPERED)?;
    Ok(Some(RunTrust {
        account_id: file.account_id,
        snapshot,
    }))
}

pub fn read_pulls(key: &Key, data: &Path, run_dir: &Path) -> Result<Pulls, String> {
    read_signed(key, data, run_dir, "pulls.json")?.ok_or_else(|| TAMPERED.to_string())
}

pub fn save_pulls(key: &Key, data: &Path, run_dir: &Path, pulls: &Pulls) -> Result<(), String> {
    write_signed(key, data, run_dir, "pulls.json", pulls)
}

/// A damaged or missing record never reads as "nothing posted".
pub fn read_posted(key: &Key, data: &Path, run_dir: &Path) -> Result<Posted, String> {
    read_signed(key, data, run_dir, "posted.json")?.ok_or_else(|| TAMPERED.to_string())
}

/// The auto-post record (`autopost.json`); `None` before the first auto-post.
pub fn read_autopost(
    key: &Key,
    data: &Path,
    run_dir: &Path,
) -> Result<Option<super::autopost::AutoPostRecord>, String> {
    read_signed(key, data, run_dir, super::autopost::AUTOPOST_FILE)
}

/// The user's click for auto-post (`autopost-consent.json`, signed): the
/// writeback.json version they read and said "Post" for. Only the panel's
/// consent command writes it; nothing is posted on its own without it.
pub const CONSENT_FILE: &str = "autopost-consent.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoPostConsent {
    pub schema: u32,
    /// sha-256 of the writeback.json the user saw.
    pub writeback_sha: String,
    pub at: String,
}

pub fn read_consent(
    key: &Key,
    data: &Path,
    run_dir: &Path,
) -> Result<Option<AutoPostConsent>, String> {
    read_signed(key, data, run_dir, CONSENT_FILE)
}

pub fn save_consent(
    key: &Key,
    data: &Path,
    run_dir: &Path,
    consent: &AutoPostConsent,
) -> Result<(), String> {
    write_signed(key, data, run_dir, CONSENT_FILE, consent)
}

pub fn save_autopost(
    key: &Key,
    data: &Path,
    run_dir: &Path,
    record: &super::autopost::AutoPostRecord,
) -> Result<(), String> {
    write_signed(key, data, run_dir, super::autopost::AUTOPOST_FILE, record)
}

/// Saves the posted record app-side, signed (must succeed), then mirrors it
/// into the run folder for people to read (best effort).
pub struct TrustStore<'a> {
    pub key: &'a Key,
    pub data: &'a Path,
    pub run_dir: &'a Path,
}

impl PostedStore for TrustStore<'_> {
    fn save(&mut self, posted: &Posted) -> Result<(), String> {
        write_signed(self.key, self.data, self.run_dir, "posted.json", posted)?;
        let _ = write_json_atomic(&self.run_dir.join(POSTED_FILE), posted);
        Ok(())
    }

    /// Merge-save: the record on disk, with only this ticket's entry ours.
    fn save_ticket(&mut self, posted: &Posted, ticket_id: &str) -> Result<(), String> {
        let mut disk = read_posted(self.key, self.data, self.run_dir)?;
        disk.schema = 1;
        match posted.drafts.get(ticket_id) {
            Some(entry) => disk.drafts.insert(ticket_id.to_string(), entry.clone()),
            None => disk.drafts.remove(ticket_id),
        };
        self.save(&disk)
    }

    fn reload(&mut self) -> Result<Option<Posted>, String> {
        read_posted(self.key, self.data, self.run_dir).map(Some)
    }
}

pub const POST_LOCK: &str = "post.lock";
pub const POST_BUSY: &str =
    "ClickUp is already being updated for this run (in another window or app). Try again in a moment.";

/// An exclusive `flock` on `run-trust/<runId>/post.lock`, held for a whole
/// post (auto or ticked), so two processes (the dev and the built app)
/// never post for one run at once. Released when dropped.
pub struct PostLock(#[allow(dead_code)] std::fs::File);

pub fn lock_posts(data: &Path, run_dir: &Path) -> Result<PostLock, String> {
    let dir = trust_dir(data, run_dir)?;
    // Never made here: a missing folder must keep reading as "no record".
    if !dir.is_dir() {
        return Err(TAMPERED.into());
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(POST_LOCK))
        .map_err(|e| format!("{POST_LOCK}: {e}"))?;
    match file.try_lock() {
        Ok(()) => Ok(PostLock(file)),
        Err(std::fs::TryLockError::WouldBlock) => Err(POST_BUSY.into()),
        Err(std::fs::TryLockError::Error(e)) => Err(format!("{POST_LOCK}: {e}")),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::clickup::intake::{tests::sample, SnapshotSource, LIMITS};
    use crate::test_support::TempDir;

    /// The in-memory key tests use; the keychain is never touched.
    pub(crate) fn key() -> Key {
        Key::from_bytes([7; 32])
    }

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
            epics: vec![],
        }
    }

    #[test]
    fn keys_round_trip_and_never_print() {
        let k = key();
        let a = hmac(&k, b"what do ya want for nothing?");
        assert_ne!(
            a,
            hmac(&Key::from_bytes([8; 32]), b"what do ya want for nothing?")
        );
        assert_eq!(a, hmac(&k, b"what do ya want for nothing?"));
        assert_eq!(Key::parse(&k.hex()), Some(k.clone()));
        assert_eq!(Key::parse("zz"), None);
        assert_eq!(format!("{k:?}"), "Key(***)");
        assert_eq!(KEY_SERVICE, "com.jtfrancisco.dcterminalv2.trust");
    }

    #[test]
    fn one_post_per_run_across_processes_and_saves_merge() {
        let dir = TempDir::new("trust_lock");
        let data = dir.join("data");
        let run = data.join("runs/r-1");
        std::fs::create_dir_all(&run).unwrap();
        let k = key();
        write_start(&k, &data, &run, "personal", &snap()).unwrap();
        let held = lock_posts(&data, &run).unwrap();
        // A second open file (as another process would have) is refused.
        assert_eq!(lock_posts(&data, &run).err().as_deref(), Some(POST_BUSY));
        drop(held);
        assert!(crate::test_support::eventually(
            || lock_posts(&data, &run).is_ok()
        ));
        // Another process saved ticket B; our stale copy saving A keeps B.
        let mut theirs = read_posted(&k, &data, &run).unwrap();
        theirs.drafts.insert("B".into(), Default::default());
        let mut store = TrustStore {
            key: &k,
            data: &data,
            run_dir: &run,
        };
        store.save(&theirs).unwrap();
        let mut ours = Posted {
            schema: 1,
            ..Posted::default()
        };
        ours.drafts.insert("A".into(), Default::default());
        store.save_ticket(&ours, "A").unwrap();
        let now = read_posted(&k, &data, &run).unwrap();
        assert!(now.drafts.contains_key("A") && now.drafts.contains_key("B"));
        assert_eq!(store.reload().unwrap(), Some(now));
    }

    #[test]
    fn hmac_matches_a_known_vector() {
        // HMAC-SHA256(key = 32 x 0x0b, "Hi There"), checked with Python's hmac module.
        let mac = hmac(&Key::from_bytes([0x0b; 32]), b"Hi There");
        assert_eq!(
            hex(&mac),
            "198a607eb44bfbc69903a0f1cf2bbdc5ba0aa3f3d9ae3c1c7a3b1696a0b68cf7"
        );
    }

    #[test]
    fn the_trusted_copy_lives_outside_the_run_folder_and_ignores_run_folder_edits() {
        let dir = TempDir::new("trust");
        let run = dir.join("data/runs/r-1");
        std::fs::create_dir_all(&run).unwrap();
        let data = dir.join("data");
        let k = key();
        assert_eq!(load(&k, &data, &run).unwrap(), None);
        write_start(&k, &data, &run, "personal", &snap()).unwrap();
        assert!(!trust_dir(&data, &run).unwrap().starts_with(&run));
        std::fs::write(run.join("tickets.json"), r#"{"tasks":[{"id":"evil1"}]}"#).unwrap();
        std::fs::write(run.join("meta.json"), r#"{"account":{"id":"work"}}"#).unwrap();
        std::fs::write(run.join(POSTED_FILE), r#"{"schema":1,"drafts":{"86x1":{"comment":{"result":"posted","at":"t","value":"x"}}}}"#).unwrap();
        let trust = load(&k, &data, &run).unwrap().unwrap();
        assert_eq!(trust.account_id, "personal");
        assert_eq!(trust.snapshot.tasks[0].id, "86x1");
        assert!(read_posted(&k, &data, &run).unwrap().drafts.is_empty());
        let mut posted = Posted {
            schema: 1,
            ..Posted::default()
        };
        posted.drafts.insert("86x1".into(), Default::default());
        TrustStore {
            key: &k,
            data: &data,
            run_dir: &run,
        }
        .save(&posted)
        .unwrap();
        assert_eq!(read_posted(&k, &data, &run).unwrap(), posted);
        assert!(std::fs::read_to_string(run.join(POSTED_FILE))
            .unwrap()
            .contains("86x1"));
        // Another run folder with the same name elsewhere does not match.
        let other = dir.join("elsewhere/r-1");
        std::fs::create_dir_all(&other).unwrap();
        assert_eq!(load(&k, &data, &other).unwrap_err(), TAMPERED);
    }

    #[test]
    fn an_edited_unsigned_swapped_or_deleted_record_is_refused() {
        let dir = TempDir::new("trust_mac");
        let data = dir.join("data");
        let run = data.join("runs/r-1");
        std::fs::create_dir_all(&run).unwrap();
        let k = key();
        write_start(&k, &data, &run, "personal", &snap()).unwrap();
        let tdir = trust_dir(&data, &run).unwrap();
        // Another account, re-signed with the wrong key.
        let trust_text = std::fs::read_to_string(tdir.join("trust.json")).unwrap();
        std::fs::write(
            tdir.join("trust.json"),
            trust_text.replace("personal", "work"),
        )
        .unwrap();
        assert_eq!(load(&k, &data, &run).unwrap_err(), TAMPERED);
        write_start(&Key::from_bytes([9; 32]), &data, &run, "work", &snap()).unwrap();
        assert_eq!(
            load(&k, &data, &run).unwrap_err(),
            TAMPERED,
            "signed with another key"
        );
        // Unsigned plain JSON.
        write_start(&k, &data, &run, "personal", &snap()).unwrap();
        std::fs::write(
            tdir.join("tickets.json"),
            serde_json::to_string(&snap()).unwrap(),
        )
        .unwrap();
        assert_eq!(load(&k, &data, &run).unwrap_err(), TAMPERED);
        // A file from another run (same key) does not verify here.
        write_start(&k, &data, &run, "personal", &snap()).unwrap();
        let run2 = data.join("runs/r-2");
        std::fs::create_dir_all(&run2).unwrap();
        let mut posted2 = Posted {
            schema: 1,
            ..Posted::default()
        };
        posted2.drafts.insert("86x1".into(), Default::default());
        write_start(&k, &data, &run2, "personal", &snap()).unwrap();
        TrustStore {
            key: &k,
            data: &data,
            run_dir: &run2,
        }
        .save(&posted2)
        .unwrap();
        std::fs::copy(
            trust_dir(&data, &run2).unwrap().join("posted.json"),
            tdir.join("posted.json"),
        )
        .unwrap();
        assert_eq!(read_posted(&k, &data, &run).unwrap_err(), TAMPERED);
        // Deleted records are refused, never "nothing posted".
        std::fs::remove_file(tdir.join("posted.json")).unwrap();
        assert_eq!(read_posted(&k, &data, &run).unwrap_err(), TAMPERED);
        std::fs::remove_file(tdir.join("pulls.json")).unwrap();
        assert_eq!(read_pulls(&k, &data, &run).unwrap_err(), TAMPERED);
        std::fs::remove_file(tdir.join("trust.json")).unwrap();
        assert_eq!(load(&k, &data, &run).unwrap_err(), TAMPERED);
        remove(&data, &run);
        assert_eq!(load(&k, &data, &run).unwrap(), None);
    }
}
