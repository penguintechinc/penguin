//! NSS-aware Unix group resolution backing [`crate::authorize::GroupResolver`].
//!
//! `getgrouplist(3)` (wrapped safely by `nix`) consults whatever NSS sources
//! the host is configured with — LDAP, sssd, flat files, or otherwise —
//! matching the Go reference's cgo-backed `os/user`. A plain `/etc/group`
//! parse would silently diverge from that on any host that resolves groups
//! through something other than flat files. This module carries no
//! authorization decisions of its own; it is a thin adapter over the OS
//! group database for [`crate::authorize::is_authorized`] to call into.

use std::ffi::CString;

use nix::unistd::{Gid, Group, Uid, User, getgrouplist};

use crate::authorize::GroupResolver;

/// [`GroupResolver`] backed by the host's real NSS group/passwd databases.
pub struct SystemGroups;

impl GroupResolver for SystemGroups {
    fn group_gid(&self, name: &str) -> Option<u32> {
        let Ok(Some(group)) = Group::from_name(name) else {
            return None;
        };
        Some(group.gid.as_raw())
    }

    fn user_groups(&self, uid: u32) -> Option<Vec<u32>> {
        let Ok(Some(user)) = User::from_uid(Uid::from_raw(uid)) else {
            return None;
        };
        let Ok(name) = CString::new(user.name) else {
            return None;
        };
        let Ok(gids) = getgrouplist(&name, user.gid) else {
            return None;
        };
        Some(gids.into_iter().map(Gid::as_raw).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests run inside the pinned `penguin-rust:1.97` container, which
    // executes as root: uid 0, primary group "root" at gid 0. That makes
    // "root"/uid 0 safe, always-resolvable fixtures without depending on
    // the wider host's group database.

    #[test]
    fn root_group_resolves_to_gid_zero() {
        let resolver = SystemGroups;
        assert_eq!(resolver.group_gid("root"), Some(0));
    }

    #[test]
    fn unknown_group_resolves_to_none() {
        let resolver = SystemGroups;
        assert_eq!(resolver.group_gid("definitely-not-a-real-group-xyz"), None);
    }

    #[test]
    fn uid_zero_groups_include_its_primary_gid() {
        let resolver = SystemGroups;
        let groups = resolver.user_groups(0).expect("uid 0 resolves");
        assert!(groups.contains(&0));
    }

    #[test]
    fn unresolvable_uid_returns_none() {
        let resolver = SystemGroups;
        assert_eq!(resolver.user_groups(u32::MAX), None);
    }
}
