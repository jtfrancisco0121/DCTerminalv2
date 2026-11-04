//! The trace index: a local SQLite cache (`flight.db`) of the Events, Spans and source refs of runs and
//! their sessions, so past runs can be searched without parsing a transcript. It holds ids, names,
//! statuses, times, phases and refs, never content, and it is rebuildable: deleting it loses nothing.
//!
//! This is the only core file that uses rusqlite, and the host (not this module) decides what to read: it
//! hands `pass` each run's source stamps and a loader for its inputs, so the core does no file IO beyond the
//! database. One pass (the host runs it on one thread) re-derives only the runs whose watermarks changed.
//!
//! A watermark is `(file, offset of the last complete line, size, mtime, sha256 of the first 4 KiB)`. It
//! decides what changed and bounds the work; it is never a parse start. The transcript adapter needs whole
//! files (tool names by id, message dedupe, the first line, parent links), so a changed session is parsed
//! whole, as the session cache does, and a two-pass index equals a one-pass index on the same files.

use super::adapters::dcv2_run::RunEvents;
use super::correlate;
use super::schema::{attr, Event, Failure, PROVIDER_DCV2};
use super::session::{self, Parsed};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

/// `PRAGMA user_version` of the schema below. Another version is dropped and rebuilt (it is a cache).
pub const SCHEMA_VERSION: i64 = 1;

/// At most this many changed runs are re-derived in one pass.
pub const MAX_RUNS_PER_PASS: usize = 16;
/// No new run is started once a pass has parsed this many bytes (one run larger than that still finishes).
pub const MAX_BYTES_PER_PASS: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_runs: usize,
    pub max_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_runs: MAX_RUNS_PER_PASS,
            max_bytes: MAX_BYTES_PER_PASS,
        }
    }
}

const SCHEMA: &str = "
CREATE TABLE runs (
    run_id TEXT PRIMARY KEY,
    dir TEXT NOT NULL,
    title TEXT NOT NULL,
    repo TEXT NOT NULL,
    outcome TEXT NOT NULL,
    excluded_off INTEGER NOT NULL DEFAULT 0,
    indexed_at TEXT NOT NULL
);
CREATE TABLE sources (
    run_id TEXT NOT NULL,
    file TEXT NOT NULL,
    session TEXT NOT NULL,
    src_offset INTEGER NOT NULL,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    head_sha TEXT NOT NULL,
    PRIMARY KEY (run_id, file)
);
CREATE TABLE events (
    run_id TEXT NOT NULL,
    id TEXT NOT NULL,
    session TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    parent TEXT,
    role TEXT,
    pr TEXT,
    pr_exact INTEGER NOT NULL DEFAULT 0,
    kind TEXT NOT NULL,
    status TEXT,
    name TEXT,
    at TEXT NOT NULL,
    stage TEXT,
    pr_state TEXT,
    phase_label TEXT,
    failure TEXT,
    paths TEXT,
    src_file TEXT NOT NULL,
    src_offset INTEGER NOT NULL,
    src_len INTEGER NOT NULL,
    src_sha256 TEXT NOT NULL,
    event_json TEXT NOT NULL,
    PRIMARY KEY (run_id, id)
);
CREATE TABLE spans (
    span TEXT NOT NULL,
    run_id TEXT NOT NULL,
    agent TEXT NOT NULL,
    name TEXT,
    start_at TEXT NOT NULL,
    end_at TEXT,
    status TEXT,
    duration_ms INTEGER
);
CREATE TABLE progress (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    seen INTEGER NOT NULL,
    current INTEGER NOT NULL,
    at TEXT NOT NULL,
    db_bytes INTEGER NOT NULL
);
CREATE INDEX events_kind_status ON events (kind, status);
CREATE INDEX events_name ON events (name);
CREATE INDEX events_run_at ON events (run_id, at);
CREATE INDEX events_stage ON events (stage);
CREATE INDEX events_pr_state ON events (pr_state);
CREATE INDEX events_role ON events (role);
CREATE INDEX events_src_file ON events (run_id, src_file);
CREATE INDEX spans_run ON spans (run_id);
";

