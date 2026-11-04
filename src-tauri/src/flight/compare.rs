//! Compare two runs (AD-47): the rows of the Compare page, in one fixed group order, and the Divergence (the first
//! row that differs). Pure: the host reads each side from the index and the run record and hands the values in;
//! nothing here opens a file or a database. The page never diffs by itself: every `differs` is decided here.

use super::derive;
use super::schema::{attr, kind, Event, Failure};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The row groups, in the order the page shows them (and so the order the Divergence is looked for in).
pub const GROUPS: [&str; 9] = [
    "stages",
    "tools",
    "failures",
    "retries",
    "repeats",
    "files",
    "checks",
    "tokens",
    "windowShare",
];

/// One indexed Event of a side: the index's columns, plus the whole Event for the kinds that need it
/// (`tool.start` for a Repeat's input hash, a failing Event, `check.end` for its source).
#[derive(Debug, Clone, Default)]
pub struct IndexedEvent {
    pub id: String,
    pub kind: String,
    pub name: Option<String>,
    pub status: Option<String>,
    pub pr: Option<String>,
    /// The label of the Failure this Event is (`failure` column); `None` for an Event that is not one.
    pub failure: Option<String>,
    /// The guard rule id that blocked it (`rule` column).
    pub rule: Option<String>,
    pub paths: Vec<String>,
    pub event: Option<Event>,
}

/// The run record's numbers (`app-usage.json`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Record {
    pub tokens: Tokens,
    /// Active minutes per stage.
    pub minutes_by_stage: BTreeMap<String, f64>,
}

/// The share of the account's usage windows a run used, in percent points; `None` = not recorded.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Share {
    pub five_hour: Option<f64>,
    pub weekly: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct Side {
    pub events: Vec<IndexedEvent>,
    /// `None` for a run with no record yet (every record number is then "not recorded").
    pub record: Option<Record>,
    pub share: Share,
}

/// One row: `a` and `b` are a number, a text or null (not recorded).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    pub group: String,
    pub key: String,
    pub a: Value,
    pub b: Value,
    pub differs: bool,
    /// The first Event of this row in run A / run B, so a click opens the Trace there (none for record numbers).
    pub a_event: Option<String>,
    pub b_event: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rows {
    pub rows: Vec<Row>,
    /// The index (into `rows`) of the first row that differs.
    pub divergence: Option<usize>,
}

type Cells = BTreeMap<String, Value>;
/// Per group and key, the id of the first Event that made the cell.
type Firsts = BTreeMap<&'static str, BTreeMap<String, String>>;

fn bump(cells: &mut Cells, key: String) {
    let n = cells.get(&key).and_then(Value::as_u64).unwrap_or(0);
    cells.insert(key, json!(n + 1));
}

