//! Enumerates the network links systemd-resolved DNS overrides apply to.
//!
//! Reads `/sys/class/net` directly — no `libc`, no `unsafe`, no extra
//! dependency — so the whole thing stays fake-free in tests via a simple
//! path seam, exactly like [`super::resolv_conf`]'s injectable
//! `resolv_conf_path` mirrors Go's package-level `resolvConfPath` var.

use std::fs;
use std::path::Path;

use crate::sysresolver::error::SysResolverError;

/// The kernel network-interface loopback name, always excluded: DNS
/// overrides on `lo` would not affect real traffic and systemd-resolved
/// does not manage it as a link.
const LOOPBACK: &str = "lo";

/// Production path for interface enumeration. Tests point
/// [`enumerate_links`] at a constructed tempdir tree instead.
pub const DEFAULT_SYSFS_NET_DIR: &str = "/sys/class/net";

/// One network interface: its kernel name and the `ifindex` systemd-resolved's
/// `SetLinkDNS`/`SetLinkDefaultRoute`/`RevertLink` calls key on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub name: String,
    pub index: i32,
}

/// Lists every interface under `sysfs_net_dir` except loopback, sorted by
/// name for deterministic apply/restore ordering.
pub fn enumerate_links(sysfs_net_dir: &Path) -> Result<Vec<Link>, SysResolverError> {
    let entries = fs::read_dir(sysfs_net_dir).map_err(|source| SysResolverError::Io {
        context: format!("list network interfaces in {}", sysfs_net_dir.display()),
        source,
    })?;

    let mut links = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| SysResolverError::Io {
            context: format!("read directory entry in {}", sysfs_net_dir.display()),
            source,
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == LOOPBACK {
            continue;
        }

        let index_path = entry.path().join("ifindex");
        let raw_index = fs::read_to_string(&index_path).map_err(|source| SysResolverError::Io {
            context: format!("read {}", index_path.display()),
            source,
        })?;
        let parsed: Result<i32, _> = raw_index.trim().parse();
        let Ok(index) = parsed else {
            return Err(SysResolverError::Backend(format!(
                "invalid ifindex content in {}",
                index_path.display()
            )));
        };

        links.push(Link { name, index });
    }

    links.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(links)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_interface(net_dir: &Path, name: &str, index: i32) {
        let iface_dir = net_dir.join(name);
        fs::create_dir_all(&iface_dir).expect("create fake interface dir");
        fs::write(iface_dir.join("ifindex"), format!("{index}\n")).expect("write fake ifindex");
    }

    #[test]
    fn enumerates_non_loopback_interfaces_sorted_by_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_interface(dir.path(), "lo", 1);
        seed_interface(dir.path(), "eth1", 3);
        seed_interface(dir.path(), "eth0", 2);

        let links = enumerate_links(dir.path()).expect("enumerate");

        assert_eq!(
            links,
            vec![
                Link {
                    name: "eth0".to_string(),
                    index: 2
                },
                Link {
                    name: "eth1".to_string(),
                    index: 3
                },
            ]
        );
    }

    #[test]
    fn empty_directory_yields_empty_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        let links = enumerate_links(dir.path()).expect("enumerate");
        assert!(links.is_empty());
    }

    #[test]
    fn only_loopback_present_yields_empty_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_interface(dir.path(), "lo", 1);
        let links = enumerate_links(dir.path()).expect("enumerate");
        assert!(links.is_empty());
    }

    #[test]
    fn missing_sysfs_dir_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist");
        let err = enumerate_links(&missing).expect_err("missing dir must error");
        assert!(matches!(err, SysResolverError::Io { .. }));
    }

    #[test]
    fn invalid_ifindex_content_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let iface_dir = dir.path().join("eth0");
        fs::create_dir_all(&iface_dir).expect("create fake interface dir");
        fs::write(iface_dir.join("ifindex"), b"not-a-number").expect("write bad ifindex");

        let err = enumerate_links(dir.path()).expect_err("invalid ifindex must error");
        assert!(matches!(err, SysResolverError::Backend(_)));
    }
}
