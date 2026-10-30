//! The fleet timeline of a run (AD-21): one lane per sub-agent (and one for the orchestrator), with
//! start, finish, state and cost, plus the stage bands and approval waits of log.md and the totals
//! for work, waiting and cost. Read-only: it reads `app-usage.json`'s sessions, their Claude logs
//! and log.md, and writes nothing. A finished run comes from the same record as a live one.

use super::record::{read_record, SessionRef};
use crate::observe::transcript::{parse_time_ms, SessionTracker};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Lane {
    pub id: String,
    /// The agent type (`dcv2:implementer` -> `implementer`; an unknown type keeps its name).
    pub label: String,
    pub description: Option<String>,
    /// `completed`, `running`, `failed`, `killed`, `spawned`, or `unfinished` (no finish was recorded).
    pub state: String,
    pub start_ms: Option<i64>,
    pub finish_ms: Option<i64>,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Band {
    /// `stage` or `wait` (an approval stop: the run waited for the user).
    pub kind: String,
    pub name: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Timeline {
    pub lanes: Vec<Lane>,
    pub bands: Vec<Band>,
    /// Minutes between the first and last log line that were not an approval wait.
    pub work_minutes: f64,
    pub wait_minutes: f64,
    pub cost_usd: Option<f64>,
    pub started_ms: Option<i64>,
    pub ended_ms: Option<i64>,
}

fn short(kind: &str) -> String {
    kind.strip_prefix("dcv2:").unwrap_or(kind).to_string()
}

/// `(time, stage, text)` of every log.md line (`<UTC time> <stage> <text>`).
fn log_lines(text: &str) -> Vec<(i64, String, String)> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, ' ');
            let at = parse_time_ms(parts.next()?)?;
            Some((
                at,
                parts.next()?.to_string(),
                parts.next().unwrap_or("").to_string(),
            ))
        })
        .collect()
}

/// Stage bands (consecutive lines of one stage) and approval waits (from a `waiting for ... approval`
/// line to the next line that is not one).
pub fn bands(log: &str) -> Vec<Band> {
    let lines = log_lines(log);
    let mut out: Vec<Band> = Vec::new();
    for (i, (at, stage, text)) in lines.iter().enumerate() {
        let next = lines.get(i + 1).map(|l| l.0);
        if text.to_ascii_lowercase().starts_with("waiting for")
            && text.to_ascii_lowercase().contains("approval")
        {
            let end = next.unwrap_or(*at);
            // A wait that continues over repeated "waiting" lines is one band.
            if let Some(last) = out
                .last_mut()
                .filter(|b| b.kind == "wait" && b.end_ms == *at)
            {
                last.end_ms = end;
            } else {
                out.push(Band {
                    kind: "wait".into(),
                    name: text.trim().to_string(),
                    start_ms: *at,
                    end_ms: end,
                });
            }
        }
        match out.iter_mut().rev().find(|b| b.kind == "stage") {
            Some(band) if band.name == *stage && band.end_ms >= *at => {
                band.end_ms = next.unwrap_or(*at).max(*at)
            }
            _ => out.push(Band {
                kind: "stage".into(),
                name: stage.clone(),
                start_ms: *at,
                end_ms: next.unwrap_or(*at).max(*at),
            }),
        }
    }
    out
}

pub fn timeline(run_dir: &Path) -> Timeline {
    let log = std::fs::read_to_string(run_dir.join("log.md")).unwrap_or_default();
    let bands = bands(&log);
    let lines = log_lines(&log);
    let started = lines.first().map(|l| l.0);
    let ended = lines.last().map(|l| l.0);
    let wait_ms: i64 = bands
        .iter()
        .filter(|b| b.kind == "wait")
        .map(|b| b.end_ms - b.start_ms)
        .sum();
    let span_ms = match (started, ended) {
        (Some(a), Some(b)) => (b - a).max(0),
        _ => 0,
    };
    let minutes = |ms: i64| (ms as f64 / 6_000.0).round() / 10.0;
    let mut lanes: Vec<Lane> = Vec::new();
    let mut total: Option<f64> = None;
    let sessions: Vec<SessionRef> = read_record(run_dir).map(|r| r.sessions).unwrap_or_default();
    for (n, session) in sessions.iter().enumerate() {
        let Some(file) = crate::claude_session_log::find_session_log(
            Path::new(&session.config_dir),
            &session.session_id,
        ) else {
            continue;
        };
        let mut tracker = SessionTracker::for_file(file);
        tracker.poll();
        let costs = tracker.cost_by_agent();
        for c in costs.values() {
            total = Some(total.unwrap_or(0.0) + c.unwrap_or(0.0));
        }
        let span = tracker.activity_span();
        lanes.push(Lane {
            id: format!("session-{n}"),
            label: "orchestrator".into(),
            description: None,
            state: "completed".into(),
            start_ms: span.map(|s| s.0),
            finish_ms: span.map(|s| s.1),
            cost_usd: costs.get(&None).copied().flatten(),
        });
        for sub in tracker.subagents() {
            let finished = sub.finished_ms.is_some();
            lanes.push(Lane {
                id: format!("{n}-{}", sub.tool_use_id),
                label: short(&sub.agent_type),
                description: sub.description.clone(),
                state: if !finished && matches!(sub.status.as_str(), "spawned" | "completed") {
                    "unfinished".into()
                } else {
                    sub.status.clone()
                },
                start_ms: sub.started_ms,
                finish_ms: sub.finished_ms,
                cost_usd: sub
                    .agent_id
                    .as_ref()
                    .and_then(|id| costs.get(&Some(id.clone())).copied().flatten()),
            });
        }
    }
    Timeline {
        lanes,
        bands,
        work_minutes: minutes((span_ms - wait_ms).max(0)),
        wait_minutes: minutes(wait_ms),
        cost_usd: total,
        started_ms: started,
        ended_ms: ended,
    }
}

