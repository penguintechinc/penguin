//! Error types this crate returns.
//!
//! Display strings here are exact and covered by tests: the daemon logs
//! them and the CLI surfaces them to operators, and both need to match the
//! frozen Go reference (`go-client/internal/ipc/listen_unix.go`) verbatim
//! for parity during the migration.

use thiserror::Error;

/// The error [`crate::authorize::check_peer`] returns when a peer must be
/// rejected.
///
/// `NoPeerInfo` and `NoPeerCredentials` are distinct because they describe
/// different transport-layer failures (no peer info attached to the request
/// at all, versus peer info attached but empty) that a caller — chiefly the
/// tonic interceptor in `listen_unix` — needs to be able to distinguish in
/// its own error mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AuthError {
    /// The request carried no peer information whatsoever (e.g. no
    /// `UdsConnectInfo` extension was ever attached to it).
    #[error("no peer info")]
    NoPeerInfo,
    /// Peer information was present, but it carried no credentials (e.g.
    /// `SO_PEERCRED` was unavailable for this connection).
    #[error("no peer credentials")]
    NoPeerCredentials,
    /// The peer was identified but is neither root, the daemon's own uid,
    /// nor a member (primary or supplementary) of the allowed group.
    #[error("peer uid {uid} (gid {gid}) not authorized")]
    NotAuthorized {
        /// The rejected peer's uid, for the error message and for logs.
        uid: u32,
        /// The rejected peer's gid, for the error message and for logs.
        gid: u32,
    },
}

/// The error [`crate::listen_unix::listen`] returns when it cannot stand up
/// the control socket.
///
/// Each variant wraps the underlying I/O failure with the setup step it
/// happened during, mirroring the four `fmt.Errorf("...: %w", err)` wraps in
/// the Go reference's `Listen`.
#[derive(Debug, Error)]
pub enum IpcError {
    /// The socket path exceeds the portable `sun_path` limit. Caught up
    /// front because `bind(2)` would otherwise fail deep in the kernel with
    /// a bare "invalid argument" that says nothing about why.
    #[error("socket path {path:?} is {len} bytes; the OS limit is {max} — use a shorter path")]
    PathTooLong {
        /// The rejected path, for the error message.
        path: String,
        /// The path's length in bytes.
        len: usize,
        /// The portable `sun_path` limit being enforced.
        max: usize,
    },
    /// Removing a stale socket left over from a previous run failed for a
    /// reason other than "it didn't exist".
    #[error("remove stale socket: {source}")]
    RemoveStale {
        /// The underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// Creating the socket's parent directory, or forcing its mode to
    /// `0750`, failed.
    #[error("mkdir parent: {source}")]
    MkdirParent {
        /// The underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// The `bind(2)` call itself failed.
    #[error("listen unix: {source}")]
    ListenUnix {
        /// The underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// Setting the socket's file mode after a successful bind failed.
    #[error("chmod socket: {source}")]
    ChmodSocket {
        /// The underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_error_display_strings_are_exact() {
        assert_eq!(AuthError::NoPeerInfo.to_string(), "no peer info");
        assert_eq!(
            AuthError::NoPeerCredentials.to_string(),
            "no peer credentials"
        );
        assert_eq!(
            AuthError::NotAuthorized { uid: 7, gid: 8 }.to_string(),
            "peer uid 7 (gid 8) not authorized"
        );
    }

    #[test]
    fn ipc_error_display_strings_are_exact() {
        let path_too_long = IpcError::PathTooLong {
            path: String::from("/run/penguind.sock"),
            len: 19,
            max: 103,
        };
        assert_eq!(
            path_too_long.to_string(),
            "socket path \"/run/penguind.sock\" is 19 bytes; the OS limit is 103 — use a shorter path"
        );

        let remove_stale = IpcError::RemoveStale {
            source: std::io::Error::other("boom"),
        };
        assert_eq!(remove_stale.to_string(), "remove stale socket: boom");

        let mkdir_parent = IpcError::MkdirParent {
            source: std::io::Error::other("boom"),
        };
        assert_eq!(mkdir_parent.to_string(), "mkdir parent: boom");

        let listen_unix = IpcError::ListenUnix {
            source: std::io::Error::other("boom"),
        };
        assert_eq!(listen_unix.to_string(), "listen unix: boom");

        let chmod_socket = IpcError::ChmodSocket {
            source: std::io::Error::other("boom"),
        };
        assert_eq!(chmod_socket.to_string(), "chmod socket: boom");
    }
}
