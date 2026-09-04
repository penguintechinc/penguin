//! The systemd-resolved backend: DNS overrides via
//! `org.freedesktop.resolve1.Manager` on the system D-Bus, applied to every
//! non-loopback link (mirrors the macOS/Windows backends looping over every
//! network service/interface — squawk has no single "the" interface to
//! target).
//!
//! Go's version of this file was a permanent stub that always returned an
//! error, so every apply/restore silently fell through to clobbering
//! `/etc/resolv.conf` — fighting the resolver that actually owns that file
//! on most modern distros. This is the real implementation.
//!
//! All D-Bus access sits behind [`Resolve1Client`]; [`SystemResolve1`] is
//! the only production implementation and the only place in this crate
//! that opens a bus connection. Every test uses [`FakeResolve1`] instead.

use std::net::IpAddr;
use std::path::PathBuf;

use async_trait::async_trait;

use crate::sysresolver::backend::PlatformBackend;
use crate::sysresolver::error::SysResolverError;

use super::links::{self, DEFAULT_SYSFS_NET_DIR};

/// Identifier persisted in the crash marker for this backend.
pub const BACKEND_NAME: &str = "systemd-resolved";

/// The well-known bus name systemd-resolved answers on — also the proxy's
/// `default_service` below, kept as a separate constant because
/// [`is_available`](Resolve1Client::is_available) needs it independently
/// for the `NameHasOwner` probe.
const SERVICE_NAME: &str = "org.freedesktop.resolve1";

/// The subset of `org.freedesktop.resolve1.Manager` sysresolver drives.
/// Implemented for real over the system bus via `zbus`
/// ([`SystemResolve1`]); every test injects [`FakeResolve1`], which
/// records calls and returns canned results — no test ever opens a bus
/// connection.
#[async_trait]
pub trait Resolve1Client: Send + Sync {
    /// True if resolved answers on the system bus right now. Read-only:
    /// used both to decide whether to prefer this backend over
    /// `resolv.conf` and as `snapshot`'s availability probe. Treating "is
    /// reachable on the bus" as "owns `/etc/resolv.conf`" is a deliberate
    /// simplification — on every distro that ships systemd-resolved
    /// enabled, that is also the convention it follows (see the module
    /// docs for the fuller reasoning).
    async fn is_available(&self) -> bool;

    /// `org.freedesktop.resolve1.Manager.SetLinkDNS`.
    async fn set_link_dns(&self, ifindex: i32, servers: &[IpAddr]) -> Result<(), SysResolverError>;

    /// `org.freedesktop.resolve1.Manager.SetLinkDefaultRoute`.
    async fn set_link_default_route(
        &self,
        ifindex: i32,
        enable: bool,
    ) -> Result<(), SysResolverError>;

    /// `org.freedesktop.resolve1.Manager.RevertLink` — undoes every
    /// `SetLink*` override for that link in one call.
    async fn revert_link(&self, ifindex: i32) -> Result<(), SysResolverError>;
}

#[zbus::proxy(
    interface = "org.freedesktop.resolve1.Manager",
    default_service = "org.freedesktop.resolve1",
    default_path = "/org/freedesktop/resolve1",
    gen_blocking = false
)]
trait Resolve1Manager {
    fn set_link_dns(&self, ifindex: i32, addresses: Vec<(i32, Vec<u8>)>) -> zbus::Result<()>;
    fn set_link_default_route(&self, ifindex: i32, enable: bool) -> zbus::Result<()>;
    fn revert_link(&self, ifindex: i32) -> zbus::Result<()>;
}

/// The real, D-Bus-backed [`Resolve1Client`]. Connects lazily on first use
/// (constructing one does no I/O) and reuses the connection afterward.
#[derive(Default)]
pub struct SystemResolve1 {
    connection: tokio::sync::OnceCell<zbus::Connection>,
}

impl SystemResolve1 {
    /// A not-yet-connected client. Building one never touches the bus.
    pub fn new() -> SystemResolve1 {
        SystemResolve1::default()
    }

