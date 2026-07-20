//! VPN tunnel lifecycle: fetches the tunnel spec from the manager and
//! drives a [`WireGuardBackend`] through connect/rotate/disconnect.
//!
//! This is the greenfield half of the module — see this crate's top-level
//! doc for exactly what Go never actually implemented here. Every method
//! below does the real thing: [`VpnManager::connect`] generates a fresh
//! local keypair, fetches the manager's tunnel config (sending that
//! keypair's public half — see [`VpnManager::fetch_tunnel_config`]'s doc
//! for the known manager-side gap this depends on), and hands a genuine
//! [`TunnelSpec`] to the backend; [`VpnManager::disconnect`] tears the
//! interface down for real; [`VpnManager::rotate`] re-fetches and
//! re-applies against the *same* local key (a config/allowed-IPs refresh,
//! not a re-keying) rather than being a no-op.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use defguard_wireguard_rs::key::Key;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::auth::AuthManager;
use crate::config::ModuleConfig;
use crate::http;
use crate::wireguard::{PeerStats, TunnelSpec, WgBackendError, WireGuardBackend};

const CONFIG_PATH: &str = "/api/v1/config";
/// Matches the manager auth client's own timeout — see `auth.rs`.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Every way the VPN lifecycle can fail.
#[derive(Debug, thiserror::Error)]
pub enum VpnError {
    /// [`VpnManager::connect`]/`rotate` need a valid access token first.
    #[error("no valid token")]
    NoToken,
    /// [`VpnManager::connect`] called while already connected.
    #[error("already connected")]
    AlreadyConnected,
    /// [`VpnManager::rotate`] called before any successful `connect`.
    #[error("not connected")]
    NotConnected,
    /// The tunnel-config request itself failed.
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    /// The manager answered, but not with 2xx.
    #[error("request failed with status {status}: {body}")]
    Status { status: u16, body: String },
    /// The 2xx response body was not the expected JSON shape.
    #[error("failed to parse response: {0}")]
    Decode(#[from] serde_json::Error),
    /// A field of the manager's response could not be turned into a valid
    /// tunnel spec (bad key, bad CIDR, non-literal endpoint, ...).
    #[error("invalid tunnel config: {0}")]
    InvalidConfig(String),
    /// The backend rejected `apply`/`teardown`/`peer_stats`.
    #[error(transparent)]
    Backend(#[from] WgBackendError),
}

/// The manager's `/api/v1/config` response.
#[derive(Debug, Clone, Deserialize)]
struct TunnelConfigResponse {
    tunnel_address: String,
    server_public_key: String,
    server_endpoint: String,
    #[serde(default)]
    allowed_ips: Vec<String>,
    #[serde(default)]
    dns: Vec<String>,
}

/// Connection state guarded by [`VpnManager`]'s single lock.
#[derive(Default)]
struct VpnState {
    connected: bool,
    /// This client's own keypair for the current session — generated once
    /// per `connect`, reused by `rotate` (a rotate is a config refresh,
    /// not a re-key), and dropped on `disconnect`.
    private_key: Option<Key>,
}

/// Drives one WireGuard tunnel's lifecycle against a manager and a
/// [`WireGuardBackend`]. See this module's doc for what each method does.
pub struct VpnManager {
    config: ModuleConfig,
    #[allow(dead_code)] // reserved for on-disk tunnel state in a later milestone
    data_dir: PathBuf,
    http: reqwest::Client,
    backend: Arc<dyn WireGuardBackend>,
    state: Mutex<VpnState>,
}

impl VpnManager {
    /// Builds a new VPN manager over `backend` — the concrete backend is
    /// chosen by the caller (production code via
    /// [`crate::wireguard::select_backend`], tests via a
    /// [`crate::wireguard::fake::FakeBackend`]), matching Go's
    /// `vpnMgr.wgClient = NewFakeWGController()` override but as
    /// constructor injection instead of a post-construction field swap.
    pub fn new(
        config: ModuleConfig,
        data_dir: PathBuf,
        backend: Arc<dyn WireGuardBackend>,
    ) -> VpnManager {
        VpnManager {
            config,
            data_dir,
            http: http::build_client(HTTP_TIMEOUT),
            backend,
            state: Mutex::new(VpnState::default()),
        }
    }

    /// The module's static local configuration (for `Status` detail).
    pub fn config(&self) -> &ModuleConfig {
        &self.config
    }

    /// Whether the tunnel is currently connected.
    pub async fn is_connected(&self) -> bool {
        self.state.lock().await.connected
    }

    /// Establishes the tunnel: generates a fresh local keypair, fetches
    /// the manager's tunnel config, and hands a real [`TunnelSpec`] to the
    /// backend, which genuinely creates and configures the interface.
    /// Errors (rather than silently reporting "connected") if already
    /// connected.
    pub async fn connect(&self, auth: &AuthManager) -> Result<(), VpnError> {
        let mut state = self.state.lock().await;
        if state.connected {
            return Err(VpnError::AlreadyConnected);
        }

        let private_key = Key::generate();
        let public_key = private_key.public_key();

        let tunnel_config = self.fetch_tunnel_config(auth, &public_key).await?;
        let spec = self.build_tunnel_spec(&private_key, &tunnel_config)?;

        self.backend
            .apply(&self.config.interface_name, &spec)
            .await?;

        state.private_key = Some(private_key);
        state.connected = true;
        Ok(())
    }

    /// Tears the tunnel down for real — unlike Go's `Disconnect`, which
    /// only flipped a boolean. Idempotent: disconnecting while not
    /// connected succeeds without calling the backend.
    pub async fn disconnect(&self) -> Result<(), VpnError> {
        let mut state = self.state.lock().await;
        if !state.connected {
            return Ok(());
        }

        self.backend.teardown(&self.config.interface_name).await?;
        state.connected = false;
        state.private_key = None;
        Ok(())
    }

    /// Re-fetches the manager's tunnel config (using the *same* local
    /// keypair established at `connect`) and re-applies it — a genuine
    /// re-apply, not a no-op. See [`crate::wireguard`]'s module doc for
    /// why re-applying is always a full replace of the peer, never an
    /// incremental accumulation.
    ///
    /// `force` is accepted for CLI parity with Go's `--force` flag but
    /// this port has no cached-and-still-valid-config fast path to skip in
    /// the first place — every `rotate` call always re-fetches — so it
    /// currently has no effect. Kept as a parameter rather than dropped so
    /// a future "skip refetch if not stale" optimization has somewhere to
    /// plug in without changing the public signature again.
    pub async fn rotate(&self, auth: &AuthManager, _force: bool) -> Result<(), VpnError> {
        // Held for the whole call so a concurrent `disconnect` can't race
        // a rotate's re-apply. `connected`/`private_key` are unchanged by
        // a successful rotate — a config refresh, not a reconnect.
        let state = self.state.lock().await;
        let Some(private_key) = state.private_key.clone() else {
            return Err(VpnError::NotConnected);
        };

        let public_key = private_key.public_key();
        let tunnel_config = self.fetch_tunnel_config(auth, &public_key).await?;
        let spec = self.build_tunnel_spec(&private_key, &tunnel_config)?;

        self.backend
            .apply(&self.config.interface_name, &spec)
            .await?;

        Ok(())
    }

    /// Reads the tunnel's live peer stats from the backend — real device
    /// reads on every call, never a value cached at `connect` time. Fixes
    /// the Go bug documented on [`crate::wireguard::PeerStats`].
    pub async fn peer_stats(&self) -> Result<PeerStats, VpnError> {
        Ok(self.backend.peer_stats(&self.config.interface_name).await?)
    }

    /// Fetches the manager's tunnel config for this node, sending
    /// `public_key` as a query parameter.
    ///
    /// **Known manager-side gap** (see `docs/PARITY.md` §1.21): the
    /// manager must accept and register this public key against the node
    /// before a real WireGuard handshake can ever succeed — Go's client
    /// never sent a public key here at all (`GET /api/v1/config` with no
    /// body or query params), so no manager implementation to date has
    /// had a reason to consume one. This client sends it correctly
    /// regardless of whether the manager currently does anything with it.
    async fn fetch_tunnel_config(
        &self,
        auth: &AuthManager,
        public_key: &Key,
    ) -> Result<TunnelConfigResponse, VpnError> {
        let token = auth.token().await;
        if token.is_empty() {
            return Err(VpnError::NoToken);
        }

        let url = format!("{}{CONFIG_PATH}", self.config.manager_url);
        let response = self
            .http
            .get(&url)
            .bearer_auth(token)
            .query(&[("public_key", public_key.to_string())])
            .send()
            .await
            .map_err(VpnError::Request)?;

        let status = response.status();
        let bytes = response.bytes().await.map_err(VpnError::Request)?;
        if !status.is_success() {
            return Err(VpnError::Status {
                status: status.as_u16(),
                body: String::from_utf8_lossy(&bytes).into_owned(),
            });
        }
        serde_json::from_slice(&bytes).map_err(VpnError::Decode)
    }

    /// Turns the manager's response into a backend-ready [`TunnelSpec`].
    /// `allowed_ips`/`dns` fall back to this module's local config when
    /// the manager supplies none — see `config.rs`'s module doc for why.
    ///
    /// `server_endpoint` must be a literal `ip:port` — unlike Go's
    /// `net.ResolveUDPAddr`, this does not perform DNS resolution.
    /// Accepting a hostname here would need an async lookup this
    /// deliberately-synchronous builder can't do, and would make this
    /// method's tests reach onto the real network — a hard constraint
    /// this milestone's tests must not cross. Real WireGuard endpoints are
    /// overwhelmingly configured as literal IPs in practice (resolving a
    /// hostname before the tunnel that would otherwise carry that
    /// resolution is its own bootstrapping problem), so this is a
    /// reasonable place to draw the line for this milestone.
    fn build_tunnel_spec(
        &self,
        private_key: &Key,
        tunnel_config: &TunnelConfigResponse,
    ) -> Result<TunnelSpec, VpnError> {
        let client_address = parse_field("tunnel_address", &tunnel_config.tunnel_address)?;

        let peer_public_key: Key = tunnel_config
            .server_public_key
            .as_str()
            .try_into()
            .map_err(|_| VpnError::InvalidConfig("invalid server_public_key".to_string()))?;

        let endpoint = SocketAddr::from_str(&tunnel_config.server_endpoint).map_err(|_| {
            VpnError::InvalidConfig(format!(
                "server_endpoint {:?} is not a literal ip:port",
                tunnel_config.server_endpoint
            ))
        })?;

        let allowed_ip_sources: &[String] = if tunnel_config.allowed_ips.is_empty() {
            &self.config.allowed_ips
        } else {
            &tunnel_config.allowed_ips
        };
        let mut allowed_ips = Vec::with_capacity(allowed_ip_sources.len());
        for raw in allowed_ip_sources {
            allowed_ips.push(parse_field("allowed_ips", raw)?);
        }

        let dns_sources: &[String] = if tunnel_config.dns.is_empty() {
            &self.config.dns
        } else {
            &tunnel_config.dns
        };
        let mut dns = Vec::with_capacity(dns_sources.len());
        for raw in dns_sources {
            let addr: IpAddr = parse_field("dns", raw)?;
            dns.push(addr);
        }

        let keepalive = if self.config.keepalive == 0 {
            None
        } else {
            Some(Duration::from_secs(self.config.keepalive))
        };

        Ok(TunnelSpec {
            private_key: private_key.clone(),
            client_address,
            peer_public_key,
            endpoint,
            allowed_ips,
            dns,
            mtu: self.config.mtu,
            keepalive,
        })
    }
}

fn parse_field<T: FromStr>(field: &str, raw: &str) -> Result<T, VpnError> {
    raw.parse()
        .map_err(|_| VpnError::InvalidConfig(format!("invalid {field} entry {raw:?}")))
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use penguin_sdk::SecretStore;

    use crate::testutil::{InMemorySecretStore, MockManager, MockResponse};
    use crate::wireguard::fake::{FakeBackend, RecordedCall};

    use super::*;

    fn sample_config(manager_url: &str) -> ModuleConfig {
        ModuleConfig {
            manager_url: manager_url.to_string(),
            node_id: "node-1".to_string(),
            interface_name: "wg-test0".to_string(),
            mtu: 1420,
            dns: Vec::new(),
            keepalive: 25,
            allowed_ips: Vec::new(),
            embedded: true,
        }
    }

    async fn authed(manager_url: &str) -> AuthManager {
        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.set("api_key", b"k").await.unwrap();
        let auth = AuthManager::new(manager_url.to_string(), secrets).await;
        auth.ensure_valid_token().await.expect("token obtained");
        auth
    }

    fn tunnel_config_json(allowed_ips: &str, dns: &str) -> String {
        format!(
            r#"{{"tunnel_address":"10.0.0.2/32","server_public_key":"sMlWwt2d4gkKsPl6gWAGqtEgp2Xo2S4xyJ1wFjNsFEs=","server_endpoint":"203.0.113.1:51820","allowed_ips":{allowed_ips},"dns":{dns}}}"#
        )
    }

    async fn seed_auth_and_config(manager: &MockManager, allowed_ips: &str, dns: &str) {
        manager
            .respond(
                "POST",
                "/api/v1/auth/token",
                MockResponse::json(200, r#"{"access_token":"tok","expires_at":9999999999}"#),
            )
            .await;
        manager
            .respond(
                "GET",
                CONFIG_PATH,
                MockResponse::json(200, tunnel_config_json(allowed_ips, dns)),
            )
            .await;
    }

    #[tokio::test]
    async fn connect_creates_and_configures_then_disconnect_removes() {
        let manager = MockManager::start().await;
        seed_auth_and_config(&manager, r#"["10.0.0.0/24"]"#, r#"["1.1.1.1"]"#).await;
        let auth = authed(&manager.base_url).await;

        let backend = Arc::new(FakeBackend::new());
        let vpn = VpnManager::new(
            sample_config(&manager.base_url),
            PathBuf::from("/tmp"),
            backend.clone(),
        );

        vpn.connect(&auth).await.expect("connect succeeds");
        assert!(vpn.is_connected().await);
        assert!(backend.is_configured("wg-test0"));

        let spec = backend.last_spec("wg-test0").expect("spec recorded");
        assert_eq!(spec.dns, vec![IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))]);
        assert_eq!(spec.allowed_ips.len(), 1);

        vpn.disconnect().await.expect("disconnect succeeds");
        assert!(!vpn.is_connected().await);
        assert!(!backend.is_configured("wg-test0"));

        assert_eq!(
            backend.calls(),
            vec![
                RecordedCall::Apply {
                    interface: "wg-test0".to_string()
                },
                RecordedCall::Teardown {
                    interface: "wg-test0".to_string()
                },
            ]
        );

        // The manager's config fetch must have carried our generated
        // public key — the fix for the known manager-side gap.
        let requests = manager.requests().await;
        let config_req = requests
            .iter()
            .find(|r| r.path.starts_with(CONFIG_PATH))
            .expect("config request recorded");
        assert!(config_req.path.contains("public_key="));
        assert_eq!(manager.request_count("GET", CONFIG_PATH).await, 1);

        manager.stop().await;
    }

    #[tokio::test]
    async fn connect_twice_fails_without_touching_the_backend_again() {
        let manager = MockManager::start().await;
        seed_auth_and_config(&manager, r#"["10.0.0.0/24"]"#, "[]").await;
        let auth = authed(&manager.base_url).await;

        let backend = Arc::new(FakeBackend::new());
        let vpn = VpnManager::new(
            sample_config(&manager.base_url),
            PathBuf::from("/tmp"),
            backend.clone(),
        );

        vpn.connect(&auth).await.unwrap();
        let err = vpn.connect(&auth).await.unwrap_err();
        assert!(matches!(err, VpnError::AlreadyConnected));
        assert_eq!(backend.calls().len(), 1, "second connect must not re-apply");

        manager.stop().await;
    }

    #[tokio::test]
    async fn rotate_re_applies_with_the_same_key_and_updated_config() {
        let manager = MockManager::start().await;
        seed_auth_and_config(&manager, r#"["10.0.0.0/24"]"#, "[]").await;
        manager
            .respond(
                "GET",
                CONFIG_PATH,
                MockResponse::json(200, tunnel_config_json(r#"["10.0.1.0/24"]"#, "[]")),
            )
            .await;
        let auth = authed(&manager.base_url).await;

        let backend = Arc::new(FakeBackend::new());
        let vpn = VpnManager::new(
            sample_config(&manager.base_url),
            PathBuf::from("/tmp"),
            backend.clone(),
        );

        vpn.connect(&auth).await.unwrap();
        let first_key = backend.last_spec("wg-test0").unwrap().private_key;

        vpn.rotate(&auth, false).await.expect("rotate succeeds");
        let second_spec = backend.last_spec("wg-test0").unwrap();

        assert_eq!(
            first_key.as_array(),
            second_spec.private_key.as_array(),
            "rotate must reuse the same local key, not re-key"
        );
        assert_eq!(
            second_spec.allowed_ips[0].to_string(),
            "10.0.1.0/24",
            "rotate must pick up the manager's updated config"
        );
        assert!(vpn.is_connected().await, "rotate must not disconnect");

        let apply_calls = backend
            .calls()
            .into_iter()
            .filter(|c| matches!(c, RecordedCall::Apply { .. }))
            .count();
        assert_eq!(apply_calls, 2, "connect + rotate each apply once");

        manager.stop().await;
    }

    #[tokio::test]
    async fn rotate_before_connect_fails() {
        let manager = MockManager::start().await;
        manager
            .respond(
                "POST",
                "/api/v1/auth/token",
                MockResponse::json(200, r#"{"access_token":"tok","expires_at":9999999999}"#),
            )
            .await;
        let auth = authed(&manager.base_url).await;
        let backend = Arc::new(FakeBackend::new());
        let vpn = VpnManager::new(
            sample_config(&manager.base_url),
            PathBuf::from("/tmp"),
            backend,
        );

        let err = vpn.rotate(&auth, false).await.unwrap_err();
        assert!(matches!(err, VpnError::NotConnected));
        manager.stop().await;
    }

    #[tokio::test]
    async fn peer_stats_come_from_the_backend_not_a_cached_timestamp() {
        let manager = MockManager::start().await;
        seed_auth_and_config(&manager, r#"["10.0.0.0/24"]"#, "[]").await;
        let auth = authed(&manager.base_url).await;

        let backend = Arc::new(FakeBackend::new());
        let vpn = VpnManager::new(
            sample_config(&manager.base_url),
            PathBuf::from("/tmp"),
            backend.clone(),
        );
        vpn.connect(&auth).await.unwrap();

        let configured_stats = crate::wireguard::PeerStats {
            last_handshake: Some(std::time::SystemTime::now()),
            rx_bytes: 4096,
            tx_bytes: 2048,
        };
        backend.set_peer_stats(configured_stats);

        let read_back = vpn.peer_stats().await.expect("peer_stats succeeds");
        assert_eq!(read_back, configured_stats);

        manager.stop().await;
    }

    #[tokio::test]
    async fn local_dns_and_allowed_ips_are_used_only_when_manager_sends_none() {
        let manager = MockManager::start().await;
        seed_auth_and_config(&manager, "[]", "[]").await;
        let auth = authed(&manager.base_url).await;

        let mut config = sample_config(&manager.base_url);
        config.allowed_ips = vec!["192.168.0.0/16".to_string()];
        config.dns = vec!["9.9.9.9".to_string()];

        let backend = Arc::new(FakeBackend::new());
        let vpn = VpnManager::new(config, PathBuf::from("/tmp"), backend.clone());
        vpn.connect(&auth).await.unwrap();

        let spec = backend.last_spec("wg-test0").unwrap();
        assert_eq!(spec.allowed_ips[0].to_string(), "192.168.0.0/16");
        assert_eq!(spec.dns, vec![IpAddr::V4(Ipv4Addr::new(9, 9, 9, 9))]);

        manager.stop().await;
    }

    #[tokio::test]
    async fn embedded_flag_selects_the_backend_kind_vpn_manager_ends_up_using() {
        use crate::wireguard::{BackendKind, select_backend};

        let embedded_backend = select_backend(true);
        assert_eq!(embedded_backend.kind(), BackendKind::Userspace);

        #[cfg(target_os = "linux")]
        {
            let non_embedded_backend = select_backend(false);
            assert_eq!(non_embedded_backend.kind(), BackendKind::Kernel);
        }
    }
}
