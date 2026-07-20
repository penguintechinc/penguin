//! The userspace WireGuard backend: `boringtun`'s Noise protocol engine.
//!
//! # What this genuinely does
//!
//! [`build_tunn`] builds a real `boringtun::noise::Tunn` — the actual
//! WireGuard handshake/transport state machine — from a [`TunnelSpec`]'s
//! keys, and [`UserspaceBackend::apply`] calls it before returning, so a
//! spec really is round-tripped through boringtun's protocol engine on
//! every call. This crate's tests exercise `Tunn` directly (see
//! `format_handshake_initiation` in this file's test module) to prove the
//! integration produces genuine WireGuard wire packets, entirely offline.
//!
//! # What this does not do, and why
//!
//! `boringtun`'s default build (what the workspace pins — see the root
//! `Cargo.toml` comment on this dependency) is exactly what its own docs
//! describe: the Noise protocol engine only. There is no TUN device, no UDP
//! socket, and no packet-forwarding event loop — `boringtun` does ship one
//! (gated behind its own `device` Cargo feature, used by `boringtun-cli`),
//! but wiring a second, independently-threaded I/O engine into this
//! module's async lifecycle, plus per-OS TUN creation and routing-table
//! integration, is a genuinely separate, large piece of work from "port the
//! Tobogganing module" and is not attempted here.
//!
//! [`UserspaceBackend::apply`] is honest about that boundary: it returns
//! [`WgBackendError::Unsupported`] rather than claiming the tunnel is up
//! when no packet can actually flow. This is a deliberate improvement over
//! Go, whose `WGController.Configure` was a hard-coded `return nil`
//! (`go-client/internal/modules/tobogganing/vpn_wgctrl.go`) — a silent
//! false "success" with no interface, no peer, and no way for a caller to
//! tell the difference from a real connection. A clear, typed "not
//! supported yet" is strictly more honest than that, even though neither
//! implementation can move a packet.
use async_trait::async_trait;
use boringtun::noise::Tunn;
use boringtun::x25519;

use super::{BackendKind, PeerStats, TunnelSpec, WgBackendError};

/// The reason [`UserspaceBackend::apply`] always fails — see this module's
/// doc for the full explanation.
const DATA_PLANE_REASON: &str = "boringtun's TUN device / UDP socket / packet-forwarding event loop is not wired up in this build; only the kernel backend (embedded: false, Linux) can bring up a live tunnel";

/// `boringtun`-backed WireGuard engine. Holds no state: see module doc for
/// why there is nothing to hold between calls yet.
#[derive(Debug, Default, Clone, Copy)]
pub struct UserspaceBackend;

impl UserspaceBackend {
    /// Builds a new userspace backend. Cheap: performs no I/O.
    pub fn new() -> UserspaceBackend {
        UserspaceBackend
    }
}

#[async_trait]
impl super::WireGuardBackend for UserspaceBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Userspace
    }

    /// Builds a real [`Tunn`] from `spec` (proving the keys round-trip
    /// through boringtun's protocol engine), then reports that bringing up
    /// a live tunnel is not supported — see this module's doc.
    async fn apply(&self, _interface: &str, spec: &TunnelSpec) -> Result<(), WgBackendError> {
        let _tunn = build_tunn(spec);
        Err(WgBackendError::Unsupported {
            operation: "apply",
            reason: DATA_PLANE_REASON,
        })
    }

    /// No userspace tunnel is ever actually brought up (see [`apply`]), so
    /// there are never live stats to report — reads the same as an
    /// interface that was never configured, not an error.
    async fn peer_stats(&self, _interface: &str) -> Result<PeerStats, WgBackendError> {
        Ok(PeerStats::default())
    }

    /// Nothing was ever created, so tearing down is trivially idempotent.
    async fn teardown(&self, _interface: &str) -> Result<(), WgBackendError> {
        Ok(())
    }
}

/// Builds a `boringtun` Noise-protocol tunnel from `spec`'s keys. Infallible
/// by construction: `spec.private_key`/`spec.peer_public_key` are already
/// validated 32-byte WireGuard keys by the time a [`TunnelSpec`] exists
/// (parsed in `vpn.rs` from the manager's response), and raw X25519 key
/// material has no further validity constraint `Tunn::new` could reject.
fn build_tunn(spec: &TunnelSpec) -> Tunn {
    let private = x25519::StaticSecret::from(spec.private_key.as_array());
    let public = x25519::PublicKey::from(spec.peer_public_key.as_array());
    let keepalive = spec
        .keepalive
        .map(|interval| interval.as_secs().min(u64::from(u16::MAX)) as u16);
    Tunn::new(private, public, None, keepalive, 0, None)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::time::Duration;

    use defguard_wireguard_rs::key::Key;

    use super::*;
    use crate::wireguard::WireGuardBackend;

    fn sample_spec() -> TunnelSpec {
        TunnelSpec {
            private_key: Key::generate(),
            client_address: "10.0.0.2/32".parse().unwrap(),
            peer_public_key: Key::generate().public_key(),
            endpoint: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)), 51820),
            allowed_ips: vec!["10.0.0.0/24".parse().unwrap()],
            dns: Vec::new(),
            mtu: 1420,
            keepalive: Some(Duration::from_secs(25)),
        }
    }

    /// Proves the boringtun integration is real: a [`Tunn`] built from a
    /// [`TunnelSpec`] can format a genuine WireGuard handshake-initiation
    /// packet, entirely offline (no socket, no TUN device, no network).
    #[test]
    fn build_tunn_formats_a_real_handshake_initiation_packet() {
        let spec = sample_spec();
        let mut tunn = build_tunn(&spec);

        let mut buf = [0u8; 148];
        let result = tunn.format_handshake_initiation(&mut buf, false);

        match result {
            boringtun::noise::TunnResult::WriteToNetwork(packet) => {
                // Message type 1 (handshake initiation), little-endian, is
                // the first four bytes of every WireGuard handshake-init
                // packet on the wire.
                assert_eq!(&packet[0..4], &1u32.to_le_bytes());
                assert_eq!(packet.len(), 148);
            }
            other => panic!("expected a handshake initiation packet, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn apply_reports_unsupported_rather_than_a_silent_success() {
        let backend = UserspaceBackend::new();
        let spec = sample_spec();

        let Err(err) = backend.apply("wg0", &spec).await else {
            panic!("userspace apply must not silently claim success");
        };
        match err {
            WgBackendError::Unsupported { operation, .. } => assert_eq!(operation, "apply"),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn peer_stats_and_teardown_are_harmless_when_nothing_was_ever_up() {
        let backend = UserspaceBackend::new();
        let stats = backend.peer_stats("wg0").await.expect("peer_stats");
        assert_eq!(stats, PeerStats::default());
        backend.teardown("wg0").await.expect("teardown");
    }

    #[test]
    fn kind_reports_userspace() {
        assert_eq!(UserspaceBackend::new().kind(), BackendKind::Userspace);
    }
}
