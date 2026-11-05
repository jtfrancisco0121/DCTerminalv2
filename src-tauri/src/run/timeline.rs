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
    /// The index of the run's recorded session this lane belongs to.
    pub session: usize,
    /// The sub-agent's id (`None` for the orchestrator lane), the filter a
    /// trace opens on.
    pub agent_id: Option<String>,
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

/// `PR<digits>`.
fn is_pr_id(word: &str) -> bool {
    word.strip_prefix("PR")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Text that is not run progress: the app's own notes (`app: ...`, attention and cleanup) and the
/// plugin's two records (`stop-check <PR|-> pass|fail|skip`, `pr <PR> <old> -> <new>`, plugin
/// 0.7.0). They never end an approval wait and never extend the span. The same two forms
/// `state.sh log` refuses. This is the only place that decides it.
fn not_progress(text: &str) -> bool {
    if text.starts_with("app:") {
        return true;
    }
    let words: Vec<&str> = text.split(' ').collect();
    match words.as_slice() {
        ["stop-check", pr, result] => {
            (*pr == "-" || is_pr_id(pr)) && matches!(*result, "pass" | "fail" | "skip")
        }
        ["pr", pr, old, "->", new] => is_pr_id(pr) && !old.is_empty() && !new.is_empty(),
        _ => false,
    }
}

/// One progress line of log.md: its time, stage, text and the byte offset of the line.
struct Entry {
    at: i64,
    stage: String,
    text: String,
    offset: u64,
}

fn log_entries(text: &str) -> Vec<Entry> {
    let mut offset = 0u64;
    let mut out = Vec::new();
    for raw in text.split_inclusive('\n') {
        let start = offset;
        offset += raw.len() as u64;
        let line = raw.trim_end_matches(['\n', '\r']);
        let mut parts = line.splitn(3, ' ');
        let (Some(time), Some(stage)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Some(at) = parse_time_ms(time) else {
            continue;
        };
        let text = parts.next().unwrap_or("");
        if !not_progress(text) {
            out.push(Entry {
                at,
                stage: stage.to_string(),
                text: text.to_string(),
                offset: start,
            });
        }
    }
    out
}

/// `(time, stage, text)` of every progress log.md line (`<UTC time> <stage> <text>`); see
/// [`not_progress`] for the lines that are left out.
fn log_lines(text: &str) -> Vec<(i64, String, String)> {
    log_entries(text)
        .into_iter()
        .map(|e| (e.at, e.stage, e.text))
        .collect()
}

fn minutes(ms: i64) -> f64 {
    (ms as f64 / 6_000.0).round() / 10.0
}

/// `(wait, span)` in minutes (one decimal) from log.md: approval waits, and first to last timed
/// line. `(None, None)` when the log has no timed line.
pub fn wait_and_span(log: &str) -> (Option<f64>, Option<f64>) {
    let lines = log_lines(log);
    let (Some(first), Some(last)) = (lines.first(), lines.last()) else {
        return (None, None);
    };
    let wait_ms: i64 = bands(log)
        .iter()
        .filter(|b| b.kind == "wait")
        .map(|b| b.end_ms - b.start_ms)
        .sum();
    (
        Some(minutes(wait_ms)),
        Some(minutes((last.0 - first.0).max(0))),
    )
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

/// One approval stop: from its `waiting for ... approval` line (or the run of them) to the next
/// progress line. `kind` is `plan`, `ship` or `other`; the offsets are the byte offsets of the
/// opening and closing lines of log.md; `outcome` comes from the closing line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalBand {
    pub kind: String,
    pub name: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub open_offset: u64,
    pub close_offset: Option<u64>,
    /// `approved`, `declined`, `ended` (any other line) or `open` (nothing followed).
    pub outcome: String,
}

fn is_wait(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.starts_with("waiting for") && lower.contains("approval")
}

fn outcome_of(text: &str) -> &'static str {
    match text.trim().to_ascii_lowercase().as_str() {
        "plan approved by the user"
        | "ship approved by the user"
        | "plan auto-approved (test-mode)"
        | "ship auto-approved (test-mode)"
        | "plan approved for eval (eval mode)" => "approved",
        "plan not approved; run paused"
        | "ship declined; run paused"
        | "plan changes requested" => "declined",
        _ => "ended",
    }
}

/// The approval stops of log.md, from the same line filter and walk as [`bands`], so their total
/// time is the wait time `wait_and_span` stores. No other module parses approval text.
pub fn approval_bands(log: &str) -> Vec<ApprovalBand> {
    let entries = log_entries(log);
    let mut out: Vec<ApprovalBand> = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        if !is_wait(&entries[i].text) {
            i += 1;
            continue;
        }
        let first = i;
        while i + 1 < entries.len() && is_wait(&entries[i + 1].text) {
            i += 1;
        }
        let close = entries.get(i + 1);
        let name = entries[first].text.trim().to_string();
        let lower = name.to_ascii_lowercase();
        out.push(ApprovalBand {
            kind: if lower.contains("plan approval") {
                "plan"
            } else if lower.contains("ship approval") {
                "ship"
            } else {
                "other"
            }
            .into(),
            name,
            start_ms: entries[first].at,
            end_ms: close.map_or(entries[i].at, |c| c.at),
            open_offset: entries[first].offset,
            close_offset: close.map(|c| c.offset),
            outcome: close.map_or("open", |c| outcome_of(&c.text)).into(),
        });
        i += 1;
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
            session: n,
            agent_id: None,
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
                session: n,
                agent_id: sub.agent_id.clone(),
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
    fn a_lane_names_its_session_and_agent_so_a_trace_can_open_on_it() {
        let dir = TempDir::new("timeline_lane_ids");
        let run = run_from_fixture(&dir);
        let t = timeline(&run);
        let json = serde_json::to_value(&t.lanes).unwrap();
        let orchestrator = &json[0];
        assert_eq!(orchestrator["session"], 0);
        assert!(orchestrator["agentId"].is_null());
        let sub = t.lanes.iter().find(|l| l.agent_id.is_some()).unwrap();
        let sub_json = serde_json::to_value(sub).unwrap();
        assert_eq!(sub_json["session"], 0);
        assert!(sub_json["agentId"].is_string());
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

    /// The shape of r-20261029-041728-330f: the app's notes sit right after the waiting line.
    const REAL_SHAPED: &str = "\
2026-10-29T04:40:00Z planning plan ready
2026-10-29T04:51:27Z planning waiting for plan approval
2026-10-29T04:51:27Z planning app: attention approval-wait
2026-10-29T05:01:27Z planning app: attention waiting-reminder
2026-10-29T05:02:27Z building plan approved
2026-10-29T05:30:00Z done run done
2026-10-29T09:30:00Z done app: cleanup
";

    #[test]
    fn app_lines_neither_end_a_wait_nor_extend_the_span() {
        let (wait, span) = wait_and_span(REAL_SHAPED);
        assert_eq!(wait, Some(11.0));
        assert_eq!(span, Some(50.0));
        let waits: Vec<Band> = bands(REAL_SHAPED)
            .into_iter()
            .filter(|b| b.kind == "wait")
            .collect();
        assert_eq!(waits.len(), 1);
        assert_eq!(waits[0].end_ms - waits[0].start_ms, 11 * 60_000);
    }

    #[test]
    fn wait_and_span_sums_waits_and_is_none_without_timed_lines() {
        let log = "\
2026-10-29T10:00:00Z planning start
2026-10-29T10:01:00Z planning waiting for plan approval
2026-10-29T10:03:00Z building approved
2026-10-29T10:10:00Z shipping waiting for ship approval
2026-10-29T10:12:30Z done run done
";
        assert_eq!(wait_and_span(log), (Some(4.5), Some(12.5)));
        let none = "2026-10-29T10:00:00Z a start\n2026-10-29T10:10:00Z a end\n";
        assert_eq!(wait_and_span(none), (Some(0.0), Some(10.0)));
        assert_eq!(wait_and_span("no timed lines here\n"), (None, None));
        assert_eq!(wait_and_span(""), (None, None));
    }

    const APPROVALS: &str = include_str!("../../../fixtures/timeline/approvals.md");

    /// The log with every plugin record line removed.
    fn without_records(log: &str) -> String {
        log.lines()
            .filter(|l| !l.contains(" stop-check ") && !l.contains(" pr PR1 "))
            .map(|l| format!("{l}\n"))
            .collect()
    }

    #[test]
    fn plugin_records_inside_a_wait_change_neither_the_wait_nor_the_span_nor_the_bands() {
        let bare = without_records(APPROVALS);
        assert_ne!(bare.len(), APPROVALS.len(), "the fixture has record lines");
        assert_eq!(wait_and_span(APPROVALS), wait_and_span(&bare));
        let shape = |log: &str| {
            bands(log)
                .into_iter()
                .map(|b| (b.kind, b.name, b.start_ms, b.end_ms))
                .collect::<Vec<_>>()
        };
        assert_eq!(shape(APPROVALS), shape(&bare));
        // A stop-check line, and a pr line, right inside a wait: the wait is still 3 minutes.
        for record in [
            "2026-10-29T10:02:00Z planning stop-check PR2 fail",
            "2026-10-29T10:02:00Z planning stop-check - skip",
            "2026-10-29T10:02:00Z planning pr PR2 building -> reviewing",
            "2026-10-29T10:02:00Z planning pr PR3 - -> pending",
        ] {
            let log = format!(
                "2026-10-29T10:00:00Z planning start\n2026-10-29T10:01:00Z planning waiting for plan approval\n{record}\n2026-10-29T10:04:00Z building plan approved\n"
            );
            assert_eq!(wait_and_span(&log), (Some(3.0), Some(4.0)), "{record}");
        }
        // Look-alikes are progress: they do end the wait.
        for text in [
            "stop-check PR2 maybe",
            "stop-check pass",
            "pr PR2 a -> b c",
            "pr two a -> b",
            "PR2 stop-check pass",
        ] {
            let log = format!(
                "2026-10-29T10:01:00Z planning waiting for plan approval\n2026-10-29T10:02:00Z planning {text}\n2026-10-29T10:04:00Z building go\n"
            );
            assert_eq!(wait_and_span(&log).0, Some(1.0), "{text}");
        }
    }

    #[test]
    fn approval_bands_give_kind_outcome_and_the_offsets_of_the_opening_and_closing_lines() {
        // The byte offset where the line holding `needle` starts.
        let at = |needle: &str| {
            let hit = APPROVALS.find(needle).unwrap();
            APPROVALS[..hit].rfind('\n').map_or(0, |n| n + 1) as u64
        };
        let bands = approval_bands(APPROVALS);
        let got: Vec<(&str, &str, u64, Option<u64>)> = bands
            .iter()
            .map(|b| {
                (
                    b.kind.as_str(),
                    b.outcome.as_str(),
                    b.open_offset,
                    b.close_offset,
                )
            })
            .collect();
        let want = vec![
            ("plan", "approved", at("09:02:00Z"), Some(at("09:05:00Z"))),
            ("plan", "approved", at("09:06:00Z"), Some(at("09:07:00Z"))),
            ("plan", "declined", at("09:10:00Z"), Some(at("09:11:00Z"))),
            ("plan", "declined", at("09:12:00Z"), Some(at("09:14:00Z"))),
            ("ship", "approved", at("09:20:00Z"), Some(at("09:21:00Z"))),
            ("ship", "approved", at("09:22:00Z"), Some(at("09:23:00Z"))),
            ("ship", "declined", at("09:24:00Z"), Some(at("09:26:00Z"))),
            ("ship", "ended", at("09:30:00Z"), Some(at("09:31:00Z"))),
            ("ship", "open", at("09:40:00Z"), None),
        ];
        assert_eq!(got, want);
        // The first wait: opened 09:02, closed by the approval at 09:05, whatever sat between.
        assert_eq!(bands[0].end_ms - bands[0].start_ms, 3 * 60_000);
        assert_eq!(bands[0].name, "waiting for plan approval");
        let open = bands.last().unwrap();
        assert_eq!(open.end_ms, open.start_ms, "an open wait has no length yet");
        // The same total as the stored wait minutes.
        let total: i64 = bands.iter().map(|b| b.end_ms - b.start_ms).sum();
        assert_eq!(Some(minutes(total)), wait_and_span(APPROVALS).0);
        assert!(approval_bands("").is_empty());
        let other = "2026-11-02T09:00:00Z planning waiting for merge approval\n2026-11-02T09:01:00Z planning go\n";
        assert_eq!(approval_bands(other)[0].kind, "other");
    }

    #[test]
    fn approval_wording_is_matched_whole_and_case_insensitively() {
        for (text, want) in [
            ("Plan approved by the user", "approved"),
            ("plan approved for eval (eval mode)", "approved"),
            ("plan approved by the user and more", "ended"),
            ("ship declined; run paused", "declined"),
            ("plan changes requested", "declined"),
            ("approved", "ended"),
        ] {
            let log = format!(
                "2026-11-02T09:00:00Z plan-approval waiting for plan approval\n2026-11-02T09:01:00Z plan-approval {text}\n"
            );
            assert_eq!(approval_bands(&log)[0].outcome, want, "{text}");
        }
    }
}
