//! One HTTPS call to ClickUp with a deadline, behind [`Transport`] so tests
//! feed recorded replies and never use the network: a GET, or (C3) a POST or
//! PUT with a JSON body. Every call has the same rules: https only, no
//! redirects, the same reply size cap.
//!
//! Blocking on purpose: the app has no async runtime of its own, and every
//! caller is already on a worker thread (`spawn_blocking`).

use super::{ClickUpToken, API_BASE};
use std::time::Duration;
// The resolver and connector parts are ureq's `unversioned` API, which may
// change in a minor release: Cargo.toml pins ureq to 3.5.x for it.
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{
    ConnectionDetails, Connector, DefaultConnector, NextTimeout, Transport as UreqConnection,
};

/// One call may take this long, like one `gh` call (`github/runner.rs`).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(25);
/// Replies past this size are refused. A page of 100 tasks is well under it.
const MAX_BODY_BYTES: u64 = 16 * 1024 * 1024;

/// The only write methods there are (C3, plan decision E4): a POST adds a
/// comment, a PUT sets a status. There is no DELETE or PATCH to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMethod {
    Post,
    Put,
}

impl WriteMethod {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Post => "POST",
            Self::Put => "PUT",
        }
    }
}

/// What the client needs from HTTP.
pub trait Transport: Sync {
    /// GET `API_BASE` + `path` (which starts with `/` and may carry a query)
    /// with `Authorization: <token>`. `Err` means no HTTP reply came back.
    fn get(&self, path: &str, token: &ClickUpToken) -> Result<Reply, NoReply>;

    /// `method` `API_BASE` + `path` with `body` as JSON, and the same
    /// header (C3). `Err` means no HTTP reply came back.
    fn send(
        &self,
        method: WriteMethod,
        path: &str,
        body: &str,
        token: &ClickUpToken,
    ) -> Result<Reply, NoReply>;
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reply {
    pub status: u16,
    /// `X-RateLimit-Remaining`: calls left in this window.
    pub rate_remaining: Option<u32>,
    /// `X-RateLimit-Reset`: unix seconds when the window refills.
    pub rate_reset: Option<i64>,
    /// `Retry-After`, in seconds, when a 429 sends one.
    pub retry_after: Option<u64>,
    pub body: String,
}

/// Why no reply came back. Kinds only: a transport's own error text is never
/// passed on, so nothing it echoes can leak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoReply {
    Timeout,
    /// Nothing was sent: the name did not resolve or the connection was
    /// never made.
    Offline,
    /// The call broke after it started (a reset, a broken read), TLS, a
    /// reply too large to read, or anything else. A write may have landed.
    Failed,
}

/// The real transport: `ureq` with rustls and its bundled root certificates.
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            // 4xx and 5xx are replies to read, not transport errors.
            .http_status_as_error(false)
            .https_only(true)
            // A redirect could carry the header to another host.
            .max_redirects(0)
            .user_agent(concat!("DCTerminalv2/", env!("CARGO_PKG_VERSION")))
            .build();
        Self {
            agent: ureq::Agent::with_parts(
                config,
                BeforeSend(DefaultConnector::default()),
                NameNotFound(DefaultResolver::default()),
            ),
        }
    }
}

/// ureq's own resolver, with every lookup error reported as
/// `HostNotFound` (C3). ureq returns a failed lookup as a plain `Io` error,
/// which would read as a call that broke midway; nothing is sent before
/// the name resolves.
#[derive(Debug, Default)]
struct NameNotFound<R = DefaultResolver>(R);

impl<R: Resolver> Resolver for NameNotFound<R> {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        self.0
            .resolve(uri, config, timeout)
            .map_err(|_| ureq::Error::HostNotFound)
    }
}

/// ureq's own connector chain (proxy, TCP, TLS), with every error it
/// returns reported as `ConnectionFailed` (C3). The request is written only
/// after the connector hands back a connection, so nothing was sent when
/// it fails, whatever the error was (refused, unreachable, a connect
/// timeout, a proxy or TLS handshake failure).
#[derive(Debug, Default)]
struct BeforeSend<C = DefaultConnector>(C);

impl<C: Connector<(), Out = Box<dyn UreqConnection>>> Connector<()> for BeforeSend<C> {
    type Out = Box<dyn UreqConnection>;

