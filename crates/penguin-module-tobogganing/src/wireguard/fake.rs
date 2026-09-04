//! A [`WireGuardBackend`] test double recording every call it receives, so
//! hermetic tests can assert *what* [`crate::vpn::VpnManager`] asked a
//! backend to do without ever touching a real network interface.
//!
//! This is the direct analogue of Go's `FakeWGController`
//! (`go-client/internal/modules/tobogganing/fake.go`), extended with call
//! recording (Go's fake only remembered the *last* config per device name)
//! and injectable failures, since this milestone's tests need to assert on
//! call order and on error propagation that Go's fake had no way to model.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use super::{BackendKind, PeerStats, TunnelSpec, WgBackendError, WireGuardBackend};

/// One call [`FakeBackend`] received, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum RecordedCall {
    Apply { interface: String },
    PeerStats { interface: String },
    Teardown { interface: String },
}

/// Records every [`WireGuardBackend`] call and tracks which interfaces are
/// currently "configured" (i.e. have had `apply` succeed since their last
/// `teardown`), so tests can assert both call history and end state.
pub struct FakeBackend {
    kind: BackendKind,
    calls: Mutex<Vec<RecordedCall>>,
    configured: Mutex<HashMap<String, TunnelSpec>>,
    next_stats: Mutex<PeerStats>,
    fail_next_apply: Mutex<bool>,
    fail_next_teardown: Mutex<bool>,
    fail_next_peer_stats: Mutex<bool>,
}

impl FakeBackend {
    /// Builds a fake reporting [`BackendKind::Userspace`] — matches
    /// [`super::select_backend`]'s own default for `embedded: true`.
    pub fn new() -> FakeBackend {
        FakeBackend::with_kind(BackendKind::Userspace)
    }

    /// Builds a fake reporting a specific `kind`, for tests asserting the
    /// `embedded` flag threads through to whichever backend ends up
    /// configured.
    pub fn with_kind(kind: BackendKind) -> FakeBackend {
        FakeBackend {
            kind,
            calls: Mutex::new(Vec::new()),
            configured: Mutex::new(HashMap::new()),
            next_stats: Mutex::new(PeerStats::default()),
            fail_next_apply: Mutex::new(false),
            fail_next_teardown: Mutex::new(false),
            fail_next_peer_stats: Mutex::new(false),
        }
    }

    /// Sets the [`PeerStats`] the next (and every subsequent, until changed
    /// again) `peer_stats` call returns for a configured interface.
    pub fn set_peer_stats(&self, stats: PeerStats) {
        *self.next_stats.lock().unwrap() = stats;
    }

    /// Makes the very next `apply` call fail, then reverts to succeeding.
    pub fn fail_next_apply(&self) {
        *self.fail_next_apply.lock().unwrap() = true;
    }

    /// Makes the very next `teardown` call fail, then reverts to
    /// succeeding.
    pub fn fail_next_teardown(&self) {
        *self.fail_next_teardown.lock().unwrap() = true;
    }

    /// Makes the very next `peer_stats` call fail, then reverts to
    /// succeeding.
    pub fn fail_next_peer_stats(&self) {
        *self.fail_next_peer_stats.lock().unwrap() = true;
    }

    /// Every call received so far, in order.
    pub fn calls(&self) -> Vec<RecordedCall> {
        self.calls.lock().unwrap().clone()
    }

    /// Whether `interface` currently has a successfully-applied spec.
    pub fn is_configured(&self, interface: &str) -> bool {
        self.configured.lock().unwrap().contains_key(interface)
    }

    /// The spec the most recent successful `apply` for `interface` used, if
    /// any.
    pub fn last_spec(&self, interface: &str) -> Option<TunnelSpec> {
        self.configured.lock().unwrap().get(interface).cloned()
    }
}

impl Default for FakeBackend {
    fn default() -> FakeBackend {
        FakeBackend::new()
    }
}

#[async_trait]
impl WireGuardBackend for FakeBackend {
    fn kind(&self) -> BackendKind {
        self.kind
    }

    async fn apply(&self, interface: &str, spec: &TunnelSpec) -> Result<(), WgBackendError> {
        self.calls.lock().unwrap().push(RecordedCall::Apply {
            interface: interface.to_string(),
        });

        let mut fail = self.fail_next_apply.lock().unwrap();
        if *fail {
            *fail = false;
            return Err(WgBackendError::Interface("fake apply failure".to_string()));
        }
        drop(fail);

        self.configured
            .lock()
            .unwrap()
            .insert(interface.to_string(), spec.clone());
        Ok(())
    }

