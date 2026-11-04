//! The run folder as schema-1 events (CAP-7): stage changes, approval waits and decisions, checks of
//! three sources, review rounds, CI, PR states, retries, requests and the guard's own log. Pure: it
//! reads only the files the host hands it (capped at 8 MiB each), writes nothing, and holds no log
//! text in an event name or attribute: the text stays behind the event's source ref.
//!
//! Every event is on the `orchestration` lane (`agent.id`), from provider `dcv2`, and points at a
//! line (offset, length, sha256) or, for a whole file, at offset 0 and its length. A malformed or
//! oversized file gives a notice, never an error.

use super::schema::{
    attr, event_id_seq, kind, sha256_hex, status, AgentRef, CapturePolicy, Content, Event,
    Provider, SourceRef, ORCHESTRATION, PROVIDER_DCV2, SCHEMA,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

/// A run file larger than this is skipped with a notice.
pub const MAX_FILE: u64 = 8 * 1024 * 1024;
/// How an event derived from a log line says so.
pub const LOG_LINE: &str = "exact:log-line";

/// One approval stop, as `run::timeline::approval_bands` gives it (plain values: the core never
/// parses approval text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalIn {
    /// `plan`, `ship` or `other`.
    pub kind: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub open_offset: u64,
    pub close_offset: Option<u64>,
    /// `approved`, `declined`, `ended` or `open`.
    pub outcome: String,
}

