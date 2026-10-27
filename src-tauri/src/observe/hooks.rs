//! The app side of the hook forwarder (A2): a std-thread HTTP listener on
//! `127.0.0.1:<random port>` that takes `dcv2-hook` posts, checks the
//! per-launch token of the tab, and keeps a per-tab state:
//! working / waiting / needs-you.

use super::forward::{PATH, TAB_HEADER, TOKEN_HEADER};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

/// What a tab's Claude is doing, from its hook events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TabActivity {
    /// No hook event yet.
    #[default]
    Unknown,
    /// A turn or a sub-agent is running.
    Working,
    /// The turn ended; Claude waits for your next message.
    Waiting,
    /// Claude asks for a permission (or another answer) in the tab.
    NeedsYou,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TabHookState {
    pub activity: TabActivity,
    pub last_event: Option<String>,
    /// Unix milliseconds of the last event.
    pub updated_ms: Option<u64>,
    /// Sub-agents started and not stopped (foreground or background).
    pub subagents_running: u32,
    /// A tool started (`PreToolUse`) and has not finished yet: a long Bash
    /// is work, not a stall.
    pub tool_running: bool,
    /// The session the events are for (a `/clear` or resume starts a new one).
    pub session_id: Option<String>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl TabHookState {
    /// One reduced hook event (`forward::reduce`).
    pub fn apply(&mut self, event: &Value, at_ms: u64) {
        let name = event
            .get("hook_event_name")
            .and_then(Value::as_str)
            .unwrap_or("");
        let background = event
            .get("background_running")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        match name {
            "SessionStart" => {
                self.activity = TabActivity::Waiting;
                self.tool_running = false;
                // Only SessionStart moves the session (a /clear or a resume
                // starts a new one). Any other event may come from a
                // sub-agent or an old session.
                if let Some(id) = event.get("session_id").and_then(Value::as_str) {
                    if crate::provider::claude::valid_session_id(id) {
                        self.session_id = Some(id.to_string());
                    }
                }
            }
            "UserPromptSubmit" => self.activity = TabActivity::Working,
            "PreToolUse" => {
                self.activity = TabActivity::Working;
                self.tool_running = true;
            }
            "PostToolUse" | "PostToolUseFailure" => {
                self.activity = TabActivity::Working;
                self.tool_running = false;
            }
            "SubagentStart" => {
                self.subagents_running += 1;
                self.activity = TabActivity::Working;
            }
            "SubagentStop" => {
                // A stop can come for an agent that never started (internal).
                self.subagents_running = self.subagents_running.saturating_sub(1);
            }
            "PermissionRequest" => self.activity = TabActivity::NeedsYou,
            "Notification" => {
                match event.get("notification_type").and_then(Value::as_str) {
                    Some("idle_prompt") => self.activity = TabActivity::Waiting,
                    Some("auth_success") => {}
                    // permission_prompt, elicitation_dialog and the like.
                    _ => self.activity = TabActivity::NeedsYou,
                }
            }
            "Stop" => {
                // The orchestrator's turn ended while background sub-agents
                // still run: it is waiting on them, not on you.
                self.subagents_running = background;
                self.tool_running = false;
                self.activity = if background > 0 {
                    TabActivity::Working
                } else {
                    TabActivity::Waiting
                };
            }
            "SessionEnd" => {
                self.activity = TabActivity::Unknown;
                self.subagents_running = 0;
                self.tool_running = false;
            }
            _ => return,
        }
        self.last_event = Some(name.to_string());
        self.updated_ms = Some(at_ms);
    }
}

#[derive(Default)]
struct Shared {
    /// tab id → the token its launch got.
    tokens: HashMap<String, String>,
    states: HashMap<String, TabHookState>,
}

/// Called with (tab id, session id) when a tab's `SessionStart` reports a
/// session other than the one the hook state had. Runs on a connection
/// thread with no hook-server lock held.
pub type SessionSink = Box<dyn Fn(&str, &str) + Send + Sync>;

/// The listener and the per-tab states. Cheap to clone.
#[derive(Clone)]
pub struct HookServer {
    pub port: u16,
    shared: Arc<Mutex<Shared>>,
    session_sink: Arc<OnceLock<SessionSink>>,
    live: Arc<AtomicUsize>,
}

/// At most this many connections are served at once; extras are dropped.
pub const MAX_CONNECTIONS: usize = 32;
/// A whole request (line, headers, body) must arrive within this.
pub const REQUEST_DEADLINE: Duration = Duration::from_secs(2);
/// The request line and headers together stay under this many bytes.
pub const MAX_HEAD: usize = 8 * 1024;

/// A reader that fails once the request's deadline has passed, and sets
/// each read's timeout to the time left.
pub struct Deadline<R> {
    pub inner: R,
    pub until: Instant,
    pub set_timeout: fn(&R, Duration),
}

impl<R: Read> Read for Deadline<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        (self.set_timeout)(&self.inner, left);
        self.inner.read(buf)
    }
}

