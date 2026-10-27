use serde::Serialize;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// Data files moved aside since startup, as one line each for the user.
/// Process-wide so every store can add to it without an AppHandle.
static RECOVERY_NOTES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// `opts` that never follows a symbolic link at the last path component (unix: `O_NOFOLLOW`).
fn no_follow(opts: &mut fs::OpenOptions) -> &mut fs::OpenOptions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    opts
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

fn link_refusal(path: &Path) -> std::io::Error {
    std::io::Error::other(format!(
        "{} is a link; the app does not write through links",
        file_name(path)
    ))
}

/// Make a brand-new file at `path` holding `data`. Whatever is there (a stale
/// leftover, or a link someone planted) is removed first, and the file is
/// opened `create_new` without following links (`O_EXCL` + `O_NOFOLLOW`), so
/// the bytes can never land in a file the link points at.
pub(crate) fn write_new_file(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.is_dir() {
            return Err(std::io::Error::other(format!(
                "{} is a folder",
                file_name(path)
            )));
        }
        fs::remove_file(path)?;
    }
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    let mut file = no_follow(&mut opts).open(path)?;
    file.write_all(data)?;
    file.sync_all()
}

/// Atomic write of `data` to a file in a run folder: a fresh temp file beside
/// it (never a link), then a rename. A destination that is a link is refused.
pub(crate) fn write_run_file(path: &Path, data: &[u8]) -> Result<(), String> {
    if is_link(path) {
        return Err(link_refusal(path).to_string());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "path has no parent".to_string())?;
    let tmp = parent.join(format!(
        ".{}.{}-{}.tmp",
        file_name(path),
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    write_new_file(&tmp, data).map_err(|e| format!("{}: {e}", file_name(path)))?;
    #[cfg(windows)]
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("{}: {e}", file_name(path))
    })
}

/// Append to a file in a run folder (created when missing); a link is refused.
pub(crate) fn append_run_file(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if cfg!(not(unix)) && is_link(path) {
        return Err(link_refusal(path));
    }
    let mut opts = fs::OpenOptions::new();
    opts.append(true).create(true);
    no_follow(&mut opts).open(path)?.write_all(data)
}

/// Create (truncate) a log file in a run folder; a link is refused.
pub(crate) fn create_run_log(path: &Path) -> std::io::Result<fs::File> {
    if cfg!(not(unix)) && is_link(path) {
        return Err(link_refusal(path));
    }
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    no_follow(&mut opts).open(path)
}

/// Why [`read_capped`] read nothing.
#[derive(Debug)]
pub(crate) enum CappedError {
    Missing,
    /// A link, a folder or a special file.
    NotRegular,
    TooLarge,
    Io(String),
}

/// Read a regular file of at most `cap` bytes: a link or a special file is
/// refused, and no more than `cap + 1` bytes are ever read.
pub(crate) fn read_capped(path: &Path, cap: u64) -> Result<Vec<u8>, CappedError> {
    let meta = fs::symlink_metadata(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => CappedError::Missing,
        _ => CappedError::Io(e.to_string()),
    })?;
    if !meta.file_type().is_file() {
        return Err(CappedError::NotRegular);
    }
    let mut opts = fs::OpenOptions::new();
    opts.read(true);
    let file = no_follow(&mut opts)
        .open(path)
        .map_err(|e| CappedError::Io(e.to_string()))?;
    let mut data = Vec::new();
    file.take(cap + 1)
        .read_to_end(&mut data)
        .map_err(|e| CappedError::Io(e.to_string()))?;
    if data.len() as u64 > cap {
        return Err(CappedError::TooLarge);
    }
    Ok(data)
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "path has no parent".to_string())?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;

    // Only a file that still parses becomes the backup: a damaged one that
    // could not be moved aside on load must not replace the good `.bak`.
    if path.exists() && read_json::<serde_json::Value>(path).is_ok() {
        let backup = path.with_extension("json.bak");
        if let Ok(good) = fs::read(path) {
            // Never `fs::copy` onto the backup: a link planted there would be followed.
            let _ = write_new_file(&backup, &good);
        }
    }

    let tmp = path.with_extension("json.tmp");
    let mut data = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    if !data.ends_with(b"\n") {
        data.push(b'\n');
    }
    write_new_file(&tmp, &data).map_err(|e| e.to_string())?;

    // Unix rename replaces the file atomically, so a crash never leaves no
    // file at all. Windows keeps the old remove-then-rename.
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let data = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&data).map_err(|e| e.to_string())
}

