//! Where Claude Code keeps a terminal tab's session log, so an exited (or
//! restored) Claude tab can offer to resume that session.
//!
//! Claude Code writes `<configDir>/projects/<encoded cwd>/<session id>.jsonl`
//! once the session has a first message. Before that there is nothing to
//! resume (`claude --resume <id>` would fail), so Resume is offered only
//! when the file exists. Read-only: the config folder is never written.

use std::path::{Path, PathBuf};

/// `<configDir>/projects/*/<id>.jsonl`, searched only under `config_dir`.
pub fn find_session_log(config_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let id = session_id.trim();
    // A Claude session id is a UUID; anything else never becomes a file name.
    if !crate::provider::claude::valid_session_id(id) {
        return None;
    }
    let name = format!("{id}.jsonl");
    let projects = std::fs::read_dir(config_dir.join("projects")).ok()?;
    projects
        .flatten()
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .map(|entry| entry.path().join(&name))
        .find(|path| path.is_file())
}

/// Claude Code's folder name for a working folder under `projects/`: every
/// character that is not a letter or a digit becomes `-`
/// (`/work/toy` is `-work-toy`).
pub fn project_dir_name(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The session's log is under the project folder of `cwd` (the folder as
/// given or its real path), so `claude --resume <id>` finds it there.
pub fn session_in_folder(config_dir: &Path, session_id: &str, cwd: &Path) -> bool {
    let id = session_id.trim();
    if !crate::provider::claude::valid_session_id(id) {
        return false;
    }
    let name = format!("{id}.jsonl");
    let real = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    [cwd, real.as_path()].iter().any(|folder| {
        config_dir
            .join("projects")
            .join(project_dir_name(&folder.display().to_string()))
            .join(&name)
            .is_file()
    })
}

/// The session a tab may resume: its saved session, when that session's log
/// exists under the folder it was written in, and that folder is still the
/// one the tab's window uses (another account cannot see the log).
pub fn resumable_session(
    saved_session: Option<&str>,
    saved_config: Option<&str>,
    current_config: &str,
) -> Option<String> {
    let id = saved_session.map(str::trim).filter(|id| !id.is_empty())?;
    let config = saved_config.map(str::trim).filter(|dir| !dir.is_empty())?;
    if !crate::provider::claude::same_config_dir(config, current_config) {
        return None;
    }
    find_session_log(Path::new(config), id).map(|_| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    const SID: &str = "685dd464-eabf-4e4c-9f6f-ac02c63aad27";

    fn write_log(account: &TempDir) -> PathBuf {
        let dir = account.join("projects").join("-w-app");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{SID}.jsonl"));
        std::fs::write(&path, "{\"type\":\"user\"}\n").unwrap();
        path
    }

    #[test]
    fn lookup_stays_in_the_given_config_dir() {
        let account = TempDir::new("session_log_acct");
        let other = TempDir::new("session_log_other");
        assert!(find_session_log(account.path(), SID).is_none());
        write_log(&other);
        assert!(find_session_log(account.path(), SID).is_none());
        let path = write_log(&account);
        assert_eq!(find_session_log(account.path(), SID), Some(path));
        assert!(find_session_log(account.path(), "../../etc/passwd").is_none());
    }

    #[test]
    fn a_session_belongs_to_the_project_folder_of_its_working_folder() {
        assert_eq!(project_dir_name("/work/toy"), "-work-toy");
        assert_eq!(
            project_dir_name("/Users/jt/My App.v2"),
            "-Users-jt-My-App-v2"
        );
        let account = TempDir::new("session_in_folder");
        let work = TempDir::new("session_in_folder_work");
        let elsewhere = TempDir::new("session_in_folder_else");
        let real = work.path().canonicalize().unwrap();
        let project = account
            .join("projects")
            .join(project_dir_name(&real.display().to_string()));
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join(format!("{SID}.jsonl")), "{}\n").unwrap();
        assert!(session_in_folder(account.path(), SID, work.path()));
        assert!(!session_in_folder(account.path(), SID, elsewhere.path()));
        assert!(!session_in_folder(account.path(), "../../x", work.path()));
        assert!(!session_in_folder(
            &elsewhere.join("none"),
            SID,
            work.path()
        ));
    }

    #[test]
    fn a_session_is_resumable_only_with_its_log_under_the_same_account() {
        let account = TempDir::new("session_resume_acct");
        let other = TempDir::new("session_resume_other");
        let dir = account.path().display().to_string();
        assert_eq!(
            resumable_session(Some(SID), Some(&dir), &dir),
            None,
            "no log yet"
        );
        write_log(&account);
        assert_eq!(
            resumable_session(Some(SID), Some(&dir), &dir).as_deref(),
            Some(SID)
        );
        let moved = other.path().display().to_string();
        assert_eq!(
            resumable_session(Some(SID), Some(&dir), &moved),
            None,
            "another account"
        );
        assert_eq!(resumable_session(None, Some(&dir), &dir), None);
        assert_eq!(resumable_session(Some(SID), None, &dir), None);
    }
}
