//! A minimal mock OTLP/HTTP collector for exercising [`penguin_otel::OtelPipeline`]
//! end-to-end without a real SigNoz/collector deployment.
//!
//! It is deliberately not a protobuf-aware OTLP receiver: it captures the raw
//! request path and body bytes for every `POST /v1/{metrics,traces,logs}` it
//! receives and lets the test assert on them as byte substrings. OTLP's
//! protobuf wire format encodes string keys/values (metric names, attribute
//! keys/values, resource attributes) as inline UTF-8 bytes, so a substring
//! check on the raw body is a valid, lightweight proof that a given name or
//! attribute was actually sent — without pulling in a full
//! `ExportMetricsServiceRequest` decode path just for this test.

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::routing::post;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant, sleep};

/// One captured OTLP export request.
#[derive(Clone)]
pub struct CapturedRequest {
    pub path: String,
    pub body: Vec<u8>,
}

impl CapturedRequest {
    /// True if both `key` and `value` appear as UTF-8 substrings anywhere in
    /// the raw protobuf body — a lightweight stand-in for decoding the
    /// `KeyValue` this attribute was encoded as.
    pub fn attributes_contain(&self, key: &str, value: &str) -> bool {
        contains_bytes(&self.body, key) && contains_bytes(&self.body, value)
    }

    /// Resource attributes are encoded the same way as regular attributes in
    /// OTLP's protobuf wire format, so this is the same check.
    pub fn resource_contains(&self, key: &str, value: &str) -> bool {
        self.attributes_contain(key, value)
    }
}

fn contains_bytes(haystack: &[u8], needle: &str) -> bool {
    let needle = needle.as_bytes();
    if needle.is_empty() || haystack.len() < needle.len() {
        return needle.is_empty();
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

type Requests = Arc<Mutex<Vec<CapturedRequest>>>;

/// A running mock collector bound to an ephemeral localhost port.
pub struct MockCollector {
    addr: std::net::SocketAddr,
    requests: Requests,
    server: JoinHandle<()>,
}

impl MockCollector {
    /// Starts the mock collector on an ephemeral port and begins accepting
    /// `POST /v1/{metrics,traces,logs}` requests immediately.
    pub async fn start() -> Self {
        let requests: Requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/metrics", post(capture))
            .route("/v1/traces", post(capture))
            .route("/v1/logs", post(capture))
            .with_state(requests.clone());

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock collector listener");
        let addr = listener.local_addr().expect("mock collector local addr");

        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("mock collector serve");
        });

        MockCollector {
            addr,
            requests,
            server,
        }
    }

    /// The collector's base URL (no trailing path) — pass this as
    /// `OtelConfig::endpoint`.
    pub fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Polls captured `/v1/metrics` requests until one whose body contains
    /// `metric_name` as a byte substring shows up, or panics after a 5s
    /// timeout. The pipeline's exporter runs on its own background
    /// thread/task, so the request may not have landed the instant this is
    /// called even after `OtelPipeline::shutdown()` forces a flush.
    pub async fn wait_for_metric(&self, metric_name: &str) -> CapturedRequest {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(found) = self.find_metric(metric_name) {
                return found;
            }
            if Instant::now() >= deadline {
                panic!("timed out waiting for a /v1/metrics request containing {metric_name:?}");
            }
            sleep(Duration::from_millis(20)).await;
        }
    }

    fn find_metric(&self, metric_name: &str) -> Option<CapturedRequest> {
        let guard = self.requests.lock().expect("requests mutex poisoned");
        guard
            .iter()
            .rev()
            .find(|r| r.path == "/v1/metrics" && contains_bytes(&r.body, metric_name))
            .cloned()
    }
}

impl Drop for MockCollector {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn capture(State(requests): State<Requests>, req: Request) -> StatusCode {
    let path = req.uri().path().to_string();
    let body: Bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    requests
        .lock()
        .expect("requests mutex poisoned")
        .push(CapturedRequest {
            path,
            body: body.to_vec(),
        });
    StatusCode::OK
}