fn pr_of(e: &IndexedEvent) -> String {
    e.pr.clone()
        .or_else(|| {
            e.event
                .as_ref()
                .and_then(|ev| ev.attrs.get(attr::PR))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "-".into())
}

fn repeat_names(failures: &[Failure], out: &mut Cells, firsts: &mut BTreeMap<String, String>) {
    for f in failures {
        for repeat in &f.repeats {
            let name = repeat.name.clone().unwrap_or_else(|| "-".into());
            firsts
                .entry(name.clone())
                .or_insert_with(|| repeat.id.clone());
            bump(out, name);
        }
        repeat_names(&f.repeats, out, firsts);
    }
}

/// The cells of every group of one side. Count groups hold only the keys the side has (the other side's
/// missing key reads as 0); record groups hold `null` for a number not recorded.
fn cells(side: &Side) -> (BTreeMap<&'static str, Cells>, Firsts) {
    let mut firsts: Firsts = BTreeMap::new();
    let mut first = |group: &'static str, key: &str, id: &str| {
        firsts
            .entry(group)
            .or_default()
            .entry(key.to_string())
            .or_insert_with(|| id.to_string());
    };
    let mut stages = Cells::new();
    let mut tokens = Cells::new();
    if let Some(record) = &side.record {
        for (stage, minutes) in &record.minutes_by_stage {
            stages.insert(stage.clone(), json!((minutes * 10.0).round() / 10.0));
        }
        let t = &record.tokens;
        for (key, value) in [
            ("input", t.input),
            ("output", t.output),
            ("cacheWrite", t.cache_write),
            ("cacheRead", t.cache_read),
        ] {
            tokens.insert(key.into(), json!(value));
        }
    }
    let (mut tools, mut failures, mut retries, mut files, mut checks) = (
        Cells::new(),
        Cells::new(),
        Cells::new(),
        Cells::new(),
        Cells::new(),
    );
    for e in &side.events {
        match e.kind.as_str() {
            kind::TOOL_START => {
                let name = e.name.clone().unwrap_or_else(|| "-".into());
                first("tools", &name, &e.id);
                bump(&mut tools, name);
            }
            kind::RETRY => {
                let pr = pr_of(e);
                first("retries", &pr, &e.id);
                bump(&mut retries, pr);
            }
            kind::CHECK_END => {
                let source = e
                    .event
                    .as_ref()
                    .and_then(|ev| ev.attrs.get(attr::SOURCE))
                    .and_then(Value::as_str)
                    .unwrap_or("check");
                // The last result of a (source, PR) wins.
                let key = format!("{source}/{}", pr_of(e));
                first("checks", &key, &e.id);
                checks.insert(
                    key,
                    json!(e.status.clone().unwrap_or_else(|| "unknown".into())),
                );
            }
            _ => {}
        }
        if e.failure.is_some() {
            let key = match &e.rule {
                Some(rule) => format!("{}:{rule}", e.kind),
                None => e.kind.clone(),
            };
            first("failures", &key, &e.id);
            bump(&mut failures, key);
        }
        for path in &e.paths {
            first("files", path, &e.id);
            bump(&mut files, path.clone());
        }
    }
    // Repeats: per session, the Failures the transcript's own events make, nested repeats counted.
    let mut repeats = Cells::new();
    let mut sessions: BTreeMap<&str, Vec<Event>> = BTreeMap::new();
    for ev in side.events.iter().filter_map(|e| e.event.as_ref()) {
        if ev.provider.name != super::schema::PROVIDER_DCV2 {
            sessions
                .entry(ev.session.as_str())
                .or_default()
                .push(ev.clone());
        }
    }
    let mut repeat_firsts = BTreeMap::new();
    for events in sessions.values() {
        repeat_names(
            &derive::failures_with_repeats(events),
            &mut repeats,
            &mut repeat_firsts,
        );
    }
    firsts.insert("repeats", repeat_firsts);
    let mut share = Cells::new();
    for (key, value) in [
        ("fiveHour", side.share.five_hour),
        ("weekly", side.share.weekly),
    ] {
        share.insert(
            key.into(),
            value.map_or(Value::Null, |v| json!((v * 10.0).round() / 10.0)),
        );
    }
    (
        BTreeMap::from([
            ("stages", stages),
            ("tools", tools),
            ("failures", failures),
            ("retries", retries),
            ("repeats", repeats),
            ("files", files),
            ("checks", checks),
            ("tokens", tokens),
            ("windowShare", share),
        ]),
        firsts,
    )
}

/// Whether a group's missing key reads as 0 (a count) or as "not recorded" (null).
fn counts(group: &str) -> bool {
    matches!(
        group,
        "tools" | "failures" | "retries" | "repeats" | "files"
    )
}

/// The rows of two sides, groups in the fixed order and keys sorted inside a group, with the Divergence.
/// A number not recorded is `null`; it differs from the other side's value unless that is null too.
pub fn rows(a: &Side, b: &Side) -> Rows {
    let ((ca, fa), (cb, fb)) = (cells(a), cells(b));
    let mut out = Vec::new();
    for group in GROUPS {
        let (ga, gb) = (&ca[group], &cb[group]);
        let mut keys: Vec<&String> = ga.keys().chain(gb.keys()).collect();
        keys.sort();
        keys.dedup();
        let missing = if counts(group) { json!(0) } else { Value::Null };
        for key in keys {
            let va = ga.get(key).cloned().unwrap_or_else(|| missing.clone());
            let vb = gb.get(key).cloned().unwrap_or_else(|| missing.clone());
            let event = |f: &Firsts| f.get(group).and_then(|g| g.get(key)).cloned();
            out.push(Row {
                group: group.into(),
                key: key.clone(),
                differs: va != vb,
                a: va,
                b: vb,
                a_event: event(&fa),
                b_event: event(&fb),
            });
        }
    }
    let divergence = out.iter().position(|r| r.differs);
    Rows {
        rows: out,
        divergence,
    }
}