#[tauri::command]
pub async fn run_timeline(app: tauri::AppHandle, run_dir: String) -> Result<Timeline, String> {
    let data = crate::data_dir::app_data_dir(&app).path;
    tauri::async_runtime::spawn_blocking(move || {
        let dir = super::checked_run_dir(&data, &run_dir)?;
        Ok(timeline(&dir))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    const SID: &str = "11111111-2222-4333-8444-555555555555";

    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    fn run_from_fixture(dir: &TempDir) -> std::path::PathBuf {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/timeline");
        let run = dir.join("run");
        std::fs::create_dir_all(&run).unwrap();
        copy(&fixture.join("config"), &dir.join("config"));
        std::fs::copy(fixture.join("log.md"), run.join("log.md")).unwrap();
        std::fs::write(
            run.join("app-usage.json"),
            serde_json::json!({"schema": 1, "sessions": [{"sessionId": SID, "configDir": dir.join("config").display().to_string()}]})
                .to_string(),
        )
        .unwrap();
        run
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        out.sort();
        out
    }

    #[test]
    fn the_fixture_gives_one_lane_per_subagent_with_state_times_cost_and_bands() {
        let dir = TempDir::new("timeline");
        let run = run_from_fixture(&dir);
        let before = files_in(&run);
        let t = timeline(&run);
        assert_eq!(files_in(&run), before, "run_timeline writes no file");
        let lane = |label: &str| {
            t.lanes
                .iter()
                .find(|l| l.label == label)
                .unwrap_or_else(|| panic!("{label}: {:?}", t.lanes))
        };
        assert_eq!(
            t.lanes.len(),
            4,
            "orchestrator + three sub-agents: {:?}",
            t.lanes
        );
        let reviewer = lane("plan-reviewer");
        assert_eq!(reviewer.state, "completed");
        assert!(reviewer.start_ms.is_some() && reviewer.finish_ms.is_some());
        assert!(
            reviewer.cost_usd.unwrap() > 0.0,
            "cost from the price table"
        );
        // An unknown agent type keeps its own lane under its own name.
        assert_eq!(lane("custom:mystery").state, "completed");
        // A sub-agent that never finished has no finish and says so.
        let unfinished = lane("implementer");
        assert_eq!(unfinished.state, "unfinished");
        assert!(unfinished.finish_ms.is_none());
        assert!(t.cost_usd.unwrap() > 0.0);
        // Stage bands and the two approval waits from log.md.
        let waits: Vec<&Band> = t.bands.iter().filter(|b| b.kind == "wait").collect();
        assert_eq!(waits.len(), 2, "{:?}", t.bands);
        assert_eq!(waits[0].end_ms - waits[0].start_ms, 3 * 60_000);
        assert!(t
            .bands
            .iter()
            .any(|b| b.kind == "stage" && b.name == "building"));
        assert_eq!(t.wait_minutes, 3.5);
        assert!(t.work_minutes > 0.0 && t.work_minutes < 12.0);
    }

    #[test]
    fn a_run_with_no_record_or_no_log_gives_an_empty_timeline() {
        let dir = TempDir::new("timeline_empty");
        let t = timeline(dir.path());
        assert!(t.lanes.is_empty() && t.bands.is_empty());
        assert_eq!((t.work_minutes, t.wait_minutes), (0.0, 0.0));
    }
}
