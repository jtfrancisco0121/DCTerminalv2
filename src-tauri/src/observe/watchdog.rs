//! R2: no `status.json` change and no transcript write for N minutes while
//! the run is not waiting for you and not finished → the panel says so and
//! offers Nudge (only on click) and Resume.

use super::hooks::TabActivity;
use serde::Serialize;
use std::time::{Duration, SystemTime};

pub const DEFAULT_MINUTES: u32 = 10;

#[derive(Debug, Clone, Default)]
pub struct WatchInput {
    pub status_changed: Option<SystemTime>,
    pub transcript_written: Option<SystemTime>,
    /// status.json's `waitingFor` is set (Claude asks you in the tab).
    pub waiting_for: bool,
    /// The run is `done` or `failed`, or has no status.json yet.
    pub finished: bool,
    /// From hook events; None when the tab has no hook state.
    pub activity: Option<TabActivity>,
    /// A tool started and has not finished (a long Bash, say).
    pub tool_running: bool,
    /// The last hook event.
    pub hook_updated: Option<SystemTime>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stall {
    /// Whole minutes since the last sign of progress.
    pub minutes: u64,
    pub threshold: u32,
}

/// `now` is passed in so tests can move the clock.
pub fn check(now: SystemTime, input: &WatchInput, threshold_minutes: u32) -> Option<Stall> {
    if input.waiting_for || input.finished || threshold_minutes == 0 {
        return None;
    }
    let threshold = Duration::from_secs(u64::from(threshold_minutes) * 60);
    match input.activity {
        // Claude asks you something in the tab: that is not a stall.
        Some(TabActivity::NeedsYou) => return None,
        // A running tool is work, but a tool whose last hook event is older
        // than twice the threshold is stuck (a crash never sent PostToolUse).
        Some(TabActivity::Working) if input.tool_running => {
            let age = input
                .hook_updated
                .map(|at| now.duration_since(at).unwrap_or(Duration::ZERO));
            if age.is_none_or(|age| age < threshold * 2) {
                return None;
            }
        }
        _ => {}
    }
    let last = [
        input.status_changed,
        input.transcript_written,
        input.hook_updated,
    ]
    .into_iter()
    .flatten()
    .max()?;
    let idle = now.duration_since(last).unwrap_or(Duration::ZERO);
    (idle >= threshold).then_some(Stall {
        minutes: idle.as_secs() / 60,
        threshold: threshold_minutes,
    })
}

/// What Nudge types into the orchestrator's terminal (then Enter).
pub const NUDGE_TEXT: &str = "continue from status.json";

/// Nudge never types into a tab that may show a permission menu.
pub fn nudge_refusal(activity: Option<TabActivity>) -> Option<&'static str> {
    (activity == Some(TabActivity::NeedsYou))
        .then_some("Claude is asking you something in the tab; answer it there first.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stall_needs_both_signals_quiet_for_the_whole_threshold() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let min = |m: u64| Duration::from_secs(m * 60);
        let input = WatchInput {
            status_changed: Some(t0),
            transcript_written: Some(t0 + min(4)),
            ..Default::default()
        };
        assert_eq!(
            check(t0 + min(13), &input, 10),
            None,
            "the transcript moved 9 min ago"
        );
        assert_eq!(
            check(t0 + min(14), &input, 10),
            Some(Stall {
                minutes: 10,
                threshold: 10
            })
        );
        assert_eq!(
            check(
                t0 + min(30),
                &WatchInput {
                    waiting_for: true,
                    ..input.clone()
                },
                10
            ),
            None
        );
        assert_eq!(
            check(
                t0 + min(30),
                &WatchInput {
                    finished: true,
                    ..input.clone()
                },
                10
            ),
            None
        );
        assert_eq!(
            check(t0 + min(30), &input, 0),
            None,
            "0 turns the watchdog off"
        );
        assert_eq!(
            check(t0 + min(30), &WatchInput::default(), 10),
            None,
            "nothing to measure"
        );
        let quiet = t0 + min(30);
        let asking = WatchInput {
            activity: Some(TabActivity::NeedsYou),
            ..input.clone()
        };
        assert_eq!(
            check(quiet, &asking, 10),
            None,
            "a permission prompt is not a stall"
        );
        let long_bash = WatchInput {
            activity: Some(TabActivity::Working),
            tool_running: true,
            ..input.clone()
        };
        assert_eq!(
            check(quiet, &long_bash, 10),
            None,
            "a tool still running is work"
        );
        let hook_moved = WatchInput {
            hook_updated: Some(t0 + min(25)),
            ..input.clone()
        };
        assert_eq!(
            check(quiet, &hook_moved, 10),
            None,
            "a recent hook event is progress"
        );
        let idle = WatchInput {
            activity: Some(TabActivity::Working),
            ..input.clone()
        };
        assert!(check(quiet, &idle, 10).is_some());
        let only_status = WatchInput {
            status_changed: Some(t0),
            ..Default::default()
        };
        assert_eq!(check(t0 + min(11), &only_status, 10).unwrap().minutes, 11);
        assert_eq!(
            check(
                t0,
                &WatchInput {
                    status_changed: Some(t0 + min(1)),
                    ..Default::default()
                },
                1
            ),
            None,
            "clock skew"
        );
    }

    // Ticket 14yq6g30jek: a tool that never finished (the program crashed)
    // held the watchdog off for good.
    #[test]
    fn a_tool_running_for_twice_the_threshold_stops_holding_off_the_watchdog() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let secs = Duration::from_secs;
        // Threshold 10 min = 600 s. The last hook event is at t0.
        let input = WatchInput {
            status_changed: Some(t0),
            transcript_written: Some(t0),
            activity: Some(TabActivity::Working),
            tool_running: true,
            hook_updated: Some(t0),
            ..Default::default()
        };
        assert_eq!(
            check(t0 + secs(1140), &input, 10),
            None,
            "1.9 x: still work"
        );
        assert!(check(t0 + secs(1200), &input, 10).is_some(), "2 x: a stall");
        assert!(check(t0 + secs(3000), &input, 10).is_some());
    }
}