/// Equal-time compare, so the token cannot be guessed byte by byte.
pub fn same_token(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub const MAX_BODY: usize = 64 * 1024;

/// A parsed post: (tab, token, body). None for anything else.
pub fn parse_request(reader: &mut impl BufRead) -> Option<(String, String, Vec<u8>)> {
    let mut line = String::new();
    reader
        .by_ref()
        .take(MAX_HEAD as u64)
        .read_line(&mut line)
        .ok()?;
    let mut head = line.len();
    let mut parts = line.split_whitespace();
    if parts.next()? != "POST" || parts.next()? != PATH {
        return None;
    }
    let (mut tab, mut token, mut length) = (None, None, 0usize);
    for _ in 0..64 {
        let mut header = String::new();
        let left = MAX_HEAD.checked_sub(head)?;
        reader
            .by_ref()
            .take(left as u64)
            .read_line(&mut header)
            .ok()?;
        head += header.len();
        if head >= MAX_HEAD || !header.ends_with('\n') {
            return None;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':')?;
        let value = value.trim().to_string();
        match name.trim().to_ascii_lowercase().as_str() {
            TAB_HEADER => tab = Some(value),
            TOKEN_HEADER => token = Some(value),
            "content-length" => length = value.parse().ok()?,
            _ => {}
        }
    }
    if length > MAX_BODY {
        return None;
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    Some((tab?, token?, body))
}

impl HookServer {
    /// Binds `127.0.0.1:0` and serves on a thread.
    pub fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let server = Self {
            port,
            shared: Arc::new(Mutex::new(Shared::default())),
            session_sink: Arc::new(OnceLock::new()),
            live: Arc::new(AtomicUsize::new(0)),
        };
        let serving = server.clone();
        std::thread::Builder::new()
            .name("dcv2-hook-listener".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    if serving.live.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                        serving.live.fetch_sub(1, Ordering::SeqCst);
                        drop(stream);
                        continue;
                    }
                    let handler = serving.clone();
                    let spawned = std::thread::Builder::new()
                        .name("dcv2-hook-conn".into())
                        .spawn(move || {
                            handler.handle(stream);
                            handler.live.fetch_sub(1, Ordering::SeqCst);
                        });
                    if spawned.is_err() {
                        serving.live.fetch_sub(1, Ordering::SeqCst);
                    }
                }
            })?;
        Ok(server)
    }

    /// Installs the sink for new session ids (once; a second call is ignored).
    pub fn set_session_sink(&self, sink: SessionSink) {
        let _ = self.session_sink.set(sink);
    }

    /// A new token for a tab's launch (the old one stops working).
    pub fn register_tab(&self, tab_id: &str) -> String {
        let token = uuid::Uuid::new_v4().simple().to_string();
        if let Ok(mut shared) = self.shared.lock() {
            shared.tokens.insert(tab_id.to_string(), token.clone());
            shared.states.remove(tab_id);
        }
        token
    }

    /// The tab's launch got hooks (a token) from this app run.
    pub fn has_tab(&self, tab_id: &str) -> bool {
        self.shared
            .lock()
            .is_ok_and(|s| s.tokens.contains_key(tab_id))
    }

    pub fn forget_tab(&self, tab_id: &str) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.tokens.remove(tab_id);
            shared.states.remove(tab_id);
        }
    }

    pub fn state(&self, tab_id: &str) -> Option<TabHookState> {
        self.shared.lock().ok()?.states.get(tab_id).cloned()
    }

    pub fn states(&self) -> HashMap<String, TabHookState> {
        self.shared
            .lock()
            .map(|s| s.states.clone())
            .unwrap_or_default()
    }

    /// Takes one post when its token is the tab's. Returns the HTTP status.
    pub fn accept(&self, tab: &str, token: &str, body: &[u8]) -> u16 {
        let new_session = {
            let Ok(mut shared) = self.shared.lock() else {
                return 500;
            };
            let known = shared
                .tokens
                .get(tab)
                .is_some_and(|want| same_token(want, token));
            if !known {
                return 403;
            }
            let Ok(event) = serde_json::from_slice::<Value>(body) else {
                return 400;
            };
            let state = shared.states.entry(tab.to_string()).or_default();
            let before = state.session_id.clone();
            state.apply(&event, now_ms());
            let started = state.last_event.as_deref() == Some("SessionStart");
            if started && state.session_id != before {
                state.session_id.clone()
            } else {
                None
            }
        };
        // After the lock is released: the sink takes the state store's.
        if let (Some(session), Some(sink)) = (new_session, self.session_sink.get()) {
            sink(tab, &session);
        }
        204
    }

    fn handle(&self, stream: TcpStream) {
        let _ = stream.set_write_timeout(Some(REQUEST_DEADLINE));
        let Ok(read_half) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(Deadline {
            inner: read_half,
            until: Instant::now() + REQUEST_DEADLINE,
            set_timeout: |s: &TcpStream, left| {
                let _ = s.set_read_timeout(Some(left));
            },
        });
        let status = match parse_request(&mut reader) {
            Some((tab, token, body)) => self.accept(&tab, &token, &body),
            None => 400,
        };
        let text = match status {
            204 => "No Content",
            403 => "Forbidden",
            500 => "Internal Server Error",
            _ => "Bad Request",
        };
        let mut stream = stream;
        let _ = write!(
            stream,
            "HTTP/1.1 {status} {text}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
    }
}

