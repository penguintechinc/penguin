//! The pluggable platform-mechanics contract the state machine drives.

use std::net::IpAddr;

use async_trait::async_trait;

use crate::sysresolver::error::SysResolverError;

/// The host-mutating mechanics for one DNS-configuration mechanism
/// (`/etc/resolv.conf`, systemd-resolved, `networksetup`, `netsh`). The
/// cross-platform state machine in [`crate::sysresolver`] drives exactly
/// one of these per `apply()`/`restore()`/`current()` call; which one is
/// re-decided fresh every time (a backend that stops being available
/// between calls is simply skipped in favour of the next).
///
/// Every method takes `&self` (matches [`penguin_sdk`]-style modules
/// elsewhere in this workspace): backends hold no call-order state of
/// their own — the marker in [`crate::sysresolver::mod`] is the only place
/// "what did we last do" is tracked.
#[async_trait]
pub trait PlatformBackend: Send + Sync {
    /// A short, stable identifier (`"resolv.conf"`, `"systemd-resolved"`,
    /// `"networksetup"`, `"netsh"`), persisted in the crash marker so
    /// `restore`/`recover_from_crash` reuse the exact mechanism that
    /// applied the change, even if a different backend would be preferred
    /// by the time recovery runs.
    fn name(&self) -> &'static str;

    /// Read-only availability probe *and* durable pre-mutation backup, in
    /// one step: returns the servers currently configured (the marker's
    /// `previous_servers`), taking whatever byte-exact backup this backend
    /// needs internally before returning.
    ///
    /// An `Err` here means "skip this backend, try the next" and must
    /// never leave any host-visible state mutated. Called, and must fully
    /// complete — including any backend-local backup file write — strictly
    /// before the crash marker is written, so that once the marker is
    /// durable on disk, any backend-local backup [`Self::restore`] needs
    /// already exists too. This ordering is what closes the crash window
    /// Go left open; see [`crate::sysresolver`] module docs.
    async fn snapshot(&self) -> Result<Vec<IpAddr>, SysResolverError>;

    /// The actual host mutation: point the resolver at `servers`. Only
    /// ever called after `snapshot` succeeded and the crash marker
    /// recording how to undo it is already durable.
    async fn commit(&self, servers: &[IpAddr]) -> Result<(), SysResolverError>;

    /// Reverts this backend's changes. `fallback_servers` is the crash
    /// marker's bare server list, used when this backend has no better
    /// record of its own (e.g. the resolv.conf backend's byte-exact backup
    /// file has precedence over this parameter when present — see
    /// `linux::resolv_conf` — while systemd-resolved's link revert and the
    /// macOS/Windows DHCP reset ignore it entirely).
    async fn restore(&self, fallback_servers: &[IpAddr]) -> Result<(), SysResolverError>;

    /// Best-effort read of the servers this backend currently has active.
    async fn current(&self) -> Result<Vec<IpAddr>, SysResolverError>;
}
