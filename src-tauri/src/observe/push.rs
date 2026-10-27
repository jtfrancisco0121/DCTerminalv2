//! Phone pushes for AFK runs: ntfy, Pushover or a webhook. A push carries
//! the run id and the event kind and nothing else (no ticket text, no
//! paths, no tab names). The ntfy topic, the Pushover token and user key
//! and the webhook URL are secrets in the keychain (service
//! `com.jtfrancisco.dcterminalv2.push`); settings.json holds none of them.
//!
//! Sending is on a worker thread of its own, fed by a bounded channel: a
//! slow or failing push never delays the detector tick. One call takes at
//! most [`TIMEOUT`], with at most one retry; failures are logged.

use super::attention::Event;
use crate::store::settings_store::PushSettings;
use serde::Serialize;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::time::Duration;

pub const KEYCHAIN_SERVICE: &str = "com.jtfrancisco.dcterminalv2.push";
/// One call may take this long.
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// A pause before the one retry.
const RETRY_AFTER: Duration = Duration::from_secs(2);
/// The events that go to the phone: the ones that matter when nobody is
/// at the Mac.
pub const PUSH_KINDS: [&str; 4] = ["stall", "check-failed", "merged-cleaned", "autopost-failed"];
/// The secret slots (the item's account field in the keychain).
pub const SECRET_NAMES: [&str; 4] = [
    "ntfy-topic",
    "pushover-token",
    "pushover-user",
    "webhook-url",
];

/// What a push says: exactly these two fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Payload {
    pub run: String,
    pub event: String,
}

impl Payload {
    /// The push for `event`, when its kind is one the phone gets and the
    /// settings have it on.
    pub fn for_event(event: &Event, settings: &PushSettings) -> Option<Payload> {
        let kind = event.kind.key();
        let on = settings.target != "none"
            && PUSH_KINDS.contains(&kind)
            && settings.events.get(kind).copied().unwrap_or(true);
        on.then(|| Payload {
            run: event.run_id.clone().unwrap_or_default(),
            event: kind.to_string(),
        })
    }

    fn line(&self) -> String {
        format!("{}: {}", self.event, self.run)
    }
}

/// An http(s) URL for a push target: https always; http only for this
/// machine (a test server), so nothing travels in the clear.
pub fn check_url(url: &str) -> Result<(), String> {
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("The address cannot contain spaces or control characters.".into());
    }
    let rest = if let Some(rest) = url.strip_prefix("https://") {
        rest
    } else if let Some(rest) = url.strip_prefix("http://") {
        let host = rest.split(['/', ':']).next().unwrap_or("");
        if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
            return Err("Use an https:// address (http:// only for this machine).".into());
        }
        rest
    } else {
        return Err("The address must start with https://".into());
    };
    if rest.is_empty() || rest.starts_with('/') {
        return Err("The address has no host.".into());
    }
    Ok(())
}

/// One HTTP POST, built from the payload and the secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// The secrets of the configured target, read from the keychain.
#[derive(Debug, Clone, Default)]
pub struct Secrets {
    pub ntfy_topic: Option<String>,
    pub pushover_token: Option<String>,
    pub pushover_user: Option<String>,
    pub webhook_url: Option<String>,
}

