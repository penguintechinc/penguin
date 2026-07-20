//! The WireGuard data-plane contract [`vpn::VpnManager`](crate::vpn::VpnManager)
//! drives, plus the two concrete implementations Go never actually had (see
//! this crate's top-level doc for exactly what was stubbed).
//!
//! # Kernel vs. userspace
//!
//! [`kernel`] wraps `defguard_wireguard_rs`'s Linux netlink backend: it
//! genuinely creates the interface, configures it, reads real per-peer
//! stats back from the kernel, and removes the interface on teardown.
//!
//! [`userspace`] wraps `boringtun`'s Noise protocol engine to validate a
//! tunnel spec's keys are well-formed, but does not implement the TUN
//! device / UDP socket / packet-forwarding event loop a working userspace
//! data plane needs — see its module doc for exactly why and what a future
//! milestone would need to add.
//!
//! # Replace, not accumulate
//!
//! Every `apply()` call — both the first `connect` and every later `rotate`
//! — is a full re-specification of the interface's one peer, not an
//! incremental diff. This is a deliberate fix for a real Go gap: Go's
//! `VPNManager.Connect` built a fresh `wgtypes.Config` on every call but its
//! `WGController.Configure` never set `ReplacePeers`/`ReplaceAllowedIPs`
//! (`go-client/internal/modules/tobogganing/vpn_wgctrl.go` — moot in
//! practice there since `Configure` was `return nil`, but the gap was real:
//! a working implementation built the same way would have appended a new
//! peer entry on every rotation instead of updating the existing one).
//! [`kernel::KernelBackend`] avoids that by construction:
//! `defguard_wireguard_rs`'s own `Host::as_nlas`/`Peer::as_nlas_peer`
//! unconditionally set `WireguardDeviceFlags::ReplacePeers` and
//! `WireguardPeerFlags::ReplaceAllowedIps` on every device-level netlink
//! `SetDevice` call, so re-applying a [`TunnelSpec`] always yields exactly
//! the peer set the spec describes — never a superset of a previous one.

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use defguard_wireguard_rs::{key::Key, net::IpAddrMask};

// `defguard_wireguard_rs` only implements `WireguardInterfaceApi for
// WGApi<Kernel>` on Linux, FreeBSD, and Windows — not macOS, which has no
// in-kernel WireGuard at all. This crate only wires the kernel backend up
// on Linux (see `kernel_or_userspace_fallback` below), so the module is
// gated the same way: leaving it unconditional would fail to compile on
// macOS even though nothing there would ever construct a `KernelBackend`.
#[cfg(target_os = "linux")]
pub mod kernel;
pub mod userspace;

#[cfg(test)]
pub mod fake;

/// One WireGuard peer's realtime stats, always read back from the live
/// device — never a value stamped locally at connect time.
///
/// Fixes a real Go bug: `VPNManager.lastHandshake` was set to `time.Now()`
/// once inside `Connect` and never touched again
/// (`go-client/internal/modules/tobogganing/vpn.go`), so a tunnel whose
/// handshake had gone stale — or that had silently died — still reported
/// "just handshaked" for the lifetime of the process. Every field here
/// comes from [`WireGuardBackend::peer_stats`], which reads the live
/// device on every call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PeerStats {
    /// When the peer last completed a WireGuard handshake, or `None` if it
    /// never has.
    pub last_handshake: Option<SystemTime>,
    /// Bytes received from the peer, per the device's own counter.
    pub rx_bytes: u64,
    /// Bytes sent to the peer, per the device's own counter.
    pub tx_bytes: u64,
}

/// Everything needed to bring up, or re-apply, the single-peer client
/// tunnel: our own key and address, and the manager-supplied peer to talk
/// to. One [`TunnelSpec`] fully determines interface state — see this
/// module's doc for why re-applying one is always a full replace.
#[derive(Debug, Clone)]
pub struct TunnelSpec {
    /// This client's own WireGuard private key.
    pub private_key: Key,
    /// This client's address inside the tunnel (e.g. `10.0.0.2/32`).
    pub client_address: IpAddrMask,
    /// The manager's WireGuard public key.
    pub peer_public_key: Key,
    /// The manager's UDP endpoint.
    pub endpoint: SocketAddr,
    /// Routes to send down the tunnel.
    pub allowed_ips: Vec<IpAddrMask>,
    /// DNS servers to configure on the interface, if any. Empty means
    /// "leave host DNS alone" — see [`kernel::KernelBackend::apply`]'s doc
    /// for exactly when this is applied.
    pub dns: Vec<IpAddr>,
    /// Interface MTU.
    pub mtu: u32,
    /// Persistent keepalive interval, if any.
    pub keepalive: Option<Duration>,
}

