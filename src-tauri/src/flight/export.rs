//! OTLP export (AF12): a Trace as one OpenTelemetry `ExportTraceServiceRequest`, in OTLP/HTTP
//! JSON or OTLP/HTTP protobuf, with the GenAI semantic conventions (`SEMCONV`).
//!
//! Pure: no IO, no process, no network. The host (`host.rs`) hands in the merged Events, the agent
//! trees, the failures and, only for the policy `full`, the redacted text of the tool calls, and
//! sends or saves the bytes this module builds. The protobuf writer is hand-written (no `prost`);
//! its field numbers live in the one table [`field`].
//!
//! Mapping: `docs/launch.md` (Export), from the BMAD `record-formats.md`.

use super::correlate;
use super::schema::{
    attr, kind, sha256_hex, AgentNode, Event, Failure, Span, ORCHESTRATION, SCHEMA,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};

/// The GenAI semantic conventions version the attributes follow (also `agentflight.semconv`).
pub const SEMCONV: &str = "1.37.0";
/// A payload larger than this is refused.
pub const MAX_PAYLOAD: usize = 32 * 1024 * 1024;
/// Tool arguments or results (policy `full`) are cut at this many bytes each.
pub const MAX_CONTENT: usize = 16 * 1024;
/// Shown when the payload is over [`MAX_PAYLOAD`].
pub const TOO_LARGE: &str = "This trace is larger than 32 MiB; export a single session instead.";

/// The OTLP protobuf field numbers this writer uses (opentelemetry-proto `trace/v1`,
/// `resource/v1`, `common/v1`, `collector/trace/v1`).
pub mod field {
    pub const REQUEST_RESOURCE_SPANS: u32 = 1;
    pub const RESOURCE_SPANS_RESOURCE: u32 = 1;
    pub const RESOURCE_SPANS_SCOPE_SPANS: u32 = 2;
    pub const RESOURCE_ATTRIBUTES: u32 = 1;
    pub const SCOPE_SPANS_SCOPE: u32 = 1;
    pub const SCOPE_SPANS_SPANS: u32 = 2;
    pub const SCOPE_NAME: u32 = 1;
    pub const SCOPE_VERSION: u32 = 2;
    pub const SPAN_TRACE_ID: u32 = 1;
    pub const SPAN_SPAN_ID: u32 = 2;
    pub const SPAN_PARENT_SPAN_ID: u32 = 4;
    pub const SPAN_NAME: u32 = 5;
    pub const SPAN_KIND: u32 = 6;
    pub const SPAN_START_TIME: u32 = 7;
    pub const SPAN_END_TIME: u32 = 8;
    pub const SPAN_ATTRIBUTES: u32 = 9;
    pub const SPAN_EVENTS: u32 = 11;
    pub const SPAN_STATUS: u32 = 15;
    pub const EVENT_TIME: u32 = 1;
    pub const EVENT_NAME: u32 = 2;
    pub const EVENT_ATTRIBUTES: u32 = 3;
    pub const STATUS_MESSAGE: u32 = 2;
    pub const STATUS_CODE: u32 = 3;
    pub const KEY_VALUE_KEY: u32 = 1;
    pub const KEY_VALUE_VALUE: u32 = 2;
    pub const ANY_STRING: u32 = 1;
    pub const ANY_BOOL: u32 = 2;
    pub const ANY_INT: u32 = 3;
    pub const ANY_ARRAY: u32 = 5;
    pub const ARRAY_VALUES: u32 = 1;
}

const SPAN_KIND_INTERNAL: i32 = 1;
const SPAN_KIND_CLIENT: i32 = 3;
const STATUS_OK: i32 = 1;
const STATUS_ERROR: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportPolicy {
    /// No tool text leaves.
    Metadata,
    /// Redacted tool arguments and results, capped at [`MAX_CONTENT`] each.
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    Json,
    Protobuf,
}

impl Encoding {
    pub fn content_type(self) -> &'static str {
        match self {
            Encoding::Json => "application/json",
            Encoding::Protobuf => "application/x-protobuf",
        }
    }
}

/// The already redacted text behind one event, read by the host (policy `full` only).
#[derive(Debug, Clone, Default)]
pub struct ToolText {
    pub text: String,
    pub redactions: u32,
}

/// What the host hands in.
#[derive(Debug, Clone, Default)]
pub struct ExportInput {
    /// The Trace id: the run id, or the session id outside runs.
    pub trace: String,
    /// The merged Events in Trace order.
    pub events: Vec<Event>,
    /// The agent tree of each session.
    pub agents: Vec<AgentNode>,
    pub failures: Vec<Failure>,
    /// Text by event id (`tool.start`: the arguments, `tool.end`: the result).
    pub content: BTreeMap<String, ToolText>,
}

/// How many spans or events carry one attribute key, and how many values it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldCount {
    pub field: String,
    pub count: usize,
    pub leaves: usize,
}

/// The bytes and what the Preview shows about them.
#[derive(Debug, Clone)]
pub struct Payload {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    pub spans: usize,
    pub events: usize,
    pub fields: Vec<FieldCount>,
    /// sha256 of `bytes`.
    pub digest: String,
}

