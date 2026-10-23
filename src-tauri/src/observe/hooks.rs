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
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

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
            "SessionStart" => self.activity = TabActivity::Waiting,
            "UserPromptSubmit" | "PostToolUse" | "PreToolUse" => {
                self.activity = TabActivity::Working
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
                self.activity = if background > 0 {
                    TabActivity::Working
                } else {
                    TabActivity::Waiting
                };
            }
            "SessionEnd" => {
                self.activity = TabActivity::Unknown;
                self.subagents_running = 0;
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

/// The listener and the per-tab states. Cheap to clone.
#[derive(Clone)]
pub struct HookServer {
    pub port: u16,
    shared: Arc<Mutex<Shared>>,
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
    reader.by_ref().take(4096).read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    if parts.next()? != "POST" || parts.next()? != PATH {
        return None;
    }
    let (mut tab, mut token, mut length) = (None, None, 0usize);
    for _ in 0..64 {
        let mut header = String::new();
        reader.by_ref().take(4096).read_line(&mut header).ok()?;
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
        };
        let serving = server.clone();
        std::thread::Builder::new()
            .name("dcv2-hook-listener".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let handler = serving.clone();
                    let _ = std::thread::Builder::new()
                        .name("dcv2-hook-conn".into())
                        .spawn(move || handler.handle(stream));
                }
            })?;
        Ok(server)
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

    #[allow(dead_code)] // tests; tabs are re-registered on each launch
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
        shared
            .states
            .entry(tab.to_string())
            .or_default()
            .apply(&event, now_ms());
        204
    }

    fn handle(&self, stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        let Ok(read_half) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(read_half);
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
    "PostToolUse",
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
        state.apply(&json!({"hook_event_name": "Unheard"}), 6);
        assert_eq!(state.updated_ms, Some(5), "unknown events change nothing");
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
