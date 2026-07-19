//! Pure, injectable peer-authorization decision logic.
//!
//! Every rule that decides whether an IPC caller may drive the daemon lives
//! here as plain functions over an injectable [`GroupResolver`], so the
//! whole decision matrix is unit-testable without real sockets, real OS
//! users, or root privileges. The `listen_*` / `dial_*` / `groups_unix`
//! modules are thin OS adapters that call into this module — they carry no
//! decision logic of their own.

use crate::error::AuthError;

/// The identity of a connected IPC peer, as read off the transport
/// (`SO_PEERCRED` on Unix). Carried as plain data so the authorization rules
/// never need to touch the transport layer directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCredentials {
    /// The peer's user id.
    pub uid: u32,
    /// The peer's primary group id.
    pub gid: u32,
}

/// Resolves OS group membership on behalf of [`is_authorized`].
///
/// Implemented by `groups_unix::SystemGroups` for production and by a fake
/// struct in tests, so the authorization decision logic never depends on
/// the real group database.
pub trait GroupResolver: Send + Sync {
    /// Looks up a group by name, mirroring Go's `user.LookupGroup`. Returns
    /// `None` if the group does not exist.
    fn group_gid(&self, name: &str) -> Option<u32>;

    /// Looks up every group id (primary and supplementary) a uid belongs
    /// to, mirroring Go's `user.LookupId` followed by `User.GroupIds`.
    /// Returns `None` if the uid cannot be resolved.
    fn user_groups(&self, uid: u32) -> Option<Vec<u32>>;
}

/// Decides whether `creds` may drive a daemon running as `self_uid`. Rules
/// are applied in order; the first match wins:
///
/// 1. root (`uid == 0`) is always allowed.
/// 2. the daemon's own uid is always allowed (so an unprivileged developer
///    daemon is still usable by its own owner).
/// 3. an empty `allowed_group` denies everyone else.
/// 4. an unresolvable `allowed_group` denies everyone else — fail closed: an
///    absent group must never be silently treated as "allow all".
/// 5. a primary group match allows.
/// 6. an unresolvable peer uid denies.
/// 7. supplementary group membership allows.
/// 8. anything else is denied.
pub fn is_authorized(
    creds: PeerCredentials,
    self_uid: u32,
    allowed_group: &str,
    resolver: &dyn GroupResolver,
) -> bool {
    if creds.uid == 0 || creds.uid == self_uid {
        return true;
    }
    if allowed_group.is_empty() {
        return false;
    }
    let Some(allowed_gid) = resolver.group_gid(allowed_group) else {
        return false;
    };
    if creds.gid == allowed_gid {
        return true;
    }
    let Some(groups) = resolver.user_groups(creds.uid) else {
        return false;
    };
    groups.contains(&allowed_gid)
}

