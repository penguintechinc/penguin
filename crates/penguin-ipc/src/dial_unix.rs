//! Client-side connector to the daemon's Unix-domain control socket.

use std::path::Path;

use tonic::transport::{Channel, Endpoint, Error};

/// Connects to the daemon's control socket at `path`.
///
/// This connects eagerly — a deliberate improvement over the frozen Go
/// reference (`go-client/internal/ipc/dial_unix.go`), not strict parity
/// with it. The Go side uses `grpc.NewClient`, which lazily connects on the
/// first RPC; that means "is penguind running?" essentially never fails at
/// dial time, and a dead daemon instead surfaces later as an opaque error
/// on whatever RPC the caller happens to make first. Connecting eagerly
/// here means a dead daemon is detected right here, at the call that is
/// actually asking "can I reach the daemon?"
///
/// The transport carries no TLS: the authorization boundary is the OS
/// (socket permissions plus the per-RPC `SO_PEERCRED` check performed by
/// `listen_unix::PeerAuthInterceptor`), not a certificate.
pub async fn dial(path: impl AsRef<Path>) -> Result<Channel, Error> {
    let uri = format!("unix://{}", path.as_ref().display());
    Endpoint::from_shared(uri)?.connect().await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Demonstrates the deliberate divergence documented on `dial`: an
    /// eager connect fails immediately against a socket path nothing is
    /// listening on, rather than only surfacing the failure on a later RPC.
    #[tokio::test]
    async fn dial_fails_eagerly_when_nothing_is_listening() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let socket_path = temp_dir.path().join("nobody-listening.sock");

        let result = dial(&socket_path).await;

        assert!(result.is_err());
    }
}