/// Load JSON, or move a damaged file aside and fall back to `.bak` then `Default`.
/// A corrupt data file must not stop the app from opening.
pub fn read_json_or_recover<T>(path: &Path) -> Result<T, String>
where
    T: serde::de::DeserializeOwned + Serialize + Default,
{
    read_json_or_recover_with(path, T::default)
}

/// [`read_json_or_recover`] with `fallback()` in place of `T::default()`.
/// A file that does not parse but names a newer `schemaVersion` than
/// `fallback()` does is from a newer build: it is set aside, not called damaged.
pub fn read_json_or_recover_with<T: serde::de::DeserializeOwned + Serialize>(
    path: &Path,
    fallback: impl FnOnce() -> T,
) -> Result<T, String> {
    if !path.exists() {
        // No file, but an intact copy beside it: a save interrupted between
        // removing the old file and renaming the new one in (Windows), or a
        // recovery from `.bak` that was never saved before the app quit.
        return Ok(match intact_copy(path) {
            Some((value, source)) => {
                restore(&source, path);
                value
            }
            None => fallback(),
        });
    }
    match read_json::<T>(path) {
        Ok(value) => Ok(value),
        Err(_) => {
            let fallback_value = fallback();
            if let (Some(found), Some(current)) = (
                schema_version_of(path),
                serde_json::to_value(&fallback_value)
                    .ok()
                    .as_ref()
                    .and_then(schema_version_in),
            ) {
                if found > current && set_aside_other_version(path, found).is_some() {
                    return Ok(fallback_value);
                }
            }
            let corrupt = path.with_extension(format!("json.corrupt-{}", stamp()));
            let moved = fs::rename(path, &corrupt).is_ok();
            if moved {
                touch(&corrupt);
            }
            let bak = path.with_extension("json.bak");
            let restored = if bak.exists() {
                read_json::<T>(&bak).ok()
            } else {
                None
            };
            if moved && restored.is_some() {
                restore(&bak, path);
            }
            if moved {
                note_recovery(format!(
                    "{} was damaged; kept as {}. {}",
                    file_name(path),
                    file_name(&corrupt),
                    if restored.is_some() {
                        "Restored from the backup."
                    } else {
                        "Defaults are in use."
                    }
                ));
            }
            Ok(restored.unwrap_or(fallback_value))
        }
    }
}

/// Move a file written by a different DCTerminalv2 version to
/// `<name>.json.from-v{found}-<stamp>`, so no later save or sweep removes it.
/// When `path` is gone, its `.bak` is moved instead. The `.bak` beside a
/// moved `path` goes aside too, as `from-v<its version>` (one identical to
/// `path` is only removed). Never loses data. Returns the kept file.
pub fn set_aside_other_version(path: &Path, found: u32) -> Option<PathBuf> {
    let bak = path.with_extension("json.bak");
    let mut kept_bak = None;
    let target = if path.exists() {
        let target = move_aside(path, path, found)?;
        // A `.bak` left behind would be restored without a notice on the next
        // launch (or swept as an orphan): keep it aside now, whatever version.
        match schema_version_of(&bak) {
            Some(version) if version == found && fs::read(&bak).ok() == fs::read(&target).ok() => {
                let _ = fs::remove_file(&bak);
            }
            Some(version) => kept_bak = move_aside(&bak, path, version),
            None => {}
        }
        target
    } else if bak.exists() {
        move_aside(&bak, path, found)?
    } else {
        return None;
    };
    note_recovery(format!(
        "{} was written by a different DCTerminalv2 version (v{found}); kept as {}.{} Defaults are in use.",
        file_name(path),
        file_name(&target),
        kept_bak
            .map(|kept| format!(" Its backup was kept as {}.", file_name(&kept)))
            .unwrap_or_default()
    ));
    Some(target)
}

/// Move `source` to a fresh `<path>.from-v{found}-<stamp>` and date it now.
fn move_aside(source: &Path, path: &Path, found: u32) -> Option<PathBuf> {
    let base = path.with_extension(format!("json.from-v{found}-{}", stamp()));
    // Two set-asides in the same millisecond must not replace each other.
    let target = (0..)
        .map(|n| {
            if n == 0 {
                base.clone()
            } else {
                PathBuf::from(format!("{}-{n}", base.display()))
            }
        })
        .find(|candidate| !candidate.exists())?;
    if !(fs::rename(source, &target).is_ok() || fs::copy(source, &target).is_ok()) {
        return None;
    }
    touch(&target);
    Some(target)
}

