//! Wave dispatch (AD-14, AD-16): an epic-plan run that reached `done` leaves `waves.json`; each wave is offered
//! once every dependency wave's run has all its PRs merged, and starts as a queue entry through the shared
//! start path (so the cap applies). An invalid `waves.json` starts nothing.

use super::dispatch::{self, enqueue, Dispatch, Epic, WaveRef, WaveRun};
use super::StartParams;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

#[derive(Debug, Clone, PartialEq)]
pub struct WaveSpec {
    pub n: u32,
    pub title: String,
    pub tickets: Vec<String>,
    pub depends_on: Vec<u32>,
    pub why: String,
}

/// `waves.json` checked again before anything is offered: ticket ids not in tickets.json are dropped, every
/// wave must still hold a ticket, and a dependency must name an earlier wave.
pub fn validate_waves(
    waves: &Value,
    ticket_ids: &HashSet<String>,
) -> Result<Vec<WaveSpec>, String> {
    let list = waves["waves"].as_array().ok_or("waves.json has no waves")?;
    let mut out: Vec<WaveSpec> = Vec::new();
    for (at, w) in list.iter().enumerate() {
        let n = w["n"].as_u64().ok_or("a wave has no number")? as u32;
        if n as usize != at + 1 {
            return Err(format!("wave {n} is out of order"));
        }
        let tickets: Vec<String> = w["tickets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str().map(String::from))
                    .filter(|t| ticket_ids.contains(t))
                    .collect()
            })
            .unwrap_or_default();
        if tickets.is_empty() {
            return Err(format!("wave {n} has no ticket that is in tickets.json"));
        }
        let depends_on: Vec<u32> = w["dependsOn"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|d| d.as_u64().map(|d| d as u32))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(bad) = depends_on.iter().find(|d| **d >= n || **d == 0) {
            return Err(format!(
                "wave {n} depends on wave {bad}: dependencies name earlier waves only"
            ));
        }
        out.push(WaveSpec {
            n,
            title: w["title"].as_str().unwrap_or("").to_string(),
            tickets,
            depends_on,
            why: w["why"].as_str().unwrap_or("").to_string(),
        });
    }
    if out.is_empty() {
        return Err("waves.json has no waves".into());
    }
    Ok(out)
}

/// Where a dependency run stands on GitHub.
#[derive(Debug, Clone, PartialEq)]
pub enum Merge {
    /// Every PR is merged (a finished run with no PR counts as merged).
    Merged,
    /// A PR is open or was closed without merging.
    Waiting(String),
    /// A PR's merge state was never read.
    NotRefreshed(String),
}

