//! A2 + R9 + A4 from Claude Code's own session log.
//!
//! Layout (Claude Code 2.1.287, checked on real sessions):
//! `<config>/projects/<encoded cwd>/<session id>.jsonl`, and per sub-agent
//! `<session id>/subagents/agent-<agent id>.jsonl` with an
//! `agent-<agent id>.meta.json` (`agentType`, `description`, `toolUseId`).
//!
//! Read-only: the config folder is never written.
//!
//! - Usage: `assistant` lines carry `message.usage`; one API message can
//!   span several lines (one per content block) with the same `message.id`,
//!   so it is counted once (the last line's numbers win).
//! - Sub-agents: an `Agent` (or older `Task`) `tool_use` spawns one; its
//!   `tool_result` line's `toolUseResult` says `async_launched` (background)
//!   or `completed` (foreground) and gives the `agentId`; a background one's
//!   end comes as a `<task-notification>` (queued, then a user message)
//!   with `<tool-use-id>` and `<status>`. A foreground result's own `usage`
//!   is not counted (the sub-agent's file has it).

use super::price::{context_window, Tokens};
use super::tail::JsonlTail;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const ORCHESTRATOR: &str = "orchestrator";
/// A gap longer than this between two log events is idle, not work.
pub const ACTIVE_GAP_MS: i64 = 5 * 60 * 1000;
/// The context meter warns from this share of the window (A4).
pub const CONTEXT_WARN: f64 = 0.60;

pub fn parse_time_ms(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text.trim())
        .ok()
        .map(|t| t.timestamp_millis())
}

