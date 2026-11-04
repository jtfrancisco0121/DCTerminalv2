//! The seam between the flight core and the app: the only file in `flight/`
//! that may use `crate::`. It maps each session folder to its configured
//! account and capture policy before any file is opened, resolves the files of
//! a run or session, and reads a line's content (always redacted) on request.

use super::adapters::{AgentMeta, SubagentFile};
use super::redact::redact;
use super::schema::{sha256_hex, CapturePolicy, SourceRef};
use super::session::{self, PageRequest, Parsed, SessionView};
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

/// Checks the policy for every folder first, and only then looks for files.
pub fn resolve(claude: &ClaudeSettings, query: &Query, opens: &Opens) -> Access {
    let mut wanted: Vec<(String, String)> = Vec::new();
    let mut meta: Option<Value> = None;
    if let Some(run) = &query.run_dir {
        let record = crate::run::record::read_record(run);
        for s in record.into_iter().flat_map(|r| r.sessions) {
            wanted.push((s.config_dir, s.session_id));
        }
        meta = crate::run::folder::read_json_value(&run.join("meta.json"));
    } else if let (Some(dir), Some(id)) = (&query.config_dir, &query.session_id) {
        wanted.push((dir.clone(), id.clone()));
    }
    let dirs: Vec<String> = wanted.iter().map(|(d, _)| d.clone()).collect();
    let policy = match policy_for_files(claude, &dirs, meta.as_ref()) {
        Ok(CapturePolicy::Off) => return Access::Off,
        Ok(policy) => policy,
        Err(refusal) => return Access::Refused(refusal),
    };
    let mut sessions = Vec::new();
    let mut notices = Vec::new();
    for (config_dir, session_id) in wanted {
        let Some(main) =
            crate::claude_session_log::find_session_log(Path::new(&config_dir), &session_id)
        else {
            notices.push(format!("transcript gone: {session_id}"));
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
    })
}

impl SessionSet {
    /// Whether `file` is one of this set's transcripts.
    pub fn contains(&self, file: &str) -> bool {
        let wanted = ident(Path::new(file));
        self.sessions.iter().any(|s| {
            ident(&s.main) == wanted || s.subagents.iter().any(|a| ident(&a.path) == wanted)
        })
    }
}

/// How many parsed sessions the cache keeps.
const CACHE_SESSIONS: usize = 8;

/// A file's identity for the cache: its real path, size and mtime.
type Stamp = (String, u64, Option<SystemTime>);

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok();
    (
        ident(path),
        meta.as_ref().map(|m| m.len()).unwrap_or(0),
        meta.and_then(|m| m.modified().ok()),
    )
}

#[derive(Debug, Default)]
struct CacheInner {
    /// Oldest first; a session is keyed by the stamps of all its files.
    entries: Mutex<Vec<(Vec<Stamp>, Arc<Parsed>)>>,
    hits: AtomicUsize,
    misses: AtomicUsize,
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

    fn parsed(&self, files: &SessionFiles) -> Arc<Parsed> {
        let mut key = vec![stamp(&files.main)];
        key.extend(files.subagents.iter().map(|s| stamp(&s.path)));
        if let Ok(entries) = self.0.entries.lock() {
            if let Some((_, hit)) = entries.iter().find(|(k, _)| *k == key) {
                self.0.hits.fetch_add(1, Ordering::SeqCst);
                return hit.clone();
            }
        }
        self.0.misses.fetch_add(1, Ordering::SeqCst);
        let parsed = Arc::new(session::parse(&files.main, &files.subagents));
        if let Ok(mut entries) = self.0.entries.lock() {
            // The file grew: drop the older parse of the same main file.
            entries.retain(|(k, _)| k.first().map(|f| &f.0) != key.first().map(|f| &f.0));
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
    let parsed: Vec<Arc<Parsed>> = set.sessions.iter().map(|s| cache.parsed(s)).collect();
    session::view(&parsed, trace.as_deref(), set.policy, request, set.notices)
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
            let (clean, n) = redact(text);
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
    if !set.contains(&source.file) {
        return Err("that file is not part of this session".into());
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
}
