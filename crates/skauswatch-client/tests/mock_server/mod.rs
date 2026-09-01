//! A minimal, single-purpose mock HTTP/1.1 server for exercising
//! [`skauswatch_client::SkausWatchClient`] without a real network call.
//!
//! A bare `tokio::net::TcpListener` rather than a mocking crate — matches
//! `crates/penguin-licensing/tests/mock_server/mod.rs` and
//! `crates/waddlebot-client/tests/mock_http/mod.rs`, so this crate's own
//! `cargo tree` dependency graph never grows a test-only HTTP framework.
//!
//! The one request received per connection is recorded into shared state
//! *before* the response is written back, not after —
//! [`MockServer::last_path`]/[`MockServer::last_body_contains`] are plain
//! sync methods, so a caller reading them right after `client.<call>().await`
//! returns must never race the server task's own bookkeeping.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// One request the mock server received, captured before any response is
/// written.
#[derive(Debug, Clone, Default)]
struct RecordedRequest {
    method: String,
    path: String,
    /// Header names lowercased, values as sent.
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

/// A running mock server bound to an ephemeral localhost port, answering
/// every accepted connection with the same canned status + JSON body.
pub struct MockServer {
    base_url: String,
    accept_loop: Option<JoinHandle<()>>,
    last_request: Arc<Mutex<Option<RecordedRequest>>>,
}

impl MockServer {
    /// Starts a server that answers every accepted connection with
    /// `(status, json_body)`.
    async fn start(status: u16, json_body: String) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server listener");
        let base_url = format!("http://{}", listener.local_addr().expect("local addr"));

        let last_request: Arc<Mutex<Option<RecordedRequest>>> = Arc::new(Mutex::new(None));
        let last_request_for_loop = Arc::clone(&last_request);
        let json_body = Arc::new(json_body);

        let accept_loop = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let last_request = Arc::clone(&last_request_for_loop);
                let json_body = Arc::clone(&json_body);
                tokio::spawn(async move {
                    serve_one(stream, &last_request, |_req| (status, (*json_body).clone())).await;
                });
            }
        });

        MockServer {
            base_url,
            accept_loop: Some(accept_loop),
            last_request,
        }
    }

    /// Base URL of the mock server, suitable for
    /// [`skauswatch_client::ClientConfig::base_url`].
    pub fn base_url(&self) -> String {
        self.base_url.clone()
    }

    /// The request-line target (path, including any query string) of the
    /// most recently received request. Panics if no request has been
    /// received yet.
    pub fn last_path(&self) -> String {
        self.last_request
            .lock()
            .expect("mock server state mutex poisoned")
            .as_ref()
            .expect("no request received yet")
            .path
            .clone()
    }

    /// The request-line HTTP method (e.g. `"GET"`, `"POST"`) of the most
    /// recently received request. `None` if no request has been received
    /// yet.
    pub fn last_method(&self) -> Option<String> {
        self.last_request
            .lock()
            .expect("mock server state mutex poisoned")
            .as_ref()
            .map(|r| r.method.clone())
    }

    /// Whether the most recently received request's raw body contains
    /// `needle`. Panics if no request has been received yet.
    pub fn last_body_contains(&self, needle: &str) -> bool {
        let guard = self
            .last_request
            .lock()
            .expect("mock server state mutex poisoned");
        let recorded = guard.as_ref().expect("no request received yet");
        String::from_utf8_lossy(&recorded.body).contains(needle)
    }

    /// The value of header `name` (case-insensitive) on the most recently
    /// received request. `None` if no request has been received yet, or
    /// the header wasn't sent — unlike [`Self::last_path`]/
    /// [`Self::last_body_contains`], this doesn't panic on either case,
    /// since a missing header is exactly what a caller needs to assert on
    /// (see [`start_auth_echo`]'s 401 path).
    pub fn last_header(&self, name: &str) -> Option<String> {
        self.last_request
            .lock()
            .expect("mock server state mutex poisoned")
            .as_ref()?
            .headers
            .get(&name.to_ascii_lowercase())
            .cloned()
    }

    /// Stops accepting connections and waits for the accept loop to fully
    /// exit, so the port is guaranteed closed before this returns.
    pub async fn stop(mut self) {
        let Some(handle) = self.accept_loop.take() else {
            return;
        };
        handle.abort();
        let _ = handle.await;
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        if let Some(handle) = self.accept_loop.take() {
            handle.abort();
        }
    }
}

/// Starts a mock server that answers every request with a 200
/// `{"message": ..., "agent_id": ..., "status": ...}` body — the real
/// `register_agent` response shape (`RegisterResponse`, endpoint.rs
/// ~line 358-363), deliberately carrying no `api_key`.
pub async fn start_register_ok(agent_id: &str, message: &str, status: &str) -> MockServer {
    let body = serde_json::json!({
        "message": message,
        "agent_id": agent_id,
        "status": status,
    })
    .to_string();
    MockServer::start(200, body).await
}

/// Starts a server that answers every request with a fixed non-2xx
/// `status`, regardless of headers or body sent — exercises the non-2xx
/// error branch ([`skauswatch_client::ClientError::Http`]), as opposed to
/// [`start_auth_echo`], which only returns non-2xx when auth headers are
/// missing.
pub async fn start_error(status: u16) -> MockServer {
    MockServer::start(status, "{}".to_string()).await
}