fn form_escape(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The request for `payload`. Bodies hold the two payload fields only.
pub fn build(
    settings: &PushSettings,
    secrets: &Secrets,
    payload: &Payload,
) -> Result<Request, String> {
    let missing = |what: &str| format!("Set the {what} first (Settings > Phone push).");
    match settings.target.as_str() {
        "ntfy" => {
            let topic = secrets
                .ntfy_topic
                .as_deref()
                .ok_or_else(|| missing("ntfy topic"))?;
            let url = format!("{}/{}", settings.ntfy_server.trim_end_matches('/'), topic);
            check_url(&url)?;
            Ok(Request {
                url,
                headers: vec![
                    ("Title".into(), "DCTerminalv2".into()),
                    ("Content-Type".into(), "text/plain".into()),
                ],
                body: payload.line(),
            })
        }
        "pushover" => {
            let token = secrets
                .pushover_token
                .as_deref()
                .ok_or_else(|| missing("Pushover token"))?;
            let user = secrets
                .pushover_user
                .as_deref()
                .ok_or_else(|| missing("Pushover user key"))?;
            Ok(Request {
                url: PUSHOVER_URL.to_string(),
                headers: vec![(
                    "Content-Type".into(),
                    "application/x-www-form-urlencoded".into(),
                )],
                body: format!(
                    "token={}&user={}&title=DCTerminalv2&message={}",
                    form_escape(token),
                    form_escape(user),
                    form_escape(&payload.line())
                ),
            })
        }
        "webhook" => {
            let url = secrets
                .webhook_url
                .clone()
                .ok_or_else(|| missing("webhook URL"))?;
            check_url(&url)?;
            Ok(Request {
                url,
                headers: vec![("Content-Type".into(), "application/json".into())],
                body: serde_json::to_string(payload).map_err(|e| e.to_string())?,
            })
        }
        _ => Err("Phone push is off.".into()),
    }
}

pub const PUSHOVER_URL: &str = "https://api.pushover.net/1/messages.json";

/// Sends one request. `Ok` for a 2xx reply.
pub trait Transport: Send + Sync {
    fn post(&self, request: &Request) -> Result<(), String>;
}

/// The real transport: ureq with rustls, no redirects.
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new(timeout: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent(concat!("DCTerminalv2/", env!("CARGO_PKG_VERSION")))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

impl Transport for UreqTransport {
    fn post(&self, request: &Request) -> Result<(), String> {
        let mut call = self.agent.post(&request.url);
        for (name, value) in &request.headers {
            call = call.header(name, value);
        }
        match call.send(request.body.as_str()) {
            Ok(reply) if reply.status().is_success() => Ok(()),
            // The address is a secret: only the status is reported.
            Ok(reply) => Err(format!("the server answered {}", reply.status().as_u16())),
            Err(ureq::Error::Timeout(_)) => Err("the server did not answer in time".into()),
            Err(_) => Err("the server could not be reached".into()),
        }
    }
}

/// Where the secrets are kept: the keychain; memory in tests.
pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<String>, String>;
    fn set(&self, name: &str, value: &str) -> Result<(), String>;
    fn clear(&self, name: &str) -> Result<(), String>;
}

pub struct Keychain;

fn entry(name: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, name).map_err(|_| keychain_error())
}

fn keychain_error() -> String {
    "Could not use the system keychain for the phone push settings.".to_string()
}

impl SecretStore for Keychain {
    fn get(&self, name: &str) -> Result<Option<String>, String> {
        match entry(name)?.get_password() {
            Ok(saved) => Ok(Some(saved)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(keychain_error()),
        }
    }

    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        entry(name)?
            .set_password(value)
            .map_err(|_| keychain_error())
    }

    fn clear(&self, name: &str) -> Result<(), String> {
        match entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(keychain_error()),
        }
    }
}

pub fn read_secrets(store: &dyn SecretStore) -> Result<Secrets, String> {
    Ok(Secrets {
        ntfy_topic: store.get("ntfy-topic")?,
        pushover_token: store.get("pushover-token")?,
        pushover_user: store.get("pushover-user")?,
        webhook_url: store.get("webhook-url")?,
    })
}

/// Which secrets are saved (never their values), for Settings.
pub fn saved_names(store: &dyn SecretStore) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for name in SECRET_NAMES {
        if store.get(name)?.is_some() {
            out.push(name.to_string());
        }
    }
    Ok(out)
}

struct Job {
    settings: PushSettings,
    payload: Payload,
    /// Set by "Send test": the result goes back.
    reply: Option<SyncSender<Result<(), String>>>,
}

/// The queue in front of the worker thread. Cheap to share.
pub struct PushClient {
    tx: SyncSender<Job>,
}

/// One send with at most one retry.
fn send_with_retry(
    transport: &dyn Transport,
    request: &Request,
    pause: Duration,
) -> Result<(), String> {
    match transport.post(request) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::thread::sleep(pause);
            transport.post(request)
        }
    }
}

fn run_job(
    job: &Job,
    transport: &dyn Transport,
    secrets: &dyn SecretStore,
    pause: Duration,
) -> Result<(), String> {
    let secrets = read_secrets(secrets)?;
    let request = build(&job.settings, &secrets, &job.payload)?;
    send_with_retry(transport, &request, pause)
}

