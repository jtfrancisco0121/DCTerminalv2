//! Claude Code transcripts (`projects/<dir>/<session>.jsonl` and
//! `<session>/subagents/agent-<id>.jsonl`) to schema-1 events.
//!
//! An event holds names, ids, paths, exit codes, error kinds and counts, never
//! a prompt, a tool input or an output: `content.ref` points at the line and
//! `flight_content` reads it on demand (redacted). The adapter has no error
//! path for content: a line it cannot read becomes `x.claude.unknown`.

use super::schema::{
    event_id_seq, kind, sha256_hex, AgentRef, CapturePolicy, Content, Event, Provider, SourceRef,
    Usage, SCHEMA,
};
use super::tail::{JsonlTail, Line};
use super::{AgentMeta, SubagentFile};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

const PROVIDER: &str = "claude-code";
/// Tools whose input names the files they change.
const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// The files an edit tool_use input names (`file_path` or `notebook_path`).
pub fn edit_paths(tool: &str, input: &Value) -> Vec<String> {
    if !EDIT_TOOLS.contains(&tool) {
        return Vec::new();
    }
    ["file_path", "notebook_path"]
        .iter()
        .filter_map(|key| input[*key].as_str())
        .map(str::to_string)
        .collect()
}

/// A short hash of a tool input with whitespace collapsed and directory
/// prefixes of absolute paths dropped, so the same call from two checkouts
/// compares equal. Only the hash is stored.
pub fn input_hash(tool: &str, input: &Value) -> String {
    static PATHS: OnceLock<Regex> = OnceLock::new();
    let paths = PATHS.get_or_init(|| Regex::new(r"(?:/[A-Za-z0-9._@+~-]+)+/").unwrap());
    let text = match (tool, input["command"].as_str()) {
        ("Bash", Some(command)) => command.to_string(),
        _ => input.to_string(),
    };
    let text = paths.replace_all(&text, "/").to_string();
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    sha256_hex(collapsed.as_bytes())[..16].to_string()
}

/// `Exit code N` at the start of a tool result.
fn exit_code(text: &str) -> Option<i64> {
    let rest = text.strip_prefix("Exit code ")?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// The text of a tool_result's content (a string or text blocks).
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

/// `PreToolUse:Bash` out of `PreToolUse:Bash hook error: ...`.
fn hook_name(text: &str) -> Option<String> {
    let (head, _) = text.split_once(" hook error:")?;
    (head.starts_with("PreToolUse:") && !head.contains(char::is_whitespace))
        .then(|| head.to_string())
}

fn usage_of(message: &Value) -> Option<Usage> {
    let u = message.get("usage")?;
    let n = |key: &str| u[key].as_u64().unwrap_or(0);
    let cache = &u["cache_creation"];
    let (w5, w1) = (
        cache["ephemeral_5m_input_tokens"].as_u64(),
        cache["ephemeral_1h_input_tokens"].as_u64(),
    );
    // Without the split, count all cache writes as 5-minute ones.
    let (w5, w1) = match (w5, w1) {
        (None, None) => (n("cache_creation_input_tokens"), 0),
        (a, b) => (a.unwrap_or(0), b.unwrap_or(0)),
    };
    Some(Usage {
        input: n("input_tokens"),
        output: n("output_tokens"),
        cache_write5m: w5,
        cache_write1h: w1,
        cache_read: n("cache_read_input_tokens"),
        model: message["model"].as_str().map(str::to_string),
        usd: None,
    })
}

/// One transcript, read to EOF.
struct Source {
    file: String,
    /// `None` for the main log, else the sub-agent id.
    agent: Option<String>,
    lines: Vec<(Line, Option<Value>)>,
}

/// Every complete line, however many 8 MiB reads that takes.
fn read_all(path: &Path) -> Vec<Line> {
    let mut tail = JsonlTail::new(path);
    let mut lines = Vec::new();
    loop {
        let before = tail.offset;
        lines.extend(tail.read_lines());
        if tail.offset == before {
            return lines;
        }
    }
}

fn load(path: &Path, agent: Option<String>) -> Source {
    let lines = read_all(path)
        .into_iter()
        .map(|line| {
            let value = serde_json::from_slice::<Value>(&line.bytes)
                .ok()
                .filter(Value::is_object);
            (line, value)
        })
        .collect();
    Source {
        file: path.to_string_lossy().to_string(),
        agent,
        lines,
    }
}

struct Ctx<'a> {
    source: &'a Source,
    line: &'a Line,
    sha: String,
    session: &'a str,
    version: Option<String>,
    at: String,
    agent: AgentRef,
    seq: usize,
}

