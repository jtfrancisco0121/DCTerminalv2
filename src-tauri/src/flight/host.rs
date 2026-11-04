//! The seam between the flight core and the app: the only file in `flight/`
//! that may use `crate::`. It maps each session folder to its configured
//! account and capture policy before any file is opened, resolves the files of
//! a run or session, and reads a line's content (always redacted) on request.

use super::adapters::dcv2_run::{self, ApprovalIn, RunEvents, RunInputs};
use super::adapters::{AgentMeta, SubagentFile};
use super::index::{
    self, Counts, Exclusion, Limits, PassStats, RunInput, SearchFilter, SearchOut, SessionInput,
    SourceStamp,
};
use super::redact::{first_hit, redact};
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

/// `flight_session`: the policy first, then the files, then one page.
pub fn session_view(
    claude: &ClaudeSettings,
    query: &Query,
    request: &PageRequest,
    cache: &FlightCache,
    opens: &Opens,
) -> Result<SessionView, String> {
    let trace = query
        .run_dir
        .as_ref()
        .and_then(|d| d.file_name())
        .map(|n| n.to_string_lossy().to_string());
    let set = match resolve(claude, query, opens) {
        Access::Refused(why) => return Err(why),
        Access::Off => return Ok(SessionView::off(trace.as_deref().unwrap_or(""))),
        Access::Ready(set) => set,
    };
    let exact = query.run_dir.as_deref().is_some_and(exact_pr);
    let parsed: Vec<Arc<Parsed>> = set
        .sessions
        .iter()
        .map(|s| cache.parsed(s, exact))
        .collect();
    let run = set
        .run_dir
        .as_deref()
        .map(|dir| cache.run_events(dir, opens));
    session::view(
        &parsed,
        run.as_deref(),
        trace.as_deref(),
        set.policy,
        request,
        set.notices,
    )
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
    sessions: Vec<Option<SessionFiles>>,
}

impl index::Loader for HostLoader {
    fn session(&self, index: usize) -> Arc<Parsed> {
        match self.sessions.get(index).and_then(Option::as_ref) {
            Some(files) => Arc::new(session::parse(&files.main, &files.subagents, self.exact)),
            None => Arc::new(Parsed::from_events(Vec::new())),
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
        prepared.loader.sessions.push(Some(files.clone()));
    }
    for id in &set.gone {
        prepared.sessions.push(SessionInput {
            id: id.clone(),
            stamps: Vec::new(),
        });
        prepared.loader.sessions.push(None);
    }
    let mut names: Vec<(String, PathBuf)> = run_file_list(dir);
    for extra in ["meta.json", "repos.json", "app-usage.json"] {
        let path = dir.join(extra);
        if regular_file(&path) {
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

/// The three files of a run's snapshot, read without following links; `None` when it is absent or damaged.
fn read_snapshot(run_dir: &Path) -> Option<snapshot::Old> {
    let dir = run_dir.join(SNAPSHOT_DIR);
    let manifest: snapshot::Manifest = serde_json::from_str(
        &read_snapshot_text(&dir.join("manifest.json"), SNAPSHOT_MANIFEST_MAX).ok()?,
    )
    .ok()?;
    if manifest.schema != snapshot::SCHEMA {
        return None;
    }
    Some(snapshot::Old {
        events_jsonl: read_snapshot_text(&dir.join("events.jsonl"), SNAPSHOT_FILE_MAX).ok()?,
        content_jsonl: read_snapshot_text(&dir.join("content.jsonl"), SNAPSHOT_FILE_MAX).ok(),
        manifest,
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
    if let Some(old) = read_snapshot(run_dir) {
        if old.manifest.outcome == outcome && old.manifest.record_status_updated_at == updated {
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
        let result = build_snapshot(
            dir,
            &set,
            &outcome,
            updated,
            old.as_ref(),
            snapshot::MAX_BYTES,
        )
        .and_then(|built| write_snapshot(dir, &built));
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
    let set = match resolve(claude, query, opens) {
        Access::Refused(why) => return Err(why),
        Access::Off => {
            return Ok(ContentOut {
                text: String::new(),
                redactions: 0,
                policy: CapturePolicy::Off,
            })
        }
        Access::Ready(set) => set,
    };
    if set.contains_run_file(&source.file) {
        return read_run_file(source, set.policy, opens);
    }
    if !set.contains(&source.file) {
        return Err(if set.run_dir.is_some() {
            "that file is not part of this run"
        } else {
            "that file is not part of this session"
        }
        .into());
    }
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
    let (text, redactions) = render_line(&bytes);
    Ok(ContentOut {
        text,
        redactions,
        policy: set.policy,
    })
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
}