    async fn peer_stats(&self, interface: &str) -> Result<PeerStats, WgBackendError> {
        self.calls.lock().unwrap().push(RecordedCall::PeerStats {
            interface: interface.to_string(),
        });

        let mut fail = self.fail_next_peer_stats.lock().unwrap();
        if *fail {
            *fail = false;
            return Err(WgBackendError::Interface(
                "fake peer_stats failure".to_string(),
            ));
        }
        drop(fail);

        if !self.configured.lock().unwrap().contains_key(interface) {
            return Ok(PeerStats::default());
        }
        Ok(*self.next_stats.lock().unwrap())
    }

    async fn teardown(&self, interface: &str) -> Result<(), WgBackendError> {
        self.calls.lock().unwrap().push(RecordedCall::Teardown {
            interface: interface.to_string(),
        });

        let mut fail = self.fail_next_teardown.lock().unwrap();
        if *fail {
            *fail = false;
            return Err(WgBackendError::Interface(
                "fake teardown failure".to_string(),
            ));
        }
        drop(fail);

        self.configured.lock().unwrap().remove(interface);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use defguard_wireguard_rs::key::Key;

    use super::*;

    fn sample_spec() -> TunnelSpec {
        TunnelSpec {
            private_key: Key::generate(),
            client_address: "10.0.0.2/32".parse().unwrap(),
            peer_public_key: Key::generate().public_key(),
            endpoint: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)), 51820),
            allowed_ips: vec!["10.0.0.0/24".parse().unwrap()],
            dns: Vec::new(),
            mtu: 1420,
            keepalive: None,
        }
    }

    #[tokio::test]
    async fn apply_then_teardown_records_calls_and_clears_configured_state() {
        let backend = FakeBackend::new();
        backend.apply("wg0", &sample_spec()).await.unwrap();
        assert!(backend.is_configured("wg0"));

        backend.teardown("wg0").await.unwrap();
        assert!(!backend.is_configured("wg0"));

        assert_eq!(
            backend.calls(),
            vec![
                RecordedCall::Apply {
                    interface: "wg0".to_string()
                },
                RecordedCall::Teardown {
                    interface: "wg0".to_string()
                },
            ]
        );
    }

    #[tokio::test]
    async fn fail_next_apply_fails_once_then_recovers() {
        let backend = FakeBackend::new();
        backend.fail_next_apply();

        assert!(backend.apply("wg0", &sample_spec()).await.is_err());
        assert!(!backend.is_configured("wg0"));

        backend.apply("wg0", &sample_spec()).await.unwrap();
        assert!(backend.is_configured("wg0"));
    }

    #[tokio::test]
    async fn fail_next_teardown_fails_once_then_recovers() {
        let backend = FakeBackend::new();
        backend.apply("wg0", &sample_spec()).await.unwrap();
        backend.fail_next_teardown();

        assert!(backend.teardown("wg0").await.is_err());
        assert!(
            backend.is_configured("wg0"),
            "a failed teardown must not clear configured state"
        );

        backend.teardown("wg0").await.unwrap();
        assert!(!backend.is_configured("wg0"));
    }

    #[tokio::test]
    async fn fail_next_peer_stats_fails_once_then_recovers() {
        let backend = FakeBackend::new();
        backend.apply("wg0", &sample_spec()).await.unwrap();
        backend.fail_next_peer_stats();

        assert!(backend.peer_stats("wg0").await.is_err());
        backend
            .peer_stats("wg0")
            .await
            .expect("recovers on the next call");
    }

    #[tokio::test]
    async fn peer_stats_before_apply_defaults_and_after_apply_returns_configured_value() {
        let backend = FakeBackend::new();
        assert_eq!(
            backend.peer_stats("wg0").await.unwrap(),
            PeerStats::default()
        );

        backend.apply("wg0", &sample_spec()).await.unwrap();
        let stats = PeerStats {
            last_handshake: None,
            rx_bytes: 42,
            tx_bytes: 7,
        };
        backend.set_peer_stats(stats);
        assert_eq!(backend.peer_stats("wg0").await.unwrap(), stats);
    }
}
