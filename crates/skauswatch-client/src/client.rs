//! The SkausWatch agent API client: registration against the Manager.

use std::time::Duration;

use crate::config::ClientConfig;
use crate::error::ClientError;
use crate::model::{AgentIdentity, RegisterRequest};
use crate::tls_support::ensure_crypto_provider_installed;

/// Path the Manager exposes for agent enrollment.
const REGISTER_PATH: &str = "/api/v1/endpoint/register";

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
}

/// Best-effort local hostname for the registration payload. The Manager
/// treats this field as informational only, never an identity key (that's
/// [`AgentIdentity::agent_id`], assigned by the Manager itself at
/// register), so a std-only lookup is enough — no new dependency for
/// something this minor: `COMPUTERNAME` on Windows, `HOSTNAME` on Unix
/// (exported by many shells/init systems, though not guaranteed), falling
/// back to a fixed placeholder when neither is set.
fn local_hostname() -> String {
    #[cfg(windows)]
    let var = "COMPUTERNAME";
    #[cfg(not(windows))]
    let var = "HOSTNAME";

    std::env::var(var).unwrap_or_else(|_| "unknown-host".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_builds_a_client_for_a_valid_config() {
        let cfg = ClientConfig::new("https://manager.example.com".to_string(), "tok".to_string());
        assert!(SkausWatchClient::new(cfg).is_ok());
    }
}
