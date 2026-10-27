//! "Warn before starting the same request twice in the same folder": a new
//! Eagle-Eye start that matches an unfinished run (the same request text,
//! folder and ClickUp tickets) is refused with a typed `duplicate` answer;
//! the page asks to resume that run or start anyway (`force`).

use super::folder::{read_json_value, request_md_body};
use super::git::canonical;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// What `start_run` answers instead of starting a duplicate.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateRun {
    pub run_id: String,
    pub run_dir: String,
}

/// An earlier run, as far as the match needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Earlier {
    pub run_id: String,
    pub run_dir: String,
    /// The request's words, whitespace-normalised, without the ticket lines.
    pub request: String,
    pub folder: PathBuf,
    pub tickets: BTreeSet<String>,
    pub stage: Option<String>,
}

/// Whitespace collapsed to single spaces and trimmed.
pub fn normalize_request(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The user's words in a request.md body: the ticket lines the app adds
/// ("Tickets from ClickUp (…") are cut off.
pub fn words_of(body: &str) -> String {
    let cut = body
        .lines()
        .position(|line| line.starts_with("Tickets from ClickUp ("))
        .map(|at| body.lines().take(at).collect::<Vec<_>>().join("\n"))
        .unwrap_or_else(|| body.to_string());
    normalize_request(&cut)
}

/// The earlier run with the same request, folder and ticket set that has
/// not finished (`done` and `failed` runs never match).
pub fn find<'a>(
    earlier: &'a [Earlier],
    request: &str,
    folder: &Path,
    tickets: &BTreeSet<String>,
) -> Option<&'a Earlier> {
    let request = normalize_request(request);
    let folder = canonical(folder);
    earlier.iter().find(|run| {
        !matches!(run.stage.as_deref(), Some("done") | Some("failed"))
            && run.request == request
            && canonical(&run.folder) == folder
            && &run.tickets == tickets
    })
}

fn ticket_set(run_dir: &Path) -> BTreeSet<String> {
    read_json_value(&run_dir.join("tickets.json"))
        .and_then(|v| v.get("tasks").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|task| task.get("id").and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// Every run in the list, read for the match.
pub fn earlier_runs(runs: &[super::folder::RunListItem]) -> Vec<Earlier> {
    runs.iter()
        .filter_map(|run| {
            let dir = Path::new(&run.run_dir);
            let text = std::fs::read_to_string(dir.join("request.md")).ok()?;
            Some(Earlier {
                run_id: run.run_id.clone(),
                run_dir: run.run_dir.clone(),
                request: words_of(request_md_body(&text)),
                folder: PathBuf::from(run.folder.as_deref()?),
                tickets: ticket_set(dir),
                stage: run.stage.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn earlier(request: &str, folder: &Path, stage: Option<&str>, tickets: &[&str]) -> Earlier {
        Earlier {
            run_id: "r-1".into(),
            run_dir: "/d/runs/r-1".into(),
            request: normalize_request(request),
            folder: folder.to_path_buf(),
            tickets: tickets.iter().map(|t| t.to_string()).collect(),
            stage: stage.map(str::to_string),
        }
    }

    fn set(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn whitespace_does_not_hide_a_duplicate() {
        let dir = TempDir::new("dup_ws");
        let runs = [earlier(
            "Add   multiply()\n\nto mathlib",
            dir.path(),
            Some("building"),
            &[],
        )];
        assert!(find(
            &runs,
            "  Add multiply() to\tmathlib ",
            dir.path(),
            &set(&[])
        )
        .is_some());
        assert!(find(&runs, "Add multiply() to mathlib.", dir.path(), &set(&[])).is_none());
    }

    #[test]
    fn the_folder_is_compared_by_its_canonical_path() {
        let dir = TempDir::new("dup_folder");
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let runs = [earlier("x", &work, None, &[])];
        let roundabout = dir.join("work/../work/");
        assert!(find(&runs, "x", &roundabout, &set(&[])).is_some());
        #[cfg(unix)]
        {
            let link = dir.join("link");
            std::os::unix::fs::symlink(&work, &link).unwrap();
            assert!(find(&runs, "x", &link, &set(&[])).is_some());
        }
        let other = dir.join("other");
        std::fs::create_dir_all(&other).unwrap();
        assert!(find(&runs, "x", &other, &set(&[])).is_none());
    }

    #[test]
    fn a_different_ticket_set_is_not_a_duplicate() {
        let dir = TempDir::new("dup_tickets");
        let runs = [earlier("x", dir.path(), Some("planning"), &["a", "b"])];
        assert!(find(&runs, "x", dir.path(), &set(&["b", "a"])).is_some());
        assert!(find(&runs, "x", dir.path(), &set(&["a"])).is_none());
        assert!(find(&runs, "x", dir.path(), &set(&["a", "b", "c"])).is_none());
        assert!(find(&runs, "x", dir.path(), &set(&[])).is_none());
    }

    #[test]
    fn done_and_failed_runs_never_match() {
        let dir = TempDir::new("dup_done");
        for stage in ["done", "failed"] {
            let runs = [earlier("x", dir.path(), Some(stage), &[])];
            assert!(find(&runs, "x", dir.path(), &set(&[])).is_none(), "{stage}");
        }
        for stage in [
            None,
            Some("planning"),
            Some("plan-approval"),
            Some("building"),
            Some("shipping"),
        ] {
            let runs = [earlier("x", dir.path(), stage, &[])];
            assert!(
                find(&runs, "x", dir.path(), &set(&[])).is_some(),
                "{stage:?}"
            );
        }
    }

    #[test]
    fn the_words_of_a_stored_request_drop_the_ticket_lines() {
        let body = "Do the thing\n\nplease\n\nTickets from ClickUp (1; their details are in tickets.json):\n- 86b1 (QA-1) Title\n";
        assert_eq!(words_of(body), "Do the thing please");
        assert_eq!(words_of("Plain"), "Plain");
    }

    #[test]
    fn earlier_runs_are_read_from_their_folders() {
        let dir = TempDir::new("dup_read");
        let meta = crate::run::folder::RunMeta::default();
        let created = crate::run::folder::create_run(
            dir.path(),
            dir.path(),
            &[],
            &[],
            "Add  multiply()",
            &meta,
        )
        .unwrap();
        std::fs::write(
            Path::new(&created.run_dir).join("tickets.json"),
            r#"{"schema":1,"tasks":[{"id":"t1"},{"id":"t2"}]}"#,
        )
        .unwrap();
        let listed = crate::run::folder::list_runs(dir.path());
        let earlier = earlier_runs(&listed);
        assert_eq!(earlier.len(), 1);
        assert_eq!(earlier[0].request, "Add multiply()");
        assert_eq!(earlier[0].tickets, set(&["t1", "t2"]));
        assert!(find(&earlier, "Add multiply()", dir.path(), &set(&["t1", "t2"])).is_some());
    }
}
