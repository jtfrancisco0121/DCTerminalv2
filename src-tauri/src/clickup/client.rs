//! The ClickUp calls. Each one goes through a [`Transport`], with the rules
//! the GitHub poller follows: at most 4 calls in flight across the app, the
//! rate-limit headers honoured, a 429 backed off and retried, and errors in
//! plain English.
//!
//! There are exactly two writes (C3, plan decision E4), and both go through
//! [`ClickUp::write`] under the same rules: [`ClickUp::post_comment`] and
//! [`ClickUp::set_status`]. Nothing here creates, deletes, assigns or edits
//! a description; a test reads this file to keep it that way. A 429 on a
//! write is retried like a read's, since ClickUp did not act on it; any
//! other failure is returned, never retried.
//!
//! ClickUp counts calls per token per minute, so each token has its own
//! rate-limit window: one account's spent limit never stalls another's
//! calls. When a reply says none are left, later calls with that token wait
//! for the reset instead of spending a 429 each; a wait longer than
//! [`MAX_WAIT_SECS`] is returned as an error instead, so a command never
//! hangs on it.

use super::transport::{NoReply, Reply, Transport, WriteMethod, REQUEST_TIMEOUT};
use super::types::{Comment, Comments, List, Task, TaskPage, Team, Teams};
use super::{valid_id, ClickUpToken};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Condvar, LazyLock, Mutex};
use std::time::Duration;

/// ClickUp calls in flight at once, app-wide (the GitHub poller's rule).
pub const MAX_IN_FLIGHT: usize = 4;
/// The longest a call waits for the rate limit before giving up.
const MAX_WAIT_SECS: u64 = 60;
/// The longest wait a rate-limit header is taken to ask for. ClickUp's window
/// is a minute, so more is a bad header; the cap also keeps the time
/// arithmetic in range whatever the header says.
const MAX_HEADER_WAIT_SECS: u64 = 3_600;
/// A 429 is retried this many times.
const RETRIES_ON_429: u32 = 2;
/// The wait after a 429 that names no reset time.
const DEFAULT_429_WAIT: u64 = 10;
/// ClickUp's page size for `GET /list/{id}/task`.
const PAGE_SIZE: usize = 100;
/// The most pages read from one list (10,000 tasks).
const MAX_PAGES: u32 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClickUpError {
    NoToken,
    /// 401 or 403.
    Rejected,
    /// 404. Names what was asked for.
    NotFound(&'static str),
    BadId,
    /// Seconds until the limit resets, when known.
    RateLimited(u64),
    Server(u16),
    /// 400 or 422: ClickUp would not make a change (C3), such as a status
    /// the task's list does not have.
    Refused(u16),
    Unexpected(u16),
    Unreadable,
    NoReply(NoReply),
}

impl fmt::Display for ClickUpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoToken => {
                f.write_str("No ClickUp token yet. Add one under Settings › ClickUp.")
            }
            Self::Rejected => {
                f.write_str("ClickUp rejected the token. Check it under Settings › ClickUp.")
            }
            Self::NotFound(what) => write!(
                f,
                "ClickUp has no {what} with that id, or your token cannot see it."
            ),
            Self::BadId => f.write_str("That is not a ClickUp id."),
            Self::RateLimited(secs) => write!(
                f,
                "ClickUp's rate limit is used up. Try again in {secs} seconds."
            ),
            Self::Server(status) => write!(
                f,
                "ClickUp is having trouble (HTTP {status}). Try again in a minute."
            ),
            Self::Refused(status) => write!(
                f,
                "ClickUp refused the change (HTTP {status}). Check that the status still exists in the ticket's list and the comment is not empty."
            ),
            Self::Unexpected(status) => write!(f, "ClickUp answered with HTTP {status}."),
            Self::Unreadable => f.write_str("ClickUp sent a reply DCTerminalv2 could not read."),
            Self::NoReply(NoReply::Timeout) => write!(
                f,
                "ClickUp did not answer within {} seconds. Try again.",
                REQUEST_TIMEOUT.as_secs()
            ),
            Self::NoReply(NoReply::Offline) => {
                f.write_str("Could not reach ClickUp. Check your connection.")
            }
            Self::NoReply(NoReply::Failed) => {
                f.write_str("The connection to ClickUp failed. Try again.")
            }
        }
    }
}

impl From<ClickUpError> for String {
    fn from(err: ClickUpError) -> Self {
        err.to_string()
    }
}

/// Time, so tests can run the rate-limit waits without sleeping.
pub trait Clock: Sync {
    /// Unix seconds.
    fn now(&self) -> i64;
    fn sleep(&self, secs: u64);
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        chrono::Utc::now().timestamp()
    }

    fn sleep(&self, secs: u64) {
        std::thread::sleep(Duration::from_secs(secs));
    }
}

/// What the last reply said about the rate limit.
#[derive(Debug, Default)]
struct Window {
    remaining: Option<u32>,
    reset_at: Option<i64>,
}

/// Which token a window belongs to: the first 8 bytes of the token's
/// SHA-256, so the token itself is not kept here. It is a lookup key only,
/// never logged or printed.
type TokenKey = [u8; 8];

fn token_key(token: &ClickUpToken) -> TokenKey {
    let digest = Sha256::digest(token.expose().as_bytes());
    let mut key = [0; 8];
    key.copy_from_slice(&digest[..8]);
    key
}

