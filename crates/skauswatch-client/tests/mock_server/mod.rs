//! A minimal, single-purpose mock HTTP/1.1 server for exercising
//! [`skauswatch_client::SkausWatchClient::register`] without a real network
//! call.
//!
//! A bare `tokio::net::TcpListener` rather than a mocking crate — matches
//! `crates/penguin-licensing/tests/mock_server/mod.rs` and
//! `crates/waddlebot-client/tests/mock_http/mod.rs`, so this crate's own
//! `cargo tree` dependency graph never grows a test-only HTTP framework.
//!
//! Unlike those two, the one request received per connection is recorded
//! into shared state *before* the response is written back, not after —
//! [`MockServer::last_path`]/[`MockServer::last_body_contains`] are plain
//! sync methods (per the Task 2 brief), so a caller reading them right
//! after `client.register().await` returns must never race the server
//! task's own bookkeeping.
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

/// Starts a mock server that answers `/api/v1/endpoint/register` (or any
/// other path — this mock doesn't route on it) with a 200
/// `{"agent_id": ..., "api_key": ...}` body: the happy-path response
/// [`skauswatch_client::SkausWatchClient::register`] expects.
pub async fn start_register_ok(agent_id: &str, api_key: &str) -> MockServer {
    let body = serde_json::json!({ "agent_id": agent_id, "api_key": api_key }).to_string();
    MockServer::start(200, body).await
}

/// Starts a server that answers 200 with a canned config body
/// (`{"heartbeat_secs":30}` — a valid [`skauswatch_client::AgentConfig`]
/// body, so `fetch_config` round-trips against it too) iff the request
/// carries both a non-empty `x-agent-id` header and a well-formed
/// `x-api-key` header (64 lowercase hex chars — a hex-encoded HMAC-SHA256
/// digest), else 401. Doesn't recompute the HMAC itself (this test module
/// never holds the signing key) — it only checks that
/// `SkausWatchClient::send_signed` actually attached both auth headers,
/// which is what these tests exist to prove.
pub async fn start_auth_echo() -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));

    let last_request: Arc<Mutex<Option<RecordedRequest>>> = Arc::new(Mutex::new(None));
    let last_request_for_loop = Arc::clone(&last_request);

    let accept_loop = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let last_request = Arc::clone(&last_request_for_loop);
            tokio::spawn(async move {
                serve_one(stream, &last_request, |req| {
                    if has_valid_auth_headers(req) {
                        (200, r#"{"heartbeat_secs":30}"#.to_string())
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

/// Builds a [`skauswatch_client::SkausWatchClient`] pointed at `server`.
/// `agent_id`/`api_key` don't affect how the client itself is built — auth
/// happens per-call from the [`skauswatch_client::AgentIdentity`] each
/// method takes directly — they're folded into a synthetic enrollment
/// token purely so call sites read as "the client used by this agent" and
/// the parameters aren't dead weight.
pub fn client_for(
    server: &MockServer,
    agent_id: &str,
    api_key: &str,
) -> skauswatch_client::SkausWatchClient {
    let cfg = skauswatch_client::ClientConfig::new(
        server.base_url(),
        format!("mock-enrollment-{agent_id}-{api_key}"),
    );
    skauswatch_client::SkausWatchClient::new(cfg).expect("client builds for mock server")
}

/// Whether `req` carries the two auth headers
/// [`skauswatch_client::SkausWatchClient`]'s `send_signed` is required to
/// attach: a non-empty `x-agent-id`, and an `x-api-key` that's exactly 64
/// lowercase hex characters (a hex-encoded HMAC-SHA256 digest).
fn has_valid_auth_headers(req: &RecordedRequest) -> bool {
    let has_agent_id = req.headers.get("x-agent-id").is_some_and(|v| !v.is_empty());
    let has_api_key = req
        .headers
        .get("x-api-key")
        .is_some_and(|v| v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit()));
    has_agent_id && has_api_key
}

/// Reads one HTTP/1.1 request off `stream` (request line, headers, and an
/// optional `Content-Length` body), records it, computes a response via
/// `respond` (called with the recorded request so a mock like
/// [`MockServer::start_auth_echo`] can answer based on what was sent), then
/// writes the response and closes the connection.
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
    let path = request_line
        .trim_end()
        .split(' ')
        .nth(1)
        .unwrap_or("")
        .to_string();

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
        400 => "Bad Request",
        500 => "Internal Server Error",
        _ => "Status",
    }
}
