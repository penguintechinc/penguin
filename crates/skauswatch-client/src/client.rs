//! The SkausWatch agent API client: registration against the Manager, plus
//! the three HMAC-authenticated calls an enrolled agent makes on an ongoing
//! basis (heartbeat, event reporting, config fetch).

use std::time::Duration;

use reqwest::Method;

use crate::auth::HmacSigner;
use crate::config::ClientConfig;
use crate::error::ClientError;
use crate::model::{AgentConfig, AgentIdentity, EndpointEvent, HeartbeatBody, RegisterRequest};
use crate::tls_support::ensure_crypto_provider_installed;

/// Path the Manager exposes for agent enrollment.
const REGISTER_PATH: &str = "/api/v1/endpoint/register";

/// Path the Manager exposes for agent heartbeats.
const HEARTBEAT_PATH: &str = "/api/v1/endpoint/heartbeat";

/// Path the Manager exposes for batched event reporting.
const EVENTS_PATH: &str = "/api/v1/endpoint/events";

/// Path the Manager exposes for fetching an agent's runtime config.
const CONFIG_PATH: &str = "/api/v1/endpoint/config";

/// Request timeout for every call this client makes — matches
/// `penguin-licensing`'s client default.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// An async client for the SkausWatch agent API. Holds its own
/// `reqwest::Client` (cheap to reuse across requests), so a
/// `SkausWatchClient` is meant to be built once and shared, not rebuilt per
/// request.
pub struct SkausWatchClient {
    http: reqwest::Client,
    cfg: ClientConfig,
}

impl SkausWatchClient {
    /// Builds a client. TLS is wired manually — rustls with the aws-lc-rs
    /// crypto provider (installed once, process-wide) and root
    /// certificates supplied from `webpki-roots` — exactly as
    /// `squawk-client`/`penguin-licensing` do, so this workspace never
    /// pulls in a second crypto backend (`ring`) alongside aws-lc-rs. Fails
    /// only if the HTTP/TLS stack can't be constructed; never touches the
    /// network.
    pub fn new(cfg: ClientConfig) -> Result<SkausWatchClient, ClientError> {
        ensure_crypto_provider_installed();

        let root_store = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        // NOT wrapped in `Some(...)`: reqwest's `use_preconfigured_tls`
        // wraps its argument in `Some(...)` itself before downcasting to
        // `Option<rustls::ClientConfig>` — see `penguin-licensing`'s
        // `build_http_client` doc comment for the failure mode this avoids
        // (an already-wrapped `Option` makes the downcast target
        // `Option<Option<ClientConfig>>` and silently falls through to
        // "Unknown TLS backend").
        let http = reqwest::Client::builder()
            .use_preconfigured_tls(tls_config)
            .timeout(DEFAULT_TIMEOUT)
            .build()?;

        Ok(SkausWatchClient { http, cfg })
    }

