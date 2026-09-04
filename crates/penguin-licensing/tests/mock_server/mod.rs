//! A minimal, single-purpose mock HTTP/1.1 server for exercising
//! [`penguin_licensing::LicenseClient`] without a real network call.
//!
//! This deliberately is not a general-purpose test-HTTP-server: it
//! understands just enough of HTTP/1.1 (a request line, headers up to the
//! blank line, and an optional `Content-Length` body) to receive the one
//! POST request the client ever sends, then writes back a canned response
//! and closes the connection. Built on a bare `tokio::net::TcpListener`
//! rather than a mocking crate so the crate's `cargo tree -i ring` gate
//! only ever has to account for `penguin-licensing`'s own real dependency
//! graph, never a test helper's.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// One canned HTTP response the mock server hands back for a request.
pub struct MockResponse {
    status: u16,
    body: String,
}

impl MockResponse {
    /// A response with `Content-Type: application/json` and the given
    /// pre-serialized JSON body.
    pub fn json(status: u16, body: &str) -> Self {
        MockResponse {
            status,
            body: body.to_string(),
        }
    }

    /// A response with a plain-text body — used for the non-JSON and
    /// error-status test cases.
    pub fn text(status: u16, body: &str) -> Self {
        MockResponse {
            status,
            body: body.to_string(),
        }
    }
}

/// A running mock server bound to an ephemeral localhost port.
pub struct MockServer {
    pub base_url: String,
    // `Option` so `stop`/`Drop` can `take()` it out — a plain `JoinHandle`
    // can't be moved out of a type that implements `Drop`.
    accept_loop: Option<JoinHandle<()>>,
    calls: Arc<AtomicUsize>,
}

impl MockServer {
    /// Starts a server that hands back `responses[call_index]` for the Nth
    /// accepted connection, repeating the last entry forever once the list
    /// is exhausted — a one-entry list models "always answers the same
    /// way", a two-entry list models "first call succeeds, every call
    /// after behaves differently" (the shape most graceful-degradation
    /// tests need).
    pub async fn start(responses: Vec<MockResponse>) -> MockServer {
        assert!(
            !responses.is_empty(),
            "MockServer needs at least one response"
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server listener");
        let base_url = format!("http://{}", listener.local_addr().expect("local addr"));

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_loop = Arc::clone(&calls);
        let responses = Arc::new(responses);

        let accept_loop = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let responses = Arc::clone(&responses);
                let calls = Arc::clone(&calls_for_loop);
                tokio::spawn(async move {
                    let index = calls.fetch_add(1, Ordering::SeqCst);
                    let response = &responses[index.min(responses.len() - 1)];
                    serve_one(stream, response).await;
                });
            }
        });

        MockServer {
            base_url,
            accept_loop: Some(accept_loop),
            calls,
        }
    }

    /// Returns a base URL bound to a port nothing is listening on: the
    /// listener is created (to reserve a real, otherwise-unused port) and
    /// then immediately dropped, so any connection attempt to it fails
    /// fast with "connection refused" instead of hanging. Models a license
    /// server that has never been reachable, for tests that don't need a
    /// prior successful call first.
    pub async fn unreachable_base_url() -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind throwaway listener");
        let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
        drop(listener);
        base_url
    }

    /// The number of connections accepted so far.
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Stops accepting connections and waits for the accept loop to fully
    /// exit, so the port is guaranteed closed (and any new connection
    /// attempt refused) before this returns. Preferred over relying on
    /// `Drop` whenever a test needs the "now unreachable" transition to
    /// have already happened before it proceeds.
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

/// Reads one HTTP/1.1 request off `stream` (headers plus an optional
/// `Content-Length` body) and discards it, then writes `response` and
/// closes the connection. Sufficient for a client that sends exactly one
/// request per connection and never pipelines — which is all this crate's
/// HTTP client ever does.
async fn serve_one(stream: TcpStream, response: &MockResponse) {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut content_length = 0usize;

    loop {
        let mut line = String::new();
        let Ok(bytes_read) = reader.read_line(&mut line).await else {
            return;
        };
        if bytes_read == 0 || line == "\r\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        let Some(value) = lower.strip_prefix("content-length:") else {
            continue;
        };
        content_length = value.trim().parse().unwrap_or(0);
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body).await.is_err() {
        return;
    }

    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        status_text(response.status),
        response.body.len(),
    );
    let _ = writer.write_all(head.as_bytes()).await;
    let _ = writer.write_all(response.body.as_bytes()).await;
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
