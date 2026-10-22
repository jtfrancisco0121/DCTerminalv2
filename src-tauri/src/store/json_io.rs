use serde::Serialize;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// Data files moved aside since startup, as one line each for the user.
/// Process-wide so every store can add to it without an AppHandle.
static RECOVERY_NOTES: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "path has no parent".to_string())?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;

    // Only a file that still parses becomes the backup: a damaged one that
    // could not be moved aside on load must not replace the good `.bak`.
    if path.exists() && read_json::<serde_json::Value>(path).is_ok() {
        let backup = path.with_extension("json.bak");
        let _ = fs::copy(path, backup);
    }

    let tmp = path.with_extension("json.tmp");
    let mut data = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    if !data.ends_with(b"\n") {
        data.push(b'\n');
    }
    {
        let mut file = fs::File::create(&tmp).map_err(|e| e.to_string())?;
        file.write_all(&data).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }

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
/// A corrupt transcript or scratch file must not stop the app from opening.
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

/// `schemaVersion` (or `schema_version`, as roles.json spells it) in `value`.
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
    if copied && fs::copy(source, &tmp).is_err() {
        return;
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
}