/// Shared by every call: the in-flight cap, app-wide, and one rate-limit
/// window per token, since ClickUp counts each token on its own.
pub struct Gate {
    in_flight: Mutex<usize>,
    freed: Condvar,
    windows: Mutex<HashMap<TokenKey, Window>>,
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

impl Gate {
    pub fn new() -> Self {
        Self {
            in_flight: Mutex::new(0),
            freed: Condvar::new(),
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// The app's one gate.
    pub fn shared() -> &'static Gate {
        static GATE: LazyLock<Gate> = LazyLock::new(Gate::new);
        &GATE
    }

    fn acquire(&self) -> Permit<'_> {
        let mut count = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        while *count >= MAX_IN_FLIGHT {
            count = self.freed.wait(count).unwrap_or_else(|e| e.into_inner());
        }
        *count += 1;
        Permit(self)
    }

    /// Seconds to wait before `key`'s next call, when it has no calls left.
    fn wait_needed(&self, key: &TokenKey, now: i64) -> u64 {
        let mut windows = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        let Some(window) = windows.get(key) else {
            return 0;
        };
        match (window.remaining, window.reset_at) {
            (Some(0), Some(reset)) if reset > now => secs_until(reset, now),
            (Some(0), _) => {
                // The reset has passed: forget the window until a reply says more.
                windows.remove(key);
                0
            }
            _ => 0,
        }
    }

    fn note(&self, key: &TokenKey, reply: &Reply, now: i64, wait_429: Option<u64>) {
        if reply.rate_remaining.is_none() && wait_429.is_none() {
            return;
        }
        let mut windows = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        let window = windows.entry(*key).or_default();
        if let Some(remaining) = reply.rate_remaining {
            window.remaining = Some(remaining);
            window.reset_at = reply.rate_reset;
        }
        if let Some(wait) = wait_429 {
            window.remaining = Some(0);
            window.reset_at = Some(now.saturating_add(clamp_wait(wait) as i64));
        }
    }
}

/// A header's wait, capped at [`MAX_HEADER_WAIT_SECS`].
fn clamp_wait(secs: u64) -> u64 {
    secs.min(MAX_HEADER_WAIT_SECS)
}

/// Whole seconds from `now` to `reset` (0 when it has passed), capped, so an
/// absurd reset time cannot overflow.
fn secs_until(reset: i64, now: i64) -> u64 {
    clamp_wait(u64::try_from(reset.saturating_sub(now)).unwrap_or(0))
}

struct Permit<'a>(&'a Gate);

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut count = self.0.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        *count = count.saturating_sub(1);
        self.0.freed.notify_one();
    }
}

/// How long a 429 asks us to wait: `Retry-After`, else the reset time, else
/// a short default.
fn wait_after_429(reply: &Reply, now: i64) -> u64 {
    reply
        .retry_after
        .map(clamp_wait)
        .or_else(|| reply.rate_reset.map(|reset| secs_until(reset, now).max(1)))
        .unwrap_or(DEFAULT_429_WAIT)
}

/// ClickUp answers 401 for an id the token cannot see, not only for a bad
/// token: a missing or foreign task or list comes back as `OAUTH_027` ("Team
/// not authorized"), a bad token as `OAUTH_025` ("Token invalid"). Seen live on
/// 2026-10-22. Only the `ECODE` field is read, never the body's text.
fn not_visible(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("ECODE")
                .and_then(|code| code.as_str())
                .map(|code| code == "OAUTH_027")
        })
        .unwrap_or(false)
}

/// A task by ClickUp's own id (`86b1aa001`, the usual case), or by a custom
/// task id (`QA-128`), which the API only finds with the workspace id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskRef {
    Id(String),
    Custom { id: String, team_id: String },
}

impl TaskRef {
    fn id(&self) -> &str {
        match self {
            Self::Id(id) | Self::Custom { id, .. } => id,
        }
    }

    /// `custom_task_ids=true&team_id=…` for a custom id, else nothing.
    fn custom_query(&self) -> Option<String> {
        match self {
            Self::Id(_) => None,
            Self::Custom { team_id, .. } => Some(format!("custom_task_ids=true&team_id={team_id}")),
        }
    }

    fn valid(&self) -> bool {
        match self {
            Self::Id(id) => valid_id(id),
            Self::Custom { id, team_id } => valid_id(id) && valid_id(team_id),
        }
    }
}

/// One request: a read, or (C3) a write with its JSON body.
#[derive(Clone, Copy)]
enum Call<'r> {
    Get(&'r str),
    Write(WriteMethod, &'r str, &'r str),
}

/// One token's access to ClickUp: the reads, and the two writes.
pub struct ClickUp<'a> {
    transport: &'a dyn Transport,
    token: &'a ClickUpToken,
    /// The token's rate-limit window in the gate.
    key: TokenKey,
    gate: &'a Gate,
    clock: &'a dyn Clock,
}

impl<'a> ClickUp<'a> {
    pub fn new(transport: &'a dyn Transport, token: &'a ClickUpToken) -> Self {
        Self::with(transport, token, Gate::shared(), &SystemClock)
    }

    pub fn with(
        transport: &'a dyn Transport,
        token: &'a ClickUpToken,
        gate: &'a Gate,
        clock: &'a dyn Clock,
    ) -> Self {
        Self {
            transport,
            token,
            key: token_key(token),
            gate,
            clock,
        }
    }

    /// GET `path` and read the JSON. `what` names the thing for a 404.
    fn get<T: DeserializeOwned>(&self, path: &str, what: &'static str) -> Result<T, ClickUpError> {
        let body = self.call(Call::Get(path), what)?;
        serde_json::from_str(&body).map_err(|_| ClickUpError::Unreadable)
    }