    async fn connection(&self) -> Result<&zbus::Connection, SysResolverError> {
        self.connection.get_or_try_init(connect_system_bus).await
    }

    async fn proxy(&self) -> Result<Resolve1ManagerProxy<'_>, SysResolverError> {
        let connection = self.connection().await?;
        Resolve1ManagerProxy::new(connection)
            .await
            .map_err(proxy_build_error)
    }
}

async fn connect_system_bus() -> Result<zbus::Connection, SysResolverError> {
    zbus::Connection::system()
        .await
        .map_err(|source| SysResolverError::Backend(format!("connect to system D-Bus: {source}")))
}

fn proxy_build_error(source: zbus::Error) -> SysResolverError {
    SysResolverError::Backend(format!("build resolve1 proxy: {source}"))
}

#[async_trait]
impl Resolve1Client for SystemResolve1 {
    async fn is_available(&self) -> bool {
        let connection = self.connection().await;
        let Ok(connection) = connection else {
            return false;
        };
        let dbus = zbus::fdo::DBusProxy::new(connection).await;
        let Ok(dbus) = dbus else {
            return false;
        };
        let service_name: Result<zbus::names::BusName, _> = SERVICE_NAME.try_into();
        let Ok(service_name) = service_name else {
            return false;
        };
        matches!(dbus.name_has_owner(service_name).await, Ok(true))
    }

    async fn set_link_dns(&self, ifindex: i32, servers: &[IpAddr]) -> Result<(), SysResolverError> {
        let proxy = self.proxy().await?;
        let mut addresses = Vec::with_capacity(servers.len());
        for server in servers {
            addresses.push(to_family_bytes(*server));
        }
        proxy
            .set_link_dns(ifindex, addresses)
            .await
            .map_err(|source| SysResolverError::Backend(format!("SetLinkDNS: {source}")))
    }

    async fn set_link_default_route(
        &self,
        ifindex: i32,
        enable: bool,
    ) -> Result<(), SysResolverError> {
        let proxy = self.proxy().await?;
        proxy
            .set_link_default_route(ifindex, enable)
            .await
            .map_err(|source| SysResolverError::Backend(format!("SetLinkDefaultRoute: {source}")))
    }

    async fn revert_link(&self, ifindex: i32) -> Result<(), SysResolverError> {
        let proxy = self.proxy().await?;
        proxy
            .revert_link(ifindex)
            .await
            .map_err(|source| SysResolverError::Backend(format!("RevertLink: {source}")))
    }
}

/// Converts an address into resolve1's `(family, bytes)` wire shape for
/// `a(iay)` — `AF_INET`/`AF_INET6` per the Linux socket ABI (this module
/// only ever runs on Linux, so these numeric values are fixed).
fn to_family_bytes(addr: IpAddr) -> (i32, Vec<u8>) {
    const AF_INET: i32 = 2;
    const AF_INET6: i32 = 10;
    match addr {
        IpAddr::V4(v4) => (AF_INET, v4.octets().to_vec()),
        IpAddr::V6(v6) => (AF_INET6, v6.octets().to_vec()),
    }
}

/// The [`PlatformBackend`] that drives [`Resolve1Client`] across every
/// non-loopback link, enumerated via an injectable sysfs path (see
/// [`super::links`]).
pub struct ResolvedBackend {
    client: Box<dyn Resolve1Client>,
    sysfs_net_dir: PathBuf,
}

impl ResolvedBackend {
    /// Production code passes [`SystemResolve1`] and
    /// [`DEFAULT_SYSFS_NET_DIR`]; tests pass [`FakeResolve1`] and a
    /// constructed tempdir tree.
    pub fn new(client: Box<dyn Resolve1Client>, sysfs_net_dir: PathBuf) -> ResolvedBackend {
        ResolvedBackend {
            client,
            sysfs_net_dir,
        }
    }

    /// Production constructor: real D-Bus client, real sysfs path.
    pub fn system() -> ResolvedBackend {
        ResolvedBackend::new(
            Box::new(SystemResolve1::new()),
            PathBuf::from(DEFAULT_SYSFS_NET_DIR),
        )
    }
}