impl Ctx<'_> {
    #[allow(clippy::too_many_arguments)]
    fn event(
        &mut self,
        kind: &str,
        native: &str,
        span: Option<&str>,
        status: Option<&str>,
        name: Option<&str>,
        attrs: Map<String, Value>,
        usage: Option<Usage>,
    ) -> Event {
        let id = event_id_seq(&self.source.file, self.line.offset, &self.sha, self.seq);
        self.seq += 1;
        Event {
            schema: SCHEMA,
            id,
            trace: format!("session:{}", self.session),
            session: self.session.to_string(),
            agent: self.agent.clone(),
            provider: Provider {
                name: PROVIDER.into(),
                version: self.version.clone(),
                native: native.into(),
            },
            kind: kind.into(),
            at: self.at.clone(),
            span: span.map(str::to_string),
            status: status.map(str::to_string),
            name: name.map(str::to_string),
            attrs,
            usage,
            content: Content {
                source: SourceRef {
                    file: self.source.file.clone(),
                    offset: self.line.offset,
                    len: self.line.len,
                    sha256: self.sha.clone(),
                },
                policy: CapturePolicy::Metadata,
                redactions: 0,
            },
            derived: None,
        }
    }
}

fn attrs(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

/// What the whole session knows before a line is mapped.
#[derive(Default)]
struct Facts {
    /// tool_use id → tool name.
    tools: HashMap<String, String>,
    /// message id → the usage of its last line (a split message streams).
    usage: HashMap<String, Usage>,
}

fn gather(sources: &[Source]) -> Facts {
    let mut facts = Facts::default();
    for source in sources {
        for (_, value) in &source.lines {
            let Some(v) = value else { continue };
            if v["type"] != "assistant" {
                continue;
            }
            if let (Some(id), Some(usage)) = (v["message"]["id"].as_str(), usage_of(&v["message"]))
            {
                facts.usage.insert(id.to_string(), usage);
            }
            for block in v["message"]["content"].as_array().into_iter().flatten() {
                if block["type"] == "tool_use" {
                    if let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str()) {
                        facts.tools.insert(id.to_string(), name.to_string());
                    }
                }
            }
        }
    }
    facts
}

