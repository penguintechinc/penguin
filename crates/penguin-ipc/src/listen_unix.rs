//! Unix-domain socket listener for the daemon's control socket.
//!
//! The socket ends up `0660` inside a `0750` parent directory: only root
//! and members of the configured group can even traverse to the socket
//! path, and every RPC is independently re-checked against the connecting
//! peer's `SO_PEERCRED` identity via [`crate::authorize::check_peer`] (see
//! [`PeerAuthInterceptor`]).
//!
//! There is a brief window between `bind(2)` succeeding and the follow-up
//! `chmod` landing during which the socket carries whatever default mode
//! the kernel applied at creation time (typically `0777` minus umask) —
//! wider than the `0660` we want. That window is safe for two independent
//! reasons: the parent directory is already `0750` before the bind ever
//! happens, so nothing outside root/the owning group can traverse into the
//! directory to reach the socket path during the window in the first
//! place; and even a caller who somehow did connect during the window gets
//! no special treatment, because every RPC — not just the initial connect
//! — is re-authorized independently against the peer's live credentials.
//! Fixing the window with a process-wide `umask` call instead was
//! considered and rejected: umask is process-global and not thread-safe, so
//! it would leak into every other file this process creates concurrently on
//! another thread, for the sake of narrowing a window that is already safe.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::service::Interceptor;
use tonic::transport::server::UdsConnectInfo;
use tonic::{Request, Status};

use crate::authorize::{GroupResolver, PeerCredentials, check_peer};
use crate::error::{AuthError, IpcError};

/// Portable minimum of the `sun_path` limit across the platforms this crate
/// targets (Linux 108, Darwin 104), minus the trailing NUL. `bind(2)` fails
/// deep in the kernel with a bare "invalid argument" past this, so it is
/// checked up front, before touching the filesystem, where the error can
/// say what is actually wrong.
const MAX_UNIX_PATH: usize = 103;

/// Configuration for [`listen`].
pub struct ListenerConfig {
    /// Filesystem path for the control socket.
    pub path: PathBuf,
    /// OS group name (besides root and the daemon's own uid) allowed to
    /// connect.
    pub allowed_group: String,
}

/// Binds the daemon's control socket at `cfg.path`, preparing its parent
/// directory and permissions along the way. See the module doc for the
/// bind-then-chmod safety argument covering the brief gap between steps.
///
/// Must be called from within a running Tokio runtime (`UnixListener::bind`
/// registers the socket with the async reactor).
pub fn listen(cfg: &ListenerConfig) -> Result<UnixListener, IpcError> {
    let path_len = cfg.path.as_os_str().len();
    if path_len > MAX_UNIX_PATH {
        return Err(IpcError::PathTooLong {
            path: cfg.path.display().to_string(),
            len: path_len,
            max: MAX_UNIX_PATH,
        });
    }

    remove_stale_socket(&cfg.path)?;
    prepare_parent_dir(&cfg.path)?;

    let listener =
        UnixListener::bind(&cfg.path).map_err(|source| IpcError::ListenUnix { source })?;

    if let Err(source) = fs::set_permissions(&cfg.path, fs::Permissions::from_mode(0o660)) {
        drop(listener);
        return Err(IpcError::ChmodSocket { source });
    }

    Ok(listener)
}

/// Removes a stale socket (or file) left at `path` by a previous run. A
/// missing path is not an error; anything else is.
fn remove_stale_socket(path: &Path) -> Result<(), IpcError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(IpcError::RemoveStale { source }),
    }
}

/// Creates the socket's parent directory if needed and forces its mode to
/// `0750`, regardless of the process umask (see the module doc for why
/// umask itself is not used to achieve this).
fn prepare_parent_dir(path: &Path) -> Result<(), IpcError> {
    let parent = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    fs::create_dir_all(parent).map_err(|source| IpcError::MkdirParent { source })?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o750))
        .map_err(|source| IpcError::MkdirParent { source })
}

/// Wraps a bound listener as the incoming-connection stream tonic's
/// `Server::serve_with_incoming` (or `..._shutdown`) expects.
pub fn incoming(listener: UnixListener) -> UnixListenerStream {
    UnixListenerStream::new(listener)
}

/// Per-RPC peer authorization for tonic servers built on [`listen`]'s
/// socket. Every RPC is re-checked independently — see the module doc for
/// why that matters beyond just the bind-then-chmod window: it also means
/// authorization tracks the peer's live group membership rather than a
/// snapshot taken once at accept time.
#[derive(Clone)]
pub struct PeerAuthInterceptor {
    self_uid: u32,
    allowed_group: String,
    resolver: Arc<dyn GroupResolver>,
}

