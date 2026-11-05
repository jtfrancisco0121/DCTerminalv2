//! The seam between the flight core and the app: the only file in `flight/`
//! that may use `crate::`. It maps each session folder to its configured
//! account and capture policy before any file is opened, resolves the files of
//! a run or session, and reads a line's content (always redacted) on request.

use super::adapters::dcv2_run::{self, ApprovalIn, RunEvents, RunInputs};
use super::adapters::{AgentMeta, SubagentFile};
use super::compare;
use super::export;
use super::index::{
    self, Counts, Exclusion, Limits, PassStats, RunInput, SearchFilter, SearchOut, SessionInput,
    SourceStamp,
};
use super::redact::{first_hit, redact};
use super::replay;
use super::schema::{sha256_hex, CapturePolicy, SourceRef};
use super::session::{self, PageRequest, Parsed, SessionView};
use super::snapshot;
use crate::provider::claude::same_config_dir;
use crate::store::settings_store::{resolved_account_config, ClaudeAccount, ClaudeSettings};
use serde::Serialize;
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Shown when a folder belongs to no configured account.
pub const UNKNOWN_FOLDER: &str = "unknown account folder";
/// A line longer than this is not read back for display.
const MAX_LINE: u64 = 8 * 1024 * 1024;
/// Text returned to the UI is cut here.
const MAX_TEXT: usize = 256 * 1024;

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

/// How many account files the host handed out to be read. A test asserts 0
/// for an `off` account.
#[derive(Debug, Default)]
pub struct Opens(AtomicUsize);

impl Opens {
    #[cfg(test)]
    pub fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
    fn add(&self, n: usize) {
        self.0.fetch_add(n, Ordering::SeqCst);
    }
}

/// The configured account whose folder is `config_dir` (the first account's
/// folder honours `DCTV2_CLAUDE_CONFIG_DIR`).
pub fn account_for_dir<'a>(
    claude: &'a ClaudeSettings,
    config_dir: &str,
) -> Option<&'a ClaudeAccount> {
    claude.accounts.iter().find(|account| {
        let folder = resolved_account_config(claude, &account.id);
        same_config_dir(&folder.path, config_dir)
    })
}

/// The policy for reading the folders of one run or session: each folder is
/// mapped to its own account, the strictest wins, a folder that matches no
/// account refuses the whole set, and a run-meta `flight.capturePolicy` may
/// only make it stricter.
pub fn policy_for_files(
    claude: &ClaudeSettings,
    config_dirs: &[String],
    run_meta: Option<&Value>,
) -> Result<CapturePolicy, String> {
    let mut policy = CapturePolicy::Full;
    for dir in config_dirs {
        let account = account_for_dir(claude, dir).ok_or_else(|| UNKNOWN_FOLDER.to_string())?;
        policy = policy.strictest(account.capture_policy);
    }
    let over = run_meta
        .and_then(|m| m["flight"]["capturePolicy"].as_str())
        .and_then(|p| serde_json::from_value::<CapturePolicy>(Value::String(p.into())).ok());
    Ok(match over {
        Some(over) => policy.strictest(over),
        None => policy,
    })
}

/// The plugin version from which every implementer and PR reviewer is launched with a
/// `PR<n> ` description (SKILL.md Step 3), so a PR tied by that prefix is exact. A test ties
/// it to `plugin/VERSION` and SKILL.md.
pub const EXACT_PR_FROM: &str = "0.7.0";

/// `major.minor.patch` as numbers; anything else is `None`.
fn version_triple(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let mut next = || parts.next()?.parse::<u64>().ok();
    let triple = (next()?, next()?, next()?);
    parts.next().is_none().then_some(triple)
}

/// Whether the run's meta.json `pluginVersion` is at least [`EXACT_PR_FROM`]. A missing or
/// unparsable version is older.
pub fn exact_pr(run_dir: &Path) -> bool {
    let from = version_triple(EXACT_PR_FROM);
    crate::run::folder::read_json_value(&run_dir.join("meta.json"))
        .and_then(|m| m["pluginVersion"].as_str().map(str::to_string))
        .and_then(|v| version_triple(&v))
        .is_some_and(|v| Some(v) >= from)
}

/// What to open: a run's recorded sessions, or one session of one folder.
#[derive(Debug, Clone, Default)]
pub struct Query {
    /// A run folder already checked by `checked_run_dir`.
    pub run_dir: Option<PathBuf>,
    pub config_dir: Option<String>,
    pub session_id: Option<String>,
}

/// The files of one session.
#[derive(Debug, Clone)]
pub struct SessionFiles {
    pub main: PathBuf,
    pub subagents: Vec<SubagentFile>,
}

#[derive(Debug, Clone)]
pub struct SessionSet {
    pub policy: CapturePolicy,
    pub sessions: Vec<SessionFiles>,
    /// A recorded session whose transcript is gone, and the like.
    pub notices: Vec<String>,
    /// The run folder, for a run query: its own files join the Trace.
    pub run_dir: Option<PathBuf>,
    /// Recorded sessions whose transcript is gone (their ids, in record order).
    pub gone: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Access {
    /// A folder is no configured account's: nothing was opened.
    Refused(String),
    /// The strictest policy is `off`: nothing was opened.
    Off,
    Ready(SessionSet),
}

/// The files compared by identity: real path, and on Windows case-folded.
fn ident(path: &Path) -> String {
    let real = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let text = real.to_string_lossy().to_string();
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

fn load_meta(path: &Path) -> Option<AgentMeta> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(AgentMeta::from_json(
        &serde_json::from_str::<Value>(&text).ok()?,
    ))
}

/// The account folders that decide a run's capture policy: its recorded sessions' folders plus the
/// account the run was started with (meta.json `account.configDir`), so a run with no recorded
/// session yet still has its account's policy.
pub fn run_policy_dirs(run_dir: &Path) -> Vec<String> {
    let mut dirs: Vec<String> = crate::run::record::read_record(run_dir)
        .into_iter()
        .flat_map(|r| r.sessions)
        .map(|s| s.config_dir)
        .collect();
    let meta = crate::run::folder::read_json_value(&run_dir.join("meta.json"));
    if let Some(dir) = meta
        .as_ref()
        .and_then(|m| m["account"]["configDir"].as_str())
        .filter(|d| !d.trim().is_empty())
    {
        dirs.push(dir.to_string());
    }
    dirs
}

/// Shown when a run has neither a recorded session nor a started-with account.
pub const NO_RUN_ACCOUNT: &str = "no account for this run";

/// Checks the policy for every folder first, and only then looks for files.
pub fn resolve(claude: &ClaudeSettings, query: &Query, opens: &Opens) -> Access {
    let mut wanted: Vec<(String, String)> = Vec::new();
    let mut dirs: Vec<String> = Vec::new();
    let mut meta: Option<Value> = None;
    if let Some(run) = &query.run_dir {
        let record = crate::run::record::read_record(run);
        for s in record.into_iter().flat_map(|r| r.sessions) {
            wanted.push((s.config_dir, s.session_id));
        }
        dirs = run_policy_dirs(run);
        if dirs.is_empty() {
            return Access::Refused(NO_RUN_ACCOUNT.to_string());
        }
        meta = crate::run::folder::read_json_value(&run.join("meta.json"));
    } else if let (Some(dir), Some(id)) = (&query.config_dir, &query.session_id) {
        wanted.push((dir.clone(), id.clone()));
        dirs = vec![dir.clone()];
    }
    let policy = match policy_for_files(claude, &dirs, meta.as_ref()) {
        Ok(CapturePolicy::Off) => return Access::Off,
        Ok(policy) => policy,
        Err(refusal) => return Access::Refused(refusal),
    };
    let mut sessions = Vec::new();
    let mut notices = Vec::new();
    let mut gone = Vec::new();
    for (config_dir, session_id) in wanted {
        let Some(main) =
            crate::claude_session_log::find_session_log(Path::new(&config_dir), &session_id)
        else {
            notices.push(format!("transcript gone: {session_id}"));
            gone.push(session_id);
            continue;
        };
        let subagents: Vec<SubagentFile> = subagent_files(&main)
            .into_iter()
            .map(|(agent_id, path, meta)| SubagentFile {
                agent_id,
                path,
                meta: load_meta(&meta),
            })
            .collect();
        opens.add(1 + subagents.len());
        sessions.push(SessionFiles { main, subagents });
    }
    Access::Ready(SessionSet {
        policy,
        sessions,
        notices,
        run_dir: query.run_dir.clone(),
        gone,
    })
}

impl SessionSet {
    /// Whether `file` is one of the run's own files the adapter reads (`run_file_list`).
    pub fn contains_run_file(&self, file: &str) -> bool {
        let wanted = ident(Path::new(file));
        self.run_dir
            .as_deref()
            .is_some_and(|run| run_file_list(run).iter().any(|(_, p)| ident(p) == wanted))
    }

    /// Whether `file` is one of this set's transcripts.
    pub fn contains(&self, file: &str) -> bool {
        let wanted = ident(Path::new(file));
        self.sessions.iter().any(|s| {
            ident(&s.main) == wanted || s.subagents.iter().any(|a| ident(&a.path) == wanted)
        })
    }
}

/// Picks the names of one run subfolder the adapter reads.
type NameFilter = fn(&str) -> bool;

/// A run folder's own files the adapter may read: a fixed set of names, never a link.
const RUN_FILES: [&str; 5] = [
    "log.md",
    "status.json",
    "app-checks.json",
    "app-github.json",
    "logs/guard.log",
];
/// At most this many files of one run folder are listed.
const MAX_RUN_FILES: usize = 600;

fn regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

/// The names (relative to the run folder) and paths of the files the run adapter reads:
/// the record files above, `reviews/*.md`, `gates/*.yml`, `requests/*.json` and the check logs
/// `logs/*-check.txt`, `*-app-check.txt` and `*-stop-check.txt`. Links and folders are skipped.
pub fn run_file_list(run_dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = RUN_FILES
        .iter()
        .map(|n| (n.to_string(), run_dir.join(n)))
        .filter(|(_, p)| regular_file(p))
        .collect();
    let dirs: [(&str, NameFilter); 4] = [
        ("reviews", |n| n.ends_with(".md")),
        ("gates", |n| n.ends_with(".yml")),
        ("requests", |n| n.ends_with(".json")),
        ("logs", |n| n.ends_with("-check.txt")),
    ];
    for (dir, wanted) in dirs {
        let mut names: Vec<String> = std::fs::read_dir(run_dir.join(dir))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| wanted(n) && !n.starts_with('.'))
            .collect();
        names.sort();
        for name in names {
            let path = run_dir.join(dir).join(&name);
            if regular_file(&path) && out.len() < MAX_RUN_FILES {
                out.push((format!("{dir}/{name}"), path));
            }
        }
    }
    out
}

/// What the run adapter gets for `run_dir`: the file list, the approval stops from
/// `run::timeline` (the one place that parses approval text) and each repo's check from repos.json.
pub fn run_inputs(run_dir: &Path) -> RunInputs {
    let files = run_file_list(run_dir);
    let approvals = files
        .iter()
        .find(|(n, _)| n == "log.md")
        .and_then(|(_, p)| {
            let meta = std::fs::metadata(p).ok()?;
            if meta.len() > dcv2_run::MAX_FILE {
                return None;
            }
            std::fs::read_to_string(p).ok()
        })
        .map(|log| {
            crate::run::timeline::approval_bands(&log)
                .into_iter()
                .map(|b| ApprovalIn {
                    kind: b.kind,
                    start_ms: b.start_ms,
                    end_ms: b.end_ms,
                    open_offset: b.open_offset,
                    close_offset: b.close_offset,
                    outcome: b.outcome,
                })
                .collect()
        })
        .unwrap_or_default();
    let repo_checks = crate::run::folder::read_json_value(&run_dir.join("repos.json"))
        .and_then(|v| {
            Some(
                v["repos"]
                    .as_array()?
                    .iter()
                    .filter_map(|r| {
                        Some((
                            r["path"].as_str()?.to_string(),
                            r["check"].as_str()?.to_string(),
                        ))
                    })
                    .collect(),
            )
        })
        .unwrap_or_default();
    RunInputs {
        run_dir: run_dir.to_path_buf(),
        files,
        approvals,
        repo_checks,
    }
}

/// How many parsed sessions the cache keeps.
const CACHE_SESSIONS: usize = 8;

/// A file's identity for the cache: its real path, size and mtime.
type Stamp = (String, u64, Option<SystemTime>);

/// A parsed session is keyed by the stamps of all its files and by the exact-PR flag
/// (the same files give a different PR label under the other flag).
type CacheKey = (Vec<Stamp>, bool);

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok();
    (
        ident(path),
        meta.as_ref().map(|m| m.len()).unwrap_or(0),
        meta.and_then(|m| m.modified().ok()),
    )
}

/// A run's identity, the stamps of its listed files and its events.
type RunCacheEntry = (String, Vec<Stamp>, Arc<RunEvents>);

/// How many runs' own events the cache keeps.
const CACHE_RUNS: usize = 8;

#[derive(Debug, Default)]
struct CacheInner {
    /// Oldest first; a session is keyed by the stamps of all its files.
    entries: Mutex<Vec<(CacheKey, Arc<Parsed>)>>,
    /// Oldest first; a run's events are keyed by the run folder and the stamps of its listed files.
    runs: Mutex<Vec<RunCacheEntry>>,
    hits: AtomicUsize,
    misses: AtomicUsize,
    /// How many times a run's own events were built (not served from the cache).
    run_misses: AtomicUsize,
}

/// Parsed sessions, so paging does not parse again. Separate from the stores:
/// the app keeps it as its own managed state, and no lock is held while a
/// file is parsed.
#[derive(Debug, Clone, Default)]
pub struct FlightCache(Arc<CacheInner>);

impl FlightCache {
    #[cfg(test)]
    pub fn hits(&self) -> usize {
        self.0.hits.load(Ordering::SeqCst)
    }
    #[cfg(test)]
    pub fn misses(&self) -> usize {
        self.0.misses.load(Ordering::SeqCst)
    }
    #[cfg(test)]
    pub fn run_misses(&self) -> usize {
        self.0.run_misses.load(Ordering::SeqCst)
    }

    /// The run's own events, from the cache while none of its listed files changed.
    fn run_events(&self, run_dir: &Path, opens: &Opens) -> Arc<RunEvents> {
        let files = run_file_list(run_dir);
        let stamps: Vec<Stamp> = files.iter().map(|(_, p)| stamp(p)).collect();
        let run = ident(run_dir);
        if let Ok(runs) = self.0.runs.lock() {
            if let Some((_, _, hit)) = runs.iter().find(|(r, k, _)| *r == run && *k == stamps) {
                return hit.clone();
            }
        }
        self.0.run_misses.fetch_add(1, Ordering::SeqCst);
        opens.add(files.len());
        let events = Arc::new(dcv2_run::events_for(&run_inputs(run_dir)));
        if let Ok(mut runs) = self.0.runs.lock() {
            runs.retain(|(r, _, _)| *r != run);
            runs.push((run, stamps, events.clone()));
            while runs.len() > CACHE_RUNS {
                runs.remove(0);
            }
        }
        events
    }

    fn parsed(&self, files: &SessionFiles, exact: bool) -> Arc<Parsed> {
        let mut stamps = vec![stamp(&files.main)];
        stamps.extend(files.subagents.iter().map(|s| stamp(&s.path)));
        let key: CacheKey = (stamps, exact);
        if let Ok(entries) = self.0.entries.lock() {
            if let Some((_, hit)) = entries.iter().find(|(k, _)| *k == key) {
                self.0.hits.fetch_add(1, Ordering::SeqCst);
                return hit.clone();
            }
        }
        self.0.misses.fetch_add(1, Ordering::SeqCst);
        let parsed = Arc::new(session::parse(&files.main, &files.subagents, exact));
        if let Ok(mut entries) = self.0.entries.lock() {
            // The file grew (or the flag changed): drop the older parse of the same main file.
            entries.retain(|(k, _)| k.0.first().map(|f| &f.0) != key.0.first().map(|f| &f.0));
            entries.push((key, parsed.clone()));
            while entries.len() > CACHE_SESSIONS {
                entries.remove(0);
            }
        }
        parsed
    }
}

/// A query resolved and parsed: the sessions (and the snapshot's, for a gone transcript) and the run's own events.
struct Loaded {
    set: SessionSet,
    parsed: Vec<Arc<Parsed>>,
    run: Option<Arc<RunEvents>>,
    snapshot: Option<session::SnapshotInfo>,
}

/// The policy first, then the files, then the parse; `None` when the policy is `off`.
fn load(
    claude: &ClaudeSettings,
    query: &Query,
    cache: &FlightCache,
    opens: &Opens,
) -> Result<Option<Loaded>, String> {
    let set = match resolve(claude, query, opens) {
        Access::Refused(why) => return Err(why),
        Access::Off => return Ok(None),
        Access::Ready(set) => set,
    };
    let exact = query.run_dir.as_deref().is_some_and(exact_pr);
    let mut parsed: Vec<Arc<Parsed>> = set
        .sessions
        .iter()
        .map(|s| cache.parsed(s, exact))
        .collect();
    // A recorded session whose transcript is gone is read from the run's snapshot, that session only.
    let mut snapshot_info = None;
    if let Some((manifest, sessions)) = set
        .run_dir
        .as_deref()
        .and_then(|dir| snapshot_sessions(dir, &set.gone))
    {
        for (_, events) in sessions {
            parsed.push(Arc::new(Parsed::from_events(events)));
        }
        snapshot_info = Some(session::SnapshotInfo {
            created_at: manifest.created_at.clone(),
            policy: manifest.policy,
            content_kept: manifest.content_lines > 0,
        });
    }
    let run = set
        .run_dir
        .as_deref()
        .map(|dir| cache.run_events(dir, opens));
    Ok(Some(Loaded {
        set,
        parsed,
        run,
        snapshot: snapshot_info,
    }))
}

/// The run id of a run query.
fn trace_of(query: &Query) -> Option<String> {
    query
        .run_dir
        .as_ref()
        .and_then(|d| d.file_name())
        .map(|n| n.to_string_lossy().to_string())
}

/// `flight_session`: the policy first, then the files, then one page.
pub fn session_view(
    claude: &ClaudeSettings,
    query: &Query,
    request: &PageRequest,
    cache: &FlightCache,
    opens: &Opens,
) -> Result<SessionView, String> {
    let trace = trace_of(query);
    let Some(loaded) = load(claude, query, cache, opens)? else {
        return Ok(SessionView::off(trace.as_deref().unwrap_or("")));
    };
    let mut view = session::view(
        &loaded.parsed,
        loaded.run.as_deref(),
        trace.as_deref(),
        loaded.set.policy,
        request,
        loaded.set.notices,
    )?;
    view.snapshot = loaded.snapshot;
    Ok(view)
}

/// The file of the trace index, in the app's data folder.
pub const INDEX_FILE: &str = "flight.db";
/// The index thread's first pass waits this long, and every pass after it.
const INDEX_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

fn read_prefix(path: &Path, max: usize) -> Option<Vec<u8>> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; max];
    let mut got = 0;
    while got < max {
        match file.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(_) => return None,
        }
    }
    buf.truncate(got);
    Some(buf)
}

/// The end of the last complete line: the file length unless it ends in a partial line (read from the
/// last 64 KiB).
fn complete_len(path: &Path, size: u64) -> u64 {
    const TAIL: u64 = 64 * 1024;
    let Ok(mut file) = std::fs::File::open(path) else {
        return size;
    };
    let from = size.saturating_sub(TAIL);
    if file.seek(SeekFrom::Start(from)).is_err() {
        return size;
    }
    let mut buf = Vec::new();
    if file.take(TAIL).read_to_end(&mut buf).is_err() {
        return size;
    }
    match buf.iter().rposition(|b| *b == b'\n') {
        Some(i) if i + 1 < buf.len() => from + i as u64 + 1,
        Some(_) => size,
        None if from == 0 => 0,
        None => size,
    }
}

/// A file's watermark: size, mtime, the hash of its first 4 KiB and the end of its last complete line.
fn source_stamp(path: &Path) -> Option<SourceStamp> {
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64);
    Some(SourceStamp {
        file: path.display().to_string(),
        offset: complete_len(path, meta.len()),
        size: meta.len(),
        mtime_ms,
        head_sha: sha256_hex(&read_prefix(path, 4096)?),
    })
}

/// One run's inputs for the index, loaded only when the run changed.
struct HostLoader {
    run_dir: PathBuf,
    exact: bool,
    sessions: Vec<LoaderSession>,
}

/// Where one session of a run comes from when the index has to parse it.
enum LoaderSession {
    /// Its transcript files.
    Live(SessionFiles),
    /// Its transcript is gone, but the run's snapshot holds its Events.
    Snapshot(Vec<super::schema::Event>),
    /// Its transcript is gone and nothing else is known.
    Gone,
}

impl index::Loader for HostLoader {
    fn session(&self, index: usize) -> Arc<Parsed> {
        match self.sessions.get(index) {
            Some(LoaderSession::Live(files)) => {
                Arc::new(session::parse(&files.main, &files.subagents, self.exact))
            }
            Some(LoaderSession::Snapshot(events)) => Arc::new(Parsed::from_events(events.clone())),
            _ => Arc::new(Parsed::from_events(Vec::new())),
        }
    }
    fn run(&self) -> RunEvents {
        dcv2_run::events_for(&run_inputs(&self.run_dir))
    }
    fn exact(&self) -> bool {
        self.exact
    }
}

struct Prepared {
    run_id: String,
    dir: String,
    title: String,
    repo: String,
    outcome: String,
    exclusion: Option<Exclusion>,
    sessions: Vec<SessionInput>,
    run_stamps: Vec<SourceStamp>,
    loader: HostLoader,
}

fn session_id_of(main: &Path) -> String {
    main.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn prepare_run(claude: &ClaudeSettings, dir: &Path) -> Option<Prepared> {
    let run_id = dir.file_name()?.to_str()?.to_string();
    let status = crate::run::folder::read_json_value(&dir.join("status.json"));
    let usage = crate::run::folder::read_json_value(&dir.join("app-usage.json"));
    let repos: Vec<String> = status
        .as_ref()
        .and_then(|s| s["prs"].as_array())
        .map(|prs| {
            let mut names: Vec<String> = prs
                .iter()
                .filter_map(|p| p["repo"].as_str().map(str::to_string))
                .collect();
            names.sort();
            names.dedup();
            names
        })
        .filter(|n| !n.is_empty())
        .or_else(|| crate::run::folder::repo_names_of(dir))
        .unwrap_or_default();
    let mut prepared = Prepared {
        run_id,
        dir: dir.display().to_string(),
        title: crate::run::folder::request_title(dir).unwrap_or_default(),
        repo: repos.join(", "),
        outcome: usage
            .as_ref()
            .and_then(|u| u["outcome"].as_str())
            .or_else(|| status.as_ref().and_then(|s| s["stage"].as_str()))
            .unwrap_or("")
            .to_string(),
        exclusion: None,
        sessions: Vec::new(),
        run_stamps: Vec::new(),
        loader: HostLoader {
            run_dir: dir.to_path_buf(),
            exact: exact_pr(dir),
            sessions: Vec::new(),
        },
    };
    let query = Query {
        run_dir: Some(dir.to_path_buf()),
        ..Default::default()
    };
    // The same resolver as the Trace: an `off` run opens no file here either.
    let set = match resolve(claude, &query, &Opens::default()) {
        Access::Off => {
            prepared.exclusion = Some(Exclusion::Off);
            return Some(prepared);
        }
        Access::Refused(_) => {
            prepared.exclusion = Some(Exclusion::Refused);
            return Some(prepared);
        }
        Access::Ready(set) => set,
    };
    for files in &set.sessions {
        let stamps: Vec<SourceStamp> = std::iter::once(files.main.as_path())
            .chain(files.subagents.iter().map(|s| s.path.as_path()))
            .filter_map(source_stamp)
            .collect();
        prepared.sessions.push(SessionInput {
            id: session_id_of(&files.main),
            stamps,
        });
        prepared
            .loader
            .sessions
            .push(LoaderSession::Live(files.clone()));
    }
    // A gone session reads its Events from the run's snapshot (so a rebuilt flight.db matches the Trace).
    let mut from_snapshot = snapshot_sessions(dir, &set.gone)
        .map(|(_, sessions)| sessions)
        .unwrap_or_default();
    for id in &set.gone {
        prepared.sessions.push(SessionInput {
            id: id.clone(),
            stamps: Vec::new(),
        });
        let at = from_snapshot.iter().position(|(sid, _)| sid == id);
        prepared.loader.sessions.push(match at {
            Some(at) => LoaderSession::Snapshot(from_snapshot.swap_remove(at).1),
            None => LoaderSession::Gone,
        });
    }
    let mut names: Vec<(String, PathBuf)> = run_file_list(dir);
    for extra in [
        "meta.json",
        "repos.json",
        "app-usage.json",
        "flight/manifest.json",
    ] {
        let path = dir.join(extra);
        if regular_file(&path) && (!extra.starts_with("flight/") || snapshot_dir(dir).is_some()) {
            names.push((extra.to_string(), path));
        }
    }
    prepared.run_stamps = names.iter().filter_map(|(_, p)| source_stamp(p)).collect();
    Some(prepared)
}

/// The app's run folders: those under `<data>/runs` and the adopted ones, each checked.
pub fn app_run_dirs(data: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(crate::run::folder::runs_dir(data))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    dirs.extend(crate::run::folder::adopted_runs(data));
    dirs.sort();
    dirs.dedup();
    dirs.into_iter()
        .filter(|d| {
            d.is_dir()
                && !d
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        })
        .filter_map(|d| crate::run::checked_run_dir(data, &d.display().to_string()).ok())
        .collect()
}

/// One index pass over `run_dirs`: every run is resolved through the same policy as the Trace, its sources
/// stamped, and `index::pass` re-derives what changed. Nothing here is called from the run list or the
/// sweeps: only [`spawn_index`] reaches it.
pub fn index_pass(
    conn: &mut rusqlite::Connection,
    claude: &ClaudeSettings,
    run_dirs: &[PathBuf],
    limits: &Limits,
) -> Result<PassStats, String> {
    let prepared: Vec<Prepared> = run_dirs
        .iter()
        .filter_map(|dir| prepare_run(claude, dir))
        .collect();
    let inputs: Vec<RunInput<'_>> = prepared
        .iter()
        .map(|p| RunInput {
            run_id: p.run_id.clone(),
            dir: p.dir.clone(),
            title: p.title.clone(),
            repo: p.repo.clone(),
            outcome: p.outcome.clone(),
            exclusion: p.exclusion,
            sessions: p.sessions.clone(),
            run_stamps: p.run_stamps.clone(),
            loader: &p.loader,
        })
        .collect();
    index::pass(conn, &inputs, limits)
}

/// Starts the one thread that keeps `<data>/flight.db` current: it waits 30 s, then runs a pass every 30 s.
/// It is spawned from the app's setup, never from the run list, `record_totals` or the 2 s sweeps, and it
/// is the only opener of the database for writing (it keeps its connection for the app's lifetime). The
/// settings are copied under a short lock that is released before any file is read.
pub fn spawn_index(app: &tauri::AppHandle) {
    use tauri::Manager;
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("flight-index".into())
        .spawn(move || {
            let mut conn: Option<rusqlite::Connection> = None;
            let mut snapshots = SnapshotState::default();
            loop {
                std::thread::sleep(INDEX_EVERY);
                let data = crate::data_dir::app_data_dir(&app).path;
                if conn.is_none() {
                    conn = index::open_writer(&data.join(INDEX_FILE)).ok();
                }
                let state = app.state::<Mutex<crate::store::SettingsStore>>();
                let claude = state.lock().map(|s| s.claude().clone()).ok();
                if let (Some(conn), Some(claude)) = (conn.as_mut(), claude) {
                    let dirs = app_run_dirs(&data);
                    // A bad run must not end the thread: the next pass tries again.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        index_pass(conn, &claude, &dirs, &Limits::default())
                    }));
                }
                // The snapshot pass: the same thread, after the index pass, over the same run folders.
                let settings = app.state::<Mutex<crate::store::SettingsStore>>();
                let claude = settings.lock().map(|s| s.claude().clone()).ok();
                if let Some(claude) = claude {
                    let dirs = app_run_dirs(&data);
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        snapshot_pass(&claude, &dirs, &mut snapshots)
                    }));
                }
            }
        });
}

