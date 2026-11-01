//! The attention detector: one backend thread that decides every
//! notification, in-app notice, badge and reminder (docs/launch.md,
//! "Attention").
//!
//! - Fast pass, every 2 s: the open tabs of all windows, their hook states
//!   and trackers (through the same `observe_tab_now` the run panel uses),
//!   and for each run tab the run folder's status.json, app-checks.json,
//!   app-cleanup.json and the signed auto-post record.
//! - Background pass, every 60 s: the app's own runs changed within 30
//!   days that have no open run tab (status.json, app-cleanup.json and the
//!   auto-post record only), so a merged-cleaned or a late auto-post result
//!   of a run whose tab is closed is still seen.
//! - [`Detector::diff`] is the pure part: edge-triggered events keyed by
//!   (subject, kind), a 30 s debounce, one waiting reminder per episode, a
//!   silent first tick, and a run that first appears is seeded silently.
//! - [`Landing`] is the pure part of "a notification click lands in the
//!   tab": desktop notification actions are ignored, so the target waits
//!   for the next real app activation.
//!
//! The thread starts at app setup and never from a command: it works with
//! no window polling. No store lock is held across a file read.

use super::hooks::{TabActivity, TabHookState};
use super::watchdog::{self, WatchInput};
use crate::clickup::{autopost, trust};
use crate::run::{cleanup, folder, github, record, status};
use crate::store::{StateStore, TabKind, TabRecord};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter, Manager};

/// How often the open tabs are looked at.
pub const FAST_TICK: Duration = Duration::from_secs(2);
/// The background pass runs every this many fast ticks (60 s).
pub const BACKGROUND_EVERY: u64 = 30;
/// The same (subject, kind) is not announced twice within this.
pub const DEBOUNCE_MS: u64 = 30_000;
/// A notified tab is landed in at the next app activation within this.
pub const LANDING_TTL_MS: u64 = 120_000;
/// An app activation is a window gaining focus after no window of the app
/// had it for at least this long. A switch between two of the app's windows
/// is faster.
pub const ACTIVATION_GAP_MS: u64 = 250;
/// The background set: runs changed within this many days.
pub const BACKGROUND_DAYS: i64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    NeedsYou,
    ApprovalWait,
    Stall,
    RunDone,
    RunFailed,
    CheckFailed,
    CiFailed,
    AutopostFailed,
    WaitingReminder,
    MergedCleaned,
    /// A batch: every run's plan is ready for approval (the subject is the batch id).
    BatchPlanned,
    /// A batch: every run is ready to ship.
    BatchReady,
    /// An epic: the next wave is ready to start (the subject is the epic run id).
    WaveReady,
}

impl Kind {
    /// The kind named `key` in settings.json.
    pub fn from_key(key: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.key() == key)
    }

    pub const ALL: [Kind; 13] = [
        Kind::NeedsYou,
        Kind::ApprovalWait,
        Kind::Stall,
        Kind::RunDone,
        Kind::RunFailed,
        Kind::CheckFailed,
        Kind::CiFailed,
        Kind::AutopostFailed,
        Kind::WaitingReminder,
        Kind::MergedCleaned,
        Kind::BatchPlanned,
        Kind::BatchReady,
        Kind::WaveReady,
    ];

    /// Needs-you and approval-wait are the ones that count in the badge and
    /// ring the dock harder.
    pub fn is_urgent(self) -> bool {
        matches!(self, Kind::NeedsYou | Kind::ApprovalWait)
    }

    /// The name in settings.json and in the `attention` event.
    pub fn key(self) -> &'static str {
        match self {
            Kind::NeedsYou => "needs-you",
            Kind::ApprovalWait => "approval-wait",
            Kind::Stall => "stall",
            Kind::RunDone => "run-done",
            Kind::RunFailed => "run-failed",
            Kind::CheckFailed => "check-failed",
            Kind::CiFailed => "ci-failed",
            Kind::AutopostFailed => "autopost-failed",
            Kind::WaitingReminder => "waiting-reminder",
            Kind::MergedCleaned => "merged-cleaned",
            Kind::BatchPlanned => "batch-planned",
            Kind::BatchReady => "batch-ready",
            Kind::WaveReady => "wave-ready",
        }
    }
}

/// One thing that needs a person's attention, once.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub kind: Kind,
    /// What it is about: a tab id (needs-you, stall, reminder) or a run id.
    pub subject: String,
    /// The tab to land in, when the subject has an open one.
    pub tab_id: Option<String>,
    pub window_label: Option<String>,
    pub tab_label: Option<String>,
    pub run_id: Option<String>,
    pub run_dir: Option<String>,
    /// A PR id, a ticket id, or the approval stop (`plan-approval`).
    pub detail: Option<String>,
}

impl Event {
    /// The words of the notification and the in-app notice, the same.
    pub fn text(&self) -> String {
        let run = self
            .run_id
            .as_deref()
            .map(|id| format!(" ({id})"))
            .unwrap_or_default();
        let tab = self.tab_label.as_deref().unwrap_or("a tab");
        let detail = self.detail.as_deref().unwrap_or("");
        match self.kind {
            Kind::NeedsYou => format!("dcv2: {tab} needs you"),
            Kind::ApprovalWait => match detail {
                "ship-approval" => format!("dcv2: ship approval needed{run}"),
                _ => format!("dcv2: plan ready for approval{run}"),
            },
            Kind::Stall => format!("dcv2: no progress in {tab}"),
            Kind::RunDone => format!("dcv2: run finished{run}"),
            Kind::RunFailed => format!("dcv2: run failed{run}"),
            Kind::CheckFailed => format!("dcv2: {detail} check failed{run}"),
            Kind::CiFailed => format!("dcv2: {detail} CI failed on GitHub{run}"),
            Kind::AutopostFailed => format!("dcv2: ClickUp auto-post failed for {detail}{run}"),
            Kind::WaitingReminder => format!("dcv2: {tab} is still waiting for you"),
            Kind::MergedCleaned => format!("dcv2: {detail} merged and cleaned up{run}"),
            Kind::BatchPlanned => {
                format!("dcv2: every plan of the batch is ready for approval{run}")
            }
            Kind::BatchReady => format!("dcv2: every run of the batch is ready to ship{run}"),
            Kind::WaveReady => format!("dcv2: the next wave is ready to start{run}"),
        }
    }
}

/// What the detector sees of one open tab at one tick.
#[derive(Debug, Clone, PartialEq)]
pub struct TabFacts {
    pub tab_id: String,
    pub window_label: String,
    pub label: String,
    pub is_claude: bool,
    pub run_id: Option<String>,
    pub run_dir: Option<String>,
    pub activity: TabActivity,
    pub stalled: bool,
    /// The wait is past the watchdog minutes: when it began (the episode).
    pub waiting_since_ms: Option<u64>,
    /// A tool runs (hook state); part of the `tab-state` push.
    pub tool_running: bool,
    /// Sub-agents running (hook state); part of the `tab-state` push.
    pub subagents_running: u32,
}

/// What the detector sees of one run at one tick.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunFacts {
    pub run_id: String,
    pub run_dir: String,
    pub stage: Option<String>,
    /// status.json `waitingFor` (`plan-approval`, `ship-approval`).
    pub waiting_for: Option<String>,
    /// status.json `updatedAt` while `waiting_for` is set: when the wait was
    /// written (approximate: a later status write during the wait moves it).
    pub waiting_since: Option<String>,
    /// meta.json's `mode` (`epic-plan`); `None` is a normal run.
    pub run_mode: Option<String>,
    pub prs_done: u32,
    pub prs_total: u32,
    /// PRs whose app check failed, timed out or errored.
    pub failed_checks: BTreeSet<String>,
    /// PR id → outcome (`cleaned` / `kept`) of final cleanup records.
    pub cleaned: BTreeMap<String, String>,
    /// Tickets whose signed auto-post result is `failed`.
    pub autopost_failed: BTreeSet<String>,
    /// PRs whose GitHub CI (`app-github.json`) is `fail`.
    pub ci_failed: BTreeSet<String>,
    /// Per PR: its state, app check result, GitHub CI and PR state, for the
    /// `run-state` push.
    pub prs: Vec<PrFact>,
}

/// One PR of a run as the `run-state` push tells it.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrFact {
    pub id: String,
    /// status.json's PR state (`building`, `reviewed`, `opened`, …).
    pub state: Option<String>,
    /// The app's own check result.
    pub app_check: Option<String>,
    /// `pass`, `fail`, `pending`, `none`, `unknown`; None until read.
    pub ci: Option<String>,
    /// `OPEN`, `MERGED`, `CLOSED`; None until read.
    pub pr_state: Option<String>,
}