/// A reading of one usage window: when it was seen, the percent used, and when that window resets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    pub at_ms: i64,
    pub utilization: Option<f64>,
    pub resets_at: Option<i64>,
}

/// How much of one usage window a run used between `start_ms` and `end_ms`: the rise in percent from the last
/// reading at or before the start (else the first inside the span) to the last reading inside the span, only
/// when both belong to the same window (same reset). Fewer than two usable readings, or a different window
/// (the window reset during the run), give `None`: "not recorded". An estimate, never negative.
pub fn window_share(readings: &[Reading], start_ms: i64, end_ms: i64) -> Option<f64> {
    let usable: Vec<&Reading> = readings
        .iter()
        .filter(|r| r.utilization.is_some() && r.resets_at.is_some())
        .collect();
    let end = usable
        .iter()
        .rev()
        .find(|r| r.at_ms <= end_ms && r.at_ms >= start_ms)?;
    let base = usable
        .iter()
        .rev()
        .find(|r| r.at_ms <= start_ms)
        .or_else(|| {
            usable
                .iter()
                .find(|r| r.at_ms >= start_ms && r.at_ms < end.at_ms)
        })?;
    if base.at_ms == end.at_ms || base.resets_at != end.resets_at {
        return None;
    }
    Some((end.utilization? - base.utilization?).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flight::schema::{AgentRef, CapturePolicy, Content, Provider, SourceRef};
    use serde_json::Map;

    fn ev(kind_: &str) -> IndexedEvent {
        IndexedEvent {
            kind: kind_.into(),
            ..Default::default()
        }
    }

    fn tool(name: &str) -> IndexedEvent {
        IndexedEvent {
            name: Some(name.into()),
            ..ev(kind::TOOL_START)
        }
    }

    fn block(rule: Option<&str>) -> IndexedEvent {
        IndexedEvent {
            failure: Some("exact:hook-denial:dcv2-guard".into()),
            rule: rule.map(str::to_string),
            ..ev(kind::TOOL_BLOCKED)
        }
    }

    fn record(minutes: &[(&str, f64)], input: u64) -> Record {
        Record {
            tokens: Tokens {
                input,
                output: 5,
                cache_write: 0,
                cache_read: 0,
            },
            minutes_by_stage: minutes.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        }
    }

    fn side(events: Vec<IndexedEvent>) -> Side {
        Side {
            events,
            record: Some(record(&[("building", 10.0)], 100)),
            share: Share::default(),
        }
    }

    #[test]
    fn the_divergence_is_the_guard_block_row_when_only_a_block_differs() {
        let pass = side(vec![tool("Bash"), tool("Edit")]);
        let fail = side(vec![
            tool("Bash"),
            tool("Edit"),
            block(Some("run-file-write")),
        ]);
        let r = rows(&pass, &fail);
        let at = r.divergence.expect("a difference");
        assert_eq!(r.rows[at].group, "failures");
        assert_eq!(r.rows[at].key, "tool.blocked:run-file-write");
        assert_eq!((&r.rows[at].a, &r.rows[at].b), (&json!(0), &json!(1)));
        assert_eq!(r.rows.iter().filter(|row| row.differs).count(), 1);
        // The same two sides compare as equal.
        assert_eq!(rows(&pass, &pass).divergence, None);
    }

    #[test]
    fn groups_come_in_the_fixed_order_and_keys_are_sorted() {
        let a = Side {
            events: vec![tool("Edit"), tool("Bash")],
            record: Some(record(&[("planning", 1.0), ("building", 2.0)], 1)),
            share: Share {
                five_hour: Some(3.0),
                weekly: None,
            },
        };
        let r = rows(&a, &a);
        let mut seen: Vec<&str> = r.rows.iter().map(|row| row.group.as_str()).collect();
        seen.dedup();
        assert_eq!(
            seen,
            ["stages", "tools", "tokens", "windowShare"],
            "groups with rows, in order"
        );
        let tools: Vec<&str> = r
            .rows
            .iter()
            .filter(|x| x.group == "tools")
            .map(|x| x.key.as_str())
            .collect();
        assert_eq!(tools, ["Bash", "Edit"]);
        let stages: Vec<&str> = r
            .rows
            .iter()
            .filter(|x| x.group == "stages")
            .map(|x| x.key.as_str())
            .collect();
        assert_eq!(stages, ["building", "planning"]);
        for (i, group) in GROUPS.iter().enumerate() {
            assert_eq!(GROUPS.iter().position(|g| g == group), Some(i));
        }
    }

    #[test]
    fn a_number_not_recorded_differs_only_when_the_other_side_has_one() {
        let mut a = side(vec![]);
        let mut b = side(vec![]);
        a.share.five_hour = None;
        b.share.five_hour = None;
        let r = rows(&a, &b);
        let five = r.rows.iter().find(|x| x.key == "fiveHour").unwrap();
        assert_eq!(
            (&five.a, &five.b, five.differs),
            (&Value::Null, &Value::Null, false)
        );
        b.share.five_hour = Some(4.25);
        let r = rows(&a, &b);
        let five = r.rows.iter().find(|x| x.key == "fiveHour").unwrap();
        assert_eq!(
            (&five.a, &five.b, five.differs),
            (&Value::Null, &json!(4.3), true)
        );
        // A run with no record at all: every record number is null, and a stage only the other has differs.
        let none = Side::default();
        let r = rows(&none, &side(vec![]));
        let building = r
            .rows
            .iter()
            .find(|x| x.group == "stages" && x.key == "building")
            .unwrap();
        assert_eq!((&building.a, building.differs), (&Value::Null, true));
        let tokens: Vec<&Row> = r.rows.iter().filter(|x| x.group == "tokens").collect();
        assert_eq!(tokens.len(), 4);
        assert!(tokens.iter().all(|t| t.a == Value::Null && t.differs));
        // A count missing on one side reads as 0.
        let counts = rows(&side(vec![]), &side(vec![tool("Bash")]));
        let bash = counts.rows.iter().find(|x| x.group == "tools").unwrap();
        assert_eq!((&bash.a, &bash.b), (&json!(0), &json!(1)));
    }

    fn event_with(
        kind_: &str,
        session: &str,
        id: &str,
        name: &str,
        attrs: Map<String, Value>,
    ) -> Event {
        Event {
            schema: 1,
            id: id.into(),
            trace: "t".into(),
            session: session.into(),
            agent: AgentRef::main(),
            provider: Provider {
                name: "claude".into(),
                version: None,
                native: "x".into(),
            },
            kind: kind_.into(),
            at: "2026-11-05T10:00:00Z".into(),
            span: Some(format!("s-{id}")),
            status: (kind_ == kind::TOOL_END).then(|| "error".to_string()),
            name: Some(name.into()),
            attrs,
            usage: None,
            content: Content {
                source: SourceRef {
                    file: "/c/s.jsonl".into(),
                    offset: 0,
                    len: 1,
                    sha256: id.into(),
                },
                policy: CapturePolicy::Metadata,
                redactions: 0,
            },
            derived: None,
        }
    }

    #[test]
    fn retries_checks_files_and_repeats_have_their_own_rows() {
        let retry = |pr: &str| IndexedEvent {
            pr: Some(pr.into()),
            ..ev(kind::RETRY)
        };
        let check = |status: &str, source: &str| {
            let mut attrs = Map::new();
            attrs.insert(attr::SOURCE.into(), json!(source));
            IndexedEvent {
                status: Some(status.into()),
                pr: Some("PR1".into()),
                event: Some(event_with(kind::CHECK_END, "d", "c", "check", attrs)),
                ..ev(kind::CHECK_END)
            }
        };
        let edit = IndexedEvent {
            paths: vec!["/w/a.rs".into(), "/w/b.rs".into()],
            ..ev(kind::FILE_CHANGE)
        };
        // Two identical failing Bash calls in one session: one Repeat of Bash.
        let mut input = Map::new();
        input.insert("inputHash".into(), json!("h1"));
        let mut events = Vec::new();
        for n in 0..2 {
            events.push(IndexedEvent {
                event: Some(event_with(
                    kind::TOOL_START,
                    "s",
                    &format!("start{n}"),
                    "Bash",
                    input.clone(),
                )),
                ..tool("Bash")
            });
            let mut end = event_with(kind::TOOL_END, "s", &format!("end{n}"), "Bash", Map::new());
            end.span = Some(format!("s-start{n}"));
            events.push(IndexedEvent {
                status: Some("error".into()),
                failure: Some("exact:is_error".into()),
                event: Some(end),
                ..ev(kind::TOOL_END)
            });
        }
        events.extend([
            retry("PR1"),
            retry("PR1"),
            retry("PR2"),
            check("fail", "skill"),
            check("pass", "skill"),
            check("pass", "app"),
            edit,
        ]);
        let a = side(events);
        let r = rows(&a, &side(vec![]));
        let get = |group: &str, key: &str| {
            r.rows
                .iter()
                .find(|x| x.group == group && x.key == key)
                .map(|x| (x.a.clone(), x.b.clone(), x.differs))
        };
        assert_eq!(get("retries", "PR1"), Some((json!(2), json!(0), true)));
        assert_eq!(get("retries", "PR2"), Some((json!(1), json!(0), true)));
        assert_eq!(
            get("checks", "skill/PR1"),
            Some((json!("pass"), Value::Null, true)),
            "the last result wins"
        );
        assert_eq!(
            get("checks", "app/PR1"),
            Some((json!("pass"), Value::Null, true))
        );
        assert_eq!(get("files", "/w/a.rs"), Some((json!(1), json!(0), true)));
        assert_eq!(
            get("failures", "tool.end"),
            Some((json!(2), json!(0), true))
        );
        assert_eq!(get("repeats", "Bash"), Some((json!(1), json!(0), true)));
        assert_eq!(get("tools", "Bash"), Some((json!(2), json!(0), true)));
        assert!(r.divergence.is_some());
    }

    fn reading(at_ms: i64, utilization: f64, resets_at: i64) -> Reading {
        Reading {
            at_ms,
            utilization: Some(utilization),
            resets_at: Some(resets_at),
        }
    }

    #[test]
    fn window_share_needs_two_readings_of_the_same_window() {
        let r = [
            reading(100, 10.0, 9000),
            reading(200, 12.0, 9000),
            reading(300, 15.5, 9000),
            reading(900, 20.0, 9000),
        ];
        // Baseline: the last reading at or before the start; end: the last inside the span.
        assert_eq!(window_share(&r, 150, 400), Some(5.5));
        assert_eq!(window_share(&r, 200, 400), Some(3.5));
        assert_eq!(window_share(&r, 100, 300), Some(5.5));
        // Before any reading: the first inside the span is the baseline.
        assert_eq!(window_share(&r, 50, 300), Some(5.5));
        // Only one reading in the span and none before: not recorded.
        assert_eq!(
            window_share(&r, 850, 1000),
            Some(4.5),
            "baseline is the reading before"
        );
        assert_eq!(
            window_share(&r, 1000, 2000),
            None,
            "no reading inside the span"
        );
        assert_eq!(window_share(&[reading(300, 5.0, 9000)], 100, 400), None);
        assert_eq!(window_share(&[], 0, 10), None);
        // The window reset during the run: not recorded.
        let reset = [reading(100, 90.0, 9000), reading(300, 4.0, 18000)];
        assert_eq!(window_share(&reset, 100, 400), None);
        // Readings with no percent or reset are ignored; a drop never gives a negative share.
        let partial = [
            Reading {
                at_ms: 100,
                utilization: None,
                resets_at: Some(9000),
            },
            reading(200, 9.0, 9000),
            reading(300, 8.0, 9000),
        ];
        assert_eq!(window_share(&partial, 100, 400), Some(0.0));
    }
}