/// The `schemaVersion` in the JSON file at `path`, if it has one.
fn schema_version_of(path: &Path) -> Option<u32> {
    schema_version_in(&read_json::<serde_json::Value>(path).ok()?)
}

/// `schemaVersion` (or `schema_version`) in `value`.
fn schema_version_in(value: &serde_json::Value) -> Option<u32> {
    let version = value
        .get("schemaVersion")
        .or_else(|| value.get("schema_version"))?;
    u32::try_from(version.as_u64()?).ok()
}

/// Put recovered data back as the main file, so it survives the sweep of an
/// orphan `.bak` and a quit before the next save. Byte for byte, so a file
/// from another version is still recognised (and set aside) by its store.
fn restore(source: &Path, path: &Path) {
    let tmp = path.with_extension("json.tmp");
    let copied = source != tmp;
    if copied {
        let Ok(data) = fs::read(source) else {
            return;
        };
        if write_new_file(&tmp, &data).is_err() {
            return;
        }
    }
    // Remove only our own copy: a `.tmp` source is the data itself.
    if fs::rename(&tmp, path).is_err() && copied {
        let _ = fs::remove_file(&tmp);
    }
}

pub fn note_recovery(msg: String) {
    eprintln!("DCTerminalv2: {msg}");
    if let Ok(mut notes) = RECOVERY_NOTES.lock() {
        notes.push(msg);
    }
}

/// The notes recorded so far; each is returned once.
pub fn take_recovery_notes() -> Vec<String> {
    RECOVERY_NOTES
        .lock()
        .map(|mut notes| std::mem::take(&mut *notes))
        .unwrap_or_default()
}

fn stamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%S%3f").to_string()
}

