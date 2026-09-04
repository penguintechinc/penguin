//! Linux platform mechanics: systemd-resolved (preferred, over D-Bus) with
//! `/etc/resolv.conf` as the fallback used only when resolved is genuinely
//! absent.

pub mod links;
pub mod resolv_conf;
pub mod resolved;

use std::path::{Path, PathBuf};

use crate::sysresolver::backend::PlatformBackend;

/// Builds the Linux backend list for [`super::SysResolver::new`], in
/// priority order: systemd-resolved first, `/etc/resolv.conf` as the
/// universal fallback. [`super::SysResolver::apply`] tries each in turn via
/// `snapshot()`, so "prefer resolved, fall back only when genuinely
/// absent" falls out of that ordering rather than needing a separate
/// detection step here.
pub fn build_backends(data_dir: &Path) -> Vec<Box<dyn PlatformBackend>> {
    let resolved: Box<dyn PlatformBackend> = Box::new(resolved::ResolvedBackend::system());
    let resolv_conf: Box<dyn PlatformBackend> = Box::new(resolv_conf::ResolvConfBackend::new(
        PathBuf::from(resolv_conf::DEFAULT_RESOLV_CONF_PATH),
        data_dir,
    ));
    vec![resolved, resolv_conf]
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::resolv_conf::ResolvConfBackend;
    use super::resolved::{FakeResolve1, ResolvedBackend};
    use crate::sysresolver::SysResolver;

    fn addr(s: &str) -> IpAddr {
        s.parse().expect("valid test address")
    }

    fn seed_interface(net_dir: &std::path::Path, name: &str, index: i32) {
        let iface_dir = net_dir.join(name);
        std::fs::create_dir_all(&iface_dir).expect("create fake interface dir");
        std::fs::write(iface_dir.join("ifindex"), format!("{index}\n"))
            .expect("write fake ifindex");
    }

    /// Wires the real [`ResolvedBackend`] to a [`FakeResolve1`] reporting
    /// resolved as available, and a real [`ResolvConfBackend`] pointed at a
    /// tempdir file. `apply()` must pick systemd-resolved: the resolved
    /// fake never errors, so selection never falls through to the second
    /// backend in the list.
    #[tokio::test]
    async fn apply_chooses_systemd_resolved_when_available() {
        let data_dir = tempfile::tempdir().expect("tempdir");
        let net_dir = tempfile::tempdir().expect("tempdir");
        seed_interface(net_dir.path(), "eth0", 2);
        let resolv_conf_path = data_dir.path().join("resolv.conf"); // never touched if resolved wins

        let resolved: Box<dyn crate::sysresolver::PlatformBackend> =
            Box::new(ResolvedBackend::new(
                Box::new(FakeResolve1::new(true)),
                net_dir.path().to_path_buf(),
            ));
        let resolv_conf: Box<dyn crate::sysresolver::PlatformBackend> = Box::new(
            ResolvConfBackend::new(resolv_conf_path.clone(), data_dir.path()),
        );

        let resolver =
            SysResolver::with_backends(data_dir.path().to_path_buf(), vec![resolved, resolv_conf]);
        resolver.apply(&[addr("127.0.0.1")]).await.expect("apply");

        assert!(
            !resolv_conf_path.exists(),
            "resolv.conf must be untouched when systemd-resolved handled the apply"
        );
    }

    /// Same wiring, but the fake reports resolved as unavailable —
    /// selection must fall through to the resolv.conf backend, and the
    /// live file must actually change.
    #[tokio::test]
    async fn apply_falls_back_to_resolv_conf_when_resolved_unavailable() {
        let data_dir = tempfile::tempdir().expect("tempdir");
        let net_dir = tempfile::tempdir().expect("tempdir");
        let resolv_conf_path = data_dir.path().join("resolv.conf");
        std::fs::write(&resolv_conf_path, "nameserver 8.8.8.8\n").expect("seed resolv.conf");

        let resolved: Box<dyn crate::sysresolver::PlatformBackend> =
            Box::new(ResolvedBackend::new(
                Box::new(FakeResolve1::new(false)),
                net_dir.path().to_path_buf(),
            ));
        let resolv_conf: Box<dyn crate::sysresolver::PlatformBackend> = Box::new(
            ResolvConfBackend::new(resolv_conf_path.clone(), data_dir.path()),
        );

        let resolver =
            SysResolver::with_backends(data_dir.path().to_path_buf(), vec![resolved, resolv_conf]);
        resolver.apply(&[addr("1.1.1.1")]).await.expect("apply");

        let content = std::fs::read_to_string(&resolv_conf_path).expect("read");
        assert!(content.contains("nameserver 1.1.1.1"));
    }

    /// `build_backends` itself must not panic and must produce the
    /// documented priority order, using the real sysfs default path
    /// (harmless to reference — it is never read unless `commit`/`restore`
    /// actually runs, which this test does not trigger).
    #[test]
    fn build_backends_orders_resolved_before_resolv_conf() {
        let data_dir = tempfile::tempdir().expect("tempdir");
        let backends = super::build_backends(data_dir.path());
        assert_eq!(backends.len(), 2);
        assert_eq!(backends[0].name(), "systemd-resolved");
        assert_eq!(backends[1].name(), "resolv.conf");
    }
}
