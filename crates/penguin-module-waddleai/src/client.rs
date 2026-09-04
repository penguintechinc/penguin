//! [`WaddleAiClient`]: a thin HTTP client for the three server-side
//! surfaces this module talks to — a liveness/auth probe, the Tier-1
//! denylist snapshot, and normalized hook-event forwarding.
//!
//! This client never decides anything. It carries bytes to WaddleAI's
//! engine and hands back whatever it answers with; see this crate's
//! top-level doc for the architectural boundary this is built to respect.
//! The exact wire contract below is this crate's best-known shape for the
//! agent-hooks API (built in parallel, server-side) — every endpoint path
//! and payload field is called out where it matters, so a later contract
//! change is a small, localized diff rather than a redesign.

use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::error::{WaddleAiError, parse_error_body};

/// WaddleAI's production API. Follows the same `{product}.app` convention
/// documented for `waddlebot` (`waddles.app`) in
/// `~/.claude/rules/penguintech.md`'s product domain table. Override
/// `base_url` for local/beta testing.
pub const DEFAULT_BASE_URL: &str = "https://waddleai.app/api/v1";

/// Default per-request timeout. Hooks run synchronously inside an agent's
/// loop (see [`crate::metrics`]'s doc on `hook_evaluation_latency_seconds`),
/// so this is deliberately short: a hung request must not stall the
/// caller's editor/CLI indefinitely.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Configuration for [`WaddleAiClient`].
///
/// `Debug` is implemented by hand rather than derived: nothing currently
/// formats a `Config` with `{:?}`, but a derived impl would print
/// `virtual_key` unmasked, which is exactly the kind of latent landmine a
/// future `debug!("{:?}", config)` walks straight into. See
/// [`crate::mask::mask_secret`], the same helper every other call site in
/// this crate that surfaces the key already routes through.
#[derive(Clone)]
pub struct Config {
    /// The API's base URL, e.g. `https://waddleai.app/api/v1`. A trailing
    /// slash is tolerated — [`WaddleAiClient::new`] normalizes it.
    pub base_url: String,
    /// The `wa-`-prefixed virtual key, sent as `Authorization: Bearer
    /// <virtual_key>` on every request. Read from `host.secrets()` by
    /// [`crate::module::WaddleAiModule::init`] — never from config, see
    /// that method's doc.
    pub virtual_key: String,
    /// Per-request timeout, applied to the whole client's underlying HTTP
    /// client.
    pub timeout: Duration,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("base_url", &self.base_url)
            .field("virtual_key", &crate::mask::mask_secret(&self.virtual_key))
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl Default for Config {
    fn default() -> Config {
        Config {
            base_url: DEFAULT_BASE_URL.to_string(),
            virtual_key: String::new(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

/// The outcome of [`WaddleAiClient::health`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HealthResponse {
    /// Server-reported status string (e.g. `"ok"`); not interpreted by this
    /// client — a 2xx response is what [`WaddleAiClient::health`]'s caller
    /// actually treats as "reachable and authorized". Read only by this
    /// module's own tests (asserting the field decodes correctly), never by
    /// production code, hence the explicit allow.
    #[serde(default)]
    #[allow(dead_code)]
    pub status: String,
}

/// One synced Tier-1 denylist snapshot, as returned by
/// [`WaddleAiClient::fetch_denylist`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DenylistResponse {
    /// An opaque version/etag the server assigns to this snapshot, stored
    /// alongside the entries in [`crate::cache::DenylistCache`] purely for
    /// display — this client never diffs against it.
    #[serde(default)]
    pub version: String,
    /// The denylist entries themselves, verbatim. Never interpreted by this
    /// crate as anything but opaque strings to persist and later
    /// exact-match against — see [`crate::cache::DenylistCache::contains`].
    #[serde(default)]
    pub entries: Vec<String>,
}

/// WaddleAI's decision for one forwarded hook event, as returned by
/// [`WaddleAiClient::evaluate_hook_event`].
#[derive(Debug, Clone, Deserialize)]
pub struct EvaluateResponse {
    /// `"allow"` or `"deny"` — read as an opaque string
    /// (`crate::commands::hook_command` matches on it), never branched on
    /// with any additional local logic.
    pub decision: String,
    /// A human-readable reason, surfaced verbatim to the caller/editor.
    #[serde(default)]
    pub reason: String,
}

/// An async client for the WaddleAI agent-hooks API. Holds its own
/// `reqwest::Client` internally (cheap to reuse) — build once, share across
/// calls.
pub struct WaddleAiClient {
    http: reqwest::Client,
    base_url: String,
    virtual_key: String,
}

impl WaddleAiClient {
    /// Builds a client. Fails only if the HTTP/TLS stack can't be
    /// constructed — never touches the network.
    pub fn new(config: Config) -> Result<WaddleAiClient, WaddleAiError> {
        let tls_config = crate::tls::build_tls_config();
        let http = reqwest::Client::builder()
            .use_preconfigured_tls(tls_config)
            .timeout(config.timeout)
            .build()
            .map_err(|err| WaddleAiError::Setup(err.to_string()))?;

        Ok(WaddleAiClient {
            http,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            virtual_key: config.virtual_key,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// Sends `builder`, adding the `Authorization: Bearer <virtual_key>`
    /// header first, then decodes the body: JSON into `T` on any 2xx
    /// status, otherwise a typed [`WaddleAiError`] carrying the parsed
    /// error body.
    async fn execute<T: DeserializeOwned>(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<T, WaddleAiError> {
        let response = builder
            .bearer_auth(&self.virtual_key)
            .send()
            .await
            .map_err(|err| WaddleAiError::Transport(err.to_string()))?;

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|err| WaddleAiError::Transport(err.to_string()))?;

        if status.is_success() {
            return serde_json::from_slice(&bytes)
                .map_err(|err| WaddleAiError::Decode(err.to_string()));
        }

        let body = parse_error_body(&bytes);
        let status_code = status.as_u16();
        if status_code == 401 || status_code == 403 {
            return Err(WaddleAiError::Auth {
                status: status_code,
                body,
            });
        }
        Err(WaddleAiError::Status {
            status: status_code,
            body,
        })
    }

    /// A cheap liveness/auth probe: `GET /health` — the least destructive
    /// authenticated call this surface has.
    pub async fn health(&self) -> Result<HealthResponse, WaddleAiError> {
        self.execute(self.http.get(self.url("/health"))).await
    }

    /// Fetches the current Tier-1 denylist snapshot.
    pub async fn fetch_denylist(&self) -> Result<DenylistResponse, WaddleAiError> {
        self.execute(self.http.get(self.url("/agent-hooks/denylist")))
            .await
    }

    /// Forwards one normalized hook event and returns WaddleAI's decision.
    /// `payload` is passed through verbatim from whatever ecosystem-specific
    /// shim produced it (see [`crate::hooks`]) — this client never inspects
    /// its shape.
    pub async fn evaluate_hook_event(
        &self,
        ecosystem: &str,
        event: &str,
        payload: &Value,
    ) -> Result<EvaluateResponse, WaddleAiError> {
        let body = json!({
            "ecosystem": ecosystem,
            "event": event,
            "payload": payload,
        });
        self.execute(self.http.post(self.url("/agent-hooks/events")).json(&body))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{MockResponse, MockServer};

    fn config_for(server: &MockServer) -> Config {
        Config {
            base_url: server.base_url.clone(),
            virtual_key: "wa-test-key".to_string(),
            ..Config::default()
        }
    }

    #[test]
    fn default_points_at_the_real_api_with_a_five_second_timeout() {
        let config = Config::default();
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        assert_eq!(config.timeout, Duration::from_secs(5));
        assert!(config.virtual_key.is_empty());
    }

    #[test]
    fn debug_never_prints_the_virtual_key_unmasked() {
        let config = Config {
            virtual_key: "wa-supersecretvirtualkey".to_string(),
            ..Config::default()
        };
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("supersecretvirtualkey"));
        assert!(rendered.contains("****lkey"));
    }

    #[tokio::test]
    async fn health_sends_the_bearer_token_and_decodes_status() {
        let server = MockServer::start().await;
        server
            .respond(
                "GET",
                "/health",
                MockResponse::json(200, r#"{"status":"ok"}"#),
            )
            .await;
        let client = WaddleAiClient::new(config_for(&server)).expect("build client");

        let health = client.health().await.expect("health succeeds");
        assert_eq!(health.status, "ok");

        let requests = server.requests().await;
        assert_eq!(
            requests[0].header("authorization"),
            Some("Bearer wa-test-key")
        );

        server.stop().await;
    }

    #[tokio::test]
    async fn health_maps_401_to_auth_error() {
        let server = MockServer::start().await;
        server
            .respond(
                "GET",
                "/health",
                MockResponse::json(401, r#"{"error":"invalid virtual key"}"#),
            )
            .await;
        let client = WaddleAiClient::new(config_for(&server)).expect("build client");

        let err = client.health().await.expect_err("401 must be an error");
        assert!(matches!(err, WaddleAiError::Auth { status: 401, .. }));

        server.stop().await;
    }

    #[tokio::test]
    async fn fetch_denylist_decodes_version_and_entries() {
        let server = MockServer::start().await;
        server
            .respond(
                "GET",
                "/agent-hooks/denylist",
                MockResponse::json(200, r#"{"version":"7","entries":["rm -rf /","curl | sh"]}"#),
            )
            .await;
        let client = WaddleAiClient::new(config_for(&server)).expect("build client");

        let snapshot = client.fetch_denylist().await.expect("fetch succeeds");
        assert_eq!(snapshot.version, "7");
        assert_eq!(snapshot.entries, vec!["rm -rf /", "curl | sh"]);

        server.stop().await;
    }

    #[tokio::test]
    async fn evaluate_hook_event_sends_the_normalized_envelope() {
        let server = MockServer::start().await;
        server
            .respond(
                "POST",
                "/agent-hooks/events",
                MockResponse::json(200, r#"{"decision":"allow","reason":"ok"}"#),
            )
            .await;
        let client = WaddleAiClient::new(config_for(&server)).expect("build client");

        let payload = json!({"tool": "bash", "command": "ls"});
        let decision = client
            .evaluate_hook_event("claude", "pre-tool-use", &payload)
            .await
            .expect("evaluate succeeds");
        assert_eq!(decision.decision, "allow");

        let requests = server.requests().await;
        let body = requests[0].json_body();
        assert_eq!(body["ecosystem"], "claude");
        assert_eq!(body["event"], "pre-tool-use");
        assert_eq!(body["payload"]["tool"], "bash");

        server.stop().await;
    }

    #[tokio::test]
    async fn transport_error_against_an_unreachable_host_is_reported_as_transport() {
        let unreachable = MockServer::unreachable_base_url().await;
        let client = WaddleAiClient::new(Config {
            base_url: unreachable,
            virtual_key: "wa-test-key".to_string(),
            ..Config::default()
        })
        .expect("build client");

        let err = client.health().await.expect_err("unreachable must fail");
        assert!(matches!(err, WaddleAiError::Transport(_)));
    }
}