    /// Enrolls this agent against the Manager: POSTs
    /// `{ enrollment_token, hostname, os, arch, agent_version }` to
    /// `/api/v1/endpoint/register` and returns the [`AgentIdentity`] the
    /// Manager assigns. Any non-2xx response maps to
    /// [`ClientError::Http`].
    pub async fn register(&self) -> Result<AgentIdentity, ClientError> {
        let request = RegisterRequest {
            enrollment_token: self.cfg.enrollment_token.clone(),
            hostname: local_hostname(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            agent_version: env!("CARGO_PKG_VERSION").to_string(),
        };

        let url = format!("{}{REGISTER_PATH}", self.cfg.base_url.trim_end_matches('/'));
        let response = self.http.post(url).json(&request).send().await?;

        let status = response.status();
        if !status.is_success() {
            return Err(ClientError::Http {
                status: status.as_u16(),
            });
        }

        let identity: AgentIdentity = response.json().await?;
        Ok(identity)
    }

    /// Reports agent health to `/api/v1/endpoint/heartbeat`, HMAC-signed
    /// with `id`'s api_key. Any non-2xx response maps to
    /// [`ClientError::Http`].
    pub async fn heartbeat(
        &self,
        id: &AgentIdentity,
        status: &HeartbeatBody,
    ) -> Result<(), ClientError> {
        let body = serde_json::to_vec(status)?;
        self.send_signed(Method::POST, HEARTBEAT_PATH, id, body)
            .await?;
        Ok(())
    }

    /// Reports a batch of observed events to `/api/v1/endpoint/events`,
    /// HMAC-signed with `id`'s api_key. Any non-2xx response maps to
    /// [`ClientError::Http`].
    pub async fn report_events(
        &self,
        id: &AgentIdentity,
        events: &[EndpointEvent],
    ) -> Result<(), ClientError> {
        let body = serde_json::to_vec(events)?;
        self.send_signed(Method::POST, EVENTS_PATH, id, body)
            .await?;
        Ok(())
    }

    /// Fetches this agent's current runtime config from
    /// `/api/v1/endpoint/config` — an HMAC-signed GET over an empty body.
    /// Any non-2xx response maps to [`ClientError::Http`].
    pub async fn fetch_config(&self, id: &AgentIdentity) -> Result<AgentConfig, ClientError> {
        let response = self
            .send_signed(Method::GET, CONFIG_PATH, id, Vec::new())
            .await?;
        let config: AgentConfig = response.json().await?;
        Ok(config)
    }

    /// Shared plumbing for every HMAC-authenticated call: serializes
    /// nothing itself (the caller already has `body_bytes`), signs
    /// `body_bytes` with an [`HmacSigner`] built from `id`, attaches the
    /// resulting `x-agent-id`/`x-api-key` headers, sends the request, and
    /// maps a non-2xx status to [`ClientError::Http`] before the caller
    /// ever tries to parse a response body.
    async fn send_signed(
        &self,
        method: Method,
        path: &str,
        id: &AgentIdentity,
        body_bytes: Vec<u8>,
    ) -> Result<reqwest::Response, ClientError> {
        let signer = HmacSigner::new(id.agent_id.clone(), id.api_key.as_bytes().to_vec());
        let auth_headers = signer.headers(&body_bytes);

        let url = format!("{}{path}", self.cfg.base_url.trim_end_matches('/'));
        let mut request = self
            .http
            .request(method, url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body_bytes);
        for (name, value) in auth_headers {
            request = request.header(name, value);
        }

        let response = request.send().await?;

        let status = response.status();
        if !status.is_success() {
            return Err(ClientError::Http {
                status: status.as_u16(),
            });
        }
        Ok(response)
    }
}

/// Best-effort local hostname for the registration payload. The Manager
/// treats this field as informational only, never an identity key (that's
/// [`AgentIdentity::agent_id`], assigned by the Manager itself at
/// register) — but it's still the operator-facing label a monitoring
/// product's admin uses to tell endpoints apart, so it must be the real
/// hostname, not an env-var placeholder: `$HOSTNAME` is not exported under
/// systemd units, Windows services, or most other supervisors — exactly how
/// this agent runs in production — so an env-var fallback would report the
/// same placeholder for most real deployments, defeating endpoint
/// identification.
///
/// Unix: `nix::unistd::gethostname()`, the real `gethostname(2)` syscall —
/// matches `bins/penguind/src/daemon_main.rs`'s identical call for the
/// daemon's own `node_id`. Windows: `COMPUTERNAME`, which Windows reliably
/// sets for every process (unlike Unix's `$HOSTNAME`). Only a genuine
/// syscall/lookup failure falls back to the fixed placeholder.
fn local_hostname() -> String {
    #[cfg(unix)]
    {
        nix::unistd::gethostname()
            .ok()
            .and_then(|name| name.into_string().ok())
            .unwrap_or_else(|| "unknown-host".to_string())
    }
    #[cfg(windows)]
    {
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown-host".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_builds_a_client_for_a_valid_config() {
        let cfg = ClientConfig::new("https://manager.example.com".to_string(), "tok".to_string());
        assert!(SkausWatchClient::new(cfg).is_ok());
    }

    /// Regression guard for a prior version of `local_hostname()` that read
    /// the `$HOSTNAME` env var, which most real deployments (systemd
    /// units, Windows services, most other supervisors) never export — so
    /// it silently fell back to `"unknown-host"` for nearly every endpoint.
    /// Asserting equality with the real `gethostname(2)` result (not just
    /// "non-empty") means any regression back to an env-var read fails this
    /// test in this sandboxed CI environment too, where `$HOSTNAME` also
    /// happens to be unset.
    #[test]
    #[cfg(unix)]
    fn local_hostname_reports_the_real_syscall_hostname_on_unix() {
        let expected = nix::unistd::gethostname()
            .expect("gethostname(2) syscall")
            .into_string()
            .expect("hostname is valid UTF-8");
        assert_eq!(local_hostname(), expected);
        assert_ne!(local_hostname(), "unknown-host");
    }
}