fn db_err(e: rusqlite::Error) -> String {
    format!("index: {e}")
}

fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(SCHEMA).map_err(db_err)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(db_err)
}

/// The writer connection: WAL, schema ensured. The host keeps it open for the app's lifetime.
pub fn open_writer(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(db_err)?;
    conn.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(db_err)?;
    conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))
        .map_err(db_err)?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(db_err)?;
    ensure_schema(&conn)?;
    Ok(conn)
}

/// An in-memory index (tests).
#[cfg(test)]
pub fn open_memory() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    ensure_schema(&conn).unwrap();
    conn
}

fn ensure_schema(conn: &Connection) -> Result<(), String> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(db_err)?;
    if version == SCHEMA_VERSION {
        return Ok(());
    }
    if version != 0 {
        // Another schema: it is a cache, so start over.
        conn.execute_batch(
            "DROP TABLE IF EXISTS runs; DROP TABLE IF EXISTS sources; DROP TABLE IF EXISTS events;
             DROP TABLE IF EXISTS spans; DROP TABLE IF EXISTS progress;",
        )
        .map_err(db_err)?;
    }
    create_schema(conn)
}

/// A read-only connection for a search: it can never write, whatever the SQL.
pub fn open_reader(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(db_err)?;
    conn.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(db_err)?;
    conn.pragma_update(None, "query_only", true)
        .map_err(db_err)?;
    Ok(conn)
}

/// What the host saw of one source file: its watermark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStamp {
    pub file: String,
    /// The end of the last complete line (the file length when it has no partial last line).
    pub offset: u64,
    pub size: u64,
    pub mtime_ms: i64,
    /// sha256 of the first 4 KiB: a rewritten file differs even at the same size.
    pub head_sha: String,
}

/// One recorded session of a run: its id and the stamps of its transcript files (empty when the
/// transcript is gone, and then its stored Events stay).
#[derive(Debug, Clone, Default)]
pub struct SessionInput {
    pub id: String,
    pub stamps: Vec<SourceStamp>,
}

/// Why a run has no rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exclusion {
    /// The account's capture policy is `off`: counted as excluded in a search.
    Off,
    /// The run's folders match no account (or none at all): no rows, not counted.
    Refused,
}

/// The host's side of a run: its inputs, loaded only when the run changed.
pub trait Loader {
    /// Session `index` (of `RunInput.sessions`), parsed whole.
    fn session(&self, index: usize) -> Arc<Parsed>;
    /// The run folder's own events.
    fn run(&self) -> RunEvents;
    /// Whether the run's agents carry the exact `PR<n> ` description rule.
    fn exact(&self) -> bool;
}

pub struct RunInput<'a> {
    pub run_id: String,
    pub dir: String,
    pub title: String,
    pub repo: String,
    pub outcome: String,
    pub exclusion: Option<Exclusion>,
    pub sessions: Vec<SessionInput>,
    /// The run files the events and the exactness read (log.md, status.json, meta.json, ...).
    pub run_stamps: Vec<SourceStamp>,
    pub loader: &'a dyn Loader,
}

/// What one pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PassStats {
    /// Runs re-derived.
    pub rebuilt: usize,
    /// Runs whose watermarks were all unchanged: no parse.
    pub skipped: usize,
    /// Changed runs left for a later pass by the limits.
    pub deferred: usize,
    /// Runs whose folder is gone: rows deleted.
    pub removed: usize,
    /// Runs excluded by an `off` account (their rows are deleted, the exclusion is counted).
    pub excluded: usize,
    /// Sessions parsed whole.
    pub sessions_parsed: usize,
    /// Of those, sessions with a source that shrank or was rewritten (read again from 0).
    pub rewritten: usize,
    /// Bytes of the sessions parsed.
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Stored {
    size: u64,
    mtime_ms: i64,
    head_sha: String,
    session: String,
}

