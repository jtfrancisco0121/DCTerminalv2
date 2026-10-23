//! The hook forwarder (A2): what `dcv2-hook` does with one hook call.
//!
//! Claude Code runs `dcv2-hook` for each registered hook event with the
//! event's JSON on stdin. It POSTs a reduced copy to the app on
//! `127.0.0.1:$DCV2_HOOK_PORT` with `$DCV2_HOOK_TOKEN` and `$DCV2_TAB_ID`,
//! prints nothing and always exits 0 within about a second, so it never
//! blocks or changes Claude's run.
//!
//! Only the fields the app needs leave the hook: never the prompt, tool
//! input or output, or the last assistant message.
//!
//! Shared by the `dcv2-hook` binary (`src/bin/dcv2-hook.rs`, which includes
//! this file by path and needs only std + serde_json) and the app's tests.

use serde_json::{json, Map, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

pub const PORT_ENV: &str = "DCV2_HOOK_PORT";
pub const TOKEN_ENV: &str = "DCV2_HOOK_TOKEN";
pub const TAB_ENV: &str = "DCV2_TAB_ID";
pub const TOKEN_HEADER: &str = "x-dcv2-token";
pub const TAB_HEADER: &str = "x-dcv2-tab";
pub const PATH: &str = "/hook";
/// The hook's stdin is read up to this many bytes (hook JSON is small).
pub const MAX_INPUT: u64 = 1024 * 1024;
/// Connect, write and read together stay under a second.
pub const STEP_TIMEOUT: Duration = Duration::from_millis(300);

/// The fields kept from a hook payload. `background_running` counts the
/// background tasks Claude reports as running (Stop, SubagentStop).
pub fn reduce(payload: &Value) -> Value {
    let mut out = Map::new();
    for key in [
        "hook_event_name",
        "session_id",
        "agent_id",
        "agent_type",
        "notification_type",
        "tool_name",
        "source",
        "reason",
    ] {
        if let Some(Value::String(text)) = payload.get(key) {
            out.insert(key.into(), Value::String(text.chars().take(200).collect()));
        }
    }
    if let Some(Value::Array(tasks)) = payload.get("background_tasks") {
        let running = tasks
            .iter()
            .filter(|t| t.get("status").and_then(Value::as_str) == Some("running"))
            .count();
        out.insert("background_running".into(), json!(running));
    }
    Value::Object(out)
}

/// The HTTP request the forwarder sends.
pub fn request(port: u16, token: &str, tab: &str, body: &str) -> String {
    format!(
        "POST {PATH} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
         X-Dcv2-Token: {token}\r\nX-Dcv2-Tab: {tab}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn header_safe(text: &str) -> bool {
    !text.is_empty() && text.len() <= 200 && text.chars().all(|c| c.is_ascii_graphic())
}

/// Sends `stdin` to the app. Every failure is silent: the hook must never
/// fail Claude's run.
pub fn forward(
    port: Option<&str>,
    token: Option<&str>,
    tab: Option<&str>,
    stdin: &mut dyn Read,
) -> bool {
    let (Some(port), Some(token), Some(tab)) = (port, token, tab) else {
        return false;
    };
    let Ok(port) = port.trim().parse::<u16>() else {
        return false;
    };
    if port == 0 || !header_safe(token) || !header_safe(tab) {
        return false;
    }
    let mut raw = Vec::new();
    if stdin.take(MAX_INPUT).read_to_end(&mut raw).is_err() {
        return false;
    }
    let Ok(payload) = serde_json::from_slice::<Value>(&raw) else {
        return false;
    };
    let body = reduce(&payload).to_string();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, STEP_TIMEOUT) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(STEP_TIMEOUT));
    let _ = stream.set_read_timeout(Some(STEP_TIMEOUT));
    if stream
        .write_all(request(port, token, tab, &body).as_bytes())
        .is_err()
    {
        return false;
    }
    let mut status = [0u8; 12];
    stream.read(&mut status).is_ok_and(|n| n > 0)
}

/// `dcv2-hook`'s main: env + stdin, then exit 0 whatever happened.
pub fn run_from_env() {
    let port = std::env::var(PORT_ENV).ok();
    let token = std::env::var(TOKEN_ENV).ok();
    let tab = std::env::var(TAB_ENV).ok();
    let mut stdin = std::io::stdin();
    let _ = forward(
        port.as_deref(),
        token.as_deref(),
        tab.as_deref(),
        &mut stdin,
    );
}