/// The events of one session: its main log and its sub-agent logs, sorted by
/// time, then file, then offset. The same files give the same events (ids
/// come from the position and the line hash).
pub fn events_for(main: &Path, subagents: &[SubagentFile]) -> Vec<Event> {
    let mut sources = vec![load(main, None)];
    sources.extend(
        subagents
            .iter()
            .map(|s| load(&s.path, Some(s.agent_id.clone()))),
    );
    let facts = gather(&sources);
    let metas: HashMap<&str, &AgentMeta> = subagents
        .iter()
        .filter_map(|s| s.meta.as_ref().map(|m| (s.agent_id.as_str(), m)))
        .collect();
    let session = main
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut events = Vec::new();
    let mut seen_messages: std::collections::HashSet<String> = Default::default();
    for source in &sources {
        let ats = line_times(source);
        for (index, (line, value)) in source.lines.iter().enumerate() {
            let at = ats[index].clone();
            let agent = match &source.agent {
                None => AgentRef::main(),
                Some(id) => {
                    let agent_type = metas.get(id.as_str()).and_then(|m| m.agent_type.clone());
                    AgentRef {
                        id: id.clone(),
                        parent: Some("main".into()),
                        kind: "subagent".into(),
                        role: agent_type
                            .as_deref()
                            .map(|t| t.strip_prefix("dcv2:").unwrap_or(t).to_string()),
                        agent_type,
                        pr: None,
                    }
                }
            };
            let mut ctx = Ctx {
                source,
                line,
                sha: sha256_hex(&line.bytes),
                session: value
                    .as_ref()
                    .and_then(|v| v["sessionId"].as_str())
                    .unwrap_or(&session),
                version: value
                    .as_ref()
                    .and_then(|v| v["version"].as_str())
                    .map(str::to_string),
                at,
                agent,
                seq: 0,
            };
            if source.agent.is_none() && index == 0 {
                events.push(ctx.event(
                    kind::SESSION_START,
                    "session",
                    None,
                    None,
                    None,
                    Map::new(),
                    None,
                ));
            }
            match value {
                None => events.push(ctx.event(
                    kind::UNKNOWN,
                    "invalid",
                    None,
                    None,
                    None,
                    Map::new(),
                    None,
                )),
                Some(v) => map_line(v, &mut ctx, &facts, &mut seen_messages, &mut events),
            }
        }
    }
    // Stable: events of one line keep the order they were made in.
    events.sort_by(|a, b| {
        (&a.at, &a.content.source.file, a.content.source.offset).cmp(&(
            &b.at,
            &b.content.source.file,
            b.content.source.offset,
        ))
    });
    link_parents(&mut events);
    events
}

/// The time of each line: its own `timestamp`, else the nearest earlier
/// one, else the first later one (lines like `last-prompt` have none).
fn line_times(source: &Source) -> Vec<String> {
    let own: Vec<Option<String>> = source
        .lines
        .iter()
        .map(|(_, v)| {
            v.as_ref()
                .and_then(|v| v["timestamp"].as_str())
                .map(str::to_string)
        })
        .collect();
    let first = own.iter().flatten().next().cloned().unwrap_or_default();
    let mut last = first;
    own.into_iter()
        .map(|at| {
            if let Some(at) = at {
                last = at;
            }
            last.clone()
        })
        .collect()
}

/// A sub-agent's parent is the agent whose spawn result named it.
fn link_parents(events: &mut [Event]) {
    let parents: HashMap<String, String> = events
        .iter()
        .filter(|e| e.kind == kind::TOOL_END)
        .filter_map(|e| {
            Some((
                e.attrs.get("agentId")?.as_str()?.to_string(),
                e.agent.id.clone(),
            ))
        })
        .collect();
    for event in events.iter_mut() {
        if event.agent.kind == "subagent" {
            if let Some(parent) = parents.get(&event.agent.id) {
                event.agent.parent = Some(parent.clone());
            }
        }
    }
}

fn map_line(
    v: &Value,
    ctx: &mut Ctx,
    facts: &Facts,
    seen: &mut std::collections::HashSet<String>,
    out: &mut Vec<Event>,
) {
    let kind_of = v["type"].as_str().unwrap_or("");
    let message = &v["message"];
    match kind_of {
        "user" => match &message["content"] {
            Value::String(_) => out.push(ctx.event(
                kind::TURN_START,
                "user.text",
                None,
                None,
                None,
                Map::new(),
                None,
            )),
            Value::Array(blocks) => {
                let mut any = false;
                for block in blocks.iter().filter(|b| b["type"] == "tool_result") {
                    any = true;
                    let event = tool_result(v, block, ctx, facts);
                    out.push(event);
                }
                let has_text = blocks.iter().any(|b| b["type"] == "text");
                if !any {
                    out.push(ctx.event(
                        if has_text {
                            kind::TURN_START
                        } else {
                            kind::UNKNOWN
                        },
                        "user.text",
                        None,
                        None,
                        None,
                        Map::new(),
                        None,
                    ));
                }
            }
            _ => unknown(ctx, "user", out),
        },
        "assistant" if v["isApiErrorMessage"] == json!(true) => {
            let mut a = Map::new();
            if let Some(error) = v["error"].as_str() {
                a.insert("errorKind".into(), json!(error));
            }
            if let Some(status) = v["apiErrorStatus"].as_u64() {
                a.insert("status".into(), json!(status));
            }
            out.push(ctx.event(
                kind::ERROR,
                "assistant.api_error",
                None,
                Some("error"),
                None,
                a,
                None,
            ));
        }
        "assistant" => {
            let message_id = message["id"].as_str().map(str::to_string);
            let first = match &message_id {
                Some(id) => seen.insert(id.clone()),
                None => true,
            };
            if first {
                let usage = message_id
                    .as_ref()
                    .and_then(|id| facts.usage.get(id).cloned())
                    .or_else(|| usage_of(message));
                let mut a = Map::new();
                if let Some(id) = &message_id {
                    a.insert("messageId".into(), json!(id));
                }
                out.push(ctx.event(
                    kind::LLM_CALL,
                    "assistant.message",
                    None,
                    None,
                    message["model"].as_str(),
                    a,
                    usage,
                ));
            }
            for block in message["content"].as_array().into_iter().flatten() {
                if block["type"] == "tool_use" {
                    tool_use(block, ctx, out);
                }
            }
        }
        "system" if v["subtype"] == "stop_hook_summary" => out.push(ctx.event(
            kind::TURN_END,
            "system.stop_hook_summary",
            None,
            None,
            None,
            Map::new(),
            None,
        )),
        other => unknown(ctx, if other.is_empty() { "invalid" } else { other }, out),
    }
}

