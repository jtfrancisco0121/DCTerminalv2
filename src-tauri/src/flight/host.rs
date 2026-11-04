//! The seam between the flight core and the app: the only file in `flight/`
//! that may use `crate::`. It maps each session folder to its configured
//! account and capture policy before any file is opened, resolves the files of
//! a run or session, and reads a line's content (always redacted) on request.

use super::adapters::{AgentMeta, SubagentFile};
use super::redact::redact;
use super::schema::{sha256_hex, CapturePolicy, SourceRef};
use crate::provider::claude::same_config_dir;
use crate::store::settings_store::{resolved_account_config, ClaudeAccount, ClaudeSettings};
use serde::Serialize;
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

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
    pub config_dir: String,
    pub session_id: String,
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
        sessions.push(SessionFiles {
            config_dir,
            session_id,
            main,
            subagents,
        });
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