/// One tick's reading.
#[derive(Debug, Clone, Default)]
pub struct Observed {
    pub tabs: Vec<TabFacts>,
    pub runs: Vec<RunFacts>,
    /// This tick read every run there is (the background pass): runs not in
    /// `runs` are forgotten.
    pub complete_runs: bool,
}

/// The memory between ticks. Pure: no files, no clock of its own.
#[derive(Debug, Default)]
pub struct Detector {
    started: bool,
    tabs: HashMap<String, TabFacts>,
    runs: HashMap<String, RunFacts>,
    fired: HashMap<(String, Kind), u64>,
    reminded: HashSet<(String, u64)>,
}

impl Detector {
    /// The events of this tick. `now_ms` is Unix milliseconds.
    pub fn diff(&mut self, now: Observed, now_ms: u64) -> Vec<Event> {
        let silent = !self.started;
        self.started = true;
        let mut out = Vec::new();

        let tab_of_run = |run_id: &str| -> Option<&TabFacts> {
            now.tabs
                .iter()
                .find(|t| t.run_id.as_deref() == Some(run_id))
        };

        for run in &now.runs {
            // A run first seen (app start, or it just entered the set) is
            // seeded: its next change fires, not its current state.
            if let Some(prev) = self.runs.get(&run.run_id) {
                let tab = tab_of_run(&run.run_id);
                let mut push = |kind: Kind, detail: Option<String>| {
                    out.push(Event {
                        kind,
                        subject: run.run_id.clone(),
                        tab_id: tab.map(|t| t.tab_id.clone()),
                        window_label: tab.map(|t| t.window_label.clone()),
                        tab_label: tab.map(|t| t.label.clone()),
                        run_id: Some(run.run_id.clone()),
                        run_dir: Some(run.run_dir.clone()),
                        detail,
                    });
                };
                if prev.waiting_for.is_none() && run.waiting_for.is_some() {
                    push(Kind::ApprovalWait, run.waiting_for.clone());
                }
                let was = prev.stage.as_deref();
                if was != Some("done") && run.stage.as_deref() == Some("done") {
                    push(Kind::RunDone, None);
                }
                if was != Some("failed") && run.stage.as_deref() == Some("failed") {
                    push(Kind::RunFailed, None);
                }
                for pr in run.failed_checks.difference(&prev.failed_checks) {
                    push(Kind::CheckFailed, Some(pr.clone()));
                }
                for pr in run.ci_failed.difference(&prev.ci_failed) {
                    push(Kind::CiFailed, Some(pr.clone()));
                }
                for ticket in run.autopost_failed.difference(&prev.autopost_failed) {
                    push(Kind::AutopostFailed, Some(ticket.clone()));
                }
                for pr in run.cleaned.keys() {
                    if !prev.cleaned.contains_key(pr) {
                        push(Kind::MergedCleaned, Some(pr.clone()));
                    }
                }
            }
            self.runs.insert(run.run_id.clone(), run.clone());
        }
        if now.complete_runs {
            let seen: HashSet<&str> = now.runs.iter().map(|r| r.run_id.as_str()).collect();
            self.runs.retain(|id, _| seen.contains(id.as_str()));
        }

        for tab in &now.tabs {
            let prev_activity = self
                .tabs
                .get(&tab.tab_id)
                .map_or(TabActivity::Unknown, |p| p.activity);
            let prev_stalled = self.tabs.get(&tab.tab_id).is_some_and(|p| p.stalled);
            let event = |kind: Kind| Event {
                kind,
                subject: tab.tab_id.clone(),
                tab_id: Some(tab.tab_id.clone()),
                window_label: Some(tab.window_label.clone()),
                tab_label: Some(tab.label.clone()),
                run_id: tab.run_id.clone(),
                run_dir: tab.run_dir.clone(),
                detail: None,
            };
            if prev_activity != TabActivity::NeedsYou && tab.activity == TabActivity::NeedsYou {
                out.push(event(Kind::NeedsYou));
            }
            if !prev_stalled && tab.stalled {
                out.push(event(Kind::Stall));
            }
            if let Some(episode) = tab.waiting_since_ms {
                if self.reminded.insert((tab.tab_id.clone(), episode)) {
                    out.push(event(Kind::WaitingReminder));
                }
            }
        }
        let open: HashMap<&str, &TabFacts> =
            now.tabs.iter().map(|t| (t.tab_id.as_str(), t)).collect();
        self.reminded.retain(|(tab, episode)| {
            open.get(tab.as_str())
                .is_some_and(|t| t.waiting_since_ms == Some(*episode))
        });
        self.tabs = now
            .tabs
            .iter()
            .map(|t| (t.tab_id.clone(), t.clone()))
            .collect();

        if silent {
            return Vec::new();
        }
        // The debounce: the same (subject, kind) is not announced twice
        // within 30 s (a flapping state), whatever the edge.
        self.fired
            .retain(|_, at| now_ms.saturating_sub(*at) < DEBOUNCE_MS);
        out.retain(|event| {
            let key = (
                format!(
                    "{}\u{0}{}",
                    event.subject,
                    event.detail.as_deref().unwrap_or("")
                ),
                event.kind,
            );
            if self.fired.contains_key(&key) {
                return false;
            }
            self.fired.insert(key, now_ms);
            true
        });
        out
    }
}

// ---- push events (tab-state, run-state, stall) ---------------------------

/// What the page is told when a tab's hook state changes.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabStatePush {
    pub tab_id: String,
    pub activity: TabActivity,
    pub tool_running: bool,
    pub subagents_running: u32,
}

/// What the page is told when a tab's stall flag flips.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StallPush {
    pub tab_id: String,
    pub run_id: Option<String>,
    pub stalled: bool,
}

/// What the page is told when a run's state changes: ids and state fields
/// only (no ticket or log text, no paths but the run folder).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStatePush {
    pub run_id: String,
    pub run_dir: String,
    pub stage: Option<String>,
    pub waiting_for: Option<String>,
    pub prs_done: u32,
    pub prs_total: u32,
    /// PR ids whose app check failed, timed out or errored.
    pub failed_checks: Vec<String>,
    /// Each PR's state, app check, CI and GitHub state.
    pub prs: Vec<PrFact>,
}

impl RunStatePush {
    fn of(run: &RunFacts) -> Self {
        RunStatePush {
            run_id: run.run_id.clone(),
            run_dir: run.run_dir.clone(),
            stage: run.stage.clone(),
            waiting_for: run.waiting_for.clone(),
            prs_done: run.prs_done,
            prs_total: run.prs_total,
            failed_checks: run.failed_checks.iter().cloned().collect(),
            prs: run.prs.clone(),
        }
    }
}

/// One event for the page.
#[derive(Debug, Clone, PartialEq)]
pub enum Push {
    TabState(TabStatePush),
    RunState(RunStatePush),
    Stall(StallPush),
}

/// What the last tick told the page about one tab.
#[derive(Debug, Clone, PartialEq)]
struct TabPushed {
    state: (TabActivity, bool, u32),
    stalled: bool,
}

/// Which events a tick's reading makes: only changes since the last one.
/// Pure like [`Detector`]: no files, no clock. The first tick is silent
/// (the page reads once on its own). A tab closed is forgotten at once
/// (every tick lists every open tab). A run missing from a tick that is not
/// `complete_runs` (a fast tick skips runs with no open tab) keeps its
/// memory and emits nothing; it is forgotten only on a `complete_runs` tick
/// that does not list it, exactly like [`Detector::diff`].
#[derive(Debug, Default)]
pub struct PushDiff {
    started: bool,
    tabs: HashMap<String, TabPushed>,
    runs: HashMap<String, RunStatePush>,
}

impl PushDiff {
    pub fn diff(&mut self, now: &Observed) -> Vec<Push> {
        let silent = !self.started;
        self.started = true;
        let mut out = Vec::new();

        let mut tabs = HashMap::new();
        for tab in &now.tabs {
            let pushed = TabPushed {
                state: (tab.activity, tab.tool_running, tab.subagents_running),
                stalled: tab.stalled,
            };
            let prev = self.tabs.get(&tab.tab_id);
            if !silent {
                if prev.map(|p| p.state) != Some(pushed.state) {
                    out.push(Push::TabState(TabStatePush {
                        tab_id: tab.tab_id.clone(),
                        activity: tab.activity,
                        tool_running: tab.tool_running,
                        subagents_running: tab.subagents_running,
                    }));
                }
                if prev.is_some_and(|p| p.stalled != pushed.stalled)
                    || (prev.is_none() && pushed.stalled)
                {
                    out.push(Push::Stall(StallPush {
                        tab_id: tab.tab_id.clone(),
                        run_id: tab.run_id.clone(),
                        stalled: tab.stalled,
                    }));
                }
            }
            tabs.insert(tab.tab_id.clone(), pushed);
        }
        self.tabs = tabs;

        for run in &now.runs {
            let pushed = RunStatePush::of(run);
            // A run first seen after the first tick is a new run for the page.
            if !silent && self.runs.get(&run.run_id) != Some(&pushed) {
                out.push(Push::RunState(pushed.clone()));
            }
            self.runs.insert(run.run_id.clone(), pushed);
        }
        if now.complete_runs {
            let seen: HashSet<&str> = now.runs.iter().map(|r| r.run_id.as_str()).collect();
            self.runs.retain(|id, _| seen.contains(id.as_str()));
        }
        out
    }
}

