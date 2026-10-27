//! App actions reachable by typing (plan 2026-10-27, AFK runs). Claude asks
//! with `state.sh request <run> <kind>`, which writes
//! `<run>/requests/<id>.json`; the app performs only its fixed, idempotent
//! actions for the known kinds and answers in `<run>/requests/<id>.result.json`
//! plus an `app:` line in `log.md`.
//!
//! A request is untrusted Claude output: the app takes nothing from it but
//! the kind (the file name's and the body's must agree), never a path, a PR
//! id or text. Unknown kinds are answered `ignored`; one request per run and
//! kind per [`RATE`] is performed (later ones are answered `refused`).

use super::folder::{append_app_log, read_json_value};
use crate::store::write_json_atomic;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const REQUESTS_DIR: &str = "requests";
pub const KINDS: &[&str] = &["recheck", "cleanup", "tickets", "selftest"];
/// One performed request per run and kind in this time.
pub const RATE: Duration = Duration::from_secs(60);
/// A request file larger than this is not read.
const MAX_REQUEST_BYTES: u64 = 4096;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RequestResult {
    pub schema: u32,
    pub id: String,
    pub kind: String,
    /// `running`, `done`, `refused` or `ignored`.
    pub state: String,
    pub ok: Option<bool>,
    pub message: String,
    pub at: String,
}

/// The actions the app performs (the app's own in production, fakes in
/// tests). `Ok` is a one-line success message, `Err` a one-line failure.
pub trait Actions {
    fn perform(&self, run_dir: &Path, kind: &str) -> Result<String, String>;
}

/// When each run and kind was last performed.
#[derive(Default)]
pub struct Limiter {
    last: HashMap<(PathBuf, String), Instant>,
}

impl Limiter {
    /// `Some(seconds ago)` when the kind ran for this run within [`RATE`];
    /// else records `now` and returns None.
    fn take(&mut self, run_dir: &Path, kind: &str, now: Instant) -> Option<u64> {
        let key = (run_dir.to_path_buf(), kind.to_string());
        if let Some(at) = self.last.get(&key) {
            let ago = now.saturating_duration_since(*at);
            if ago < RATE {
                return Some(ago.as_secs());
            }
        }
        self.last.insert(key, now);
        None
    }
}

/// `<id>.json` names the app answers: letters, digits and `-` only.
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn result_path(run_dir: &Path, id: &str) -> PathBuf {
    run_dir.join(REQUESTS_DIR).join(format!("{id}.result.json"))
}

fn now_text() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn one_line(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(500)
        .collect()
}

/// A request waiting for its answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub id: String,
    /// The kind when the file name and the body agree on a known one.
    pub kind: Option<String>,
}