    /// One of the two writes (C3): `method` `path` with `body` as JSON. A
    /// 2xx is success whatever its body says.
    fn write(
        &self,
        method: WriteMethod,
        path: &str,
        body: &serde_json::Value,
        what: &'static str,
    ) -> Result<(), ClickUpError> {
        self.call(Call::Write(method, path, &body.to_string()), what)
            .map(|_| ())
    }

    /// Sends one call through the gate and returns a 2xx reply's body.
    fn call(&self, call: Call<'_>, what: &'static str) -> Result<String, ClickUpError> {
        let mut retries = 0;
        loop {
            let wait = self.gate.wait_needed(&self.key, self.clock.now());
            if wait > MAX_WAIT_SECS {
                return Err(ClickUpError::RateLimited(wait));
            }
            if wait > 0 {
                self.clock.sleep(wait);
            }
            let permit = self.gate.acquire();
            // Waiting for a permit takes time, and a call that finished
            // meanwhile may have spent the window: check again, and wait
            // without holding the permit.
            if self.gate.wait_needed(&self.key, self.clock.now()) > 0 {
                drop(permit);
                continue;
            }
            let reply = match call {
                Call::Get(path) => self.transport.get(path, self.token),
                Call::Write(method, path, body) => {
                    self.transport.send(method, path, body, self.token)
                }
            };
            drop(permit);
            let reply = reply.map_err(ClickUpError::NoReply)?;
            let now = self.clock.now();
            let wait_429 = (reply.status == 429).then(|| wait_after_429(&reply, now));
            self.gate.note(&self.key, &reply, now, wait_429);
            return match reply.status {
                200..=299 => Ok(reply.body),
                429 => {
                    let wait = wait_429.unwrap_or(DEFAULT_429_WAIT);
                    if retries < RETRIES_ON_429 && wait <= MAX_WAIT_SECS {
                        retries += 1;
                        // The gate now holds the reset, so the loop's wait sleeps it.
                        continue;
                    }
                    Err(ClickUpError::RateLimited(wait))
                }
                401 | 403 if not_visible(&reply.body) => Err(ClickUpError::NotFound(what)),
                401 | 403 => Err(ClickUpError::Rejected),
                404 => Err(ClickUpError::NotFound(what)),
                400 | 422 => Err(ClickUpError::Refused(reply.status)),
                500..=599 => Err(ClickUpError::Server(reply.status)),
                status => Err(ClickUpError::Unexpected(status)),
            };
        }
    }

    /// The workspaces this token can see (`GET /team`).
    pub fn teams(&self) -> Result<Vec<Team>, ClickUpError> {
        Ok(self.get::<Teams>("/team", "workspace")?.teams)
    }

    /// One task with its subtasks, closed ones included, and its Markdown
    /// description.
    pub fn task(&self, task: &TaskRef) -> Result<Task, ClickUpError> {
        if !task.valid() {
            return Err(ClickUpError::BadId);
        }
        let mut path = format!(
            "/task/{}?include_subtasks=true&include_markdown_description=true",
            task.id()
        );
        if let Some(custom) = task.custom_query() {
            path.push('&');
            path.push_str(&custom);
        }
        self.get(&path, "task")
    }

    /// A task's comments, newest first (ClickUp sends the latest 25).
    pub fn comments(&self, task: &TaskRef) -> Result<Vec<Comment>, ClickUpError> {
        if !task.valid() {
            return Err(ClickUpError::BadId);
        }
        let mut path = format!("/task/{}/comment", task.id());
        if let Some(custom) = task.custom_query() {
            path.push('?');
            path.push_str(&custom);
        }
        Ok(self.get::<Comments>(&path, "task")?.comments)
    }

    /// One page (from 0) of a list's tasks, subtasks and closed tasks
    /// included. ClickUp leaves closed tasks out unless asked, and intake
    /// lists them as excluded, so the caller filters.
    pub fn list_tasks_page(&self, list_id: &str, page: u32) -> Result<TaskPage, ClickUpError> {
        if !valid_id(list_id) {
            return Err(ClickUpError::BadId);
        }
        self.get(
            &format!("/list/{list_id}/task?page={page}&subtasks=true&include_closed=true"),
            "list",
        )
    }

    /// Every task in a list, page by page.
    pub fn list_tasks(&self, list_id: &str) -> Result<Vec<Task>, ClickUpError> {
        let mut tasks = Vec::new();
        for page in 0..MAX_PAGES {
            let reply = self.list_tasks_page(list_id, page)?;
            let count = reply.tasks.len();
            tasks.extend(reply.tasks);
            let last = reply.last_page.unwrap_or(count < PAGE_SIZE);
            if last || count == 0 {
                break;
            }
        }
        Ok(tasks)
    }

    /// A list and its statuses (a write-back status must be one of them).
    pub fn list(&self, list_id: &str) -> Result<List, ClickUpError> {
        if !valid_id(list_id) {
            return Err(ClickUpError::BadId);
        }
        self.get(&format!("/list/{list_id}"), "list")
    }

    /// `path` for `task`, with the custom-id query when it needs one.
    fn task_path(task: &TaskRef, tail: &str) -> String {
        let mut path = format!("/task/{}{tail}", task.id());
        if let Some(custom) = task.custom_query() {
            path.push('?');
            path.push_str(&custom);
        }
        path
    }