// ---- the snapshot (S6 badge, S12 record, S16 overview) -------------------

/// One open tab as the overview shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabSnap {
    pub window_label: String,
    pub tab_id: String,
    pub label: String,
    pub activity: TabActivity,
    pub run_id: Option<String>,
    pub run_dir: Option<String>,
    pub stage: Option<String>,
    pub waiting_for: Option<String>,
    /// When the current approval wait was written; `None` unless waiting.
    pub waiting_since: Option<String>,
    /// The run's mode (`epic-plan`); `None` is a normal run.
    pub run_mode: Option<String>,
    pub stalled: bool,
    pub prs_done: u32,
    pub prs_total: u32,
    /// The run's PRs with their states, for the dashboard.
    pub prs: Vec<PrFact>,
}

impl TabSnap {
    /// The tab needs the person: Claude asks (needs-you) or a run waits at
    /// an approval stop.
    pub fn needs_you(&self) -> bool {
        self.activity == TabActivity::NeedsYou || self.waiting_for.is_some()
    }
}

/// The latest reading of every open tab.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub tabs: Vec<TabSnap>,
}

/// The status bar's attention control.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// Tabs that need the person, in every window.
    pub needs_you: usize,
    /// Run tabs whose run has not finished.
    pub active_runs: usize,
}

impl Snapshot {
    pub fn build(tabs: &[TabFacts], runs: &HashMap<String, RunFacts>) -> Self {
        Snapshot {
            tabs: tabs
                .iter()
                .map(|t| {
                    let run = t.run_id.as_deref().and_then(|id| runs.get(id));
                    TabSnap {
                        window_label: t.window_label.clone(),
                        tab_id: t.tab_id.clone(),
                        label: t.label.clone(),
                        activity: t.activity,
                        run_id: t.run_id.clone(),
                        run_dir: t.run_dir.clone(),
                        stage: run.and_then(|r| r.stage.clone()),
                        waiting_for: run.and_then(|r| r.waiting_for.clone()),
                        waiting_since: run.and_then(|r| r.waiting_since.clone()),
                        run_mode: run.and_then(|r| r.run_mode.clone()),
                        stalled: t.stalled,
                        prs_done: run.map_or(0, |r| r.prs_done),
                        prs_total: run.map_or(0, |r| r.prs_total),
                        prs: run.map(|r| r.prs.clone()).unwrap_or_default(),
                    }
                })
                .collect(),
        }
    }
}

impl Snapshot {
    /// Every run tab, finished or not, windows in tab order.
    pub fn run_tabs(&self) -> Vec<TabSnap> {
        self.tabs
            .iter()
            .filter(|t| t.run_id.is_some())
            .cloned()
            .collect()
    }

    /// Run tabs whose run has not finished, windows in tab order.
    pub fn active_runs(&self) -> Vec<TabSnap> {
        self.run_tabs()
            .into_iter()
            .filter(|t| !matches!(t.stage.as_deref(), Some("done") | Some("failed")))
            .collect()
    }

    pub fn summary(&self) -> Summary {
        Summary {
            needs_you: self.tabs.iter().filter(|t| t.needs_you()).count(),
            active_runs: self.active_runs().len(),
        }
    }
}

// ---- landing in the notified tab ------------------------------------------

/// "A click on the notification lands in the tab". The desktop plugin
/// ignores notification actions, so a target is kept when an event was
/// notified while the app was in the background, and the next real app
/// activation (a window gaining focus after no window had it for
/// [`ACTIVATION_GAP_MS`]) within [`LANDING_TTL_MS`] lands in it.
#[derive(Debug, Default)]
pub struct Landing {
    target: Option<(String, u64)>,
    focused: HashSet<String>,
    unfocused_since_ms: Option<u64>,
}

impl Landing {
    /// Some window of the app has focus.
    pub fn app_active(&self) -> bool {
        !self.focused.is_empty()
    }

    /// Remember `tab_id` for the next activation. Only an event notified
    /// while the app is not active stores one (the caller checks).
    pub fn set_target(&mut self, tab_id: &str, now_ms: u64) {
        self.target = Some((tab_id.to_string(), now_ms));
    }

    pub fn clear_target(&mut self) {
        self.target = None;
    }

    fn take_fresh(&mut self, now_ms: u64) -> Option<String> {
        let (tab, at) = self.target.take()?;
        (now_ms.saturating_sub(at) <= LANDING_TTL_MS).then_some(tab)
    }

    /// A window gained or lost focus. Returns the tab to land in when this
    /// is an app activation.
    pub fn window_focus(&mut self, label: &str, focused: bool, now_ms: u64) -> Option<String> {
        if !focused {
            self.focused.remove(label);
            if self.focused.is_empty() {
                self.unfocused_since_ms = Some(now_ms);
            }
            return None;
        }
        let was_active = self.app_active();
        let gap = self
            .unfocused_since_ms
            .map_or(u64::MAX, |since| now_ms.saturating_sub(since));
        self.focused.insert(label.to_string());
        if was_active || gap < ACTIVATION_GAP_MS {
            return None;
        }
        self.take_fresh(now_ms)
    }

    /// The dock icon was clicked (macOS `Reopen`): an activation as well.
    #[cfg(any(target_os = "macos", test))]
    pub fn reopen(&mut self, now_ms: u64) -> Option<String> {
        self.take_fresh(now_ms)
    }
}

// ---- reading the run folders ---------------------------------------------

/// Where the auto-post key comes from (the keychain; a fake in tests).
pub trait KeySource {
    fn key(&self) -> Result<trust::Key, String>;
}

pub struct KeychainKey;

impl KeySource for KeychainKey {
    fn key(&self) -> Result<trust::Key, String> {
        trust::install_key()
    }
}

/// What the readers remember between ticks.
#[derive(Default)]
pub struct Reader {
    /// run id → (mtime of autopost.json when read, its failed tickets).
    autopost: HashMap<String, (SystemTime, BTreeSet<String>)>,
    /// Runs whose auto-post record could not be read (logged once).
    autopost_errors: HashSet<String>,
    /// Runs whose usage record could not be written (logged once).
    record_errors: HashSet<String>,
}

impl Reader {
    /// The tickets whose signed auto-post result is `failed`. The keychain
    /// is read only when drafts are written and ship is approved, the record
    /// file exists and its mtime changed since the last read; otherwise the
    /// last answer stands. A read error is logged once and counts as "no
    /// change".
    pub fn autopost_failed(
        &mut self,
        data: &Path,
        run_dir: &Path,
        run_id: &str,
        keys: &dyn KeySource,
    ) -> BTreeSet<String> {
        let last = self.autopost.get(run_id).map(|(_, set)| set.clone());
        if !autopost::may_be_due(run_dir) {
            return last.unwrap_or_default();
        }
        let Some(file) = trust::trust_dir(data, run_dir)
            .ok()
            .map(|dir| dir.join(autopost::AUTOPOST_FILE))
        else {
            return last.unwrap_or_default();
        };
        let Ok(mtime) = std::fs::metadata(&file).and_then(|m| m.modified()) else {
            return last.unwrap_or_default();
        };
        if self
            .autopost
            .get(run_id)
            .is_some_and(|(seen, _)| *seen == mtime)
        {
            return last.unwrap_or_default();
        }
        let read = keys
            .key()
            .and_then(|key| trust::read_autopost(&key, data, run_dir));
        match read {
            Ok(record) => {
                let failed: BTreeSet<String> = record
                    .map(|r| {
                        r.results
                            .into_iter()
                            .filter(|result| result.result == "failed")
                            .map(|result| result.ticket_id)
                            .collect()
                    })
                    .unwrap_or_default();
                self.autopost
                    .insert(run_id.to_string(), (mtime, failed.clone()));
                self.autopost_errors.remove(run_id);
                failed
            }
            Err(err) => {
                if self.autopost_errors.insert(run_id.to_string()) {
                    let _ = folder::append_app_log(
                        run_dir,
                        &format!("attention: could not read the auto-post record: {err}"),
                    );
                }
                // Remember the mtime: a failed read is retried only when the
                // record file changes, not every tick.
                let kept = last.unwrap_or_default();
                self.autopost
                    .insert(run_id.to_string(), (mtime, kept.clone()));
                kept
            }
        }
    }