/// `fs::rename` keeps the old mtime, and the startup sweep deletes leftovers
/// by age: date a moved file from the move, so it gets its full 30 days.
fn touch(path: &Path) {
    if let Ok(file) = fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// `.json.tmp` (a finished write that was not renamed in), then `.json.bak`.
/// Returns the value and the file it came from.
fn intact_copy<T: serde::de::DeserializeOwned>(path: &Path) -> Option<(T, PathBuf)> {
    [
        path.with_extension("json.tmp"),
        path.with_extension("json.bak"),
    ]
    .into_iter()
    .filter(|candidate| candidate.exists())
    .find_map(|candidate| {
        read_json::<T>(&candidate)
            .ok()
            .map(|value| (value, candidate))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{names_in, TempDir};

    #[test]
    fn a_failed_restore_leaves_no_temp_copy() {
        let dir = TempDir::new("io_restore_fails");
        let source = dir.join("doc.json.bak");
        fs::write(&source, b"{}").unwrap();
        // A non-empty directory in the way makes the rename fail.
        let path = dir.join("doc.json");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("inside"), b"x").unwrap();
        restore(&source, &path);
        assert_eq!(names_in(dir.path()), vec!["doc.json", "doc.json.bak"]);
    }

    // Review: when a damaged file could not be moved aside on load, the
    // next save copied it over the good `.bak`, losing the only good copy.
    #[test]
    fn a_damaged_file_never_replaces_the_good_backup() {
        let dir = TempDir::new("io_damaged_backup");
        let path = dir.join("doc.json");
        fs::write(&path, b"{ not json").unwrap();
        fs::write(path.with_extension("json.bak"), b"{\"good\":true}").unwrap();
        write_json_atomic(&path, &serde_json::json!({"new": 1})).unwrap();
        assert_eq!(
            fs::read_to_string(path.with_extension("json.bak")).unwrap(),
            "{\"good\":true}"
        );
        // A good file still becomes the backup on the next save.
        write_json_atomic(&path, &serde_json::json!({"new": 2})).unwrap();
        let bak: serde_json::Value = read_json(&path.with_extension("json.bak")).unwrap();
        assert_eq!(bak["new"], 1);
    }

    #[cfg(unix)]
    fn plant_link(dir: &TempDir, name: &str) -> (PathBuf, PathBuf) {
        let outside = dir.join(format!("outside-{name}"));
        fs::write(&outside, "outside").unwrap();
        let link = dir.join("run").join(name);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        (link, outside)
    }

    #[cfg(unix)]
    #[test]
    fn json_writes_never_follow_a_link_planted_at_the_temp_or_backup_name() {
        let dir = TempDir::new("io_links");
        let path = dir.join("run").join("doc.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_json_atomic(&path, &serde_json::json!({"n": 1})).unwrap();
        let outside_tmp = dir.join("outside-tmp");
        let outside_bak = dir.join("outside-bak");
        fs::write(&outside_tmp, "outside tmp").unwrap();
        fs::write(&outside_bak, "outside bak").unwrap();
        std::os::unix::fs::symlink(&outside_tmp, path.with_extension("json.tmp")).unwrap();
        std::os::unix::fs::symlink(&outside_bak, path.with_extension("json.bak")).unwrap();
        write_json_atomic(&path, &serde_json::json!({"n": 2})).unwrap();
        assert_eq!(fs::read_to_string(&outside_tmp).unwrap(), "outside tmp");
        assert_eq!(fs::read_to_string(&outside_bak).unwrap(), "outside bak");
        let bak = path.with_extension("json.bak");
        assert!(!is_link(&bak), "the backup is a real file again");
        let kept: serde_json::Value = read_json(&bak).unwrap();
        assert_eq!(kept["n"], 1);
        let now: serde_json::Value = read_json(&path).unwrap();
        assert_eq!(now["n"], 2);
    }

    #[cfg(unix)]
    #[test]
    fn run_folder_helpers_refuse_a_link_and_leave_its_target_alone() {
        let dir = TempDir::new("io_run_links");
        let (link, outside) = plant_link(&dir, "x.txt");
        assert!(write_run_file(&link, b"new").is_err());
        assert!(append_run_file(&link, b"new").is_err());
        assert!(create_run_log(&link).is_err());
        assert_eq!(fs::read_to_string(&outside).unwrap(), "outside");
        // A missing file is made, an existing regular one replaced / appended to.
        let file = dir.join("run").join("plain.txt");
        write_run_file(&file, b"one").unwrap();
        append_run_file(&file, b"+two").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "one+two");
        write_run_file(&file, b"three").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "three");
        // No temp file is left behind.
        assert_eq!(names_in(&dir.join("run")), vec!["plain.txt", "x.txt"]);
    }

    #[test]
    fn read_capped_reads_at_most_the_cap_and_refuses_what_is_not_a_regular_file() {
        let dir = TempDir::new("io_capped");
        let big = dir.join("big");
        // 3 MB sparse: the size is known from metadata, but at most cap + 1 bytes are read.
        fs::File::create(&big)
            .unwrap()
            .set_len(3 * 1024 * 1024)
            .unwrap();
        assert!(matches!(
            read_capped(&big, 2 * 1024 * 1024),
            Err(CappedError::TooLarge)
        ));
        let exact = dir.join("exact");
        fs::write(&exact, vec![b'a'; 10]).unwrap();
        assert_eq!(read_capped(&exact, 10).unwrap().len(), 10);
        assert!(matches!(read_capped(&exact, 9), Err(CappedError::TooLarge)));
        assert!(matches!(
            read_capped(&dir.join("none"), 10),
            Err(CappedError::Missing)
        ));
        assert!(matches!(
            read_capped(dir.path(), 10),
            Err(CappedError::NotRegular)
        ));
        #[cfg(unix)]
        {
            let link = dir.join("link");
            std::os::unix::fs::symlink(&exact, &link).unwrap();
            assert!(matches!(
                read_capped(&link, 100),
                Err(CappedError::NotRegular)
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_at_the_old_fixed_temp_name_is_never_written_through() {
        let dir = TempDir::new("io_old_tmp");
        let outside = dir.join("outside.md");
        fs::write(&outside, "outside").unwrap();
        fs::create_dir_all(dir.join("run")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("run/.tickets-update.md.tmp")).unwrap();
        write_run_file(&dir.join("run/tickets-update.md"), b"update").unwrap();
        assert_eq!(fs::read_to_string(&outside).unwrap(), "outside");
        assert_eq!(
            fs::read_to_string(dir.join("run/tickets-update.md")).unwrap(),
            "update"
        );
    }
}