// ---- the in-memory OTLP model ----

#[derive(Debug, Clone, PartialEq)]
enum Any {
    Str(String),
    Int(i64),
    Bool(bool),
    Array(Vec<Any>),
}

type Attrs = Vec<(String, Any)>;

#[derive(Debug, Clone)]
struct SpanEvent {
    time: u64,
    name: String,
    attrs: Attrs,
}

#[derive(Debug, Clone)]
struct OSpan {
    id: [u8; 8],
    parent: Option<[u8; 8]>,
    name: String,
    kind: i32,
    start: u64,
    end: u64,
    attrs: Attrs,
    events: Vec<SpanEvent>,
    status: Option<(i32, String)>,
}

struct Model {
    trace_id: [u8; 16],
    resource: Attrs,
    spans: Vec<OSpan>,
}

fn s(text: impl Into<String>) -> Any {
    Any::Str(text.into())
}

fn hash(text: &str) -> Vec<u8> {
    Sha256::digest(text.as_bytes()).to_vec()
}

fn span_id(key: &str) -> [u8; 8] {
    let mut id = [0u8; 8];
    id.copy_from_slice(&hash(key)[..8]);
    if id == [0u8; 8] {
        id[7] = 1;
    }
    id
}

fn unix_nanos(at: &str) -> u64 {
    chrono::DateTime::parse_from_rfc3339(at)
        .ok()
        .and_then(|t| t.timestamp_nanos_opt())
        .map_or(0, |n| n.max(0) as u64)
}

fn clip(text: &str) -> String {
    if text.len() <= MAX_CONTENT {
        return text.to_string();
    }
    let mut cut = MAX_CONTENT;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}\n[truncated]\n", &text[..cut])
}

/// A tool span's id comes from its agent and `tool_use` id, not from the Event id: Event ids hash the
/// transcript's absolute path, so they differ between machines for the same Trace.
fn tool_span_id(agent: &str, tool_use: &str) -> [u8; 8] {
    span_id(&format!("tool:{agent}:{tool_use}"))
}

fn agent_key(session: &str, id: &str) -> String {
    if id == "main" {
        format!("agent:{session}:main")
    } else {
        format!("agent:{id}")
    }
}

/// An agent of the Trace: from the trees, or made up for an Event whose agent no tree names.
struct AgentInfo {
    key: String,
    id: String,
    name: String,
    pr: Option<String>,
    pr_label: Option<String>,
    spawn_span: Option<String>,
    parent_key: Option<String>,
}

fn collect_agents(input: &ExportInput) -> Vec<AgentInfo> {
    fn walk(node: &AgentNode, parent: Option<&str>, out: &mut Vec<AgentInfo>) {
        let key = agent_key(&node.session, &node.id);
        out.push(AgentInfo {
            key: key.clone(),
            id: node.id.clone(),
            name: node.agent_type.clone().unwrap_or_else(|| "main".into()),
            pr: node.pr.clone(),
            pr_label: node.pr_derived.as_ref().map(|d| d.label.clone()),
            spawn_span: node.spawn_span.clone(),
            parent_key: parent.map(str::to_string),
        });
        for child in &node.children {
            walk(child, Some(&key), out);
        }
    }
    let mut out = Vec::new();
    for tree in &input.agents {
        walk(tree, None, &mut out);
    }
    // Events of an agent no tree names (a snapshot has bare trees): a span of their own.
    let mut seen: Vec<String> = out.iter().map(|a| a.key.clone()).collect();
    for e in input.events.iter().filter(|e| e.agent.id != ORCHESTRATION) {
        let key = agent_key(&e.session, &e.agent.id);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key.clone());
        let parent = (e.agent.id != "main").then(|| agent_key(&e.session, "main"));
        out.push(AgentInfo {
            key,
            id: e.agent.id.clone(),
            name: e
                .agent
                .agent_type
                .clone()
                .unwrap_or_else(|| e.agent.id.clone()),
            pr: e.agent.pr.clone(),
            pr_label: None,
            spawn_span: None,
            parent_key: parent,
        });
    }
    out
}

fn orchestration_attrs(e: &Event) -> Attrs {
    let mut out: Attrs = Vec::new();
    if let Some(status) = &e.status {
        out.push(("agentflight.status".into(), s(status.clone())));
    }
    if let Some(name) = &e.name {
        out.push(("agentflight.name".into(), s(name.clone())));
    }
    for key in [
        attr::SOURCE,
        attr::PR,
        attr::ROUND,
        attr::VERDICT,
        attr::HEAD,
        attr::EXIT_CODE,
        attr::CI,
        attr::PR_STATE,
        attr::RULE,
        attr::TOOL,
        attr::FROM,
        attr::TO,
        attr::REQUEST_KIND,
        attr::RESULT,
        attr::AGENT,
    ] {
        match e.attrs.get(key) {
            Some(Value::String(v)) => out.push((format!("agentflight.{key}"), s(v.clone()))),
            Some(Value::Number(n)) if n.is_i64() => out.push((
                format!("agentflight.{key}"),
                Any::Int(n.as_i64().unwrap_or(0)),
            )),
            Some(Value::Bool(b)) => out.push((format!("agentflight.{key}"), Any::Bool(*b))),
            _ => {}
        }
    }
    out
}

