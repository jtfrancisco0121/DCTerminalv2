//! Auto-post retries that fail before anything is recorded (no token, the
//! keychain, a save error) wait 1, 2, 4 ... minutes, at most 60, instead of
//! trying every sweep. Kept in process memory only: a restart is a fresh
//! start, and there is no second persistence format. Setting or clearing a
//! token, or changing the auto-post setting, clears every wait; the panel's
//! "Try now" clears one run's.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The first wait, in seconds; each failure doubles it.
pub const FIRST_DELAY: u64 = 60;
/// The longest wait, in seconds.
pub const MAX_DELAY: u64 = 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wait {
    pub reason: String,
    /// Unix seconds of the next automatic try; `None`: not before the user changes something.
    pub next_at: Option<u64>,
    /// The ClickUp token is missing for the run's account.
    pub no_token: bool,
    failures: u32,
}

/// What waits, and until when. The clock is passed in (`now`, unix seconds).
#[derive(Debug, Default)]
pub struct Backoff {
    runs: HashMap<PathBuf, Wait>,
    /// Accounts known to have no token: their keychain item is not read again until cleared.
    no_token: HashSet<String>,
}

/// The wait after `failures` failures in a row.
pub fn delay_after(failures: u32) -> u64 {
    FIRST_DELAY
        .saturating_mul(1u64 << failures.saturating_sub(1).min(20))
        .min(MAX_DELAY)
}

impl Backoff {
    /// An attempt for this run may start now.
    pub fn allowed(&self, run: &Path, now: u64) -> bool {
        self.runs
            .get(run)
            .is_none_or(|wait| wait.next_at.is_some_and(|at| now >= at))
    }

    /// An attempt failed before a record was saved: wait twice as long as last time.
    pub fn failed(&mut self, run: &Path, reason: &str, now: u64) {
        let failures = self.runs.get(run).map_or(0, |w| w.failures) + 1;
        self.runs.insert(
            run.to_path_buf(),
            Wait {
                reason: reason.to_string(),
                next_at: Some(now + delay_after(failures)),
                no_token: false,
                failures,
            },
        );
    }

    /// The account has no token: no keychain read, for this run or any run of the account, until cleared.
    pub fn missing_token(&mut self, run: &Path, account: &str, reason: &str) {
        self.no_token.insert(account.to_string());
        self.runs.insert(
            run.to_path_buf(),
            Wait {
                reason: reason.to_string(),
                next_at: None,
                no_token: true,
                failures: 0,
            },
        );
    }

    pub fn account_has_no_token(&self, account: &str) -> bool {
        self.no_token.contains(account)
    }

    pub fn waiting(&self, run: &Path) -> Option<&Wait> {
        self.runs.get(run)
    }

    /// An attempt for this run finished without failing.
    pub fn succeeded(&mut self, run: &Path) {
        self.runs.remove(run);
    }

    /// "Try now": this run's wait, and its account's no-token mark, are over.
    pub fn clear_run(&mut self, run: &Path, account: &str) {
        self.runs.remove(run);
        self.no_token.remove(account);
    }

    /// A token was set or cleared, or the auto-post setting changed.
    pub fn clear_all(&mut self) {
        self.runs.clear();
        self.no_token.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_attempt_inside_the_delay_is_skipped_and_the_delay_doubles_up_to_an_hour() {
        let run = Path::new("/runs/r-1");
        let mut b = Backoff::default();
        assert!(b.allowed(run, 1000), "nothing failed yet");
        b.failed(run, "disk full", 1000);
        assert!(!b.allowed(run, 1059), "inside the first minute");
        assert!(b.allowed(run, 1060));
        b.failed(run, "disk full", 1060);
        assert!(!b.allowed(run, 1060 + 119));
        assert!(b.allowed(run, 1060 + 120), "the second wait is two minutes");
        let minutes: Vec<u64> = (1..=9).map(|n| delay_after(n) / 60).collect();
        assert_eq!(minutes, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(delay_after(10_000), MAX_DELAY, "no overflow");
        for _ in 0..12 {
            b.failed(run, "disk full", 0);
        }
        assert_eq!(b.waiting(run).unwrap().next_at, Some(MAX_DELAY));
        // Other runs are not held.
        assert!(b.allowed(Path::new("/runs/r-2"), 0));
        // A clean attempt ends the wait; the next failure starts at one minute again.
        b.succeeded(run);
        assert!(b.allowed(run, 0));
        b.failed(run, "x", 0);
        assert_eq!(b.waiting(run).unwrap().next_at, Some(FIRST_DELAY));
    }

    #[test]
    fn no_token_holds_the_account_until_a_token_is_set() {
        let (one, two) = (Path::new("/runs/r-1"), Path::new("/runs/r-2"));
        let mut b = Backoff::default();
        b.missing_token(one, "work", "no token");
        assert!(b.account_has_no_token("work"));
        assert!(!b.account_has_no_token("personal"));
        assert!(!b.allowed(one, u64::MAX), "no timer ends a missing token");
        assert!(b.waiting(one).unwrap().no_token);
        assert_eq!(b.waiting(one).unwrap().next_at, None);
        // Setting (or clearing) a token clears every wait and the mark.
        b.failed(two, "x", 0);
        b.clear_all();
        assert!(!b.account_has_no_token("work"));
        assert!(b.allowed(one, 0) && b.allowed(two, 0));
        assert!(b.waiting(one).is_none());
    }

    #[test]
    fn try_now_clears_one_run_and_its_account() {
        let (one, two) = (Path::new("/runs/r-1"), Path::new("/runs/r-2"));
        let mut b = Backoff::default();
        b.failed(one, "x", 0);
        b.missing_token(two, "work", "no token");
        b.clear_run(one, "personal");
        assert!(b.allowed(one, 0));
        assert!(!b.allowed(two, 0), "the other run still waits");
        b.clear_run(two, "work");
        assert!(b.allowed(two, 0) && !b.account_has_no_token("work"));
    }
}
