//! The real WireGuard backend: Linux kernel WireGuard over netlink, via
//! `defguard_wireguard_rs`.
//!
//! Unlike Go's `realWGController` (`go-client/internal/modules/tobogganing/vpn_wgctrl.go`,
//! whose `Configure` was a hard-coded `return nil` and never created an
//! interface at all), every method here does the real thing:
//! [`KernelBackend::apply`] creates the interface if it is not already up
//! and configures its private key/address/MTU/peer;
//! [`KernelBackend::peer_stats`] reads the live device back over netlink;
//! [`KernelBackend::teardown`] removes the interface.
//!
//! `defguard_wireguard_rs::WGApi::new` does no I/O (it only stores the
//! interface name), so a fresh one is constructed per call rather than
//! stored — there is no meaningful state to hold between calls, and this
//! keeps every method a plain `&self` without an internal lock.

use std::net::IpAddr;

use async_trait::async_trait;
use defguard_wireguard_rs::error::WireguardInterfaceError;
use defguard_wireguard_rs::peer::Peer;
use defguard_wireguard_rs::{InterfaceConfiguration, Kernel, WGApi, WireguardInterfaceApi};

use super::{BackendKind, PeerStats, TunnelSpec, WgBackendError};

/// WireGuard-over-netlink backend. Holds no state — see module doc.
#[derive(Debug, Default, Clone, Copy)]
pub struct KernelBackend;

impl KernelBackend {
    /// Builds a new kernel backend. Cheap: performs no I/O.
    pub fn new() -> KernelBackend {
        KernelBackend
    }

    /// Opens a fresh netlink handle for `interface`. Never fails: matches
    /// `WGApi::new`, which only stores the name and cannot itself error on
    /// any real input.
    fn api(interface: &str) -> WGApi<Kernel> {
        WGApi::<Kernel>::new(interface.to_string())
            .expect("WGApi::new only stores the interface name and cannot fail")
    }
}

#[async_trait]
impl super::WireGuardBackend for KernelBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Kernel
    }

    /// Creates `interface` if [`WireguardInterfaceApi::read_interface_data`]
    /// shows it does not already exist — `defguard_wireguard_rs` creates
    /// with `NLM_F_EXCL`, which fails outright on an interface that is
    /// already up, so a `rotate` re-apply on an already-connected tunnel
    /// must skip creation and only reconfigure. Then applies the full
    /// interface configuration (private key, address, MTU, one peer) and,
    /// if `spec.dns` is non-empty, the interface's DNS servers. A DNS
    /// application failure is surfaced as a real error rather than
    /// swallowed — `dns` is part of the caller's request, and silently
    /// ignoring a failure to honor it is exactly the kind of silent
    /// failure this milestone's brief calls out Go for elsewhere.
    async fn apply(&self, interface: &str, spec: &TunnelSpec) -> Result<(), WgBackendError> {
        let mut api = Self::api(interface);

        if api.read_interface_data().is_err() {
            api.create_interface()
                .map_err(|err| WgBackendError::Interface(err.to_string()))?;
        }

        let mut peer = Peer::new(spec.peer_public_key.clone());
        peer.endpoint = Some(spec.endpoint);
        peer.allowed_ips = spec.allowed_ips.clone();
        if let Some(keepalive) = spec.keepalive {
            let seconds = keepalive.as_secs().min(u64::from(u16::MAX));
            peer.persistent_keepalive_interval = Some(seconds as u16);
        }

        let config = InterfaceConfiguration {
            name: interface.to_string(),
            prvkey: spec.private_key.to_string(),
            addresses: vec![spec.client_address.clone()],
            // 0 means "let the kernel pick a random source port" — the
            // netlink-level equivalent of Go's `ListenPort: nil`.
            port: 0,
            peers: vec![peer],
            mtu: Some(spec.mtu),
            fwmark: None,
        };

        api.configure_interface(&config)
            .map_err(|err| WgBackendError::Interface(err.to_string()))?;

        if !spec.dns.is_empty() {
            configure_dns(&api, &spec.dns)?;
        }

        Ok(())
    }

    /// Reads the live device back over netlink and returns the first (and,
    /// for this single-peer client tunnel, only) peer's stats. An absent
    /// interface, or one with no configured peer yet, reads as
    /// [`PeerStats::default`] — "not connected", not an error.
    async fn peer_stats(&self, interface: &str) -> Result<PeerStats, WgBackendError> {
        let api = Self::api(interface);
        let host = match api.read_interface_data() {
            Ok(host) => host,
            Err(_not_up_yet) => return Ok(PeerStats::default()),
        };
        let Some(peer) = host.peers.values().next() else {
            return Ok(PeerStats::default());
        };
        Ok(PeerStats {
            last_handshake: peer.last_handshake,
            rx_bytes: peer.rx_bytes,
            tx_bytes: peer.tx_bytes,
        })
    }

    /// Removes `interface`. Idempotent: an interface that is already gone
    /// (or never existed) is treated as successfully torn down.
    async fn teardown(&self, interface: &str) -> Result<(), WgBackendError> {
        let api = Self::api(interface);
        if api.read_interface_data().is_err() {
            return Ok(());
        }
        api.remove_interface()
            .map_err(|err| WgBackendError::Interface(err.to_string()))?;
        Ok(())
    }
}

/// Applies `dns` as the interface's DNS servers via `resolvconf` (what
/// `defguard_wireguard_rs::configure_dns` shells out to on Linux). No
/// search domains — the client has none to offer.
fn configure_dns(api: &WGApi<Kernel>, dns: &[IpAddr]) -> Result<(), WgBackendError> {
    api.configure_dns(dns, &[]).map_err(map_dns_error)
}

fn map_dns_error(err: WireguardInterfaceError) -> WgBackendError {
    WgBackendError::Interface(format!("configure DNS: {err}"))
}