#[derive(Debug, Clone, Default)]
pub struct RunInputs {
    pub run_dir: PathBuf,
    /// `(name relative to the run folder, path)` of every file the host lets the adapter read.
    pub files: Vec<(String, PathBuf)>,
    pub approvals: Vec<ApprovalIn>,
    /// A repo's real path -> its check command (repos.json).
    pub repo_checks: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct RunEvents {
    pub events: Vec<Event>,
    pub notices: Vec<String>,
    /// Each PR's current `state` in status.json: the fallback phase of a run with no `pr` lines.
    pub status_now: BTreeMap<String, String>,
}

struct Loaded {
    path: String,
    bytes: Vec<u8>,
}

struct Maker {
    run_id: String,
    events: Vec<Event>,
    notices: Vec<String>,
    seqs: HashMap<(String, u64), usize>,
}

/// `2026-11-05T10:00:00Z` -> `2026-11-05T10:00:00.000Z` (the transcripts' shape, so the text order is
/// the time order).
fn iso(text: &str) -> Option<String> {
    let t = DateTime::parse_from_rfc3339(text.trim()).ok()?;
    Some(
        t.with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    )
}

fn iso_ms(ms: i64) -> Option<String> {
    Some(DateTime::<Utc>::from_timestamp_millis(ms)?.to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn iso_mtime(path: &str) -> String {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| DateTime::<Utc>::from(t).to_rfc3339_opts(SecondsFormat::Millis, true))
        .unwrap_or_else(|_| "1970-01-01T00:00:00.000Z".into())
}

fn is_pr_id(word: &str) -> bool {
    word.strip_prefix("PR")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn map(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .filter(|(_, v)| !v.is_null())
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn whole_ref(file: &Loaded) -> SourceRef {
    SourceRef {
        file: file.path.clone(),
        offset: 0,
        len: file.bytes.len() as u64,
        sha256: sha256_hex(&file.bytes),
    }
}

fn line_ref(file: &Loaded, offset: u64, len: u64) -> SourceRef {
    let from = (offset as usize).min(file.bytes.len());
    let to = (from + len as usize).min(file.bytes.len());
    SourceRef {
        file: file.path.clone(),
        offset,
        len: (to - from) as u64,
        sha256: sha256_hex(&file.bytes[from..to]),
    }
}

/// The lines of a file as `(offset, text, length)`, without the line break.
fn lines(file: &Loaded) -> Vec<(u64, String, u64)> {
    let mut out = Vec::new();
    let mut offset = 0u64;
    for raw in file.bytes.split_inclusive(|b| *b == b'\n') {
        let mut end = raw.len();
        while end > 0 && matches!(raw[end - 1], b'\n' | b'\r') {
            end -= 1;
        }
        out.push((
            offset,
            String::from_utf8_lossy(&raw[..end]).to_string(),
            end as u64,
        ));
        offset += raw.len() as u64;
    }
    out
}

impl Maker {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        kind: &str,
        native: &str,
        at: String,
        status: Option<&str>,
        name: Option<String>,
        span: Option<String>,
        attrs: Map<String, Value>,
        src: SourceRef,
        derived: Option<&str>,
    ) {
        let n = self.seqs.entry((src.file.clone(), src.offset)).or_insert(0);
        let id = event_id_seq(&src.file, src.offset, &src.sha256, *n);
        *n += 1;
        self.events.push(Event {
            schema: SCHEMA,
            id,
            trace: self.run_id.clone(),
            session: self.run_id.clone(),
            agent: AgentRef {
                id: ORCHESTRATION.into(),
                parent: None,
                kind: ORCHESTRATION.into(),
                agent_type: None,
                role: None,
                pr: None,
            },
            provider: Provider {
                name: PROVIDER_DCV2.into(),
                version: None,
                native: native.into(),
            },
            kind: kind.into(),
            at,
            span,
            status: status.map(str::to_string),
            name,
            attrs,
            usage: None,
            content: Content {
                source: src,
                policy: CapturePolicy::Metadata,
                redactions: 0,
            },
            derived: derived.map(str::to_string),
        });
    }
}

struct Run<'a> {
    inputs: &'a RunInputs,
    make: Maker,
    cache: HashMap<String, Option<std::rc::Rc<Loaded>>>,
}

#[derive(Default)]
struct PrInfo {
    repo_path: Option<String>,
    state: Option<String>,
}

fn check_status(result: &str) -> Option<&'static str> {
    match result {
        "pass" => Some(status::PASS),
        "fail" => Some(status::FAIL),
        "error" | "timeout" => Some("error"),
        "none" => Some(status::SKIP),
        _ => None,
    }
}

impl Run<'_> {
    fn path_of(&self, name: &str) -> Option<&PathBuf> {
        self.inputs
            .files
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, p)| p)
    }

    /// The file's bytes, once: `None` when it is not handed over, gone, unreadable or over 8 MiB
    /// (the last two with a notice).
    fn load(&mut self, name: &str) -> Option<std::rc::Rc<Loaded>> {
        if let Some(hit) = self.cache.get(name) {
            return hit.clone();
        }
        let loaded = self.path_of(name).cloned().and_then(|path| {
            let meta = std::fs::metadata(&path).ok()?;
            if !meta.is_file() {
                return None;
            }
            if meta.len() > MAX_FILE {
                self.make
                    .notices
                    .push(format!("{name}: skipped, larger than 8 MiB"));
                return None;
            }
            match std::fs::read(&path) {
                Ok(bytes) => Some(std::rc::Rc::new(Loaded {
                    path: path.display().to_string(),
                    bytes,
                })),
                Err(e) => {
                    self.make.notices.push(format!("{name}: cannot read ({e})"));
                    None
                }
            }
        });
        self.cache.insert(name.to_string(), loaded.clone());
        loaded
    }

    /// A JSON file: `None` for a file that is absent, and a notice for one that does not parse.
    fn json(&mut self, name: &str) -> Option<(std::rc::Rc<Loaded>, Value)> {
        let file = self.load(name)?;
        match serde_json::from_slice::<Value>(&file.bytes) {
            Ok(v) => Some((file, v)),
            Err(_) => {
                self.make
                    .notices
                    .push(format!("{name}: malformed JSON, its events are left out"));
                None
            }
        }
    }

    fn run_all(&mut self) -> BTreeMap<String, String> {
        let prs = self.status_json();
        self.log_md(&prs);
        self.approvals();
        self.app_checks();
        self.reviews();
        self.app_github();
        self.requests();
        self.guard_log();
        prs.into_iter()
            .filter_map(|(id, info)| Some((id, info.state?)))
            .collect()
    }

    /// status.json: the PR check events; returns the PRs' repo paths for the stop-check names.
    fn status_json(&mut self) -> BTreeMap<String, PrInfo> {
        let mut out = BTreeMap::new();
        let Some((file, doc)) = self.json("status.json") else {
            return out;
        };
        let doc_at = doc["updatedAt"].as_str().and_then(iso);
        for pr in doc["prs"].as_array().into_iter().flatten() {
            let Some(id) = pr["id"].as_str().filter(|i| is_pr_id(i)) else {
                continue;
            };
            out.insert(
                id.to_string(),
                PrInfo {
                    repo_path: pr["repoPath"].as_str().map(str::to_string),
                    state: pr["state"].as_str().map(str::to_string),
                },
            );
            let check = &pr["check"];
            let Some(st) = check["result"].as_str().and_then(check_status) else {
                continue;
            };
            let at = pr["updatedAt"]
                .as_str()
                .and_then(iso)
                .or_else(|| doc_at.clone())
                .unwrap_or_else(|| iso_mtime(&file.path));
            let log = check["log"]
                .as_str()
                .filter(|l| !l.contains(".."))
                .and_then(|l| self.load(l));
            let src = log.map_or_else(|| whole_ref(&file), |l| whole_ref(&l));
            self.make.push(
                kind::CHECK_END,
                "status.json:check",
                at,
                Some(st),
                check["command"].as_str().map(str::to_string),
                None,
                map(&[(attr::SOURCE, json!("skill")), (attr::PR, json!(id))]),
                src,
                None,
            );
        }
        out
    }

    fn log_md(&mut self, prs: &BTreeMap<String, PrInfo>) {
        let Some(file) = self.load("log.md") else {
            return;
        };
        let skip: HashSet<u64> = self
            .inputs
            .approvals
            .iter()
            .flat_map(|a| {
                let close = matches!(a.outcome.as_str(), "approved" | "declined")
                    .then_some(a.close_offset)
                    .flatten();
                [Some(a.open_offset), close]
            })
            .flatten()
            .collect();
        let mut stage: Option<String> = None;
        for (offset, line, len) in lines(&file) {
            let mut parts = line.splitn(3, ' ');
            let (Some(time), Some(st)) = (parts.next(), parts.next()) else {
                continue;
            };
            let Some(at) = iso(time) else { continue };
            let text = parts.next().unwrap_or("");
            if text.starts_with("app:") {
                continue;
            }
            let src = line_ref(&file, offset, len);
            if stage.as_deref() != Some(st) {
                self.make.push(
                    kind::STAGE_CHANGE,
                    "log.md:stage",
                    at.clone(),
                    None,
                    Some(st.to_string()),
                    None,
                    map(&[(attr::FROM, json!(stage)), (attr::TO, json!(st))]),
                    src.clone(),
                    None,
                );
                stage = Some(st.to_string());
            }
            if skip.contains(&offset) {
                continue;
            }
            let words: Vec<&str> = text.split(' ').collect();
            match words.as_slice() {
                ["stop-check", pr, result]
                    if (*pr == "-" || is_pr_id(pr))
                        && matches!(*result, "pass" | "fail" | "skip") =>
                {
                    let pr_id = is_pr_id(pr).then(|| pr.to_string());
                    let name = pr_id
                        .as_ref()
                        .and_then(|p| prs.get(p))
                        .and_then(|i| i.repo_path.as_ref())
                        .and_then(|r| self.inputs.repo_checks.get(r))
                        .cloned()
                        .unwrap_or_else(|| "stop-check".into());
                    let output = (*result == "fail")
                        .then_some(pr_id.as_ref())
                        .flatten()
                        .and_then(|p| self.load(&format!("logs/{p}-stop-check.txt")));
                    let src = output.map_or(src, |o| whole_ref(&o));
                    self.make.push(
                        kind::CHECK_END,
                        "log.md:stop-check",
                        at,
                        Some(result),
                        Some(name),
                        None,
                        map(&[
                            (attr::SOURCE, json!("stop-check")),
                            (attr::PR, json!(pr_id)),
                        ]),
                        src,
                        None,
                    );
                }
                ["pr", pr, old, "->", new]
                    if is_pr_id(pr) && !old.is_empty() && !new.is_empty() =>
                {
                    let attrs = map(&[
                        (attr::PR, json!(pr)),
                        (attr::FROM, json!(old)),
                        (attr::TO, json!(new)),
                    ]);
                    self.make.push(
                        kind::PR_STATE,
                        "log.md:pr",
                        at.clone(),
                        None,
                        Some(new.to_string()),
                        None,
                        attrs,
                        src.clone(),
                        None,
                    );
                    if *old == "reviewing" && *new == "fixing" {
                        self.make.push(
                            kind::RETRY,
                            "log.md:pr",
                            at,
                            None,
                            Some(pr.to_string()),
                            None,
                            map(&[(attr::PR, json!(pr))]),
                            src,
                            Some(LOG_LINE),
                        );
                    }
                }
                _ if text.contains("retried") => {
                    let pr = words.first().filter(|w| is_pr_id(w));
                    self.make.push(
                        kind::RETRY,
                        "log.md:retried",
                        at,
                        None,
                        pr.map(|p| p.to_string()),
                        None,
                        map(&[(attr::PR, json!(pr))]),
                        src,
                        Some(LOG_LINE),
                    );
                }
                ["attention", rest @ ..] => {
                    let name = rest
                        .first()
                        .filter(|w| w.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'))
                        .map(|w| w.to_string());
                    self.make.push(
                        kind::ATTENTION,
                        "log.md:attention",
                        at,
                        None,
                        name,
                        None,
                        Map::new(),
                        src,
                        None,
                    );
                }
                _ => self.make.push(
                    kind::LOG,
                    "log.md",
                    at,
                    None,
                    None,
                    None,
                    Map::new(),
                    src,
                    None,
                ),
            }
        }
    }

    /// Approval waits and decisions, from the host's bands (`run::timeline`), never parsed here.
    fn approvals(&mut self) {
        let Some(file) = self.load("log.md") else {
            return;
        };
        let all = lines(&file);
        let at_offset = |offset: u64| all.iter().find(|(o, _, _)| *o == offset);
        for band in self.inputs.approvals.clone() {
            if let (Some((o, _, len)), Some(at)) =
                (at_offset(band.open_offset), iso_ms(band.start_ms))
            {
                self.make.push(
                    kind::APPROVAL_WAIT,
                    "log.md:approval",
                    at,
                    None,
                    Some(band.kind.clone()),
                    None,
                    Map::new(),
                    line_ref(&file, *o, *len),
                    None,
                );
            }
            let decided = match band.outcome.as_str() {
                "approved" => Some(status::OK),
                "declined" => Some(status::DECLINED),
                _ => None,
            };
            if let (Some(st), Some(close), Some(at)) =
                (decided, band.close_offset, iso_ms(band.end_ms))
            {
                if let Some((o, _, len)) = at_offset(close) {
                    self.make.push(
                        kind::APPROVAL_DECISION,
                        "log.md:approval",
                        at,
                        Some(st),
                        Some(band.kind.clone()),
                        None,
                        Map::new(),
                        line_ref(&file, *o, *len),
                        None,
                    );
                }
            }
        }
    }

    fn app_checks(&mut self) {
        let Some((file, doc)) = self.json("app-checks.json") else {
            return;
        };
        let Some(checks) = doc["checks"].as_object() else {
            return;
        };
        for (pr, c) in checks {
            if !is_pr_id(pr) {
                continue;
            }
            let Some(st) = c["result"].as_str().and_then(check_status) else {
                continue;
            };
            let at = c["finishedAt"]
                .as_str()
                .and_then(iso)
                .unwrap_or_else(|| iso_mtime(&file.path));
            let log = c["log"]
                .as_str()
                .filter(|l| !l.contains(".."))
                .and_then(|l| self.load(l));
            let src = log.map_or_else(|| whole_ref(&file), |l| whole_ref(&l));
            self.make.push(
                kind::CHECK_END,
                "app-checks.json",
                at,
                Some(st),
                c["command"].as_str().map(str::to_string),
                None,
                map(&[
                    (attr::SOURCE, json!("app")),
                    (attr::PR, json!(pr)),
                    (attr::HEAD, c["head"].clone()),
                    (attr::EXIT_CODE, c["exitCode"].clone()),
                ]),
                src,
                None,
            );
        }
    }

    /// `reviews/*-round<r>.md`, with the verdict of the PR's gate on the PR's last round.
    fn reviews(&mut self) {
        let mut found: Vec<(String, String, u32)> = Vec::new();
        for (name, _) in &self.inputs.files {
            let Some(stem) = name
                .strip_prefix("reviews/")
                .and_then(|n| n.strip_suffix(".md"))
            else {
                continue;
            };
            let Some((who, round)) = stem.rsplit_once("-round") else {
                continue;
            };
            let Ok(round) = round.parse::<u32>() else {
                continue;
            };
            found.push((name.clone(), who.to_string(), round));
        }
        found.sort();
        let mut last: BTreeMap<String, u32> = BTreeMap::new();
        for (_, who, round) in &found {
            let e = last.entry(who.clone()).or_insert(0);
            *e = (*e).max(*round);
        }
        for (name, who, round) in found {
            let Some(file) = self.load(&name) else {
                continue;
            };
            let gate = is_pr_id(&who)
                .then(|| self.load(&format!("gates/{who}.yml")))
                .flatten()
                .map(|g| parse_gate(&String::from_utf8_lossy(&g.bytes)));
            let verdict = gate
                .filter(|(_, r)| r.map_or(last.get(&who) == Some(&round), |r| r == round))
                .map(|(g, _)| g);
            let stem = name
                .strip_prefix("reviews/")
                .and_then(|n| n.strip_suffix(".md"))
                .unwrap_or(&name)
                .to_string();
            self.make.push(
                kind::REVIEW_ROUND,
                "reviews",
                iso_mtime(&file.path),
                None,
                Some(stem),
                None,
                map(&[
                    (
                        attr::PR,
                        if is_pr_id(&who) {
                            json!(who)
                        } else {
                            Value::Null
                        },
                    ),
                    (attr::ROUND, json!(round)),
                    (attr::VERDICT, json!(verdict.flatten())),
                ]),
                whole_ref(&file),
                None,
            );
        }
    }

    fn app_github(&mut self) {
        let Some((file, doc)) = self.json("app-github.json") else {
            return;
        };
        let Some(prs) = doc["prs"].as_object() else {
            return;
        };
        for (pr, c) in prs {
            if !is_pr_id(pr) {
                continue;
            }
            let ci = c["ci"].as_str().unwrap_or("");
            if ci.is_empty() {
                continue;
            }
            let st = match ci {
                "pass" => Some(status::PASS),
                "fail" => Some(status::FAIL),
                _ => None,
            };
            let at = c["at"]
                .as_str()
                .and_then(iso)
                .unwrap_or_else(|| iso_mtime(&file.path));
            self.make.push(
                kind::CI_STATE,
                "app-github.json",
                at,
                st,
                Some(ci.to_string()),
                None,
                map(&[
                    (attr::PR, json!(pr)),
                    (attr::CI, json!(ci)),
                    (attr::PR_STATE, c["prState"].clone()),
                ]),
                whole_ref(&file),
                None,
            );
        }
    }

    /// `requests/<id>.json` and `<id>.result.json`: the kind and the result state only.
    fn requests(&mut self) {
        let names: Vec<String> = self
            .inputs
            .files
            .iter()
            .filter(|(n, _)| n.starts_with("requests/") && n.ends_with(".json"))
            .map(|(n, _)| n.clone())
            .collect();
        for name in names {
            let Some((file, doc)) = self.json(&name) else {
                continue;
            };
            let is_result = name.ends_with(".result.json");
            let kind_of = doc["kind"].as_str().map(str::to_string);
            let at = doc["at"]
                .as_str()
                .and_then(iso)
                .unwrap_or_else(|| iso_mtime(&file.path));
            let state = is_result
                .then(|| doc["state"].as_str().map(str::to_string))
                .flatten();
            let st = if is_result {
                match doc["ok"].as_bool() {
                    Some(true) => Some(status::OK),
                    Some(false) => Some("error"),
                    None => None,
                }
            } else {
                None
            };
            self.make.push(
                kind::REQUEST,
                "requests",
                at,
                st,
                kind_of.clone(),
                None,
                map(&[
                    (attr::REQUEST_KIND, json!(kind_of)),
                    (attr::RESULT, json!(state)),
                ]),
                whole_ref(&file),
                None,
            );
        }
    }

    /// `logs/guard.log`: `<UTC>\t<rule id>\t<tool>\t<agent id|main>\t<tool_use_id|->`.
    fn guard_log(&mut self) {
        let Some(file) = self.load("logs/guard.log") else {
            return;
        };
        let mut bad = 0usize;
        for (offset, line, len) in lines(&file) {
            if line.trim().is_empty() {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            let (Some(at), true) = (f.first().and_then(|t| iso(t)), f.len() == 5) else {
                bad += 1;
                continue;
            };
            let span = (f[4] != "-" && !f[4].is_empty()).then(|| f[4].to_string());
            self.make.push(
                kind::TOOL_BLOCKED,
                "logs/guard.log",
                at,
                Some(status::BLOCKED),
                Some(f[2].to_string()),
                span,
                map(&[
                    (attr::SOURCE, json!("guard.log")),
                    (attr::RULE, json!(f[1])),
                    (attr::TOOL, json!(f[2])),
                    (attr::AGENT, json!(f[3])),
                ]),
                line_ref(&file, offset, len),
                None,
            );
        }
        if bad > 0 {
            self.make.notices.push(format!(
                "logs/guard.log: {bad} line(s) not in the guard format were left out"
            ));
        }
    }
}

/// `(gate, round)` from a gates/<PR>.yml (`gate: PASS`, `round: 1`).
fn parse_gate(text: &str) -> (Option<String>, Option<u32>) {
    let (mut gate, mut round) = (None, None);
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("gate:") {
            gate = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("round:") {
            round = v.trim().parse().ok();
        }
    }
    (gate, round)
}

/// The run folder's events. Reads only `inputs.files`; the returned `files` are the ones the events'
/// refs point into.
pub fn events_for(inputs: &RunInputs) -> RunEvents {
    let run_id = inputs
        .run_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut run = Run {
        inputs,
        make: Maker {
            run_id,
            events: Vec::new(),
            notices: Vec::new(),
            seqs: HashMap::new(),
        },
        cache: HashMap::new(),
    };
    let status_now = run.run_all();
    RunEvents {
        events: run.make.events,
        notices: run.make.notices,
        status_now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flight::host::run_inputs;
    use crate::flight::testkit::wave1_run;
    use crate::test_support::TempDir;

    fn wave1(name: &str) -> (TempDir, PathBuf, RunEvents) {
        let dir = TempDir::new(name);
        let (run, _) = wave1_run(&dir);
        let out = events_for(&run_inputs(&run));
        (dir, run, out)
    }

    fn of<'a>(out: &'a RunEvents, k: &str) -> Vec<&'a Event> {
        out.events.iter().filter(|e| e.kind == k).collect()
    }

    fn attr_of<'a>(e: &'a Event, key: &str) -> Option<&'a str> {
        e.attrs.get(key).and_then(|v| v.as_str())
    }

    fn assert_common(e: &Event, run: &std::path::Path) {
        assert_eq!(e.schema, SCHEMA);
        assert_eq!(e.agent.id, ORCHESTRATION);
        assert_eq!(e.provider.name, PROVIDER_DCV2);
        assert!(e.at.ends_with('Z') && e.at.len() == 24, "{}", e.at);
        let src = &e.content.source;
        assert!(
            src.file.starts_with(&run.display().to_string()),
            "{}",
            src.file
        );
        assert_eq!(src.sha256.len(), 64);
        assert_eq!(e.trace, "r-20261105-100000-fa17");
        // The ref is checkable: the bytes it names hash to its sha256.
        let bytes = std::fs::read(&src.file).unwrap();
        let slice = &bytes[src.offset as usize..(src.offset + src.len) as usize];
        assert_eq!(sha256_hex(slice), src.sha256, "{} {}", e.kind, src.file);
    }

    #[test]
    fn every_event_is_on_the_orchestration_lane_with_a_checkable_source_ref_and_unique_id() {
        let (_d, run, out) = wave1("dcv2_run_common");
        assert!(out.notices.is_empty(), "{:?}", out.notices);
        assert!(out.events.len() > 20, "{}", out.events.len());
        for e in &out.events {
            assert_common(e, &run);
        }
        let ids: HashSet<&str> = out.events.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids.len(), out.events.len(), "ids are unique");
        let again = events_for(&run_inputs(&run));
        let first: Vec<&str> = out.events.iter().map(|e| e.id.as_str()).collect();
        let second: Vec<&str> = again.events.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(first, second, "the same files give the same ids");
    }

    #[test]
    fn the_stage_column_gives_stage_changes_with_the_first_line_opening_the_first_stage() {
        let (_d, _run, out) = wave1("dcv2_run_stages");
        let stages: Vec<(&str, Option<&str>)> = of(&out, kind::STAGE_CHANGE)
            .iter()
            .map(|e| (e.name.as_deref().unwrap(), attr_of(e, attr::FROM)))
            .collect();
        assert_eq!(
            stages,
            [
                ("planning", None),
                ("plan-approval", Some("planning")),
                ("building", Some("plan-approval"))
            ]
        );
        assert_eq!(
            of(&out, kind::STAGE_CHANGE)[0].at,
            "2026-11-05T10:00:00.000Z"
        );
    }

    #[test]
    fn approval_bands_give_a_wait_and_a_decision_at_the_line_times() {
        let (_d, _run, out) = wave1("dcv2_run_approvals");
        let waits = of(&out, kind::APPROVAL_WAIT);
        let decisions = of(&out, kind::APPROVAL_DECISION);
        assert_eq!((waits.len(), decisions.len()), (1, 1));
        assert_eq!(waits[0].name.as_deref(), Some("plan"));
        assert_eq!(waits[0].at, "2026-11-05T10:03:00.000Z");
        assert_eq!(decisions[0].status.as_deref(), Some(status::OK));
        assert_eq!(decisions[0].at, "2026-11-05T10:06:00.000Z");
        // The approval lines are not also plain log rows: the four plain lines of the fixture remain.
        assert_eq!(of(&out, kind::LOG).len(), 4);
    }

    #[test]
    fn stop_check_lines_give_check_end_events_named_by_the_repos_check_with_the_output_as_content()
    {
        let (_d, run, out) = wave1("dcv2_run_stop_check");
        let checks: Vec<&Event> = of(&out, kind::CHECK_END)
            .into_iter()
            .filter(|e| attr_of(e, attr::SOURCE) == Some("stop-check"))
            .collect();
        assert_eq!(checks.len(), 2);
        let fail = checks
            .iter()
            .find(|e| e.status.as_deref() == Some(status::FAIL))
            .unwrap();
        assert_eq!(attr_of(fail, attr::PR), Some("PR2"));
        assert_eq!(
            fail.name.as_deref(),
            Some("cargo test --manifest-path src-tauri/Cargo.toml")
        );
        assert_eq!(fail.at, "2026-11-05T10:35:00.000Z");
        assert!(
            fail.content
                .source
                .file
                .ends_with("logs/PR2-stop-check.txt"),
            "{}",
            fail.content.source.file
        );
        let skip = checks
            .iter()
            .find(|e| e.status.as_deref() == Some(status::SKIP))
            .unwrap();
        assert!(
            skip.attrs.get(attr::PR).is_none(),
            "the main Stop has no PR"
        );
        assert!(skip.content.source.file.ends_with("log.md"));
        assert!(run.join("log.md").is_file());
    }

    #[test]
    fn pr_lines_give_pr_state_events_and_a_reviewing_to_fixing_change_also_gives_a_retry() {
        let (_d, _run, out) = wave1("dcv2_run_pr_state");
        let states: Vec<(&str, &str, &str)> = of(&out, kind::PR_STATE)
            .iter()
            .map(|e| {
                (
                    attr_of(e, attr::PR).unwrap(),
                    attr_of(e, attr::FROM).unwrap(),
                    attr_of(e, attr::TO).unwrap(),
                )
            })
            .collect();
        assert_eq!(
            states,
            [
                ("PR2", "-", "building"),
                ("PR2", "building", "reviewing"),
                ("PR2", "reviewing", "fixing")
            ]
        );
        let retries = of(&out, kind::RETRY);
        assert_eq!(retries.len(), 2, "the pr line and the `retried` line");
        assert!(retries
            .iter()
            .all(|e| e.derived.as_deref() == Some(LOG_LINE)));
        assert!(retries.iter().all(|e| attr_of(e, attr::PR) == Some("PR2")));
    }

    #[test]
    fn app_lines_are_ignored_and_other_lines_are_log_events_with_no_log_text() {
        let (_d, _run, out) = wave1("dcv2_run_log_rows");
        let rows = of(&out, kind::LOG);
        assert!(!rows.is_empty());
        for e in &rows {
            assert!(e.name.is_none() && e.attrs.is_empty(), "{e:?}");
            assert!(e.status.is_none());
        }
        let json = serde_json::to_string(&out.events).unwrap();
        for text in [
            "Planner wrote plan.md",
            "REQUEST CHANGES",
            "no JSON block",
            "run started",
        ] {
            assert!(!json.contains(text), "{text} leaked into an event");
        }
        assert!(!json.contains("approval-wait"), "app: lines are left out");
    }

    #[test]
    fn status_json_checks_become_skill_check_events_at_the_pr_update_time() {
        let (_d, _run, out) = wave1("dcv2_run_skill_check");
        let skill: Vec<&Event> = of(&out, kind::CHECK_END)
            .into_iter()
            .filter(|e| attr_of(e, attr::SOURCE) == Some("skill"))
            .collect();
        assert_eq!(skill.len(), 1);
        assert_eq!(skill[0].status.as_deref(), Some(status::FAIL));
        assert_eq!(skill[0].at, "2026-11-05T10:21:00.000Z");
        assert_eq!(attr_of(skill[0], attr::PR), Some("PR2"));
        assert!(skill[0].content.source.file.ends_with("status.json"));
    }

    #[test]
    fn app_checks_become_app_check_events_with_head_and_exit_code() {
        let (_d, _run, out) = wave1("dcv2_run_app_check");
        let app: Vec<&Event> = of(&out, kind::CHECK_END)
            .into_iter()
            .filter(|e| attr_of(e, attr::SOURCE) == Some("app"))
            .collect();
        assert_eq!(app.len(), 1);
        let e = app[0];
        assert_eq!(e.status.as_deref(), Some(status::FAIL));
        assert_eq!(e.at, "2026-11-05T10:22:00.000Z");
        assert_eq!(attr_of(e, attr::HEAD), Some("abc1234"));
        assert_eq!(e.attrs[attr::EXIT_CODE], 101);
        assert_eq!(
            e.name.as_deref(),
            Some("cargo test --manifest-path src-tauri/Cargo.toml")
        );
        assert!(e.content.source.file.ends_with("logs/PR2-app-check.txt"));
    }

    #[test]
    fn a_review_file_gives_a_round_event_with_the_gate_verdict_on_the_last_round() {
        let (_d, _run, out) = wave1("dcv2_run_review");
        let rounds = of(&out, kind::REVIEW_ROUND);
        assert_eq!(rounds.len(), 1);
        let e = rounds[0];
        assert_eq!(attr_of(e, attr::PR), Some("PR2"));
        assert_eq!(e.attrs[attr::ROUND], 1);
        assert_eq!(attr_of(e, attr::VERDICT), Some("FAIL"));
        assert!(e.content.source.file.ends_with("reviews/PR2-round1.md"));
        assert_eq!(e.content.source.offset, 0);
    }

    #[test]
    fn the_gate_verdict_goes_only_to_its_own_round() {
        let dir = TempDir::new("dcv2_run_two_rounds");
        let (run, _) = wave1_run(&dir);
        std::fs::write(run.join("reviews/PR2-round2.md"), "## Verdict\nAPPROVED\n").unwrap();
        std::fs::write(run.join("reviews/plan-round1.md"), "## Verdict\nAPPROVED\n").unwrap();
        std::fs::write(run.join("gates/PR2.yml"), "gate: PASS\nround: 2\n").unwrap();
        let out = events_for(&run_inputs(&run));
        let rounds = of(&out, kind::REVIEW_ROUND);
        let by_name: HashMap<&str, Option<&str>> = rounds
            .iter()
            .map(|e| (e.name.as_deref().unwrap(), attr_of(e, attr::VERDICT)))
            .collect();
        assert_eq!(by_name["PR2-round1"], None);
        assert_eq!(by_name["PR2-round2"], Some("PASS"));
        assert_eq!(by_name["plan-round1"], None);
        let plan = rounds
            .iter()
            .find(|e| e.name.as_deref() == Some("plan-round1"))
            .unwrap();
        assert!(plan.attrs.get(attr::PR).is_none());
    }

    #[test]
    fn app_github_gives_the_ci_state_and_requests_give_kind_and_result_only() {
        let (_d, _run, out) = wave1("dcv2_run_ci_requests");
        let ci = of(&out, kind::CI_STATE);
        assert_eq!(ci.len(), 1);
        assert_eq!(attr_of(ci[0], attr::PR), Some("PR2"));
        assert_eq!(attr_of(ci[0], attr::CI), Some("fail"));
        assert_eq!(attr_of(ci[0], attr::PR_STATE), Some("OPEN"));
        assert_eq!(ci[0].status.as_deref(), Some(status::FAIL));
        let reqs = of(&out, kind::REQUEST);
        assert_eq!(reqs.len(), 2);
        assert!(reqs
            .iter()
            .all(|e| attr_of(e, attr::REQUEST_KIND) == Some("recheck")));
        let result = reqs
            .iter()
            .find(|e| attr_of(e, attr::RESULT).is_some())
            .unwrap();
        assert_eq!(attr_of(result, attr::RESULT), Some("done"));
        assert_eq!(result.status.as_deref(), Some(status::OK));
        let json = serde_json::to_string(&reqs).unwrap();
        assert!(
            !json.contains("PR2: fail"),
            "the result message stays in the file"
        );
    }

    #[test]
    fn guard_log_lines_give_tool_blocked_events_with_rule_tool_agent_and_span() {
        let (_d, _run, out) = wave1("dcv2_run_guard");
        let blocked = of(&out, kind::TOOL_BLOCKED);
        assert_eq!(blocked.len(), 2);
        let one = blocked.iter().find(|e| e.span.is_some()).unwrap();
        assert_eq!(one.span.as_deref(), Some("toolu_guard01"));
        assert_eq!(attr_of(one, attr::RULE), Some("run-file-write"));
        assert_eq!(attr_of(one, attr::TOOL), Some("Bash"));
        assert_eq!(attr_of(one, attr::AGENT), Some("a1pr2impl0000000001"));
        assert_eq!(one.status.as_deref(), Some(status::BLOCKED));
        let other = blocked.iter().find(|e| e.span.is_none()).unwrap();
        assert_eq!(attr_of(other, attr::RULE), Some("git-push"));
        assert!(one.attrs.get("dcv2Guard").is_none());
    }

    #[test]
    fn a_malformed_file_is_a_notice_naming_it_and_the_other_events_stay() {
        let dir = TempDir::new("dcv2_run_malformed");
        let (run, _) = wave1_run(&dir);
        let before = events_for(&run_inputs(&run));
        std::fs::write(run.join("status.json"), "{ not json").unwrap();
        let out = events_for(&run_inputs(&run));
        assert!(
            out.notices.iter().any(|n| n.starts_with("status.json")),
            "{:?}",
            out.notices
        );
        assert!(of(&out, kind::CHECK_END)
            .iter()
            .all(|e| attr_of(e, attr::SOURCE) != Some("skill")));
        assert_eq!(out.events.len(), before.events.len() - 1);
        assert!(!of(&out, kind::APPROVAL_WAIT).is_empty());
        // A bad guard.log line is counted, not fatal.
        std::fs::write(
            run.join("logs/guard.log"),
            "garbage\n2026-11-05T10:12:00Z\tr\tBash\tmain\t-\n",
        )
        .unwrap();
        let out = events_for(&run_inputs(&run));
        assert!(out.notices.iter().any(|n| n.contains("guard.log")));
        assert_eq!(of(&out, kind::TOOL_BLOCKED).len(), 1);
    }

    #[test]
    fn a_file_over_8_mib_is_skipped_with_a_notice() {
        let dir = TempDir::new("dcv2_run_big");
        let (run, _) = wave1_run(&dir);
        let big = vec![b'x'; (MAX_FILE + 1) as usize];
        std::fs::write(run.join("app-checks.json"), big).unwrap();
        let out = events_for(&run_inputs(&run));
        assert!(
            out.notices
                .iter()
                .any(|n| n.contains("app-checks.json") && n.contains("8 MiB")),
            "{:?}",
            out.notices
        );
        assert!(of(&out, kind::CHECK_END)
            .iter()
            .all(|e| attr_of(e, attr::SOURCE) != Some("app")));
    }

    #[test]
    fn the_adapter_writes_nothing_and_a_run_with_no_files_gives_nothing() {
        let dir = TempDir::new("dcv2_run_readonly");
        let (run, _) = wave1_run(&dir);
        let listing = |p: &std::path::Path| {
            let mut all = Vec::new();
            fn walk(p: &std::path::Path, all: &mut Vec<(String, u64)>) {
                for e in std::fs::read_dir(p).unwrap().flatten() {
                    if e.path().is_dir() {
                        walk(&e.path(), all);
                    } else {
                        all.push((e.path().display().to_string(), e.metadata().unwrap().len()));
                    }
                }
            }
            walk(p, &mut all);
            all.sort();
            all
        };
        let before = listing(&run);
        events_for(&run_inputs(&run));
        assert_eq!(listing(&run), before);
        let empty = TempDir::new("dcv2_run_empty");
        let out = events_for(&run_inputs(empty.path()));
        assert!(out.events.is_empty() && out.notices.is_empty());
    }

    #[test]
    fn an_old_run_with_no_plugin_lines_still_gives_stages_and_checks() {
        let dir = TempDir::new("dcv2_run_old");
        let (run, _) = wave1_run(&dir);
        let log = std::fs::read_to_string(run.join("log.md")).unwrap();
        let old: String = log
            .lines()
            .filter(|l| !l.contains(" stop-check ") && !l.contains(" pr PR2 "))
            .map(|l| format!("{l}\n"))
            .collect();
        std::fs::write(run.join("log.md"), old).unwrap();
        let out = events_for(&run_inputs(&run));
        assert!(of(&out, kind::PR_STATE).is_empty());
        assert!(!of(&out, kind::STAGE_CHANGE).is_empty());
        assert!(!of(&out, kind::CHECK_END).is_empty());
    }
}
