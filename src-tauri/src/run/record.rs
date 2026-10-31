//! The run's record, `<run>/app-usage.json` (docs/run-folder.md, "Files the
//! app adds"): written only here, by the app. It lists the Claude sessions
//! of the run (added when a run tab starts or a `/clear` moves the tab to a
//! new session) and, once the run is `done` or `failed`, its outcome, PRs,
//! tokens and an estimate summed over every session, the active minutes per
//! stage, and the first and last activity.
//!
//! `ensure_record` is level-triggered: the attention detector calls it for
//! every run it reads, so a run that finished while the app was off (or whose
//! tab was closed before a tick) still gets its record. An unchanged run is a
//! cheap no-op (two small JSON reads).
//!
//! Claude can write the run folder, so a planted symlink at `app-usage.json`,
//! its `.tmp` or its `.bak` is refused instead of followed.

use super::folder::read_json_value;
use super::status::parse_status;
use crate::observe::price::Tokens;
use crate::observe::transcript::{self, SessionTracker};
use crate::store::write_json_atomic;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const RECORD_FILE: &str = "app-usage.json";
/// A gap between two log lines longer than this counts as this long (idle).
pub const IDLE_CAP_MS: i64 = 30 * 60 * 1000;
/// log.md is read up to this size.
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

/// One read-modify-write at a time, in this process.
static WRITE: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionRef {
    pub session_id: String,
    pub config_dir: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecordTokens {
    pub input: u64,
    pub output: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
}

impl From<Tokens> for RecordTokens {
    fn from(t: Tokens) -> Self {
        Self {
            input: t.input,
            output: t.output,
            cache_write_5m: t.cache_write_5m,
            cache_write_1h: t.cache_write_1h,
            cache_read: t.cache_read,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub sessions: Vec<SessionRef>,
    /// `done` or `failed`; None until the run finished.
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub pr_count: u32,
    #[serde(default)]
    pub pr_urls: Vec<String>,
    #[serde(default)]
    pub tokens: RecordTokens,
    /// An estimate at list prices; None when a model has no price.
    #[serde(default)]
    pub estimate_usd: Option<f64>,
    #[serde(default)]
    pub active_minutes_by_stage: BTreeMap<String, f64>,
    /// Minutes spent in approval waits (`timeline::wait_and_span`); None in a
    /// record written before the field existed.
    #[serde(default)]
    pub wait_minutes: Option<f64>,
    /// Minutes from the first to the last log.md line.
    #[serde(default)]
    pub span_minutes: Option<f64>,
    #[serde(default)]
    pub first_activity: Option<String>,
    #[serde(default)]
    pub last_activity: Option<String>,
    /// The status.json `updatedAt` the totals were computed from.
    #[serde(default)]
    pub status_updated_at: Option<String>,
}

fn record_path(run_dir: &Path) -> PathBuf {
    run_dir.join(RECORD_FILE)
}

/// A symlink at the record, its temp file or its backup is never followed.
fn refuse_symlinks(run_dir: &Path) -> Result<(), String> {
    for name in ["app-usage.json", "app-usage.json.tmp", "app-usage.json.bak"] {
        if let Ok(meta) = std::fs::symlink_metadata(run_dir.join(name)) {
            if meta.file_type().is_symlink() {
                return Err(format!("{name} is a symlink; not touched"));
            }
        }
    }
    Ok(())
}

/// The record, or None for a run without one (an old run) or an unreadable
/// file.
pub fn read_record(run_dir: &Path) -> Option<RunRecord> {
    serde_json::from_value(read_json_value(&record_path(run_dir))?).ok()
}

fn write_record(run_dir: &Path, record: &RunRecord) -> Result<(), String> {
    refuse_symlinks(run_dir)?;
    write_json_atomic(&record_path(run_dir), record)
}

/// Adds a session to the run's record (idempotent). A new record starts
/// empty: its totals come with `ensure_record`.
pub fn append_session(run_dir: &Path, session_id: &str, config_dir: &str) -> Result<(), String> {
    let _guard = WRITE.lock().map_err(|e| e.to_string())?;
    refuse_symlinks(run_dir)?;
    let mut record = read_record(run_dir).unwrap_or(RunRecord {
        schema: 1,
        ..Default::default()
    });
    let entry = SessionRef {
        session_id: session_id.to_string(),
        config_dir: config_dir.to_string(),
    };
    if record.sessions.contains(&entry) {
        return Ok(());
    }
    record.schema = 1;
    record.sessions.push(entry);
    write_record(run_dir, &record)
}

/// Active minutes per stage from log.md (`<UTC time> <stage> <text>`): the
/// time between two consecutive lines is credited to the earlier line's
/// stage; a gap over 30 min counts as 30 (idle); the last line gets nothing.
pub fn minutes_by_stage(log: &str) -> BTreeMap<String, f64> {
    let lines: Vec<(i64, String)> = log
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, ' ');
            let at = transcript::parse_time_ms(parts.next()?)?;
            let stage = parts.next()?.to_string();
            Some((at, stage))
        })
        .collect();
    let mut out: BTreeMap<String, f64> = BTreeMap::new();
    for pair in lines.windows(2) {
        let gap = (pair[1].0 - pair[0].0).clamp(0, IDLE_CAP_MS);
        *out.entry(pair[0].1.clone()).or_default() += gap as f64 / 60_000.0;
    }
    for minutes in out.values_mut() {
        *minutes = (*minutes * 10.0).round() / 10.0;
    }
    out
}