fn stored_sources(conn: &Connection, run_id: &str) -> Result<HashMap<String, Stored>, String> {
    let mut stmt = conn
        .prepare("SELECT file, size, mtime, head_sha, session FROM sources WHERE run_id = ?1")
        .map_err(db_err)?;
    let rows = stmt
        .query_map([run_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Stored {
                    size: r.get::<_, i64>(1)? as u64,
                    mtime_ms: r.get(2)?,
                    head_sha: r.get(3)?,
                    session: r.get(4)?,
                },
            ))
        })
        .map_err(db_err)?;
    rows.collect::<Result<_, _>>().map_err(db_err)
}

fn same_stamp(stored: &Stored, now: &SourceStamp) -> bool {
    stored.size == now.size && stored.mtime_ms == now.mtime_ms && stored.head_sha == now.head_sha
}

fn delete_run_rows(conn: &Connection, run_id: &str, with_run: bool) -> Result<(), String> {
    for table in ["events", "spans", "sources"] {
        conn.execute(&format!("DELETE FROM {table} WHERE run_id = ?1"), [run_id])
            .map_err(db_err)?;
    }
    if with_run {
        conn.execute("DELETE FROM runs WHERE run_id = ?1", [run_id])
            .map_err(db_err)?;
    }
    Ok(())
}

/// A session's stored Events (their `event_json`), in the order they were stored.
fn stored_session_events(
    conn: &Connection,
    run_id: &str,
    session: &str,
) -> Result<Vec<Event>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT event_json FROM events WHERE run_id = ?1 AND src_file IN
               (SELECT file FROM sources WHERE run_id = ?1 AND session = ?2)
             ORDER BY rowid",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![run_id, session], |r| r.get::<_, String>(0))
        .map_err(db_err)?;
    let mut out = Vec::new();
    for row in rows {
        if let Ok(event) = serde_json::from_str::<Event>(&row.map_err(db_err)?) {
            out.push(event);
        }
    }
    Ok(out)
}

fn flatten_failures<'a>(failures: &'a [Failure], out: &mut HashMap<&'a str, &'a str>) {
    for f in failures {
        out.insert(f.id.as_str(), f.derived.label.as_str());
        flatten_failures(&f.repeats, out);
    }
}

fn text_attr<'a>(e: &'a Event, key: &str) -> Option<&'a str> {
    e.attrs.get(key).and_then(|v| v.as_str())
}

