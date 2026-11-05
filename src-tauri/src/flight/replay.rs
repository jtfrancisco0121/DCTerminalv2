//! Event replay (AF13): the text of a file after each recorded edit.
//!
//! Pure: a base text and the edits an agent made (`Write`, `Edit`, `MultiEdit`) are applied in order
//! to rebuild the file. Nothing here runs a command, reads a file or touches a folder: the host
//! reads the base from git and the edit inputs from the transcript and hands them in. Changes made by
//! shell commands are not in a transcript's edit tools, so they are never seen.

/// One replacement of an `Edit` (or one step of a `MultiEdit`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    pub old: String,
    pub new: String,
    pub replace_all: bool,
}

/// A recorded edit tool call that succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// The whole file is replaced.
    Write {
        content: String,
    },
    Edit(Replacement),
    /// The steps apply in order; if one does not apply, none does.
    MultiEdit {
        edits: Vec<Replacement>,
    },
}

/// The file after the edits that applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileState {
    /// `None`: the file does not exist yet (no base, no edit created it).
    pub text: Option<String>,
    /// How many edits applied.
    pub applied: usize,
    /// The index of the edit that did not apply; the text is the state before it.
    pub stopped_at: Option<usize>,
    pub reason: Option<String>,
}

/// A file is not rebuilt past this size.
pub const MAX_FILE: usize = 2 * 1024 * 1024;

fn apply_one(text: &mut Option<String>, r: &Replacement) -> Result<(), String> {
    let current = text.as_deref().unwrap_or("");
    if r.old.is_empty() {
        // The edit tool creates a file from an empty `old_string`; it is not a replacement otherwise.
        return if current.is_empty() {
            *text = Some(r.new.clone());
            Ok(())
        } else {
            Err("an edit with an empty old text does not apply to a file with content".into())
        };
    }
    if text.is_none() {
        return Err("the file did not exist at that point".into());
    }
    let found = current.matches(r.old.as_str()).count();
    if found == 0 {
        return Err("the text this edit replaces is not in the file".into());
    }
    if found > 1 && !r.replace_all {
        return Err("the text this edit replaces appears more than once".into());
    }
    let next = if r.replace_all {
        current.replace(r.old.as_str(), r.new.as_str())
    } else {
        current.replacen(r.old.as_str(), r.new.as_str(), 1)
    };
    if next.len() > MAX_FILE {
        return Err("the file grows past 2 MiB".into());
    }
    *text = Some(next);
    Ok(())
}

fn apply(text: &mut Option<String>, edit: &Edit) -> Result<(), String> {
    match edit {
        Edit::Write { content } => {
            if content.len() > MAX_FILE {
                return Err("the file grows past 2 MiB".into());
            }
            *text = Some(content.clone());
            Ok(())
        }
        Edit::Edit(r) => apply_one(text, r),
        Edit::MultiEdit { edits } => {
            // Work on a copy: a step that fails leaves the file as it was before the whole call.
            let mut work = text.clone();
            for r in edits {
                apply_one(&mut work, r)?;
            }
            *text = work;
            Ok(())
        }
    }
}