fn rfc3339(ms: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ms).map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// Totals over every listed session's log (and its sub-agents'). Reads
/// files only; the caller holds no lock.
fn session_totals(sessions: &[SessionRef]) -> (Tokens, Option<f64>, Option<(i64, i64)>) {
    let mut tokens = Tokens::default();
    let mut cost = Some(0.0);
    let mut span: Option<(i64, i64)> = None;
    for session in sessions {
        let Some(file) = crate::claude_session_log::find_session_log(
            Path::new(&session.config_dir),
            &session.session_id,
        ) else {
            continue;
        };
        let mut tracker = SessionTracker::for_file(file);
        tracker.poll();
        let summary = tracker.summary(&[], "run");
        for stage in &summary.stages {
            tokens.add(&stage.tokens);
        }
        cost = match (cost, summary.total_cost_usd) {
            (Some(sum), Some(c)) => Some(sum + c),
            _ => None,
        };
        if let Some((first, last)) = tracker.activity_span() {
            span = Some(span.map_or((first, last), |(f, l)| (f.min(first), l.max(last))));
        }
    }
    (tokens, cost, span)
}

/// Writes the record's totals when the run is `done` or `failed` and the
/// record has none yet or was computed from an older status.json. Returns
/// whether it wrote.
pub fn ensure_record(run_dir: &Path) -> Result<bool, String> {
    let Some(status) = read_json_value(&run_dir.join("status.json")) else {
        return Ok(false);
    };
    let stage = status.get("stage").and_then(|v| v.as_str()).unwrap_or("");
    if stage != "done" && stage != "failed" {
        return Ok(false);
    }
    let updated = status
        .get("updatedAt")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let current = read_record(run_dir);
    if current
        .as_ref()
        .is_some_and(|r| r.outcome.is_some() && r.status_updated_at.as_deref() == Some(&updated))
    {
        return Ok(false);
    }
    refuse_symlinks(run_dir)?;

    // The slow part, outside the write lock.
    let sessions = current.map(|r| r.sessions).unwrap_or_default();
    let (tokens, cost, span) = session_totals(&sessions);
    let view = parse_status(&status).ok();
    let prs = view.as_ref().map(|v| v.prs.as_slice()).unwrap_or_default();
    let pr_urls: Vec<String> = prs.iter().filter_map(|pr| pr.pr_url.clone()).collect();
    let log = read_log(run_dir);

    let _guard = WRITE.lock().map_err(|e| e.to_string())?;
    // Sessions may have been added while the logs were read.
    let mut record = read_record(run_dir).unwrap_or_default();
    record.schema = 1;
    record.outcome = Some(stage.to_string());
    record.pr_count = prs.len() as u32;
    record.pr_urls = pr_urls;
    record.tokens = tokens.into();
    record.estimate_usd = cost;
    record.active_minutes_by_stage = minutes_by_stage(&log);
    // A log cut at the read cap would give a partial figure: none then.
    let (wait_min, span_min) = if log.len() as u64 >= MAX_LOG_BYTES {
        (None, None)
    } else {
        super::timeline::wait_and_span(&log)
    };
    record.wait_minutes = wait_min;
    record.span_minutes = span_min;
    record.first_activity = span.and_then(|(first, _)| rfc3339(first));
    record.last_activity = span.and_then(|(_, last)| rfc3339(last));
    record.status_updated_at = Some(updated);
    write_record(run_dir, &record)?;
    Ok(true)
}

fn read_log(run_dir: &Path) -> String {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(run_dir.join("log.md")) else {
        return String::new();
    };
    let mut text = String::new();
    let _ = file.take(MAX_LOG_BYTES).read_to_string(&mut text);
    text
}