    /// Write 1 of 2 (C3): adds `text` as a comment on `task`
    /// (`POST /task/{id}/comment`), notifying nobody.
    pub fn post_comment(&self, task: &TaskRef, text: &str) -> Result<(), ClickUpError> {
        if !task.valid() {
            return Err(ClickUpError::BadId);
        }
        let body = serde_json::json!({ "comment_text": text, "notify_all": false });
        let path = Self::task_path(task, "/comment");
        self.write(WriteMethod::Post, &path, &body, "task")
    }

    /// Write 2 of 2 (C3): moves `task` to `status` (`PUT /task/{id}`). The
    /// body holds the status and nothing else.
    pub fn set_status(&self, task: &TaskRef, status: &str) -> Result<(), ClickUpError> {
        if !task.valid() {
            return Err(ClickUpError::BadId);
        }
        let body = serde_json::json!({ "status": status });
        let path = Self::task_path(task, "");
        self.write(WriteMethod::Put, &path, &body, "task")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

    pub(crate) fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures/clickup")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    pub(crate) fn ok(body: String) -> Result<Reply, NoReply> {
        Ok(Reply {
            status: 200,
            rate_remaining: Some(99),
            rate_reset: Some(1_790_000_060),
            body,
            ..Reply::default()
        })
    }

    pub(crate) fn status(status: u16) -> Result<Reply, NoReply> {
        Ok(Reply {
            status,
            body: "{\"err\":\"Token invalid\",\"ECODE\":\"OAUTH_025\"}".into(),
            ..Reply::default()
        })
    }

    /// One write a test transport was asked to send (C3).
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct SentWrite {
        pub method: &'static str,
        pub path: String,
        pub body: serde_json::Value,
        pub auth: String,
    }

    /// Recorded replies by path, served in order; every request is logged
    /// with the `Authorization` value it carried. A write's reply is keyed
    /// `"POST /path"` or `"PUT /path"`, and it is logged in `calls` under
    /// that key and in `writes` with its body.
    #[derive(Default)]
    pub(crate) struct Recorded {
        pub replies: Mutex<HashMap<String, VecDeque<Result<Reply, NoReply>>>>,
        pub calls: Mutex<Vec<(String, String)>>,
        pub writes: Mutex<Vec<SentWrite>>,
    }

    impl Recorded {
        pub(crate) fn on(self, path: &str, reply: Result<Reply, NoReply>) -> Self {
            self.replies
                .lock()
                .unwrap()
                .entry(path.to_string())
                .or_default()
                .push_back(reply);
            self
        }