fn agent_event_attrs(e: &Event) -> Attrs {
    let mut out: Attrs = Vec::new();
    if let Some(status) = &e.status {
        out.push(("agentflight.status".into(), s(status.clone())));
    }
    if let Some(name) = &e.name {
        out.push(("agentflight.name".into(), s(name.clone())));
    }
    out
}

fn model(input: &ExportInput, policy: ExportPolicy) -> Model {
    let trace_id = {
        let mut id = [0u8; 16];
        id.copy_from_slice(&hash(&input.trace)[..16]);
        id
    };
    let first = input
        .events
        .iter()
        .map(|e| unix_nanos(&e.at))
        .min()
        .unwrap_or(0);
    let last = input
        .events
        .iter()
        .map(|e| unix_nanos(&e.at))
        .max()
        .unwrap_or(0);
    let resource: Attrs = vec![
        ("service.name".into(), s("agentflight")),
        ("agentflight.trace".into(), s(input.trace.clone())),
        ("agentflight.schema".into(), Any::Int(i64::from(SCHEMA))),
        ("agentflight.semconv".into(), s(SEMCONV)),
    ];
    let tool_spans: Vec<Span> = correlate::spans(&input.events);
    let by_event: HashMap<&str, &Event> = input.events.iter().map(|e| (e.id.as_str(), e)).collect();
    let spawn_parent: HashMap<&str, [u8; 8]> = tool_spans
        .iter()
        .map(|t| (t.span.as_str(), tool_span_id(&t.agent, &t.span)))
        .collect();
    let failure_label: HashMap<&str, &str> = input
        .failures
        .iter()
        .map(|f| (f.id.as_str(), f.derived.label.as_str()))
        .collect();

    let agents = collect_agents(input);
    let agent_ids: HashMap<&str, [u8; 8]> = agents
        .iter()
        .map(|a| (a.key.as_str(), span_id(&a.key)))
        .collect();
    let mut spans: Vec<OSpan> = Vec::new();

    // The run span: the orchestration Events.
    let orchestration: Vec<&Event> = input
        .events
        .iter()
        .filter(|e| e.agent.id == ORCHESTRATION)
        .collect();
    if !orchestration.is_empty() {
        spans.push(OSpan {
            id: span_id(&format!("run:{}", input.trace)),
            parent: None,
            name: "agentflight.run".into(),
            kind: SPAN_KIND_INTERNAL,
            start: first,
            end: last,
            attrs: vec![("openinference.span.kind".into(), s("CHAIN"))],
            events: orchestration
                .iter()
                .map(|e| SpanEvent {
                    time: unix_nanos(&e.at),
                    name: format!("agentflight.{}", e.kind),
                    attrs: orchestration_attrs(e),
                })
                .collect(),
            status: None,
        });
    }

    // Agent spans, each with its other Events.
    for a in &agents {
        let mine: Vec<&Event> = input
            .events
            .iter()
            .filter(|e| e.agent.id != ORCHESTRATION && agent_key(&e.session, &e.agent.id) == a.key)
            .collect();
        let start = mine
            .iter()
            .map(|e| unix_nanos(&e.at))
            .min()
            .unwrap_or(first);
        let end = mine.iter().map(|e| unix_nanos(&e.at)).max().unwrap_or(last);
        let parent = a
            .spawn_span
            .as_deref()
            .and_then(|sp| spawn_parent.get(sp).copied())
            .or_else(|| {
                a.parent_key
                    .as_deref()
                    .and_then(|k| agent_ids.get(k).copied())
            });
        let mut attrs: Attrs = vec![
            ("gen_ai.operation.name".into(), s("invoke_agent")),
            ("gen_ai.provider.name".into(), s("anthropic")),
            ("gen_ai.agent.id".into(), s(a.id.clone())),
            ("gen_ai.agent.name".into(), s(a.name.clone())),
        ];
        if let Some(pr) = &a.pr {
            attrs.push(("agentflight.pr".into(), s(pr.clone())));
        }
        if let Some(label) = &a.pr_label {
            attrs.push(("agentflight.pr.label".into(), s(label.clone())));
        }
        attrs.push(("openinference.span.kind".into(), s("AGENT")));
        let events = mine
            .iter()
            .filter(|e| {
                !matches!(
                    e.kind.as_str(),
                    kind::TOOL_START | kind::TOOL_END | kind::LLM_CALL
                )
            })
            .map(|e| SpanEvent {
                time: unix_nanos(&e.at),
                name: format!("agentflight.{}", e.kind),
                attrs: agent_event_attrs(e),
            })
            .collect();
        spans.push(OSpan {
            id: agent_ids[a.key.as_str()],
            parent,
            name: format!("invoke_agent {}", a.name),
            kind: SPAN_KIND_INTERNAL,
            start,
            end,
            attrs,
            events,
            status: None,
        });
    }

    let agent_span_of = |e: &Event| -> Option<[u8; 8]> {
        agent_ids
            .get(agent_key(&e.session, &e.agent.id).as_str())
            .copied()
    };

    // Tool spans.
    for t in &tool_spans {
        let Some(start) = by_event.get(t.start.as_str()) else {
            continue;
        };
        let end = t.end.as_deref().and_then(|id| by_event.get(id)).copied();
        let name = t.name.clone().unwrap_or_else(|| "tool".into());
        let mut attrs: Attrs = vec![
            ("gen_ai.operation.name".into(), s("execute_tool")),
            ("gen_ai.tool.name".into(), s(name.clone())),
            ("gen_ai.tool.call.id".into(), s(t.span.clone())),
        ];
        if let Some(files) = start.attrs.get("files").and_then(Value::as_array) {
            let files: Vec<Any> = files
                .iter()
                .filter_map(Value::as_str)
                .map(|f| s(f.to_string()))
                .collect();
            if !files.is_empty() {
                attrs.push(("agentflight.files".into(), Any::Array(files)));
            }
        }
        attrs.push(("openinference.span.kind".into(), s("TOOL")));
        if policy == ExportPolicy::Full {
            let mut redactions = 0u32;
            let mut any = false;
            for (key, event) in [
                ("gen_ai.tool.call.arguments", Some(*start)),
                ("gen_ai.tool.call.result", end),
            ] {
                if let Some(text) = event.and_then(|e| input.content.get(&e.id)) {
                    attrs.push((key.into(), s(clip(&text.text))));
                    redactions += text.redactions;
                    any = true;
                }
            }
            if any {
                attrs.push((
                    "agentflight.redactions".into(),
                    Any::Int(i64::from(redactions)),
                ));
            }
        }
        let status = end.map(|e| match e.status.as_deref() {
            Some("error") | Some("blocked") => {
                let label = failure_label
                    .get(e.id.as_str())
                    .or_else(|| failure_label.get(start.id.as_str()))
                    .map_or_else(|| e.status.clone().unwrap_or_default(), |l| l.to_string());
                (STATUS_ERROR, label)
            }
            _ => (STATUS_OK, String::new()),
        });
        spans.push(OSpan {
            id: tool_span_id(&t.agent, &t.span),
            parent: agent_span_of(start),
            name: format!("execute_tool {name}"),
            kind: SPAN_KIND_INTERNAL,
            start: unix_nanos(&start.at),
            end: end.map_or(last, |e| unix_nanos(&e.at)),
            attrs,
            events: Vec::new(),
            status,
        });
    }

    // LLM calls.
    let mut llm_seen: HashMap<String, usize> = HashMap::new();
    for e in input.events.iter().filter(|e| e.kind == kind::LLM_CALL) {
        let message = e
            .attrs
            .get("messageId")
            .and_then(Value::as_str)
            .map_or_else(|| e.at.clone(), str::to_string);
        let base = format!("llm:{}:{message}", agent_key(&e.session, &e.agent.id));
        let nth = llm_seen.entry(base.clone()).or_insert(0);
        let llm_id = span_id(&format!("{base}:{nth}"));
        *nth += 1;
        let usage = e.usage.clone().unwrap_or_default();
        let model_name = usage.model.clone().unwrap_or_else(|| "unknown".into());
        let at = unix_nanos(&e.at);
        spans.push(OSpan {
            id: llm_id,
            parent: agent_span_of(e),
            name: format!("chat {model_name}"),
            kind: SPAN_KIND_CLIENT,
            start: at,
            end: at,
            attrs: vec![
                ("gen_ai.operation.name".into(), s("chat")),
                ("gen_ai.request.model".into(), s(model_name)),
                (
                    "gen_ai.usage.input_tokens".into(),
                    Any::Int(usage.input as i64),
                ),
                (
                    "gen_ai.usage.output_tokens".into(),
                    Any::Int(usage.output as i64),
                ),
                (
                    "agentflight.usage.cache_read".into(),
                    Any::Int(usage.cache_read as i64),
                ),
                (
                    "agentflight.usage.cache_write".into(),
                    Any::Int((usage.cache_write5m + usage.cache_write1h) as i64),
                ),
                ("openinference.span.kind".into(), s("LLM")),
            ],
            events: Vec::new(),
            status: None,
        });
    }
    Model {
        trace_id,
        resource,
        spans,
    }
}

