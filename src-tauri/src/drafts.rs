//! The prompt scratchpad's drafts (`<app data>/drafts.json`, schema 1): one
//! draft per pane and the last 50 sent prompts. Written atomically; fields
//! this version does not know are kept when it writes the file back.

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

pub const HISTORY_MAX: usize = 50;
/// One draft or history entry is cut here (a prompt is long, not a file).
pub const TEXT_MAX: usize = 200_000;

pub fn drafts_path(data: &Path) -> PathBuf {
    data.join("drafts.json")
}

#[derive(Debug, Default, serde::Serialize, PartialEq, Eq)]
pub struct Drafts {
    pub drafts: BTreeMap<String, String>,
    pub history: Vec<String>,
}

fn read_value(path: &Path) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

fn clip(text: &str) -> String {
    text.chars().take(TEXT_MAX).collect()
}

/// The drafts and history in the file (empty when it is missing or unreadable).
pub fn load(data: &Path) -> Drafts {
    let value = read_value(&drafts_path(data));
    let drafts = value["drafts"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let history = value["history"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Drafts { drafts, history }
}

/// Writes the drafts and the last [`HISTORY_MAX`] history entries; unknown
/// top-level fields of the existing file are kept. Empty drafts are dropped.
pub fn save(
    data: &Path,
    drafts: &BTreeMap<String, String>,
    history: &[String],
) -> Result<(), String> {
    let path = drafts_path(data);
    let mut root: Map<String, Value> = read_value(&path).as_object().cloned().unwrap_or_default();
    let kept: Map<String, Value> = drafts
        .iter()
        .filter(|(_, text)| !text.is_empty())
        .map(|(k, text)| (k.clone(), json!(clip(text))))
        .collect();
    let start = history.len().saturating_sub(HISTORY_MAX);
    root.insert("schema".into(), json!(1));
    root.insert("drafts".into(), Value::Object(kept));
    root.insert(
        "history".into(),
        json!(history[start..].iter().map(|h| clip(h)).collect::<Vec<_>>()),
    );
    crate::store::write_json_atomic(&path, &Value::Object(root))
}

#[tauri::command]
pub async fn drafts_load(app: AppHandle) -> Result<Drafts, String> {
    let data = crate::data_dir::app_data_dir(&app).path;
    tauri::async_runtime::spawn_blocking(move || load(&data))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn drafts_save(
    app: AppHandle,
    drafts: BTreeMap<String, String>,
    history: Vec<String>,
) -> Result<(), String> {
    let data = crate::data_dir::app_data_dir(&app).path;
    tauri::async_runtime::spawn_blocking(move || save(&data, &drafts, &history))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dcv2-drafts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_saved_draft_comes_back_after_a_restart_and_unknown_fields_survive() {
        let dir = temp("round");
        std::fs::write(
            drafts_path(&dir),
            r#"{"schema":1,"drafts":{},"history":[],"futureField":{"a":1}}"#,
        )
        .unwrap();
        let mut drafts = BTreeMap::new();
        drafts.insert(
            "tab-1".to_string(),
            "fix the flaky test\nand the retry".to_string(),
        );
        drafts.insert("tab-2".to_string(), String::new());
        save(&dir, &drafts, &["first".to_string(), "second".to_string()]).unwrap();
        let back = load(&dir);
        assert_eq!(back.drafts.len(), 1, "an empty draft is not kept");
        assert_eq!(back.drafts["tab-1"], "fix the flaky test\nand the retry");
        assert_eq!(back.history, ["first", "second"]);
        let raw: Value =
            serde_json::from_str(&std::fs::read_to_string(drafts_path(&dir)).unwrap()).unwrap();
        assert_eq!(raw["futureField"]["a"], 1);
        assert_eq!(raw["schema"], 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn history_keeps_the_last_fifty_and_a_missing_file_is_empty() {
        let dir = temp("history");
        assert_eq!(load(&dir), Drafts::default());
        let history: Vec<String> = (0..80).map(|n| format!("prompt {n}")).collect();
        save(&dir, &BTreeMap::new(), &history).unwrap();
        let back = load(&dir);
        assert_eq!(back.history.len(), HISTORY_MAX);
        assert_eq!(back.history.first().unwrap(), "prompt 30");
        assert_eq!(back.history.last().unwrap(), "prompt 79");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_reads_as_empty() {
        let dir = temp("bad");
        std::fs::write(drafts_path(&dir), "{not json").unwrap();
        assert_eq!(load(&dir), Drafts::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
