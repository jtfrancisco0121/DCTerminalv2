//! R2: no `status.json` change and no transcript write for N minutes while
//! the run is not waiting for you and not finished → the panel says so and
//! offers Nudge (only on click) and Resume.

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
    let last = match (input.status_changed, input.transcript_written) {
        (Some(a), Some(b)) => a.max(b),
        (a, b) => a.or(b)?,
    };
    let idle = now.duration_since(last).unwrap_or(Duration::ZERO);
    let threshold = Duration::from_secs(u64::from(threshold_minutes) * 60);
    (idle >= threshold).then_some(Stall {
        minutes: idle.as_secs() / 60,
        threshold: threshold_minutes,
    })
}

/// What Nudge types into the orchestrator's terminal (then Enter).
pub const NUDGE_TEXT: &str = "continue from status.json";

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
}