    /// The facts of one run folder.
    pub fn run_facts(
        &mut self,
        data: &Path,
        run_dir: &Path,
        keys: &dyn KeySource,
    ) -> Option<RunFacts> {
        let run_id = run_dir.file_name()?.to_str()?.to_string();
        // Independent of the diff: a run that is already done at app start,
        // or whose tab closed before a tick, still gets its record. The
        // dev app's adopted runs are left to it.
        if record::own_run_dir(data, &run_dir.display().to_string()).is_some() {
            if let Err(err) = record::ensure_record(run_dir) {
                if self.record_errors.insert(run_id.clone()) {
                    let _ = folder::append_app_log(
                        run_dir,
                        &format!("attention: could not write app-usage.json: {err}"),
                    );
                }
            }
        }
        let value = folder::read_json_value(&run_dir.join("status.json"));
        let view = value.as_ref().and_then(|v| status::parse_status(v).ok());
        let prs = view.as_ref().map(|v| v.prs.as_slice()).unwrap_or_default();
        let checks = status::read_app_checks(run_dir);
        let github = github::read_github(run_dir);
        let cleaned = cleanup::read_cleanup(run_dir)
            .prs
            .into_iter()
            .filter(|(_, rec)| !rec.retry && matches!(rec.outcome.as_str(), "cleaned" | "kept"))
            .map(|(id, rec)| (id, rec.outcome))
            .collect();
        let pr_facts: Vec<PrFact> = prs
            .iter()
            .map(|pr| {
                let gh = github.prs.get(&pr.id);
                PrFact {
                    id: pr.id.clone(),
                    state: pr.state.clone(),
                    app_check: checks.get(&pr.id).map(|c| c.result.clone()),
                    ci: gh.map(|g| g.ci.clone()),
                    pr_state: gh.map(|g| g.pr_state.clone()).filter(|s| !s.is_empty()),
                }
            })
            .collect();
        let waiting_for = view
            .as_ref()
            .and_then(|v| v.waiting_for.clone())
            .filter(|w| !w.is_empty());
        Some(RunFacts {
            run_dir: run_dir.display().to_string(),
            stage: view.as_ref().and_then(|v| v.stage.clone()),
            waiting_for: waiting_for.clone(),
            waiting_since: waiting_for
                .as_ref()
                .and_then(|_| view.as_ref().and_then(|v| v.updated_at.clone())),
            run_mode: folder::run_mode_of(run_dir),
            prs_done: prs
                .iter()
                .filter(|pr| {
                    matches!(
                        pr.state.as_deref(),
                        Some("reviewed" | "drafted" | "opened" | "pushed" | "local-only")
                    )
                })
                .count() as u32,
            prs_total: prs.len() as u32,
            failed_checks: checks
                .into_iter()
                .filter(|(_, check)| matches!(check.result.as_str(), "fail" | "timeout" | "error"))
                .map(|(id, _)| id)
                .collect(),
            cleaned,
            ci_failed: github
                .prs
                .iter()
                .filter(|(_, rec)| rec.ci == "fail")
                .map(|(id, _)| id.clone())
                .collect(),
            prs: pr_facts,
            autopost_failed: self.autopost_failed(data, run_dir, &run_id, keys),
            run_id,
        })
    }
}

