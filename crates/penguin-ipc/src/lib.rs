//! Local IPC transport for the daemon's control socket.
//!
//! Unix: a `0660` unix-domain socket inside a `0750` parent directory, with
//! every RPC re-checked against the peer's `SO_PEERCRED` identity. Windows: a
//! named pipe whose DACL is the sole authorization boundary — there is
//! deliberately no per-RPC peer check there, matching the Go reference's
//! platform asymmetry.
//!
//! The authorization *rules* live in [`authorize`] as pure functions over an
//! injectable [`authorize::GroupResolver`], so the entire decision matrix is
//! unit-testable with no sockets and no privileges. The `listen_*` / `dial_*`
//! modules are thin OS adapters carrying no decision logic, isolated into their
//! own files so the unit-coverage gate can exclude them the same way the Go
//! build excludes its boundary adapters.

pub mod authorize;
pub mod error;

#[cfg(unix)]
pub mod dial_unix;
#[cfg(unix)]
pub mod groups_unix;
#[cfg(unix)]
pub mod listen_unix;

#[cfg(windows)]
pub mod dial_windows;
#[cfg(windows)]
pub mod listen_windows;

pub use authorize::{GroupResolver, PeerCredentials, check_peer, is_authorized};
pub use error::{AuthError, IpcError};