/// Starts a server that answers 200 iff the request carries both a
/// non-empty `x-agent-id` header and a non-empty `x-api-key` header, else
/// 401. Doesn't recompute/verify the HMAC the real Manager expects (this
/// test module never holds `ENDPOINT_API_SECRET`) — it only checks that
/// [`skauswatch_client::SkausWatchClient`] actually attached both static
/// auth headers, which is what these tests exist to prove; the api_key is
/// now an opaque provisioned string, not a client-computed hex digest, so
/// this deliberately does not assert any particular length/format on it
/// (unlike the pre-conform mock, which wrongly assumed a 64-hex-char HMAC
/// digest).
///
/// The 200 body is a single JSON object with every field any of
/// `heartbeat`/`report_events`/`fetch_config`'s response parsing needs —
/// `HeartbeatResponse`, `ReportEventsResponse`, and `AgentConfig` all
/// ignore fields they don't recognize, so one canned body round-trips
/// through all three.
pub async fn start_auth_echo() -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));

    let last_request: Arc<Mutex<Option<RecordedRequest>>> = Arc::new(Mutex::new(None));
    let last_request_for_loop = Arc::clone(&last_request);

    let ok_body = serde_json::json!({
        "status": "ok",
        "agent_id": "echoed-agent",
        "timestamp": "2026-08-31T00:00:00",
        "events_received": 1,
        "events_stored": 1,
        "errors": [],
        "config": {
            "reporting_interval": 60,
            "heartbeat_interval": 30,
            "event_batch_size": 50,
            "enabled_collectors": ["process", "network", "file"],
            "severity_threshold": "low",
        },
    })
    .to_string();

    let accept_loop = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let last_request = Arc::clone(&last_request_for_loop);
            let ok_body = ok_body.clone();
            tokio::spawn(async move {
                serve_one(stream, &last_request, |req| {
                    if has_valid_auth_headers(req) {
                        (200, ok_body.clone())
                    } else {
                        (401, "{}".to_string())
                    }
                })
                .await;
            });
        }
    });

    MockServer {
        base_url,
        accept_loop: Some(accept_loop),
        last_request,
    }
}

/// Builds a [`skauswatch_client::SkausWatchClient`] pointed at `server`,
/// configured with the given provisioned `agent_id`/`api_key` — no
/// enrollment token involved, since both are supplied out-of-band per the
/// real contract (see [`skauswatch_client::ClientConfig`]'s doc).
pub fn client_for(
    server: &MockServer,
    agent_id: &str,
    api_key: &str,
) -> skauswatch_client::SkausWatchClient {
    let cfg = skauswatch_client::ClientConfig::new(
        server.base_url(),
        agent_id.to_string(),
        api_key.to_string(),
        None,
    );
    skauswatch_client::SkausWatchClient::new(cfg).expect("client builds for mock server")
}

/// Whether `req` carries the two static auth headers
/// [`skauswatch_client::SkausWatchClient`] is required to attach: a
/// non-empty `x-agent-id` and a non-empty `x-api-key`.
fn has_valid_auth_headers(req: &RecordedRequest) -> bool {
    let has_agent_id = req.headers.get("x-agent-id").is_some_and(|v| !v.is_empty());
    let has_api_key = req.headers.get("x-api-key").is_some_and(|v| !v.is_empty());
    has_agent_id && has_api_key
}

/// Reads one HTTP/1.1 request off `stream` (request line, headers, and an
/// optional `Content-Length` body), records it, computes a response via
/// `respond` (called with the recorded request so a mock like
/// [`start_auth_echo`] can answer based on what was sent), then writes the
/// response and closes the connection.
async fn serve_one<F>(
    stream: TcpStream,
    last_request: &Arc<Mutex<Option<RecordedRequest>>>,
    respond: F,
) where
    F: FnOnce(&RecordedRequest) -> (u16, String),
{
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let mut request_line = String::new();
    let Ok(bytes_read) = reader.read_line(&mut request_line).await else {
        return;
    };
    if bytes_read == 0 {
        return;
    }
    let mut request_line_parts = request_line.trim_end().split(' ');
    let method = request_line_parts.next().unwrap_or("").to_string();
    let path = request_line_parts.next().unwrap_or("").to_string();

    let mut headers = HashMap::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        let Ok(bytes_read) = reader.read_line(&mut line).await else {
            break;
        };
        if bytes_read == 0 || line == "\r\n" {
            break;
        }
        let trimmed = line.trim_end();
        let Some((name, value)) = trimmed.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        if name == "content-length" {
            content_length = value.parse().unwrap_or(0);
        }
        headers.insert(name, value);
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body).await.is_err() {
        return;
    }

    let recorded = RecordedRequest {
        method,
        path,
        headers,
        body,
    };
    let (status, json_body) = respond(&recorded);

    // Recorded before the response is written — see the module doc for why
    // this ordering matters.
    *last_request
        .lock()
        .expect("mock server state mutex poisoned") = Some(recorded);

    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status,
        status_text(status),
        json_body.len(),
    );
    let _ = writer.write_all(head.as_bytes()).await;
    let _ = writer.write_all(json_body.as_bytes()).await;
    let _ = writer.shutdown().await;
}

/// A short reason phrase for the status codes this test suite actually
/// sends — not a general HTTP status registry.
fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        400 => "Bad Request",
        500 => "Internal Server Error",
        _ => "Status",
    }
}