        pub(crate) fn paths(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|(p, _)| p.clone())
                .collect()
        }
    }

    impl Transport for Recorded {
        fn get(&self, path: &str, token: &ClickUpToken) -> Result<Reply, NoReply> {
            self.calls
                .lock()
                .unwrap()
                .push((path.to_string(), token.expose().to_string()));
            self.replies
                .lock()
                .unwrap()
                .get_mut(path)
                .and_then(VecDeque::pop_front)
                .unwrap_or_else(|| panic!("no recorded reply for {path}"))
        }

        fn send(
            &self,
            method: WriteMethod,
            path: &str,
            body: &str,
            token: &ClickUpToken,
        ) -> Result<Reply, NoReply> {
            let key = format!("{} {path}", method.as_str());
            self.writes.lock().unwrap().push(SentWrite {
                method: method.as_str(),
                path: path.to_string(),
                body: serde_json::from_str(body).expect("a write's body is JSON"),
                auth: token.expose().to_string(),
            });
            self.get(&key, token)
        }
    }

    /// A clock that only moves when slept.
    pub(crate) struct FakeClock {
        pub now: AtomicI64,
        pub slept: Mutex<Vec<u64>>,
    }

    impl FakeClock {
        pub(crate) fn at(now: i64) -> Self {
            Self {
                now: AtomicI64::new(now),
                slept: Mutex::new(Vec::new()),
            }
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> i64 {
            self.now.load(Ordering::SeqCst)
        }

        fn sleep(&self, secs: u64) {
            self.slept.lock().unwrap().push(secs);
            self.now.fetch_add(secs as i64, Ordering::SeqCst);
        }
    }

    const NOW: i64 = 1_790_000_000;

    fn token() -> ClickUpToken {
        ClickUpToken::parse("pk_test_TOKEN").unwrap()
    }

    #[test]
    fn reads_workspaces_and_sends_the_token_as_the_authorization_header() {
        let api = Recorded::default().on("/team", ok(fixture("teams.json")));
        let (gate, clock, token) = (Gate::new(), FakeClock::at(NOW), token());
        let teams = ClickUp::with(&api, &token, &gate, &clock).teams().unwrap();
        assert_eq!(
            teams,
            vec![
                Team {
                    id: "9012345678".into(),
                    name: "Koneksi QA".into()
                },
                Team {
                    id: "9087654321".into(),
                    name: "Sandbox".into()
                },
            ]
        );
        assert_eq!(
            *api.calls.lock().unwrap(),
            vec![("/team".to_string(), "pk_test_TOKEN".to_string())]
        );
    }

    #[test]
    fn reads_a_task_with_every_subtask_and_its_markdown_description() {
        let path = "/task/7xk2epic01?include_subtasks=true&include_markdown_description=true";
        let api = Recorded::default().on(path, ok(fixture("task-with-subtasks.json")));
        let (gate, clock, token) = (Gate::new(), FakeClock::at(NOW), token());
        let task = ClickUp::with(&api, &token, &gate, &clock)
            .task(&TaskRef::Id("7xk2epic01".into()))
            .unwrap();
        assert_eq!(task.id, "7xk2epic01");
        assert_eq!(task.custom_id, None);
        assert_eq!(task.status.status, "in progress");
        assert!(task.description().contains("**medium**"));
        // Closed subtasks come back too.
        assert_eq!(task.subtasks.len(), 3);
        let closed = &task.subtasks[2];
        assert!(closed.status.is_closed());
        assert_eq!(closed.priority, None);
        assert_eq!(task.subtasks[0].priority.as_ref().unwrap().priority, "high");
        assert_eq!(task.subtasks[0].tags[0].name, "desktop");
        assert_eq!(task.subtasks[0].parent.as_deref(), Some("7xk2epic01"));
        assert_eq!(task.subtasks[0].custom_id.as_deref(), Some("QA-129"));
        // No Markdown description: the plain text one.
        assert_eq!(
            task.subtasks[0].description(),
            "Upload of a large folder stops at 61% on Windows."
        );
        assert_eq!(task.list.id, "901800112233");
    }

    #[test]
    fn a_custom_task_id_is_looked_up_with_the_workspace() {
        let path = "/task/QA-129?include_subtasks=true&include_markdown_description=true&custom_task_ids=true&team_id=9012345678";
        let comments = "/task/QA-129/comment?custom_task_ids=true&team_id=9012345678";
        let api = Recorded::default()
            .on(
                path,
                ok(r#"{"id":"86b1aa001","custom_id":"QA-129","name":"Sync"}"#.into()),
            )
            .on(comments, ok(r#"{"comments":[]}"#.into()));
        let (gate, clock, token) = (Gate::new(), FakeClock::at(NOW), token());
        let client = ClickUp::with(&api, &token, &gate, &clock);
        let task = TaskRef::Custom {
            id: "QA-129".into(),
            team_id: "9012345678".into(),
        };
        assert_eq!(client.task(&task).unwrap().id, "86b1aa001");
        assert!(client.comments(&task).unwrap().is_empty());
    }

    #[test]
    fn reads_comments_and_list_statuses() {
        let api = Recorded::default()
            .on("/task/86b1aa001/comment", ok(fixture("comments.json")))
            .on("/list/901800112233", ok(fixture("list.json")));
        let (gate, clock, token) = (Gate::new(), FakeClock::at(NOW), token());
        let client = ClickUp::with(&api, &token, &gate, &clock);
        let comments = client.comments(&TaskRef::Id("86b1aa001".into())).unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].user.username, "Dana Reyes");
        assert_eq!(comments[0].user.id, "81234567");
        assert_eq!(comments[0].user.email, "dana@example.com");
        assert_eq!(comments[0].date, "1789950000000");
        assert!(comments[0].comment_text.contains("7,839 files"));
        let list = client.list("901800112233").unwrap();
        let names: Vec<&str> = list.statuses.iter().map(|s| s.status.as_str()).collect();
        assert_eq!(
            names,
            ["to do", "in progress", "qa failed", "canceled", "complete"]
        );
        let kinds: Vec<&str> = list.statuses.iter().map(|s| s.kind.as_str()).collect();
        assert_eq!(kinds, ["open", "custom", "custom", "done", "closed"]);
        assert!(list.statuses[3].is_closed() && list.statuses[4].is_closed());
    }

    #[test]
    fn pages_through_a_list_until_the_last_page() {
        let api = Recorded::default()
            .on(
                "/list/901800112233/task?page=0&subtasks=true&include_closed=true",
                ok(fixture("list-tasks-page0.json")),
            )
            .on(
                "/list/901800112233/task?page=1&subtasks=true&include_closed=true",
                ok(fixture("list-tasks-page1.json")),
            );
        let (gate, clock, token) = (Gate::new(), FakeClock::at(NOW), token());
        let tasks = ClickUp::with(&api, &token, &gate, &clock)
            .list_tasks("901800112233")
            .unwrap();
        let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["86b1aa001", "86b1aa002", "86b1aa003"]);
        assert_eq!(api.paths().len(), 2);
    }

    #[test]
    fn a_short_page_without_last_page_ends_the_list() {
        let api = Recorded::default().on(
            "/list/901800112233/task?page=0&subtasks=true&include_closed=true",
            ok(r#"{"tasks":[{"id":"a1","name":"One"}]}"#.into()),
        );
        let (gate, clock, token) = (Gate::new(), FakeClock::at(NOW), token());
        let tasks = ClickUp::with(&api, &token, &gate, &clock)
            .list_tasks("901800112233")
            .unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, Default::default());
    }

    #[test]
    fn errors_are_plain_english() {
        let cases = [
            (
                status(401),
                "ClickUp rejected the token. Check it under Settings › ClickUp.",
            ),
            (
                status(403),
                "ClickUp rejected the token. Check it under Settings › ClickUp.",
            ),
            (
                status(404),
                "ClickUp has no task with that id, or your token cannot see it.",
            ),
            (
                Ok(Reply {
                    status: 401,
                    body: "{\"err\":\"Team not authorized\",\"ECODE\":\"OAUTH_027\"}".into(),
                    ..Reply::default()
                }),
                "ClickUp has no task with that id, or your token cannot see it.",
            ),
            (
                Ok(Reply {
                    status: 401,
                    body: "not json".into(),
                    ..Reply::default()
                }),
                "ClickUp rejected the token. Check it under Settings › ClickUp.",
            ),
            (
                status(502),
                "ClickUp is having trouble (HTTP 502). Try again in a minute.",
            ),
            (status(418), "ClickUp answered with HTTP 418."),
            (
                Err(NoReply::Offline),
                "Could not reach ClickUp. Check your connection.",
            ),
            (
                Err(NoReply::Timeout),
                "ClickUp did not answer within 25 seconds. Try again.",
            ),
            (
                ok("not json".into()),
                "ClickUp sent a reply DCTerminalv2 could not read.",
            ),
        ];
        for (reply, message) in cases {
            let path = "/task/abc?include_subtasks=true&include_markdown_description=true";
            let api = Recorded::default().on(path, reply);
            let (gate, clock, token) = (Gate::new(), FakeClock::at(NOW), token());
            let err = ClickUp::with(&api, &token, &gate, &clock)
                .task(&TaskRef::Id("abc".into()))
                .unwrap_err();
            assert_eq!(err.to_string(), message);
        }
    }

    #[test]
    fn ids_that_could_change_the_path_are_refused_before_any_call() {
        let api = Recorded::default();
        let (gate, clock, token) = (Gate::new(), FakeClock::at(NOW), token());
        let client = ClickUp::with(&api, &token, &gate, &clock);
        assert_eq!(
            client.task(&TaskRef::Id("a/../team".into())),
            Err(ClickUpError::BadId)
        );
        assert_eq!(
            client.comments(&TaskRef::Custom {
                id: "QA-1".into(),
                team_id: "1&x=2".into()
            }),
            Err(ClickUpError::BadId)
        );
        assert_eq!(client.list("x?y").unwrap_err(), ClickUpError::BadId);
        assert!(api.paths().is_empty());
    }

    #[test]
    fn a_429_waits_for_the_reset_and_retries() {
        let limited = Ok(Reply {
            status: 429,
            rate_remaining: Some(0),
            rate_reset: Some(NOW + 7),
            ..Reply::default()
        });
        let api = Recorded::default()
            .on("/team", limited)
            .on("/team", ok(fixture("teams.json")));
        let (gate, clock, token) = (Gate::new(), FakeClock::at(NOW), token());
        let teams = ClickUp::with(&api, &token, &gate, &clock).teams().unwrap();
        assert_eq!(teams.len(), 2);
        assert_eq!(*clock.slept.lock().unwrap(), vec![7]);
    }

    #[test]
    fn retry_after_wins_and_repeated_429s_give_up() {
        let limited = || {
            Ok(Reply {
                status: 429,
                retry_after: Some(3),
                ..Reply::default()
            })
        };
        let api = Recorded::default()
            .on("/team", limited())
            .on("/team", limited())
            .on("/team", limited());
        let (gate, clock, token) = (Gate::new(), FakeClock::at(NOW), token());
        let err = ClickUp::with(&api, &token, &gate, &clock)
            .teams()
            .unwrap_err();
        assert_eq!(err, ClickUpError::RateLimited(3));
        assert_eq!(*clock.slept.lock().unwrap(), vec![3, 3]);
        assert_eq!(api.paths().len(), 3);
    }

    #[test]
    fn no_calls_left_waits_for_the_window_and_a_long_wait_is_an_error() {
        let spent = Ok(Reply {
            status: 200,
            rate_remaining: Some(0),
            rate_reset: Some(NOW + 20),
            body: fixture("teams.json"),
            ..Reply::default()
        });
        let api = Recorded::default()
            .on("/team", spent)
            .on("/team", ok(fixture("teams.json")));
        let (gate, clock, token) = (Gate::new(), FakeClock::at(NOW), token());
        let client = ClickUp::with(&api, &token, &gate, &clock);
        client.teams().unwrap();
        client.teams().unwrap();
        assert_eq!(*clock.slept.lock().unwrap(), vec![20]);

        let far = Gate::new();
        far.note(
            &token_key(&token),
            &Reply {
                rate_remaining: Some(0),
                rate_reset: Some(NOW + 600),
                ..Reply::default()
            },
            NOW,
            None,
        );
        let idle = Recorded::default();
        let err = ClickUp::with(&idle, &token, &far, &FakeClock::at(NOW))
            .teams()
            .unwrap_err();
        assert_eq!(err, ClickUpError::RateLimited(600));
        assert!(idle.paths().is_empty(), "no call is spent on a known 429");
    }

    fn spent(reset: i64) -> Reply {
        Reply {
            status: 200,
            rate_remaining: Some(0),
            rate_reset: Some(reset),
            body: r#"{"teams":[]}"#.into(),
            ..Reply::default()
        }
    }

    #[test]
    fn one_tokens_spent_limit_does_not_delay_another() {
        let (a, b) = (token(), ClickUpToken::parse("pk_test_OTHER").unwrap());
        let api = Recorded::default()
            .on("/team", Ok(spent(NOW + 30)))
            .on("/team", ok(r#"{"teams":[]}"#.into()))
            .on("/team", ok(r#"{"teams":[]}"#.into()));
        let (gate, clock) = (Gate::new(), FakeClock::at(NOW));
        ClickUp::with(&api, &a, &gate, &clock).teams().unwrap();
        assert_eq!(gate.wait_needed(&token_key(&a), NOW), 30);
        // B has its own window: no wait, and its reply leaves A's alone.
        ClickUp::with(&api, &b, &gate, &clock).teams().unwrap();
        assert!(clock.slept.lock().unwrap().is_empty());
        assert_eq!(gate.wait_needed(&token_key(&b), NOW), 0);
        assert_eq!(gate.wait_needed(&token_key(&a), NOW), 30);
        // A still waits for its own reset.
        ClickUp::with(&api, &a, &gate, &clock).teams().unwrap();
        assert_eq!(*clock.slept.lock().unwrap(), vec![30]);
        let auth: Vec<String> = api
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.1.clone())
            .collect();
        assert_eq!(auth, ["pk_test_TOKEN", "pk_test_OTHER", "pk_test_TOKEN"]);
    }

    #[test]
    fn absurd_rate_limit_headers_do_not_overflow() {
        let key = token_key(&token());
        let gate = Gate::new();
        gate.note(&key, &spent(i64::MAX), i64::MIN + 1, None);
        assert_eq!(gate.wait_needed(&key, i64::MIN + 1), MAX_HEADER_WAIT_SECS);
        gate.note(&key, &Reply::default(), i64::MAX - 1, Some(u64::MAX));
        assert_eq!(gate.wait_needed(&key, i64::MAX - 2), 2);
        let reply = Reply {
            retry_after: Some(u64::MAX),
            ..Reply::default()
        };
        assert_eq!(wait_after_429(&reply, NOW), MAX_HEADER_WAIT_SECS);
        let reply = Reply {
            rate_reset: Some(i64::MIN),
            ..Reply::default()
        };
        assert_eq!(wait_after_429(&reply, i64::MAX), 1);
    }

    /// A clock whose second reading comes after another call spent the
    /// window, as happens while a call waits for a permit.
    struct SpentWhileQueued<'a> {
        inner: FakeClock,
        gate: &'a Gate,
        key: TokenKey,
        reads: AtomicUsize,
    }

    impl Clock for SpentWhileQueued<'_> {
        fn now(&self) -> i64 {
            if self.reads.fetch_add(1, Ordering::SeqCst) == 1 {
                self.gate.note(&self.key, &spent(NOW + 5), NOW, None);
            }
            self.inner.now()
        }

        fn sleep(&self, secs: u64) {
            self.inner.sleep(secs);
        }
    }

    #[test]
    fn a_call_that_waited_for_a_permit_checks_the_window_again() {
        let token = token();
        let gate = Gate::new();
        let clock = SpentWhileQueued {
            inner: FakeClock::at(NOW),
            gate: &gate,
            key: token_key(&token),
            reads: AtomicUsize::new(0),
        };
        let api = Recorded::default().on("/team", ok(r#"{"teams":[]}"#.into()));
        ClickUp::with(&api, &token, &gate, &clock).teams().unwrap();
        // It waited for the reset instead of sending into a spent window.
        assert_eq!(*clock.inner.slept.lock().unwrap(), vec![5]);
        assert_eq!(api.paths().len(), 1);
    }

    #[test]
    fn the_two_writes_send_their_paths_and_bodies_with_the_token_header() {
        let api = Recorded::default()
            .on(
                "POST /task/86b1aa001/comment",
                ok(r#"{"id":"458","date":1790000000000}"#.into()),
            )
            .on("PUT /task/86b1aa001", ok(String::new()))
            .on(
                "POST /task/QA-129/comment?custom_task_ids=true&team_id=9012345678",
                ok("{}".into()),
            );
        let (gate, clock, token) = (Gate::new(), FakeClock::at(NOW), token());
        let client = ClickUp::with(&api, &token, &gate, &clock);
        let task = TaskRef::Id("86b1aa001".into());
        client.post_comment(&task, "Fixed in PR #12.").unwrap();
        // A 2xx is success, whatever its body.
        client.set_status(&task, "QA PASSED").unwrap();
        let custom = TaskRef::Custom {
            id: "QA-129".into(),
            team_id: "9012345678".into(),
        };
        client.post_comment(&custom, "x").unwrap();
        let writes = api.writes.lock().unwrap().clone();
        assert_eq!(
            writes,
            vec![
                SentWrite {
                    method: "POST",
                    path: "/task/86b1aa001/comment".into(),
                    body: serde_json::json!({ "comment_text": "Fixed in PR #12.", "notify_all": false }),
                    auth: "pk_test_TOKEN".into(),
                },
                SentWrite {
                    method: "PUT",
                    path: "/task/86b1aa001".into(),
                    body: serde_json::json!({ "status": "QA PASSED" }),
                    auth: "pk_test_TOKEN".into(),
                },
                SentWrite {
                    method: "POST",
                    path: "/task/QA-129/comment?custom_task_ids=true&team_id=9012345678".into(),
                    body: serde_json::json!({ "comment_text": "x", "notify_all": false }),
                    auth: "pk_test_TOKEN".into(),
                },
            ]
        );
    }

    #[test]
    fn writes_share_the_error_mapping_the_rate_limit_and_the_id_check() {
        let limited = Ok(Reply {
            status: 429,
            retry_after: Some(4),
            ..Reply::default()
        });
        let refused = Ok(Reply {
            status: 400,
            body: r#"{"err":"Status does not exist","ECODE":"ITEM_114"}"#.into(),
            ..Reply::default()
        });
        let api = Recorded::default()
            .on("PUT /task/abc", limited)
            .on("PUT /task/abc", ok("{}".into()))
            .on("PUT /task/abc", refused)
            .on("POST /task/abc/comment", status(401))
            .on("POST /task/abc/comment", Err(NoReply::Timeout));
        let (gate, clock, token) = (Gate::new(), FakeClock::at(NOW), token());
        let client = ClickUp::with(&api, &token, &gate, &clock);
        let task = TaskRef::Id("abc".into());
        client.set_status(&task, "done").unwrap();
        assert_eq!(
            *clock.slept.lock().unwrap(),
            vec![4],
            "the 429 waited, then retried"
        );
        let err = client.set_status(&task, "nope").unwrap_err();
        assert_eq!(err, ClickUpError::Refused(400));
        assert!(
            !err.to_string().contains("ITEM_114") && !err.to_string().contains("does not exist")
        );
        assert_eq!(
            client.post_comment(&task, "x").unwrap_err().to_string(),
            "ClickUp rejected the token. Check it under Settings › ClickUp."
        );
        assert_eq!(
            client.post_comment(&task, "x").unwrap_err(),
            ClickUpError::NoReply(NoReply::Timeout)
        );
        let before = api.writes.lock().unwrap().len();
        assert_eq!(
            client.post_comment(&TaskRef::Id("a/../team".into()), "x"),
            Err(ClickUpError::BadId)
        );
        assert_eq!(
            client.set_status(&TaskRef::Id("a?b".into()), "x"),
            Err(ClickUpError::BadId)
        );
        assert_eq!(
            api.writes.lock().unwrap().len(),
            before,
            "no call for a bad id"
        );
    }

    /// The client's code, without its tests.
    fn client_source() -> &'static str {
        let text = include_str!("client.rs");
        let end = text.find("#[cfg(test)]").expect("the test module");
        &text[..end]
    }

    #[test]
    fn the_client_has_exactly_two_writes_and_nothing_else_reaches_send() {
        let source = client_source();
        // Every write goes through `self.write(`, and there are two.
        let writes: Vec<&str> = source
            .lines()
            .filter(|line| line.contains("self.write("))
            .map(str::trim)
            .collect();
        assert_eq!(
            writes,
            [
                r#"self.write(WriteMethod::Post, &path, &body, "task")"#,
                r#"self.write(WriteMethod::Put, &path, &body, "task")"#,
            ]
        );
        // Only `call` reaches the transport's send, and only through `write`.
        assert_eq!(source.matches("transport.send(").count(), 1);
        assert_eq!(
            source.matches("Call::Write(").count(),
            2,
            "the match arm and `write`"
        );
        for banned in [
            "Delete",
            "Patch",
            "assignees",
            "description\":",
            "/attachment",
            "/checklist",
        ] {
            assert!(!source.contains(banned), "{banned} in client.rs");
        }
        // Every method that sends: reads are GETs, the rest are these two.
        let api = Recorded::default()
            .on("/team", ok(r#"{"teams":[]}"#.into()))
            .on(
                "/task/t1?include_subtasks=true&include_markdown_description=true",
                ok(r#"{"id":"t1"}"#.into()),
            )
            .on("/task/t1/comment", ok(r#"{"comments":[]}"#.into()))
            .on("/list/l1", ok(r#"{"id":"l1"}"#.into()))
            .on(
                "/list/l1/task?page=0&subtasks=true&include_closed=true",
                ok(r#"{"tasks":[]}"#.into()),
            )
            .on("POST /task/t1/comment", ok("{}".into()))
            .on("PUT /task/t1", ok("{}".into()));
        let (gate, clock, token) = (Gate::new(), FakeClock::at(NOW), token());
        let client = ClickUp::with(&api, &token, &gate, &clock);
        let task = TaskRef::Id("t1".into());
        client.teams().unwrap();
        client.task(&task).unwrap();
        client.comments(&task).unwrap();
        client.list("l1").unwrap();
        client.list_tasks("l1").unwrap();
        client.post_comment(&task, "c").unwrap();
        client.set_status(&task, "s").unwrap();
        let writes: Vec<(String, String)> = api
            .writes
            .lock()
            .unwrap()
            .iter()
            .map(|w| (w.method.to_string(), w.path.clone()))
            .collect();
        assert_eq!(
            writes,
            [
                ("POST".to_string(), "/task/t1/comment".to_string()),
                ("PUT".to_string(), "/task/t1".to_string()),
            ]
        );
        assert_eq!(api.paths().len(), 7);
    }

    /// Counts calls in flight; each one takes a moment.
    struct Slow {
        now: AtomicUsize,
        peak: AtomicUsize,
    }

    impl Transport for Slow {
        fn get(&self, _path: &str, _token: &ClickUpToken) -> Result<Reply, NoReply> {
            let now = self.now.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(30));
            self.now.fetch_sub(1, Ordering::SeqCst);
            ok(r#"{"teams":[]}"#.into())
        }

        fn send(
            &self,
            _method: WriteMethod,
            _path: &str,
            _body: &str,
            _token: &ClickUpToken,
        ) -> Result<Reply, NoReply> {
            Err(NoReply::Failed)
        }
    }

    #[test]
    fn at_most_four_calls_are_in_flight() {
        let api = Slow {
            now: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        };
        let (gate, clock, token) = (Gate::new(), FakeClock::at(NOW), token());
        let client = ClickUp::with(&api, &token, &gate, &clock);
        std::thread::scope(|scope| {
            for _ in 0..12 {
                scope.spawn(|| client.teams().unwrap());
            }
        });
        let peak = api.peak.load(Ordering::SeqCst);
        assert!(peak <= MAX_IN_FLIGHT, "peak {peak}");
        assert!(peak > 1, "calls ran one at a time");
    }
}
