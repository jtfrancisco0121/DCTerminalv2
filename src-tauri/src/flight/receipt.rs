//! The shape of `<run>/app-receipt.json` (docs/run-folder.md, "Files the app adds"), the app-only facts of
//! a PR body Receipt: which agents ran, minutes, tokens and the run's share of the five-hour window.
//! Pure: it shapes numbers into the schema-1 JSON and decides when the file is rewritten. Reading the run,
//! the sessions and the usage history, and the atomic write, are `host.rs`.
//!
//! Counts, minutes and ids only: never transcript text, a config folder or an account name.

use serde_json::{json, Value};

/// The file's schema; a reader refuses another.
pub const SCHEMA: u32 = 1;
/// The file name in the run folder.
pub const FILE: &str = "app-receipt.json";

/// Stages at which the receipt is (re)computed: the PR bodies are built from `ship-approval` on.
pub const STAGES: [&str; 2] = ["ship-approval", "shipping"];

/// One agent kind: how many lanes ran and their active time.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentUse {
    pub kind: String,
    pub count: u32,
    pub active_minutes: f64,
}

/// Token totals over the run's sessions (cache writes are the 5-minute and 1-hour ones together).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenTotals {
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
}

/// Everything the file holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    pub computed_at: String,
    pub status_updated_at: String,
    pub agents: Vec<AgentUse>,
    pub active_minutes: f64,
    pub wait_minutes: f64,
    pub tokens: TokenTotals,
    /// The run's share of the account's five-hour window, in percent points; `None`: not known.
    pub window_share_points: Option<f64>,
}

/// Minutes with one decimal.
pub fn round1(minutes: f64) -> f64 {
    if minutes.is_finite() {
        (minutes.max(0.0) * 10.0).round() / 10.0
    } else {
        0.0
    }
}

/// Percent points to a 0-1 fraction (four decimals); `None` for a missing or non-finite figure.
pub fn share_fraction(points: Option<f64>) -> Option<f64> {
    let p = points.filter(|p| p.is_finite())?;
    Some(((p / 100.0).clamp(0.0, 1.0) * 10_000.0).round() / 10_000.0)
}

/// Lanes `(kind, started ms, finished ms)` grouped by kind, sorted by kind. A lane without both times
/// counts as an agent but adds no active time.
pub fn group_lanes(lanes: &[(String, Option<i64>, Option<i64>)]) -> Vec<AgentUse> {
    let mut out: Vec<AgentUse> = Vec::new();
    for (kind, start, finish) in lanes {
        let ms = match (start, finish) {
            (Some(a), Some(b)) => (b - a).max(0),
            _ => 0,
        };
        let minutes = ms as f64 / 60_000.0;
        match out.iter_mut().find(|a| &a.kind == kind) {
            Some(a) => {
                a.count += 1;
                a.active_minutes += minutes;
            }
            None => out.push(AgentUse {
                kind: kind.clone(),
                count: 1,
                active_minutes: minutes,
            }),
        }
    }
    out.sort_by(|a, b| a.kind.cmp(&b.kind));
    out
}

/// The schema-1 document.
pub fn to_json(facts: &Facts) -> Value {
    json!({
        "schema": SCHEMA,
        "computedAt": facts.computed_at,
        "statusUpdatedAt": facts.status_updated_at,
        "agents": facts.agents.iter().map(|a| json!({
            "kind": a.kind,
            "count": a.count,
            "activeMinutes": round1(a.active_minutes),
        })).collect::<Vec<_>>(),
        "minutes": {
            "active": round1(facts.active_minutes),
            "wait": round1(facts.wait_minutes),
        },
        "tokens": {
            "input": facts.tokens.input,
            "output": facts.tokens.output,
            "cacheWrite": facts.tokens.cache_write,
            "cacheRead": facts.tokens.cache_read,
        },
        "windowShare": share_fraction(facts.window_share_points),
    })
}

/// Whether the file is (re)written: the run is at a stage in [`STAGES`], one of its repos has `receipt: true`,
/// status.json has an `updatedAt`, and the file does not already carry it.
pub fn should_write(
    stage: &str,
    receipt_repo: bool,
    status_updated_at: Option<&str>,
    existing_status_updated_at: Option<&str>,
) -> bool {
    receipt_repo
        && STAGES.contains(&stage)
        && status_updated_at.is_some_and(|u| !u.is_empty())
        && existing_status_updated_at != status_updated_at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            computed_at: "2026-11-06T10:00:00Z".into(),
            status_updated_at: "2026-11-06T09:59:00Z".into(),
            agents: group_lanes(&[
                ("implementer".into(), Some(0), Some(150_000)),
                ("orchestrator".into(), Some(0), Some(600_000)),
                ("implementer".into(), Some(0), Some(90_000)),
                ("pr-reviewer".into(), None, None),
            ]),
            active_minutes: 12.345,
            wait_minutes: 3.04,
            tokens: TokenTotals {
                input: 10,
                output: 20,
                cache_write: 30,
                cache_read: 40,
            },
            window_share_points: Some(12.5),
        }
    }

    #[test]
    fn the_file_has_the_schema_1_shape_and_rounds_minutes_to_one_decimal() {
        let v = to_json(&facts());
        assert_eq!(v["schema"], 1);
        assert_eq!(v["computedAt"], "2026-11-06T10:00:00Z");
        assert_eq!(v["statusUpdatedAt"], "2026-11-06T09:59:00Z");
        assert_eq!(v["minutes"]["active"], 12.3);
        assert_eq!(v["minutes"]["wait"], 3.0);
        assert_eq!(v["tokens"]["cacheWrite"], 30);
        assert_eq!(v["tokens"]["cacheRead"], 40);
        assert_eq!(v["windowShare"], 0.125);
        let agents = v["agents"].as_array().unwrap();
        let kinds: Vec<_> = agents.iter().map(|a| a["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["implementer", "orchestrator", "pr-reviewer"]);
        assert_eq!(agents[0]["count"], 2);
        assert_eq!(agents[0]["activeMinutes"], 4.0);
        assert_eq!(agents[2]["count"], 1);
        assert_eq!(agents[2]["activeMinutes"], 0.0);
    }

    #[test]
    fn the_window_share_is_a_fraction_or_null() {
        assert_eq!(share_fraction(None), None);
        assert_eq!(share_fraction(Some(f64::NAN)), None);
        assert_eq!(share_fraction(Some(250.0)), Some(1.0));
        assert_eq!(share_fraction(Some(-4.0)), Some(0.0));
        let mut f = facts();
        f.window_share_points = None;
        assert!(to_json(&f)["windowShare"].is_null());
    }

    #[test]
    fn nothing_but_numbers_and_ids_reaches_the_file() {
        let text = to_json(&facts()).to_string();
        for needle in ["config", "account", "/", "@"] {
            assert!(!text.contains(needle), "{needle} in {text}");
        }
    }

    #[test]
    fn it_is_rewritten_only_for_a_receipt_repo_at_a_late_stage_with_a_new_updated_at() {
        let at = Some("2026-11-06T09:59:00Z");
        assert!(should_write("ship-approval", true, at, None));
        assert!(should_write("shipping", true, at, Some("older")));
        assert!(
            !should_write("ship-approval", true, at, at),
            "same updatedAt"
        );
        assert!(
            !should_write("ship-approval", false, at, None),
            "no receipt repo"
        );
        assert!(!should_write("building", true, at, None), "too early");
        assert!(!should_write("done", true, at, None));
        assert!(!should_write("shipping", true, None, None), "no updatedAt");
        assert!(!should_write("shipping", true, Some(""), None));
    }
}