/// A run changed within [`BACKGROUND_DAYS`] days (status.json `updatedAt`).
pub fn recent_enough(updated_at: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> bool {
    updated_at
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .is_some_and(|t| now.signed_duration_since(t) <= chrono::Duration::days(BACKGROUND_DAYS))
}

// ---- the thread ------------------------------------------------------------

/// What happens to an event. `notify.rs` has the real sinks.
pub trait Notifier: Send + Sync {
    /// `app_active`: some window of the app has focus.
    fn notify(&self, event: &Event, app_active: bool);
    /// Whether the person is actually told about the event (the master switch
    /// and the event's own switch). Only a told event leaves a landing target.
    fn is_shown(&self, _event: &Event) -> bool {
        true
    }
    /// The latest reading, after every tick (the badge follows it).
    fn badge(&self, _snapshot: &Snapshot) {}
}

/// Run events go to the run's `log.md` (a line of the app's own).
pub struct RunLogNotifier;

impl Notifier for RunLogNotifier {
    fn notify(&self, event: &Event, _app_active: bool) {
        if let Some(dir) = event.run_dir.as_deref() {
            let _ = folder::append_app_log(
                Path::new(dir),
                &format!("attention {}: {}", event.kind.key(), event.text()),
            );
        }
    }
}

/// Hands each event to the sinks. Returns the tabs to land in at the next
/// activation: only events notified while the app is not active have one.
pub fn deliver(events: &[Event], app_active: bool, notifier: &dyn Notifier) -> Vec<String> {
    let mut targets = Vec::new();
    for event in events {
        if !app_active && notifier.is_shown(event) {
            if let Some(tab) = &event.tab_id {
                targets.push(tab.clone());
            }
        }
        notifier.notify(event, app_active);
    }
    targets
}

/// The detector's shared state, managed by the app.
#[derive(Default)]
pub struct AttentionState {
    snapshot: Mutex<Snapshot>,
    landing: Mutex<Landing>,
}

impl AttentionState {
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn app_active(&self) -> bool {
        self.landing.lock().is_ok_and(|l| l.app_active())
    }

    fn set_snapshot(&self, snapshot: Snapshot) {
        if let Ok(mut current) = self.snapshot.lock() {
            *current = snapshot;
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Every unfinished run tab of every window, from the detector's latest reading.
#[tauri::command]
pub fn active_runs(state: tauri::State<AttentionState>) -> Vec<TabSnap> {
    state.snapshot().active_runs()
}

/// Every run tab of every window, finished ones included (the tab progress
/// and the run dashboard); `active_runs` is this minus done/failed.
#[tauri::command]
pub fn run_tabs(state: tauri::State<AttentionState>) -> Vec<TabSnap> {
    state.snapshot().run_tabs()
}

/// The count behind the status bar's attention control (every window).
#[tauri::command]
pub fn attention_summary(state: tauri::State<AttentionState>) -> Summary {
    state.snapshot().summary()
}

/// A window gained or lost focus: tracks app activity and lands in the
/// notified tab on an app activation.
pub fn on_window_focus(app: &AppHandle, label: &str, focused: bool) {
    let Some(state) = app.try_state::<AttentionState>() else {
        return;
    };
    let tab = state
        .landing
        .lock()
        .ok()
        .and_then(|mut l| l.window_focus(label, focused, now_ms()));
    if let Some(tab) = tab {
        land(app, &tab);
    }
}

/// The dock icon was clicked.
#[cfg(target_os = "macos")]
pub fn on_reopen(app: &AppHandle) {
    let Some(state) = app.try_state::<AttentionState>() else {
        return;
    };
    let tab = state
        .landing
        .lock()
        .ok()
        .and_then(|mut l| l.reopen(now_ms()));
    if let Some(tab) = tab {
        land(app, &tab);
    }
}

/// A tab was selected by the person: nothing is pending any more.
pub fn clear_landing(app: &AppHandle) {
    if let Some(state) = app.try_state::<AttentionState>() {
        if let Ok(mut landing) = state.landing.lock() {
            landing.clear_target();
        }
    }
}

fn land(app: &AppHandle, tab_id: &str) {
    if let Err(err) = crate::run::focus_tab(app, tab_id) {
        eprintln!("DCTerminalv2: could not land in the notified tab: {err}");
    }
}

fn data_dir(app: &AppHandle) -> PathBuf {
    crate::data_dir::app_data_dir(app).path
}

fn tab_facts(
    app: &AppHandle,
    tab: &TabRecord,
    minutes: u32,
    hook: Option<&TabHookState>,
) -> TabFacts {
    let run_dir = tab
        .launch
        .as_ref()
        .and_then(|l| l.run_dir.as_deref())
        .and_then(|d| crate::run::checked_run_dir(&data_dir(app), d).ok());
    let activity = hook.map_or(TabActivity::Unknown, |h| h.activity);
    let (stalled, waiting_since_ms) = if tab.kind != TabKind::Claude {
        (false, None)
    } else if run_dir.is_some() {
        match super::observe_tab_now(app, tab, minutes) {
            Ok(seen) => (seen.stall.is_some(), seen.waiting.map(|w| w.since_ms)),
            Err(_) => (false, None),
        }
    } else {
        // A plain tab has no run: only its own wait counts.
        let at = |ms: u64| SystemTime::UNIX_EPOCH + Duration::from_millis(ms);
        let input = WatchInput {
            activity: hook.map(|h| h.activity),
            activity_since: hook.and_then(|h| h.activity_since_ms).map(at),
            ..Default::default()
        };
        (
            false,
            watchdog::waiting(SystemTime::now(), &input, minutes).map(|w| w.since_ms),
        )
    };
    TabFacts {
        tool_running: hook.is_some_and(|h| h.tool_running),
        subagents_running: hook.map_or(0, |h| h.subagents_running),
        tab_id: tab.id.clone(),
        window_label: crate::store::window_label(&tab.window_id).to_string(),
        label: tab.label.clone(),
        is_claude: tab.kind == TabKind::Claude,
        run_id: run_dir
            .as_ref()
            .and_then(|d| d.file_name())
            .map(|n| n.to_string_lossy().to_string()),
        run_dir: run_dir.map(|d| d.display().to_string()),
        activity,
        stalled,
        waiting_since_ms,
    }
}

/// One reading: copies out of the stores, releases them, then reads files.
fn collect(
    app: &AppHandle,
    reader: &mut Reader,
    keys: &dyn KeySource,
    background: bool,
) -> Observed {
    let tabs: Vec<TabRecord> = match app.state::<Mutex<StateStore>>().lock() {
        Ok(state) => state.sorted_tabs().into_iter().cloned().collect(),
        Err(_) => return Observed::default(),
    };
    let minutes = app
        .state::<Mutex<crate::store::SettingsStore>>()
        .lock()
        .map(|s| s.run().watchdog_minutes)
        .unwrap_or(watchdog::DEFAULT_MINUTES);
    let hooks = app
        .state::<super::ObserveState>()
        .hooks
        .as_ref()
        .map(|h| h.states())
        .unwrap_or_default();
    let data = data_dir(app);

    let facts: Vec<TabFacts> = tabs
        .iter()
        .map(|tab| tab_facts(app, tab, minutes, hooks.get(&tab.id)))
        .collect();

    let mut seen_runs: HashSet<String> = HashSet::new();
    let mut runs = Vec::new();
    for tab in &facts {
        let (Some(id), Some(dir)) = (&tab.run_id, &tab.run_dir) else {
            continue;
        };
        if seen_runs.insert(id.clone()) {
            if let Some(run) = reader.run_facts(&data, Path::new(dir), keys) {
                runs.push(run);
            }
        }
    }
    if background {
        let now = chrono::Utc::now();
        for item in folder::list_runs(&data) {
            if item.adopted
                || seen_runs.contains(&item.run_id)
                || !recent_enough(item.updated_at.as_deref(), now)
            {
                continue;
            }
            if let Some(run) = reader.run_facts(&data, Path::new(&item.run_dir), keys) {
                seen_runs.insert(item.run_id);
                runs.push(run);
            }
        }
    }
    Observed {
        tabs: facts,
        runs,
        complete_runs: background,
    }
}

/// Sends one change to every window. A failed emit is dropped: the page
/// polls on its own and catches up.
fn emit_push(app: &AppHandle, push: Push) {
    use super::notify::{RUN_STATE_EVENT, STALL_EVENT, TAB_STATE_EVENT};
    let _ = match &push {
        Push::TabState(p) => app.emit(TAB_STATE_EVENT, p),
        Push::RunState(p) => app.emit(RUN_STATE_EVENT, p),
        Push::Stall(p) => app.emit(STALL_EVENT, p),
    };
}

/// Starts the detector thread (app setup, beside `spawn_afk_sweeps`).
pub fn spawn(app: &AppHandle, notifier: Box<dyn Notifier>) {
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("dcv2-attention".into())
        .spawn(move || {
            let mut detector = Detector::default();
            let mut push_diff = PushDiff::default();
            let mut reader = Reader::default();
            let mut tick: u64 = 0;
            loop {
                let observed = collect(
                    &app,
                    &mut reader,
                    &KeychainKey,
                    tick.is_multiple_of(BACKGROUND_EVERY),
                );
                tick += 1;
                let snapshot = Snapshot::build(
                    &observed.tabs,
                    &observed
                        .runs
                        .iter()
                        .map(|r| (r.run_id.clone(), r.clone()))
                        .collect(),
                );
                let live_runs: std::collections::HashSet<String> = observed
                    .tabs
                    .iter()
                    .filter_map(|t| t.run_dir.clone())
                    .collect();
                crate::run::dispatch::on_tick(&app, &live_runs);
                for push in push_diff.diff(&observed) {
                    emit_push(&app, push);
                }
                let mut events = detector.diff(observed, now_ms());
                events.extend(crate::run::dispatch::milestone_events(&app));
                if let Some(state) = app.try_state::<AttentionState>() {
                    state.set_snapshot(snapshot.clone());
                    // The landing lock is not held while sinks run.
                    let active = state.app_active();
                    let targets = deliver(&events, active, notifier.as_ref());
                    if let (Some(tab), Ok(mut landing)) = (targets.last(), state.landing.lock()) {
                        landing.set_target(tab, now_ms());
                    }
                }
                notifier.badge(&snapshot);
                std::thread::sleep(FAST_TICK);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clickup::autopost::{AutoPostRecord, AutoPostResult};
    use crate::test_support::TempDir;
    use std::cell::Cell;
    use std::sync::Mutex as StdMutex;

    fn tab(id: &str) -> TabFacts {
        TabFacts {
            tab_id: id.into(),
            window_label: "main".into(),
            label: format!("Claude · {id}"),
            is_claude: true,
            run_id: None,
            run_dir: None,
            activity: TabActivity::Working,
            stalled: false,
            waiting_since_ms: None,
            tool_running: false,
            subagents_running: 0,
        }
    }

    fn run_tab(id: &str, run: &str) -> TabFacts {
        TabFacts {
            run_id: Some(run.into()),
            run_dir: Some(format!("/runs/{run}")),
            ..tab(id)
        }
    }

    fn run(id: &str) -> RunFacts {
        RunFacts {
            run_id: id.into(),
            run_dir: format!("/runs/{id}"),
            stage: Some("building".into()),
            ..Default::default()
        }
    }

    fn observed(tabs: Vec<TabFacts>, runs: Vec<RunFacts>) -> Observed {
        Observed {
            tabs,
            runs,
            complete_runs: false,
        }
    }

    fn kinds(events: &[Event]) -> Vec<Kind> {
        events.iter().map(|e| e.kind).collect()
    }

    #[derive(Default)]
    struct FakeNotifier {
        got: StdMutex<Vec<(Kind, bool)>>,
    }

    impl Notifier for FakeNotifier {
        fn notify(&self, event: &Event, app_active: bool) {
            self.got.lock().unwrap().push((event.kind, app_active));
        }
    }

    const T: u64 = 1_000_000;

    fn push_kinds(pushes: &[Push]) -> Vec<&'static str> {
        pushes
            .iter()
            .map(|p| match p {
                Push::TabState(_) => "tab-state",
                Push::RunState(_) => "run-state",
                Push::Stall(_) => "stall",
            })
            .collect()
    }

    #[test]
    fn ci_failed_is_raised_once_on_the_edge_to_fail_per_pr() {
        let mut d = Detector::default();
        let tabs = vec![run_tab("t1", "r-1")];
        d.diff(observed(tabs.clone(), vec![run("r-1")]), T);
        let mut failing = run("r-1");
        failing.ci_failed.insert("PR1".into());
        let fired = d.diff(observed(tabs.clone(), vec![failing.clone()]), T + 2_000);
        assert_eq!(kinds(&fired), [Kind::CiFailed]);
        assert_eq!(fired[0].detail.as_deref(), Some("PR1"));
        assert!(fired[0].text().contains("PR1 CI failed"));
        assert!(
            d.diff(observed(tabs.clone(), vec![failing.clone()]), T + 4_000)
                .is_empty(),
            "once"
        );
        // Another PR failing is its own event; the first stays quiet.
        failing.ci_failed.insert("PR2".into());
        let fired = d.diff(observed(tabs.clone(), vec![failing]), T + 6_000);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].detail.as_deref(), Some("PR2"));
        // A run first seen already failing is seeded silently.
        let mut seeded = run("r-2");
        seeded.ci_failed.insert("PR1".into());
        assert!(d.diff(observed(vec![], vec![seeded]), T + 8_000).is_empty());
    }

    #[test]
    fn a_ci_change_is_one_run_state_with_the_per_pr_fields() {
        let mut p = PushDiff::default();
        let mut r = run("r-1");
        r.prs = vec![PrFact {
            id: "PR1".into(),
            state: Some("opened".into()),
            app_check: Some("pass".into()),
            ci: Some("pending".into()),
            pr_state: Some("OPEN".into()),
        }];
        p.diff(&observed(vec![], vec![r.clone()]));
        assert!(p.diff(&observed(vec![], vec![r.clone()])).is_empty());
        r.prs[0].ci = Some("fail".into());
        let pushes = p.diff(&observed(vec![], vec![r.clone()]));
        assert_eq!(push_kinds(&pushes), ["run-state"]);
        let json = match &pushes[0] {
            Push::RunState(r) => serde_json::to_value(r).unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(json["prs"][0]["ci"], "fail");
        assert_eq!(json["prs"][0]["prState"], "OPEN");
        assert_eq!(json["prs"][0]["appCheck"], "pass");
    }

    #[test]
    fn push_diff_is_silent_first_and_when_nothing_changed() {
        let mut p = PushDiff::default();
        let now = observed(vec![run_tab("t1", "r-1")], vec![run("r-1")]);
        assert!(p.diff(&now).is_empty(), "the first tick is silent");
        assert!(
            p.diff(&now).is_empty(),
            "unchanged observation emits nothing"
        );
        assert!(p.diff(&now).is_empty());
    }

    #[test]
    fn a_hook_state_change_is_one_tab_state_and_a_stall_flip_is_one_stall() {
        let mut p = PushDiff::default();
        p.diff(&observed(vec![tab("a")], vec![]));
        for change in [
            |t: &mut TabFacts| t.activity = TabActivity::NeedsYou,
            |t: &mut TabFacts| t.tool_running = true,
            |t: &mut TabFacts| t.subagents_running = 2,
        ] {
            let mut next = tab("a");
            change(&mut next);
            let first = p.diff(&observed(vec![next.clone()], vec![]));
            assert_eq!(push_kinds(&first), ["tab-state"]);
            assert!(p.diff(&observed(vec![next.clone()], vec![])).is_empty());
            p.diff(&observed(vec![tab("a")], vec![]));
        }
        let mut stalled = tab("a");
        stalled.stalled = true;
        let pushes = p.diff(&observed(vec![stalled.clone()], vec![]));
        assert_eq!(push_kinds(&pushes), ["stall"]);
        assert!(matches!(&pushes[0], Push::Stall(s) if s.stalled && s.tab_id == "a"));
        assert!(p.diff(&observed(vec![stalled], vec![])).is_empty());
        let back = p.diff(&observed(vec![tab("a")], vec![]));
        assert!(matches!(&back[0], Push::Stall(s) if !s.stalled));
    }

    #[test]
    fn a_closed_tab_is_forgotten_at_once() {
        let mut p = PushDiff::default();
        let mut busy = tab("a");
        busy.tool_running = true;
        p.diff(&observed(vec![busy.clone()], vec![]));
        p.diff(&observed(vec![], vec![]));
        // The same tab id opening again is news, not a repeat.
        assert_eq!(
            push_kinds(&p.diff(&observed(vec![busy], vec![]))),
            ["tab-state"]
        );
    }

    #[test]
    fn fast_ticks_that_skip_a_tabless_run_keep_its_memory_and_emit_nothing() {
        let mut p = PushDiff::default();
        let complete = |tabs, runs| Observed {
            tabs,
            runs,
            complete_runs: true,
        };
        // Background tick: the run (no open tab) is read.
        p.diff(&complete(vec![], vec![run("r-9")]));
        // Fast ticks omit it: nothing emitted, memory kept.
        assert!(p.diff(&observed(vec![], vec![])).is_empty());
        assert!(p.diff(&observed(vec![], vec![])).is_empty());
        // The next background tick with the same state emits nothing.
        assert!(p.diff(&complete(vec![], vec![run("r-9")])).is_empty());
        // A background tick with a changed stage emits exactly one run-state.
        let mut done = run("r-9");
        done.stage = Some("done".into());
        let pushes = p.diff(&complete(vec![], vec![done.clone()]));
        assert_eq!(push_kinds(&pushes), ["run-state"]);
        assert!(matches!(&pushes[0], Push::RunState(r) if r.stage.as_deref() == Some("done")));
        // A background tick without the run forgets it: it is new when it returns.
        assert!(p.diff(&complete(vec![], vec![])).is_empty());
        assert_eq!(
            push_kinds(&p.diff(&complete(vec![], vec![done]))),
            ["run-state"]
        );
    }

    #[test]
    fn a_run_first_seen_after_the_first_tick_and_a_stage_change_are_one_run_state() {
        let mut p = PushDiff::default();
        p.diff(&observed(vec![], vec![]));
        let tabs = vec![run_tab("t1", "r-1")];
        assert_eq!(
            push_kinds(&p.diff(&observed(tabs.clone(), vec![run("r-1")]))),
            ["tab-state", "run-state"],
            "a new tab and a new run appear"
        );
        assert!(p.diff(&observed(tabs.clone(), vec![run("r-1")])).is_empty());
        let mut next = run("r-1");
        next.waiting_for = Some("plan-approval".into());
        let pushes = p.diff(&observed(tabs, vec![next]));
        assert_eq!(push_kinds(&pushes), ["run-state"]);
        // Ids and state only: nothing of the run's text.
        let text = serde_json::to_string(&match &pushes[0] {
            Push::RunState(r) => r.clone(),
            _ => unreachable!(),
        })
        .unwrap();
        assert!(text.contains("\"runId\":\"r-1\"") && !text.contains("title"));
    }

    #[test]
    fn the_first_tick_is_silent_and_a_steady_state_stays_silent() {
        let mut d = Detector::default();
        let mut needs = tab("a");
        needs.activity = TabActivity::NeedsYou;
        let mut waiting = run("r-1");
        waiting.waiting_for = Some("plan-approval".into());
        waiting.failed_checks.insert("PR1".into());
        let first = observed(vec![needs.clone()], vec![waiting.clone()]);
        assert!(
            d.diff(first.clone(), T).is_empty(),
            "app start fires nothing"
        );
        assert!(d.diff(first, T + 2_000).is_empty(), "the state holds");
    }

    #[test]
    fn a_tab_needing_you_fires_once_per_transition_and_again_on_re_entry() {
        let mut d = Detector::default();
        d.diff(observed(vec![tab("a")], vec![]), T);
        let mut needs = tab("a");
        needs.activity = TabActivity::NeedsYou;
        let fired = d.diff(observed(vec![needs.clone()], vec![]), T + 2_000);
        assert_eq!(kinds(&fired), [Kind::NeedsYou]);
        assert_eq!(fired[0].tab_id.as_deref(), Some("a"));
        assert!(d
            .diff(observed(vec![needs.clone()], vec![]), T + 4_000)
            .is_empty());
        d.diff(observed(vec![tab("a")], vec![]), T + 6_000);
        // Back in needs-you after the debounce: fires again.
        let again = d.diff(observed(vec![needs], vec![]), T + 40_000);
        assert_eq!(kinds(&again), [Kind::NeedsYou]);
    }

    #[test]
    fn flapping_inside_thirty_seconds_is_announced_once() {
        let mut d = Detector::default();
        d.diff(observed(vec![tab("a")], vec![]), T);
        let mut needs = tab("a");
        needs.activity = TabActivity::NeedsYou;
        assert_eq!(
            d.diff(observed(vec![needs.clone()], vec![]), T + 2_000)
                .len(),
            1
        );
        d.diff(observed(vec![tab("a")], vec![]), T + 4_000);
        assert!(
            d.diff(observed(vec![needs.clone()], vec![]), T + 6_000)
                .is_empty(),
            "29 s after the first: debounced"
        );
        d.diff(observed(vec![tab("a")], vec![]), T + 8_000);
        assert_eq!(
            d.diff(observed(vec![needs], vec![]), T + 33_000).len(),
            1,
            "past 30 s it fires"
        );
    }

    #[test]
    fn stall_and_the_reminder_fire_once_and_the_reminder_once_per_episode() {
        let mut d = Detector::default();
        d.diff(observed(vec![tab("a")], vec![]), T);
        let mut stalled = tab("a");
        stalled.stalled = true;
        assert_eq!(
            kinds(&d.diff(observed(vec![stalled.clone()], vec![]), T + 2_000)),
            [Kind::Stall]
        );
        assert!(d
            .diff(observed(vec![stalled], vec![]), T + 4_000)
            .is_empty());

        let mut waiting = tab("a");
        waiting.activity = TabActivity::NeedsYou;
        waiting.waiting_since_ms = Some(500);
        let fired = d.diff(observed(vec![waiting.clone()], vec![]), T + 100_000);
        assert_eq!(kinds(&fired), [Kind::NeedsYou, Kind::WaitingReminder]);
        assert!(
            d.diff(observed(vec![waiting.clone()], vec![]), T + 200_000)
                .is_empty(),
            "one reminder per episode"
        );
        // A new episode (a later start) reminds again.
        waiting.waiting_since_ms = Some(900_000);
        d.diff(observed(vec![tab("a")], vec![]), T + 300_000);
        let next = d.diff(observed(vec![waiting], vec![]), T + 400_000);
        assert!(kinds(&next).contains(&Kind::WaitingReminder));
    }

    #[test]
    fn run_events_fire_once_per_transition() {
        let mut d = Detector::default();
        let tabs = vec![run_tab("t1", "r-1")];
        d.diff(observed(tabs.clone(), vec![run("r-1")]), T);

        let mut asked = run("r-1");
        asked.waiting_for = Some("ship-approval".into());
        let fired = d.diff(observed(tabs.clone(), vec![asked.clone()]), T + 2_000);
        assert_eq!(kinds(&fired), [Kind::ApprovalWait]);
        assert_eq!(
            fired[0].tab_id.as_deref(),
            Some("t1"),
            "lands in the run's tab"
        );
        assert!(fired[0].text().contains("ship approval"));
        assert!(d
            .diff(observed(tabs.clone(), vec![asked]), T + 4_000)
            .is_empty());

        let mut checked = run("r-1");
        checked.failed_checks.insert("PR1".into());
        let fired = d.diff(observed(tabs.clone(), vec![checked.clone()]), T + 6_000);
        assert_eq!(kinds(&fired), [Kind::CheckFailed]);
        assert_eq!(fired[0].detail.as_deref(), Some("PR1"));
        checked.failed_checks.insert("PR2".into());
        let fired = d.diff(observed(tabs.clone(), vec![checked.clone()]), T + 8_000);
        assert_eq!(fired.len(), 1, "only the new failing PR");

        let mut done = checked.clone();
        done.stage = Some("done".into());
        assert_eq!(
            kinds(&d.diff(observed(tabs.clone(), vec![done.clone()]), T + 10_000)),
            [Kind::RunDone]
        );
        assert!(d
            .diff(observed(tabs.clone(), vec![done]), T + 12_000)
            .is_empty());

        d.diff(observed(vec![], vec![run("r-2")]), T + 14_000);
        let mut failed = run("r-2");
        failed.stage = Some("failed".into());
        assert_eq!(
            kinds(&d.diff(observed(vec![], vec![failed]), T + 16_000)),
            [Kind::RunFailed]
        );
    }

    #[test]
    fn a_failed_autopost_result_fires_once_per_ticket() {
        let mut d = Detector::default();
        let mut base = run("r-1");
        base.autopost_failed.insert("T1".into());
        d.diff(observed(vec![], vec![base.clone()]), T);
        // T1 was already failed at the seed: nothing. T2 newly fails: one.
        base.autopost_failed.insert("T2".into());
        let fired = d.diff(observed(vec![], vec![base.clone()]), T + 2_000);
        assert_eq!(kinds(&fired), [Kind::AutopostFailed]);
        assert_eq!(fired[0].detail.as_deref(), Some("T2"));
        assert!(d.diff(observed(vec![], vec![base]), T + 4_000).is_empty());
    }

    #[test]
    fn a_run_first_seen_later_is_seeded_silently_and_its_next_change_fires() {
        let mut d = Detector::default();
        d.diff(observed(vec![], vec![]), T);
        // It enters the background set already done and already cleaned.
        let mut late = run("r-9");
        late.stage = Some("done".into());
        late.cleaned.insert("PR1".into(), "cleaned".into());
        assert!(d
            .diff(observed(vec![], vec![late.clone()]), T + 60_000)
            .is_empty());
        late.cleaned.insert("PR2".into(), "kept".into());
        let fired = d.diff(observed(vec![], vec![late]), T + 120_000);
        assert_eq!(kinds(&fired), [Kind::MergedCleaned]);
        assert_eq!(fired[0].detail.as_deref(), Some("PR2"));
    }

    #[test]
    fn merged_cleaned_comes_from_a_run_with_no_open_tab_and_a_retry_record_fires_nothing() {
        let tmp = TempDir::new("attention_cleaned");
        let dir = tmp.join("runs/r-20261027-0900-abcd");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("status.json"),
            r#"{"schema":1,"stage":"done","prs":[{"id":"PR1","state":"opened"}]}"#,
        )
        .unwrap();
        let mut reader = Reader::default();
        let keys = CountingKeys::default();
        let mut d = Detector::default();
        let seed = reader.run_facts(tmp.path(), &dir, &keys).unwrap();
        d.diff(observed(vec![], vec![seed]), T);

        let record = |retry: bool, outcome: &str| {
            serde_json::json!({"schema":1,"prs":{"PR1":{
                "prId":"PR1","prUrl":"https://github.com/o/r/pull/1","outcome":outcome,
                "note":"","retry":retry,"worktree":"","branch":"","remote":"","at":"2026-10-27T10:00:00Z"}}})
        };
        std::fs::write(
            dir.join("app-cleanup.json"),
            record(true, "kept").to_string(),
        )
        .unwrap();
        let facts = reader.run_facts(tmp.path(), &dir, &keys).unwrap();
        assert!(facts.cleaned.is_empty(), "a retry record is not final");
        assert!(d.diff(observed(vec![], vec![facts]), T + 60_000).is_empty());

        std::fs::write(
            dir.join("app-cleanup.json"),
            record(false, "cleaned").to_string(),
        )
        .unwrap();
        let facts = reader.run_facts(tmp.path(), &dir, &keys).unwrap();
        let fired = d.diff(observed(vec![], vec![facts]), T + 120_000);
        assert_eq!(kinds(&fired), [Kind::MergedCleaned]);
        assert_eq!(fired[0].tab_id, None, "no open tab");
        assert_eq!(fired[0].run_id.as_deref(), Some("r-20261027-0900-abcd"));
    }

    #[test]
    fn every_kind_reaches_a_fake_notifier_exactly_once() {
        let mut d = Detector::default();
        let tabs = vec![run_tab("t1", "r-1")];
        d.diff(observed(tabs.clone(), vec![run("r-1")]), T);
        let mut t = tabs[0].clone();
        t.activity = TabActivity::NeedsYou;
        t.stalled = true;
        t.waiting_since_ms = Some(7);
        let mut r = run("r-1");
        r.waiting_for = Some("plan-approval".into());
        r.stage = Some("done".into());
        r.failed_checks.insert("PR1".into());
        r.ci_failed.insert("PR1".into());
        r.autopost_failed.insert("T1".into());
        r.cleaned.insert("PR1".into(), "cleaned".into());
        let fired = d.diff(observed(vec![t.clone()], vec![r.clone()]), T + 2_000);
        let fake = FakeNotifier::default();
        deliver(&fired, false, &fake);
        let mut got: Vec<Kind> = fake.got.lock().unwrap().iter().map(|(k, _)| *k).collect();
        got.sort();
        let mut want = vec![
            Kind::NeedsYou,
            Kind::ApprovalWait,
            Kind::Stall,
            Kind::RunDone,
            Kind::CheckFailed,
            Kind::CiFailed,
            Kind::AutopostFailed,
            Kind::WaitingReminder,
            Kind::MergedCleaned,
        ];
        want.sort();
        // Every kind is here but RunFailed (its own transition below) and the three milestone kinds
        // (`batch-planned`, `batch-ready`, `wave-ready`), which `dispatch::milestone_events` raises.
        assert_eq!(
            want.len() + 1 + 3,
            Kind::ALL.len(),
            "every kind is in this test"
        );
        assert_eq!(got, want);
        // The failed kind too, from its own transition.
        let mut f = run("r-1");
        f.stage = Some("failed".into());
        let again = d.diff(observed(vec![t], vec![f]), T + 4_000);
        assert_eq!(kinds(&again), [Kind::RunFailed]);
        assert!(d.diff(observed(vec![], vec![]), T + 6_000).is_empty());
    }

    #[test]
    fn only_events_notified_in_the_background_leave_a_landing_target() {
        let events = vec![Event {
            kind: Kind::NeedsYou,
            subject: "t1".into(),
            tab_id: Some("t1".into()),
            window_label: Some("main".into()),
            tab_label: None,
            run_id: None,
            run_dir: None,
            detail: None,
        }];
        let fake = FakeNotifier::default();
        assert_eq!(deliver(&events, false, &fake), ["t1"]);
        assert!(
            deliver(&events, true, &fake).is_empty(),
            "the app was active"
        );
        assert_eq!(fake.got.lock().unwrap().len(), 2, "both were notified");
    }

    struct SilentNotifier;

    impl Notifier for SilentNotifier {
        fn notify(&self, _event: &Event, _app_active: bool) {}
        fn is_shown(&self, _event: &Event) -> bool {
            false
        }
    }

    #[test]
    fn an_event_that_is_not_shown_leaves_no_landing_target() {
        let events = vec![Event {
            kind: Kind::NeedsYou,
            subject: "t1".into(),
            tab_id: Some("t1".into()),
            window_label: Some("main".into()),
            tab_label: None,
            run_id: None,
            run_dir: None,
            detail: None,
        }];
        assert!(deliver(&events, false, &SilentNotifier).is_empty());
    }

    #[derive(Default)]
    struct CountingKeys {
        calls: Cell<u32>,
    }

    impl KeySource for CountingKeys {
        fn key(&self) -> Result<trust::Key, String> {
            self.calls.set(self.calls.get() + 1);
            Ok(trust::Key::from_bytes([7; 32]))
        }
    }

    fn approved_run(tmp: &TempDir) -> PathBuf {
        let dir = tmp.join("runs/r-20261027-0900-beef");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("status.json"),
            r#"{"schema":1,"stage":"done","shipApproval":{"at":"x"}}"#,
        )
        .unwrap();
        dir
    }

    #[test]
    fn the_keychain_is_never_read_without_drafts_a_record_or_a_new_mtime() {
        let tmp = TempDir::new("attention_keys");
        let dir = approved_run(&tmp);
        let keys = CountingKeys::default();
        let mut reader = Reader::default();
        // No drafts: may_be_due is false.
        reader.run_facts(tmp.path(), &dir, &keys);
        assert_eq!(keys.calls.get(), 0);
        // Drafts written and ship approved, but no auto-post record yet.
        std::fs::write(dir.join(crate::clickup::writeback::DRAFTS_FILE), "{}").unwrap();
        reader.run_facts(tmp.path(), &dir, &keys);
        assert_eq!(keys.calls.get(), 0, "no record file, no key");

        let record = AutoPostRecord {
            schema: 1,
            writeback_sha: "abc".into(),
            at: "2026-10-27T10:00:00Z".into(),
            results: vec![
                AutoPostResult {
                    ticket_id: "T1".into(),
                    result: "posted".into(),
                    ..Default::default()
                },
                AutoPostResult {
                    ticket_id: "T2".into(),
                    result: "failed".into(),
                    ..Default::default()
                },
            ],
        };
        trust::save_autopost(&keys.key().unwrap(), tmp.path(), &dir, &record).unwrap();
        keys.calls.set(0);
        let facts = reader.run_facts(tmp.path(), &dir, &keys).unwrap();
        assert_eq!(keys.calls.get(), 1);
        assert_eq!(facts.autopost_failed.iter().collect::<Vec<_>>(), ["T2"]);
        reader.run_facts(tmp.path(), &dir, &keys);
        assert_eq!(
            keys.calls.get(),
            1,
            "the mtime did not change: not read again"
        );
    }

    #[test]
    fn a_run_done_at_the_first_read_gets_its_record_and_fires_nothing() {
        let tmp = TempDir::new("attention_record");
        let dir = tmp.join("runs/r-20261027-0900-cafe");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("status.json"),
            r#"{"schema":1,"stage":"done","updatedAt":"2026-10-27T10:30:00Z","prs":[]}"#,
        )
        .unwrap();
        let mut reader = Reader::default();
        let keys = CountingKeys::default();
        let mut d = Detector::default();
        let facts = reader.run_facts(tmp.path(), &dir, &keys).unwrap();
        assert!(record::read_record(&dir).is_some_and(|r| r.outcome.as_deref() == Some("done")));
        assert!(d.diff(observed(vec![], vec![facts.clone()]), T).is_empty());
        assert!(d.diff(observed(vec![], vec![facts]), T + 60_000).is_empty());
    }

    #[test]
    fn a_run_with_no_recent_change_is_not_in_the_background_set() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-27T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(recent_enough(Some("2026-10-01T12:00:00Z"), now));
        assert!(!recent_enough(Some("2026-09-01T12:00:00Z"), now));
        assert!(!recent_enough(None, now));
    }

    #[test]
    fn landing_expires_at_120_s_and_a_window_switch_is_not_an_activation() {
        let mut l = Landing::default();
        // The app was in the background; a notification stored a target.
        l.window_focus("main", true, 1_000);
        l.window_focus("main", false, 2_000);
        l.set_target("t1", 3_000);
        // 1 s unfocused, then focus: an activation, and it lands.
        assert_eq!(l.window_focus("main", true, 3_000).as_deref(), Some("t1"));

        // A switch between windows: A loses focus, B gains it 50 ms later.
        l.set_target("t2", 10_000);
        assert_eq!(l.window_focus("main", false, 10_100), None);
        assert_eq!(
            l.window_focus("win-2", true, 10_150),
            None,
            "50 ms: a switch"
        );
        assert!(l.app_active());
        // Later, a real activation lands the still-pending target.
        l.window_focus("win-2", false, 20_000);
        assert_eq!(l.window_focus("main", true, 21_000).as_deref(), Some("t2"));

        // Expiry: a target 121 s old is dropped.
        l.window_focus("main", false, 30_000);
        l.set_target("t3", 30_000);
        assert_eq!(l.window_focus("main", true, 30_000 + 121_000), None);
        l.window_focus("main", false, 200_000);
        assert_eq!(
            l.window_focus("main", true, 201_000),
            None,
            "taken by the expiry"
        );
    }

    #[test]
    fn the_dock_click_lands_a_fresh_target_and_select_clears_it() {
        let mut l = Landing::default();
        l.set_target("t1", 1_000);
        assert_eq!(l.reopen(2_000).as_deref(), Some("t1"));
        l.set_target("t1", 1_000);
        l.clear_target();
        assert_eq!(l.reopen(2_000), None);
        l.set_target("t1", 1_000);
        assert_eq!(l.reopen(1_000 + LANDING_TTL_MS + 1), None);
    }

    #[test]
    fn the_snapshot_carries_each_tabs_run_and_pr_progress() {
        let mut waiting = run("r-1");
        waiting.waiting_for = Some("plan-approval".into());
        waiting.prs_total = 3;
        waiting.prs_done = 1;
        waiting.run_mode = Some("epic-plan".into());
        let mut done = run("r-2");
        done.stage = Some("done".into());
        let runs: HashMap<String, RunFacts> = [waiting, done]
            .into_iter()
            .map(|r| (r.run_id.clone(), r))
            .collect();
        let mut asking = tab("a");
        asking.activity = TabActivity::NeedsYou;
        let tabs = vec![run_tab("t1", "r-1"), run_tab("t2", "r-2"), asking, tab("b")];
        let snap = Snapshot::build(&tabs, &runs);
        assert_eq!(snap.tabs.len(), 4);
        // The approval wait and the needs-you tab need the person; the
        // finished run's tab is not an active run.
        assert_eq!(
            snap.summary(),
            Summary {
                needs_you: 2,
                active_runs: 1
            }
        );
        let active = snap.active_runs();
        assert_eq!(active.len(), 1);
        assert_eq!((active[0].prs_done, active[0].prs_total), (1, 3));
        assert_eq!(active[0].tab_id, "t1");
        assert_eq!(active[0].run_mode.as_deref(), Some("epic-plan"));
        assert_eq!(snap.tabs[1].run_mode, None);
        assert_eq!(Snapshot::default().summary(), Summary::default());
        assert_eq!(snap.tabs[0].waiting_for.as_deref(), Some("plan-approval"));
        assert_eq!(snap.tabs[2].activity, TabActivity::NeedsYou);
        assert_eq!((snap.tabs[0].prs_done, snap.tabs[0].prs_total), (1, 3));
    }

    #[test]
    fn run_tabs_keep_finished_runs_and_carry_the_wait_start_and_prs() {
        let mut waiting = run("r-1");
        waiting.waiting_for = Some("plan-approval".into());
        waiting.waiting_since = Some("2026-11-01T10:00:00Z".into());
        waiting.prs = vec![PrFact {
            id: "PR1".into(),
            state: Some("opened".into()),
            ..Default::default()
        }];
        let mut done = run("r-2");
        done.stage = Some("done".into());
        let mut failed = run("r-3");
        failed.stage = Some("failed".into());
        let runs: HashMap<String, RunFacts> = [waiting, done, failed]
            .into_iter()
            .map(|r| (r.run_id.clone(), r))
            .collect();
        let tabs = vec![
            run_tab("t1", "r-1"),
            run_tab("t2", "r-2"),
            run_tab("t3", "r-3"),
            tab("plain"),
        ];
        let snap = Snapshot::build(&tabs, &runs);
        let all = snap.run_tabs();
        assert_eq!(
            all.iter().map(|t| t.tab_id.as_str()).collect::<Vec<_>>(),
            ["t1", "t2", "t3"]
        );
        assert_eq!(snap.active_runs().len(), 1);
        assert_eq!(snap.summary().active_runs, 1);
        assert_eq!(
            all[0].waiting_since.as_deref(),
            Some("2026-11-01T10:00:00Z")
        );
        assert_eq!(all[1].waiting_since, None);
        assert_eq!(all[0].prs.len(), 1);
        assert_eq!(all[0].prs[0].id, "PR1");
        let json = serde_json::to_value(&all[0]).unwrap();
        assert!(json.get("waitingSince").is_some() && json.get("prs").is_some());
    }
}