/// Full peer check for an incoming RPC: confirms peer credentials were even
/// present, then applies [`is_authorized`].
///
/// Kept separate from `is_authorized` because "no credentials at all" is a
/// transport-layer failure, not a verdict the authorization matrix itself
/// renders — `creds` is `None` here only when the caller (typically the
/// tonic interceptor in `listen_unix`) could not read peer credentials off
/// the connection in the first place.
pub fn check_peer(
    creds: Option<PeerCredentials>,
    self_uid: u32,
    allowed_group: &str,
    resolver: &dyn GroupResolver,
) -> Result<(), AuthError> {
    let Some(creds) = creds else {
        return Err(AuthError::NoPeerInfo);
    };
    if is_authorized(creds, self_uid, allowed_group, resolver) {
        return Ok(());
    }
    Err(AuthError::NotAuthorized {
        uid: creds.uid,
        gid: creds.gid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic, privilege-free stand-in for `groups_unix::SystemGroups`.
    /// Plain fields, no closures, so the whole authorization matrix is
    /// testable without touching the real group database.
    struct FakeResolver {
        groups: Vec<(String, u32)>,
        memberships: Vec<(u32, Vec<u32>)>,
    }

    impl GroupResolver for FakeResolver {
        fn group_gid(&self, name: &str) -> Option<u32> {
            for (group_name, gid) in &self.groups {
                if group_name == name {
                    return Some(*gid);
                }
            }
            None
        }

        fn user_groups(&self, uid: u32) -> Option<Vec<u32>> {
            for (member_uid, gids) in &self.memberships {
                if *member_uid == uid {
                    return Some(gids.clone());
                }
            }
            None
        }
    }

    fn empty_resolver() -> FakeResolver {
        FakeResolver {
            groups: Vec::new(),
            memberships: Vec::new(),
        }
    }

    #[test]
    fn root_is_always_authorized_even_with_empty_or_unknown_group() {
        let resolver = empty_resolver();
        let creds = PeerCredentials { uid: 0, gid: 0 };
        assert!(is_authorized(creds, 1000, "", &resolver));
        assert!(is_authorized(creds, 1000, "nonexistent", &resolver));
    }

    #[test]
    fn daemons_own_uid_is_authorized() {
        let resolver = empty_resolver();
        let creds = PeerCredentials {
            uid: 1000,
            gid: 1000,
        };
        assert!(is_authorized(creds, 1000, "penguin", &resolver));
    }

    #[test]
    fn non_root_non_self_with_empty_group_is_denied() {
        let resolver = empty_resolver();
        let creds = PeerCredentials {
            uid: 2000,
            gid: 2000,
        };
        assert!(!is_authorized(creds, 1000, "", &resolver));
    }

    #[test]
    fn primary_group_match_is_authorized() {
        let resolver = FakeResolver {
            groups: vec![(String::from("penguin"), 42)],
            memberships: Vec::new(),
        };
        let creds = PeerCredentials { uid: 2000, gid: 42 };
        assert!(is_authorized(creds, 1000, "penguin", &resolver));
    }

    #[test]
    fn unknown_group_is_denied_fail_closed() {
        let resolver = empty_resolver();
        let creds = PeerCredentials {
            uid: 2000,
            gid: 2000,
        };
        assert!(!is_authorized(creds, 1000, "nonexistent-group", &resolver));
    }

    #[test]
    fn supplementary_membership_is_authorized() {
        let resolver = FakeResolver {
            groups: vec![(String::from("penguin"), 42)],
            memberships: vec![(2000, vec![7, 42, 9])],
        };
        let creds = PeerCredentials { uid: 2000, gid: 99 };
        assert!(is_authorized(creds, 1000, "penguin", &resolver));
    }

    #[test]
    fn unresolvable_uid_is_denied() {
        let resolver = FakeResolver {
            groups: vec![(String::from("penguin"), 42)],
            memberships: Vec::new(),
        };
        let creds = PeerCredentials {
            uid: 999_999,
            gid: 99,
        };
        assert!(!is_authorized(creds, 1000, "penguin", &resolver));
    }

    #[test]
    fn non_root_non_self_with_no_group_match_is_denied() {
        let resolver = FakeResolver {
            groups: vec![(String::from("penguin"), 42)],
            memberships: vec![(2000, vec![7, 9])],
        };
        let creds = PeerCredentials { uid: 2000, gid: 99 };
        assert!(!is_authorized(creds, 1000, "penguin", &resolver));
    }

    #[test]
    fn check_peer_without_credentials_is_no_peer_info() {
        let resolver = empty_resolver();
        let err = check_peer(None, 1000, "penguin", &resolver).unwrap_err();
        assert_eq!(err, AuthError::NoPeerInfo);
    }

    #[test]
    fn check_peer_allows_authorized_peers() {
        let resolver = empty_resolver();
        let creds = PeerCredentials { uid: 0, gid: 0 };
        assert_eq!(check_peer(Some(creds), 1000, "penguin", &resolver), Ok(()));
    }

    #[test]
    fn check_peer_denies_with_the_exact_message() {
        let resolver = empty_resolver();
        let creds = PeerCredentials {
            uid: 2000,
            gid: 3000,
        };
        let err = check_peer(Some(creds), 1000, "", &resolver).unwrap_err();
        assert_eq!(err.to_string(), "peer uid 2000 (gid 3000) not authorized");
    }
}