/// Which concrete [`WireGuardBackend`] a trait object is. Exists so the
/// `embedded` config flag's effect is directly observable — in Go that
/// field was declared in the schema and read nowhere
/// (`go-client/internal/modules/tobogganing/module.go`'s `ModuleConfig`),
/// so nothing could ever prove it did anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// `defguard_wireguard_rs` over Linux netlink.
    Kernel,
    /// `boringtun`'s Noise protocol engine.
    Userspace,
}

impl BackendKind {
    /// A short, stable, lowercase identifier — logged at `init` so an
    /// operator can see which backend the `embedded` flag actually
    /// selected.
    pub fn as_str(&self) -> &'static str {
        match self {
            BackendKind::Kernel => "kernel",
            BackendKind::Userspace => "userspace",
        }
    }
}

/// Every way a [`WireGuardBackend`] call can fail.
#[derive(Debug, thiserror::Error)]
pub enum WgBackendError {
    /// The backend's underlying library reported a failure applying,
    /// reading, or removing interface state.
    #[error("wireguard interface error: {0}")]
    Interface(String),
    /// This backend cannot perform `operation` in this build. Returned
    /// instead of a silent no-op — see [`userspace`]'s module doc for the
    /// one case this currently applies to.
    #[error("{operation} is not supported by this backend: {reason}")]
    Unsupported {
        operation: &'static str,
        reason: &'static str,
    },
}

/// The WireGuard interface lifecycle [`vpn::VpnManager`](crate::vpn::VpnManager)
/// drives. Implemented by [`kernel::KernelBackend`] (real) and
/// [`userspace::UserspaceBackend`] (partial — see its doc), plus
/// [`fake::FakeBackend`] for hermetic tests.
#[async_trait]
pub trait WireGuardBackend: Send + Sync {
    /// Identifies which concrete implementation this is.
    fn kind(&self) -> BackendKind;

    /// Creates `interface` if it does not already exist, then fully
    /// configures it from `spec` — private key, address, MTU, and the
    /// single peer (public key, endpoint, allowed IPs, keepalive). Always a
    /// full replace, never an incremental update — see this module's doc.
    async fn apply(&self, interface: &str, spec: &TunnelSpec) -> Result<(), WgBackendError>;

    /// Reads `interface`'s live peer stats. Returns
    /// [`PeerStats::default`] (nothing set) if the interface has no
    /// configured peer yet, rather than an error — that is a normal
    /// "not connected" state, not a failure.
    async fn peer_stats(&self, interface: &str) -> Result<PeerStats, WgBackendError>;

    /// Removes `interface` entirely. Idempotent: tearing down an
    /// already-absent interface is not an error.
    async fn teardown(&self, interface: &str) -> Result<(), WgBackendError>;
}

/// Builds the backend the `embedded` config flag selects: `true` (Go's own
/// default) is the portable userspace engine, `false` opts into kernel
/// WireGuard where it's available. This is the one piece of the Go config
/// schema's `embedded` field actually being read anywhere.
pub fn select_backend(embedded: bool) -> Box<dyn WireGuardBackend> {
    if embedded {
        Box::new(userspace::UserspaceBackend::new())
    } else {
        kernel_or_userspace_fallback()
    }
}

/// On Linux, `Box::new(kernel::KernelBackend::new())`. Elsewhere — no
/// platform this workspace targets yet ships a kernel WireGuard backend
/// through `defguard_wireguard_rs` other than Linux netlink — falls back to
/// the userspace engine rather than failing `select_backend` outright,
/// since a module that cannot build its VPN backend at all cannot load.
#[cfg(target_os = "linux")]
fn kernel_or_userspace_fallback() -> Box<dyn WireGuardBackend> {
    Box::new(kernel::KernelBackend::new())
}

/// See [`kernel_or_userspace_fallback`]'s Linux-side doc.
#[cfg(not(target_os = "linux"))]
fn kernel_or_userspace_fallback() -> Box<dyn WireGuardBackend> {
    Box::new(userspace::UserspaceBackend::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_true_selects_userspace() {
        let backend = select_backend(true);
        assert_eq!(backend.kind(), BackendKind::Userspace);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn embedded_false_selects_kernel_on_linux() {
        let backend = select_backend(false);
        assert_eq!(backend.kind(), BackendKind::Kernel);
    }
}