/// The run's requests with no result yet, oldest first (ids start with
/// the time). Files with an unsafe name are skipped without an answer.
pub fn pending(run_dir: &Path) -> Vec<Pending> {
    let Ok(entries) = std::fs::read_dir(run_dir.join(REQUESTS_DIR)) else {
        return Vec::new();
    };
    let mut out: Vec<Pending> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            if name.starts_with('.') || name.ends_with(".result.json") {
                return None;
            }
            let id = name.strip_suffix(".json")?.to_string();
            if !safe_id(&id) || result_path(run_dir, &id).exists() {
                return None;
            }
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let named = KINDS
                .iter()
                .find(|k| id.ends_with(&format!("-{k}")))
                .map(|k| k.to_string());
            let body = (meta.len() <= MAX_REQUEST_BYTES)
                .then(|| read_json_value(&e.path()))
                .flatten();
            let said = body
                .as_ref()
                .and_then(|b| b.get("kind"))
                .and_then(|k| k.as_str())
                .map(str::to_string);
            let kind = named.filter(|k| said.as_deref() == Some(k.as_str()));
            Some(Pending { id, kind })
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn write_result(
    run_dir: &Path,
    id: &str,
    kind: &str,
    state: &str,
    ok: Option<bool>,
    message: &str,
) {
    let result = RequestResult {
        schema: 1,
        id: id.to_string(),
        kind: kind.to_string(),
        state: state.to_string(),
        ok,
        message: one_line(message),
        at: now_text(),
    };
    if let Err(err) = write_json_atomic(&result_path(run_dir, id), &result) {
        eprintln!("DCTerminalv2: request {id}: {err}");
    }
}

/// What to do with one pending request: answered right away (ignored or
/// refused: `None`), or to perform (`Some(kind)`, its `running` answer
/// written first, so a second look never starts it again).
pub fn accept(run_dir: &Path, p: &Pending, limiter: &mut Limiter, now: Instant) -> Option<String> {
    let Some(kind) = p.kind.clone() else {
        write_result(
            run_dir,
            &p.id,
            "",
            "ignored",
            Some(false),
            &format!("unknown request: the app does only {}", KINDS.join(", ")),
        );
        return None;
    };
    if let Some(ago) = limiter.take(run_dir, &kind, now) {
        let message = format!(
            "refused: one {kind} request per {} s (the last one was {ago} s ago)",
            RATE.as_secs()
        );
        write_result(run_dir, &p.id, &kind, "refused", Some(false), &message);
        let _ = append_app_log(run_dir, &format!("request {kind}: {message}"));
        return None;
    }
    write_result(
        run_dir,
        &p.id,
        &kind,
        "running",
        None,
        &format!("{kind}: running"),
    );
    Some(kind)
}

/// Performs an accepted request and writes its answer and log line.
pub fn perform(run_dir: &Path, id: &str, kind: &str, actions: &dyn Actions) -> RequestResult {
    let (ok, message) = match actions.perform(run_dir, kind) {
        Ok(m) => (true, m),
        Err(m) => (false, m),
    };
    write_result(run_dir, id, kind, "done", Some(ok), &message);
    let _ = append_app_log(run_dir, &format!("request {kind}: {}", one_line(&message)));
    read_json_value(&result_path(run_dir, id))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

/// At app start: answers a previous app left `running` (it stopped
/// mid-way) are closed as not done, so `state.sh request-wait` stops waiting.
pub fn fail_interrupted(run_dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(run_dir.join(REQUESTS_DIR)) else {
        return 0;
    };
    let mut n = 0;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix(".result.json") else {
            continue;
        };
        let Some(old) = read_json_value(&e.path())
            .and_then(|v| serde_json::from_value::<RequestResult>(v).ok())
        else {
            continue;
        };
        if old.state == "running" && safe_id(id) {
            let message = "the app stopped before this finished; ask again";
            write_result(run_dir, id, &old.kind, "done", Some(false), message);
            n += 1;
        }
    }
    n
}

/// Accepts and performs the run's pending requests in turn (the tests'
/// path; the app performs each on its own thread).
#[cfg(test)]
pub fn handle_run(
    run_dir: &Path,
    actions: &dyn Actions,
    limiter: &mut Limiter,
    now: Instant,
) -> Vec<RequestResult> {
    let mut out = Vec::new();
    for p in pending(run_dir) {
        if let Some(kind) = accept(run_dir, &p, limiter, now) {
            out.push(perform(run_dir, &p.id, &kind, actions));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use serde_json::json;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        done: RefCell<Vec<String>>,
    }

    impl Actions for Fake {
        fn perform(&self, _run_dir: &Path, kind: &str) -> Result<String, String> {
            self.done.borrow_mut().push(kind.to_string());
            if kind == "tickets" {
                Err("ClickUp: no token for this account".into())
            } else {
                Ok(format!("{kind} done\nsecond line"))
            }
        }
    }

    fn request(run: &Path, id: &str, body: serde_json::Value) {
        let dir = run.join(REQUESTS_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{id}.json")), body.to_string()).unwrap();
    }

    fn result(run: &Path, id: &str) -> RequestResult {
        serde_json::from_value(read_json_value(&result_path(run, id)).unwrap()).unwrap()
    }

    #[test]
    fn known_requests_run_once_and_unknown_or_forged_ones_are_ignored() {
        let dir = TempDir::new("requests");
        let run = dir.join("runs/r-1");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(
            run.join("status.json"),
            json!({"stage": "done"}).to_string(),
        )
        .unwrap();
        request(
            &run,
            "20261027T100000Z-recheck",
            json!({"schema": 1, "kind": "recheck"}),
        );
        request(
            &run,
            "20261027T100001Z-merge",
            json!({"schema": 1, "kind": "merge"}),
        );
        // The name says recheck, the body something else: not performed.
        request(
            &run,
            "20261027T100002Z-recheck",
            json!({"kind": "cleanup", "path": "/etc"}),
        );
        request(&run, "20261027T100003Z-tickets", json!({"kind": "tickets"}));
        // An unsafe name is never answered (no file is made from it).
        std::fs::write(run.join(REQUESTS_DIR).join("a b.json"), "{}").unwrap();
        let fake = Fake::default();
        let mut limiter = Limiter::default();
        let now = Instant::now();
        let results = handle_run(&run, &fake, &mut limiter, now);
        assert_eq!(fake.done.borrow().as_slice(), ["recheck", "tickets"]);
        assert_eq!(results.len(), 2);

        let r = result(&run, "20261027T100000Z-recheck");
        assert_eq!((r.state.as_str(), r.ok), ("done", Some(true)));
        assert_eq!(r.message, "recheck done second line");
        let t = result(&run, "20261027T100003Z-tickets");
        assert_eq!((t.state.as_str(), t.ok), ("done", Some(false)));
        assert!(t.message.contains("no token"));
        for id in ["20261027T100001Z-merge", "20261027T100002Z-recheck"] {
            let x = result(&run, id);
            assert_eq!(x.state, "ignored", "{id}");
            assert!(x.message.contains("unknown request"), "{id}");
        }
        assert!(!run.join(REQUESTS_DIR).join("a b.result.json").exists());

        // Answered requests are never performed again.
        assert!(handle_run(&run, &fake, &mut limiter, now + RATE * 2).is_empty());
        assert_eq!(fake.done.borrow().len(), 2);

        let log = std::fs::read_to_string(run.join("log.md")).unwrap();
        assert!(
            log.contains(" done app: request recheck: recheck done second line"),
            "{log}"
        );
        assert!(
            log.contains(" done app: request tickets: ClickUp: no token"),
            "{log}"
        );
    }

    #[test]
    fn one_request_per_kind_and_run_in_sixty_seconds() {
        let dir = TempDir::new("requests_rate");
        let run = dir.join("runs/r-2");
        let other = dir.join("runs/r-3");
        for r in [&run, &other] {
            std::fs::create_dir_all(r).unwrap();
        }
        let fake = Fake::default();
        let mut limiter = Limiter::default();
        let t0 = Instant::now();
        request(&run, "20261027T100000Z-recheck", json!({"kind": "recheck"}));
        handle_run(&run, &fake, &mut limiter, t0);
        request(&run, "20261027T100010Z-recheck", json!({"kind": "recheck"}));
        request(
            &run,
            "20261027T100011Z-selftest",
            json!({"kind": "selftest"}),
        );
        request(
            &other,
            "20261027T100012Z-recheck",
            json!({"kind": "recheck"}),
        );
        handle_run(&run, &fake, &mut limiter, t0 + Duration::from_secs(10));
        handle_run(&other, &fake, &mut limiter, t0 + Duration::from_secs(10));
        let refused = result(&run, "20261027T100010Z-recheck");
        assert_eq!(refused.state, "refused");
        assert!(
            refused.message.contains("one recheck request per 60 s"),
            "{refused:?}"
        );
        assert_eq!(result(&run, "20261027T100011Z-selftest").state, "done");
        assert_eq!(result(&other, "20261027T100012Z-recheck").state, "done");
        request(&run, "20261027T100100Z-recheck", json!({"kind": "recheck"}));
        handle_run(&run, &fake, &mut limiter, t0 + Duration::from_secs(61));
        assert_eq!(result(&run, "20261027T100100Z-recheck").state, "done");
        assert_eq!(
            fake.done.borrow().as_slice(),
            ["recheck", "selftest", "recheck", "recheck"]
        );
    }

    #[test]
    fn an_accepted_request_is_marked_running_before_it_runs() {
        let dir = TempDir::new("requests_running");
        let run = dir.join("runs/r-4");
        std::fs::create_dir_all(&run).unwrap();
        request(
            &run,
            "20261027T100000Z-selftest",
            json!({"kind": "selftest"}),
        );
        let mut limiter = Limiter::default();
        let p = pending(&run);
        assert_eq!(p.len(), 1);
        assert_eq!(
            accept(&run, &p[0], &mut limiter, Instant::now()).as_deref(),
            Some("selftest")
        );
        assert_eq!(result(&run, "20261027T100000Z-selftest").state, "running");
        assert!(pending(&run).is_empty(), "never picked up twice");
        // An app that stopped mid-way: the next one closes the answer.
        assert_eq!(fail_interrupted(&run), 1);
        let closed = result(&run, "20261027T100000Z-selftest");
        assert_eq!((closed.state.as_str(), closed.ok), ("done", Some(false)));
        assert_eq!(closed.kind, "selftest");
        assert_eq!(fail_interrupted(&run), 0);
        // A huge request file is not read: it is answered as unknown.
        let big = format!("{{\"kind\": \"recheck\", \"x\": \"{}\"}}", "a".repeat(5000));
        std::fs::write(
            run.join(REQUESTS_DIR).join("20261027T100001Z-recheck.json"),
            big,
        )
        .unwrap();
        assert_eq!(pending(&run)[0].kind, None);
    }
}