impl PushClient {
    /// Starts the worker thread.
    pub fn start(
        transport: Arc<dyn Transport>,
        secrets: Arc<dyn SecretStore>,
        retry_pause: Duration,
    ) -> Self {
        let (tx, rx): (SyncSender<Job>, Receiver<Job>) = sync_channel(32);
        let _ = std::thread::Builder::new()
            .name("dcv2-push".into())
            .spawn(move || {
                for job in rx {
                    let result = run_job(&job, transport.as_ref(), secrets.as_ref(), retry_pause);
                    match (&job.reply, result) {
                        (Some(reply), result) => {
                            let _ = reply.send(result);
                        }
                        (None, Err(err)) => eprintln!("DCTerminalv2: phone push failed: {err}"),
                        (None, Ok(())) => {}
                    }
                }
            });
        Self { tx }
    }

    pub fn real() -> Self {
        Self::start(
            Arc::new(UreqTransport::new(TIMEOUT)),
            Arc::new(Keychain),
            RETRY_AFTER,
        )
    }

    /// Queues the push for `event`; never blocks (a full queue drops it).
    pub fn push(&self, event: &Event, settings: &PushSettings) {
        if let Some(payload) = Payload::for_event(event, settings) {
            let _ = self.tx.try_send(Job {
                settings: settings.clone(),
                payload,
                reply: None,
            });
        }
    }

    /// "Send test": one push, and its result. Blocks the caller (a worker
    /// thread of the command), not the detector.
    pub fn test(&self, settings: &PushSettings) -> Result<(), String> {
        let (reply, answer) = sync_channel(1);
        let payload = Payload {
            run: "test".into(),
            event: "test".into(),
        };
        self.tx
            .try_send(Job {
                settings: settings.clone(),
                payload,
                reply: Some(reply),
            })
            .map_err(|_| "Phone push is busy; try again.".to_string())?;
        answer
            .recv_timeout(TIMEOUT * 2 + RETRY_AFTER * 2)
            .map_err(|_| "No answer from the push worker.".to_string())?
    }
}

// ---- commands ---------------------------------------------------------------

use crate::store::SettingsStore;
use std::sync::Mutex;
use tauri::{Manager, State};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushStatus {
    /// Which secrets are saved (names only).
    pub saved: Vec<String>,
}

