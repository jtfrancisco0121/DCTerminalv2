//! Working-folder checks. Paths are passed to the PTY's `cwd` as `PathBuf`
//! values, never interpolated into a shell string.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderError {
    Empty,
    Missing { path: String },
    NotDirectory { path: String },
    Unreadable { path: String },
}

impl FolderError {
    pub fn message(&self) -> String {
        match self {
            FolderError::Empty => "Working folder is required".to_string(),
            FolderError::Missing { path } => {
                format!("Working folder was not found (it may have been moved or deleted): {path}")
            }
            FolderError::NotDirectory { path } => {
                format!("Working folder must be a directory, not a file: {path}")
            }
            FolderError::Unreadable { path } => {
                format!("Working folder is not readable: {path}")
            }
        }
    }

    pub fn status_code(&self) -> &'static str {
        match self {
            FolderError::Empty => "empty",
            FolderError::Missing { .. } => "missing",
            FolderError::NotDirectory { .. } => "not-a-directory",
            FolderError::Unreadable { .. } => "unreadable",
        }
    }
}

pub fn validate_working_folder(raw: &str) -> Result<PathBuf, FolderError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(FolderError::Empty);
    }
    if trimmed.contains('\0') {
        return Err(FolderError::Missing {
            path: trimmed.to_string(),
        });
    }
    let path = PathBuf::from(trimmed);
    inspect_folder(&path)
}

pub fn folder_status_code(raw: &str) -> String {
    match validate_working_folder(raw) {
        Ok(_) => "ok".to_string(),
        Err(err) => err.status_code().to_string(),
    }
}

fn inspect_folder(path: &Path) -> Result<PathBuf, FolderError> {
    let display = path.display().to_string();
    match path.metadata() {
        Ok(meta) if meta.is_dir() => match std::fs::read_dir(path) {
            Ok(_) => Ok(path.to_path_buf()),
            Err(err) if err.kind() == ErrorKind::PermissionDenied => {
                Err(FolderError::Unreadable { path: display })
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                Err(FolderError::Missing { path: display })
            }
            Err(_) => Err(FolderError::Unreadable { path: display }),
        },
        Ok(_) => Err(FolderError::NotDirectory { path: display }),
        Err(err) if err.kind() == ErrorKind::PermissionDenied => {
            Err(FolderError::Unreadable { path: display })
        }
        Err(_) => Err(FolderError::Missing { path: display }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch_dir() -> PathBuf {
        // Unique per call: two tests in the same clock tick must not share a
        // folder (one of them makes its folder unreadable).
        crate::test_support::temp_path("dctv2_paths")
    }

    #[test]
    fn rejects_blank_missing_and_file() {
        assert!(matches!(
            validate_working_folder("   "),
            Err(FolderError::Empty)
        ));
        let missing = scratch_dir().join("does-not-exist");
        assert!(matches!(
            validate_working_folder(&missing.display().to_string()),
            Err(FolderError::Missing { .. })
        ));
        let dir = scratch_dir();
        let file = dir.join("not-a-dir.txt");
        fs::write(&file, b"x").unwrap();
        let err = validate_working_folder(&file.display().to_string()).unwrap_err();
        assert!(matches!(err, FolderError::NotDirectory { .. }), "{err:?}");
        assert!(validate_working_folder(&dir.display().to_string()).is_ok());
        let _ = fs::remove_dir_all(dir);
        assert_eq!(folder_status_code(""), "empty");
        assert_eq!(folder_status_code("   "), "empty");
        assert_eq!(
            folder_status_code(&missing.display().to_string()),
            "missing"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directory_is_rejected_when_not_root() {
        use std::os::unix::fs::PermissionsExt;
        if is_root() {
            return;
        }
        let dir = scratch_dir();
        let perms = fs::Permissions::from_mode(0o000);
        fs::set_permissions(&dir, perms).unwrap();
        let err = validate_working_folder(&dir.display().to_string());
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&dir);
        assert!(
            matches!(err, Err(FolderError::Unreadable { .. })),
            "{err:?}"
        );
    }

    #[cfg(unix)]
    fn is_root() -> bool {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|line| line.starts_with("Uid:"))
                    .map(|line| line.split_whitespace().nth(1) == Some("0"))
            })
            .unwrap_or(false)
    }
}