#[async_trait]
impl PlatformBackend for ResolvedBackend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn snapshot(&self) -> Result<Vec<IpAddr>, SysResolverError> {
        if !self.client.is_available().await {
            return Err(SysResolverError::Backend(
                "systemd-resolved is not reachable on the system bus".to_string(),
            ));
        }
        // resolve1's Manager interface exposes no "current DNS for this
        // link" call among Set*/Revert (the only ones this port targets),
        // so there is nothing to read back here — an empty previous list
        // is the honest best-effort answer, exactly like Go logging a
        // warning and proceeding with an empty `PreviousServers` when its
        // own `Current()` read fails.
        Ok(Vec::new())
    }

    async fn commit(&self, servers: &[IpAddr]) -> Result<(), SysResolverError> {
        let targets = links::enumerate_links(&self.sysfs_net_dir)?;
        if targets.is_empty() {
            return Err(SysResolverError::Backend(
                "no network links found".to_string(),
            ));
        }
        for link in &targets {
            self.client.set_link_dns(link.index, servers).await?;
            self.client.set_link_default_route(link.index, true).await?;
        }
        Ok(())
    }

    async fn restore(&self, _fallback_servers: &[IpAddr]) -> Result<(), SysResolverError> {
        let targets = links::enumerate_links(&self.sysfs_net_dir)?;
        for link in &targets {
            self.client.revert_link(link.index).await?;
        }
        Ok(())
    }

    async fn current(&self) -> Result<Vec<IpAddr>, SysResolverError> {
        // Same gap as `snapshot`: no read-back call in scope for this
        // port. The state machine falls through to the resolv.conf
        // backend's `current()`, which on a resolved-managed host usually
        // still reflects reality (`/etc/resolv.conf` is typically the
        // 127.0.0.53 stub resolver pointer).
        Err(SysResolverError::Backend(
            "systemd-resolved current-servers query not supported".to_string(),
        ))
    }
}