    fn connect(
        &self,
        details: &ConnectionDetails,
        chained: Option<()>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        self.0
            .connect(details, chained)
            .map_err(|_| ureq::Error::ConnectionFailed)
    }
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

/// A header's value, parsed. ClickUp sends `X-Ratelimit-Remaining` in mixed
/// case; header names match case-insensitively.
fn header<T: std::str::FromStr>(headers: &ureq::http::HeaderMap, name: &str) -> Option<T> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

/// A reply with the rate-limit headers read and no body yet.
fn reply_head(status: u16, headers: &ureq::http::HeaderMap) -> Reply {
    Reply {
        status,
        rate_remaining: header(headers, "x-ratelimit-remaining"),
        rate_reset: header(headers, "x-ratelimit-reset"),
        retry_after: header(headers, "retry-after"),
        body: String::new(),
    }
}

/// Why no reply came back. `Offline` is only for errors before anything was
/// sent (the name did not resolve, the connection was never made: see
/// [`NameNotFound`] and [`BeforeSend`]), so a write that is `Offline` was
/// certainly not received. Any error mid-call (a reset, a broken read) is
/// `Failed`: ClickUp may have acted on it (C3). A refused connection is
/// pre-send too: a reset on an open connection reads as `ConnectionReset`.
fn no_reply(err: &ureq::Error) -> NoReply {
    match err {
        ureq::Error::HostNotFound
        | ureq::Error::ConnectionFailed
        | ureq::Error::ConnectProxyFailed(_)
        | ureq::Error::Timeout(ureq::Timeout::Resolve | ureq::Timeout::Connect) => NoReply::Offline,
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => {
            NoReply::Offline
        }
        ureq::Error::Timeout(_) => NoReply::Timeout,
        _ => NoReply::Failed,
    }
}

/// A reply from its status line and the body read. For a write, a status
/// line that arrived is the answer even when its body then failed to read:
/// a write's body is never used, so a 2xx is done, a 4xx is ClickUp's
/// refusal (not posted) and a 5xx stays a server error (maybe posted).
fn finish(
    mut head: Reply,
    body: Result<String, ureq::Error>,
    write: bool,
) -> Result<Reply, NoReply> {
    match body {
        Ok(body) => head.body = body,
        Err(_) if write => {}
        Err(err) => return Err(no_reply(&err)),
    }
    Ok(head)
}

/// The reply's status, rate-limit headers and body, read up to the cap.
fn read_reply(
    response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    write: bool,
) -> Result<Reply, NoReply> {
    let mut response = response.map_err(|err| no_reply(&err))?;
    let head = reply_head(response.status().as_u16(), response.headers());
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_BODY_BYTES)
        .read_to_string();
    finish(head, body, write)
}

impl Transport for UreqTransport {
    fn get(&self, path: &str, token: &ClickUpToken) -> Result<Reply, NoReply> {
        read_reply(
            self.agent
                .get(format!("{API_BASE}{path}"))
                .header("Authorization", token.expose())
                .header("Accept", "application/json")
                .call(),
            false,
        )
    }