// ---- JSON ----

fn any_json(v: &Any) -> Value {
    match v {
        Any::Str(t) => json!({"stringValue": t}),
        Any::Int(n) => json!({"intValue": n.to_string()}),
        Any::Bool(b) => json!({"boolValue": b}),
        Any::Array(items) => {
            json!({"arrayValue": {"values": items.iter().map(any_json).collect::<Vec<_>>()}})
        }
    }
}

fn attrs_json(attrs: &Attrs) -> Value {
    Value::Array(
        attrs
            .iter()
            .map(|(k, v)| json!({"key": k, "value": any_json(v)}))
            .collect(),
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn to_json(m: &Model) -> Vec<u8> {
    let spans: Vec<Value> = m
        .spans
        .iter()
        .map(|sp| {
            let mut o = Map::new();
            o.insert("traceId".into(), json!(hex(&m.trace_id)));
            o.insert("spanId".into(), json!(hex(&sp.id)));
            if let Some(p) = sp.parent {
                o.insert("parentSpanId".into(), json!(hex(&p)));
            }
            o.insert("name".into(), json!(sp.name));
            o.insert("kind".into(), json!(sp.kind));
            o.insert("startTimeUnixNano".into(), json!(sp.start.to_string()));
            o.insert("endTimeUnixNano".into(), json!(sp.end.to_string()));
            o.insert("attributes".into(), attrs_json(&sp.attrs));
            if !sp.events.is_empty() {
                let events: Vec<Value> = sp
                    .events
                    .iter()
                    .map(|e| {
                        json!({"timeUnixNano": e.time.to_string(), "name": e.name,
                               "attributes": attrs_json(&e.attrs)})
                    })
                    .collect();
                o.insert("events".into(), Value::Array(events));
            }
            if let Some((code, message)) = &sp.status {
                let mut st = Map::new();
                if !message.is_empty() {
                    st.insert("message".into(), json!(message));
                }
                st.insert("code".into(), json!(code));
                o.insert("status".into(), Value::Object(st));
            }
            Value::Object(o)
        })
        .collect();
    let doc = json!({"resourceSpans": [{
        "resource": {"attributes": attrs_json(&m.resource)},
        "scopeSpans": [{"scope": {"name": "agentflight", "version": "1"}, "spans": spans}]
    }]});
    serde_json::to_vec(&doc).unwrap_or_default()
}

// ---- protobuf ----

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_tag(out: &mut Vec<u8>, number: u32, wire: u8) {
    put_varint(out, (u64::from(number) << 3) | u64::from(wire));
}

fn put_bytes(out: &mut Vec<u8>, number: u32, bytes: &[u8]) {
    put_tag(out, number, 2);
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn put_varint_field(out: &mut Vec<u8>, number: u32, v: u64) {
    put_tag(out, number, 0);
    put_varint(out, v);
}

fn put_fixed64(out: &mut Vec<u8>, number: u32, v: u64) {
    put_tag(out, number, 1);
    out.extend_from_slice(&v.to_le_bytes());
}

fn any_pb(v: &Any) -> Vec<u8> {
    let mut out = Vec::new();
    match v {
        Any::Str(t) => put_bytes(&mut out, field::ANY_STRING, t.as_bytes()),
        Any::Bool(b) => put_varint_field(&mut out, field::ANY_BOOL, u64::from(*b)),
        // int64 is the two's complement varint.
        Any::Int(n) => put_varint_field(&mut out, field::ANY_INT, *n as u64),
        Any::Array(items) => {
            let mut inner = Vec::new();
            for item in items {
                put_bytes(&mut inner, field::ARRAY_VALUES, &any_pb(item));
            }
            put_bytes(&mut out, field::ANY_ARRAY, &inner);
        }
    }
    out
}

fn put_attrs(out: &mut Vec<u8>, number: u32, attrs: &Attrs) {
    for (k, v) in attrs {
        let mut kv = Vec::new();
        put_bytes(&mut kv, field::KEY_VALUE_KEY, k.as_bytes());
        put_bytes(&mut kv, field::KEY_VALUE_VALUE, &any_pb(v));
        put_bytes(out, number, &kv);
    }
}

fn to_protobuf(m: &Model) -> Vec<u8> {
    let mut spans = Vec::new();
    for sp in &m.spans {
        let mut b = Vec::new();
        put_bytes(&mut b, field::SPAN_TRACE_ID, &m.trace_id);
        put_bytes(&mut b, field::SPAN_SPAN_ID, &sp.id);
        if let Some(p) = sp.parent {
            put_bytes(&mut b, field::SPAN_PARENT_SPAN_ID, &p);
        }
        put_bytes(&mut b, field::SPAN_NAME, sp.name.as_bytes());
        put_varint_field(&mut b, field::SPAN_KIND, sp.kind as u64);
        put_fixed64(&mut b, field::SPAN_START_TIME, sp.start);
        put_fixed64(&mut b, field::SPAN_END_TIME, sp.end);
        put_attrs(&mut b, field::SPAN_ATTRIBUTES, &sp.attrs);
        for e in &sp.events {
            let mut eb = Vec::new();
            put_fixed64(&mut eb, field::EVENT_TIME, e.time);
            put_bytes(&mut eb, field::EVENT_NAME, e.name.as_bytes());
            put_attrs(&mut eb, field::EVENT_ATTRIBUTES, &e.attrs);
            put_bytes(&mut b, field::SPAN_EVENTS, &eb);
        }
        if let Some((code, message)) = &sp.status {
            let mut st = Vec::new();
            if !message.is_empty() {
                put_bytes(&mut st, field::STATUS_MESSAGE, message.as_bytes());
            }
            put_varint_field(&mut st, field::STATUS_CODE, *code as u64);
            put_bytes(&mut b, field::SPAN_STATUS, &st);
        }
        put_bytes(&mut spans, field::SCOPE_SPANS_SPANS, &b);
    }
    let mut scope = Vec::new();
    put_bytes(&mut scope, field::SCOPE_NAME, b"agentflight");
    put_bytes(&mut scope, field::SCOPE_VERSION, b"1");
    let mut scope_spans = Vec::new();
    put_bytes(&mut scope_spans, field::SCOPE_SPANS_SCOPE, &scope);
    scope_spans.extend_from_slice(&spans);
    let mut resource = Vec::new();
    put_attrs(&mut resource, field::RESOURCE_ATTRIBUTES, &m.resource);
    let mut rs = Vec::new();
    put_bytes(&mut rs, field::RESOURCE_SPANS_RESOURCE, &resource);
    put_bytes(&mut rs, field::RESOURCE_SPANS_SCOPE_SPANS, &scope_spans);
    let mut out = Vec::new();
    put_bytes(&mut out, field::REQUEST_RESOURCE_SPANS, &rs);
    out
}

// ---- the Preview counts ----

fn leaves(v: &Any) -> usize {
    match v {
        Any::Array(items) => items.iter().map(leaves).sum(),
        _ => 1,
    }
}

fn field_counts(m: &Model) -> Vec<FieldCount> {
    let mut map: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut add = |attrs: &Attrs| {
        for (k, v) in attrs {
            let e = map.entry(k.clone()).or_default();
            e.0 += 1;
            e.1 += leaves(v);
        }
    };
    add(&m.resource);
    for sp in &m.spans {
        add(&sp.attrs);
        for e in &sp.events {
            add(&e.attrs);
        }
    }
    map.into_iter()
        .map(|(field, (count, leaves))| FieldCount {
            field,
            count,
            leaves,
        })
        .collect()
}

/// The Trace as one OTLP request. Deterministic: the same input gives the same bytes.
pub fn build(
    input: &ExportInput,
    policy: ExportPolicy,
    encoding: Encoding,
) -> Result<Payload, String> {
    let m = model(input, policy);
    let bytes = match encoding {
        Encoding::Json => to_json(&m),
        Encoding::Protobuf => to_protobuf(&m),
    };
    if bytes.len() > MAX_PAYLOAD {
        return Err(TOO_LARGE.into());
    }
    Ok(Payload {
        digest: sha256_hex(&bytes),
        content_type: encoding.content_type(),
        spans: m.spans.len(),
        events: m.spans.iter().map(|sp| sp.events.len()).sum(),
        fields: field_counts(&m),
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flight::schema::{
        AgentRef, CapturePolicy, Content, Derived, Provider, SourceRef, Usage,
    };

    fn ev(id: &str, k: &str, at: &str, agent: &str, span: Option<&str>) -> Event {
        Event {
            schema: SCHEMA,
            id: id.into(),
            trace: "r-1".into(),
            session: "s1".into(),
            agent: AgentRef {
                id: agent.into(),
                parent: None,
                kind: "main".into(),
                agent_type: None,
                role: None,
                pr: None,
            },
            provider: Provider {
                name: "claude-code".into(),
                version: None,
                native: "x".into(),
            },
            kind: k.into(),
            at: at.into(),
            span: span.map(str::to_string),
            status: None,
            name: None,
            attrs: Map::new(),
            usage: None,
            content: Content {
                source: SourceRef {
                    file: "f".into(),
                    offset: 0,
                    len: 1,
                    sha256: "x".into(),
                },
                policy: CapturePolicy::Metadata,
                redactions: 0,
            },
            derived: None,
        }
    }

    fn node() -> AgentNode {
        AgentNode {
            id: "main".into(),
            session: "s1".into(),
            kind: "main".into(),
            agent_type: None,
            role: None,
            pr: None,
            pr_derived: None,
            description: None,
            spawn_span: None,
            files: Vec::new(),
            children: Vec::new(),
            derived: Derived {
                label: "exact:session-root".into(),
                evidence: Vec::new(),
            },
        }
    }

    fn small() -> ExportInput {
        let mut start = ev(
            "ev_a",
            kind::TOOL_START,
            "2026-11-05T10:00:01Z",
            "main",
            Some("t1"),
        );
        start.name = Some("Bash".into());
        let mut end = ev(
            "ev_b",
            kind::TOOL_END,
            "2026-11-05T10:00:03Z",
            "main",
            Some("t1"),
        );
        end.status = Some("error".into());
        let mut llm = ev("ev_c", kind::LLM_CALL, "2026-11-05T10:00:02Z", "main", None);
        llm.usage = Some(Usage {
            input: 5,
            output: 7,
            model: Some("m1".into()),
            ..Default::default()
        });
        let mut stage = ev(
            "ev_d",
            kind::STAGE_CHANGE,
            "2026-11-05T10:00:00Z",
            ORCHESTRATION,
            None,
        );
        stage.attrs.insert(attr::TO.into(), json!("building"));
        let mut content = BTreeMap::new();
        content.insert(
            "ev_a".to_string(),
            ToolText {
                text: "echo hi".into(),
                redactions: 1,
            },
        );
        content.insert(
            "ev_b".to_string(),
            ToolText {
                text: "x".repeat(MAX_CONTENT + 10),
                redactions: 0,
            },
        );
        ExportInput {
            trace: "r-1".into(),
            events: vec![stage, start, llm, end],
            agents: vec![node()],
            failures: Vec::new(),
            content,
        }
    }

    /// A minimal protobuf walk: (field number, wire type, payload).
    fn walk(buf: &[u8]) -> Vec<(u32, u8, Vec<u8>)> {
        let mut out = Vec::new();
        let mut i = 0;
        let varint = |i: &mut usize| {
            let mut v = 0u64;
            let mut shift = 0;
            loop {
                let b = buf[*i];
                *i += 1;
                v |= u64::from(b & 0x7f) << shift;
                if b & 0x80 == 0 {
                    return v;
                }
                shift += 7;
            }
        };
        while i < buf.len() {
            let tag = varint(&mut i);
            let (number, wire) = ((tag >> 3) as u32, (tag & 7) as u8);
            match wire {
                0 => {
                    let v = varint(&mut i);
                    out.push((number, wire, v.to_le_bytes().to_vec()));
                }
                1 => {
                    out.push((number, wire, buf[i..i + 8].to_vec()));
                    i += 8;
                }
                2 => {
                    let len = varint(&mut i) as usize;
                    out.push((number, wire, buf[i..i + len].to_vec()));
                    i += len;
                }
                other => panic!("wire type {other}"),
            }
        }
        out
    }

    fn pb_spans(bytes: &[u8]) -> Vec<Vec<(u32, u8, Vec<u8>)>> {
        let rs = &walk(bytes)[0].2;
        let scope_spans = walk(rs).into_iter().find(|f| f.0 == 2).unwrap().2;
        walk(&scope_spans)
            .into_iter()
            .filter(|f| f.0 == 2)
            .map(|f| walk(&f.2))
            .collect()
    }

    fn name_of(span: &[(u32, u8, Vec<u8>)]) -> String {
        String::from_utf8(span.iter().find(|f| f.0 == 5).unwrap().2.clone()).unwrap()
    }

    #[test]
    fn the_protobuf_decodes_to_the_same_spans_names_and_parent_links() {
        let input = small();
        let pb = build(&input, ExportPolicy::Metadata, Encoding::Protobuf).unwrap();
        let spans = pb_spans(&pb.bytes);
        assert_eq!(spans.len(), pb.spans);
        let names: Vec<String> = spans.iter().map(|s| name_of(s)).collect();
        assert_eq!(
            names,
            [
                "agentflight.run",
                "invoke_agent main",
                "execute_tool Bash",
                "chat m1"
            ]
        );
        let id = |s: &Vec<(u32, u8, Vec<u8>)>| s.iter().find(|f| f.0 == 2).unwrap().2.clone();
        let parent = |s: &Vec<(u32, u8, Vec<u8>)>| s.iter().find(|f| f.0 == 4).map(|f| f.2.clone());
        assert_eq!(parent(&spans[0]), None);
        assert_eq!(parent(&spans[1]), None);
        assert_eq!(parent(&spans[2]), Some(id(&spans[1])));
        assert_eq!(parent(&spans[3]), Some(id(&spans[1])));
        // The same spans in JSON.
        let js = build(&input, ExportPolicy::Metadata, Encoding::Json).unwrap();
        let doc: Value = serde_json::from_slice(&js.bytes).unwrap();
        let json_spans = doc["resourceSpans"][0]["scopeSpans"][0]["spans"]
            .as_array()
            .unwrap();
        assert_eq!(json_spans.len(), 4);
        assert_eq!(json_spans[2]["parentSpanId"], json_spans[1]["spanId"]);
        assert_eq!(json_spans[2]["status"]["code"], 2);
        assert_eq!(
            json_spans[3]["startTimeUnixNano"].as_str().map(str::len),
            Some(19)
        );
        assert_eq!(js.content_type, "application/json");
        assert_eq!(pb.content_type, "application/x-protobuf");
    }

    /// Field numbers copied by hand from opentelemetry-proto (trace.proto, resource.proto,
    /// common.proto, trace_service.proto), not derived from the table above.
    #[test]
    fn the_protobuf_field_numbers_equal_the_opentelemetry_proto_ones() {
        assert_eq!(field::REQUEST_RESOURCE_SPANS, 1);
        assert_eq!(field::RESOURCE_SPANS_RESOURCE, 1);
        assert_eq!(field::RESOURCE_SPANS_SCOPE_SPANS, 2);
        assert_eq!(field::SCOPE_SPANS_SCOPE, 1);
        assert_eq!(field::SCOPE_SPANS_SPANS, 2);
        assert_eq!(field::SPAN_TRACE_ID, 1);
        assert_eq!(field::SPAN_SPAN_ID, 2);
        assert_eq!(field::SPAN_PARENT_SPAN_ID, 4);
        assert_eq!(field::SPAN_NAME, 5);
        assert_eq!(field::SPAN_KIND, 6);
        assert_eq!(field::SPAN_START_TIME, 7);
        assert_eq!(field::SPAN_END_TIME, 8);
        assert_eq!(field::SPAN_ATTRIBUTES, 9);
        assert_eq!(field::SPAN_EVENTS, 11);
        assert_eq!(field::SPAN_STATUS, 15);
        assert_eq!(field::EVENT_TIME, 1);
        assert_eq!(field::EVENT_NAME, 2);
        assert_eq!(field::EVENT_ATTRIBUTES, 3);
        assert_eq!(field::STATUS_MESSAGE, 2);
        assert_eq!(field::STATUS_CODE, 3);
        assert_eq!(field::KEY_VALUE_KEY, 1);
        assert_eq!(field::KEY_VALUE_VALUE, 2);
        assert_eq!(field::ANY_STRING, 1);
        assert_eq!(field::ANY_BOOL, 2);
        assert_eq!(field::ANY_INT, 3);
        assert_eq!(field::ANY_ARRAY, 5);
        assert_eq!(field::RESOURCE_ATTRIBUTES, 1);
        assert_eq!(field::SCOPE_NAME, 1);
        assert_eq!(field::SCOPE_VERSION, 2);
    }

    #[test]
    fn two_builds_give_identical_bytes() {
        let input = small();
        for encoding in [Encoding::Json, Encoding::Protobuf] {
            let a = build(&input, ExportPolicy::Full, encoding).unwrap();
            let b = build(&input, ExportPolicy::Full, encoding).unwrap();
            assert_eq!(a.bytes, b.bytes);
            assert_eq!(a.digest, b.digest);
        }
    }

    #[test]
    fn metadata_carries_no_tool_text_and_full_carries_it_capped() {
        let input = small();
        for encoding in [Encoding::Json, Encoding::Protobuf] {
            let meta = build(&input, ExportPolicy::Metadata, encoding).unwrap();
            let text = String::from_utf8_lossy(&meta.bytes);
            assert!(!text.contains("gen_ai.tool.call.arguments"));
            assert!(!text.contains("gen_ai.tool.call.result"));
            assert!(!text.contains("echo hi"));
            assert!(
                !meta
                    .fields
                    .iter()
                    .any(|f| f.field.starts_with("gen_ai.tool.call.a")
                        || f.field.ends_with(".result"))
            );
        }
        let full = build(&input, ExportPolicy::Full, Encoding::Json).unwrap();
        let doc: Value = serde_json::from_slice(&full.bytes).unwrap();
        let tool = &doc["resourceSpans"][0]["scopeSpans"][0]["spans"][2];
        let attrs = tool["attributes"].as_array().unwrap();
        let value = |key: &str| {
            attrs
                .iter()
                .find(|a| a["key"] == key)
                .map(|a| a["value"]["stringValue"].as_str().unwrap().to_string())
        };
        assert_eq!(
            value("gen_ai.tool.call.arguments").as_deref(),
            Some("echo hi")
        );
        let result = value("gen_ai.tool.call.result").unwrap();
        assert!(result.len() < MAX_CONTENT + 20 && result.ends_with("[truncated]\n"));
        let redactions = attrs
            .iter()
            .find(|a| a["key"] == "agentflight.redactions")
            .unwrap();
        assert_eq!(redactions["value"]["intValue"], "1");
    }

    #[test]
    fn the_preview_counts_name_every_attribute_key() {
        let p = build(&small(), ExportPolicy::Metadata, Encoding::Json).unwrap();
        let count = |name: &str| p.fields.iter().find(|f| f.field == name).map(|f| f.count);
        assert_eq!(count("gen_ai.operation.name"), Some(3));
        assert_eq!(count("agentflight.semconv"), Some(1));
        assert_eq!(
            count("agentflight.to"),
            Some(1),
            "the span event's attribute"
        );
        assert_eq!(p.events, 1);
        assert_eq!(p.spans, 4);
        assert_eq!(p.digest, sha256_hex(&p.bytes));
    }

    #[test]
    fn a_payload_over_the_cap_is_refused() {
        let mut input = small();
        input.content.insert(
            "ev_a".into(),
            ToolText {
                text: "y".repeat(MAX_CONTENT),
                redactions: 0,
            },
        );
        // Many tool spans make the payload exceed the cap.
        let template = input.events[1].clone();
        let n = MAX_PAYLOAD / (MAX_CONTENT + 100) + 10;
        for i in 0..n {
            let mut e = template.clone();
            e.id = format!("ev_big{i}");
            e.span = Some(format!("tb{i}"));
            input.content.insert(
                e.id.clone(),
                ToolText {
                    text: "z".repeat(MAX_CONTENT),
                    redactions: 0,
                },
            );
            input.events.push(e);
        }
        let err = build(&input, ExportPolicy::Full, Encoding::Protobuf).unwrap_err();
        assert_eq!(err, TOO_LARGE);
    }

    #[test]
    fn export_rs_does_no_io_and_names_no_process_or_app_code() {
        let src = include_str!("export.rs");
        let live = crate::test_support::without_test_items(src);
        for banned in [
            "crate::",
            "std::process",
            "Command",
            "quiet_command",
            "run_one",
            "std::fs",
            "std::net",
            "ureq",
        ] {
            assert!(!live.contains(banned), "export.rs names {banned}");
        }
    }
}