pub fn merge_state(run_dir: &Path) -> Merge {
    let status: Option<Value> = std::fs::read_to_string(run_dir.join("status.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let Some(status) = status else {
        return Merge::Waiting("the run has no status yet".into());
    };
    let no_prs = status["prs"].as_array().is_none_or(|a| a.is_empty());
    if no_prs {
        // A run that finished without any PR has nothing to merge; one still going has not opened its PRs yet.
        return if status["stage"].as_str() == Some("done") {
            Merge::Merged
        } else {
            Merge::Waiting("the run has no PRs yet".into())
        };
    }
    let gh = super::github::read_github(run_dir);
    let mut stale: Option<String> = None;
    let mut waiting: Option<String> = None;
    for pr in status["prs"].as_array().into_iter().flatten() {
        let id = pr["id"].as_str().unwrap_or("?");
        match gh.prs.get(id).map(|g| g.pr_state.as_str()) {
            Some("MERGED") => {}
            Some("CLOSED") => return Merge::Waiting(format!("{id} was closed without merging")),
            Some("OPEN") => waiting = Some(format!("{id} is open")),
            _ => stale = Some(format!("merge state not refreshed for {id}")),
        }
    }
    match (waiting, stale) {
        (Some(w), _) => Merge::Waiting(w),
        (None, Some(s)) => Merge::NotRefreshed(s),
        _ => Merge::Merged,
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WaveCard {
    pub n: u32,
    pub title: String,
    pub ticket_ids: Vec<String>,
    /// `done`, `building`, `ready` or `waiting`.
    pub state: String,
    pub reason: Option<String>,
}

/// The cards of an epic: a wave with a run is `building` (or `done` when its PRs are all merged); one without is
/// `ready` when every dependency wave has a run whose PRs are all merged, else `waiting` with the reason.
pub fn cards(
    specs: &[WaveSpec],
    wave_runs: &[WaveRun],
    merge: &dyn Fn(&Path) -> Merge,
) -> Vec<WaveCard> {
    let run_of = |n: u32| wave_runs.iter().find(|r| r.n == n);
    specs
        .iter()
        .map(|w| {
            let (state, reason) = match run_of(w.n) {
                Some(run) => match run.run_dir.as_deref() {
                    Some(dir)
                        if merge(Path::new(dir)) == Merge::Merged
                            && super::epics::run_is_done(Path::new(dir)) =>
                    {
                        ("done".to_string(), None)
                    }
                    Some(_) => ("building".to_string(), None),
                    None => ("building".to_string(), Some("queued".to_string())),
                },
                None => {
                    let mut why: Option<String> = None;
                    for dep in &w.depends_on {
                        let blocked = match run_of(*dep).and_then(|r| r.run_dir.as_deref()) {
                            None => Some(format!("wave {dep} has not started")),
                            Some(dir) => match merge(Path::new(dir)) {
                                Merge::Merged => None,
                                Merge::Waiting(m) => Some(format!("wave {dep}: {m}")),
                                Merge::NotRefreshed(m) => Some(format!("wave {dep}: {m}")),
                            },
                        };
                        if blocked.is_some() {
                            why = blocked;
                            break;
                        }
                    }
                    match why {
                        None => ("ready".to_string(), None),
                        Some(r) => ("waiting".to_string(), Some(r)),
                    }
                }
            };
            WaveCard {
                n: w.n,
                title: w.title.clone(),
                ticket_ids: w.tickets.clone(),
                state,
                reason,
            }
        })
        .collect()
}

/// A wave's run is finished (its stage is done), so a merged-only check is not read as done for a run still working.
pub fn run_is_done(run_dir: &Path) -> bool {
    std::fs::read_to_string(run_dir.join("status.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .is_some_and(|v| matches!(v["stage"].as_str(), Some("done")))
}

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

/// The epic-plan runs that reached `done`, newest first.
pub fn epic_runs(data: &Path) -> Vec<PathBuf> {
    super::folder::list_runs(data)
        .into_iter()
        .filter(|r| !r.adopted && r.stage.as_deref() == Some("done"))
        .map(|r| PathBuf::from(r.run_dir))
        .filter(|d| read_json(&d.join("meta.json")).is_some_and(|m| m["mode"] == "epic-plan"))
        .collect()
}

/// The ticket ids of the epic run's tickets.json.
fn ticket_ids(run_dir: &Path) -> HashSet<String> {
    read_json(&run_dir.join("tickets.json"))
        .and_then(|v| v.get("tasks").cloned().or(Some(v)))
        .and_then(|t| t.as_array().cloned())
        .map(|a| {
            a.iter()
                .filter_map(|t| t["id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// The specs of an epic run, or why they cannot be offered.
pub fn specs_of(run_dir: &Path) -> Result<Vec<WaveSpec>, String> {
    let waves = read_json(&run_dir.join("waves.json")).ok_or("waves.json is missing")?;
    validate_waves(&waves, &ticket_ids(run_dir))
}

/// The run folders of waves already started by the epics that still have a wave to do.
pub fn dependency_runs(data: &Path) -> HashSet<String> {
    let state = dispatch::load(data);
    let mut out = HashSet::new();
    for epic in &state.epics {
        let Ok(specs) = specs_of(Path::new(&epic.run_dir)) else {
            continue;
        };
        let open = specs
            .iter()
            .any(|w| !epic.wave_runs.iter().any(|r| r.n == w.n));
        if open {
            out.extend(epic.wave_runs.iter().filter_map(|r| r.run_dir.clone()));
        }
    }
    out
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EpicView {
    pub run_dir: String,
    pub run_id: String,
    /// "waves file invalid: ..." when `waves.json` cannot be offered; nothing starts then.
    pub problem: Option<String>,
    pub cards: Vec<WaveCard>,
}

fn views(data: &Path, state: &Dispatch) -> Vec<EpicView> {
    epic_runs(data)
        .into_iter()
        .map(|dir| {
            let run_dir = dir.display().to_string();
            let run_id = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let wave_runs = state
                .epics
                .iter()
                .find(|e| e.run_dir == run_dir)
                .map(|e| e.wave_runs.clone())
                .unwrap_or_default();
            match specs_of(&dir) {
                Ok(specs) => EpicView {
                    run_dir,
                    run_id,
                    problem: None,
                    cards: cards(&specs, &wave_runs, &merge_state),
                },
                Err(why) => EpicView {
                    run_dir,
                    run_id,
                    problem: Some(format!("waves file invalid: {why}")),
                    cards: vec![],
                },
            }
        })
        .collect()
}

/// A wave becomes a queue entry for the epic run's account: its tickets (as task refs), the epic's folder, review
/// and BMAD mode. The epic's `started` and `wave_runs` remember it so it is never queued twice.
pub fn queue_wave(data: &Path, epic_dir: &Path, n: u32) -> Result<String, String> {
    let specs = specs_of(epic_dir).map_err(|e| format!("waves file invalid: {e}"))?;
    let spec = specs
        .iter()
        .find(|w| w.n == n)
        .ok_or_else(|| format!("no wave {n}"))?;
    let meta = read_json(&epic_dir.join("meta.json")).ok_or("the epic run has no meta.json")?;
    let account = meta["account"]["id"]
        .as_str()
        .ok_or("the epic run records no account")?
        .to_string();
    let folder = meta["folder"]
        .as_str()
        .ok_or("the epic run records no folder")?
        .to_string();
    let epic_key = epic_dir.display().to_string();
    let epic_id = epic_dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let refs = spec
        .tickets
        .iter()
        .map(|t| crate::clickup::intake::IntakeRef {
            kind: "task".into(),
            id: t.clone(),
            team_id: None,
            custom: false,
            line: 0,
        })
        .collect();
    let params = StartParams {
        window_id: None,
        folder,
        request: format!("Wave {n} of epic {epic_id}: {}\n\n{}", spec.title, spec.why),
        role: "eagle-eye".into(),
        clickup: Some(super::ClickUpStart {
            refs,
            ticket_ids: spec.tickets.clone(),
        }),
        review_mode: meta["reviewMode"].as_str().map(String::from),
        bmad_mode: meta["bmad"]["mode"].as_str().map(String::from),
        force: None,
        repos: None,
        max_implementers: None,
        run_mode: None,
        dispatch_entry_id: None,
    };
    let label = format!("Wave {n}: {}", spec.title);
    dispatch::with(data, |d| {
        let epic = match d.epics.iter().position(|e| e.run_dir == epic_key) {
            Some(at) => &mut d.epics[at],
            None => {
                d.epics.push(Epic {
                    run_dir: epic_key.clone(),
                    ..Default::default()
                });
                d.epics.last_mut().expect("just pushed")
            }
        };
        if epic.wave_runs.iter().any(|r| r.n == n) {
            return Err(format!("wave {n} was already started"));
        }
        let mut scratch = Dispatch::default();
        let id = enqueue(&mut scratch, params, &account, &label, None);
        let mut entry = scratch.queue.remove(0);
        entry.wave = Some(WaveRef {
            epic_run: epic_key.clone(),
            n,
        });
        epic.started.push(n);
        epic.wave_runs.push(WaveRun {
            n,
            entry_id: id.clone(),
            run_dir: None,
        });
        d.queue.push(entry);
        Ok(id)
    })?
}

fn data_dir(app: &AppHandle) -> PathBuf {
    crate::data_dir::app_data_dir(app).path
}

/// With `run.autoStartWaves`, every ready wave is queued (through the cap and the shared start path).
pub fn auto_start(app: &AppHandle) {
    use tauri::Manager;
    let on = app
        .try_state::<std::sync::Mutex<crate::store::SettingsStore>>()
        .and_then(|s| s.lock().ok().map(|s| s.run().auto_start_waves))
        .unwrap_or(false);
    if !on {
        return;
    }
    let data = data_dir(app);
    let state = dispatch::load(&data);
    for view in views(&data, &state) {
        if view.problem.is_some() {
            continue;
        }
        for card in view.cards.iter().filter(|c| c.state == "ready") {
            let _ = queue_wave(&data, Path::new(&view.run_dir), card.n);
        }
    }
}

/// Each newly ready wave raises one `wave-ready` event (its subject is the epic run id), once (remembered).
pub fn wave_ready_events(app: &AppHandle) -> Vec<crate::observe::attention::Event> {
    use crate::observe::attention::{Event, Kind};
    let data = data_dir(app);
    let state = dispatch::load(&data);
    if state.epics.is_empty() && epic_runs(&data).is_empty() {
        return vec![];
    }
    let mut fired: Vec<String> = Vec::new();
    for view in views(&data, &state) {
        if view.problem.is_some() {
            continue;
        }
        let ready: Vec<u32> = view
            .cards
            .iter()
            .filter(|c| c.state == "ready")
            .map(|c| c.n)
            .collect();
        let new: Vec<u32> = dispatch::with(&data, |d| {
            let epic = match d.epics.iter().position(|e| e.run_dir == view.run_dir) {
                Some(at) => &mut d.epics[at],
                None => {
                    d.epics.push(Epic {
                        run_dir: view.run_dir.clone(),
                        ..Default::default()
                    });
                    d.epics.last_mut().expect("just pushed")
                }
            };
            let fresh: Vec<u32> = ready
                .iter()
                .copied()
                .filter(|n| !epic.notified.contains(&format!("wave-{n}")))
                .collect();
            epic.notified
                .extend(fresh.iter().map(|n| format!("wave-{n}")));
            fresh
        })
        .unwrap_or_default();
        fired.extend(new.iter().map(|_| view.run_id.clone()));
    }
    fired
        .into_iter()
        .map(|id| Event {
            kind: Kind::WaveReady,
            subject: id.clone(),
            tab_id: None,
            window_label: None,
            tab_label: None,
            run_id: Some(id),
            run_dir: None,
            detail: None,
        })
        .collect()
}

#[tauri::command]
pub async fn epics_overview(app: AppHandle) -> Result<Vec<EpicView>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let data = data_dir(&app);
        views(&data, &dispatch::load(&data))
    })
    .await
    .map_err(|e| e.to_string())
}

/// "Start wave": the wave becomes a queue entry (the cap applies); refused when it is not ready.
#[tauri::command]
pub async fn epic_start_wave(app: AppHandle, run_dir: String, n: u32) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let data = data_dir(&app);
        let dir = super::checked_run_dir(&data, &run_dir)?;
        let view = views(&data, &dispatch::load(&data))
            .into_iter()
            .find(|v| v.run_dir == dir.display().to_string())
            .ok_or("not an epic run that is done")?;
        if let Some(p) = &view.problem {
            return Err(p.clone());
        }
        match view.cards.iter().find(|c| c.n == n) {
            Some(c) if c.state == "ready" => queue_wave(&data, &dir, n),
            Some(c) => Err(format!(
                "wave {n} is {}{}",
                c.state,
                c.reason
                    .as_deref()
                    .map(|r| format!(": {r}"))
                    .unwrap_or_default()
            )),
            None => Err(format!("no wave {n}")),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ids(list: &[&str]) -> HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn waves() -> Value {
        json!({"schema": 1, "waves": [
            {"n": 1, "title": "Foundations", "tickets": ["t1", "t2", "t3"], "dependsOn": [], "why": "first"},
            {"n": 2, "title": "Features", "tickets": ["t4", "t5", "t6"], "dependsOn": [1], "why": "second"},
            {"n": 3, "title": "Polish", "tickets": ["t7", "t8", "t9"], "dependsOn": [1, 2], "why": "third"},
        ]})
    }

    fn all() -> HashSet<String> {
        ids(&["t1", "t2", "t3", "t4", "t5", "t6", "t7", "t8", "t9"])
    }

    fn specs() -> Vec<WaveSpec> {
        validate_waves(&waves(), &all()).unwrap()
    }

    #[test]
    fn ticket_ids_not_in_tickets_json_are_dropped_and_an_empty_or_forward_wave_is_refused() {
        let mut w = waves();
        w["waves"][0]["tickets"] = json!(["t1", "ghost"]);
        let specs = validate_waves(&w, &all()).unwrap();
        assert_eq!(specs[0].tickets, ["t1"], "an unknown ticket id is dropped");
        let mut empty = waves();
        empty["waves"][0]["tickets"] = json!(["ghost"]);
        assert!(validate_waves(&empty, &all())
            .unwrap_err()
            .contains("wave 1 has no ticket"));
        let mut forward = waves();
        forward["waves"][0]["dependsOn"] = json!([2]);
        assert!(validate_waves(&forward, &all())
            .unwrap_err()
            .contains("earlier waves only"));
        assert!(validate_waves(&json!({"waves": []}), &all()).is_err());
        assert!(validate_waves(&json!({}), &all()).is_err());
    }

    fn merge_of(table: Vec<(&'static str, Merge)>) -> impl Fn(&Path) -> Merge {
        move |p: &Path| {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            table
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, m)| m.clone())
                .unwrap_or(Merge::Merged)
        }
    }

    fn started(n: u32, dir: &str) -> WaveRun {
        WaveRun {
            n,
            entry_id: format!("q-{n}"),
            run_dir: Some(format!("/runs/{dir}")),
        }
    }

    #[test]
    fn a_dependent_wave_is_not_ready_while_any_dependency_pr_is_open_or_closed_and_is_ready_when_all_are_merged(
    ) {
        let state_of = |m: Merge| {
            let c = cards(
                &specs()[..2],
                &[started(1, "w1")],
                &merge_of(vec![("w1", m)]),
            );
            c[1].clone()
        };
        let open = state_of(Merge::Waiting("PR1 is open".into()));
        assert_eq!(
            (open.state.as_str(), open.reason.as_deref()),
            ("waiting", Some("wave 1: PR1 is open"))
        );
        let closed = state_of(Merge::Waiting("PR1 was closed without merging".into()));
        assert_eq!(closed.state, "waiting");
        assert!(closed.reason.unwrap().contains("closed without merging"));
        let stale = state_of(Merge::NotRefreshed(
            "merge state not refreshed for PR1".into(),
        ));
        assert_eq!(stale.state, "waiting");
        assert!(
            stale.reason.unwrap().contains("merge state not refreshed"),
            "the card says so"
        );
        assert_eq!(state_of(Merge::Merged).state, "ready");
        // Wave 3 needs both 1 and 2; wave 2 has no run yet.
        let c = cards(&specs(), &[started(1, "w1")], &merge_of(vec![]));
        assert_eq!(c[1].state, "ready");
        assert_eq!(c[2].state, "waiting");
        assert_eq!(c[2].reason.as_deref(), Some("wave 2 has not started"));
        // The first wave (no dependency) is ready at once; a wave with a run is building.
        let first = cards(&specs(), &[], &merge_of(vec![]));
        assert_eq!(first[0].state, "ready");
        let building = cards(
            &specs(),
            &[started(1, "w1")],
            &merge_of(vec![("w1", Merge::Waiting("PR1 is open".into()))]),
        );
        assert_eq!(building[0].state, "building");
    }

    #[test]
    fn merge_state_reads_the_prs_of_a_run_and_zero_prs_counts_as_merged() {
        let dir = std::env::temp_dir().join(format!("dcv2-epic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let write = |status: Value, gh: Value| {
            std::fs::write(dir.join("status.json"), status.to_string()).unwrap();
            std::fs::write(dir.join("app-github.json"), gh.to_string()).unwrap();
        };
        let pr = |state: &str| json!({"ci": "pass", "prState": state, "at": "t"});
        write(json!({"stage": "done", "prs": []}), json!({"prs": {}}));
        assert_eq!(merge_state(&dir), Merge::Merged);
        write(json!({"stage": "building", "prs": []}), json!({"prs": {}}));
        assert!(matches!(merge_state(&dir), Merge::Waiting(_)), "no PRs yet");
        std::fs::remove_file(dir.join("status.json")).unwrap();
        assert!(matches!(merge_state(&dir), Merge::Waiting(_)), "no status");
        write(
            json!({"prs": [{"id": "PR1"}, {"id": "PR2"}]}),
            json!({"prs": {"PR1": pr("MERGED"), "PR2": pr("MERGED")}}),
        );
        assert_eq!(merge_state(&dir), Merge::Merged);
        write(
            json!({"prs": [{"id": "PR1"}, {"id": "PR2"}]}),
            json!({"prs": {"PR1": pr("MERGED"), "PR2": pr("OPEN")}}),
        );
        assert_eq!(merge_state(&dir), Merge::Waiting("PR2 is open".into()));
        write(
            json!({"prs": [{"id": "PR1"}]}),
            json!({"prs": {"PR1": pr("CLOSED")}}),
        );
        assert!(matches!(merge_state(&dir), Merge::Waiting(m) if m.contains("closed")));
        write(
            json!({"prs": [{"id": "PR1"}]}),
            json!({"prs": {"PR1": pr("")}}),
        );
        assert!(matches!(merge_state(&dir), Merge::NotRefreshed(_)));
        write(json!({"prs": [{"id": "PR1"}]}), json!({"prs": {}}));
        assert!(
            matches!(merge_state(&dir), Merge::NotRefreshed(_)),
            "never read"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn queueing_a_wave_stores_its_tickets_and_account_once_and_the_epic_state_survives_a_reload() {
        let data = std::env::temp_dir().join(format!("dcv2-epic-q-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let epic = data.join("runs").join("r-epic");
        std::fs::create_dir_all(&epic).unwrap();
        std::fs::write(epic.join("waves.json"), waves().to_string()).unwrap();
        std::fs::write(
            epic.join("tickets.json"),
            json!({"tasks": all().iter().map(|i| json!({"id": i})).collect::<Vec<_>>()})
                .to_string(),
        )
        .unwrap();
        std::fs::write(
            epic.join("meta.json"),
            json!({"mode": "epic-plan", "folder": "/w/app", "reviewMode": "github", "bmad": {"mode": "auto"}, "account": {"id": "acct-3"}}).to_string(),
        )
        .unwrap();
        let id = queue_wave(&data, &epic, 1).unwrap();
        let state = dispatch::load(&data);
        let entry = &state.queue[0];
        assert_eq!(entry.id, id);
        assert_eq!(entry.account_id, "acct-3");
        assert_eq!(entry.params.folder, "/w/app");
        assert_eq!(entry.params.review_mode.as_deref(), Some("github"));
        let c = entry.params.clickup.as_ref().unwrap();
        assert_eq!(c.ticket_ids, ["t1", "t2", "t3"]);
        assert_eq!(c.refs.len(), 3);
        assert_eq!(entry.wave.as_ref().unwrap().n, 1);
        assert_eq!(state.epics[0].started, [1]);
        assert!(queue_wave(&data, &epic, 1).is_err(), "never queued twice");
        // A start records the run folder in the epic.
        let mut state = dispatch::load(&data);
        dispatch::finish(&mut state, &id, Ok(Some("/runs/w1".into())));
        assert_eq!(
            state.epics[0].wave_runs[0].run_dir.as_deref(),
            Some("/runs/w1")
        );
        dispatch::save(&data, &state).unwrap();
        assert_eq!(
            dispatch::load(&data).epics[0].wave_runs[0]
                .run_dir
                .as_deref(),
            Some("/runs/w1"),
            "survives a restart"
        );
        // An invalid waves.json starts nothing.
        std::fs::write(
            epic.join("waves.json"),
            json!({"waves": [{"n": 1, "title": "x", "tickets": ["t1"], "dependsOn": [1]}]})
                .to_string(),
        )
        .unwrap();
        assert!(queue_wave(&data, &epic, 1)
            .unwrap_err()
            .contains("waves file invalid"));
        let _ = std::fs::remove_dir_all(&data);
    }
}