// ---- the run snapshot (CAP-11, AD-45) ----------------------------------------------------

/// The snapshot folder inside a run folder.
pub const SNAPSHOT_DIR: &str = "flight";
/// A snapshot being built, or the old one being replaced (`<this><hex>` and `<this><hex>-old`).
pub const SNAPSHOT_TMP: &str = ".flight.tmp-";
/// At most this many runs get a snapshot in one pass.
pub const SNAPSHOTS_PER_PASS: usize = 2;
/// A run is tried at most this many times per app session.
pub const SNAPSHOT_ATTEMPTS: u32 = 3;
const SNAPSHOT_MANIFEST_MAX: u64 = 4 * 1024 * 1024;
const SNAPSHOT_FILE_MAX: u64 = 512 * 1024 * 1024;

/// Tries and the last reason logged, per run folder, for this app session only.
#[derive(Debug, Default)]
pub struct SnapshotState {
    tried: std::collections::HashMap<PathBuf, (u32, Option<String>)>,
}

impl SnapshotState {
    #[cfg(test)]
    pub fn attempts(&self, run: &Path) -> u32 {
        self.tried.get(run).map_or(0, |t| t.0)
    }
}

/// A transcript file's identity for the manifest: path, size, mtime (ms) and the sha256 of its bytes.
fn snap_source(path: &Path) -> Option<snapshot::SnapSource> {
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let bytes = std::fs::read(path).ok()?;
    Some(snapshot::SnapSource {
        file: path.display().to_string(),
        size: meta.len(),
        mtime: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as i64),
        sha256: sha256_hex(&bytes),
    })
}

/// `<run>/flight` when it is a real folder (not a link): the three snapshot files are read only through it.
fn snapshot_dir(run_dir: &Path) -> Option<PathBuf> {
    let dir = run_dir.join(SNAPSHOT_DIR);
    std::fs::symlink_metadata(&dir)
        .ok()
        .filter(|m| m.file_type().is_dir())
        .map(|_| dir)
}

fn read_snapshot_text(path: &Path, cap: u64) -> Result<String, String> {
    use crate::store::{read_capped, CappedError};
    let bytes = read_capped(path, cap).map_err(|e| match e {
        CappedError::Missing => "missing".to_string(),
        CappedError::NotRegular => "not a regular file".to_string(),
        CappedError::TooLarge => "too large".to_string(),
        CappedError::Io(e) => e,
    })?;
    String::from_utf8(bytes).map_err(|_| "not UTF-8".to_string())
}

/// Only the manifest of a run's snapshot (a small file); `None` when it is absent, damaged or of another schema.
fn read_snapshot_manifest(run_dir: &Path) -> Option<snapshot::Manifest> {
    let dir = snapshot_dir(run_dir)?;
    let manifest: snapshot::Manifest = serde_json::from_str(
        &read_snapshot_text(&dir.join("manifest.json"), SNAPSHOT_MANIFEST_MAX).ok()?,
    )
    .ok()?;
    (manifest.schema == snapshot::SCHEMA).then_some(manifest)
}

/// The three files of a run's snapshot, read without following links; `None` when it is absent or damaged.
fn read_snapshot(run_dir: &Path) -> Option<snapshot::Old> {
    let dir = snapshot_dir(run_dir)?;
    let manifest = read_snapshot_manifest(run_dir)?;
    Some(snapshot::Old {
        events_jsonl: read_snapshot_text(&dir.join("events.jsonl"), SNAPSHOT_FILE_MAX).ok()?,
        content_jsonl: read_snapshot_text(&dir.join("content.jsonl"), SNAPSHOT_FILE_MAX).ok(),
        manifest,
    })
}

/// One session's id and its Events.
type SessionEvents = (String, Vec<super::schema::Event>);

/// The Events of the recorded sessions in `gone` that the run's snapshot holds, one list per session (that
/// session only, unchanged), with the manifest. Reads `manifest.json` and `events.jsonl` as regular files, never
/// through a link. A live transcript always wins: the caller passes only the sessions whose transcripts are gone.
fn snapshot_sessions(
    run_dir: &Path,
    gone: &[String],
) -> Option<(snapshot::Manifest, Vec<SessionEvents>)> {
    if gone.is_empty() {
        return None;
    }
    let dir = snapshot_dir(run_dir)?;
    let manifest: snapshot::Manifest = serde_json::from_str(
        &read_snapshot_text(&dir.join("manifest.json"), SNAPSHOT_MANIFEST_MAX).ok()?,
    )
    .ok()?;
    if manifest.schema != snapshot::SCHEMA {
        return None;
    }
    let wanted: Vec<String> = gone
        .iter()
        .filter(|id| manifest.sessions.iter().any(|s| &s.id == *id))
        .cloned()
        .collect();
    if wanted.is_empty() {
        return None;
    }
    let text = read_snapshot_text(&dir.join("events.jsonl"), SNAPSHOT_FILE_MAX).ok()?;
    let mut by_session: Vec<SessionEvents> =
        wanted.into_iter().map(|id| (id, Vec::new())).collect();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(event) = serde_json::from_str::<super::schema::Event>(line) {
            if let Some((_, events)) = by_session.iter_mut().find(|(id, _)| *id == event.session) {
                events.push(event);
            }
        }
    }
    by_session.retain(|(_, events)| !events.is_empty());
    (!by_session.is_empty()).then_some((manifest, by_session))
}

/// A gone session's content from the run's snapshot: only for a source file the snapshot lists, only while the
/// current policy is `redacted` or `full`, and only the line whose sha256 is the ref's; else "content not kept".
/// `None` when the snapshot does not know the file at all (the caller's usual refusal applies).
fn snapshot_content_for(
    run_dir: &Path,
    policy: CapturePolicy,
    source: &SourceRef,
) -> Option<Result<ContentOut, String>> {
    let dir = snapshot_dir(run_dir)?;
    let manifest: snapshot::Manifest = serde_json::from_str(
        &read_snapshot_text(&dir.join("manifest.json"), SNAPSHOT_MANIFEST_MAX).ok()?,
    )
    .ok()?;
    if manifest.schema != snapshot::SCHEMA
        || !manifest.sources.iter().any(|s| s.file == source.file)
    {
        return None;
    }
    const NOT_KEPT: &str = "content not kept";
    if !matches!(policy, CapturePolicy::Redacted | CapturePolicy::Full) {
        return Some(Err(NOT_KEPT.into()));
    }
    let Ok(text) = read_snapshot_text(&dir.join("content.jsonl"), SNAPSHOT_FILE_MAX) else {
        return Some(Err(NOT_KEPT.into()));
    };
    let found = text
        .lines()
        .filter_map(|l| serde_json::from_str::<snapshot::ContentLine>(l).ok())
        .find(|c| c.sha256 == source.sha256);
    Some(match found {
        Some(line) => {
            // Redacted when it was written; once more on the way out (idempotent).
            let (text, n) = redact(&line.text);
            Ok(ContentOut {
                text,
                redactions: line.redactions + n as u32,
                policy,
            })
        }
        None => Err(NOT_KEPT.into()),
    })
}

fn remove_tree(path: &Path) {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_dir() => {
            let _ = std::fs::remove_dir_all(path);
        }
        Ok(_) => {
            let _ = std::fs::remove_file(path);
        }
        Err(_) => {}
    }
}

/// Puts a run folder right after a crash: a `.flight.tmp-<hex>-old` folder with no `flight/` is the snapshot
/// a swap had set aside, so it comes back first; every other `.flight.tmp-*` entry is removed.
fn recover_snapshot_leftovers(run_dir: &Path) {
    let entries: Vec<(String, PathBuf)> = std::fs::read_dir(run_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.file_name().to_str()?.to_string(), e.path())))
        .filter(|(n, _)| n.starts_with(SNAPSHOT_TMP))
        .collect();
    if entries.is_empty() {
        return;
    }
    let live = run_dir.join(SNAPSHOT_DIR);
    let mut restored = std::fs::symlink_metadata(&live).is_ok();
    for (name, path) in &entries {
        if !restored && name.ends_with("-old") && std::fs::rename(path, &live).is_ok() {
            restored = true;
        }
    }
    for (_, path) in entries {
        if path.exists() || std::fs::symlink_metadata(&path).is_ok() {
            remove_tree(&path);
        }
    }
}

/// The redacted text behind every distinct source line of `events` (found again by the line's sha256), read
/// from the transcripts. A line that changed since it was parsed is skipped.
fn snapshot_content(events: &[super::schema::Event]) -> Vec<snapshot::ContentLine> {
    let mut files: std::collections::HashMap<String, std::fs::File> =
        std::collections::HashMap::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for event in events {
        let source = &event.content.source;
        if source.len == 0 || source.len > MAX_LINE || !seen.insert(source.sha256.clone()) {
            continue;
        }
        let file = match files.entry(source.file.clone()) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => match std::fs::File::open(&source.file)
            {
                Ok(f) => e.insert(f),
                Err(_) => continue,
            },
        };
        let mut bytes = Vec::new();
        if file.seek(SeekFrom::Start(source.offset)).is_err()
            || file.take(source.len).read_to_end(&mut bytes).is_err()
            || bytes.len() as u64 != source.len
            || sha256_hex(&bytes) != source.sha256
        {
            continue;
        }
        let (text, redactions) = render_line(&bytes);
        out.push(snapshot::ContentLine {
            id: event.id.clone(),
            sha256: source.sha256.clone(),
            text,
            redactions,
        });
    }
    out
}

/// Builds a run's snapshot text: the live sessions parsed now plus, from the old snapshot, the sessions whose
/// transcripts are gone.
fn build_snapshot(
    run_dir: &Path,
    set: &SessionSet,
    outcome: &str,
    updated: Option<String>,
    old: Option<&snapshot::Old>,
    cap: usize,
) -> Result<snapshot::Built, String> {
    let recorded = crate::run::record::read_record(run_dir)
        .map(|r| r.sessions)
        .unwrap_or_default();
    let exact = exact_pr(run_dir);
    let mut live = Vec::new();
    let mut all_events = Vec::new();
    for files in &set.sessions {
        let id = session_id_of(&files.main);
        let config_dir = recorded
            .iter()
            .find(|s| s.session_id == id)
            .map(|s| s.config_dir.clone())
            .unwrap_or_default();
        let parsed = session::parse(&files.main, &files.subagents, exact);
        let sources = std::iter::once(files.main.as_path())
            .chain(files.subagents.iter().map(|s| s.path.as_path()))
            .filter_map(snap_source)
            .collect();
        all_events.extend(parsed.events.iter().cloned());
        live.push(snapshot::SessionIn {
            session: snapshot::SnapSession { id, config_dir },
            events: parsed.events,
            sources,
        });
    }
    let content = if matches!(set.policy, CapturePolicy::Redacted | CapturePolicy::Full) {
        snapshot_content(&all_events)
    } else {
        Vec::new()
    };
    let carry = old
        .map(|o| snapshot::carry_over(o, &set.gone))
        .unwrap_or_default();
    snapshot::build(snapshot::Input {
        created_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        policy: set.policy,
        outcome: outcome.to_string(),
        record_status_updated_at: updated,
        live: &live,
        carry,
        content,
        cap,
    })
}

/// Writes `built` as `<run>/flight/`: into `.flight.tmp-<hex>/`, then the swap. The old `flight/` is renamed
/// `.flight.tmp-<hex>-old`, the new one renamed `flight`, the old one removed; a failure puts the old one back.
/// `flight/` is never edited in place, and no write follows a link.
fn write_snapshot(run_dir: &Path, built: &snapshot::Built) -> Result<(), String> {
    let live = run_dir.join(SNAPSHOT_DIR);
    if let Ok(meta) = std::fs::symlink_metadata(&live) {
        if !meta.file_type().is_dir() {
            return Err("flight is not a folder".into());
        }
    }
    let hex = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let tmp = run_dir.join(format!("{SNAPSHOT_TMP}{hex}"));
    std::fs::create_dir(&tmp).map_err(|e| format!("temp folder: {e}"))?;
    let written = (|| -> Result<(), String> {
        let manifest = serde_json::to_vec_pretty(&built.manifest).map_err(|e| e.to_string())?;
        crate::store::write_run_file(&tmp.join("events.jsonl"), built.events_jsonl.as_bytes())?;
        if let Some(content) = &built.content_jsonl {
            crate::store::write_run_file(&tmp.join("content.jsonl"), content.as_bytes())?;
        }
        // The manifest last: a folder without one is never read as a snapshot.
        crate::store::write_run_file(&tmp.join("manifest.json"), &manifest)
    })();
    if let Err(why) = written {
        remove_tree(&tmp);
        return Err(why);
    }
    let old = run_dir.join(format!("{SNAPSHOT_TMP}{hex}-old"));
    let had_old = std::fs::symlink_metadata(&live).is_ok();
    if had_old {
        if let Err(e) = std::fs::rename(&live, &old) {
            remove_tree(&tmp);
            return Err(format!("could not set the old snapshot aside: {e}"));
        }
    }
    if let Err(e) = std::fs::rename(&tmp, &live) {
        if had_old {
            let _ = std::fs::rename(&old, &live);
        }
        remove_tree(&tmp);
        return Err(format!("could not move the snapshot in: {e}"));
    }
    if had_old {
        remove_tree(&old);
    }
    Ok(())
}

/// Why a run gets no snapshot now, or `None` when it does: its record is terminal, its account keeps run
/// traces, its policy is not `off`, and `flight/` is absent or belongs to another record.
fn snapshot_candidate(
    claude: &ClaudeSettings,
    run_dir: &Path,
) -> Option<(String, Option<String>, SessionSet)> {
    let record = crate::run::record::read_record(run_dir)?;
    let outcome = record
        .outcome
        .clone()
        .filter(|o| matches!(o.as_str(), "done" | "failed" | "eval-done"))?;
    let updated = record.status_updated_at.clone();
    let account = crate::run::folder::read_json_value(&run_dir.join("meta.json"))
        .and_then(|m| m["account"]["id"].as_str().map(str::to_string))?;
    if !claude
        .accounts
        .iter()
        .any(|a| a.id == account && a.keep_run_traces)
    {
        return None;
    }
    // Only the manifest: this runs for every kept run on every pass, so it must not read the big files.
    if let Some(old) = read_snapshot_manifest(run_dir) {
        if old.outcome == outcome && old.record_status_updated_at == updated {
            return None;
        }
    }
    let query = Query {
        run_dir: Some(run_dir.to_path_buf()),
        ..Default::default()
    };
    match resolve(claude, &query, &Opens::default()) {
        Access::Ready(set) => Some((outcome, updated, set)),
        Access::Off | Access::Refused(_) => None,
    }
}

/// Whether the last `trace snapshot` line of the run's log.md is a failure.
fn last_snapshot_line_failed(run_dir: &Path) -> bool {
    read_snapshot_text(&run_dir.join("log.md"), 8 * 1024 * 1024)
        .ok()
        .and_then(|log| {
            log.lines()
                .rev()
                .find(|l| l.contains(" app: trace snapshot "))
                .map(|l| l.contains(" app: trace snapshot failed: "))
        })
        .unwrap_or(false)
}

/// One snapshot pass over `run_dirs`, run by the index thread after each index pass. A run gets `<run>/flight/`
/// once per terminal record (see [`snapshot_candidate`]); at most [`SNAPSHOTS_PER_PASS`] runs per pass. A
/// failure leaves the previous `flight/` in place, is retried on a later pass at most [`SNAPSHOT_ATTEMPTS`]
/// times per run per app session, and is logged once per run, session and reason. Returns how many were written.
pub fn snapshot_pass(
    claude: &ClaudeSettings,
    run_dirs: &[PathBuf],
    state: &mut SnapshotState,
) -> usize {
    let mut written = 0;
    for dir in run_dirs {
        recover_snapshot_leftovers(dir);
        if written >= SNAPSHOTS_PER_PASS {
            continue;
        }
        if state
            .tried
            .get(dir)
            .is_some_and(|t| t.0 >= SNAPSHOT_ATTEMPTS)
        {
            continue;
        }
        let Some((outcome, updated, set)) = snapshot_candidate(claude, dir) else {
            continue;
        };
        let old = read_snapshot(dir);
        // A snapshot whose manifest is fine but whose Events cannot be read: replacing it would drop the
        // Events of sessions whose transcripts are gone, so keep it and fail with a reason.
        let result = if old.is_none() && read_snapshot_manifest(dir).is_some() {
            Err("the previous snapshot's events cannot be read".to_string())
        } else {
            build_snapshot(
                dir,
                &set,
                &outcome,
                updated,
                old.as_ref(),
                snapshot::MAX_BYTES,
            )
            .and_then(|built| write_snapshot(dir, &built))
        };
        match result {
            Ok(()) => {
                state.tried.remove(dir);
                written += 1;
                // The run panel shows the last failure until a later line says it was written.
                if last_snapshot_line_failed(dir) {
                    let _ = crate::run::folder::append_app_log(dir, "trace snapshot written");
                }
            }
            Err(reason) => {
                let entry = state.tried.entry(dir.clone()).or_default();
                entry.0 += 1;
                if entry.1.as_deref() != Some(reason.as_str()) {
                    entry.1 = Some(reason.clone());
                    let _ = crate::run::folder::append_app_log(
                        dir,
                        &format!("trace snapshot failed: {reason}"),
                    );
                }
            }
        }
    }
    written
}

/// `flight_search`: the structured search over `<data>/flight.db`, on a connection that can only read. A
/// database that does not exist yet gives no runs and the progress `0 of M` (M = the app's run folders).
pub fn search_index(data: &Path, filter: &SearchFilter) -> Result<SearchOut, String> {
    let db = data.join(INDEX_FILE);
    let mut out = if db.is_file() {
        let conn = index::open_reader(&db)?;
        index::search(&conn, filter)?
    } else {
        SearchOut {
            runs: Vec::new(),
            excluded_off: 0,
            progress: Counts::default(),
        }
    };
    if out.progress.total == 0 {
        out.progress.total = app_run_dirs(data).len() as i64;
    }
    Ok(out)
}

// ---- compare (CAP-10, AD-47) ----------------------------------------------------------

/// One side of a comparison, as the page shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareSide {
    pub run_id: String,
    pub run_dir: String,
    pub title: String,
    pub outcome: String,
    /// The run is in the trace index.
    pub indexed: bool,
    /// The account's policy is `off`: nothing of this side is read.
    pub off: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareOut {
    pub a: CompareSide,
    pub b: CompareSide,
    /// Empty unless both sides are indexed and readable.
    pub rows: Vec<compare::Row>,
    pub divergence: Option<usize>,
    /// How far the index is (runs current of runs seen), for the "not indexed yet" meter.
    pub progress: Counts,
}

/// The usage readings of an account's windows, for Window share: the host asks by config folder.
pub type UsageHistory = Vec<crate::usage::WindowHistory>;

fn rfc3339_ms(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|d| d.timestamp_millis())
}

fn share_of(
    claude: &ClaudeSettings,
    run_dir: &Path,
    record: &crate::run::record::RunRecord,
    usage: &dyn Fn(&str) -> UsageHistory,
) -> compare::Share {
    let account = crate::run::folder::read_json_value(&run_dir.join("meta.json"))
        .and_then(|m| m["account"]["id"].as_str().map(str::to_string));
    let (Some(account), Some(start), Some(end)) = (
        account,
        record.first_activity.as_deref().and_then(rfc3339_ms),
        record.last_activity.as_deref().and_then(rfc3339_ms),
    ) else {
        return compare::Share::default();
    };
    let config = resolved_account_config(claude, &account).path;
    let history = usage(&config);
    let of = |window: &str| {
        let readings: Vec<compare::Reading> = history
            .iter()
            .filter(|h| h.rate_limit_type == window)
            .flat_map(|h| h.readings.iter())
            .map(|r| compare::Reading {
                at_ms: r.at_ms,
                utilization: r.utilization,
                resets_at: r.resets_at,
            })
            .collect();
        compare::window_share(&readings, start, end)
    };
    compare::Share {
        five_hour: of("five_hour"),
        weekly: of("seven_day"),
    }
}

