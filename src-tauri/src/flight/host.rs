//! The seam between the flight core and the app: the only file in `flight/`
//! that may use `crate::` (S6 and S7 add accounts, policy and file reads).
//! For now it holds path helpers.

use std::path::{Path, PathBuf};

/// The folder of a session's sub-agent transcripts, next to the main log:
/// `<project>/<session id>/subagents/`.
pub fn subagents_dir(main: &Path) -> PathBuf {
    main.with_extension("").join("subagents")
}

/// `(agent id, transcript, meta file)` for every `agent-<id>.jsonl` of a
/// session, sorted by agent id so two reads agree.
pub fn subagent_files(main: &Path) -> Vec<(String, PathBuf, PathBuf)> {
    let dir = subagents_dir(main);
    let mut out: Vec<(String, PathBuf, PathBuf)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?.to_string();
            let id = name.strip_prefix("agent-")?.strip_suffix(".jsonl")?;
            Some((
                id.to_string(),
                path.clone(),
                path.with_extension("meta.json"),
            ))
        })
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn sub_agent_files_sit_next_to_the_main_log() {
        let dir = TempDir::new("flight_host_paths");
        let main = dir.join("proj/abc.jsonl");
        let subs = subagents_dir(&main);
        std::fs::create_dir_all(&subs).unwrap();
        std::fs::write(subs.join("agent-b2.jsonl"), "{}\n").unwrap();
        std::fs::write(subs.join("agent-a1.jsonl"), "{}\n").unwrap();
        std::fs::write(subs.join("agent-a1.meta.json"), "{}").unwrap();
        let got = subagent_files(&main);
        let ids: Vec<&str> = got.iter().map(|(id, _, _)| id.as_str()).collect();
        assert_eq!(ids, ["a1", "b2"]);
        assert!(got[0].2.ends_with("agent-a1.meta.json"));
        assert!(subagent_files(&dir.join("none.jsonl")).is_empty());
    }
}