fn unknown(ctx: &mut Ctx, native: &str, out: &mut Vec<Event>) {
    out.push(ctx.event(kind::UNKNOWN, native, None, None, None, Map::new(), None));
}

fn tool_use(block: &Value, ctx: &mut Ctx, out: &mut Vec<Event>) {
    let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str()) else {
        return;
    };
    let input = &block["input"];
    let files = edit_paths(name, input);
    let mut a = Map::new();
    a.insert("inputHash".into(), json!(input_hash(name, input)));
    if !files.is_empty() {
        a.insert("files".into(), json!(files));
    }
    out.push(ctx.event(
        kind::TOOL_START,
        "assistant.tool_use",
        Some(id),
        None,
        Some(name),
        a,
        None,
    ));
    if !files.is_empty() {
        out.push(ctx.event(
            kind::FILE_CHANGE,
            "assistant.tool_use",
            Some(id),
            None,
            Some(name),
            attrs(&[("files", json!(files))]),
            None,
        ));
    }
    if matches!(name, "Agent" | "Task") {
        let mut a = Map::new();
        if let Some(agent_type) = input["subagent_type"].as_str() {
            a.insert("agentType".into(), json!(agent_type));
        }
        out.push(ctx.event(
            kind::AGENT_SPAWN,
            "assistant.tool_use",
            Some(id),
            None,
            Some(name),
            a,
            None,
        ));
    }
}

