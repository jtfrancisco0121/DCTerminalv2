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

pub mod adapters;
pub mod correlate;
pub mod derive;
pub mod host;
pub mod index;
pub mod redact;
pub mod schema;
pub mod session;
pub mod tail;

/// Builders shared by the tests of the adapter, correlation and host.
#[cfg(test)]
pub mod testkit {
    use super::adapters::{claude, AgentMeta, SubagentFile};
    use super::schema::Event;
    use crate::test_support::TempDir;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    pub fn toy() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fixtures/claude-2.1.287/projects/-work-toy")
    }

    /// A session of the recorded fixtures: events and the sub-agent metas.
    pub fn recorded(session: &str) -> (Vec<Event>, BTreeMap<String, AgentMeta>) {
        let main = toy().join(format!("{session}.jsonl"));
        let dir = toy().join(session).join("subagents");
        let (mut subs, mut metas) = (Vec::new(), BTreeMap::new());
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(id) = name
                .strip_prefix("agent-")
                .and_then(|n| n.strip_suffix(".jsonl"))
            else {
                continue;
            };
            let meta = std::fs::read_to_string(entry.path().with_extension("meta.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .map(|v| AgentMeta::from_json(&v));
            if let Some(m) = &meta {
                metas.insert(id.to_string(), m.clone());
            }
            subs.push(SubagentFile {
                agent_id: id.to_string(),
                path: entry.path(),
                meta,
            });
        }
        (claude::events_for(&main, &subs), metas)
    }

    /// The checked-in run folder of wave 1 (stages, approvals, checks, a review, CI, a guard block and a
    /// transcript with a PR2 implementer and PR2 pr-reviewer).
    pub fn wave1_fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/run/flight-wave1")
    }

    fn copy_tree(from: &Path, to: &Path, skip_config: bool, config: &str) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if skip_config && name == "config" {
                continue;
            }
            // `logs` and `*.log` are git-ignored, so the fixture keeps them as `_logs` and `guard-log.txt`.
            let target_name = match name.as_str() {
                "_logs" => "logs",
                "guard-log.txt" => "guard.log",
                other => other,
            };
            let target = to.join(target_name);
            if entry.path().is_dir() {
                copy_tree(&entry.path(), &target, false, config);
            } else if name == "meta.json" || name == "app-usage.json" {
                let text = std::fs::read_to_string(entry.path()).unwrap();
                std::fs::write(target, text.replace("@CONFIG@", config)).unwrap();
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    /// The wave 1 fixture copied into `dir`: the run folder `r-20261105-100000-fa17` and, next to it,
    /// the account folder `config` it names. Returns `(run folder, account folder)`.
    pub fn wave1_run(dir: &TempDir) -> (PathBuf, PathBuf) {
        let config = dir.join("config");
        let run = dir.join("r-20261105-100000-fa17");
        let config_text = config.display().to_string().replace('\\', "/");
        copy_tree(&wave1_fixture(), &run, true, &config_text);
        copy_tree(
            &wave1_fixture().join("config"),
            &config,
            false,
            &config_text,
        );
        (run, config)
    }

    /// Events of a synthetic main log made of `lines`.
    pub fn synthetic(dir: &TempDir, name: &str, lines: &[Value]) -> Vec<Event> {
        let text: String = lines.iter().map(|l| l.to_string() + "\n").collect();
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        claude::events_for(&path, &[])
    }

    pub fn prompt(at: &str) -> Value {
        json!({"type": "user", "timestamp": at, "message": {"role": "user", "content": "do it"}})
    }

    pub fn tool_use(at: &str, msg: &str, id: &str, name: &str, input: Value) -> Value {
        json!({"type": "assistant", "timestamp": at, "message": {"id": msg, "model": "m",
            "content": [{"type": "tool_use", "id": id, "name": name, "input": input}],
            "usage": {"input_tokens": 1, "output_tokens": 1}}})
    }

    pub fn tool_result(at: &str, id: &str, text: &str, is_error: bool) -> Value {
        json!({"type": "user", "timestamp": at, "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": id, "content": text, "is_error": is_error}]}})
    }
}

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

    /// The run list and the sweeps never reach the inspector: no flight call
    /// from `run/` or the attention passes (AD-8).
    #[test]
    fn nothing_on_the_run_list_or_sweep_path_names_the_inspector() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&src.join("run"), &mut files);
        files.push(src.join("observe/attention.rs"));
        assert!(files.len() > 10, "{}", files.len());
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            for name in ["flight_session", "FlightCache", "flight::"] {
                assert!(
                    !text.contains(name),
                    "{} names {name}: the run list and the sweeps must not call AgentFlight",
                    file.display()
                );
            }
        }
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