/// A run folder this app made (not an adopted dev-app one), for the record.
pub fn own_run_dir(data: &Path, raw: &str) -> Option<PathBuf> {
    let dir = super::checked_run_dir(data, raw).ok()?;
    let runs = super::folder::runs_dir(data).canonicalize().ok()?;
    (dir.parent() == Some(runs.as_path())).then_some(dir)
}

/// A run tab's Claude session started or moved: remember it in the record.
pub fn note_session(
    data: &Path,
    tab: &crate::store::TabRecord,
    session_id: &str,
    config_dir: &str,
) {
    let Some(raw) = tab.launch.as_ref().and_then(|l| l.run_dir.as_deref()) else {
        return;
    };
    let Some(dir) = own_run_dir(data, raw) else {
        return;
    };
    if let Err(err) = append_session(&dir, session_id, config_dir) {
        eprintln!("DCTerminalv2: could not note a session in the run record: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use serde_json::json;

    const SID: &str = "de6a6329-de0f-49da-960b-f50fed9236e4";
    const SID2: &str = "3b5f0c1e-0000-4000-8000-000000000002";

    fn fixture_config(dir: &TempDir, name: &str) -> PathBuf {
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
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fixtures/claude-2.1.287");
        let config = dir.join(name);
        copy(&root.join("projects"), &config.join("projects"));
        config
    }

    /// A second, synthetic session of the same shape: two messages.
    fn second_session(dir: &TempDir) -> PathBuf {
        let config = dir.join("config2");
        let project = config.join("projects/-work-toy");
        std::fs::create_dir_all(&project).unwrap();
        let line = |id: &str, at: &str| {
            json!({"type": "assistant", "timestamp": at, "message": {"id": id, "model": "claude-opus-5-5",
                "content": [{"type": "text"}],
                "usage": {"input_tokens": 100, "output_tokens": 50, "cache_read_input_tokens": 0,
                          "cache_creation_input_tokens": 0}}})
            .to_string()
        };
        std::fs::write(
            project.join(format!("{SID2}.jsonl")),
            format!(
                "{}\n{}\n",
                line("m1", "2026-10-27T10:00:00Z"),
                line("m2", "2026-10-27T10:02:00Z")
            ),
        )
        .unwrap();
        config
    }

    fn run_dir(dir: &TempDir, stage: &str, updated: &str) -> PathBuf {
        let run = dir.join("data/runs/r-20261027-1000-abcd");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(
            run.join("status.json"),
            json!({"schema": 1, "stage": stage, "updatedAt": updated, "prs": [
                {"id": "PR1", "prUrl": "https://github.com/o/r/pull/1"},
                {"id": "PR2"}]})
            .to_string(),
        )
        .unwrap();
        run
    }

    #[test]
    fn minutes_per_stage_credit_the_earlier_line_and_cap_idle_gaps() {
        let log = "\
2026-10-27T10:00:00Z planning started
2026-10-27T10:10:00Z planning plan written
2026-10-27T10:12:00Z plan-approval waiting
2026-10-27T14:12:00Z building user said yes
2026-10-27T14:20:00Z building PR1 built
not a log line
2026-10-27T14:21:00Z done finished
";
        let minutes = minutes_by_stage(log);
        assert_eq!(minutes["planning"], 12.0);
        assert_eq!(
            minutes["plan-approval"], 30.0,
            "four idle hours count as 30 min"
        );
        assert_eq!(minutes["building"], 9.0);
        assert!(!minutes.contains_key("done"), "the last line gets nothing");
    }

    #[test]
    fn append_is_idempotent_and_a_symlinked_target_or_tmp_is_refused() {
        let dir = TempDir::new("record_append");
        let run = run_dir(&dir, "building", "t1");
        append_session(&run, SID, "/c1").unwrap();
        append_session(&run, SID, "/c1").unwrap();
        append_session(&run, SID2, "/c2").unwrap();
        let record = read_record(&run).unwrap();
        assert_eq!(record.sessions.len(), 2);
        assert_eq!(record.schema, 1);
        assert!(record.outcome.is_none(), "totals come at done");

        #[cfg(unix)]
        {
            let elsewhere = dir.join("elsewhere.json");
            std::fs::write(&elsewhere, "{}").unwrap();
            for name in ["app-usage.json", "app-usage.json.tmp", "app-usage.json.bak"] {
                let fresh = TempDir::new("record_symlink");
                let run = run_dir(&fresh, "done", "t1");
                std::os::unix::fs::symlink(&elsewhere, run.join(name)).unwrap();
                assert!(append_session(&run, SID, "/c").is_err(), "{name}");
                assert!(ensure_record(&run).is_err(), "{name}");
                assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "{}");
            }
        }
    }

    #[test]
    fn a_run_that_is_done_at_the_first_call_gets_its_record_summed_over_two_sessions() {
        let dir = TempDir::new("record_totals");
        let config1 = fixture_config(&dir, "config1");
        let config2 = second_session(&dir);
        let run = run_dir(&dir, "done", "2026-10-27T10:30:00Z");
        std::fs::write(
            run.join("log.md"),
            "2026-10-27T10:00:00Z planning start\n2026-10-27T10:10:00Z building go\n2026-10-27T10:25:00Z done end\n",
        )
        .unwrap();
        append_session(&run, SID, &config1.display().to_string()).unwrap();
        append_session(&run, SID2, &config2.display().to_string()).unwrap();

        // Each session alone, for the expected sums.
        let one = |sid: &str, config: &Path| {
            let file = crate::claude_session_log::find_session_log(config, sid).unwrap();
            let mut tracker = SessionTracker::for_file(file);
            tracker.poll();
            tracker.summary(&[], "run")
        };
        let a = one(SID, &config1);
        let b = one(SID2, &config2);

        assert!(ensure_record(&run).unwrap(), "no record yet: written");
        let record = read_record(&run).unwrap();
        assert_eq!(record.outcome.as_deref(), Some("done"));
        assert_eq!(record.pr_count, 2);
        assert_eq!(record.pr_urls, ["https://github.com/o/r/pull/1"]);
        let total_tokens = record.tokens.input
            + record.tokens.output
            + record.tokens.cache_write_5m
            + record.tokens.cache_write_1h
            + record.tokens.cache_read;
        assert_eq!(total_tokens, a.total_tokens + b.total_tokens);
        let want = a.total_cost_usd.unwrap() + b.total_cost_usd.unwrap();
        assert!((record.estimate_usd.unwrap() - want).abs() < 1e-9);
        assert_eq!(record.active_minutes_by_stage["planning"], 10.0);
        assert_eq!(record.active_minutes_by_stage["building"], 15.0);
        assert!(record.first_activity.is_some() && record.last_activity.is_some());
        assert_eq!(
            record.status_updated_at.as_deref(),
            Some("2026-10-27T10:30:00Z")
        );
        assert_eq!(record.sessions.len(), 2);
    }

    #[test]
    fn an_unchanged_run_is_not_rewritten_and_a_new_updated_at_rewrites() {
        let dir = TempDir::new("record_rewrite");
        let run = run_dir(&dir, "done", "t1");
        assert!(ensure_record(&run).unwrap());
        let path = run.join(RECORD_FILE);
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(!ensure_record(&run).unwrap(), "same updatedAt: a no-op");
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before
        );
        // A re-finish at a new updatedAt rewrites it.
        std::fs::write(
            run.join("status.json"),
            json!({"schema": 1, "stage": "failed", "updatedAt": "t2", "prs": []}).to_string(),
        )
        .unwrap();
        assert!(ensure_record(&run).unwrap());
        let record = read_record(&run).unwrap();
        assert_eq!(record.outcome.as_deref(), Some("failed"));
        assert_eq!(record.status_updated_at.as_deref(), Some("t2"));
        assert_eq!(record.pr_count, 0);
    }

    #[test]
    fn a_run_in_progress_or_without_status_gets_no_record_and_an_old_run_reads_as_none() {
        let dir = TempDir::new("record_none");
        let run = run_dir(&dir, "building", "t1");
        assert!(!ensure_record(&run).unwrap());
        assert!(read_record(&run).is_none());
        let empty = dir.join("data/runs/r-20261027-1001-beef");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(!ensure_record(&empty).unwrap());
        assert!(read_record(&empty).is_none());
    }

    #[test]
    fn only_the_apps_own_runs_get_a_record_not_adopted_ones() {
        let dir = TempDir::new("record_own");
        let data = dir.join("data");
        let own = run_dir(&dir, "done", "t1");
        assert!(own_run_dir(&data, &own.display().to_string()).is_some());
        let dev = dir.join("dev-data/runs/r-20261027-0900-dead");
        std::fs::create_dir_all(&dev).unwrap();
        crate::run::folder::adopt_runs(&data, std::slice::from_ref(&dev)).unwrap();
        assert!(super::super::checked_run_dir(&data, &dev.display().to_string()).is_ok());
        assert!(own_run_dir(&data, &dev.display().to_string()).is_none());
    }
}