#[tauri::command]
pub async fn push_status() -> Result<PushStatus, String> {
    tauri::async_runtime::spawn_blocking(|| {
        Ok(PushStatus {
            saved: saved_names(&Keychain)?,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Saves one secret in the keychain (an empty value clears it).
#[tauri::command]
pub async fn push_set_secret(name: String, value: String) -> Result<PushStatus, String> {
    if !SECRET_NAMES.contains(&name.as_str()) {
        return Err("unknown push secret".into());
    }
    let value = value.trim().to_string();
    if value.chars().any(char::is_control) {
        return Err("The value cannot contain control characters.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        if value.is_empty() {
            Keychain.clear(&name)?;
        } else {
            Keychain.set(&name, &value)?;
        }
        Ok(PushStatus {
            saved: saved_names(&Keychain)?,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// "Send test": one push; Ok when the server accepted it.
#[tauri::command]
pub async fn push_send_test(
    app: tauri::AppHandle,
    settings: State<'_, Mutex<SettingsStore>>,
) -> Result<(), String> {
    let push = settings.lock().map_err(|e| e.to_string())?.push().clone();
    if push.target == "none" {
        return Err("Choose a target first.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let notifier = app.state::<super::notify::PushHandle>();
        notifier.0.test(&push)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::attention::Kind;
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;

    fn event(kind: Kind, run: Option<&str>) -> Event {
        Event {
            kind,
            subject: "s".into(),
            tab_id: None,
            window_label: None,
            tab_label: Some("Claude · secret-project".into()),
            run_id: run.map(str::to_string),
            run_dir: Some("/Users/me/secret/path".into()),
            detail: Some("PR7 add the billing token".into()),
        }
    }

    fn settings(target: &str) -> PushSettings {
        PushSettings {
            target: target.into(),
            ..Default::default()
        }
    }

    #[derive(Default)]
    struct MemorySecrets(Mutex<HashMap<String, String>>);

    impl SecretStore for MemorySecrets {
        fn get(&self, name: &str) -> Result<Option<String>, String> {
            Ok(self.0.lock().unwrap().get(name).cloned())
        }
        fn set(&self, name: &str, value: &str) -> Result<(), String> {
            self.0.lock().unwrap().insert(name.into(), value.into());
            Ok(())
        }
        fn clear(&self, name: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(name);
            Ok(())
        }
    }

    #[test]
    fn the_payload_has_exactly_the_run_and_the_event() {
        let payload = Payload::for_event(
            &event(Kind::MergedCleaned, Some("r-1")),
            &settings("webhook"),
        )
        .unwrap();
        let json = serde_json::to_value(&payload).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort();
        assert_eq!(keys, ["event", "run"]);
        assert_eq!(json["run"], "r-1");
        assert_eq!(json["event"], "merged-cleaned");
    }

    #[test]
    fn only_the_app_only_kinds_with_a_target_and_the_toggle_on_are_pushed() {
        let on = settings("ntfy");
        for kind in [
            Kind::Stall,
            Kind::CheckFailed,
            Kind::MergedCleaned,
            Kind::AutopostFailed,
        ] {
            assert!(
                Payload::for_event(&event(kind, Some("r")), &on).is_some(),
                "{kind:?}"
            );
        }
        for kind in [
            Kind::NeedsYou,
            Kind::ApprovalWait,
            Kind::RunDone,
            Kind::RunFailed,
            Kind::WaitingReminder,
        ] {
            assert!(
                Payload::for_event(&event(kind, Some("r")), &on).is_none(),
                "{kind:?}"
            );
        }
        assert!(Payload::for_event(&event(Kind::Stall, Some("r")), &settings("none")).is_none());
        let mut off = on.clone();
        off.events.insert("stall".into(), false);
        assert!(Payload::for_event(&event(Kind::Stall, Some("r")), &off).is_none());
    }

    #[test]
    fn bodies_are_built_from_the_two_fields_only() {
        let payload =
            Payload::for_event(&event(Kind::CheckFailed, Some("r-9")), &settings("ntfy")).unwrap();
        let secrets = Secrets {
            ntfy_topic: Some("my-topic".into()),
            pushover_token: Some("tok&en".into()),
            pushover_user: Some("usr".into()),
            webhook_url: Some("https://hooks.example.com/abc".into()),
        };
        let ntfy = build(&settings("ntfy"), &secrets, &payload).unwrap();
        assert_eq!(ntfy.url, "https://ntfy.sh/my-topic");
        assert_eq!(ntfy.body, "check-failed: r-9");
        let pushover = build(&settings("pushover"), &secrets, &payload).unwrap();
        assert_eq!(pushover.url, PUSHOVER_URL);
        assert_eq!(
            pushover.body,
            "token=tok%26en&user=usr&title=DCTerminalv2&message=check-failed%3A%20r-9"
        );
        let hook = build(&settings("webhook"), &secrets, &payload).unwrap();
        assert_eq!(hook.url, "https://hooks.example.com/abc");
        assert_eq!(hook.body, r#"{"run":"r-9","event":"check-failed"}"#);
        for request in [&ntfy, &pushover, &hook] {
            for leak in ["secret-project", "/Users/me", "billing", "PR7"] {
                assert!(!request.body.contains(leak), "{leak} leaked");
            }
        }
        assert!(build(&settings("ntfy"), &Secrets::default(), &payload).is_err());
        assert!(build(&settings("none"), &secrets, &payload).is_err());
    }

    #[test]
    fn addresses_are_https_or_this_machine() {
        assert!(check_url("https://ntfy.sh/topic").is_ok());
        assert!(check_url("http://127.0.0.1:8080/x").is_ok());
        assert!(check_url("http://localhost/x").is_ok());
        assert!(check_url("http://ntfy.example.com/x").is_err());
        assert!(check_url("ftp://x").is_err());
        assert!(check_url("https://").is_err());
        assert!(check_url("https://a b").is_err());
    }

    #[test]
    fn settings_never_hold_a_secret() {
        let secrets = MemorySecrets::default();
        secrets.set("ntfy-topic", "SECRET-TOPIC-123").unwrap();
        secrets.set("pushover-token", "SECRET-TOKEN-456").unwrap();
        secrets
            .set("webhook-url", "https://hooks.example.com/SECRET-789")
            .unwrap();
        let mut push = settings("ntfy");
        push.events.insert("stall".into(), false);
        let file = crate::store::settings_store::SettingsFile {
            push,
            ..Default::default()
        };
        let text = serde_json::to_string(&file).unwrap();
        for secret in ["SECRET-TOPIC-123", "SECRET-TOKEN-456", "SECRET-789"] {
            assert!(!text.contains(secret));
        }
        // Settings can only name which secrets are saved, never their values.
        assert_eq!(
            saved_names(&secrets).unwrap(),
            ["ntfy-topic", "pushover-token", "webhook-url"]
        );
    }

    /// The request as text: the head, then the body by its Content-Length
    /// (a client may send them in separate segments).
    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut data = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = stream.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&data).to_string();
            if let Some((head, body)) = text.split_once("\r\n\r\n") {
                let want = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                if body.len() >= want {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&data).to_string()
    }

    /// A server on this machine that answers each request with the next
    /// status of `statuses` (or hangs for `hang`), and records the requests.
    fn fake_server(statuses: Vec<u16>, hang: bool) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            let mut statuses = statuses.into_iter();
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let request = read_request(&mut stream);
                log.lock().unwrap().push(request);
                if hang {
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
                let status = statuses.next().unwrap_or(200);
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                );
            }
        });
        (format!("http://127.0.0.1:{port}"), seen)
    }

    fn client(secrets: Arc<MemorySecrets>, timeout: Duration) -> PushClient {
        PushClient::start(
            Arc::new(UreqTransport::new(timeout)),
            secrets,
            Duration::from_millis(10),
        )
    }

    fn webhook_to(url: &str) -> (PushSettings, Arc<MemorySecrets>) {
        let secrets = Arc::new(MemorySecrets::default());
        secrets.set("webhook-url", &format!("{url}/hook")).unwrap();
        (settings("webhook"), secrets)
    }

    #[test]
    fn a_push_is_posted_as_json_to_the_webhook() {
        let (url, seen) = fake_server(vec![200], false);
        let (push, secrets) = webhook_to(&url);
        let client = client(secrets, Duration::from_secs(2));
        client.test(&push).unwrap();
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("POST /hook"), "{}", requests[0]);
        assert!(requests[0].contains(r#"{"run":"test","event":"test"}"#));
    }

    #[test]
    fn a_failed_push_is_retried_once_and_then_given_up() {
        let (url, seen) = fake_server(vec![500, 500, 500], false);
        let (push, secrets) = webhook_to(&url);
        let client = client(secrets, Duration::from_secs(2));
        let err = client.test(&push).unwrap_err();
        assert!(err.contains("500"), "{err}");
        assert_eq!(seen.lock().unwrap().len(), 2, "one try and one retry");

        let (url, seen) = fake_server(vec![500, 200], false);
        let (push, secrets) = webhook_to(&url);
        let client = self::client(secrets, Duration::from_secs(2));
        client.test(&push).unwrap();
        assert_eq!(seen.lock().unwrap().len(), 2, "the retry got through");
    }

    #[test]
    fn a_slow_server_times_out_and_does_not_hold_the_caller() {
        let (url, _) = fake_server(vec![], true);
        let (push, secrets) = webhook_to(&url);
        let client = client(secrets, Duration::from_millis(300));
        let started = std::time::Instant::now();
        let err = client.test(&push).unwrap_err();
        assert!(err.contains("time"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(4));
        // Queueing an event returns at once even while the worker is busy.
        let queued = std::time::Instant::now();
        client.push(&event(Kind::Stall, Some("r")), &push);
        assert!(queued.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn a_merged_cleaned_event_of_a_run_with_no_open_tab_sends_one_push() {
        let (url, seen) = fake_server(vec![200], false);
        let (push, secrets) = webhook_to(&url);
        let client = client(secrets, Duration::from_secs(2));
        // The S5 fixture: no tab, only the run.
        let cleaned = event(Kind::MergedCleaned, Some("r-20261027-0900-abcd"));
        assert!(cleaned.tab_id.is_none());
        client.push(&cleaned, &push);
        assert!(crate::test_support::eventually(|| seen
            .lock()
            .unwrap()
            .len()
            == 1));
        assert!(seen.lock().unwrap()[0]
            .contains(r#""run":"r-20261027-0900-abcd","event":"merged-cleaned""#));
    }
}