fn tool_result(line: &Value, block: &Value, ctx: &mut Ctx, facts: &Facts) -> Event {
    let id = block["tool_use_id"].as_str();
    let name = id.and_then(|i| facts.tools.get(i)).map(String::as_str);
    let text = result_text(&block["content"]);
    let is_error = block["is_error"] == json!(true);
    let hook = hook_name(&text);
    let denial = line["toolDenialKind"].as_str();
    let blocked = denial.is_some() || hook.is_some();
    let mut a = Map::new();
    if let Some(code) = exit_code(&text) {
        a.insert("exit".into(), json!(code));
    }
    if blocked {
        if let Some(denial) = denial {
            a.insert("denialKind".into(), json!(denial));
        }
        if let Some(hook) = hook {
            a.insert("hook".into(), json!(hook));
        }
        if text.contains("Blocked by the dcv2 guard") {
            a.insert("dcv2Guard".into(), json!(true));
        }
    }
    let result = &line["toolUseResult"];
    if let Some(agent_id) = result["agentId"].as_str() {
        a.insert("agentId".into(), json!(agent_id));
        if let Some(agent_type) = result["agentType"].as_str() {
            a.insert("agentType".into(), json!(agent_type));
        }
    }
    let (ev_kind, status) = if blocked {
        (kind::TOOL_BLOCKED, "blocked")
    } else if is_error {
        (kind::TOOL_END, "error")
    } else {
        (kind::TOOL_END, "ok")
    };
    ctx.event(ev_kind, "user.tool_result", id, Some(status), name, a, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::path::PathBuf;

    const TOY: &str = "src/fixtures/claude-2.1.287/projects/-work-toy";
    const FAILING: &str = "5b0d6e0e-3a52-4c61-9d1f-7a2c4e8b9f10";
    const SELFTEST: &str = "de6a6329-de0f-49da-960b-f50fed9236e4";

    fn toy() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(TOY)
    }

    /// The events of a recorded session with its sub-agents.
    fn recorded(session: &str) -> (PathBuf, Vec<Event>) {
        let main = toy().join(format!("{session}.jsonl"));
        let dir = toy().join(session).join("subagents");
        let mut subs = Vec::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(id) = name
                .strip_prefix("agent-")
                .and_then(|n| n.strip_suffix(".jsonl"))
            {
                let meta = std::fs::read_to_string(entry.path().with_extension("meta.json"))
                    .ok()
                    .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                    .map(|v| AgentMeta::from_json(&v));
                subs.push(SubagentFile {
                    agent_id: id.to_string(),
                    path: entry.path(),
                    meta,
                });
            }
        }
        let events = events_for(&main, &subs);
        (main, events)
    }

    fn lines_of(file: &Path) -> Vec<Value> {
        std::fs::read_to_string(file)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn all_files(session: &str) -> Vec<PathBuf> {
        let mut files = vec![toy().join(format!("{session}.jsonl"))];
        let dir = toy().join(session).join("subagents");
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            if entry.path().extension().is_some_and(|e| e == "jsonl") {
                files.push(entry.path());
            }
        }
        files
    }

    #[test]
    fn every_tool_use_and_result_maps_to_one_event_with_a_matching_ref() {
        for session in [SELFTEST, FAILING] {
            let (_, events) = recorded(session);
            let mut uses = 0;
            let mut results = 0;
            for file in all_files(session) {
                for line in lines_of(&file) {
                    for block in line["message"]["content"].as_array().into_iter().flatten() {
                        let id = block["id"]
                            .as_str()
                            .or_else(|| block["tool_use_id"].as_str());
                        let n = |kinds: &[&str]| {
                            events
                                .iter()
                                .filter(|e| {
                                    kinds.contains(&e.kind.as_str())
                                        && e.span.as_deref() == id
                                        && e.provider.native
                                            == if block["type"] == "tool_use" {
                                                "assistant.tool_use"
                                            } else {
                                                "user.tool_result"
                                            }
                                })
                                .count()
                        };
                        match block["type"].as_str() {
                            Some("tool_use") => {
                                uses += 1;
                                assert_eq!(n(&[kind::TOOL_START]), 1, "{id:?}");
                            }
                            Some("tool_result") => {
                                results += 1;
                                assert_eq!(n(&[kind::TOOL_END, kind::TOOL_BLOCKED]), 1, "{id:?}");
                            }
                            _ => {}
                        }
                    }
                }
            }
            assert!(uses >= 1 && results >= 1, "{session}");
            for event in &events {
                let r = &event.content.source;
                let bytes = std::fs::read(&r.file).unwrap();
                let at = r.offset as usize;
                let line = &bytes[at..at + r.len as usize];
                assert_eq!(sha256_hex(line), r.sha256, "{}", event.id);
            }
        }
    }

    /// Every string of a JSON value, keys excluded.
    fn texts(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Array(a) => a.iter().for_each(|x| texts(x, out)),
            Value::Object(m) => m.values().for_each(|x| texts(x, out)),
            _ => {}
        }
    }

    #[test]
    fn no_prompt_tool_input_or_output_text_is_held_in_any_event() {
        for session in [SELFTEST, FAILING] {
            let (_, events) = recorded(session);
            let json = serde_json::to_string(&events).unwrap();
            let mut secrets = Vec::new();
            for file in all_files(session) {
                for line in lines_of(&file) {
                    let message = &line["message"];
                    if let Value::String(prompt) = &message["content"] {
                        secrets.push(prompt.clone());
                    }
                    for block in message["content"].as_array().into_iter().flatten() {
                        // Names and paths are attrs by design; the rest is content.
                        if let Value::Object(input) = &block["input"] {
                            for (key, value) in input {
                                if !["subagent_type", "model", "file_path"].contains(&key.as_str())
                                {
                                    texts(value, &mut secrets);
                                }
                            }
                        }
                        texts(&block["content"], &mut secrets);
                        texts(&block["text"], &mut secrets);
                    }
                    let result = &line["toolUseResult"];
                    if result.is_object() {
                        texts(&result["content"], &mut secrets);
                        texts(&result["prompt"], &mut secrets);
                    } else {
                        texts(result, &mut secrets);
                    }
                }
            }
            let long: Vec<&String> = secrets.iter().filter(|s| s.chars().count() >= 8).collect();
            assert!(long.len() >= 3, "{session}: {}", long.len());
            for text in long {
                let escaped = serde_json::to_string(text).unwrap();
                let escaped = &escaped[1..escaped.len() - 1];
                assert!(!json.contains(escaped), "{session} leaks {text:?}");
            }
            assert!(!json.contains("Blocked by the dcv2 guard"));
        }
    }

    #[test]
    fn the_failing_session_has_a_guard_block_and_an_exit_code_failure() {
        let (_, events) = recorded(FAILING);
        let blocked: Vec<&Event> = events
            .iter()
            .filter(|e| e.kind == kind::TOOL_BLOCKED)
            .collect();
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].status.as_deref(), Some("blocked"));
        assert_eq!(blocked[0].name.as_deref(), Some("Bash"));
        assert_eq!(blocked[0].attrs["denialKind"], "permission-rule");
        assert_eq!(blocked[0].attrs["hook"], "PreToolUse:Bash");
        assert_eq!(blocked[0].attrs["dcv2Guard"], true);
        let ends: Vec<&Event> = events.iter().filter(|e| e.kind == kind::TOOL_END).collect();
        let statuses: Vec<_> = ends.iter().map(|e| e.status.as_deref()).collect();
        assert_eq!(statuses, [Some("error"), Some("ok")]);
        assert_eq!(ends[0].attrs["exit"], 1);
        let edits: Vec<&Event> = events
            .iter()
            .filter(|e| e.kind == kind::FILE_CHANGE)
            .collect();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].attrs["files"], json!(["/work/toy/src/add.js"]));
        assert_eq!(events[0].kind, kind::SESSION_START);
    }

    #[test]
    fn a_spawn_links_to_its_sub_agent_in_the_recorded_session() {
        let (_, events) = recorded(SELFTEST);
        let spawn = events.iter().find(|e| e.kind == kind::AGENT_SPAWN).unwrap();
        assert_eq!(spawn.attrs["agentType"], "dcv2:plan-reviewer");
        let end = events
            .iter()
            .find(|e| e.kind == kind::TOOL_END && e.span == spawn.span)
            .unwrap();
        assert_eq!(end.attrs["agentId"], "a2f2a89910f21f43e");
        let sub = events.iter().find(|e| e.agent.kind == "subagent").unwrap();
        assert_eq!(sub.agent.id, "a2f2a89910f21f43e");
        assert_eq!(sub.agent.parent.as_deref(), Some("main"));
        assert_eq!(sub.agent.role.as_deref(), Some("plan-reviewer"));
        // One llm.call per message id, even when a message spans lines.
        let calls: Vec<&str> = events
            .iter()
            .filter(|e| e.kind == kind::LLM_CALL)
            .filter_map(|e| e.attrs["messageId"].as_str())
            .collect();
        let mut unique = calls.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(calls.len(), unique.len());
        assert!(events
            .iter()
            .filter(|e| e.kind == kind::LLM_CALL)
            .all(|e| e.usage.is_some()));
    }

    fn write(dir: &TempDir, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn garbage_and_unknown_lines_become_unknown_events_never_errors() {
        let dir = TempDir::new("flight_claude_unknown");
        let main = write(
            &dir,
            "s.jsonl",
            "not json at all\n[1,2]\n{\"type\":\"future-thing\",\"timestamp\":\"2026-10-22T10:00:00Z\"}\n{\"type\":\"cost-state\"}\n",
        );
        let events = events_for(&main, &[]);
        let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "session.start",
                "x.claude.unknown",
                "x.claude.unknown",
                "x.claude.unknown",
                "x.claude.unknown"
            ]
        );
        assert!(events.iter().all(|e| !e.content.source.sha256.is_empty()));
    }

    #[test]
    fn a_prompt_written_as_text_blocks_is_a_turn_start() {
        let dir = TempDir::new("flight_claude_text_prompt");
        let main = write(
            &dir,
            "s.jsonl",
            "{\"type\":\"user\",\"timestamp\":\"2026-10-22T10:00:00Z\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]}}\n",
        );
        let events = events_for(&main, &[]);
        assert!(events.iter().any(|e| e.kind == "turn.start"));
        assert!(events.iter().all(|e| e.kind != "x.claude.unknown"));
    }

    #[test]
    fn re_reading_gives_the_same_ids() {
        let (main, first) = recorded(FAILING);
        let again = events_for(&main, &[]);
        assert_eq!(
            first.iter().map(|e| &e.id).collect::<Vec<_>>(),
            again.iter().map(|e| &e.id).collect::<Vec<_>>()
        );
        let mut ids: Vec<&String> = first.iter().map(|e| &e.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), first.len(), "ids are unique");
    }

    #[test]
    fn an_api_error_line_is_one_error_event_and_no_model_call() {
        let dir = TempDir::new("flight_claude_api_error");
        let line = |error: &str, status: Option<u64>| {
            let mut v = json!({"type": "assistant", "isApiErrorMessage": true, "error": error,
                "timestamp": "2026-10-22T10:00:00Z", "version": "2.1.287",
                "message": {"id": "msg_x", "model": "<synthetic>",
                    "content": [{"type": "text", "text": "API Error: rate limited, try again later"}],
                    "usage": {"input_tokens": 0, "output_tokens": 0}}});
            if let Some(status) = status {
                v["apiErrorStatus"] = json!(status);
            }
            v.to_string() + "\n"
        };
        let main = write(&dir, "a.jsonl", &line("rate_limit", Some(429)));
        let events = events_for(&main, &[]);
        let errors: Vec<&Event> = events.iter().filter(|e| e.kind == kind::ERROR).collect();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].attrs["errorKind"], "rate_limit");
        assert_eq!(errors[0].attrs["status"], 429);
        assert!(events.iter().all(|e| e.kind != kind::LLM_CALL));
        assert!(!serde_json::to_string(&events)
            .unwrap()
            .contains("rate limited"));

        let main = write(&dir, "b.jsonl", &line("server_error", None));
        let events = events_for(&main, &[]);
        let error = events.iter().find(|e| e.kind == kind::ERROR).unwrap();
        assert_eq!(error.attrs["errorKind"], "server_error");
        assert!(!error.attrs.contains_key("status"));
    }

    #[test]
    fn edit_tools_list_their_paths_and_a_repeated_call_hashes_the_same() {
        assert_eq!(
            edit_paths("Edit", &json!({"file_path": "/a/b.rs"})),
            ["/a/b.rs"]
        );
        assert_eq!(
            edit_paths("NotebookEdit", &json!({"notebook_path": "/a/n.ipynb"})),
            ["/a/n.ipynb"]
        );
        assert!(edit_paths("Bash", &json!({"file_path": "/a"})).is_empty());
        let a = input_hash("Bash", &json!({"command": "cd /x/one/proj  &&  make test"}));
        let b = input_hash("Bash", &json!({"command": "cd /y/two/proj && make   test"}));
        assert_eq!(a, b, "whitespace and path prefixes are normalized");
        assert_ne!(a, input_hash("Bash", &json!({"command": "make build"})));
    }
}