    fn send(
        &self,
        method: WriteMethod,
        path: &str,
        body: &str,
        token: &ClickUpToken,
    ) -> Result<Reply, NoReply> {
        let url = format!("{API_BASE}{path}");
        let request = match method {
            WriteMethod::Post => self.agent.post(url),
            WriteMethod::Put => self.agent.put(url),
        };
        read_reply(
            request
                .header("Authorization", token.expose())
                .header("Accept", "application/json")
                .header("Content-Type", "application/json")
                .send(body),
            true,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ureq::http::{HeaderMap, HeaderName, HeaderValue};

    #[test]
    fn rate_limit_headers_are_read_in_any_case() {
        let mut headers = HeaderMap::new();
        for (name, value) in [
            ("X-Ratelimit-Limit", "100"),
            ("X-Ratelimit-Remaining", "37"),
            ("X-Ratelimit-Reset", "1790000060"),
            ("Retry-After", " 12 "),
        ] {
            headers.insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_static(value),
            );
        }
        let reply = reply_head(200, &headers);
        assert_eq!(reply.rate_remaining, Some(37));
        assert_eq!(reply.rate_reset, Some(1_790_000_060));
        assert_eq!(reply.retry_after, Some(12));
        assert_eq!(reply_head(200, &HeaderMap::new()).rate_remaining, None);
    }

    fn io(kind: std::io::ErrorKind) -> ureq::Error {
        ureq::Error::Io(std::io::Error::from(kind))
    }

    #[test]
    fn only_errors_before_anything_was_sent_count_as_offline() {
        use std::io::ErrorKind::*;
        // Before anything was sent: the shapes ureq 3.5 really returns.
        for (err, label) in [
            (ureq::Error::HostNotFound, "no address"),
            (ureq::Error::ConnectionFailed, "connector failed"),
            (io(ConnectionRefused), "connection refused"),
            (
                ureq::Error::Timeout(ureq::Timeout::Resolve),
                "lookup timed out",
            ),
            (
                ureq::Error::Timeout(ureq::Timeout::Connect),
                "connect timed out",
            ),
            (
                ureq::Error::ConnectProxyFailed("407".into()),
                "proxy refused",
            ),
        ] {
            assert_eq!(no_reply(&err), NoReply::Offline, "{label}");
        }
        assert_eq!(
            no_reply(&ureq::Error::Timeout(ureq::Timeout::Global)),
            NoReply::Timeout
        );
        assert_eq!(
            no_reply(&ureq::Error::Timeout(ureq::Timeout::RecvResponse)),
            NoReply::Timeout
        );
        // A reset or a broken read mid-call: the request may have landed.
        for kind in [
            ConnectionReset,
            BrokenPipe,
            UnexpectedEof,
            ConnectionAborted,
        ] {
            assert_eq!(no_reply(&io(kind)), NoReply::Failed, "{kind:?}");
        }
        assert_eq!(no_reply(&ureq::Error::BodyExceedsLimit(1)), NoReply::Failed);
    }

    #[test]
    fn a_write_s_status_line_is_its_answer_even_if_its_body_breaks() {
        let head = |status| Reply {
            status,
            ..Reply::default()
        };
        let reset = || Err(io(std::io::ErrorKind::ConnectionReset));
        assert_eq!(
            finish(head(200), reset(), true),
            Ok(head(200)),
            "ClickUp accepted the write"
        );
        assert_eq!(
            finish(head(400), reset(), true),
            Ok(head(400)),
            "ClickUp refused it: not posted"
        );
        assert_eq!(
            finish(head(502), reset(), true),
            Ok(head(502)),
            "a server error stays one"
        );
        assert_eq!(
            finish(head(200), reset(), false),
            Err(NoReply::Failed),
            "a read needs its body"
        );
        assert_eq!(
            finish(head(200), Ok("{}".into()), false).unwrap().body,
            "{}"
        );
    }

    /// A resolver that always fails the way ureq's own does on a lookup error.
    #[derive(Debug)]
    struct FailingLookup;

    impl Resolver for FailingLookup {
        fn resolve(
            &self,
            _: &ureq::http::Uri,
            _: &ureq::config::Config,
            _: NextTimeout,
        ) -> Result<ResolvedSocketAddrs, ureq::Error> {
            Err(io(std::io::ErrorKind::Other))
        }
    }

    #[test]
    fn a_failed_lookup_reads_as_host_not_found() {
        let uri: ureq::http::Uri = "https://api.clickup.com/api/v2/team".parse().unwrap();
        let config = ureq::Agent::config_builder().build();
        let err = NameNotFound(FailingLookup)
            .resolve(&uri, &config, NextTimeout::default())
            .unwrap_err();
        assert!(matches!(err, ureq::Error::HostNotFound), "{err}");
        assert_eq!(no_reply(&err), NoReply::Offline);
    }

    #[test]
    fn a_refused_connection_through_the_real_agent_reads_as_offline() {
        // A loopback port nothing listens on: no network, no ClickUp.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let transport = UreqTransport::new();
        let err = transport
            .agent
            .post(format!("https://127.0.0.1:{port}/task/x/comment"))
            .send("{}")
            .unwrap_err();
        assert!(matches!(err, ureq::Error::ConnectionFailed), "{err}");
        assert_eq!(no_reply(&err), NoReply::Offline);
    }

    #[test]
    fn post_and_put_are_the_only_write_methods() {
        // An exhaustive match: a new variant fails to build here first.
        for method in [WriteMethod::Post, WriteMethod::Put] {
            let name = match method {
                WriteMethod::Post => "POST",
                WriteMethod::Put => "PUT",
            };
            assert_eq!(method.as_str(), name);
        }
    }

    #[test]
    fn the_transport_refuses_plain_http_and_redirects() {
        // No network: these settings keep the header from going anywhere
        // but ClickUp.
        let transport = UreqTransport::new();
        let config = transport.agent.config();
        assert!(config.https_only());
        assert_eq!(config.max_redirects(), 0);
        assert!(!config.http_status_as_error());
        assert!(API_BASE.starts_with("https://api.clickup.com/"));
    }
}