impl PeerAuthInterceptor {
    /// Builds an interceptor that authorizes callers against
    /// `allowed_group` (plus root and `self_uid`) using `resolver` for
    /// group lookups.
    pub fn new(
        self_uid: u32,
        allowed_group: impl Into<String>,
        resolver: Arc<dyn GroupResolver>,
    ) -> Self {
        PeerAuthInterceptor {
            self_uid,
            allowed_group: allowed_group.into(),
            resolver,
        }
    }
}

impl Interceptor for PeerAuthInterceptor {
    /// Fails closed: any request this cannot positively authorize is
    /// rejected, never passed through. `UdsConnectInfo` being altogether
    /// absent and it being present-but-empty are distinguished so callers
    /// see the right one of "no peer info" / "no peer credentials" —
    /// `check_peer`'s `Option<PeerCredentials>` only has one "missing"
    /// state, so that distinction is made here rather than inside it.
    fn call(&mut self, request: Request<()>) -> Result<Request<()>, Status> {
        let Some(connect_info) = request.extensions().get::<UdsConnectInfo>() else {
            return Err(Status::unauthenticated(AuthError::NoPeerInfo.to_string()));
        };
        let Some(peer_cred) = connect_info.peer_cred.as_ref() else {
            return Err(Status::unauthenticated(
                AuthError::NoPeerCredentials.to_string(),
            ));
        };
        let creds = PeerCredentials {
            uid: peer_cred.uid(),
            gid: peer_cred.gid(),
        };
        if let Err(err) = check_peer(
            Some(creds),
            self.self_uid,
            &self.allowed_group,
            self.resolver.as_ref(),
        ) {
            return Err(Status::permission_denied(err.to_string()));
        }
        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::FileTypeExt;

    use super::*;

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    fn config(path: PathBuf) -> ListenerConfig {
        ListenerConfig {
            path,
            allowed_group: String::from("penguin"),
        }
    }

    #[tokio::test]
    async fn socket_gets_mode_0660() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let socket_path = temp_dir.path().join("test.sock");

        let listener = listen(&config(socket_path.clone())).expect("listen");
        assert_eq!(mode_of(&socket_path), 0o660);
        drop(listener);
    }

    #[tokio::test]
    async fn parent_dir_gets_mode_0750() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let socket_path = temp_dir.path().join("run").join("test.sock");

        let listener = listen(&config(socket_path.clone())).expect("listen");
        assert_eq!(mode_of(socket_path.parent().expect("has parent")), 0o750);
        drop(listener);
    }

    #[tokio::test]
    async fn stale_regular_file_at_the_path_is_replaced() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let socket_path = temp_dir.path().join("stale.sock");
        fs::write(&socket_path, b"stale").expect("write stale file");

        let listener = listen(&config(socket_path.clone())).expect("listen");

        let file_type = fs::metadata(&socket_path).expect("metadata").file_type();
        assert!(file_type.is_socket());
        drop(listener);
    }

    #[tokio::test]
    async fn stale_socket_from_a_previous_listener_is_replaced() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let socket_path = temp_dir.path().join("reuse.sock");

        let first = listen(&config(socket_path.clone())).expect("first listen");
        drop(first);

        let second = listen(&config(socket_path));
        assert!(second.is_ok());
    }

    #[tokio::test]
    async fn path_over_the_limit_errors_before_touching_the_fs() {
        let long_path = PathBuf::from(format!("/{}", "a".repeat(MAX_UNIX_PATH + 10)));

        let Err(err) = listen(&config(long_path)) else {
            panic!("expected an error for an over-limit path");
        };
        let IpcError::PathTooLong { .. } = err else {
            panic!("expected PathTooLong, got {err:?}");
        };
    }

    #[tokio::test]
    async fn path_exactly_at_the_limit_succeeds() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let base_len = temp_dir.path().as_os_str().len();
        // -1 accounts for the path separator `join` inserts.
        let Some(filler_len) = MAX_UNIX_PATH.checked_sub(base_len + 1) else {
            // The container's temp dir is unusually long; nothing to test.
            return;
        };
        let socket_path = temp_dir.path().join("a".repeat(filler_len));
        assert_eq!(socket_path.as_os_str().len(), MAX_UNIX_PATH);

        let listener = listen(&config(socket_path));
        assert!(listener.is_ok());
    }
}
