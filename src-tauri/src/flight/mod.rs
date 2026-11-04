//! AgentFlight core (M1, the Session Inspector): reads what an agent already
//! recorded, normalizes it into schema-1 events that point at their source
//! (never copy content), correlates them into spans, an agent tree, failures
//! and repeats, and redacts content on the way out.
//!
//! The core is std + serde + regex + sha2 only, so it can become a crate
//! later (AD-1). `host.rs` is the one file that may name `crate::`; a source
//! scan in the tests below enforces it.
//!
//! - `schema`: the event and derived types (schema 1).
//! - `tail`: the JSONL tailer, with byte offsets.
//! - `adapters`: provider transcripts to events.
//! - `correlate`, `derive`: spans, tree, failures, files, repeats.
//! - `redact`: the one secret scanner (also used by ClickUp).
//! - `host`: the seam to the app (paths, accounts, policy, reads).

pub mod host;
pub mod schema;
pub mod tail;

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// What breaks the boundary in one file's non-test code.
    fn boundary_problems(src: &str) -> Vec<&'static str> {
        let live = crate::test_support::without_test_items(src);
        let mut found = Vec::new();
        for needle in ["crate::", "super::super", "tauri"] {
            if live.contains(needle) {
                found.push(needle);
            }
        }
        found
    }

    #[test]
    fn the_scan_flags_the_app_and_skips_test_items() {
        assert_eq!(boundary_problems("use crate::store::X;"), ["crate::"]);
        assert_eq!(boundary_problems("use super::super::a;"), ["super::super"]);
        assert_eq!(boundary_problems("fn f() { tauri::x(); }"), ["tauri"]);
        let test_only = "fn live() {}\n#[cfg(test)]\nmod tests {\n    use crate::a;\n}\n";
        assert!(boundary_problems(test_only).is_empty());
    }

    #[test]
    fn only_host_rs_names_the_app_outside_tests() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/flight");
        let mut files = Vec::new();
        rust_files(&root, &mut files);
        assert!(files.len() >= 4, "{files:?}");
        for file in files {
            if file.file_name().is_some_and(|n| n == "host.rs") {
                continue;
            }
            let src = std::fs::read_to_string(&file).unwrap();
            let problems = boundary_problems(&src);
            assert!(
                problems.is_empty(),
                "{} uses {problems:?} outside tests; only flight/host.rs may",
                file.display()
            );
        }
    }
}