/// The hook events the app registers, each running the forwarder.
pub const EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "SubagentStart",
    "SubagentStop",
    "Stop",
    "Notification",
    "PermissionRequest",
    "SessionEnd",
];

/// The `hooks` settings value: every event in [`EVENTS`] runs `command`.
pub fn hook_settings(command: &str) -> Value {
    let mut hooks = serde_json::Map::new();
    for event in EVENTS {
        hooks.insert(
            event.to_string(),
            serde_json::json!([{"hooks": [{"type": "command", "command": command, "timeout": 5}]}]),
        );
    }
    serde_json::json!({ "hooks": hooks })
}

/// The forwarder binary: `DCV2_HOOK_BIN`, else `dcv2-hook` beside the app
/// executable (the bundle's `Contents/MacOS`, or cargo's `target/<profile>`
/// in dev, once `cargo build` built it).
pub fn forwarder_path(
    env_override: Option<&str>,
    exe: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    if let Some(path) = env_override.map(str::trim).filter(|p| !p.is_empty()) {
        let path = std::path::PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let name = if cfg!(windows) {
        "dcv2-hook.exe"
    } else {
        "dcv2-hook"
    };
    let path = exe?.parent()?.join(name);
    path.is_file().then_some(path)
}

pub fn quoted(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    if cfg!(windows) {
        format!("\"{}\"", text.replace('"', ""))
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::forward;
    use serde_json::json;

    fn fixture(name: &str) -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/fixtures/claude-2.1.287/hooks")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn recorded_hook_payloads_still_have_the_fields_the_app_reads() {
        for (file, event) in [
            ("SessionStart.json", "SessionStart"),
            ("UserPromptSubmit.json", "UserPromptSubmit"),
            ("SubagentStart.json", "SubagentStart"),
            ("SubagentStop.json", "SubagentStop"),
            ("PostToolUse.json", "PostToolUse"),
            ("Stop.json", "Stop"),
            ("SessionEnd.json", "SessionEnd"),
            ("Notification.synthetic.json", "Notification"),
            ("PermissionRequest.synthetic.json", "PermissionRequest"),
        ] {
            let payload = fixture(file);
            assert_eq!(payload["hook_event_name"], event, "{file}");
            assert!(payload["session_id"].is_string(), "{file}");
        }
        let start = fixture("SubagentStart.json");
        assert_eq!(start["agent_type"], "dcv2:plan-reviewer");
        assert!(start["agent_id"].is_string());
        assert!(fixture("Stop.json")["background_tasks"].is_array());
        assert!(fixture("SubagentStop.json")["background_tasks"].is_array());
    }

    #[test]
    fn the_forwarder_keeps_no_prompt_tool_input_or_answer() {
        let post = forward::reduce(&fixture("PostToolUse.json"));
        let text = post.to_string();
        assert_eq!(post["hook_event_name"], "PostToolUse");
        assert_eq!(post["tool_name"], "Agent");
        assert!(!text.contains("Self-test only"), "{text}");
        let prompt = forward::reduce(&fixture("UserPromptSubmit.json"));
        assert!(prompt.get("prompt").is_none());
        let stop = forward::reduce(&fixture("SubagentStop.json"));
        assert!(stop.get("last_assistant_message").is_none());
        assert_eq!(stop["background_running"], 0);
    }

    #[test]
    fn the_recorded_session_walks_the_tab_through_its_states() {
        let mut state = TabHookState::default();
        let order = [
            ("SessionStart.json", TabActivity::Waiting),
            ("UserPromptSubmit.json", TabActivity::Working),
            ("SubagentStart.json", TabActivity::Working),
            ("SubagentStop.json", TabActivity::Working),
            ("PostToolUse.json", TabActivity::Working),
            ("Stop.json", TabActivity::Waiting),
        ];
        for (file, want) in order {
            state.apply(&forward::reduce(&fixture(file)), 1);
            assert_eq!(state.activity, want, "{file}");
        }
        assert_eq!(state.subagents_running, 0);
        state.apply(
            &forward::reduce(&fixture("PermissionRequest.synthetic.json")),
            2,
        );
        assert_eq!(state.activity, TabActivity::NeedsYou);
        state.apply(&json!({"hook_event_name": "PostToolUse"}), 3);
        assert_eq!(state.activity, TabActivity::Working);
        state.apply(
            &json!({"hook_event_name": "Stop", "background_running": 2}),
            4,
        );
        assert_eq!(
            (state.activity, state.subagents_running),
            (TabActivity::Working, 2),
            "background sub-agents keep the orchestrator working"
        );
        state.apply(
            &json!({"hook_event_name": "Notification", "notification_type": "idle_prompt"}),
            5,
        );
        assert_eq!(state.activity, TabActivity::Waiting);
        state.apply(
            &json!({"hook_event_name": "PreToolUse", "tool_name": "Bash"}),
            6,
        );
        assert!(state.tool_running, "a long Bash is running");
        state.apply(&json!({"hook_event_name": "PostToolUseFailure"}), 6);
        assert!(!state.tool_running);
        assert_eq!(
            state.session_id.as_deref(),
            Some("de6a6329-de0f-49da-960b-f50fed9236e4")
        );
        state.apply(&json!({"hook_event_name": "SessionStart", "session_id": "11111111-2222-4333-8444-555555555555"}), 6);
        assert_eq!(
            state.session_id.as_deref(),
            Some("11111111-2222-4333-8444-555555555555"),
            "a /clear moves to the new session"
        );
        state.apply(&json!({"hook_event_name": "Unheard"}), 7);
        assert_eq!(state.updated_ms, Some(6), "unknown events change nothing");
    }

    #[test]
    fn posts_with_a_wrong_or_old_token_are_refused() {
        let server = HookServer::start().unwrap();
        let token = server.register_tab("tab-a");
        let body = br#"{"hook_event_name":"UserPromptSubmit"}"#;
        assert_eq!(server.accept("tab-a", "nope", body), 403);
        assert_eq!(server.accept("tab-b", &token, body), 403, "another tab");
        assert_eq!(server.accept("tab-a", &token, b"not json"), 400);
        assert!(server.state("tab-a").is_none());
        assert_eq!(server.accept("tab-a", &token, body), 204);
        assert_eq!(
            server.state("tab-a").unwrap().activity,
            TabActivity::Working
        );
        let fresh = server.register_tab("tab-a");
        assert_eq!(
            server.accept("tab-a", &token, body),
            403,
            "relaunch drops the old token"
        );
        assert_eq!(server.accept("tab-a", &fresh, body), 204);
        server.forget_tab("tab-a");
        assert_eq!(server.accept("tab-a", &fresh, body), 403);
        assert!(same_token("abc", "abc") && !same_token("abc", "abd") && !same_token("ab", "abc"));
    }

    #[test]
    fn a_session_start_with_a_new_id_reaches_the_sink_once() {
        let server = HookServer::start().unwrap();
        let seen: Arc<Mutex<Vec<(String, String)>>> = Default::default();
        let into = Arc::clone(&seen);
        server.set_session_sink(Box::new(move |tab, session| {
            into.lock()
                .unwrap()
                .push((tab.to_string(), session.to_string()));
        }));
        let token = server.register_tab("tab-s");
        let a = "11111111-2222-4333-8444-555555555555";
        let b = "66666666-7777-4888-8999-000000000000";
        let start =
            |id: &str| format!(r#"{{"hook_event_name":"SessionStart","session_id":"{id}"}}"#);
        let other =
            |id: &str| format!(r#"{{"hook_event_name":"PostToolUse","session_id":"{id}"}}"#);
        // A wrong token calls nothing.
        assert_eq!(server.accept("tab-s", "nope", start(a).as_bytes()), 403);
        assert!(seen.lock().unwrap().is_empty());
        assert_eq!(server.accept("tab-s", &token, start(a).as_bytes()), 204);
        // The same id again, and other events with another id: nothing.
        assert_eq!(server.accept("tab-s", &token, start(a).as_bytes()), 204);
        assert_eq!(server.accept("tab-s", &token, other(b).as_bytes()), 204);
        assert_eq!(
            server.state("tab-s").unwrap().session_id.as_deref(),
            Some(a)
        );
        // A bad id is not a session.
        assert_eq!(
            server.accept("tab-s", &token, start("../x").as_bytes()),
            204
        );
        assert_eq!(server.accept("tab-s", &token, start(b).as_bytes()), 204);
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                ("tab-s".to_string(), a.to_string()),
                ("tab-s".to_string(), b.to_string())
            ]
        );
    }

    #[test]
    fn the_forwarder_posts_over_http_and_fails_quietly() {
        let server = HookServer::start().unwrap();
        let token = server.register_tab("tab-h");
        let port = server.port.to_string();
        let payload = serde_json::to_vec(&fixture("PermissionRequest.synthetic.json")).unwrap();
        assert!(forward::forward(
            Some(&port),
            Some(&token),
            Some("tab-h"),
            &mut payload.as_slice()
        ));
        // The listener handles the post on its own thread.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while server.state("tab-h").is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            server.state("tab-h").unwrap().activity,
            TabActivity::NeedsYou
        );

        // No env, bad port, bad JSON, header injection, nobody listening:
        // all return quickly without an error.
        let started = std::time::Instant::now();
        assert!(!forward::forward(
            None,
            Some("t"),
            Some("x"),
            &mut &b"{}"[..]
        ));
        assert!(!forward::forward(
            Some("0"),
            Some("t"),
            Some("x"),
            &mut &b"{}"[..]
        ));
        assert!(!forward::forward(
            Some(&port),
            Some(&token),
            Some("tab-h"),
            &mut &b"{"[..]
        ));
        assert!(!forward::forward(
            Some(&port),
            Some("a\r\nX: y"),
            Some("tab-h"),
            &mut &b"{}"[..]
        ));
        let closed = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let dead = closed.local_addr().unwrap().port().to_string();
        drop(closed);
        assert!(!forward::forward(
            Some(&dead),
            Some("t"),
            Some("x"),
            &mut &b"{}"[..]
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn requests_are_parsed_strictly() {
        let good = forward::request(9, "tok", "tab", "{}");
        let (tab, token, body) = parse_request(&mut good.as_bytes()).unwrap();
        assert_eq!(
            (tab.as_str(), token.as_str(), body.as_slice()),
            ("tab", "tok", &b"{}"[..])
        );
        assert!(parse_request(&mut &b"GET /hook HTTP/1.1\r\n\r\n"[..]).is_none());
        let many = format!("POST /hook HTTP/1.1\r\nX-Dcv2-Tab: t\r\nX-Dcv2-Token: k\r\nX-Pad: {}\r\nContent-Length: 2\r\n\r\n{{}}", "a".repeat(MAX_HEAD));
        assert!(
            parse_request(&mut many.as_bytes()).is_none(),
            "headers over 8 KiB"
        );
        let huge = format!(
            "POST /hook HTTP/1.1\r\nX-Dcv2-Tab: t\r\nX-Dcv2-Token: k\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY + 1
        );
        assert!(parse_request(&mut huge.as_bytes()).is_none());
        let settings = hook_settings("'/x/dcv2-hook'");
        assert_eq!(
            settings["hooks"]["Stop"][0]["hooks"][0]["command"],
            "'/x/dcv2-hook'"
        );
        assert_eq!(settings["hooks"].as_object().unwrap().len(), EVENTS.len());
    }
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    #[test]
    fn a_slow_request_is_cut_at_the_overall_deadline() {
        let server = HookServer::start().unwrap();
        let mut slow = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        let started = Instant::now();
        slow.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        // A byte now and then never finishes the request line.
        while started.elapsed() < REQUEST_DEADLINE + Duration::from_millis(500) {
            if slow.write_all(b"P").is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let mut answer = Vec::new();
        let _ = slow.read_to_end(&mut answer);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(
            String::from_utf8_lossy(&answer).starts_with("HTTP/1.1 400"),
            "{answer:?}"
        );
    }

    #[test]
    fn connections_over_the_cap_are_dropped() {
        let server = HookServer::start().unwrap();
        let held: Vec<TcpStream> = (0..MAX_CONNECTIONS)
            .map(|_| TcpStream::connect(("127.0.0.1", server.port)).unwrap())
            .collect();
        let deadline = Instant::now() + Duration::from_secs(5);
        while server.live.load(Ordering::SeqCst) < MAX_CONNECTIONS && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(server.live.load(Ordering::SeqCst), MAX_CONNECTIONS);
        let mut extra = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        extra
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut buf = [0u8; 16];
        // Closed by the listener with no answer.
        assert!(matches!(extra.read(&mut buf), Ok(0) | Err(_)));
        assert!(server.live.load(Ordering::SeqCst) <= MAX_CONNECTIONS);
        drop(held);
    }
}

#[cfg(test)]
mod prefix_tests {
    use crate::observe::forward;

    #[test]
    fn a_payload_cut_at_the_input_cap_still_names_its_event() {
        let mut raw = br#"{"session_id":"de6a6329-de0f-49da-960b-f50fed9236e4","hook_event_name":"PostToolUse","tool_name":"Bash","tool_response":{"stdout":""#.to_vec();
        raw.resize(forward::MAX_INPUT as usize, b'x');
        let fields = forward::prefix_fields(&raw);
        assert_eq!(fields["hook_event_name"], "PostToolUse");
        assert_eq!(fields["tool_name"], "Bash");
        assert_eq!(fields["session_id"], "de6a6329-de0f-49da-960b-f50fed9236e4");
        assert!(fields.get("agent_id").is_none());
        let reduced = forward::reduce(&fields);
        assert_eq!(reduced["hook_event_name"], "PostToolUse");
    }
}