/// Test double for [`Resolve1Client`]: records every call and returns
/// caller-configured results. Never opens a bus connection.
#[cfg(test)]
pub struct FakeResolve1 {
    pub available: std::sync::atomic::AtomicBool,
    pub calls: std::sync::Mutex<Vec<String>>,
    pub fail_set_link_dns: std::sync::atomic::AtomicBool,
    pub fail_revert_link: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
impl FakeResolve1 {
    pub fn new(available: bool) -> FakeResolve1 {
        FakeResolve1 {
            available: std::sync::atomic::AtomicBool::new(available),
            calls: std::sync::Mutex::new(Vec::new()),
            fail_set_link_dns: std::sync::atomic::AtomicBool::new(false),
            fail_revert_link: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn record(&self, call: String) {
        self.calls.lock().expect("fake mutex poisoned").push(call);
    }
}

#[cfg(test)]
#[async_trait]
impl Resolve1Client for FakeResolve1 {
    async fn is_available(&self) -> bool {
        self.available.load(std::sync::atomic::Ordering::SeqCst)
    }

    async fn set_link_dns(&self, ifindex: i32, servers: &[IpAddr]) -> Result<(), SysResolverError> {
        self.record(format!("set_link_dns({ifindex}, {servers:?})"));
        if self
            .fail_set_link_dns
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(SysResolverError::Backend(
                "fake SetLinkDNS failure".to_string(),
            ));
        }
        Ok(())
    }

    async fn set_link_default_route(
        &self,
        ifindex: i32,
        enable: bool,
    ) -> Result<(), SysResolverError> {
        self.record(format!("set_link_default_route({ifindex}, {enable})"));
        Ok(())
    }

    async fn revert_link(&self, ifindex: i32) -> Result<(), SysResolverError> {
        self.record(format!("revert_link({ifindex})"));
        if self
            .fail_revert_link
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(SysResolverError::Backend(
                "fake RevertLink failure".to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> IpAddr {
        s.parse().expect("valid test address")
    }

    fn seed_interface(net_dir: &std::path::Path, name: &str, index: i32) {
        let iface_dir = net_dir.join(name);
        std::fs::create_dir_all(&iface_dir).expect("create fake interface dir");
        std::fs::write(iface_dir.join("ifindex"), format!("{index}\n"))
            .expect("write fake ifindex");
    }

    #[test]
    fn family_bytes_are_af_inet_and_af_inet6() {
        let (family, bytes) = to_family_bytes(addr("1.2.3.4"));
        assert_eq!(family, 2);
        assert_eq!(bytes, vec![1, 2, 3, 4]);

        let (family, bytes) = to_family_bytes(addr("::1"));
        assert_eq!(family, 10);
        assert_eq!(bytes.len(), 16);
    }

    #[tokio::test]
    async fn snapshot_fails_fast_when_resolved_unavailable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend =
            ResolvedBackend::new(Box::new(FakeResolve1::new(false)), dir.path().to_path_buf());

        let err = backend
            .snapshot()
            .await
            .expect_err("unavailable resolved must error");
        assert!(matches!(err, SysResolverError::Backend(_)));
    }

    #[tokio::test]
    async fn commit_sets_dns_and_default_route_on_every_link() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_interface(dir.path(), "lo", 1);
        seed_interface(dir.path(), "eth0", 2);
        seed_interface(dir.path(), "wg0", 5);

        let fake = std::sync::Arc::new(FakeResolve1::new(true));
        let backend =
            ResolvedBackend::new(Box::new(SharedFake(fake.clone())), dir.path().to_path_buf());

        backend.commit(&[addr("127.0.0.1")]).await.expect("commit");

        let calls = fake.calls.lock().expect("lock");
        assert_eq!(
            calls.len(),
            4,
            "two links x (SetLinkDNS + SetLinkDefaultRoute)"
        );
        assert!(calls.iter().any(|c| c.starts_with("set_link_dns(2,")));
        assert!(calls.iter().any(|c| c.starts_with("set_link_dns(5,")));
        assert!(
            !calls.iter().any(|c| c.contains("(1,")),
            "loopback must be skipped"
        );
    }

    #[tokio::test]
    async fn restore_reverts_every_link() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_interface(dir.path(), "eth0", 2);

        let fake = std::sync::Arc::new(FakeResolve1::new(true));
        let backend =
            ResolvedBackend::new(Box::new(SharedFake(fake.clone())), dir.path().to_path_buf());

        backend.restore(&[]).await.expect("restore");

        let calls = fake.calls.lock().expect("lock");
        assert_eq!(calls.as_slice(), &["revert_link(2)".to_string()]);
    }

    #[tokio::test]
    async fn commit_with_no_links_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend =
            ResolvedBackend::new(Box::new(FakeResolve1::new(true)), dir.path().to_path_buf());

        let err = backend
            .commit(&[addr("127.0.0.1")])
            .await
            .expect_err("no links must error");
        assert!(matches!(err, SysResolverError::Backend(_)));
    }

    /// Lets one `Arc<FakeResolve1>` be both retained by the test (to
    /// inspect recorded calls) and boxed into the backend, without a
    /// second trait implementation.
    struct SharedFake(std::sync::Arc<FakeResolve1>);

    #[async_trait]
    impl Resolve1Client for SharedFake {
        async fn is_available(&self) -> bool {
            self.0.is_available().await
        }
        async fn set_link_dns(
            &self,
            ifindex: i32,
            servers: &[IpAddr],
        ) -> Result<(), SysResolverError> {
            self.0.set_link_dns(ifindex, servers).await
        }
        async fn set_link_default_route(
            &self,
            ifindex: i32,
            enable: bool,
        ) -> Result<(), SysResolverError> {
            self.0.set_link_default_route(ifindex, enable).await
        }
        async fn revert_link(&self, ifindex: i32) -> Result<(), SysResolverError> {
            self.0.revert_link(ifindex).await
        }
    }
}