#[derive(Debug, Clone)]
struct UsageRecord {
    at_ms: Option<i64>,
    model: String,
    tokens: Tokens,
    /// The agent id of a sub-agent file, None for the session itself.
    agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SubagentInfo {
    pub tool_use_id: String,
    pub agent_id: Option<String>,
    pub agent_type: String,
    pub description: Option<String>,
    /// `spawned`, `running` (background), `completed`, `failed`, `killed`.
    pub status: String,
    pub background: bool,
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContextMeter {
    pub model: String,
    pub used: u64,
    pub window: u64,
    pub percent: f64,
    pub warn: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CostLine {
    /// The stage or the agent the line is for.
    pub name: String,
    pub tokens: Tokens,
    pub total_tokens: u64,
    /// USD estimate; None when a model of the line has no price.
    pub cost_usd: Option<f64>,
    /// Active minutes (gaps over 5 min left out).
    pub minutes: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub stages: Vec<CostLine>,
    pub agents: Vec<CostLine>,
    pub total_tokens: u64,
    pub total_cost_usd: Option<f64>,
    pub minutes: f64,
    pub messages: usize,
}

/// One tab's session: its log and its sub-agents' logs, read as they grow.
#[derive(Debug, Default)]
pub struct SessionTracker {
    pub session_file: Option<PathBuf>,
    main: Option<JsonlTail>,
    subs: BTreeMap<String, JsonlTail>,
    /// agent id → agent type (from meta.json or the spawn).
    agent_types: HashMap<String, String>,
    by_message: HashMap<String, usize>,
    records: Vec<UsageRecord>,
    subagents: BTreeMap<String, SubagentInfo>,
    context: Option<(String, u64)>,
    /// The newest write to any of the files.
    pub last_write: Option<SystemTime>,
}

fn newest(a: Option<SystemTime>, b: Option<SystemTime>) -> Option<SystemTime> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(text[start..end].trim())
}

fn short_type(agent_type: &str) -> String {
    agent_type
        .strip_prefix("dcv2:")
        .unwrap_or(agent_type)
        .to_string()
}

impl SessionTracker {
    /// Tracks a known session log file (`claude_session_log::find_session_log`).
    pub fn for_file(file: PathBuf) -> Self {
        Self {
            main: Some(JsonlTail::new(&file)),
            session_file: Some(file),
            ..Default::default()
        }
    }

    /// The first and last message times (Unix ms) read so far.
    pub fn activity_span(&self) -> Option<(i64, i64)> {
        let times = self.records.iter().filter_map(|r| r.at_ms);
        let first = times.clone().min()?;
        let last = times.max()?;
        Some((first, last))
    }

    /// `<session>/subagents` beside `<session>.jsonl`.
    pub fn subagents_dir(&self) -> Option<PathBuf> {
        let file = self.session_file.as_ref()?;
        Some(file.with_extension("").join("subagents"))
    }

    /// Reads what the files gained since the last poll.
    pub fn poll(&mut self) {
        let Some(main) = self.main.as_mut() else {
            return;
        };
        let lines = main.read_new();
        let mut last = modified(main.path());
        for line in &lines {
            self.take_line(line, None);
        }
        if let Some(dir) = self.subagents_dir() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let Some(id) = name
                        .strip_prefix("agent-")
                        .and_then(|n| n.strip_suffix(".jsonl"))
                    else {
                        continue;
                    };
                    if !self.subs.contains_key(id) {
                        let meta = dir.join(format!("agent-{id}.meta.json"));
                        if let Some(kind) = std::fs::read_to_string(meta)
                            .ok()
                            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                            .and_then(|v| v["agentType"].as_str().map(str::to_string))
                        {
                            self.agent_types.insert(id.to_string(), kind);
                        }
                        self.subs
                            .insert(id.to_string(), JsonlTail::new(entry.path()));
                    }
                }
            }
        }
        let ids: Vec<String> = self.subs.keys().cloned().collect();
        for id in ids {
            let (lines, at) = {
                let tail = self.subs.get_mut(&id).expect("listed");
                (tail.read_new(), modified(tail.path()))
            };
            last = newest(last, at);
            for line in &lines {
                self.take_line(line, Some(&id));
            }
        }
        self.last_write = newest(self.last_write, last);
    }

    /// One parsed log line. `agent`: the sub-agent whose file it is from.
    pub fn take_line(&mut self, line: &Value, agent: Option<&str>) {
        let at_ms = line["timestamp"].as_str().and_then(parse_time_ms);
        match line["type"].as_str() {
            Some("assistant") => self.take_assistant(line, agent, at_ms),
            Some("user") if agent.is_none() => self.take_user(line, at_ms),
            Some("queue-operation") if agent.is_none() => {
                if let Some(text) = line["content"].as_str() {
                    self.take_notification(text, at_ms);
                }
            }
            _ => {}
        }
    }

    fn take_assistant(&mut self, line: &Value, agent: Option<&str>, at_ms: Option<i64>) {
        let message = &line["message"];
        let model = message["model"].as_str().unwrap_or("").to_string();
        if let (Some(id), Some(usage)) = (message["id"].as_str(), message.get("usage")) {
            if model != "<synthetic>" && usage.is_object() {
                let tokens = Tokens::from_usage(usage);
                let record = UsageRecord {
                    at_ms,
                    model: model.clone(),
                    tokens,
                    agent_id: agent.map(str::to_string),
                };
                match self.by_message.get(id) {
                    Some(&at) => {
                        let first = self.records[at].at_ms;
                        self.records[at] = UsageRecord {
                            at_ms: first.or(at_ms),
                            ..record
                        };
                    }
                    None => {
                        self.by_message.insert(id.to_string(), self.records.len());
                        self.records.push(record);
                    }
                }
                let sidechain = line["isSidechain"].as_bool().unwrap_or(false);
                if agent.is_none() && !sidechain {
                    let used = tokens.input
                        + tokens.cache_write_5m
                        + tokens.cache_write_1h
                        + tokens.cache_read;
                    self.context = Some((model.clone(), used));
                }
            }
        }
        if agent.is_some() {
            return;
        }
        for block in message["content"].as_array().into_iter().flatten() {
            let name = block["name"].as_str();
            if block["type"] == "tool_use" && matches!(name, Some("Agent") | Some("Task")) {
                let Some(id) = block["id"].as_str() else {
                    continue;
                };
                let input = &block["input"];
                self.subagents
                    .entry(id.to_string())
                    .or_insert_with(|| SubagentInfo {
                        tool_use_id: id.to_string(),
                        agent_id: None,
                        agent_type: input["subagent_type"]
                            .as_str()
                            .unwrap_or("general-purpose")
                            .to_string(),
                        description: input["description"].as_str().map(str::to_string),
                        status: "spawned".into(),
                        background: input["run_in_background"].as_bool().unwrap_or(false),
                        started_ms: at_ms,
                        finished_ms: None,
                    });
            }
        }
    }

    fn take_user(&mut self, line: &Value, at_ms: Option<i64>) {
        let content = &line["message"]["content"];
        if let Some(text) = content.as_str() {
            self.take_notification(text, at_ms);
            return;
        }
        let result = &line["toolUseResult"];
        for block in content.as_array().into_iter().flatten() {
            if block["type"] != "tool_result" {
                continue;
            }
            let Some(id) = block["tool_use_id"].as_str() else {
                continue;
            };
            let Some(info) = self.subagents.get_mut(id) else {
                continue;
            };
            if let Some(agent_id) = result["agentId"].as_str() {
                info.agent_id = Some(agent_id.to_string());
                self.agent_types
                    .entry(agent_id.to_string())
                    .or_insert_with(|| info.agent_type.clone());
            }
            let status = result["status"].as_str();
            if block["is_error"].as_bool() == Some(true) {
                info.status = "failed".into();
                info.finished_ms = at_ms;
            } else if status == Some("async_launched") {
                info.status = "running".into();
                info.background = true;
            } else if let Some(status) = status {
                info.status = status.to_string();
                info.finished_ms = at_ms;
            }
        }
    }

    fn take_notification(&mut self, text: &str, at_ms: Option<i64>) {
        let Some(body) = tag(text, "task-notification") else {
            return;
        };
        let (Some(id), Some(status)) = (tag(body, "tool-use-id"), tag(body, "status")) else {
            return;
        };
        if let Some(info) = self.subagents.get_mut(id) {
            if info.finished_ms.is_none() || info.status != status {
                info.status = status.to_string();
                info.finished_ms = info.finished_ms.or(at_ms);
            }
        }
    }

    pub fn subagents(&self) -> Vec<SubagentInfo> {
        let mut list: Vec<_> = self.subagents.values().cloned().collect();
        list.sort_by_key(|s| s.started_ms.unwrap_or(i64::MAX));
        list
    }

    /// The estimated cost of each agent's messages, keyed by its agent id (`None`: the main
    /// session). A model with no price makes that agent's cost `None`.
    pub fn cost_by_agent(&self) -> HashMap<Option<String>, Option<f64>> {
        let mut out: HashMap<Option<String>, Option<f64>> = HashMap::new();
        for record in &self.records {
            let cost = record.tokens.cost(&record.model);
            let entry = out.entry(record.agent_id.clone()).or_insert(Some(0.0));
            *entry = match (*entry, cost) {
                (Some(sum), Some(c)) => Some(sum + c),
                _ => None,
            };
        }
        out
    }

    /// A4: the session's last context size against its model's window.
    pub fn context(&self) -> Option<ContextMeter> {
        let (model, used) = self.context.clone()?;
        let window = context_window(&model);
        let percent = used as f64 / window as f64;
        Some(ContextMeter {
            model,
            used,
            window,
            percent: (percent * 1000.0).round() / 10.0,
            warn: percent >= CONTEXT_WARN,
        })
    }

    fn agent_name(&self, record: &UsageRecord) -> String {
        match &record.agent_id {
            None => ORCHESTRATOR.to_string(),
            Some(id) => self
                .agent_types
                .get(id)
                .map(|t| short_type(t))
                .unwrap_or_else(|| "sub-agent".to_string()),
        }
    }

    /// R9: tokens, $ and minutes per stage and per agent. `timeline`:
    /// (time, stage) from log.md; a message counts for the stage of the
    /// last line at or before it, else `fallback_stage`.
    pub fn summary(&self, timeline: &[(i64, String)], fallback_stage: &str) -> UsageSummary {
        let mut timeline: Vec<&(i64, String)> = timeline.iter().collect();
        timeline.sort_by_key(|(t, _)| *t);
        let stage_at = |at: Option<i64>| -> String {
            at.and_then(|at| {
                timeline
                    .iter()
                    .rev()
                    .find(|(t, _)| *t <= at)
                    .map(|(_, s)| s.clone())
            })
            .or_else(|| timeline.first().map(|(_, s)| s.clone()))
            .unwrap_or_else(|| fallback_stage.to_string())
        };
        #[derive(Default)]
        struct Acc {
            tokens: Tokens,
            cost: Option<f64>,
            unpriced: bool,
            minutes: f64,
            order: usize,
        }
        let mut stages: HashMap<String, Acc> = HashMap::new();
        let mut agents: HashMap<String, Acc> = HashMap::new();
        let mut order: Vec<&UsageRecord> = self.records.iter().collect();
        order.sort_by_key(|r| r.at_ms.unwrap_or(i64::MAX));
        let mut previous: Option<i64> = None;
        let mut total_minutes = 0.0;
        for (at, record) in order.iter().enumerate() {
            let gap = match (previous, record.at_ms) {
                (Some(p), Some(now)) if now >= p && now - p <= ACTIVE_GAP_MS => {
                    (now - p) as f64 / 60_000.0
                }
                _ => 0.0,
            };
            if record.at_ms.is_some() {
                previous = record.at_ms;
            }
            total_minutes += gap;
            let cost = record.tokens.cost(&record.model);
            for (map, key, minutes) in [
                (&mut stages, stage_at(record.at_ms), gap),
                (&mut agents, self.agent_name(record), 0.0),
            ] {
                let next = map.len();
                let acc = map.entry(key).or_insert_with(|| Acc {
                    order: next,
                    ..Default::default()
                });
                acc.tokens.add(&record.tokens);
                acc.minutes += minutes;
                match cost {
                    Some(c) => acc.cost = Some(acc.cost.unwrap_or(0.0) + c),
                    None => acc.unpriced = true,
                }
                let _ = at;
            }
        }
        // An agent's minutes: from its first to its last message, active only.
        for (name, acc) in agents.iter_mut() {
            let mut times: Vec<i64> = self
                .records
                .iter()
                .filter(|r| &self.agent_name(r) == name)
                .filter_map(|r| r.at_ms)
                .collect();
            times.sort_unstable();
            acc.minutes = times
                .windows(2)
                .map(|w| w[1] - w[0])
                .filter(|d| *d <= ACTIVE_GAP_MS)
                .sum::<i64>() as f64
                / 60_000.0;
        }
        let lines = |map: HashMap<String, Acc>| {
            let mut list: Vec<(usize, CostLine)> = map
                .into_iter()
                .map(|(name, acc)| {
                    (
                        acc.order,
                        CostLine {
                            name,
                            total_tokens: acc.tokens.total(),
                            tokens: acc.tokens,
                            cost_usd: if acc.unpriced { None } else { acc.cost },
                            minutes: (acc.minutes * 10.0).round() / 10.0,
                        },
                    )
                })
                .collect();
            list.sort_by_key(|(o, _)| *o);
            list.into_iter().map(|(_, l)| l).collect::<Vec<_>>()
        };
        let stages = lines(stages);
        let agents = lines(agents);
        let priced = stages.iter().all(|s| s.cost_usd.is_some());
        UsageSummary {
            total_tokens: stages.iter().map(|s| s.total_tokens).sum(),
            total_cost_usd: priced.then(|| stages.iter().filter_map(|s| s.cost_usd).sum()),
            minutes: (total_minutes * 10.0).round() / 10.0,
            messages: self.records.len(),
            stages,
            agents,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use serde_json::json;

    const SID: &str = "de6a6329-de0f-49da-960b-f50fed9236e4";

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fixtures/claude-2.1.287")
    }

    /// A copy of the recorded config folder, so a test never reads ~/.claude*.
    fn recorded_session(dir: &TempDir) -> PathBuf {
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
        copy(
            &fixture_root().join("projects"),
            &dir.join("config/projects"),
        );
        crate::claude_session_log::find_session_log(&dir.join("config"), SID).unwrap()
    }

    #[test]
    fn the_recorded_session_gives_its_subagent_usage_and_context() {
        let dir = TempDir::new("transcript_fixture");
        let file = recorded_session(&dir);
        let mut tracker = SessionTracker::for_file(file);
        tracker.poll();
        let subs = tracker.subagents();
        assert_eq!(subs.len(), 1, "{subs:?}");
        assert_eq!(subs[0].agent_type, "dcv2:plan-reviewer");
        assert_eq!(subs[0].status, "completed");
        assert_eq!(subs[0].agent_id.as_deref(), Some("a2f2a89910f21f43e"));
        assert!(!subs[0].background);
        let summary = tracker.summary(&[], "plan");
        let names: Vec<&str> = summary.agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, [ORCHESTRATOR, "plan-reviewer"]);
        assert_eq!(summary.stages.len(), 1);
        assert_eq!(summary.stages[0].name, "plan");
        // The recorded `cost-state` line says what Claude Code itself counted.
        let cost_state = std::fs::read_to_string(tracker.session_file.as_ref().unwrap())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .find(|v| v["type"] == "cost-state")
            .unwrap();
        let claude = cost_state["totalCostUSD"].as_f64().unwrap();
        let ours = summary.total_cost_usd.unwrap();
        assert!(
            (ours - claude).abs() / claude < 0.02,
            "ours {ours} vs Claude {claude}"
        );
        let context = tracker.context().unwrap();
        assert!(context.model.contains("haiku"));
        assert_eq!(context.window, 200_000);
        assert!(!context.warn && context.used > 0);
        assert!(tracker.last_write.is_some());
        let before = summary.messages;
        tracker.poll();
        assert_eq!(
            tracker.summary(&[], "plan").messages,
            before,
            "nothing read twice"
        );
    }

    fn assistant(id: &str, at: &str, input: u64, out: u64) -> Value {
        json!({"type": "assistant", "timestamp": at, "message": {"id": id, "model": "claude-opus-5-5",
            "content": [{"type": "text"}],
            "usage": {"input_tokens": input, "output_tokens": out, "cache_read_input_tokens": 0,
                      "cache_creation_input_tokens": 0}}})
    }

    #[test]
    fn a_message_split_over_lines_counts_once_and_stages_follow_the_log() {
        let mut t = SessionTracker::default();
        t.take_line(&assistant("m1", "2026-10-22T10:00:00Z", 100, 10), None);
        t.take_line(&assistant("m1", "2026-10-22T10:00:01Z", 100, 20), None);
        t.take_line(&assistant("m2", "2026-10-22T10:02:00Z", 50, 5), None);
        t.take_line(&assistant("m3", "2026-10-22T11:00:00Z", 1, 1), None);
        let timeline = vec![
            (
                parse_time_ms("2026-10-22T09:59:00Z").unwrap(),
                "plan".to_string(),
            ),
            (
                parse_time_ms("2026-10-22T10:01:00Z").unwrap(),
                "build".to_string(),
            ),
        ];
        let s = t.summary(&timeline, "x");
        assert_eq!(s.messages, 3);
        let plan = &s.stages[0];
        assert_eq!(
            (plan.name.as_str(), plan.tokens.input, plan.tokens.output),
            ("plan", 100, 20)
        );
        let build = &s.stages[1];
        assert_eq!(build.tokens.input, 51);
        assert_eq!(build.minutes, 2.0, "the 58-minute gap is idle");
        assert_eq!(s.minutes, 2.0);
        // opus 5.5: (151 * 4 + 26 * 20) / 1e6
        assert!((s.total_cost_usd.unwrap() - 0.001124).abs() < 1e-9);
        let mut unknown = SessionTracker::default();
        let mut line = assistant("u1", "2026-10-22T10:00:00Z", 10, 10);
        line["message"]["model"] = json!("some-new-model");
        unknown.take_line(&line, None);
        assert_eq!(
            unknown.summary(&[], "x").total_cost_usd,
            None,
            "never a guessed price"
        );
    }

    #[test]
    fn background_agents_end_with_their_task_notification() {
        let mut t = SessionTracker::default();
        t.take_line(&json!({"type": "assistant", "timestamp": "2026-10-22T10:00:00Z", "message": {
            "id": "m1", "content": [{"type": "tool_use", "id": "toolu_1", "name": "Agent",
            "input": {"subagent_type": "dcv2:implementer", "description": "Build PR1", "prompt": "x"}}]}}), None);
        assert_eq!(t.subagents()[0].status, "spawned");
        t.take_line(&json!({"type": "user", "toolUseResult": {"agentId": "a1", "status": "async_launched", "isAsync": true},
            "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_1"}]}}), None);
        assert_eq!(t.subagents()[0].status, "running");
        assert!(t.subagents()[0].background);
        t.take_line(&json!({"type": "queue-operation", "operation": "enqueue", "timestamp": "2026-10-22T10:05:00Z",
            "content": "<task-notification>\n<task-id>a1</task-id>\n<tool-use-id>toolu_1</tool-use-id>\n<status>completed</status>\n</task-notification>"}), None);
        let sub = &t.subagents()[0];
        assert_eq!(sub.status, "completed");
        assert_eq!(sub.finished_ms, parse_time_ms("2026-10-22T10:05:00Z"));
        t.take_line(&assistant("s1", "2026-10-22T10:01:00Z", 5, 5), Some("a1"));
        let names: Vec<String> = t
            .summary(&[], "build")
            .agents
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(names, ["implementer"]);
    }

    #[test]
    fn the_context_meter_warns_at_sixty_percent_of_the_window() {
        let mut t = SessionTracker::default();
        let mut line = assistant("m1", "2026-10-22T10:00:00Z", 10, 1);
        line["message"]["model"] = json!("claude-haiku-4-5");
        line["message"]["usage"]["cache_read_input_tokens"] = json!(119_990);
        t.take_line(&line, None);
        let meter = t.context().unwrap();
        assert_eq!(
            (meter.used, meter.percent, meter.warn),
            (120_000, 60.0, true)
        );
        let mut side = assistant("m2", "2026-10-22T10:00:00Z", 1, 1);
        side["isSidechain"] = json!(true);
        t.take_line(&side, None);
        assert_eq!(
            t.context().unwrap().used,
            120_000,
            "a sidechain line is not the session"
        );
    }
}