/// The PR an event belongs to and whether that tie is exact.
fn pr_of(e: &Event, exact: bool) -> (Option<String>, bool) {
    if e.provider.name == PROVIDER_DCV2 {
        let pr = text_attr(e, attr::PR).map(str::to_string);
        let exact = pr.is_some();
        (pr, exact)
    } else {
        let pr = e.agent.pr.clone();
        let exact = pr.is_some() && exact;
        (pr, exact)
    }
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn db_bytes(conn: &Connection) -> i64 {
    let count: i64 = conn
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap_or(0);
    let size: i64 = conn
        .query_row("PRAGMA page_size", [], |r| r.get(0))
        .unwrap_or(0);
    count * size
}

fn rebuild(
    conn: &mut Connection,
    run: &RunInput<'_>,
    stored: &HashMap<String, Stored>,
    stats: &mut PassStats,
) -> Result<(), String> {
    let mut parsed: Vec<Arc<Parsed>> = Vec::new();
    for (i, s) in run.sessions.iter().enumerate() {
        let known: Vec<&Stored> = stored.values().filter(|st| st.session == s.id).collect();
        let reusable = !s.stamps.is_empty()
            && known.len() == s.stamps.len()
            && s.stamps
                .iter()
                .all(|st| stored.get(&st.file).is_some_and(|o| same_stamp(o, st)));
        let gone = s.stamps.is_empty();
        if gone || reusable {
            let events = stored_session_events(conn, &run.run_id, &s.id)?;
            if !events.is_empty() || gone {
                if !events.is_empty() {
                    parsed.push(Arc::new(Parsed::from_events(events)));
                }
                continue;
            }
        }
        // Changed (grown, shrunk or rewritten), new, or nothing stored for it: parse it whole.
        let rewritten = s.stamps.iter().any(|st| {
            stored
                .get(&st.file)
                .is_some_and(|o| st.size < o.size || st.head_sha != o.head_sha)
        });
        stats.rewritten += usize::from(rewritten);
        stats.sessions_parsed += 1;
        stats.bytes += s.stamps.iter().map(|st| st.size).sum::<u64>();
        parsed.push(run.loader.session(i));
    }
    let run_events = run.loader.run();
    let exact = run.loader.exact();
    let merged = session::merge(&parsed, Some(&run_events));
    let events: Vec<Event> = merged
        .ordered
        .iter()
        .map(|(_, e)| e.as_ref().clone())
        .collect();
    let spans = correlate::spans(&events);
    let mut failure_of: HashMap<&str, &str> = HashMap::new();
    flatten_failures(&merged.failures, &mut failure_of);

    let tx = conn.transaction().map_err(db_err)?;
    delete_run_rows(&tx, &run.run_id, false)?;
    {
        let mut insert = tx
            .prepare(
                "INSERT OR REPLACE INTO events (run_id, id, session, agent_id, parent, role, pr, pr_exact, kind,
                   status, name, at, stage, pr_state, phase_label, failure, paths, src_file, src_offset, src_len,
                   src_sha256, event_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
            )
            .map_err(db_err)?;
        for event in &events {
            let phase = merged.phases.get(&event.id);
            let (pr, pr_exact) = pr_of(event, exact);
            let paths = event
                .attrs
                .get("files")
                .and_then(|f| f.as_array())
                .map(|files| {
                    files
                        .iter()
                        .filter_map(|p| p.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .filter(|p| !p.is_empty());
            let json = serde_json::to_string(event).map_err(|e| e.to_string())?;
            insert
                .execute(params![
                    run.run_id,
                    event.id,
                    event.session,
                    event.agent.id,
                    event.agent.parent,
                    event.agent.role,
                    pr,
                    pr_exact,
                    event.kind,
                    event.status,
                    event.name,
                    event.at,
                    phase.and_then(|p| p.stage.clone()),
                    phase.and_then(|p| p.pr_state.clone()),
                    phase.and_then(|p| p.label.clone()),
                    failure_of.get(event.id.as_str()),
                    paths,
                    event.content.source.file,
                    event.content.source.offset as i64,
                    event.content.source.len as i64,
                    event.content.source.sha256,
                    json,
                ])
                .map_err(db_err)?;
        }
        let mut span_insert = tx
            .prepare(
                "INSERT INTO spans (span, run_id, agent, name, start_at, end_at, status, duration_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .map_err(db_err)?;
        for s in &spans {
            span_insert
                .execute(params![
                    s.span,
                    run.run_id,
                    s.agent,
                    s.name,
                    s.start_at,
                    s.end_at,
                    s.status,
                    s.duration_ms,
                ])
                .map_err(db_err)?;
        }
        let mut source_insert = tx
            .prepare(
                "INSERT OR REPLACE INTO sources (run_id, file, session, src_offset, size, mtime, head_sha)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .map_err(db_err)?;
        let session_stamps = run
            .sessions
            .iter()
            .flat_map(|s| s.stamps.iter().map(move |st| (s.id.as_str(), st)));
        let run_stamps = run.run_stamps.iter().map(|st| ("", st));
        for (session, st) in session_stamps.chain(run_stamps) {
            source_insert
                .execute(params![
                    run.run_id,
                    st.file,
                    session,
                    st.offset as i64,
                    st.size as i64,
                    st.mtime_ms,
                    st.head_sha,
                ])
                .map_err(db_err)?;
        }
        // A session whose transcript is gone keeps its sources rows (so its stored Events stay findable).
        for (gone, st) in stored.iter().filter(|(_, o)| {
            run.sessions
                .iter()
                .any(|s| s.id == o.session && s.stamps.is_empty())
        }) {
            source_insert
                .execute(params![
                    run.run_id,
                    gone,
                    st.session,
                    0i64,
                    st.size as i64,
                    st.mtime_ms,
                    st.head_sha,
                ])
                .map_err(db_err)?;
        }
    }
    upsert_run(&tx, run, false)?;
    tx.commit().map_err(db_err)
}

fn upsert_run(conn: &Connection, run: &RunInput<'_>, excluded_off: bool) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO runs (run_id, dir, title, repo, outcome, excluded_off, indexed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            run.run_id,
            run.dir,
            run.title,
            run.repo,
            run.outcome,
            excluded_off,
            now_iso()
        ],
    )
    .map_err(db_err)?;
    Ok(())
}

/// One pass over the app's runs, in the order given. A run whose watermarks are all unchanged is skipped
/// with no parse; a changed run is re-derived (changed sessions parsed whole, unchanged ones from their stored
/// Events, the run files read again) and its rows replaced in one transaction. At most `limits.max_runs`
/// changed runs are re-derived and no new one starts once `limits.max_bytes` have been parsed: the rest stay
/// not current and finish on later passes. A run that is no longer given is deleted, and so is one whose
/// account is excluded; progress and the database size are written at the end.
pub fn pass(
    conn: &mut Connection,
    runs: &[RunInput<'_>],
    limits: &Limits,
) -> Result<PassStats, String> {
    let mut stats = PassStats::default();
    let given: HashSet<&str> = runs.iter().map(|r| r.run_id.as_str()).collect();
    let known: Vec<String> = {
        let mut stmt = conn.prepare("SELECT run_id FROM runs").map_err(db_err)?;
        let rows = stmt.query_map([], |r| r.get(0)).map_err(db_err)?;
        rows.collect::<Result<_, _>>().map_err(db_err)?
    };
    for gone in known.iter().filter(|id| !given.contains(id.as_str())) {
        delete_run_rows(conn, gone, true)?;
        stats.removed += 1;
    }
    let (mut seen, mut current) = (0i64, 0i64);
    for run in runs {
        match run.exclusion {
            Some(Exclusion::Off) => {
                delete_run_rows(conn, &run.run_id, false)?;
                upsert_run(conn, run, true)?;
                stats.excluded += 1;
                continue;
            }
            Some(Exclusion::Refused) => {
                delete_run_rows(conn, &run.run_id, true)?;
                continue;
            }
            None => {}
        }
        seen += 1;
        let stored = stored_sources(conn, &run.run_id)?;
        let indexed: Option<i64> = conn
            .query_row(
                "SELECT excluded_off FROM runs WHERE run_id = ?1",
                [&run.run_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_err)?;
        let now: Vec<&SourceStamp> = run
            .sessions
            .iter()
            .flat_map(|s| s.stamps.iter())
            .chain(run.run_stamps.iter())
            .collect();
        let gone_kept = stored
            .iter()
            .filter(|(_, o)| {
                run.sessions
                    .iter()
                    .any(|s| s.id == o.session && s.stamps.is_empty())
            })
            .count();
        let unchanged = indexed == Some(0)
            && stored.len() == now.len() + gone_kept
            && now
                .iter()
                .all(|st| stored.get(&st.file).is_some_and(|o| same_stamp(o, st)));
        if unchanged {
            stats.skipped += 1;
            current += 1;
            continue;
        }
        if stats.rebuilt >= limits.max_runs || stats.bytes >= limits.max_bytes {
            stats.deferred += 1;
            continue;
        }
        rebuild(conn, run, &stored, &mut stats)?;
        stats.rebuilt += 1;
        current += 1;
    }
    conn.execute(
        "INSERT OR REPLACE INTO progress (id, seen, current, at, db_bytes) VALUES (1, ?1, ?2, ?3, ?4)",
        params![seen, current, now_iso(), db_bytes(conn)],
    )
    .map_err(db_err)?;
    Ok(stats)
}

/// How far the index is: runs seen and runs current, and its size on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub seen: i64,
    pub current: i64,
    pub db_bytes: i64,
}

pub fn progress(conn: &Connection) -> Progress {
    conn.query_row(
        "SELECT seen, current, db_bytes FROM progress WHERE id = 1",
        [],
        |r| {
            Ok(Progress {
                seen: r.get(0)?,
                current: r.get(1)?,
                db_bytes: r.get(2)?,
            })
        },
    )
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flight::testkit::*;
    use crate::test_support::TempDir;
    use serde_json::json;
    use std::cell::Cell;

    /// A loader that hands back the same events and counts its parses.
    struct Fake {
        events: Vec<Event>,
        run: RunEvents,
        parsed: Cell<usize>,
        run_loads: Cell<usize>,
        exact: bool,
    }

    impl Fake {
        fn new(events: Vec<Event>) -> Self {
            Self {
                events,
                run: RunEvents::default(),
                parsed: Cell::new(0),
                run_loads: Cell::new(0),
                exact: false,
            }
        }
    }

    impl Loader for Fake {
        fn session(&self, _index: usize) -> Arc<Parsed> {
            self.parsed.set(self.parsed.get() + 1);
            Arc::new(Parsed::from_events(self.events.clone()))
        }
        fn run(&self) -> RunEvents {
            self.run_loads.set(self.run_loads.get() + 1);
            self.run.clone()
        }
        fn exact(&self) -> bool {
            self.exact
        }
    }

    fn stamp(file: &str, size: u64, head: &str) -> SourceStamp {
        SourceStamp {
            file: file.into(),
            offset: size,
            size,
            mtime_ms: 1_000 + size as i64,
            head_sha: head.into(),
        }
    }

    /// Two events of a synthetic session `name`, and its source file.
    fn session_events(dir: &TempDir, name: &str) -> (Vec<Event>, String) {
        let events = synthetic(
            dir,
            name,
            &[
                prompt("2026-11-05T10:00:00Z"),
                tool_use(
                    "2026-11-05T10:00:01Z",
                    "m1",
                    "t1",
                    "Bash",
                    json!({"command": "ls"}),
                ),
                tool_result("2026-11-05T10:00:02Z", "t1", "boom", true),
            ],
        );
        let file = events[0].content.source.file.clone();
        (events, file)
    }

    fn input<'a>(id: &str, fake: &'a Fake, file: &str, size: u64, head: &str) -> RunInput<'a> {
        RunInput {
            run_id: id.into(),
            dir: format!("/runs/{id}"),
            title: format!("title {id}"),
            repo: "toy".into(),
            outcome: "done".into(),
            exclusion: None,
            sessions: vec![SessionInput {
                id: "s1".into(),
                stamps: vec![stamp(file, size, head)],
            }],
            run_stamps: Vec::new(),
            loader: fake,
        }
    }

    fn inputs<'a>(fakes: &'a [Fake], file: &str) -> Vec<RunInput<'a>> {
        fakes
            .iter()
            .enumerate()
            .map(|(i, f)| input(&format!("r{i:02}"), f, file, 100, "h1"))
            .collect()
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn no_column_holds_content() {
        let conn = open_memory();
        let mut columns = Vec::new();
        for table in ["runs", "sources", "events", "spans", "progress"] {
            let mut stmt = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap();
            let names = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
            columns.extend(names.map(|n| n.unwrap()));
        }
        assert!(columns.len() > 40);
        for name in &columns {
            for bad in [
                "content", "text", "body", "message", "prompt", "input", "output", "stdout",
                "command",
            ] {
                assert!(!name.contains(bad), "column {name} looks like content");
            }
        }
        assert_eq!(count(&conn, "PRAGMA user_version"), SCHEMA_VERSION);
    }

    #[test]
    fn a_pass_stores_events_spans_and_sources_and_an_unchanged_run_is_not_parsed_again() {
        let dir = TempDir::new("index_unchanged");
        let (events, file) = session_events(&dir, "s1.jsonl");
        let fake = Fake::new(events.clone());
        let mut conn = open_memory();
        let runs = [input("r1", &fake, &file, 100, "h1")];
        let stats = pass(&mut conn, &runs, &Limits::default()).unwrap();
        assert_eq!(
            (stats.rebuilt, stats.skipped, stats.sessions_parsed),
            (1, 0, 1)
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM events") as usize,
            events.len()
        );
        assert!(count(&conn, "SELECT COUNT(*) FROM spans") >= 1);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM sources"), 1);
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM events WHERE failure IS NOT NULL"
            ),
            1,
            "the failed Bash call"
        );
        let p = progress(&conn);
        assert_eq!((p.seen, p.current), (1, 1));
        assert!(p.db_bytes > 0);
        let stats = pass(&mut conn, &runs, &Limits::default()).unwrap();
        assert_eq!(
            (stats.rebuilt, stats.skipped, stats.sessions_parsed),
            (0, 1, 0)
        );
        assert_eq!(fake.parsed.get(), 1, "no second parse");
        assert_eq!(fake.run_loads.get(), 1);
    }

    #[test]
    fn a_grown_source_is_parsed_whole_and_a_shrunk_or_rewritten_one_is_read_from_zero() {
        let dir = TempDir::new("index_changed");
        let (events, file) = session_events(&dir, "s1.jsonl");
        let fake = Fake::new(events[..2].to_vec());
        let mut conn = open_memory();
        pass(
            &mut conn,
            &[input("r1", &fake, &file, 100, "h1")],
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM events"), 2);
        // Grown: parsed whole again, not rewritten.
        let grown = Fake::new(events.clone());
        let stats = pass(
            &mut conn,
            &[input("r1", &grown, &file, 150, "h1")],
            &Limits::default(),
        )
        .unwrap();
        assert_eq!((stats.sessions_parsed, stats.rewritten), (1, 0));
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM events") as usize,
            events.len()
        );
        // Shrunk: read from 0, and the rows are exactly the new content.
        let shrunk = Fake::new(events[..1].to_vec());
        let stats = pass(
            &mut conn,
            &[input("r1", &shrunk, &file, 40, "h1")],
            &Limits::default(),
        )
        .unwrap();
        assert_eq!((stats.sessions_parsed, stats.rewritten), (1, 1));
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM events"), 1);
        // Rewritten at the same size: the head hash differs.
        let same_size = Fake::new(events.clone());
        let stats = pass(
            &mut conn,
            &[input("r1", &same_size, &file, 40, "h2")],
            &Limits::default(),
        )
        .unwrap();
        assert_eq!((stats.sessions_parsed, stats.rewritten), (1, 1));
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM events") as usize,
            events.len()
        );
    }

    #[test]
    fn a_gone_run_deletes_its_rows_and_a_gone_transcript_keeps_them() {
        let dir = TempDir::new("index_gone");
        let (events, file) = session_events(&dir, "s1.jsonl");
        let fake = Fake::new(events.clone());
        let other = Fake::new(events.clone());
        let mut conn = open_memory();
        pass(
            &mut conn,
            &[
                input("r1", &fake, &file, 100, "h1"),
                input("r2", &other, &file, 100, "h1"),
            ],
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM events") as usize,
            events.len() * 2
        );
        // r2's folder is gone.
        let stats = pass(
            &mut conn,
            &[input("r1", &fake, &file, 100, "h1")],
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(stats.removed, 1);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM events WHERE run_id = 'r2'"),
            0
        );
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM runs"), 1);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM spans WHERE run_id = 'r2'"),
            0
        );
        // r1's transcript is gone (no stamps): its stored Events stay, and a pass with run files still works.
        let mut gone = input("r1", &fake, &file, 100, "h1");
        gone.sessions[0].stamps.clear();
        gone.run_stamps = vec![stamp("/runs/r1/log.md", 10, "l1")];
        let parsed = fake.parsed.get();
        pass(&mut conn, &[gone], &Limits::default()).unwrap();
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM events") as usize,
            events.len(),
            "rows stay"
        );
        assert_eq!(fake.parsed.get(), parsed, "nothing to parse");
        // And again: the run is current, so it is skipped.
        let mut gone = input("r1", &fake, &file, 100, "h1");
        gone.sessions[0].stamps.clear();
        gone.run_stamps = vec![stamp("/runs/r1/log.md", 10, "l1")];
        let stats = pass(&mut conn, &[gone], &Limits::default()).unwrap();
        assert_eq!((stats.skipped, stats.rebuilt), (1, 0));
    }

    #[test]
    fn an_excluded_run_has_no_rows_and_is_counted() {
        let dir = TempDir::new("index_off");
        let (events, file) = session_events(&dir, "s1.jsonl");
        let fake = Fake::new(events);
        let mut conn = open_memory();
        pass(
            &mut conn,
            &[input("r1", &fake, &file, 100, "h1")],
            &Limits::default(),
        )
        .unwrap();
        assert!(count(&conn, "SELECT COUNT(*) FROM events") > 0);
        let mut off = input("r1", &fake, &file, 100, "h1");
        off.exclusion = Some(Exclusion::Off);
        let stats = pass(&mut conn, &[off], &Limits::default()).unwrap();
        assert_eq!(stats.excluded, 1);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM events"), 0);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM sources"), 0);
        assert_eq!(
            count(&conn, "SELECT excluded_off FROM runs WHERE run_id = 'r1'"),
            1
        );
        // Still off: still no rows, and nothing parsed.
        let parsed = fake.parsed.get();
        let mut off = input("r1", &fake, &file, 100, "h1");
        off.exclusion = Some(Exclusion::Off);
        pass(&mut conn, &[off], &Limits::default()).unwrap();
        assert_eq!(
            (
                count(&conn, "SELECT COUNT(*) FROM events"),
                fake.parsed.get()
            ),
            (0, parsed)
        );
        // A refused run has no row at all.
        let mut refused = input("r1", &fake, &file, 100, "h1");
        refused.exclusion = Some(Exclusion::Refused);
        pass(&mut conn, &[refused], &Limits::default()).unwrap();
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM runs"), 0);
        // On again: indexed.
        pass(
            &mut conn,
            &[input("r1", &fake, &file, 100, "h1")],
            &Limits::default(),
        )
        .unwrap();
        assert!(count(&conn, "SELECT COUNT(*) FROM events") > 0);
    }

    #[test]
    fn the_run_and_byte_limits_leave_runs_not_current_and_they_finish_on_later_passes() {
        let dir = TempDir::new("index_limits");
        let (events, file) = session_events(&dir, "s1.jsonl");
        let fakes: Vec<Fake> = (0..20).map(|_| Fake::new(events.clone())).collect();
        let mut conn = open_memory();
        let stats = pass(&mut conn, &inputs(&fakes, &file), &Limits::default()).unwrap();
        assert_eq!((stats.rebuilt, stats.deferred), (16, 4));
        let p = progress(&conn);
        assert_eq!((p.seen, p.current), (20, 16));
        assert_eq!(
            count(&conn, "SELECT COUNT(DISTINCT run_id) FROM events"),
            16
        );
        let stats = pass(&mut conn, &inputs(&fakes, &file), &Limits::default()).unwrap();
        assert_eq!((stats.rebuilt, stats.skipped, stats.deferred), (4, 16, 0));
        assert_eq!(progress(&conn).current, 20);
        // Bytes: a run that starts finishes alone, then no new one starts.
        let big = Limits {
            max_runs: 16,
            max_bytes: 150,
        };
        let mut conn = open_memory();
        let runs: Vec<RunInput<'_>> = fakes[..3]
            .iter()
            .enumerate()
            .map(|(i, f)| input(&format!("b{i}"), f, &file, 100, "h1"))
            .collect();
        let stats = pass(&mut conn, &runs, &big).unwrap();
        assert_eq!(
            (stats.rebuilt, stats.deferred),
            (2, 1),
            "100 + 100 >= 150 stops the third"
        );
        let one = Limits {
            max_runs: 16,
            max_bytes: 10,
        };
        let mut conn = open_memory();
        let stats = pass(&mut conn, &runs, &one).unwrap();
        assert_eq!(
            (stats.rebuilt, stats.deferred),
            (1, 2),
            "a run larger than the budget finishes alone"
        );
    }

    #[test]
    fn another_schema_version_is_dropped_and_rebuilt() {
        let dir = TempDir::new("index_version");
        let path = dir.join("flight.db");
        {
            let conn = open_writer(&path).unwrap();
            conn.execute(
                "INSERT INTO runs VALUES ('r', '/r', 't', 'x', 'done', 0, 'now')",
                [],
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 99i64).unwrap();
        }
        let conn = open_writer(&path).unwrap();
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM runs"), 0);
        assert_eq!(count(&conn, "PRAGMA user_version"), SCHEMA_VERSION);
    }
}