/// Applies `edits` to `base` in order. An edit that does not apply stops the file: the state before it is
/// returned with `stopped_at` and the reason.
pub fn reconstruct(base: Option<&str>, edits: &[Edit]) -> FileState {
    let mut text = base.map(str::to_string);
    for (i, edit) in edits.iter().enumerate() {
        if let Err(reason) = apply(&mut text, edit) {
            return FileState {
                text,
                applied: i,
                stopped_at: Some(i),
                reason: Some(reason),
            };
        }
    }
    FileState {
        text,
        applied: edits.len(),
        stopped_at: None,
        reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(old: &str, new: &str, all: bool) -> Replacement {
        Replacement {
            old: old.into(),
            new: new.into(),
            replace_all: all,
        }
    }

    #[test]
    fn a_write_replaces_the_whole_file_and_creates_a_missing_one() {
        let s = reconstruct(
            Some("old\n"),
            &[Edit::Write {
                content: "new\n".into(),
            }],
        );
        assert_eq!(s.text.as_deref(), Some("new\n"));
        assert_eq!((s.applied, s.stopped_at), (1, None));
        let s = reconstruct(
            None,
            &[Edit::Write {
                content: "x".into(),
            }],
        );
        assert_eq!(s.text.as_deref(), Some("x"));
    }

    #[test]
    fn an_edit_replaces_one_occurrence_and_replace_all_every_one() {
        let s = reconstruct(Some("a b a"), &[Edit::Edit(rep("b", "c", false))]);
        assert_eq!(s.text.as_deref(), Some("a c a"));
        let s = reconstruct(Some("a b a"), &[Edit::Edit(rep("a", "z", true))]);
        assert_eq!(s.text.as_deref(), Some("z b z"));
    }

    #[test]
    fn a_multi_edit_applies_in_order_and_is_all_or_nothing() {
        let edits = vec![rep("one", "two", false), rep("two", "three", false)];
        let s = reconstruct(Some("one"), &[Edit::MultiEdit { edits }]);
        assert_eq!(s.text.as_deref(), Some("three"));
        let edits = vec![rep("one", "two", false), rep("missing", "x", false)];
        let s = reconstruct(Some("one"), &[Edit::MultiEdit { edits }]);
        assert_eq!(
            s.text.as_deref(),
            Some("one"),
            "no step of a failed call stays"
        );
        assert_eq!(s.stopped_at, Some(0));
    }

    #[test]
    fn an_absent_old_text_stops_the_file_with_the_state_before_it() {
        let s = reconstruct(
            Some("abc"),
            &[
                Edit::Edit(rep("a", "A", false)),
                Edit::Edit(rep("zzz", "x", false)),
                Edit::Edit(rep("b", "B", false)),
            ],
        );
        assert_eq!(s.text.as_deref(), Some("Abc"));
        assert_eq!((s.applied, s.stopped_at), (1, Some(1)));
        assert!(s.reason.unwrap().contains("not in the file"));
    }

    #[test]
    fn a_duplicate_old_text_without_replace_all_stops_the_file() {
        let s = reconstruct(Some("x x"), &[Edit::Edit(rep("x", "y", false))]);
        assert_eq!(s.text.as_deref(), Some("x x"));
        assert_eq!(s.stopped_at, Some(0));
        assert!(s.reason.unwrap().contains("more than once"));
    }

    #[test]
    fn an_edit_of_a_file_that_does_not_exist_stops_but_an_empty_old_text_creates_it() {
        let s = reconstruct(None, &[Edit::Edit(rep("a", "b", false))]);
        assert_eq!(s.stopped_at, Some(0));
        let s = reconstruct(None, &[Edit::Edit(rep("", "fresh", false))]);
        assert_eq!(s.text.as_deref(), Some("fresh"));
        let s = reconstruct(Some("has text"), &[Edit::Edit(rep("", "x", false))]);
        assert_eq!(s.stopped_at, Some(0));
    }

    #[test]
    fn five_hundred_edits_rebuild_in_under_a_second() {
        let base: String = (0..500).map(|i| format!("line {i}\n")).collect();
        let edits: Vec<Edit> = (0..500)
            .map(|i| Edit::Edit(rep(&format!("line {i}\n"), &format!("LINE {i}\n"), false)))
            .collect();
        let t = std::time::Instant::now();
        let s = reconstruct(Some(&base), &edits);
        assert!(t.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(s.applied, 500);
        assert!(s.text.unwrap().starts_with("LINE 0\nLINE 1\n"));
    }

    #[test]
    fn replay_rs_names_no_process_file_access_or_app_code() {
        let live = crate::test_support::without_test_items(include_str!("replay.rs"));
        for banned in [
            "crate::",
            "std::process",
            "Command",
            "quiet_command",
            "run_one",
            "std::fs",
            "std::net",
        ] {
            assert!(!live.contains(banned), "replay.rs names {banned}");
        }
    }
}