/// The indexed Events of one run, light columns for all and the whole Event only for the kinds Compare needs.
fn indexed_events(
    conn: &rusqlite::Connection,
    run_id: &str,
) -> Result<Vec<compare::IndexedEvent>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT kind, name, status, pr, failure, rule, paths,
                    CASE WHEN kind IN ('tool.start', 'check.end') OR failure IS NOT NULL THEN event_json END, id
             FROM events WHERE run_id = ?1 ORDER BY at, src_file, src_offset",
        )
        .map_err(|e| format!("index: {e}"))?;
    let rows = stmt
        .query_map([run_id], |r| {
            let paths: Option<String> = r.get(6)?;
            let json: Option<String> = r.get(7)?;
            Ok(compare::IndexedEvent {
                kind: r.get(0)?,
                name: r.get(1)?,
                status: r.get(2)?,
                pr: r.get(3)?,
                failure: r.get(4)?,
                rule: r.get(5)?,
                paths: paths
                    .map(|p| p.lines().map(str::to_string).collect())
                    .unwrap_or_default(),
                event: json.and_then(|j| serde_json::from_str(&j).ok()),
                id: r.get(8)?,
            })
        })
        .map_err(|e| format!("index: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("index: {e}"))
}

/// A read-only connection to `flight.db`, only when it exists with the schema this build reads (an older one is
/// rebuilt by the index thread, and until then nothing is indexed).
fn index_reader(data: &Path) -> Option<rusqlite::Connection> {
    let db = data.join(INDEX_FILE);
    if !db.is_file() {
        return None;
    }
    index::open_reader(&db).ok().filter(|c| {
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .is_ok_and(|v| v == index::SCHEMA_VERSION)
    })
}

/// The share of the account's five-hour window one finished run used (an estimate in percent points), from the
/// readings inside its span; `None` when there are not enough readings ("not known"). For the Eval dialog's
/// estimate (N times this).
pub fn run_five_hour_share(
    claude: &ClaudeSettings,
    run_dir: &Path,
    usage: &dyn Fn(&str) -> UsageHistory,
) -> Option<f64> {
    let record = crate::run::record::read_record(run_dir)?;
    share_of(claude, run_dir, &record, usage).five_hour
}

/// `flight_compare`: two run folders, each already through `checked_run_dir`. Reads the trace index through a
/// read-only connection and each run's record; it never writes `flight.db` and never indexes on demand. The
/// same folder twice is refused.
pub fn compare_runs(
    data: &Path,
    claude: &ClaudeSettings,
    a: &Path,
    b: &Path,
    usage: &dyn Fn(&str) -> UsageHistory,
) -> Result<CompareOut, String> {
    if ident(a) == ident(b) {
        return Err("Pick two different runs.".into());
    }
    let conn = index_reader(data);
    let mut sides = Vec::new();
    for dir in [a, b] {
        let run_id = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let query = Query {
            run_dir: Some(dir.to_path_buf()),
            ..Default::default()
        };
        let off = match resolve(claude, &query, &Opens::default()) {
            Access::Off => true,
            Access::Refused(why) => return Err(format!("{run_id}: {why}")),
            Access::Ready(_) => false,
        };
        let row: Option<(String, i64)> = conn.as_ref().and_then(|c| {
            c.query_row(
                "SELECT outcome, excluded_off FROM runs WHERE run_id = ?1",
                [&run_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok()
        });
        let record = crate::run::record::read_record(dir);
        let indexed = row.is_some();
        let off = off || row.as_ref().is_some_and(|(_, excluded)| *excluded != 0);
        let outcome = row
            .map(|(o, _)| o)
            .filter(|o| !o.is_empty())
            .or_else(|| record.as_ref().and_then(|r| r.outcome.clone()))
            .unwrap_or_default();
        let side = if indexed && !off {
            let events = indexed_events(conn.as_ref().expect("indexed"), &run_id)?;
            let record_in = record.as_ref().map(|r| compare::Record {
                tokens: compare::Tokens {
                    input: r.tokens.input,
                    output: r.tokens.output,
                    cache_write: r.tokens.cache_write_5m + r.tokens.cache_write_1h,
                    cache_read: r.tokens.cache_read,
                },
                minutes_by_stage: r.active_minutes_by_stage.clone(),
            });
            Some(compare::Side {
                events,
                share: record
                    .as_ref()
                    .map(|r| share_of(claude, dir, r, usage))
                    .unwrap_or_default(),
                record: record_in,
            })
        } else {
            None
        };
        sides.push((
            CompareSide {
                run_id,
                run_dir: dir.display().to_string(),
                title: crate::run::folder::request_title(dir).unwrap_or_default(),
                outcome,
                indexed,
                off,
            },
            side,
        ));
    }
    let (b_side, b_data) = sides.pop().expect("two sides");
    let (a_side, a_data) = sides.pop().expect("two sides");
    let rows = match (&a_data, &b_data) {
        (Some(x), Some(y)) => compare::rows(x, y),
        _ => compare::Rows {
            rows: Vec::new(),
            divergence: None,
        },
    };
    let progress = conn
        .as_ref()
        .map(|c| {
            let p = index::progress(c);
            Counts {
                current: p.current,
                total: p.seen,
            }
        })
        .unwrap_or_default();
    Ok(CompareOut {
        a: a_side,
        b: b_side,
        rows: rows.rows,
        divergence: rows.divergence,
        progress,
    })
}

// ---- eval results (CAP-15, AD-48) ------------------------------------------------------

/// min / median / max of one measure over the finished runs that have it; all `None` when none has.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dist {
    pub min: Option<f64>,
    pub median: Option<f64>,
    pub max: Option<f64>,
    /// How many runs the figures are over.
    pub of: usize,
}

impl Dist {
    pub fn of_values(mut values: Vec<f64>) -> Self {
        values.retain(|v| v.is_finite());
        if values.is_empty() {
            return Dist::default();
        }
        values.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
        let mid = values.len() / 2;
        let median = if values.len() % 2 == 1 {
            values[mid]
        } else {
            (values[mid - 1] + values[mid]) / 2.0
        };
        Dist {
            min: values.first().copied(),
            median: Some(median),
            max: values.last().copied(),
            of: values.len(),
        }
    }
}

/// One run of an eval group.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalRunResult {
    pub index: u32,
    pub label: String,
    pub run_id: Option<String>,
    pub run_dir: Option<String>,
    /// `finished` (eval-done), `failed`, `running` or `queued`.
    pub status: String,
    /// `eval-done` and every PR `reviewed` with a recorded check of `pass` or `none`; null for a run still going.
    pub completed: Option<bool>,
    /// PRs whose app check passed over PRs with an app check; null when the run has none ("not checked").
    pub check_pass_rate: Option<f64>,
    /// `checked`, `checking` or `not checked`.
    pub check_state: String,
    pub retries: Option<u32>,
    pub tool_failures: Option<u32>,
    pub tokens: Option<u64>,
    pub active_minutes: Option<f64>,
    /// Branches left after the worktrees were removed, with their unmerged commit count.
    pub kept_branches: Vec<crate::run::janitor::KeptBranch>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalDistributions {
    /// 1 for a completed run, 0 for one that is not.
    pub completed: Dist,
    pub check_pass_rate: Dist,
    pub retries: Dist,
    pub tool_failures: Dist,
    pub tokens: Dist,
    pub active_minutes: Dist,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalResults {
    pub group: String,
    pub source: String,
    pub n: u32,
    pub runs: Vec<EvalRunResult>,
    pub distributions: EvalDistributions,
    pub finished: usize,
    pub running: usize,
    pub failed: usize,
}

/// Retries and tool Failures of one indexed run, or `None` when the run is not in the index (or its account is
/// `off`).
fn indexed_counts(conn: &rusqlite::Connection, run_id: &str) -> Option<(u32, u32)> {
    conn.query_row(
        "SELECT 1 FROM runs WHERE run_id = ?1 AND excluded_off = 0",
        [run_id],
        |_| Ok(()),
    )
    .ok()?;
    let count = |sql: &str| {
        conn.query_row(sql, [run_id], |r| r.get::<_, i64>(0))
            .ok()
            .map(|n| n as u32)
    };
    Some((
        count("SELECT COUNT(*) FROM events WHERE run_id = ?1 AND kind = 'retry'")?,
        count(
            "SELECT COUNT(*) FROM events WHERE run_id = ?1 AND failure IS NOT NULL
               AND kind IN ('tool.end', 'tool.blocked')",
        )?,
    ))
}

/// `completed` by record-formats.md: `eval-done`, and every PR `reviewed` with a recorded check of `pass` or
/// `none` (the skill's own result in status.json, so it never depends on an app check).
fn eval_completed(run_dir: &Path, outcome: Option<&str>) -> bool {
    if outcome != Some("eval-done") {
        return false;
    }
    let Some(status) = crate::run::folder::read_json_value(&run_dir.join("status.json")) else {
        return false;
    };
    let prs = status["prs"].as_array().cloned().unwrap_or_default();
    !prs.is_empty()
        && prs.iter().all(|pr| {
            pr["state"].as_str() == Some("reviewed")
                && matches!(pr["check"]["result"].as_str(), Some("pass" | "none"))
        })
}

/// PRs whose app check passed over PRs whose app check ran; `None` with no app-checks.json or no real result.
fn app_check_rate(run_dir: &Path) -> Option<f64> {
    if !run_dir.join("app-checks.json").is_file() {
        return None;
    }
    let results: Vec<String> = crate::run::status::read_app_checks(run_dir)
        .into_values()
        .map(|c| c.result)
        .filter(|r| matches!(r.as_str(), "pass" | "fail" | "timeout" | "error"))
        .collect();
    if results.is_empty() {
        return None;
    }
    Some(results.iter().filter(|r| r.as_str() == "pass").count() as f64 / results.len() as f64)
}

/// `flight_eval_results`: the runs of one eval group with their measures and the min / median / max over the
/// finished ones, read-only. A measure that is not known is `None`, never 0. `checking` says whether an app
/// check is running for a run folder now.
pub fn eval_results(
    data: &Path,
    group: &str,
    checking: &dyn Fn(&Path) -> bool,
) -> Result<EvalResults, String> {
    let batch = crate::run::dispatch::load(data)
        .batches
        .into_iter()
        .find(|b| b.id == group && crate::run::dispatch::is_eval_batch(b))
        .ok_or("No such eval group.")?;
    let conn = index_reader(data);
    let mut runs = Vec::new();
    for (i, member) in batch.runs.iter().filter(|r| !r.removed).enumerate() {
        let index = i as u32 + 1;
        let dir = member
            .run_dir
            .as_deref()
            .and_then(|d| crate::run::checked_run_dir(data, d).ok());
        let Some(dir) = dir else {
            runs.push(EvalRunResult {
                index,
                label: member.label.clone(),
                run_id: None,
                run_dir: None,
                status: "queued".into(),
                completed: None,
                check_pass_rate: None,
                check_state: "not checked".into(),
                retries: None,
                tool_failures: None,
                tokens: None,
                active_minutes: None,
                kept_branches: Vec::new(),
            });
            continue;
        };
        let run_id = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let stage = crate::run::folder::read_json_value(&dir.join("status.json"))
            .and_then(|v| v["stage"].as_str().map(str::to_string));
        let record = crate::run::record::read_record(&dir);
        let outcome = record.as_ref().and_then(|r| r.outcome.clone());
        let eval = crate::run::folder::eval_of(&dir);
        let status = if crate::run::folder::finished_eval(eval.as_ref(), stage.as_deref()) {
            "finished"
        } else if stage.as_deref() == Some("failed") {
            "failed"
        } else {
            "running"
        };
        let ended = status != "running";
        let counts = if ended {
            conn.as_ref().and_then(|c| indexed_counts(c, &run_id))
        } else {
            None
        };
        let check_state = if checking(&dir) {
            "checking"
        } else if dir.join("app-checks.json").is_file() {
            "checked"
        } else {
            "not checked"
        };
        runs.push(EvalRunResult {
            index,
            label: member.label.clone(),
            run_id: Some(run_id),
            run_dir: Some(dir.display().to_string()),
            status: status.into(),
            completed: ended.then(|| eval_completed(&dir, outcome.as_deref())),
            check_pass_rate: if ended { app_check_rate(&dir) } else { None },
            check_state: check_state.into(),
            retries: counts.map(|c| c.0),
            tool_failures: counts.map(|c| c.1),
            tokens: record
                .as_ref()
                .filter(|_| ended)
                .map(|r| r.tokens.input + r.tokens.output),
            active_minutes: record
                .as_ref()
                .filter(|_| ended)
                .map(|r| r.active_minutes_by_stage.values().sum()),
            kept_branches: crate::run::janitor::kept_branches(&dir),
        });
    }
    let finished: Vec<&EvalRunResult> = runs.iter().filter(|r| r.status == "finished").collect();
    let over = |f: &dyn Fn(&EvalRunResult) -> Option<f64>| {
        Dist::of_values(finished.iter().filter_map(|r| f(r)).collect())
    };
    let distributions = EvalDistributions {
        completed: over(&|r| r.completed.map(|c| if c { 1.0 } else { 0.0 })),
        check_pass_rate: over(&|r| r.check_pass_rate),
        retries: over(&|r| r.retries.map(f64::from)),
        tool_failures: over(&|r| r.tool_failures.map(f64::from)),
        tokens: over(&|r| r.tokens.map(|t| t as f64)),
        active_minutes: over(&|r| r.active_minutes),
    };
    Ok(EvalResults {
        group: batch.id.clone(),
        source: batch.source.clone().unwrap_or_default(),
        n: batch.n.unwrap_or(runs.len() as u32),
        distributions,
        finished: finished.len(),
        running: runs
            .iter()
            .filter(|r| matches!(r.status.as_str(), "running" | "queued"))
            .count(),
        failed: runs.iter().filter(|r| r.status == "failed").count(),
        runs,
    })
}

/// What `flight_content` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentOut {
    pub text: String,
    pub redactions: u32,
    pub policy: CapturePolicy,
}

fn indent(text: &str, by: usize) -> String {
    let pad = " ".repeat(by);
    text.lines()
        .map(|l| format!("{pad}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Renders one JSON value as `key: value` lines. A string is redacted as raw
/// text (so a multi-line value is scanned line by line), never shown as
/// JSON-escaped text.
fn render_value(key: &str, value: &Value, depth: usize, redactions: &mut u32, out: &mut String) {
    let pad = " ".repeat(depth * 2);
    match value {
        Value::String(text) => {
            let (mut clean, mut n) = redact(text);
            // A secret named only by its key (`password: <value>`) has no
            // signal in the bare value: scan the first line as the rendered
            // `key: value` line too.
            let first = text.split(['\n', '\r']).next().unwrap_or("");
            if !clean.starts_with("[redacted: ") {
                if let Some(kind) = first_hit(&format!("{key}: {first}")) {
                    let rest = &clean[first.len().min(clean.len())..];
                    clean = format!("[redacted: {kind}]{rest}");
                    n += 1;
                }
            }
            *redactions += n as u32;
            if clean.contains('\n') || clean.contains('\r') {
                out.push_str(&format!("{pad}{key}:\n{}\n", indent(&clean, depth * 2 + 2)));
            } else {
                out.push_str(&format!("{pad}{key}: {clean}\n"));
            }
        }
        Value::Object(map) => {
            out.push_str(&format!("{pad}{key}:\n"));
            for (k, v) in map {
                render_value(k, v, depth + 1, redactions, out);
            }
        }
        Value::Array(items) => {
            out.push_str(&format!("{pad}{key}:\n"));
            for (i, v) in items.iter().enumerate() {
                render_value(&format!("[{i}]"), v, depth + 1, redactions, out);
            }
        }
        other => out.push_str(&format!("{pad}{key}: {other}\n")),
    }
}

/// Text of a tool_result `content`: a string, or the text blocks.
fn result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The display text of one transcript line, redacted, and the redaction
/// count. A line that is not JSON is shown as raw text, redacted.
pub fn render_line(bytes: &[u8]) -> (String, u32) {
    let mut redactions = 0u32;
    let mut out = String::new();
    let text = |s: &str, redactions: &mut u32, out: &mut String| {
        let (clean, n) = redact(s);
        *redactions += n as u32;
        out.push_str(&clean);
        out.push('\n');
    };
    match serde_json::from_slice::<Value>(bytes) {
        Err(_) => text(&String::from_utf8_lossy(bytes), &mut redactions, &mut out),
        Ok(line) => match &line["message"]["content"] {
            Value::String(s) => text(s, &mut redactions, &mut out),
            Value::Array(blocks) => {
                for block in blocks {
                    match block["type"].as_str() {
                        Some("tool_use") => {
                            out.push_str(&format!(
                                "tool_use {}\n",
                                block["name"].as_str().unwrap_or("?")
                            ));
                            render_value("input", &block["input"], 1, &mut redactions, &mut out);
                        }
                        Some("tool_result") => {
                            out.push_str("tool_result\n");
                            text(&result_text(&block["content"]), &mut redactions, &mut out);
                        }
                        Some("text") => text(
                            block["text"].as_str().unwrap_or(""),
                            &mut redactions,
                            &mut out,
                        ),
                        _ => {}
                    }
                }
            }
            _ => {
                out.push_str("(no text in this line)\n");
            }
        },
    }
    if out.len() > MAX_TEXT {
        let mut cut = MAX_TEXT;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push_str("\n[truncated]\n");
    }
    (out, redactions)
}

/// A run file's text, redacted line by line like a transcript line, cut at [`MAX_TEXT`]. The ref is a
/// line of the file or the whole file (offset 0, its length); the sha256 must still match.
fn read_run_file(
    source: &SourceRef,
    policy: CapturePolicy,
    opens: &Opens,
) -> Result<ContentOut, String> {
    if source.len > dcv2_run::MAX_FILE {
        return Err("that file is too large to show".into());
    }
    opens.add(1);
    let mut file = std::fs::File::open(&source.file).map_err(|e| format!("cannot read: {e}"))?;
    file.seek(SeekFrom::Start(source.offset))
        .map_err(|e| format!("cannot read: {e}"))?;
    let mut bytes = Vec::new();
    file.take(source.len)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read: {e}"))?;
    if bytes.len() as u64 != source.len || sha256_hex(&bytes) != source.sha256 {
        return Err("source changed".into());
    }
    let (mut text, redactions) = redact(&String::from_utf8_lossy(&bytes));
    if text.len() > MAX_TEXT {
        let mut cut = MAX_TEXT;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n[truncated]\n");
    }
    Ok(ContentOut {
        text,
        redactions: redactions as u32,
        policy,
    })
}

/// The content behind one event ref: policy first, then the file check, the
/// read, the sha256 check and the redaction.
pub fn read_content(
    claude: &ClaudeSettings,
    query: &Query,
    source: &SourceRef,
    opens: &Opens,
) -> Result<ContentOut, String> {
    match resolve(claude, query, opens) {
        Access::Refused(why) => Err(why),
        Access::Off => Ok(ContentOut {
            text: String::new(),
            redactions: 0,
            policy: CapturePolicy::Off,
        }),
        Access::Ready(set) => read_content_in(&set, source, opens),
    }
}

/// [`read_content`] for an already resolved set (the policy is not `off`).
fn read_content_in(
    set: &SessionSet,
    source: &SourceRef,
    opens: &Opens,
) -> Result<ContentOut, String> {
    if set.contains_run_file(&source.file) {
        return read_run_file(source, set.policy, opens);
    }
    if !set.contains(&source.file) {
        // A gone transcript: the run's snapshot may hold the content.
        if let Some(found) = set
            .run_dir
            .as_deref()
            .and_then(|dir| snapshot_content_for(dir, set.policy, source))
        {
            return found;
        }
        return Err(if set.run_dir.is_some() {
            "that file is not part of this run"
        } else {
            "that file is not part of this session"
        }
        .into());
    }
    let bytes = read_transcript_line(source, opens)?;
    let (text, redactions) = render_line(&bytes);
    Ok(ContentOut {
        text,
        redactions,
        policy: set.policy,
    })
}

/// One transcript line by its ref, checked against its sha256. The caller has checked that the file is part
/// of the resolved set.
fn read_transcript_line(source: &SourceRef, opens: &Opens) -> Result<Vec<u8>, String> {
    if source.len > MAX_LINE {
        return Err("that line is too large to show".into());
    }
    opens.add(1);
    let mut file = std::fs::File::open(&source.file).map_err(|e| format!("cannot read: {e}"))?;
    file.seek(SeekFrom::Start(source.offset))
        .map_err(|e| format!("cannot read: {e}"))?;
    let mut bytes = Vec::new();
    file.take(source.len)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read: {e}"))?;
    if bytes.len() as u64 != source.len || sha256_hex(&bytes) != source.sha256 {
        return Err("source changed".into());
    }
    Ok(bytes)
}

/// A Trace ready to export: the capture policy of its account and the data `export::build` takes.
/// `None` when the policy is `off`. Tool text is read (redacted) only when `with_content`.
pub fn export_input(
    claude: &ClaudeSettings,
    query: &Query,
    cache: &FlightCache,
    opens: &Opens,
    with_content: bool,
) -> Result<Option<(CapturePolicy, export::ExportInput)>, String> {
    let Some(loaded) = load(claude, query, cache, opens)? else {
        return Ok(None);
    };
    let merged = session::merge(&loaded.parsed, loaded.run.as_deref());
    let trace = trace_of(query).or_else(|| {
        loaded
            .parsed
            .first()
            .and_then(|p| p.events.first())
            .map(|e| e.trace.clone())
    });
    let mut events: Vec<super::schema::Event> = merged
        .ordered
        .iter()
        .map(|(_, e)| e.as_ref().clone())
        .collect();
    let trace = trace.unwrap_or_default();
    let mut content = std::collections::BTreeMap::new();
    for e in events.iter_mut() {
        e.trace = trace.clone();
        let tool_edge =
            e.kind == super::schema::kind::TOOL_START || e.kind == super::schema::kind::TOOL_END;
        if with_content && tool_edge && e.agent.id != super::schema::ORCHESTRATION {
            // A line that cannot be read now (changed, gone) is left out, not an error.
            if let Ok(out) = read_content_in(&loaded.set, &e.content.source, opens) {
                content.insert(
                    e.id.clone(),
                    export::ToolText {
                        text: out.text,
                        redactions: out.redactions,
                    },
                );
            }
        }
    }
    Ok(Some((
        loaded.set.policy,
        export::ExportInput {
            trace,
            events,
            agents: loaded.parsed.iter().map(|p| p.tree.clone()).collect(),
            failures: merged.failures,
            content,
        },
    )))
}

// ---- Event replay (AF13): the replay track and a file after each recorded edit ----

/// The track holds at most this many Events.
pub const MAX_TRACK: usize = 20_000;
/// Shown when the replayed Trace has no run (its files belong to no PR worktree).
const REPLAY_NO_RUN: &str =
    "Replaying file changes needs a run: a session alone has no PR worktrees.";

/// One tick of the replay track. Eight keys, no path.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReplayEntry {
    pub id: String,
    pub at: String,
    pub kind: String,
    pub name: Option<String>,
    pub status: Option<String>,
    /// How many files a `file.change` touched (never their paths); `None` for other kinds.
    pub file: Option<usize>,
    /// `skill`, `app` or `stop-check` for a `check.end`.
    pub check_source: Option<String>,
    pub pr: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReplayTrack {
    /// `ok`, or `off` when the policy lets nothing be read.
    pub status: String,
    pub entries: Vec<ReplayEntry>,
    pub truncated: bool,
}

/// One entry per Event of the Trace, in Trace order (the order `flight_session` pages in).
pub fn replay_track(
    claude: &ClaudeSettings,
    query: &Query,
    cache: &FlightCache,
    opens: &Opens,
) -> Result<ReplayTrack, String> {
    use super::schema::kind;
    let Some(loaded) = load(claude, query, cache, opens)? else {
        return Ok(ReplayTrack {
            status: "off".into(),
            entries: Vec::new(),
            truncated: false,
        });
    };
    let merged = session::merge(&loaded.parsed, loaded.run.as_deref());
    let truncated = merged.ordered.len() > MAX_TRACK;
    let entries = merged
        .ordered
        .iter()
        .take(MAX_TRACK)
        .map(|(_, e)| {
            let text = |key: &str| e.attrs.get(key).and_then(Value::as_str).map(str::to_string);
            ReplayEntry {
                id: e.id.clone(),
                at: e.at.clone(),
                kind: e.kind.clone(),
                name: e.name.clone(),
                status: e.status.clone(),
                file: (e.kind == kind::FILE_CHANGE).then(|| {
                    e.attrs
                        .get("files")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len)
                }),
                check_source: (e.kind == kind::CHECK_END)
                    .then(|| text(super::schema::attr::SOURCE))
                    .flatten(),
                pr: text(super::schema::attr::PR).or_else(|| e.agent.pr.clone()),
            }
        })
        .collect();
    Ok(ReplayTrack {
        status: "ok".into(),
        entries,
        truncated,
    })
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReplayStop {
    /// The 1-based edit (among the file's recorded edits) that does not apply.
    pub at: usize,
    pub reason: String,
}

/// The file after one recorded edit.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReplayFile {
    /// `ok` (text is set), `off` (the policy reads nothing) or `unavailable` (see `reason`).
    pub status: String,
    pub reason: Option<String>,
    /// The file relative to the PR's worktree.
    pub path: Option<String>,
    pub pr: Option<String>,
    pub text: Option<String>,
    pub redactions: u32,
    /// The text was cut at 256 KiB.
    pub truncated: bool,
    /// This edit is the k-th of the file's n successful recorded edits.
    pub edit: Option<usize>,
    pub edits: usize,
    pub stopped: Option<ReplayStop>,
}

impl ReplayFile {
    fn none(status: &str, reason: impl Into<String>) -> Self {
        Self {
            status: status.into(),
            reason: Some(reason.into()),
            path: None,
            pr: None,
            text: None,
            redactions: 0,
            truncated: false,
            edit: None,
            edits: 0,
            stopped: None,
        }
    }
}

fn unavailable(reason: impl Into<String>) -> ReplayFile {
    ReplayFile::none("unavailable", reason)
}

/// `path` relative to `prefix`, comparing the real paths when both exist and the spelled ones otherwise
/// (a removed worktree has no real path), so a merged run whose worktrees were cleaned up still maps.
pub(crate) fn strip_prefix(path: &Path, prefix: &Path) -> Option<PathBuf> {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let fold = |p: PathBuf| {
        if cfg!(windows) {
            PathBuf::from(p.to_string_lossy().to_lowercase())
        } else {
            p
        }
    };
    for (a, b) in [
        (canon(path), canon(prefix)),
        (path.to_path_buf(), prefix.to_path_buf()),
    ] {
        if let Ok(rel) = fold(a).strip_prefix(fold(b)) {
            return Some(rel.to_path_buf());
        }
    }
    None
}

/// A PR of a run: its verified repo, base commit and the text prefix of its (possibly removed) worktree.
struct ReplayPr {
    id: String,
    repo: PathBuf,
    base: Option<String>,
    prefix: PathBuf,
}

/// The run's PRs from status.json only. The worktree folder is never verified, read or opened: its path
/// is `janitor::expected_worktree` of the verified repo, used as text.
fn replay_prs(run_dir: &Path) -> Vec<ReplayPr> {
    let run_id = run_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let status = crate::run::folder::read_json_value(&run_dir.join("status.json"));
    let prs = status
        .as_ref()
        .and_then(|s| s.get("prs"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    prs.iter()
        .filter_map(|pr| {
            let id = pr.get("id")?.as_str()?.to_string();
            if !crate::run::status::valid_pr_id(&id) {
                return None;
            }
            let repo = crate::run::janitor::verified_repo(run_dir, pr.get("repoPath")?.as_str()?)?;
            let prefix = crate::run::janitor::expected_worktree(&repo, &run_id, &id)?;
            let base = pr
                .get("baseCommit")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|b| valid_commit(b));
            Some(ReplayPr {
                id,
                repo,
                base,
                prefix,
            })
        })
        .collect()
}

/// A commit id as git prints it: hex only, so it can never be read as an option or a ref expression.
pub(crate) fn valid_commit(text: &str) -> bool {
    (7..=64).contains(&text.len()) && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// A relative path with no `..` part, no root and no option-like start, in git's `/` form.
fn git_path(rel: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for part in rel.components() {
        match part {
            std::path::Component::Normal(p) => parts.push(p.to_string_lossy().to_string()),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The edit a recorded `tool_use` made, from its input.
fn edit_of(name: &str, input: &Value) -> Result<replay::Edit, String> {
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
    let replacement = |v: &Value| -> Option<replay::Replacement> {
        Some(replay::Replacement {
            old: text(v, "old_string")?,
            new: text(v, "new_string")?,
            replace_all: v
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    };
    let bad = || "the recorded input has an unexpected shape".to_string();
    match name {
        "Write" => Ok(replay::Edit::Write {
            content: text(input, "content").ok_or_else(bad)?,
        }),
        "Edit" => Ok(replay::Edit::Edit(replacement(input).ok_or_else(bad)?)),
        "MultiEdit" => {
            let list = input
                .get("edits")
                .and_then(Value::as_array)
                .ok_or_else(bad)?;
            let edits: Option<Vec<_>> = list.iter().map(replacement).collect();
            Ok(replay::Edit::MultiEdit {
                edits: edits.ok_or_else(bad)?,
            })
        }
        _ => Err(bad()),
    }
}

/// The recorded input of a `tool_use` (by id) on a transcript line, in memory only.
fn recorded_edit(
    set: &SessionSet,
    event: &super::schema::Event,
    opens: &Opens,
) -> Result<replay::Edit, String> {
    let source = &event.content.source;
    if !set.contains(&source.file) {
        return Err("the transcript is gone, so the recorded input cannot be read".into());
    }
    let bytes = read_transcript_line(source, opens)?;
    let line: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "the recorded line is not readable".to_string())?;
    let id = event.span.as_deref().unwrap_or("");
    let block = line["message"]["content"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|b| b["type"] == "tool_use" && b["id"] == id)
        .ok_or_else(|| "the recorded input is not on that line".to_string())?;
    edit_of(block["name"].as_str().unwrap_or(""), &block["input"])
}

/// The file a `file.change` Event changed, as it was after that edit: the PR's base commit plus the recorded
/// successful edits of that file up to this one. Read-only: the base comes from git in the run's verified repo,
/// the edits from the transcript lines, in memory; nothing recorded is ever run, and nothing is written.
pub fn replay_file(
    claude: &ClaudeSettings,
    query: &Query,
    cache: &FlightCache,
    opens: &Opens,
    event_id: &str,
) -> Result<ReplayFile, String> {
    use super::schema::{kind, Event};
    let Some(loaded) = load(claude, query, cache, opens)? else {
        return Ok(ReplayFile::none("off", EXPORT_OFF));
    };
    let merged = session::merge(&loaded.parsed, loaded.run.as_deref());
    let events: Vec<&Event> = merged.ordered.iter().map(|(_, e)| e.as_ref()).collect();
    let Some(at) = events.iter().position(|e| e.id == event_id) else {
        return Ok(unavailable("that event is not in this trace"));
    };
    let target = events[at];
    let file_of = |e: &Event| -> Option<String> {
        e.attrs
            .get("files")
            .and_then(Value::as_array)
            .and_then(|f| f.first())
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let Some(path) = (target.kind == kind::FILE_CHANGE)
        .then(|| file_of(target))
        .flatten()
    else {
        return Ok(unavailable("that event is not a file change"));
    };
    if target.name.as_deref() == Some("NotebookEdit") || path.ends_with(".ipynb") {
        return Ok(unavailable("Notebooks are not reconstructed."));
    }
    let Some(run_dir) = query.run_dir.as_deref() else {
        return Ok(unavailable(REPLAY_NO_RUN));
    };
    // Which PR's worktree holds the file.
    let spelled = PathBuf::from(&path);
    let found = replay_prs(run_dir)
        .into_iter()
        .find_map(|pr| strip_prefix(&spelled, &pr.prefix).map(|rel| (pr, rel)));
    let Some((pr, rel)) = found else {
        return Ok(unavailable(
            "This file is outside every PR worktree of the run.",
        ));
    };
    let Some(git_rel) = git_path(&rel) else {
        return Ok(unavailable("This path is not a file inside a worktree."));
    };
    let mut out = unavailable("");
    out.path = Some(git_rel.clone());
    out.pr = Some(pr.id.clone());
    let Some(base) = pr.base.as_deref() else {
        out.reason = Some("No base commit is recorded for this PR.".into());
        return Ok(out);
    };
    let is_commit = crate::run::git::git(&pr.repo, &["cat-file", "-t", base])
        .is_ok_and(|kind| kind == "commit");
    if !is_commit {
        out.reason = Some(
            "The base commit is not in this repo (the branch may have been cleaned up).".into(),
        );
        return Ok(out);
    }
    // The base text: absent at the base commit is a new file, not an error.
    let spec = format!("{base}:{git_rel}");
    let base_text = match crate::run::git::git(&pr.repo, &["cat-file", "-s", &spec]) {
        Err(_) => None,
        Ok(size) => {
            if size.parse::<usize>().unwrap_or(usize::MAX) > replay::MAX_FILE {
                out.reason = Some("This file is over 2 MiB; it is not reconstructed.".into());
                return Ok(out);
            }
            match crate::run::git::git_bytes(&pr.repo, &["show", &spec], replay::MAX_FILE) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(text) => Some(text),
                    Err(_) => {
                        out.reason = Some("This file is not text.".into());
                        return Ok(out);
                    }
                },
                Err(why) => {
                    out.reason = Some(format!("The base version cannot be read: {why}"));
                    return Ok(out);
                }
            }
        }
    };
    // The file's successful recorded edits, in order, up to this one.
    let ended_ok: std::collections::HashSet<(&str, &str)> = events
        .iter()
        .filter(|e| e.kind == kind::TOOL_END && e.status.as_deref() == Some("ok"))
        .filter_map(|e| Some((e.agent.id.as_str(), e.span.as_deref()?)))
        .collect();
    let succeeded = |e: &Event| {
        e.span
            .as_deref()
            .is_some_and(|s| ended_ok.contains(&(e.agent.id.as_str(), s)))
    };
    let mine: Vec<(usize, &Event)> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e.kind == kind::FILE_CHANGE
                && file_of(e).as_deref() == Some(path.as_str())
                && succeeded(e)
        })
        .map(|(i, e)| (i, *e))
        .collect();
    out.edits = mine.len();
    let upto: Vec<&(usize, &Event)> = mine.iter().filter(|(i, _)| *i <= at).collect();
    out.edit = succeeded(target).then_some(upto.len());
    let mut edits = Vec::new();
    let mut unreadable: Option<(usize, String)> = None;
    for (k, (_, e)) in upto.iter().enumerate() {
        match recorded_edit(&loaded.set, e, opens) {
            Ok(edit) => edits.push(edit),
            Err(why) => {
                unreadable = Some((k, why));
                break;
            }
        }
    }
    let state = replay::reconstruct(base_text.as_deref(), &edits);
    let stopped = match (state.stopped_at, &state.reason, unreadable) {
        (Some(k), Some(why), _) => Some(ReplayStop {
            at: k + 1,
            reason: why.clone(),
        }),
        (None, _, Some((k, why))) => Some(ReplayStop {
            at: k + 1,
            reason: why,
        }),
        _ => None,
    };
    out.stopped = stopped;
    match state.text {
        Some(text) => {
            let (mut text, redactions) = redact(&text);
            out.redactions = redactions as u32;
            if text.len() > MAX_TEXT {
                let mut cut = MAX_TEXT;
                while !text.is_char_boundary(cut) {
                    cut -= 1;
                }
                text.truncate(cut);
                out.truncated = true;
            }
            out.text = Some(text);
            out.status = "ok".into();
            out.reason = None;
        }
        None => {
            out.reason = Some("The file does not exist yet at this point.".into());
        }
    }
    if !succeeded(target) && out.status == "ok" {
        out.reason = Some("This edit failed, so the file is as it was before it.".into());
    }
    Ok(out)
}

// ---- Export (AF12): Preview, Send, Save ----

/// The one refusal for bytes that are no longer the previewed ones. `src/run/words.ts` holds the same text.
pub const CHANGED_SINCE_PREVIEW: &str = "the trace changed since the preview";
/// Shown when the account's capture policy is `off`.
pub const EXPORT_OFF: &str = "AgentFlight is off for this account (Settings › Activity).";
/// The export POST takes at most this long.
const EXPORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The bytes of the last Preview, one entry, replaced by the next Preview. Send and Save take exactly
/// these bytes, so what leaves is what the user saw (a Trace that grew in between is not re-read).
#[derive(Debug, Clone, Default)]
pub struct ExportState {
    previewed: Arc<Mutex<Option<Previewed>>>,
}

#[derive(Debug)]
struct Previewed {
    digest: String,
    bytes: Arc<Vec<u8>>,
    content_type: &'static str,
    spans: usize,
}

#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub policy: export::ExportPolicy,
    pub encoding: export::Encoding,
    /// The per-export tick that `full` needs.
    #[serde(default)]
    pub confirm_full: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPreview {
    /// The policy actually used (`full` is clamped to `metadata` without its conditions).
    pub policy: export::ExportPolicy,
    pub encoding: export::Encoding,
    pub endpoint: String,
    pub spans: usize,
    pub events: usize,
    pub bytes: usize,
    pub digest: String,
    pub fields: Vec<export::FieldCount>,
    pub warnings: Vec<String>,
    /// The endpoint is on this machine.
    pub loopback: bool,
    /// The Trace's capture policy: the dialog lists `full` only for `full`.
    pub capture_policy: CapturePolicy,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExportSent {
    pub endpoint: String,
    pub spans: usize,
    pub status: u16,
}

/// An export endpoint: `http://` or `https://` with a host. Returns it trimmed.
pub fn check_endpoint(text: &str) -> Result<String, String> {
    crate::store::settings_store::check_export_endpoint(text)
}

/// The endpoint's host is this machine (`localhost`, `127.x.x.x`, `::1`).
pub fn is_loopback(endpoint: &str) -> bool {
    let Ok(uri) = endpoint.trim().parse::<ureq::http::Uri>() else {
        return false;
    };
    let host = uri
        .host()
        .unwrap_or("")
        .trim_matches(|c| c == '[' || c == ']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The agent that sends an export: http or https, no redirect (a redirect could carry the body to
/// another host), no proxy (the environment's proxy variables are not read, so a loopback export stays
/// on loopback), 30 s, status codes are replies to read.
pub fn export_agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(EXPORT_TIMEOUT))
        .http_status_as_error(false)
        .max_redirects(0)
        .proxy(None)
        .user_agent(concat!("DCTerminalv2/", env!("CARGO_PKG_VERSION")))
        .build();
    ureq::Agent::new_with_config(config)
}

/// Builds the payload for a Trace and keeps its bytes for Send and Save.
pub fn export_preview(
    claude: &ClaudeSettings,
    query: &Query,
    cache: &FlightCache,
    opens: &Opens,
    state: &ExportState,
    request: &ExportRequest,
    endpoint: &str,
) -> Result<ExportPreview, String> {
    let endpoint = check_endpoint(endpoint)?;
    // The policy first: nothing is read for `off`.
    let capture = match resolve(claude, query, opens) {
        Access::Refused(why) => return Err(why),
        Access::Off => return Err(EXPORT_OFF.into()),
        Access::Ready(set) => set.policy,
    };
    let mut warnings = Vec::new();
    let mut policy = export::ExportPolicy::Metadata;
    if request.policy == export::ExportPolicy::Full {
        if capture != CapturePolicy::Full {
            warnings.push(
                "The account's capture policy is not full, so no tool text is exported."
                    .to_string(),
            );
        } else if !request.confirm_full {
            warnings.push("Tool text is not exported until you confirm it.".to_string());
        } else {
            policy = export::ExportPolicy::Full;
        }
    }
    let Some((_, input)) = export_input(
        claude,
        query,
        cache,
        opens,
        policy == export::ExportPolicy::Full,
    )?
    else {
        return Err(EXPORT_OFF.into());
    };
    let payload = export::build(&input, policy, request.encoding)?;
    let loopback = is_loopback(&endpoint);
    let preview = ExportPreview {
        policy,
        encoding: request.encoding,
        endpoint,
        spans: payload.spans,
        events: payload.events,
        bytes: payload.bytes.len(),
        digest: payload.digest.clone(),
        fields: payload.fields,
        warnings,
        loopback,
        capture_policy: capture,
    };
    *state.previewed.lock().map_err(|e| e.to_string())? = Some(Previewed {
        digest: payload.digest,
        bytes: Arc::new(payload.bytes),
        content_type: payload.content_type,
        spans: preview.spans,
    });
    Ok(preview)
}

fn previewed(
    state: &ExportState,
    digest: &str,
) -> Result<(Arc<Vec<u8>>, &'static str, usize), String> {
    let guard = state.previewed.lock().map_err(|e| e.to_string())?;
    match guard.as_ref() {
        Some(p) if p.digest == digest => Ok((p.bytes.clone(), p.content_type, p.spans)),
        _ => Err(CHANGED_SINCE_PREVIEW.into()),
    }
}

/// Posts exactly the previewed bytes (one request, no retry, no redirect).
pub fn export_send(
    state: &ExportState,
    agent: &ureq::Agent,
    digest: &str,
    endpoint: &str,
) -> Result<ExportSent, String> {
    let endpoint = check_endpoint(endpoint)?;
    let (bytes, content_type, spans) = previewed(state, digest)?;
    let response = agent
        .post(&endpoint)
        .header("Content-Type", content_type)
        .send(&bytes[..])
        .map_err(|e| format!("could not reach {endpoint}: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "{endpoint} answered {} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("")
        )
        .trim_end()
        .to_string());
    }
    Ok(ExportSent {
        endpoint,
        spans,
        status: status.as_u16(),
    })
}

/// Writes exactly the previewed bytes to `path` (created or truncated, never through a link).
pub fn export_save(state: &ExportState, digest: &str, path: &Path) -> Result<(), String> {
    use std::io::Write;
    let (bytes, _, _) = previewed(state, digest)?;
    if !path.is_absolute() {
        return Err("choose a full path to save to".into());
    }
    let mut file = crate::store::create_run_log(path)
        .map_err(|e| format!("cannot save to {}: {e}", path.display()))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("cannot save to {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flight::derive;
    use crate::flight::index;
    use crate::flight::schema::{kind, AgentNode, Event, Failure, Phase};
    use crate::flight::testkit::*;
    use crate::test_support::TempDir;
    use serde_json::json;

    const SID_A: &str = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";
    const SID_B: &str = "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb";

    fn account(id: &str, dir: &Path, policy: CapturePolicy) -> ClaudeAccount {
        ClaudeAccount {
            id: id.into(),
            name: id.to_uppercase(),
            config_dir: Some(dir.display().to_string()),
            capture_policy: policy,
            keep_run_traces: false,
        }
    }

    /// `config/projects/-p/<sid>.jsonl` holding `lines`.
    fn session(config: &Path, sid: &str, lines: &[Value]) -> PathBuf {
        let dir = config.join("projects/-p");
        std::fs::create_dir_all(&dir).unwrap();
        let text: String = lines.iter().map(|l| l.to_string() + "\n").collect();
        let path = dir.join(format!("{sid}.jsonl"));
        std::fs::write(&path, text).unwrap();
        path
    }

    fn settings(accounts: Vec<ClaudeAccount>) -> ClaudeSettings {
        ClaudeSettings {
            claude_path: None,
            accounts,
        }
    }

    /// The source ref of line `n` of `file`, as an event would hold it.
    fn source_of(file: &Path, n: usize) -> SourceRef {
        let bytes = std::fs::read(file).unwrap();
        let mut offset = 0usize;
        for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
            if i == n {
                return SourceRef {
                    file: file.display().to_string(),
                    offset: offset as u64,
                    len: line.len() as u64,
                    sha256: sha256_hex(line),
                };
            }
            offset += line.len() + 1;
        }
        panic!("no line {n}");
    }

    fn by_session(config: &Path, sid: &str) -> Query {
        Query {
            run_dir: None,
            config_dir: Some(config.display().to_string()),
            session_id: Some(sid.into()),
        }
    }

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

    #[test]
    fn a_secret_named_only_by_its_key_is_redacted_in_a_tool_input() {
        let mut out = String::new();
        let mut n = 0u32;
        let input = json!({"api_key": "Zx8Kq2Lm9Pw4Rt7Yb1Nc6Vd3", "note": "plain"});
        for (k, v) in input.as_object().unwrap() {
            render_value(k, v, 0, &mut n, &mut out);
        }
        assert_eq!(n, 1, "{out}");
        assert!(out.contains("[redacted: "), "{out}");
        assert!(!out.contains("Zx8Kq2"), "{out}");
        assert!(out.contains("note: plain"), "{out}");
        let mut out = String::new();
        let mut n = 0u32;
        render_value("password", &json!("abcd1234efgh"), 0, &mut n, &mut out);
        assert_eq!(n, 1, "{out}");
        assert!(!out.contains("abcd1234"), "{out}");
    }

    #[test]
    fn a_write_with_a_dotenv_secret_on_a_later_line_is_redacted_per_value() {
        let dir = TempDir::new("flight_content_env");
        let config = dir.join("a");
        let file = session(
            &config,
            SID_A,
            &[
                tool_use(
                    "2026-10-22T10:00:00Z",
                    "m1",
                    "t1",
                    "Write",
                    json!({"file_path": "/work/.env",
                        "content": "DB_HOST=localhost\nDB_PASSWORD=Hunter2Hunter2xyz\n"}),
                ),
                tool_use(
                    "2026-10-22T10:00:01Z",
                    "m2",
                    "t2",
                    "Bash",
                    json!({"command": "echo hi\nexport DB_PASSWORD=Hunter2Hunter2xyz"}),
                ),
                tool_result("2026-10-22T10:00:02Z", "t2", "all fine here", false),
            ],
        );
        let claude = settings(vec![account("a", &config, CapturePolicy::Metadata)]);
        let opens = Opens::default();
        for (line, kept) in [(0, "DB_HOST=localhost"), (1, "echo hi")] {
            let out = read_content(
                &claude,
                &by_session(&config, SID_A),
                &source_of(&file, line),
                &opens,
            )
            .unwrap();
            assert_eq!(out.redactions, 1, "{}", out.text);
            assert!(out.text.contains(kept), "{}", out.text);
            assert!(out.text.contains("[redacted: "), "{}", out.text);
            assert!(!out.text.contains("Hunter2"), "{}", out.text);
            assert!(!out.text.contains("\\n"), "raw values, not escaped JSON");
        }
        let out = read_content(
            &claude,
            &by_session(&config, SID_A),
            &source_of(&file, 2),
            &opens,
        )
        .unwrap();
        assert_eq!(
            (out.text.contains("all fine here"), out.redactions),
            (true, 0)
        );
        assert_eq!(out.policy, CapturePolicy::Metadata);
    }

    #[test]
    fn an_off_account_in_a_run_means_no_file_is_opened_whatever_the_meta_names() {
        let dir = TempDir::new("flight_two_accounts");
        let (ca, cb) = (dir.join("a"), dir.join("b"));
        let fa = session(&ca, SID_A, &[prompt("2026-10-22T10:00:00Z")]);
        session(&cb, SID_B, &[prompt("2026-10-22T10:00:00Z")]);
        let run = dir.join("run");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(
            run.join("app-usage.json"),
            json!({"schema": 1, "sessions": [
                {"sessionId": SID_A, "configDir": ca.display().to_string()},
                {"sessionId": SID_B, "configDir": cb.display().to_string()}]})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            run.join("meta.json"),
            json!({"account": {"id": "a"}}).to_string(),
        )
        .unwrap();
        let claude = settings(vec![
            account("a", &ca, CapturePolicy::Full),
            account("b", &cb, CapturePolicy::Off),
        ]);
        let query = Query {
            run_dir: Some(run.clone()),
            ..Default::default()
        };
        let opens = Opens::default();
        assert!(matches!(resolve(&claude, &query, &opens), Access::Off));
        let out = read_content(&claude, &query, &source_of(&fa, 0), &opens).unwrap();
        assert_eq!(out.policy, CapturePolicy::Off);
        assert!(out.text.is_empty());
        assert_eq!(opens.count(), 0, "nothing was opened");
        // Without B in the run, A's `full` applies.
        std::fs::write(
            run.join("app-usage.json"),
            json!({"schema": 1, "sessions": [
                {"sessionId": SID_A, "configDir": ca.display().to_string()}]})
            .to_string(),
        )
        .unwrap();
        let Access::Ready(set) = resolve(&claude, &query, &opens) else {
            panic!("A alone is readable");
        };
        assert_eq!(set.policy, CapturePolicy::Full);
    }

    #[test]
    fn off_returns_the_policy_off_result_even_for_a_missing_file() {
        let dir = TempDir::new("flight_off_missing");
        let config = dir.join("a");
        std::fs::create_dir_all(&config).unwrap();
        let claude = settings(vec![account("a", &config, CapturePolicy::Off)]);
        let missing = SourceRef {
            file: config.join("projects/-p/none.jsonl").display().to_string(),
            offset: 0,
            len: 10,
            sha256: "x".into(),
        };
        let opens = Opens::default();
        let out = read_content(&claude, &by_session(&config, SID_A), &missing, &opens).unwrap();
        assert_eq!(out.policy, CapturePolicy::Off);
        assert_eq!(opens.count(), 0);
        assert!(matches!(
            resolve(&claude, &by_session(&config, SID_A), &opens),
            Access::Off
        ));
    }

    #[test]
    fn a_folder_of_no_account_is_refused_and_another_spelling_resolves() {
        let dir = TempDir::new("flight_unknown_folder");
        let (ca, other) = (dir.join("a"), dir.join("other"));
        let file = session(&ca, SID_A, &[prompt("2026-10-22T10:00:00Z")]);
        std::fs::create_dir_all(&other).unwrap();
        let claude = settings(vec![account("a", &ca, CapturePolicy::Metadata)]);
        let opens = Opens::default();
        let refused = resolve(&claude, &by_session(&other, SID_A), &opens);
        assert!(matches!(&refused, Access::Refused(why) if why == UNKNOWN_FOLDER));
        assert!(read_content(
            &claude,
            &by_session(&other, SID_A),
            &source_of(&file, 0),
            &opens
        )
        .is_err());
        assert_eq!(opens.count(), 0);
        let spelled = Query {
            config_dir: Some(format!("{}/", ca.display())),
            session_id: Some(SID_A.into()),
            run_dir: None,
        };
        assert!(matches!(
            resolve(&claude, &spelled, &opens),
            Access::Ready(_)
        ));
        #[cfg(unix)]
        {
            let link = dir.join("link");
            std::os::unix::fs::symlink(&ca, &link).unwrap();
            assert!(matches!(
                resolve(&claude, &by_session(&link, SID_A), &opens),
                Access::Ready(_)
            ));
        }
    }

    #[test]
    fn a_meta_override_may_only_tighten_the_policy() {
        let dir = TempDir::new("flight_meta_override");
        let ca = dir.join("a");
        let claude = settings(vec![account("a", &ca, CapturePolicy::Metadata)]);
        let dirs = vec![ca.display().to_string()];
        let loosen = json!({"flight": {"capturePolicy": "full"}});
        assert_eq!(
            policy_for_files(&claude, &dirs, Some(&loosen)).unwrap(),
            CapturePolicy::Metadata
        );
        let tighten = json!({"flight": {"capturePolicy": "off"}});
        assert_eq!(
            policy_for_files(&claude, &dirs, Some(&tighten)).unwrap(),
            CapturePolicy::Off
        );
        assert_eq!(
            policy_for_files(&claude, &dirs, None).unwrap(),
            CapturePolicy::Metadata
        );
    }

    #[test]
    fn a_ref_outside_the_session_set_is_refused_and_a_changed_byte_is_noticed() {
        let dir = TempDir::new("flight_ref_checks");
        let config = dir.join("a");
        let file = session(&config, SID_A, &[prompt("2026-10-22T10:00:00Z")]);
        let stranger = dir.join("secret.txt");
        std::fs::write(&stranger, "{\"x\":1}\n").unwrap();
        let claude = settings(vec![account("a", &config, CapturePolicy::Metadata)]);
        let q = by_session(&config, SID_A);
        let opens = Opens::default();
        let outside = SourceRef {
            file: stranger.display().to_string(),
            offset: 0,
            len: 7,
            sha256: sha256_hex(b"{\"x\":1}"),
        };
        assert!(read_content(&claude, &q, &outside, &opens)
            .unwrap_err()
            .contains("not part of"));
        let mut r = source_of(&file, 0);
        assert!(read_content(&claude, &q, &r, &opens).is_ok());
        r.sha256 = sha256_hex(b"other bytes");
        assert_eq!(
            read_content(&claude, &q, &r, &opens).unwrap_err(),
            "source changed"
        );
        let before = source_of(&file, 0);
        let mut bytes = std::fs::read(&file).unwrap();
        let at = bytes.len() - 3;
        bytes[at] ^= 1;
        std::fs::write(&file, bytes).unwrap();
        assert_eq!(
            read_content(&claude, &q, &before, &opens).unwrap_err(),
            "source changed"
        );
    }

    const SELFTEST: &str = "de6a6329-de0f-49da-960b-f50fed9236e4";
    const FAILING: &str = "5b0d6e0e-3a52-4c61-9d1f-7a2c4e8b9f10";

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

    /// An account folder `a` holding the recorded fixtures, and its settings.
    fn recorded_account(dir: &TempDir) -> (PathBuf, ClaudeSettings) {
        let config = dir.join("a");
        copy(&toy(), &config.join("projects/-work-toy"));
        let claude = settings(vec![account("a", &config, CapturePolicy::Metadata)]);
        (config, claude)
    }

    fn page(
        claude: &ClaudeSettings,
        config: &Path,
        sid: &str,
        req: &PageRequest,
        cache: &FlightCache,
    ) -> SessionView {
        session_view(
            claude,
            &by_session(config, sid),
            req,
            cache,
            &Opens::default(),
        )
        .unwrap()
    }

    fn ids(view: &SessionView) -> Vec<String> {
        view.page.events.iter().map(|e| e.id.clone()).collect()
    }

    #[test]
    fn paging_returns_every_event_once_in_time_file_offset_order() {
        let dir = TempDir::new("flight_paging");
        let (config, claude) = recorded_account(&dir);
        let cache = FlightCache::default();
        let all = page(
            &claude,
            &config,
            SELFTEST,
            &PageRequest {
                page_size: Some(1000),
                ..Default::default()
            },
            &cache,
        );
        assert!(all.page.next_cursor.is_none());
        let everything = ids(&all);
        assert!(everything.len() > 10);
        let mut paged = Vec::new();
        let mut cursor = None;
        let mut pages = 0;
        loop {
            let v = page(
                &claude,
                &config,
                SELFTEST,
                &PageRequest {
                    page_size: Some(4),
                    cursor: cursor.clone(),
                    ..Default::default()
                },
                &cache,
            );
            assert!(v.page.events.len() <= 4);
            paged.extend(ids(&v));
            pages += 1;
            cursor = v.page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert!(pages >= 3);
        assert_eq!(paged, everything, "each event once, same order");
        let keys: Vec<_> = all
            .page
            .events
            .iter()
            .map(|e| {
                (
                    e.at.clone(),
                    e.content.source.file.clone(),
                    e.content.source.offset,
                )
            })
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert_eq!(all.status, "ok");
        assert_eq!(all.agents.len(), 1);
        assert_eq!(all.page.events[0].content.policy, CapturePolicy::Metadata);
        assert!(session_view(
            &claude,
            &by_session(&config, SELFTEST),
            &PageRequest {
                cursor: Some("not a cursor".into()),
                ..Default::default()
            },
            &cache,
            &Opens::default()
        )
        .is_err());
    }

    #[test]
    fn lines_appended_between_pages_never_repeat_or_skip_an_event() {
        let dir = TempDir::new("flight_paging_append");
        let (config, claude) = recorded_account(&dir);
        let cache = FlightCache::default();
        let req = |cursor: Option<String>| PageRequest {
            page_size: Some(5),
            cursor,
            ..Default::default()
        };
        let first = page(&claude, &config, SELFTEST, &req(None), &cache);
        let before_all = {
            let v = page(
                &claude,
                &config,
                SELFTEST,
                &PageRequest {
                    page_size: Some(1000),
                    ..Default::default()
                },
                &cache,
            );
            ids(&v)
        };
        let cursor = first.page.next_cursor.clone().expect("more pages");
        // A main line and a sub-agent line arrive after the first page.
        let main = toy_in(&config, &format!("{SELFTEST}.jsonl"));
        let sub = toy_in(
            &config,
            &format!("{SELFTEST}/subagents/agent-a2f2a89910f21f43e.jsonl"),
        );
        let append = |path: &Path, line: Value| {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
            writeln!(f, "{line}").unwrap();
        };
        append(
            &main,
            tool_use(
                "2030-01-01T00:00:00Z",
                "mnew",
                "tnew",
                "Bash",
                json!({"command": "ls"}),
            ),
        );
        append(
            &sub,
            tool_use(
                "2030-01-01T00:00:01Z",
                "snew",
                "snew1",
                "Bash",
                json!({"command": "pwd"}),
            ),
        );
        let mut rest = Vec::new();
        let mut cursor = Some(cursor);
        while let Some(c) = cursor {
            let v = page(&claude, &config, SELFTEST, &req(Some(c)), &cache);
            rest.extend(ids(&v));
            cursor = v.page.next_cursor;
        }
        let mut seen = ids(&first);
        seen.extend(rest);
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), seen.len(), "no event twice");
        for id in &before_all {
            assert!(seen.contains(id), "{id} was skipped");
        }
        assert!(
            seen.len() >= before_all.len() + 4,
            "the new lines show up: {}",
            seen.len() - before_all.len()
        );
    }

    fn toy_in(config: &Path, rel: &str) -> PathBuf {
        config.join("projects/-work-toy").join(rel)
    }

    #[test]
    fn an_agent_filter_returns_only_that_agent_and_the_cache_follows_the_files() {
        let dir = TempDir::new("flight_agent_cache");
        let (config, claude) = recorded_account(&dir);
        let cache = FlightCache::default();
        let only = PageRequest {
            agent: Some("a2f2a89910f21f43e".into()),
            page_size: Some(1000),
            ..Default::default()
        };
        let v = page(&claude, &config, SELFTEST, &only, &cache);
        assert!(!v.page.events.is_empty());
        assert!(v
            .page
            .events
            .iter()
            .all(|e| e.agent.id == "a2f2a89910f21f43e"));
        assert_eq!((cache.hits(), cache.misses()), (0, 1));
        let _ = page(&claude, &config, SELFTEST, &PageRequest::default(), &cache);
        assert_eq!(
            (cache.hits(), cache.misses()),
            (1, 1),
            "unchanged files: a hit"
        );
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(toy_in(&config, &format!("{SELFTEST}.jsonl")))
            .unwrap();
        writeln!(f, "{}", prompt("2031-01-01T00:00:00Z")).unwrap();
        drop(f);
        let again = page(
            &claude,
            &config,
            SELFTEST,
            &PageRequest {
                page_size: Some(1000),
                ..Default::default()
            },
            &cache,
        );
        assert_eq!(cache.misses(), 2, "an append is a miss");
        assert_eq!(again.page.events.last().unwrap().kind, "turn.start");
    }

    /// A session file of about `bytes`, made of the failing fixture's lines.
    fn big_session(dir: &TempDir, bytes: usize) -> (PathBuf, ClaudeSettings) {
        let config = dir.join("a");
        let proj = config.join("projects/-work-toy");
        std::fs::create_dir_all(&proj).unwrap();
        let src = std::fs::read_to_string(toy().join(format!("{FAILING}.jsonl"))).unwrap();
        let mut text = String::new();
        while text.len() < bytes {
            text.push_str(&src);
        }
        std::fs::write(proj.join(format!("{FAILING}.jsonl")), text).unwrap();
        (
            config.clone(),
            settings(vec![account("a", &config, CapturePolicy::Metadata)]),
        )
    }

    #[test]
    fn the_first_page_of_a_2_mb_session_comes_back_within_a_second() {
        let dir = TempDir::new("flight_2mb");
        let (config, claude) = big_session(&dir, 2 * 1024 * 1024);
        let started = std::time::Instant::now();
        let v = page(
            &claude,
            &config,
            FAILING,
            &PageRequest::default(),
            &FlightCache::default(),
        );
        let took = started.elapsed();
        assert_eq!(v.page.events.len(), DEFAULT_PAGE_FOR_TEST);
        assert!(v.page.next_cursor.is_some());
        assert!(
            took < std::time::Duration::from_secs(1),
            "first page took {took:?}"
        );
    }

    const DEFAULT_PAGE_FOR_TEST: usize = 200;

    /// SM-2 on a 20 MB session. Too slow for the debug gate:
    /// `cargo test --release flight_first_page_20mb -- --ignored`
    #[test]
    #[ignore]
    fn flight_first_page_20mb() {
        let dir = TempDir::new("flight_20mb");
        let (config, claude) = big_session(&dir, 20 * 1024 * 1024);
        let started = std::time::Instant::now();
        let v = page(
            &claude,
            &config,
            FAILING,
            &PageRequest::default(),
            &FlightCache::default(),
        );
        let took = started.elapsed();
        assert_eq!(v.page.events.len(), DEFAULT_PAGE_FOR_TEST);
        assert!(
            took < std::time::Duration::from_secs(1),
            "first page took {took:?}"
        );
    }

    #[test]
    fn an_off_account_gives_the_policy_off_view_and_opens_nothing() {
        let dir = TempDir::new("flight_session_off");
        let config = dir.join("a");
        std::fs::create_dir_all(&config).unwrap();
        let claude = settings(vec![account("a", &config, CapturePolicy::Off)]);
        let opens = Opens::default();
        let v = session_view(
            &claude,
            &by_session(&config, SELFTEST),
            &PageRequest::default(),
            &FlightCache::default(),
            &opens,
        )
        .unwrap();
        assert_eq!((v.status.as_str(), v.policy), ("off", CapturePolicy::Off));
        assert!(v.page.events.is_empty() && v.agents.is_empty());
        assert_eq!(opens.count(), 0);
        let other = dir.join("elsewhere");
        let refused = session_view(
            &claude,
            &by_session(&other, SELFTEST),
            &PageRequest::default(),
            &FlightCache::default(),
            &opens,
        );
        assert_eq!(refused.unwrap_err(), UNKNOWN_FOLDER);
    }

    #[test]
    fn a_runs_page_names_the_run_as_its_trace() {
        let dir = TempDir::new("flight_run_trace");
        let (config, claude) = recorded_account(&dir);
        let run = dir.join("r-20261104-000000-abcd");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(
            run.join("app-usage.json"),
            json!({"schema": 1, "sessions": [{"sessionId": FAILING, "configDir": config.display().to_string()}]}).to_string(),
        )
        .unwrap();
        let q = Query {
            run_dir: Some(run),
            ..Default::default()
        };
        let v = session_view(
            &claude,
            &q,
            &PageRequest::default(),
            &FlightCache::default(),
            &Opens::default(),
        )
        .unwrap();
        assert_eq!(v.trace, "r-20261104-000000-abcd");
        assert!(v
            .page
            .events
            .iter()
            .all(|e| e.trace == "r-20261104-000000-abcd"));
        assert_eq!(v.failures.len(), 2);
    }

    #[test]
    fn the_exact_pr_constant_is_tied_to_the_plugin_version_and_the_skill() {
        let plugin = Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugin");
        let version = std::fs::read_to_string(plugin.join("VERSION")).unwrap();
        let (from, now, before) = (
            version_triple(EXACT_PR_FROM).unwrap(),
            version_triple(&version).unwrap(),
            version_triple("0.6.0").unwrap(),
        );
        assert!(
            before < from,
            "EXACT_PR_FROM is after the version without the rule"
        );
        assert!(
            from <= now,
            "plugin/VERSION {version} is older than EXACT_PR_FROM"
        );
        let skill = std::fs::read_to_string(plugin.join("skills/eagle-eye/SKILL.md")).unwrap();
        assert!(
            skill.contains("`PR<n> `"),
            "SKILL.md lost the description rule"
        );
        for agent in ["dcv2:implementer", "dcv2:pr-reviewer"] {
            assert!(skill.contains(agent));
        }
        assert!(version_triple("0.7.0") == Some((0, 7, 0)));
        assert!(version_triple("0.7").is_none() && version_triple("x").is_none());
        assert!(version_triple("0.7.0-beta.1") == Some((0, 7, 0)));
        assert!(
            version_triple("0.10.0") > version_triple("0.9.9"),
            "numeric, not text"
        );
    }

    #[test]
    fn a_run_is_exact_from_the_plugin_version_in_its_meta() {
        let dir = TempDir::new("flight_exact_pr");
        let run = dir.join("r-1");
        std::fs::create_dir_all(&run).unwrap();
        assert!(!exact_pr(&run), "no meta.json");
        for (meta, want) in [
            (json!({"pluginVersion": "0.7.0"}), true),
            (json!({"pluginVersion": "0.7.1"}), true),
            (json!({"pluginVersion": "1.0.0"}), true),
            (json!({"pluginVersion": "0.10.0"}), true),
            (json!({"pluginVersion": "0.6.0"}), false),
            (json!({"pluginVersion": "0.6.9"}), false),
            (json!({"pluginVersion": "x"}), false),
            (json!({"pluginVersion": 7}), false),
            (json!({}), false),
        ] {
            std::fs::write(run.join("meta.json"), meta.to_string()).unwrap();
            assert_eq!(exact_pr(&run), want, "{meta}");
        }
    }

    #[test]
    fn the_cache_key_holds_the_exact_flag() {
        let dir = TempDir::new("flight_exact_cache");
        let (config, claude) = recorded_account(&dir);
        let cache = FlightCache::default();
        let run = dir.join("r-1");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(
            run.join("app-usage.json"),
            json!({"schema": 1, "sessions": [{"sessionId": SELFTEST, "configDir": config.display().to_string()}]}).to_string(),
        )
        .unwrap();
        let q = Query {
            run_dir: Some(run.clone()),
            ..Default::default()
        };
        let view = |c: &FlightCache| {
            session_view(&claude, &q, &PageRequest::default(), c, &Opens::default()).unwrap()
        };
        std::fs::write(
            run.join("meta.json"),
            json!({"pluginVersion": "0.6.0"}).to_string(),
        )
        .unwrap();
        view(&cache);
        view(&cache);
        assert_eq!((cache.hits(), cache.misses()), (1, 1));
        std::fs::write(
            run.join("meta.json"),
            json!({"pluginVersion": "0.7.0"}).to_string(),
        )
        .unwrap();
        view(&cache);
        assert_eq!(cache.misses(), 2, "the other flag is a different parse");
    }

    // ---- a run's own events (S6): the wave 1 fixture ----

    const WAVE1_SID: &str = "aaaa1111-2222-4333-8444-555555555555";
    const CARGO: &str = "cargo test --manifest-path src-tauri/Cargo.toml";

    /// The wave 1 fixture in a temp folder with one account `a` of `policy`.
    fn wave1(name: &str, policy: CapturePolicy) -> (TempDir, PathBuf, ClaudeSettings) {
        let dir = TempDir::new(name);
        let (run, config) = wave1_run(&dir);
        let claude = settings(vec![account("a", &config, policy)]);
        (dir, run, claude)
    }

    fn run_query(run: &Path) -> Query {
        Query {
            run_dir: Some(run.to_path_buf()),
            ..Default::default()
        }
    }

    fn run_view(claude: &ClaudeSettings, run: &Path, req: &PageRequest) -> SessionView {
        session_view(
            claude,
            &run_query(run),
            req,
            &FlightCache::default(),
            &Opens::default(),
        )
        .unwrap()
    }

    fn all_events(claude: &ClaudeSettings, run: &Path) -> SessionView {
        run_view(
            claude,
            run,
            &PageRequest {
                page_size: Some(1000),
                ..Default::default()
            },
        )
    }

    fn event_by(view: &SessionView, f: impl Fn(&Event) -> bool) -> &Event {
        view.page.events.iter().find(|e| f(e)).expect("event")
    }

    fn wave1_payload(policy: CapturePolicy, encoding: export::Encoding) -> export::Payload {
        let (_dir, run, claude) = wave1("export_golden", policy);
        let (_, input) = export_input(
            &claude,
            &run_query(&run),
            &FlightCache::default(),
            &Opens::default(),
            false,
        )
        .unwrap()
        .expect("not off");
        export::build(&input, export::ExportPolicy::Metadata, encoding).unwrap()
    }

    #[test]
    fn the_wave1_trace_exports_byte_for_byte_to_the_golden_files() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fixtures/flight");
        let json = wave1_payload(CapturePolicy::Metadata, export::Encoding::Json);
        let pb = wave1_payload(CapturePolicy::Metadata, export::Encoding::Protobuf);
        let hex: String = pb
            .bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
            + "\n";
        if std::env::var("FLIGHT_WRITE_GOLDEN").is_ok() {
            std::fs::write(dir.join("otlp-wave1.json"), &json.bytes).unwrap();
            std::fs::write(dir.join("otlp-wave1.pb.hex"), &hex).unwrap();
        }
        assert_eq!(
            json.bytes,
            std::fs::read(dir.join("otlp-wave1.json")).unwrap(),
            "regenerate with FLIGHT_WRITE_GOLDEN=1"
        );
        assert_eq!(
            hex,
            std::fs::read_to_string(dir.join("otlp-wave1.pb.hex")).unwrap(),
            "regenerate with FLIGHT_WRITE_GOLDEN=1"
        );
        // The Trace has agents, tools and the run's orchestration.
        assert!(
            json.spans >= 5 && json.events >= 5,
            "{} {}",
            json.spans,
            json.events
        );
        let text = String::from_utf8(json.bytes).unwrap();
        for name in [
            "invoke_agent",
            "execute_tool",
            "agentflight.run",
            "agentflight.check.end",
        ] {
            assert!(text.contains(name), "{name}");
        }
        assert!(!text.contains("gen_ai.tool.call.arguments"));
    }

    fn preview_of(
        claude: &ClaudeSettings,
        run: &Path,
        state: &ExportState,
        policy: export::ExportPolicy,
        confirm: bool,
    ) -> Result<ExportPreview, String> {
        export_preview(
            claude,
            &run_query(run),
            &FlightCache::default(),
            &Opens::default(),
            state,
            &ExportRequest {
                policy,
                encoding: export::Encoding::Protobuf,
                confirm_full: confirm,
            },
            "http://127.0.0.1:6006/v1/traces",
        )
    }

    #[test]
    fn export_is_refused_for_policy_off_and_full_is_clamped_without_its_conditions() {
        let state = ExportState::default();
        let (_d, run, claude) = wave1("export_off", CapturePolicy::Off);
        assert_eq!(
            preview_of(&claude, &run, &state, export::ExportPolicy::Metadata, false).unwrap_err(),
            EXPORT_OFF
        );
        // Capture policy metadata: `full` is clamped.
        let (_d, run, claude) = wave1("export_meta", CapturePolicy::Metadata);
        let p = preview_of(&claude, &run, &state, export::ExportPolicy::Full, true).unwrap();
        assert_eq!(p.policy, export::ExportPolicy::Metadata);
        assert!(!p.warnings.is_empty());
        // Capture policy full but no confirmation: clamped. With it: full, and the text is in the bytes.
        let (_d, run, claude) = wave1("export_full", CapturePolicy::Full);
        let p = preview_of(&claude, &run, &state, export::ExportPolicy::Full, false).unwrap();
        assert_eq!(p.policy, export::ExportPolicy::Metadata);
        let p = preview_of(&claude, &run, &state, export::ExportPolicy::Full, true).unwrap();
        assert_eq!(p.policy, export::ExportPolicy::Full);
        assert!(p
            .fields
            .iter()
            .any(|f| f.field == "gen_ai.tool.call.arguments"));
        assert!(p.loopback && p.bytes > 0 && p.spans > 0);
    }

    #[test]
    fn an_unknown_digest_is_refused_by_send_and_save_with_the_one_message() {
        let state = ExportState::default();
        let agent = export_agent();
        let err = export_send(&state, &agent, "nope", "http://127.0.0.1:1/v1/traces").unwrap_err();
        assert_eq!(err, CHANGED_SINCE_PREVIEW);
        let dir = TempDir::new("export_unknown");
        let err = export_save(&state, "nope", &dir.join("x.pb")).unwrap_err();
        assert_eq!(err, CHANGED_SINCE_PREVIEW);
        assert!(!dir.join("x.pb").exists());
    }

    #[test]
    fn only_http_and_https_with_a_host_are_endpoints_and_loopback_is_told_apart() {
        for bad in [
            "",
            "  ",
            "ftp://x/y",
            "file:///etc/passwd",
            "http://",
            "localhost:6006",
            "javascript:1",
        ] {
            assert!(check_endpoint(bad).is_err(), "{bad}");
        }
        assert_eq!(
            check_endpoint(" https://example.com/v1/traces ").unwrap(),
            "https://example.com/v1/traces"
        );
        assert!(is_loopback("http://127.0.0.1:6006/v1/traces"));
        assert!(is_loopback("http://localhost:4318/v1/traces"));
        assert!(is_loopback("http://[::1]:4318/x"));
        assert!(!is_loopback("http://10.0.0.5:6006/v1/traces"));
        assert!(!is_loopback("https://example.com/x"));
    }

    #[test]
    fn the_export_agent_has_no_proxy_and_follows_no_redirect() {
        let agent = export_agent();
        assert!(agent.config().proxy().is_none());
        assert_eq!(agent.config().max_redirects(), 0);
    }

    /// A one-shot server: answers the first request with `reply`, returns its body, and says whether a
    /// second request ever arrived.
    fn serve_once(reply: &'static str) -> (String, std::thread::JoinHandle<(Vec<u8>, bool)>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            let (head_end, length) = loop {
                let n = stream.read(&mut chunk).unwrap();
                buf.extend_from_slice(&chunk[..n]);
                if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..at]).to_lowercase();
                    let length = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    break (at + 4, length);
                }
                assert!(n > 0, "closed before the head");
            };
            while buf.len() < head_end + length {
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0, "closed before the body");
                buf.extend_from_slice(&chunk[..n]);
            }
            stream.write_all(reply.as_bytes()).unwrap();
            drop(stream);
            listener.set_nonblocking(true).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(300));
            let second = listener.accept().is_ok();
            (buf[head_end..head_end + length].to_vec(), second)
        });
        (format!("http://{addr}/v1/traces"), handle)
    }

    #[test]
    fn send_posts_exactly_the_previewed_bytes_even_after_new_events_and_follows_no_redirect() {
        let state = ExportState::default();
        let (_d, run, claude) = wave1("export_send", CapturePolicy::Metadata);
        let p = preview_of(&claude, &run, &state, export::ExportPolicy::Metadata, false).unwrap();
        let want = state
            .previewed
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .bytes
            .clone();
        assert_eq!(want.len(), p.bytes);
        // The Trace grows after the Preview.
        let log = run.join("log.md");
        let mut text = std::fs::read_to_string(&log).unwrap();
        text.push_str("2026-11-05T11:59:00Z building PR1 later line\n");
        std::fs::write(&log, text).unwrap();
        let (url, server) =
            serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let sent = export_send(&state, &export_agent(), &p.digest, &url).unwrap();
        assert_eq!((sent.status, sent.spans), (200, p.spans));
        let (body, second) = server.join().unwrap();
        assert_eq!(body, *want, "the previewed bytes, not a re-read Trace");
        assert!(!second);
        // A redirect is an error and is not followed.
        let (url, server) = serve_once(
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let err = export_send(&state, &export_agent(), &p.digest, &url).unwrap_err();
        assert!(err.contains("302"), "{err}");
        let (_, second) = server.join().unwrap();
        assert!(!second, "no second request");
        // A non-2xx reply carries its status.
        let (url, server) = serve_once(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let err = export_send(&state, &export_agent(), &p.digest, &url).unwrap_err();
        assert!(err.contains("500"), "{err}");
        server.join().unwrap();
        // A non-http endpoint never reaches the network.
        assert!(export_send(&state, &export_agent(), &p.digest, "ftp://127.0.0.1/x").is_err());
    }

    #[test]
    fn save_writes_exactly_the_previewed_bytes_and_never_through_a_link() {
        let state = ExportState::default();
        let (dir, run, claude) = wave1("export_save", CapturePolicy::Metadata);
        let p = preview_of(&claude, &run, &state, export::ExportPolicy::Metadata, false).unwrap();
        let want = state
            .previewed
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .bytes
            .clone();
        let out = dir.join("trace.otlp.pb");
        std::fs::write(&out, vec![7u8; want.len() + 50]).unwrap();
        export_save(&state, &p.digest, &out).unwrap();
        assert_eq!(
            std::fs::read(&out).unwrap(),
            *want,
            "truncated and rewritten"
        );
        assert!(export_save(&state, &p.digest, Path::new("relative.pb")).is_err());
        #[cfg(unix)]
        {
            let target = dir.join("victim.txt");
            std::fs::write(&target, "keep").unwrap();
            let link = dir.join("link.pb");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(export_save(&state, &p.digest, &link).is_err());
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
        }
    }

    /// A run on disk over a toy git repo: PR1 built on the repo's commit `base`, in a linked worktree, with a
    /// transcript of edits of `src/a.txt` (and a `rm -rf` Bash call).
    struct Toy {
        _dir: TempDir,
        run: PathBuf,
        repo: PathBuf,
        wt: PathBuf,
        claude: ClaudeSettings,
        sentinel: PathBuf,
        file: String,
    }

    const TOY_SID: &str = "bbbb1111-2222-4333-8444-555555555555";
    const TOY_RUN: &str = "r-20261105-120000-ab12";

    fn toy_replay(policy: CapturePolicy) -> Toy {
        use crate::run::git::testing::{repo_at, run};
        let dir = TempDir::new("replay_toy");
        let config = dir.join("config");
        let repo = dir.join("ws/toy");
        repo_at(&repo);
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/a.txt"), "alpha\nbeta\n").unwrap();
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-q", "-m", "add a"]);
        let base = crate::run::git::git(&repo, &["rev-parse", "HEAD"]).unwrap();
        let wt = crate::run::janitor::expected_worktree(&repo, TOY_RUN, "PR1").unwrap();
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "dcv2/pr1",
                wt.to_str().unwrap(),
            ],
        );
        let wt = wt.canonicalize().unwrap();
        let file = wt.join("src/a.txt").display().to_string();
        let sentinel = dir.join("sentinel");
        std::fs::create_dir_all(&sentinel).unwrap();
        let runs = dir.join("runs");
        let run_dir = runs.join(TOY_RUN);
        std::fs::create_dir_all(&run_dir).unwrap();
        let write =
            |name: &str, v: Value| std::fs::write(run_dir.join(name), v.to_string()).unwrap();
        write(
            "status.json",
            json!({"schema":1,"runId":TOY_RUN,"stage":"building","prs":[
                {"id":"PR1","repoPath":repo,"worktree":wt,"baseCommit":base,"state":"building"}]}),
        );
        write(
            "repos.json",
            json!({"schema":1,"repos":[{"name":"toy","path":repo}]}),
        );
        write(
            "meta.json",
            json!({"schema":1,"account":{"id":"a","name":"A","configDir":config}}),
        );
        write(
            "app-usage.json",
            json!({"schema":1,"sessions":[{"sessionId":TOY_SID,"configDir":config}]}),
        );
        let t = |n: u32| format!("2026-11-05T12:00:{n:02}Z");
        let edit =
            |old: &str, new: &str| json!({"file_path": file, "old_string": old, "new_string": new});
        let lines = vec![
            prompt(&t(0)),
            tool_use(&t(1), "m1", "t1", "Edit", edit("alpha", "ALPHA")),
            tool_result(&t(2), "t1", "ok", false),
            tool_use(&t(3), "m2", "t2", "Edit", edit("gamma", "x")),
            tool_result(&t(4), "t2", "String to replace not found", true),
            tool_use(
                &t(5),
                "m3",
                "t3",
                "MultiEdit",
                json!({"file_path": file, "edits": [
                    {"old_string": "beta", "new_string": "BETA"},
                    {"old_string": "BETA", "new_string": "BETA2"}]}),
            ),
            tool_result(&t(6), "t3", "ok", false),
            tool_use(
                &t(7),
                "m4",
                "t4",
                "Bash",
                json!({"command": format!("rm -rf {}", sentinel.display())}),
            ),
            tool_result(&t(8), "t4", "ok", false),
            tool_use(&t(9), "m5", "t5", "Edit", edit("nope", "x")),
            tool_result(&t(10), "t5", "ok", false),
            tool_use(
                &t(11),
                "m6",
                "t6",
                "Write",
                json!({"file_path": dir.join("elsewhere.txt"), "content": "x"}),
            ),
            tool_result(&t(12), "t6", "ok", false),
            tool_use(
                &t(13),
                "m7",
                "t7",
                "Write",
                json!({"file_path": format!("{}/../out.txt", wt.display()), "content": "x"}),
            ),
            tool_result(&t(14), "t7", "ok", false),
        ];
        let proj = config.join("projects/-work-toy");
        std::fs::create_dir_all(&proj).unwrap();
        let text: String = lines.iter().map(|l| l.to_string() + "\n").collect();
        std::fs::write(proj.join(format!("{TOY_SID}.jsonl")), text).unwrap();
        let claude = settings(vec![account("a", &config, policy)]);
        Toy {
            _dir: dir,
            run: run_dir,
            repo,
            wt,
            claude,
            sentinel,
            file,
        }
    }

    fn replay_of(toy: &Toy, event: &str) -> ReplayFile {
        replay_file(
            &toy.claude,
            &run_query(&toy.run),
            &FlightCache::default(),
            &Opens::default(),
            event,
        )
        .unwrap()
    }

    fn toy_track(toy: &Toy) -> ReplayTrack {
        replay_track(
            &toy.claude,
            &run_query(&toy.run),
            &FlightCache::default(),
            &Opens::default(),
        )
        .unwrap()
    }

    /// The file.change event ids in order (t1, t2, t3, t5, t6, t7).
    fn file_changes(track: &ReplayTrack) -> Vec<String> {
        track
            .entries
            .iter()
            .filter(|e| e.kind == "file.change")
            .map(|e| e.id.clone())
            .collect()
    }

    fn check_toy_states(toy: &Toy) {
        let ids = file_changes(&toy_track(toy));
        assert_eq!(ids.len(), 6, "{ids:?}");
        let first = replay_of(toy, &ids[0]);
        assert_eq!(first.status, "ok", "{first:?}");
        assert_eq!(first.text.as_deref(), Some("ALPHA\nbeta\n"));
        assert_eq!((first.edit, first.edits), (Some(1), 3 + 0));
        assert_eq!(first.path.as_deref(), Some("src/a.txt"));
        assert_eq!(first.pr.as_deref(), Some("PR1"));
        // The failed edit is skipped: the file is as it was, and it says why.
        let failed = replay_of(toy, &ids[1]);
        assert_eq!(failed.text.as_deref(), Some("ALPHA\nbeta\n"));
        assert_eq!(failed.edit, None);
        assert!(failed.reason.unwrap().contains("failed"));
        // The multi-edit applies in order.
        let multi = replay_of(toy, &ids[2]);
        assert_eq!(multi.text.as_deref(), Some("ALPHA\nBETA2\n"));
        assert_eq!(multi.edit, Some(2));
        assert_eq!(multi.stopped, None);
        // An edit whose old text is absent stops the file at the state before it.
        let stopped = replay_of(toy, &ids[3]);
        assert_eq!(stopped.text.as_deref(), Some("ALPHA\nBETA2\n"));
        let stop = stopped.stopped.expect("stopped");
        assert_eq!(stop.at, 3);
        assert!(stop.reason.contains("not in the file"));
        // A file outside every PR worktree, and a path with `..`.
        let outside = replay_of(toy, &ids[4]);
        assert_eq!(outside.status, "unavailable");
        assert!(outside
            .reason
            .unwrap()
            .contains("outside every PR worktree"));
        assert!(outside.text.is_none());
        let dots = replay_of(toy, &ids[5]);
        assert_eq!(dots.status, "unavailable");
        assert!(dots.text.is_none());
    }

    #[test]
    fn a_file_is_its_base_commit_plus_the_recorded_successful_edits() {
        let toy = toy_replay(CapturePolicy::Metadata);
        check_toy_states(&toy);
    }

    #[test]
    fn a_run_whose_worktree_was_removed_still_replays_to_the_same_text() {
        let toy = toy_replay(CapturePolicy::Metadata);
        crate::run::git::testing::run(
            &toy.repo,
            &["worktree", "remove", "--force", toy.wt.to_str().unwrap()],
        );
        assert!(!toy.wt.exists());
        check_toy_states(&toy);
    }

    #[test]
    fn replay_runs_nothing_it_read_from_a_transcript() {
        let toy = toy_replay(CapturePolicy::Full);
        let track = toy_track(&toy);
        assert!(track
            .entries
            .iter()
            .any(|e| e.name.as_deref() == Some("Bash")));
        for id in track.entries.iter().map(|e| e.id.clone()) {
            replay_of(&toy, &id);
        }
        // Export reads the same Events and must not run them either.
        export_input(
            &toy.claude,
            &run_query(&toy.run),
            &FlightCache::default(),
            &Opens::default(),
            true,
        )
        .unwrap();
        assert!(toy.sentinel.is_dir(), "the recorded `rm -rf` was never run");
        // And no replay or export source names a way to run one.
        for file in ["replay.rs", "export.rs"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/flight")
                .join(file);
            let live =
                crate::test_support::without_test_items(&std::fs::read_to_string(path).unwrap());
            for banned in ["std::process", "Command", "quiet_command", "run_one"] {
                assert!(!live.contains(banned), "{file} names {banned}");
            }
        }
    }

    #[test]
    fn policy_off_returns_no_file_text_and_no_track() {
        let toy = toy_replay(CapturePolicy::Off);
        let track = toy_track(&toy);
        assert_eq!((track.status.as_str(), track.entries.len()), ("off", 0));
        let file = replay_of(&toy, "ev_anything");
        assert_eq!(file.status, "off");
        assert!(file.text.is_none());
    }

    #[test]
    fn the_track_has_eight_keys_and_no_path() {
        let toy = toy_replay(CapturePolicy::Metadata);
        let track = toy_track(&toy);
        let value = serde_json::to_value(&track).unwrap();
        let entries = value["entries"].as_array().unwrap();
        assert!(entries.len() > 10);
        let mut files = 0;
        for entry in entries {
            let mut keys: Vec<&str> = entry
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort();
            assert_eq!(
                keys,
                [
                    "at",
                    "checkSource",
                    "file",
                    "id",
                    "kind",
                    "name",
                    "pr",
                    "status"
                ]
            );
            assert!(entry["file"].is_number() || entry["file"].is_null());
            if entry["kind"] == "file.change" {
                files += 1;
                assert_eq!(entry["file"], 1);
            }
        }
        assert_eq!(files, 6);
        let text = value.to_string();
        for path in [
            toy.file.as_str(),
            toy.wt.to_str().unwrap(),
            toy.repo.to_str().unwrap(),
            "src/a.txt",
        ] {
            assert!(!text.contains(path), "the track names {path}");
        }
    }

    #[test]
    fn replayed_text_is_redacted_and_cut_at_256_kib() {
        let toy = toy_replay(CapturePolicy::Metadata);
        // Add a huge Write to the transcript.
        let log = toy
            ._dir
            .join(format!("config/projects/-work-toy/{TOY_SID}.jsonl"));
        let mut text = std::fs::read_to_string(&log).unwrap();
        let big = "just a plain line of text\n".repeat(12 * 1024);
        text.push_str(
            &tool_use(
                "2026-11-05T11:59:00Z",
                "m9",
                "t9",
                "Write",
                json!({"file_path": toy.file, "content": big}),
            )
            .to_string(),
        );
        text.push('\n');
        text.push_str(&tool_result("2026-11-05T11:59:01Z", "t9", "ok", false).to_string());
        text.push('\n');
        std::fs::write(&log, text).unwrap();
        let ids = file_changes(&toy_track(&toy));
        let last = replay_of(&toy, &ids[0]);
        assert_eq!(last.status, "ok", "{last:?}");
        assert!(
            last.truncated,
            "{:?} {:?} {:?} len {:?}",
            last.status,
            last.reason,
            last.stopped,
            last.text.as_ref().map(|t| t.len())
        );
        assert!(last.text.unwrap().len() <= MAX_TEXT);
    }

    #[test]
    fn a_runs_view_interleaves_orchestration_events_with_the_transcript_in_time_order() {
        let (_d, run, claude) = wave1("flight_wave1_view", CapturePolicy::Full);
        let v = all_events(&claude, &run);
        let events = &v.page.events;
        let lanes: std::collections::HashSet<&str> =
            events.iter().map(|e| e.agent.id.as_str()).collect();
        assert!(lanes.contains("orchestration") && lanes.contains("main"));
        assert!(lanes.contains("a1pr2impl0000000001") && lanes.contains("a2pr2review00000002"));
        let keys: Vec<_> = events
            .iter()
            .map(|e| {
                (
                    e.at.clone(),
                    e.content.source.file.clone(),
                    e.content.source.offset,
                )
            })
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "one keyed order across both sources");
        // A transcript event sits between two orchestration events of the same run.
        let pos = |f: &dyn Fn(&Event) -> bool| events.iter().position(f).unwrap();
        let spawn = pos(&|e| e.kind == "agent.spawn" && e.at.starts_with("2026-11-05T10:08"));
        let building = pos(&|e| e.kind == kind::PR_STATE && e.at.starts_with("2026-11-05T10:07"));
        let reviewing = pos(&|e| e.kind == kind::PR_STATE && e.at.starts_with("2026-11-05T10:20"));
        assert!(building < spawn && spawn < reviewing);
        assert!(events.iter().all(|e| e.trace == "r-20261105-100000-fa17"));
        assert_eq!(v.policy, CapturePolicy::Full);
    }

    #[test]
    fn paging_a_runs_view_over_the_boundaries_drops_and_repeats_nothing() {
        let (_d, run, claude) = wave1("flight_wave1_paging", CapturePolicy::Full);
        let everything = ids(&all_events(&claude, &run));
        assert!(everything.len() > 40, "{}", everything.len());
        for size in [1usize, 3, 7, 16] {
            let cache = FlightCache::default();
            let mut paged = Vec::new();
            let mut cursor = None;
            loop {
                let v = session_view(
                    &claude,
                    &run_query(&run),
                    &PageRequest {
                        page_size: Some(size),
                        cursor: cursor.clone(),
                        ..Default::default()
                    },
                    &cache,
                    &Opens::default(),
                )
                .unwrap();
                paged.extend(ids(&v));
                cursor = v.page.next_cursor;
                if cursor.is_none() {
                    break;
                }
            }
            assert_eq!(paged, everything, "page size {size}");
        }
        // An agent filter still pages by the same keys.
        let only = run_view(
            &claude,
            &run,
            &PageRequest {
                agent: Some("orchestration".into()),
                page_size: Some(1000),
                ..Default::default()
            },
        );
        assert!(only
            .page
            .events
            .iter()
            .all(|e| e.agent.id == "orchestration"));
        assert!(only.page.events.len() < everything.len());
    }

    #[test]
    fn a_session_query_gets_no_run_events() {
        let (_d, run, claude) = wave1("flight_wave1_session_only", CapturePolicy::Full);
        let config = PathBuf::from(claude.accounts[0].config_dir.clone().unwrap());
        let v = page(
            &claude,
            &config,
            WAVE1_SID,
            &PageRequest {
                page_size: Some(1000),
                ..Default::default()
            },
            &FlightCache::default(),
        );
        assert!(!v.page.events.is_empty());
        assert!(v.page.events.iter().all(|e| e.provider.name != "dcv2"));
        assert!(v.phases.values().all(|p| p.stage.is_none()));
        assert!(run.is_dir());
    }

    #[test]
    fn off_opens_no_run_file_and_no_account_refuses() {
        let (dir, run, _claude) = wave1("flight_wave1_off", CapturePolicy::Off);
        let (_, config) = (dir.path(), dir.join("config"));
        let off = settings(vec![account("a", &config, CapturePolicy::Off)]);
        let opens = Opens::default();
        let v = session_view(
            &off,
            &run_query(&run),
            &PageRequest::default(),
            &FlightCache::default(),
            &opens,
        )
        .unwrap();
        assert_eq!(v.status, "off");
        assert!(v.page.events.is_empty());
        assert_eq!(opens.count(), 0);
        // No sessions recorded: the account of meta.json decides.
        std::fs::remove_file(run.join("app-usage.json")).unwrap();
        let opens = Opens::default();
        assert!(matches!(
            resolve(&off, &run_query(&run), &opens),
            Access::Off
        ));
        let out = read_content(
            &off,
            &run_query(&run),
            &SourceRef {
                file: run.join("log.md").display().to_string(),
                offset: 0,
                len: 5,
                sha256: "x".into(),
            },
            &opens,
        )
        .unwrap();
        assert!(out.text.is_empty() && out.policy == CapturePolicy::Off);
        assert_eq!(opens.count(), 0, "not one run file was opened");
        // No sessions and no account in meta.json: refused, nothing opened.
        std::fs::write(run.join("meta.json"), json!({"schema": 1}).to_string()).unwrap();
        let opens = Opens::default();
        assert!(matches!(
            resolve(&off, &run_query(&run), &opens),
            Access::Refused(why) if why == NO_RUN_ACCOUNT
        ));
        assert!(session_view(
            &off,
            &run_query(&run),
            &PageRequest::default(),
            &FlightCache::default(),
            &opens
        )
        .is_err());
        assert_eq!(opens.count(), 0);
        // A folder of meta.json that matches no account refuses like an unmatched session folder.
        std::fs::write(
            run.join("meta.json"),
            json!({"account": {"configDir": dir.join("elsewhere").display().to_string()}})
                .to_string(),
        )
        .unwrap();
        assert!(matches!(
            resolve(&off, &run_query(&run), &opens),
            Access::Refused(why) if why == UNKNOWN_FOLDER
        ));
        assert_eq!(opens.count(), 0);
    }

    #[test]
    fn a_run_with_no_recorded_sessions_follows_its_meta_account_policy() {
        for (policy, events) in [
            (CapturePolicy::Full, true),
            (CapturePolicy::Redacted, true),
            (CapturePolicy::Metadata, true),
        ] {
            let (_d, run, claude) = wave1("flight_wave1_no_sessions", policy);
            std::fs::remove_file(run.join("app-usage.json")).unwrap();
            let v = all_events(&claude, &run);
            assert_eq!(v.status, "ok");
            assert_eq!(v.policy, policy);
            assert_eq!(
                v.page.events.iter().any(|e| e.agent.id == "orchestration"),
                events
            );
            assert!(
                v.page.events.iter().all(|e| e.agent.id == "orchestration"),
                "no recorded session: only the run's own events"
            );
            // Content of a run file opens through flight_content with the same redaction.
            let log = v
                .page
                .events
                .iter()
                .find(|e| e.content.source.file.ends_with("log.md"))
                .unwrap();
            let out = read_content(
                &claude,
                &run_query(&run),
                &log.content.source,
                &Opens::default(),
            )
            .unwrap();
            assert_eq!(out.policy, policy);
            assert!(!out.text.is_empty());
        }
    }

    #[test]
    fn the_guard_block_is_one_failure_carrying_the_rule_id_and_the_other_line_is_a_row() {
        let (_d, run, claude) = wave1("flight_wave1_guard", CapturePolicy::Full);
        let v = all_events(&claude, &run);
        let blocked: Vec<&Event> = v
            .page
            .events
            .iter()
            .filter(|e| e.kind == kind::TOOL_BLOCKED)
            .collect();
        assert_eq!(
            blocked.len(),
            2,
            "the transcript block (with the rule) and the unmatched line"
        );
        let merged = blocked
            .iter()
            .find(|e| e.span.as_deref() == Some("toolu_guard01"))
            .unwrap();
        assert_eq!(merged.attrs["rule"], "run-file-write");
        assert_eq!(
            merged.provider.name, "claude-code",
            "the transcript row carries the rule"
        );
        let unmatched = blocked.iter().find(|e| e.provider.name == "dcv2").unwrap();
        assert_eq!(unmatched.attrs["rule"], "git-push");
        let guard: Vec<&Failure> = v
            .failures
            .iter()
            .filter(|f| f.kind == kind::TOOL_BLOCKED)
            .collect();
        assert_eq!(guard.len(), 1);
        assert_eq!(guard[0].id, merged.id);
        assert_eq!(guard[0].rule.as_deref(), Some("run-file-write"));
        assert_eq!(guard[0].derived.label, derive::HOOK_DENIAL_GUARD);
    }

    #[test]
    fn a_failed_app_check_and_a_failed_stop_check_are_failures() {
        let (_d, run, claude) = wave1("flight_wave1_check_failures", CapturePolicy::Full);
        let v = all_events(&claude, &run);
        let checks: Vec<&Failure> = v
            .failures
            .iter()
            .filter(|f| f.kind == kind::CHECK_END)
            .collect();
        // The app's re-run and the stop-check failed; the skill's own check (status.json) passed.
        assert_eq!(checks.len(), 2);
        assert!(checks.iter().all(|f| f.derived.label == derive::CHECK_FAIL));
        assert!(checks.iter().all(|f| f.name.as_deref() == Some(CARGO)));
        let sorted: Vec<&String> = v.failures.iter().map(|f| &f.at).collect();
        assert!(sorted.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn run_file_content_is_redacted_and_an_unlisted_path_is_refused() {
        let (dir, run, claude) = wave1("flight_wave1_content", CapturePolicy::Full);
        let v = all_events(&claude, &run);
        let opens = Opens::default();
        for (suffix, kept) in [
            ("reviews/PR2-round1.md", "add() subtracts"),
            ("logs/PR2-app-check.txt", "test result: FAILED"),
        ] {
            let ev = v
                .page
                .events
                .iter()
                .find(|e| e.content.source.file.ends_with(suffix))
                .unwrap_or_else(|| panic!("{suffix}"));
            let out = read_content(&claude, &run_query(&run), &ev.content.source, &opens).unwrap();
            assert!(out.text.contains(kept), "{}", out.text);
            assert_eq!(out.redactions, 1, "{}", out.text);
            assert!(!out.text.contains("Hunter2"), "{}", out.text);
            assert!(out.text.contains("[redacted: "), "{}", out.text);
        }
        // A changed byte is noticed.
        let ev = v
            .page
            .events
            .iter()
            .find(|e| e.content.source.file.ends_with("reviews/PR2-round1.md"))
            .unwrap();
        let mut stale = ev.content.source.clone();
        stale.sha256 = sha256_hex(b"other");
        assert_eq!(
            read_content(&claude, &run_query(&run), &stale, &opens).unwrap_err(),
            "source changed"
        );
        // Not in the adapter's list: meta.json, a stranger file, a path that climbs out of the run.
        let secret = dir.join("secret.txt");
        std::fs::write(&secret, "x").unwrap();
        for file in [
            run.join("meta.json"),
            run.join("repos.json"),
            secret.clone(),
            run.join("reviews/../meta.json"),
        ] {
            let source = SourceRef {
                file: file.display().to_string(),
                offset: 0,
                len: 1,
                sha256: sha256_hex(b"x"),
            };
            assert_eq!(
                read_content(&claude, &run_query(&run), &source, &opens).unwrap_err(),
                "that file is not part of this run",
                "{}",
                file.display()
            );
        }
    }

    #[test]
    fn a_whole_file_ref_is_cut_at_the_text_limit_and_a_huge_one_is_refused() {
        let (_d, run, claude) = wave1("flight_wave1_big_content", CapturePolicy::Full);
        let big = "line of output\n".repeat(30_000);
        std::fs::write(run.join("logs/PR2-app-check.txt"), &big).unwrap();
        let v = all_events(&claude, &run);
        let ev = v
            .page
            .events
            .iter()
            .find(|e| e.content.source.file.ends_with("logs/PR2-app-check.txt"))
            .unwrap();
        let out = read_content(
            &claude,
            &run_query(&run),
            &ev.content.source,
            &Opens::default(),
        )
        .unwrap();
        assert!(out.text.len() <= MAX_TEXT + 20 && out.text.ends_with("[truncated]\n"));
        let mut huge = ev.content.source.clone();
        huge.len = dcv2_run::MAX_FILE + 1;
        assert!(
            read_content(&claude, &run_query(&run), &huge, &Opens::default())
                .unwrap_err()
                .contains("too large")
        );
    }

    #[test]
    fn the_adapters_approval_wait_minutes_equal_the_timeline_rule() {
        let (_d, run, claude) = wave1("flight_wave1_wait_minutes", CapturePolicy::Full);
        let v = all_events(&claude, &run);
        let at = |k: &str| -> i64 {
            let e = event_by(&v, |e| e.kind == k);
            chrono::DateTime::parse_from_rfc3339(&e.at)
                .unwrap()
                .timestamp_millis()
        };
        let waited_ms = at(kind::APPROVAL_DECISION) - at(kind::APPROVAL_WAIT);
        let log = std::fs::read_to_string(run.join("log.md")).unwrap();
        let (wait, _) = crate::run::timeline::wait_and_span(&log);
        assert_eq!(wait, Some((waited_ms as f64 / 6_000.0).round() / 10.0));
        assert_eq!(wait, Some(3.0));
    }

    fn phase<'a>(v: &'a SessionView, e: &Event) -> &'a Phase {
        &v.phases[&e.id]
    }

    #[test]
    fn every_event_has_a_phase_from_the_stage_and_the_pr_lines() {
        let (_d, run, claude) = wave1("flight_wave1_phases", CapturePolicy::Full);
        let v = all_events(&claude, &run);
        assert_eq!(v.phases.len(), v.page.events.len(), "one phase per event");
        let named = |source: &str, status: &str| {
            event_by(&v, |e| {
                e.kind == kind::CHECK_END
                    && e.attrs.get("source").and_then(|s| s.as_str()) == Some(source)
                    && e.status.as_deref() == Some(status)
            })
        };
        let app = phase(&v, named("app", "fail"));
        assert_eq!(app.pr_state.as_deref(), Some("reviewing"));
        assert_eq!(app.label.as_deref(), Some("exact:log-line"));
        assert_eq!(app.stage.as_deref(), Some("building"));
        assert_eq!(
            phase(&v, named("stop-check", "fail")).pr_state.as_deref(),
            Some("fixing")
        );
        assert_eq!(
            phase(&v, named("skill", "pass")).pr_state.as_deref(),
            Some("reviewing")
        );
        let reviewer = v
            .page
            .events
            .iter()
            .filter(|e| e.agent.id == "a2pr2review00000002");
        for e in reviewer {
            assert_eq!(
                phase(&v, e).pr_state.as_deref(),
                Some("reviewing"),
                "{}",
                e.kind
            );
        }
        let implementer = event_by(&v, |e| {
            e.agent.id == "a1pr2impl0000000001" && e.kind == kind::TOOL_START
        });
        assert_eq!(phase(&v, implementer).pr_state.as_deref(), Some("building"));
        let change = event_by(&v, |e| {
            e.kind == kind::STAGE_CHANGE && e.name.as_deref() == Some("plan-approval")
        });
        assert_eq!(phase(&v, change).stage.as_deref(), Some("plan-approval"));
        assert_eq!(
            phase(&v, change).pr_state,
            None,
            "a stage change has no PR state"
        );
        let main = event_by(&v, |e| e.agent.id == "main" && e.kind == kind::TURN_START);
        assert_eq!(phase(&v, main).pr_state, None, "the main session has no PR");
    }

    #[test]
    fn a_run_without_pr_lines_falls_back_to_status_json_and_says_heuristic() {
        let (_d, run, claude) = wave1("flight_wave1_phases_old", CapturePolicy::Full);
        let log = std::fs::read_to_string(run.join("log.md")).unwrap();
        let old: String = log
            .lines()
            .filter(|l| !l.contains(" pr PR2 "))
            .map(|l| format!("{l}\n"))
            .collect();
        std::fs::write(run.join("log.md"), old).unwrap();
        let v = all_events(&claude, &run);
        let app = event_by(&v, |e| {
            e.kind == kind::CHECK_END && e.attrs["source"] == "app"
        });
        let p = phase(&v, app);
        assert_eq!(
            p.pr_state.as_deref(),
            Some("fixing"),
            "status.json's state now"
        );
        assert_eq!(p.label.as_deref(), Some("heuristic:status-now"));
    }

    #[test]
    fn exact_and_heuristic_pr_labels_follow_the_runs_plugin_version() {
        let (_d, run, claude) = wave1("flight_wave1_labels", CapturePolicy::Full);
        let labels = |v: &SessionView| -> Vec<String> {
            fn walk(n: &AgentNode, out: &mut Vec<String>) {
                out.extend(n.pr_derived.iter().map(|d| d.label.clone()));
                n.children.iter().for_each(|c| walk(c, out));
            }
            let mut out = Vec::new();
            v.agents.iter().for_each(|a| walk(a, &mut out));
            out
        };
        let v = all_events(&claude, &run);
        assert_eq!(labels(&v), ["exact:pr-description", "exact:pr-description"]);
        let mut meta: Value =
            serde_json::from_str(&std::fs::read_to_string(run.join("meta.json")).unwrap()).unwrap();
        meta["pluginVersion"] = json!("0.6.0");
        std::fs::write(run.join("meta.json"), meta.to_string()).unwrap();
        let v = all_events(&claude, &run);
        assert_eq!(
            labels(&v),
            [
                "heuristic:description-prefix",
                "heuristic:description-prefix"
            ]
        );
    }

    #[test]
    fn the_run_events_come_from_the_cache_until_a_listed_file_changes() {
        let (_d, run, claude) = wave1("flight_wave1_run_cache", CapturePolicy::Full);
        let cache = FlightCache::default();
        let view = || {
            session_view(
                &claude,
                &run_query(&run),
                &PageRequest::default(),
                &cache,
                &Opens::default(),
            )
            .unwrap()
        };
        view();
        view();
        assert_eq!(cache.run_misses(), 1);
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(run.join("log.md"))
            .unwrap();
        writeln!(f, "2026-11-05T11:00:00Z building later line").unwrap();
        drop(f);
        view();
        assert_eq!(cache.run_misses(), 2);
    }

    #[test]
    fn the_first_page_can_start_at_a_focused_event() {
        let (_d, run, claude) = wave1("flight_wave1_focus", CapturePolicy::Full);
        let everything = ids(&all_events(&claude, &run));
        let target = &everything[everything.len() - 3];
        let v = run_view(
            &claude,
            &run,
            &PageRequest {
                page_size: Some(10),
                focus: Some(target.clone()),
                ..Default::default()
            },
        );
        let got = ids(&v);
        assert!(got.contains(target));
        let at = everything.iter().position(|i| i == target).unwrap();
        assert_eq!(got[0], everything[at / 10 * 10]);
        let unknown = run_view(
            &claude,
            &run,
            &PageRequest {
                page_size: Some(10),
                focus: Some("ev_nope".into()),
                ..Default::default()
            },
        );
        assert_eq!(
            ids(&unknown)[0],
            everything[0],
            "an unknown focus starts at the top"
        );
    }

    // ---- the trace index (S7) ----

    /// Every row that matters for equality, in a fixed order.
    fn dump(conn: &rusqlite::Connection) -> Vec<String> {
        let mut out = Vec::new();
        let mut stmt = conn
            .prepare(
                "SELECT run_id, id, session, agent_id, parent, role, pr, pr_exact, kind, status, name, at, stage,
                        pr_state, phase_label, failure, paths, src_file, src_offset, src_len, src_sha256, event_json
                 FROM events ORDER BY run_id, id",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |r| {
                (0..22)
                    .map(|i| {
                        r.get::<_, rusqlite::types::Value>(i)
                            .map(|v| format!("{v:?}"))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(|v| v.join("|"))
            })
            .unwrap();
        out.extend(rows.map(|r| r.unwrap()));
        let mut stmt = conn
            .prepare("SELECT span, run_id, agent, name, start_at, end_at, status, duration_ms FROM spans ORDER BY run_id, span, agent")
            .unwrap();
        let rows = stmt
            .query_map([], |r| {
                (0..8)
                    .map(|i| {
                        r.get::<_, rusqlite::types::Value>(i)
                            .map(|v| format!("{v:?}"))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(|v| format!("span {}", v.join("|")))
            })
            .unwrap();
        out.extend(rows.map(|r| r.unwrap()));
        out
    }

    fn pass_over(
        conn: &mut rusqlite::Connection,
        claude: &ClaudeSettings,
        run: &Path,
    ) -> index::PassStats {
        index_pass(conn, claude, &[run.to_path_buf()], &Limits::default()).unwrap()
    }

    #[test]
    fn a_two_pass_index_equals_a_one_pass_index_on_the_same_final_files() {
        let dir = TempDir::new("flight_index_two_pass");
        let (run, config) = wave1_run(&dir);
        let claude = settings(vec![account("a", &config, CapturePolicy::Full)]);
        let proj = config.join("projects/-work-toy");
        let main = proj.join(format!("{WAVE1_SID}.jsonl"));
        let subs = proj.join(WAVE1_SID).join("subagents");
        let imp = subs.join("agent-a1pr2impl0000000001.jsonl");
        let rev = subs.join("agent-a2pr2review00000002.jsonl");
        let rev_meta = subs.join("agent-a2pr2review00000002.meta.json");
        let lines = |p: &Path| -> Vec<String> {
            std::fs::read_to_string(p)
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect()
        };
        let join = |l: &[String]| l.iter().map(|x| format!("{x}\n")).collect::<String>();
        // The main log gets a repeated assistant message (streamed twice), as Claude writes them.
        let mut full_main = lines(&main);
        full_main.insert(2, full_main[1].clone());
        let full_imp = lines(&imp);
        let full_rev = std::fs::read_to_string(&rev).unwrap();
        let full_rev_meta = std::fs::read_to_string(&rev_meta).unwrap();

        // Pass 1: cut between the spawn tool_use and its result, part-way through the sub-agent
        // file (a tool_use with no result), and the reviewer not started.
        std::fs::write(&main, join(&full_main[..3])).unwrap();
        std::fs::write(&imp, join(&full_imp[..2])).unwrap();
        std::fs::remove_file(&rev).unwrap();
        std::fs::remove_file(&rev_meta).unwrap();
        let db = dir.join("flight.db");
        let mut conn = index::open_writer(&db).unwrap();
        let first = pass_over(&mut conn, &claude, &run);
        assert_eq!((first.rebuilt, first.sessions_parsed), (1, 1));
        let partial = dump(&conn);
        assert!(!partial.is_empty());

        // Pass 2: the rest arrives.
        std::fs::write(&main, join(&full_main)).unwrap();
        std::fs::write(&imp, join(&full_imp)).unwrap();
        std::fs::write(&rev, &full_rev).unwrap();
        std::fs::write(&rev_meta, &full_rev_meta).unwrap();
        let second = pass_over(&mut conn, &claude, &run);
        assert_eq!((second.rebuilt, second.sessions_parsed), (1, 1));
        let two_pass = dump(&conn);
        assert_ne!(two_pass, partial);

        // One pass over the final files, into a fresh database.
        let mut fresh = index::open_writer(&dir.join("fresh.db")).unwrap();
        pass_over(&mut fresh, &claude, &run);
        let one_pass = dump(&fresh);
        assert_eq!(two_pass.len(), one_pass.len());
        assert_eq!(
            two_pass, one_pass,
            "ids, kinds, names, parents, statuses, failures, phases and refs"
        );
        let kinds: String = two_pass.join("\n");
        assert!(kinds.contains("check.end") && kinds.contains("agent.spawn"));

        // Deleting flight.db and running again gives the same results.
        drop(conn);
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", db.display()));
        }
        let mut again = index::open_writer(&db).unwrap();
        pass_over(&mut again, &claude, &run);
        assert_eq!(dump(&again), one_pass);
    }

    #[test]
    fn a_run_is_indexed_once_per_change_and_not_parsed_again_when_nothing_changed() {
        let dir = TempDir::new("flight_index_unchanged");
        let (run, config) = wave1_run(&dir);
        let claude = settings(vec![account("a", &config, CapturePolicy::Full)]);
        let mut conn = index::open_writer(&dir.join("flight.db")).unwrap();
        let first = pass_over(&mut conn, &claude, &run);
        assert_eq!((first.rebuilt, first.skipped), (1, 0));
        let again = pass_over(&mut conn, &claude, &run);
        assert_eq!(
            (again.rebuilt, again.skipped, again.sessions_parsed),
            (0, 1, 0)
        );
        // A run file changing re-derives the run but not the unchanged transcript.
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(run.join("log.md"))
            .unwrap();
        writeln!(f, "2026-11-05T11:00:00Z building later line").unwrap();
        drop(f);
        let third = pass_over(&mut conn, &claude, &run);
        assert_eq!(
            (third.rebuilt, third.sessions_parsed),
            (1, 0),
            "stored Events reused for the session"
        );
        let cols = dump(&conn);
        let transcript = cols
            .iter()
            .filter(|r| r.contains("a1pr2impl0000000001"))
            .count();
        assert!(transcript > 0);
        // pr_exact: the PR2 agents are exact in a 0.7.0 run.
        let exact: i64 = conn
            .query_row("SELECT COUNT(*) FROM events WHERE role = 'implementer' AND pr = 'PR2' AND pr_exact = 1", [], |r| r.get(0))
            .unwrap();
        assert!(exact > 0);
    }

    #[test]
    fn a_run_with_no_recorded_sessions_follows_its_meta_account_in_the_index() {
        for (policy, indexed) in [
            (CapturePolicy::Off, false),
            (CapturePolicy::Full, true),
            (CapturePolicy::Metadata, true),
        ] {
            let dir = TempDir::new("flight_index_meta_account");
            let (run, config) = wave1_run(&dir);
            std::fs::remove_file(run.join("app-usage.json")).unwrap();
            let claude = settings(vec![account("a", &config, policy)]);
            let mut conn = index::open_writer(&dir.join("flight.db")).unwrap();
            let stats = pass_over(&mut conn, &claude, &run);
            let events: i64 = conn
                .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
                .unwrap();
            assert_eq!(events > 0, indexed, "{policy:?}");
            assert_eq!(stats.excluded, usize::from(!indexed));
            let excluded: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM runs WHERE excluded_off = 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(excluded, i64::from(!indexed));
            if indexed {
                // Only the run's own events: no session is recorded.
                let others: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM events WHERE agent_id <> 'orchestration'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(others, 0);
            }
        }
        // No account at all: refused, no rows, not counted as excluded.
        let dir = TempDir::new("flight_index_no_account");
        let (run, config) = wave1_run(&dir);
        std::fs::remove_file(run.join("app-usage.json")).unwrap();
        std::fs::write(run.join("meta.json"), json!({"schema": 1}).to_string()).unwrap();
        let claude = settings(vec![account("a", &config, CapturePolicy::Full)]);
        let mut conn = index::open_writer(&dir.join("flight.db")).unwrap();
        let stats = pass_over(&mut conn, &claude, &run);
        assert_eq!((stats.excluded, stats.rebuilt), (0, 0));
        let runs: i64 = conn
            .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(runs, 0);
    }

    #[test]
    fn only_the_index_thread_reaches_the_index_pass() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(&src, &mut files);
        for file in files {
            let name = file.file_name().unwrap().to_string_lossy().to_string();
            let text = std::fs::read_to_string(&file).unwrap();
            let live = crate::test_support::without_test_items(&text);
            for call in ["index_pass(", "spawn_index(", "app_run_dirs("] {
                let allowed = matches!(name.as_str(), "host.rs" | "lib.rs");
                assert!(
                    allowed || !live.contains(call),
                    "{} calls {call}",
                    file.display()
                );
            }
            // The index is opened for writing in one place, and only host.rs names the writer.
            if live.contains("open_writer(") {
                assert!(
                    matches!(name.as_str(), "host.rs" | "index.rs"),
                    "{} opens the index writer",
                    file.display()
                );
            }
        }
        let lib = std::fs::read_to_string(src.join("lib.rs")).unwrap();
        assert!(lib.contains("flight::host::spawn_index(app.handle())"));
    }

    // ---- flight_search (S8) ----

    fn review_search() -> SearchFilter {
        SearchFilter {
            name_contains: Some("cargo test".into()),
            status: vec!["fail".into()],
            in_review: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_real_pass_over_the_fixture_then_the_search_finds_the_run_and_its_first_match() {
        let dir = TempDir::new("flight_search_e2e");
        let (run, config) = wave1_run(&dir);
        let claude = settings(vec![account("a", &config, CapturePolicy::Full)]);
        let mut conn = index::open_writer(&dir.join(INDEX_FILE)).unwrap();
        index_pass(
            &mut conn,
            &claude,
            std::slice::from_ref(&run),
            &Limits::default(),
        )
        .unwrap();
        let view = all_events(&claude, &run);
        let check = |source: &str, status: &str| -> String {
            event_by(&view, |e| {
                e.kind == kind::CHECK_END
                    && e.attrs.get("source").and_then(|s| s.as_str()) == Some(source)
                    && e.status.as_deref() == Some(status)
            })
            .id
            .clone()
        };
        let out = search_index(dir.path(), &review_search()).unwrap();
        assert_eq!(out.runs.len(), 1, "{out:?}");
        let hit = &out.runs[0];
        assert_eq!(hit.run_id, "r-20261105-100000-fa17");
        assert_eq!(hit.run_dir, run.display().to_string());
        assert_eq!(
            hit.matches, 1,
            "the app check; the stop-check failed in `fixing`"
        );
        assert_eq!(hit.first_event_id, check("app", "fail"));
        assert_eq!(out.excluded_off, 0);
        assert_eq!(
            out.progress,
            Counts {
                current: 1,
                total: 1
            }
        );
        // The stop-check fail is in `fixing`, and only it.
        let fixing = SearchFilter {
            name_contains: Some("cargo test".into()),
            status: vec!["fail".into()],
            pr_state: Some("fixing".into()),
            ..Default::default()
        };
        let out = search_index(dir.path(), &fixing).unwrap();
        assert_eq!(out.runs.len(), 1);
        assert_eq!(out.runs[0].matches, 1);
        assert_eq!(out.runs[0].first_event_id, check("stop-check", "fail"));
        // Roles and stages reach the transcript Events too.
        let reviewer = SearchFilter {
            pr_role: Some("pr-reviewer".into()),
            ..Default::default()
        };
        assert!(search_index(dir.path(), &reviewer).unwrap().runs[0].matches > 0);
        let blocked = SearchFilter {
            kind: Some("tool.blocked".into()),
            ..Default::default()
        };
        assert_eq!(
            search_index(dir.path(), &blocked).unwrap().runs[0].matches,
            2
        );
    }

    #[test]
    fn without_a_database_the_search_is_empty_and_progress_counts_the_run_folders() {
        let dir = TempDir::new("flight_search_missing");
        for id in ["r-20261105-000001-aaaa", "r-20261105-000002-bbbb"] {
            std::fs::create_dir_all(dir.join("runs").join(id)).unwrap();
        }
        let out = search_index(dir.path(), &SearchFilter::default()).unwrap();
        assert!(out.runs.is_empty());
        assert_eq!(out.excluded_off, 0);
        assert_eq!(
            out.progress,
            Counts {
                current: 0,
                total: 2
            }
        );
        assert!(
            !dir.join(INDEX_FILE).exists(),
            "a search creates no database"
        );
    }

    #[test]
    fn an_off_run_is_counted_in_the_search_and_never_listed() {
        let dir = TempDir::new("flight_search_off");
        let (run, config) = wave1_run(&dir);
        let off = settings(vec![account("a", &config, CapturePolicy::Off)]);
        let mut conn = index::open_writer(&dir.join(INDEX_FILE)).unwrap();
        index_pass(
            &mut conn,
            &off,
            std::slice::from_ref(&run),
            &Limits::default(),
        )
        .unwrap();
        let out = search_index(dir.path(), &SearchFilter::default()).unwrap();
        assert!(out.runs.is_empty());
        assert_eq!(out.excluded_off, 1);
    }

    #[test]
    fn a_gone_transcript_is_a_notice_not_an_error() {
        let dir = TempDir::new("flight_gone");
        let config = dir.join("a");
        std::fs::create_dir_all(&config).unwrap();
        let claude = settings(vec![account("a", &config, CapturePolicy::Metadata)]);
        let Access::Ready(set) = resolve(&claude, &by_session(&config, SID_A), &Opens::default())
        else {
            panic!("ready");
        };
        assert!(set.sessions.is_empty());
        assert!(set.notices[0].starts_with("transcript gone"));
    }

    // ---- the run snapshot ----

    fn keeping(mut accounts: Vec<ClaudeAccount>) -> ClaudeSettings {
        for a in &mut accounts {
            a.keep_run_traces = true;
        }
        settings(accounts)
    }

    /// The record of a finished run: both sessions listed, `outcome` and the status stamp it was computed from.
    fn finish(run: &Path, config: &Path, sessions: &[&str], outcome: Option<&str>, updated: &str) {
        let list: Vec<Value> = sessions
            .iter()
            .map(|s| json!({"sessionId": s, "configDir": config.display().to_string()}))
            .collect();
        std::fs::write(
            run.join("app-usage.json"),
            json!({"schema": 1, "sessions": list, "outcome": outcome, "statusUpdatedAt": updated})
                .to_string(),
        )
        .unwrap();
    }

    fn manifest_of(run: &Path) -> snapshot::Manifest {
        serde_json::from_str(&std::fs::read_to_string(run.join("flight/manifest.json")).unwrap())
            .unwrap()
    }

    fn event_ids(run: &Path) -> Vec<String> {
        std::fs::read_to_string(run.join("flight/events.jsonl"))
            .unwrap()
            .lines()
            .map(|l| {
                serde_json::from_str::<super::super::schema::Event>(l)
                    .unwrap()
                    .id
            })
            .collect()
    }

    fn entries_named(run: &Path, prefix: &str) -> Vec<String> {
        std::fs::read_dir(run)
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| n.starts_with(prefix))
            .collect()
    }

    #[test]
    fn a_finished_run_whose_account_keeps_traces_gets_one_snapshot_per_record_and_metadata_holds_no_content(
    ) {
        let dir = TempDir::new("flight_snapshot_basic");
        let (run, config) = wave1_run(&dir);
        let mut state = SnapshotState::default();
        let dirs = std::slice::from_ref(&run);
        let claude = keeping(vec![account("a", &config, CapturePolicy::Metadata)]);
        // Not finished: no outcome in the record.
        assert_eq!(snapshot_pass(&claude, dirs, &mut state), 0);
        assert!(!run.join("flight").exists());
        finish(&run, &config, &[WAVE1_SID], Some("done"), "t1");
        // Finished, but the account does not keep traces.
        let off_switch = settings(vec![account("a", &config, CapturePolicy::Metadata)]);
        assert_eq!(snapshot_pass(&off_switch, dirs, &mut state), 0);
        assert!(!run.join("flight").exists());
        // The `off` policy opens nothing, whatever the switch says.
        let off = keeping(vec![account("a", &config, CapturePolicy::Off)]);
        assert_eq!(snapshot_pass(&off, dirs, &mut state), 0);
        assert!(!run.join("flight").exists());

        assert_eq!(snapshot_pass(&claude, dirs, &mut state), 1);
        let manifest = manifest_of(&run);
        assert_eq!(
            (
                manifest.outcome.as_str(),
                manifest.record_status_updated_at.as_deref()
            ),
            ("done", Some("t1"))
        );
        assert_eq!(manifest.policy, CapturePolicy::Metadata);
        assert!(manifest.events > 0 && manifest.events == event_ids(&run).len());
        assert_eq!(manifest.sessions[0].id, WAVE1_SID);
        assert_eq!(manifest.content_lines, 0);
        assert!(
            !run.join("flight/content.jsonl").exists(),
            "metadata keeps no content"
        );
        assert!(!manifest.sources.is_empty());
        assert!(entries_named(&run, SNAPSHOT_TMP).is_empty());
        // Once per terminal record: the same record writes nothing again.
        let created = manifest.created_at.clone();
        assert_eq!(snapshot_pass(&claude, dirs, &mut state), 0);
        assert_eq!(manifest_of(&run).created_at, created);
        // The snapshot is a regular tree of three files at most.
        let names = entries_named(&run.join("flight"), "");
        assert_eq!(names.len(), 2, "{names:?}");
    }

    #[test]
    fn a_pass_reads_only_the_manifest_of_a_current_snapshot_and_keeps_one_whose_events_are_unreadable(
    ) {
        let dir = TempDir::new("flight_snapshot_manifest_only");
        let (run, config) = wave1_run(&dir);
        finish(&run, &config, &[WAVE1_SID], Some("done"), "t1");
        let claude = keeping(vec![account("a", &config, CapturePolicy::Metadata)]);
        let dirs = std::slice::from_ref(&run);
        let mut state = SnapshotState::default();
        assert_eq!(snapshot_pass(&claude, dirs, &mut state), 1);
        // Damage the events: a current snapshot is skipped without reading them.
        let events = run.join("flight/events.jsonl");
        std::fs::write(&events, [0xff, 0xfe]).unwrap();
        assert_eq!(snapshot_pass(&claude, dirs, &mut state), 0);
        assert!(state.tried.is_empty());
        // A new record with unreadable old events keeps the old snapshot and fails with a reason.
        finish(&run, &config, &[WAVE1_SID], Some("done"), "t2");
        assert_eq!(snapshot_pass(&claude, dirs, &mut state), 0);
        assert_eq!(std::fs::read(&events).unwrap(), vec![0xff, 0xfe]);
        assert_eq!(
            manifest_of(&run).record_status_updated_at.as_deref(),
            Some("t1")
        );
        assert_eq!(state.tried.get(&run).map(|t| t.0), Some(1));
    }

    #[test]
    fn redacted_and_full_snapshots_keep_redacted_content_by_source_line() {
        for policy in [CapturePolicy::Redacted, CapturePolicy::Full] {
            let dir = TempDir::new("flight_snapshot_content");
            let (run, config) = wave1_run(&dir);
            finish(&run, &config, &[WAVE1_SID], Some("failed"), "t1");
            let claude = keeping(vec![account("a", &config, policy)]);
            assert_eq!(
                snapshot_pass(
                    &claude,
                    std::slice::from_ref(&run),
                    &mut SnapshotState::default()
                ),
                1
            );
            let manifest = manifest_of(&run);
            assert_eq!(manifest.outcome, "failed");
            assert!(manifest.content_lines > 0);
            let content = std::fs::read_to_string(run.join("flight/content.jsonl")).unwrap();
            assert_eq!(content.lines().count(), manifest.content_lines);
            // Every content line is found again by a source ref of an Event.
            let events = std::fs::read_to_string(run.join("flight/events.jsonl")).unwrap();
            for line in content.lines() {
                let c: snapshot::ContentLine = serde_json::from_str(line).unwrap();
                assert!(events.contains(&c.sha256), "{}", c.sha256);
            }
        }
    }

    #[test]
    fn a_failed_then_resumed_run_gets_a_new_snapshot_that_keeps_the_gone_sessions_events() {
        let dir = TempDir::new("flight_snapshot_resume");
        let (run, config) = wave1_run(&dir);
        let claude = keeping(vec![account("a", &config, CapturePolicy::Full)]);
        let dirs = std::slice::from_ref(&run);
        let mut state = SnapshotState::default();
        finish(&run, &config, &[WAVE1_SID], Some("failed"), "t1");
        assert_eq!(snapshot_pass(&claude, dirs, &mut state), 1);
        let before = event_ids(&run);
        let content_before = manifest_of(&run).content_lines;
        assert!(!before.is_empty());

        // Claude deletes the first session's transcript; the run resumes with a second session and ends again.
        let proj = config.join("projects/-work-toy");
        std::fs::remove_file(proj.join(format!("{WAVE1_SID}.jsonl"))).unwrap();
        std::fs::remove_dir_all(proj.join(WAVE1_SID)).unwrap();
        session(
            &config,
            SID_B,
            &[
                prompt("2026-11-06T10:00:00Z"),
                tool_use(
                    "2026-11-06T10:00:01Z",
                    "m1",
                    "t1",
                    "Bash",
                    json!({"command": "ls"}),
                ),
            ],
        );
        finish(&run, &config, &[WAVE1_SID, SID_B], Some("done"), "t2");
        assert_eq!(snapshot_pass(&claude, dirs, &mut state), 1);
        let manifest = manifest_of(&run);
        assert_eq!(
            (
                manifest.outcome.as_str(),
                manifest.record_status_updated_at.as_deref()
            ),
            ("done", Some("t2"))
        );
        let after = event_ids(&run);
        assert!(
            before.iter().all(|id| after.contains(id)),
            "the gone session's Events are kept with their ids"
        );
        assert!(
            after.len() > before.len(),
            "and the new session's are added"
        );
        let ids: Vec<&str> = manifest.sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, [WAVE1_SID, SID_B]);
        assert!(
            manifest.content_lines >= content_before,
            "the gone session's content is carried"
        );
        assert!(
            entries_named(&run, SNAPSHOT_TMP).is_empty(),
            "the old snapshot was removed after the swap"
        );
    }

    #[test]
    fn leftovers_are_removed_and_a_crash_between_the_two_renames_is_restored() {
        let dir = TempDir::new("flight_snapshot_crash");
        let (run, config) = wave1_run(&dir);
        let claude = keeping(vec![account("a", &config, CapturePolicy::Metadata)]);
        let dirs = std::slice::from_ref(&run);
        let mut state = SnapshotState::default();
        finish(&run, &config, &[WAVE1_SID], Some("done"), "t1");
        assert_eq!(snapshot_pass(&claude, dirs, &mut state), 1);
        let ids = event_ids(&run);
        // A crashed build, and a crash after the old snapshot was set aside.
        std::fs::create_dir(run.join(".flight.tmp-dead0001")).unwrap();
        std::fs::write(run.join(".flight.tmp-dead0001/events.jsonl"), "x").unwrap();
        std::fs::rename(run.join("flight"), run.join(".flight.tmp-beef0002-old")).unwrap();
        assert_eq!(
            snapshot_pass(&claude, dirs, &mut state),
            0,
            "the restored snapshot is the current record's"
        );
        assert_eq!(event_ids(&run), ids, "nothing was lost");
        assert!(
            entries_named(&run, SNAPSHOT_TMP).is_empty(),
            "{:?}",
            entries_named(&run, SNAPSHOT_TMP)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_write_leaves_the_previous_snapshot_and_is_logged_once_per_reason_and_tried_three_times(
    ) {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("flight_snapshot_fail");
        let (run, config) = wave1_run(&dir);
        let claude = keeping(vec![account("a", &config, CapturePolicy::Metadata)]);
        let dirs = std::slice::from_ref(&run);
        let mut state = SnapshotState::default();
        finish(&run, &config, &[WAVE1_SID], Some("done"), "t1");
        assert_eq!(snapshot_pass(&claude, dirs, &mut state), 1);
        let log_before = std::fs::read_to_string(run.join("log.md")).unwrap();

        // The record changes, and the run folder cannot be written to: every pass fails the same way.
        finish(&run, &config, &[WAVE1_SID], Some("failed"), "t2");
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o555)).unwrap();
        // Root ignores the mode: skip then.
        if std::fs::create_dir(run.join(".probe")).is_ok() {
            std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755)).unwrap();
            let _ = std::fs::remove_dir(run.join(".probe"));
            return;
        }
        for _ in 0..5 {
            assert_eq!(snapshot_pass(&claude, dirs, &mut state), 0);
        }
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            state.attempts(&run),
            SNAPSHOT_ATTEMPTS,
            "no more than 3 tries per app session"
        );
        assert_eq!(
            manifest_of(&run).outcome,
            "done",
            "the previous snapshot stays"
        );
        let log = std::fs::read_to_string(run.join("log.md")).unwrap();
        let added = log.strip_prefix(&log_before).unwrap_or(&log);
        assert_eq!(
            added.matches("app: trace snapshot failed:").count(),
            1,
            "one line for the same reason: {added}"
        );
        // A new app session (a fresh state) tries again and succeeds.
        assert_eq!(
            snapshot_pass(&claude, dirs, &mut SnapshotState::default()),
            1
        );
        assert_eq!(manifest_of(&run).outcome, "failed");
        let log = std::fs::read_to_string(run.join("log.md")).unwrap();
        assert!(
            log.trim_end().ends_with("app: trace snapshot written"),
            "the panel can tell the failure is over: {log}"
        );
    }

    #[test]
    fn a_flight_that_is_not_a_folder_is_a_failure_and_nothing_follows_a_link() {
        let dir = TempDir::new("flight_snapshot_link");
        let (run, config) = wave1_run(&dir);
        let claude = keeping(vec![account("a", &config, CapturePolicy::Metadata)]);
        finish(&run, &config, &[WAVE1_SID], Some("done"), "t1");
        std::fs::write(run.join("flight"), "i am a file").unwrap();
        let mut state = SnapshotState::default();
        assert_eq!(
            snapshot_pass(&claude, std::slice::from_ref(&run), &mut state),
            0
        );
        assert_eq!(
            std::fs::read_to_string(run.join("flight")).unwrap(),
            "i am a file"
        );
        assert_eq!(state.attempts(&run), 1);
        assert!(std::fs::read_to_string(run.join("log.md"))
            .unwrap()
            .contains("app: trace snapshot failed: flight is not a folder"));
    }

    // ---- reading a snapshot (CAP-11) ----

    fn read_all(claude: &ClaudeSettings, run: &Path) -> SessionView {
        let query = Query {
            run_dir: Some(run.to_path_buf()),
            ..Default::default()
        };
        let request = PageRequest {
            page_size: Some(1000),
            ..Default::default()
        };
        session_view(
            claude,
            &query,
            &request,
            &FlightCache::default(),
            &Opens::default(),
        )
        .unwrap()
    }

    #[test]
    #[ignore = "regenerates fixtures/run/flight-wave2-snapshot from the wave 1 run (run it with --ignored)"]
    fn regenerate_the_wave2_snapshot_fixture() {
        let dir = TempDir::new("flight_regen_wave2");
        let (run, config) = wave1_run(&dir);
        finish(
            &run,
            &config,
            &[WAVE1_SID],
            Some("done"),
            "2026-11-05T11:00:00Z",
        );
        let claude = keeping(vec![account("a", &config, CapturePolicy::Full)]);
        assert_eq!(
            snapshot_pass(
                &claude,
                std::slice::from_ref(&run),
                &mut SnapshotState::default()
            ),
            1
        );
        let out =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/run/flight-wave2-snapshot");
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(out.join("flight")).unwrap();
        let shown = config.display().to_string().replace('\\', "/");
        let canonical = config
            .canonicalize()
            .unwrap()
            .display()
            .to_string()
            .replace('\\', "/");
        for name in [
            "meta.json",
            "app-usage.json",
            "log.md",
            "status.json",
            "flight/manifest.json",
            "flight/events.jsonl",
            "flight/content.jsonl",
        ] {
            let text = std::fs::read_to_string(run.join(name)).unwrap();
            let text = text
                .replace(&canonical, "@CONFIG@")
                .replace(&shown, "@CONFIG@");
            std::fs::write(out.join(name), text).unwrap();
        }
    }

    #[test]
    fn a_snapshot_page_has_the_same_events_as_the_live_one_once_the_transcripts_are_gone() {
        let dir = TempDir::new("flight_snapshot_same");
        let (run, config) = wave1_run(&dir);
        let claude = keeping(vec![account("a", &config, CapturePolicy::Full)]);
        finish(&run, &config, &[WAVE1_SID], Some("done"), "t1");
        let live = read_all(&claude, &run);
        assert!(live.snapshot.is_none(), "a live transcript is read live");
        assert_eq!(
            snapshot_pass(
                &claude,
                std::slice::from_ref(&run),
                &mut SnapshotState::default()
            ),
            1
        );
        // A session whose transcript still exists is read live, even with a snapshot beside it.
        assert!(read_all(&claude, &run).snapshot.is_none());
        let proj = config.join("projects/-work-toy");
        std::fs::remove_file(proj.join(format!("{WAVE1_SID}.jsonl"))).unwrap();
        std::fs::remove_dir_all(proj.join(WAVE1_SID)).unwrap();
        let from_snapshot = read_all(&claude, &run);
        let info = from_snapshot.snapshot.clone().expect("the snapshot marker");
        assert_eq!(
            (info.policy, info.content_kept),
            (CapturePolicy::Full, true)
        );
        let ids = |v: &SessionView| {
            v.page
                .events
                .iter()
                .map(|e| e.id.clone())
                .collect::<Vec<_>>()
        };
        assert!(!ids(&live).is_empty());
        assert_eq!(
            ids(&from_snapshot),
            ids(&live),
            "the same Event ids in the same order"
        );
        let refs = |v: &SessionView| {
            v.page
                .events
                .iter()
                .map(|e| e.content.source.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(refs(&from_snapshot), refs(&live), "and the same SourceRefs");
        assert_eq!(
            from_snapshot
                .failures
                .iter()
                .map(|f| f.id.clone())
                .collect::<Vec<_>>(),
            live.failures
                .iter()
                .map(|f| f.id.clone())
                .collect::<Vec<_>>(),
            "the same Failures"
        );
        assert!(from_snapshot
            .notices
            .iter()
            .any(|n| n.starts_with("transcript gone")));
        // Under `off` nothing is read, snapshot or not.
        let off = keeping(vec![account("a", &config, CapturePolicy::Off)]);
        let view = read_all(&off, &run);
        assert!((view.status.as_str(), view.page.events.len()) == ("off", 0));
        // The index reads the same Events from the snapshot for a rebuilt flight.db.
        let mut conn = index::open_writer(&dir.join("rebuilt.db")).unwrap();
        pass_over(&mut conn, &claude, &run);
        let indexed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session = ?1",
                [WAVE1_SID],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            indexed > 0,
            "a rebuilt index holds the gone session's Events from the snapshot"
        );
    }

    #[test]
    fn the_checked_in_snapshot_fixture_opens_with_its_transcript_removed() {
        use crate::flight::testkit::wave2_snapshot_run;
        let dir = TempDir::new("flight_wave2_fixture");
        let (run, config) = wave2_snapshot_run(&dir);
        let full = settings(vec![account("a", &config, CapturePolicy::Full)]);
        let view = read_all(&full, &run);
        let info = view.snapshot.clone().expect("opened from the snapshot");
        assert_eq!(
            (info.policy, info.content_kept),
            (CapturePolicy::Full, true)
        );
        let on_disk: Vec<String> = std::fs::read_to_string(run.join("flight/events.jsonl"))
            .unwrap()
            .lines()
            .map(|l| {
                serde_json::from_str::<super::super::schema::Event>(l)
                    .unwrap()
                    .id
            })
            .collect();
        assert!(!on_disk.is_empty());
        let transcript_ids: Vec<String> = view
            .page
            .events
            .iter()
            .filter(|e| e.provider.name != "dcv2")
            .map(|e| e.id.clone())
            .collect();
        for id in &on_disk {
            assert!(transcript_ids.contains(id), "{id}");
        }
        assert_eq!(transcript_ids.len(), on_disk.len());

        // Content: the line whose sha256 matches the ref, redacted, while the policy is redacted/full.
        let event = view
            .page
            .events
            .iter()
            .find(|e| e.id == on_disk[0])
            .unwrap();
        let query = Query {
            run_dir: Some(run.clone()),
            ..Default::default()
        };
        let out = read_content(&full, &query, &event.content.source, &Opens::default()).unwrap();
        assert!(!out.text.is_empty());
        assert_eq!(out.policy, CapturePolicy::Full);
        // A ref whose line was never kept: "content not kept".
        let mut other = event.content.source.clone();
        other.sha256 = "0".repeat(64);
        assert_eq!(
            read_content(&full, &query, &other, &Opens::default()).unwrap_err(),
            "content not kept"
        );
        // A file the snapshot does not know is refused as before.
        let mut foreign = event.content.source.clone();
        foreign.file = "/elsewhere/x.jsonl".into();
        assert_eq!(
            read_content(&full, &query, &foreign, &Opens::default()).unwrap_err(),
            "that file is not part of this run"
        );
        // Under a current `metadata` policy: "content not kept", though content.jsonl exists.
        assert!(run.join("flight/content.jsonl").is_file());
        let metadata = settings(vec![account("a", &config, CapturePolicy::Metadata)]);
        assert_eq!(
            read_content(&metadata, &query, &event.content.source, &Opens::default()).unwrap_err(),
            "content not kept"
        );
        let shown = read_all(&metadata, &run);
        assert!(
            shown.snapshot.is_some(),
            "the Events still open under metadata"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_snapshot_made_of_links_is_refused() {
        use crate::flight::testkit::wave2_snapshot_run;
        let dir = TempDir::new("flight_wave2_links");
        let (run, config) = wave2_snapshot_run(&dir);
        let full = settings(vec![account("a", &config, CapturePolicy::Full)]);
        assert!(read_all(&full, &run).snapshot.is_some());
        // events.jsonl is a link: nothing is read from the snapshot.
        let events = run.join("flight/events.jsonl");
        let real = dir.join("elsewhere.jsonl");
        std::fs::rename(&events, &real).unwrap();
        std::os::unix::fs::symlink(&real, &events).unwrap();
        assert!(
            read_all(&full, &run).snapshot.is_none(),
            "a linked events.jsonl"
        );
        std::fs::remove_file(&events).unwrap();
        std::fs::rename(&real, &events).unwrap();
        // flight itself is a link.
        let moved = dir.join("moved-flight");
        std::fs::rename(run.join("flight"), &moved).unwrap();
        std::os::unix::fs::symlink(&moved, run.join("flight")).unwrap();
        assert!(
            read_all(&full, &run).snapshot.is_none(),
            "a linked flight folder"
        );
    }

    // ---- compare ----

    fn no_usage(_: &str) -> UsageHistory {
        Vec::new()
    }

    /// The pair indexed under two accounts (one per account folder), plus the settings for it.
    fn compare_setup(dir: &TempDir) -> (PathBuf, PathBuf, ClaudeSettings) {
        use crate::flight::testkit::compare_guard_runs;
        let ((pass, pass_config), (fail, fail_config)) = compare_guard_runs(dir);
        let claude = settings(vec![
            account("p", &pass_config, CapturePolicy::Metadata),
            account("f", &fail_config, CapturePolicy::Metadata),
        ]);
        let mut conn = index::open_writer(&dir.join(INDEX_FILE)).unwrap();
        index_pass(
            &mut conn,
            &claude,
            &[pass.clone(), fail.clone()],
            &Limits::default(),
        )
        .unwrap();
        (pass, fail, claude)
    }

    #[test]
    fn two_runs_that_differ_by_one_guard_block_diverge_at_the_failures_row() {
        let dir = TempDir::new("flight_compare_guard");
        let (pass, fail, claude) = compare_setup(&dir);
        let out = compare_runs(dir.path(), &claude, &pass, &fail, &no_usage).unwrap();
        assert!(out.a.indexed && out.b.indexed && !out.a.off && !out.b.off);
        let at = out.divergence.expect("a difference");
        let row = &out.rows[at];
        assert_eq!(
            (row.group.as_str(), row.key.as_str()),
            ("failures", "tool.blocked:run-file-write")
        );
        assert_eq!((&row.a, &row.b), (&json!(0), &json!(1)));
        assert!(row.a_event.is_none(), "the passing run has no such Event");
        let blocked = row
            .b_event
            .clone()
            .expect("the failing run's blocked Event opens in its Trace");
        let page = read_all(
            &settings(vec![account(
                "f",
                &dir.join("config-fail"),
                CapturePolicy::Metadata,
            )]),
            &fail,
        );
        assert!(page
            .page
            .events
            .iter()
            .any(|e| e.id == blocked && e.kind == "tool.blocked"));
        assert_eq!(
            out.rows.iter().filter(|r| r.differs).count(),
            1,
            "only the guard block differs: {:?}",
            out.rows.iter().filter(|r| r.differs).collect::<Vec<_>>()
        );
        // The record numbers are the same for both and are rows too.
        let tokens: Vec<_> = out.rows.iter().filter(|r| r.group == "tokens").collect();
        assert_eq!(tokens.len(), 4);
        assert!(out
            .rows
            .iter()
            .any(|r| r.group == "tools" && r.key == "Bash" && r.a == json!(1) && r.b == json!(1)));
        // Swapping the sides swaps the values and finds the same row.
        let back = compare_runs(dir.path(), &claude, &fail, &pass, &no_usage).unwrap();
        assert_eq!(
            back.divergence.map(|i| back.rows[i].key.clone()),
            Some("tool.blocked:run-file-write".to_string())
        );
        assert_eq!(
            (
                &back.rows[back.divergence.unwrap()].a,
                &back.rows[back.divergence.unwrap()].b
            ),
            (&json!(1), &json!(0))
        );
    }

    #[test]
    fn compare_refuses_the_same_folder_and_reports_a_side_that_is_not_indexed_or_off() {
        let dir = TempDir::new("flight_compare_states");
        let (pass, fail, claude) = compare_setup(&dir);
        let same = compare_runs(dir.path(), &claude, &pass, &pass, &no_usage).unwrap_err();
        assert_eq!(same, "Pick two different runs.");

        // A run the index has not seen: its side says so, and no rows are made.
        let other = dir.join("r-20261105-100000-new");
        let other_config = dir.join("config-new");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::create_dir_all(&other_config).unwrap();
        let mut claude2 = claude.clone();
        claude2
            .accounts
            .push(account("n", &other_config, CapturePolicy::Metadata));
        std::fs::write(
            other.join("meta.json"),
            json!({"account": {"id": "n", "configDir": other_config.display().to_string()}})
                .to_string(),
        )
        .unwrap();
        std::fs::write(
            other.join("app-usage.json"),
            json!({"schema": 1, "sessions": []}).to_string(),
        )
        .unwrap();
        let out = compare_runs(dir.path(), &claude2, &pass, &other, &no_usage).unwrap();
        assert!(out.a.indexed && !out.b.indexed);
        assert!(out.rows.is_empty() && out.divergence.is_none());

        // An `off` account: that side reads nothing and the rows stay empty.
        let off = settings(vec![
            account("p", &dir.join("config-pass"), CapturePolicy::Off),
            account("f", &dir.join("config-fail"), CapturePolicy::Metadata),
        ]);
        let out = compare_runs(dir.path(), &off, &pass, &fail, &no_usage).unwrap();
        assert!(out.a.off && !out.b.off);
        assert!(out.rows.is_empty());
    }

    #[test]
    fn window_share_comes_from_the_accounts_readings_inside_the_runs_span() {
        let dir = TempDir::new("flight_compare_share");
        let (pass, fail, claude) = compare_setup(&dir);
        let at = |t: &str| {
            chrono::DateTime::parse_from_rfc3339(t)
                .unwrap()
                .timestamp_millis()
        };
        let usage = |_: &str| -> UsageHistory {
            vec![crate::usage::WindowHistory {
                rate_limit_type: "five_hour".into(),
                readings: vec![
                    crate::usage::Reading {
                        at_ms: at("2026-11-05T09:50:00Z"),
                        utilization: Some(10.0),
                        resets_at: Some(9_999_999_999),
                    },
                    crate::usage::Reading {
                        at_ms: at("2026-11-05T10:30:00Z"),
                        utilization: Some(12.0),
                        resets_at: Some(9_999_999_999),
                    },
                    crate::usage::Reading {
                        at_ms: at("2026-11-05T10:50:00Z"),
                        utilization: Some(14.5),
                        resets_at: Some(9_999_999_999),
                    },
                ],
            }]
        };
        let out = compare_runs(dir.path(), &claude, &pass, &fail, &usage).unwrap();
        let five = out.rows.iter().find(|r| r.key == "fiveHour").unwrap();
        assert_eq!(
            (&five.a, &five.b),
            (&json!(4.5), &json!(4.5)),
            "the estimate over the span"
        );
        let weekly = out.rows.iter().find(|r| r.key == "weekly").unwrap();
        assert_eq!(
            (&weekly.a, weekly.differs),
            (&Value::Null, false),
            "no weekly readings: not recorded"
        );
    }

    #[test]
    fn compare_never_writes_flight_db_and_never_creates_it() {
        let dir = TempDir::new("flight_compare_readonly");
        let (pass, fail, claude) = compare_setup(&dir);
        let db = dir.join(INDEX_FILE);
        let before = std::fs::read(&db).unwrap();
        let stamp = std::fs::metadata(&db).unwrap().modified().unwrap();
        compare_runs(dir.path(), &claude, &pass, &fail, &no_usage).unwrap();
        assert_eq!(std::fs::read(&db).unwrap(), before);
        assert_eq!(std::fs::metadata(&db).unwrap().modified().unwrap(), stamp);
        // No index at all: both sides are "not indexed" and nothing is created.
        let empty = TempDir::new("flight_compare_noindex");
        let out = compare_runs(empty.path(), &claude, &pass, &fail, &no_usage).unwrap();
        assert!(!out.a.indexed && !out.b.indexed && out.rows.is_empty());
        assert!(
            !empty.join(INDEX_FILE).exists(),
            "compare never indexes on demand"
        );
    }

    // ---- eval results ----

    /// A run folder made into an eval run of group `b-eval` at stage `stage`, with the record outcome `outcome`.
    fn make_eval(run: &Path, stage: &str, outcome: Option<&str>, reviewed: bool) {
        let read = |name: &str| -> Value {
            serde_json::from_str(&std::fs::read_to_string(run.join(name)).unwrap_or("{}".into()))
                .unwrap_or(json!({}))
        };
        let mut meta = read("meta.json");
        meta["eval"] = json!({"group": "b-eval", "index": 1, "of": 4, "source": "r-0"});
        std::fs::write(run.join("meta.json"), meta.to_string()).unwrap();
        let mut status = read("status.json");
        status["stage"] = json!(stage);
        status["waitingFor"] = Value::Null;
        if reviewed {
            status["prs"][0]["state"] = json!("reviewed");
        }
        std::fs::write(run.join("status.json"), status.to_string()).unwrap();
        let mut usage = read("app-usage.json");
        usage["outcome"] = json!(outcome);
        std::fs::write(run.join("app-usage.json"), usage.to_string()).unwrap();
    }

    #[test]
    fn eval_results_give_distributions_over_the_finished_runs_only() {
        let dir = TempDir::new("flight_eval_results");
        let (pass, fail, claude) = compare_setup(&dir);
        // A: finished and completed, with an app check that failed. B: finished, not completed, never checked.
        make_eval(&pass, "ship-approval", Some("eval-done"), true);
        make_eval(&fail, "ship-approval", Some("eval-done"), false);
        std::fs::remove_file(fail.join("app-checks.json")).unwrap();
        // C is still running, D failed.
        let running = dir.join("r-20261105-100000-c");
        let failed = dir.join("r-20261105-100000-d");
        for (run, stage) in [(&running, "building"), (&failed, "failed")] {
            std::fs::create_dir_all(run).unwrap();
            std::fs::write(
                run.join("status.json"),
                json!({"schema": 1, "stage": stage, "prs": []}).to_string(),
            )
            .unwrap();
            make_eval(run, stage, None, false);
        }
        // Index A and B after the edits (the index holds their Retries and Failures).
        let mut conn = index::open_writer(&dir.join(INDEX_FILE)).unwrap();
        index_pass(
            &mut conn,
            &claude,
            &[pass.clone(), fail.clone()],
            &Limits::default(),
        )
        .unwrap();
        let dirs: Vec<PathBuf> = [&pass, &fail, &running, &failed]
            .iter()
            .map(|p| p.canonicalize().unwrap())
            .collect();
        crate::run::folder::adopt_runs(dir.path(), &dirs).unwrap();
        crate::run::dispatch::with(dir.path(), |d| {
            d.batches.push(crate::run::dispatch::Batch {
                id: "b-eval".into(),
                created_at: "t".into(),
                runs: dirs
                    .iter()
                    .enumerate()
                    .map(|(i, p)| crate::run::dispatch::BatchRun {
                        entry_id: format!("q-{i}"),
                        ticket_ids: vec![],
                        label: format!("Eval run {} of 4", i + 1),
                        run_dir: Some(p.display().to_string()),
                        removed: false,
                    })
                    .collect(),
                notified: vec![],
                kind: Some("eval".into()),
                source: Some("r-0".into()),
                n: Some(4),
                extra: Default::default(),
            })
        })
        .unwrap();

        let checking = |p: &Path| p.ends_with("r-20261105-100000-fail");
        let out = eval_results(dir.path(), "b-eval", &checking).unwrap();
        assert_eq!((out.finished, out.running, out.failed, out.n), (2, 1, 1, 4));
        let by = |i: usize| &out.runs[i];
        assert_eq!(by(0).status, "finished");
        assert_eq!(
            by(0).completed,
            Some(true),
            "eval-done, reviewed, recorded check pass"
        );
        assert_eq!(by(1).completed, Some(false), "a PR still fixing");
        assert_eq!(by(2).status, "running");
        assert_eq!(by(2).completed, None);
        assert_eq!(by(3).status, "failed");
        assert_eq!(
            by(3).completed,
            Some(false),
            "a failed run counts as not completed"
        );
        // The check pass rate comes from app-checks.json only; none reads null, never 0.
        assert_eq!(by(0).check_pass_rate, Some(0.0));
        assert_eq!(by(0).check_state, "checked");
        assert_eq!(by(1).check_pass_rate, None);
        assert_eq!(
            by(1).check_state,
            "checking",
            "the slot is held for this run"
        );
        // Measures of a run still going are null.
        assert_eq!(
            (by(2).tokens, by(2).retries, by(2).active_minutes),
            (None, None, None)
        );
        // Retries and tool Failures come from the index; tokens and minutes from the record.
        assert_eq!((by(0).retries, by(0).tool_failures), (Some(2), Some(0)));
        assert_eq!((by(1).retries, by(1).tool_failures), (Some(2), Some(1)));
        assert_eq!(by(0).tokens, Some(1540));
        assert_eq!(by(0).active_minutes, Some(50.0));
        // The distributions cover the two finished runs.
        let d = &out.distributions;
        assert_eq!(
            (
                d.completed.min,
                d.completed.median,
                d.completed.max,
                d.completed.of
            ),
            (Some(0.0), Some(0.5), Some(1.0), 2)
        );
        assert_eq!(
            (
                d.tool_failures.min,
                d.tool_failures.median,
                d.tool_failures.max
            ),
            (Some(0.0), Some(0.5), Some(1.0))
        );
        assert_eq!(
            (d.retries.min, d.retries.max, d.retries.of),
            (Some(2.0), Some(2.0), 2)
        );
        assert_eq!(
            d.check_pass_rate,
            Dist {
                min: Some(0.0),
                median: Some(0.0),
                max: Some(0.0),
                of: 1
            },
            "over the one run that has it"
        );
        assert_eq!(d.tokens.median, Some(1540.0));
        assert!(eval_results(dir.path(), "b-nope", &checking).is_err());
    }

    #[test]
    fn a_distribution_of_nothing_is_empty_not_zero() {
        assert_eq!(Dist::of_values(vec![]), Dist::default());
        assert_eq!(Dist::of_values(vec![f64::NAN]), Dist::default());
        let d = Dist::of_values(vec![3.0, 1.0, 2.0, 10.0]);
        assert_eq!(
            (d.min, d.median, d.max, d.of),
            (Some(1.0), Some(2.5), Some(10.0), 4)
        );
    }
}
