//! Connects to the daemon's control socket the same way the CLI does
//! (`penguin_ipc`'s authenticated local transport), then probes it with the
//! `Version` RPC before handing off to a platform tray shell — which, once
//! started, owns the process for its entire lifetime, so a dead daemon must
//! be caught here rather than surfacing later as an opaque RPC error deep
//! inside a shell that has nowhere to report it.

use std::time::Duration;

use penguin_proto::daemon::v1 as pb;
use penguin_proto::daemon::v1::daemon_client::DaemonClient;
use tonic::transport::Channel;

/// The `api_version` every request this binary sends carries; unknown
/// versions are rejected by the daemon per the PenguinTech gRPC versioning
/// standard.
pub const API_VERSION: &str = "v1";

/// Deadline for both dialing the socket and the follow-up `Version` probe.
/// Matches the Go tray's `context.WithTimeout(context.Background(),
/// 3*time.Second)` (`go-client/cmd/penguin-tray/main.go`).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Why [`connect`] could not hand back a usable client.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// Dialing the socket did not complete within [`CONNECT_TIMEOUT`].
    #[error("dial timed out")]
    DialTimeout,
    /// The transport-level dial itself failed (e.g. nothing is listening).
    #[error("dial failed: {0}")]
    Dial(#[from] tonic::transport::Error),
    /// The probing `Version` call did not complete within [`CONNECT_TIMEOUT`].
    #[error("version probe timed out")]
    ProbeTimeout,
    /// The daemon answered but rejected (or failed) the `Version` call.
    #[error("version probe failed: {0}")]
    Probe(#[from] tonic::Status),
}

/// Dials the daemon's control socket and confirms it is actually alive with
/// a `Version` call, returning a ready-to-use client.
///
/// Connecting itself is eager, like `penguin_ipc::dial_unix::dial` — see
/// that function's own doc for why. The extra `Version` probe on top of
/// that exists because this binary is different from the CLI: once a
/// platform shell starts, it owns the process for good, so "is the daemon
/// actually there" needs answering once, up front, rather than discovered
/// later as a confusing failure inside the shell's event loop.
pub async fn connect(socket_path: &str) -> Result<DaemonClient<Channel>, ConnectError> {
    let channel = tokio::time::timeout(CONNECT_TIMEOUT, dial(socket_path))
        .await
        .map_err(|_elapsed| ConnectError::DialTimeout)??;
    let mut client = DaemonClient::new(channel);

    let request = pb::VersionRequest {
        api_version: API_VERSION.to_string(),
    };
    tokio::time::timeout(CONNECT_TIMEOUT, client.version(request))
        .await
        .map_err(|_elapsed| ConnectError::ProbeTimeout)??;

    Ok(client)
}

/// Opens the transport-level connection: a Unix-domain socket at
/// `socket_path` on Unix — Linux and macOS both take this path, the same
/// one the CLI uses.
#[cfg(unix)]
async fn dial(socket_path: &str) -> Result<Channel, tonic::transport::Error> {
    penguin_ipc::dial_unix::dial(socket_path).await
}

/// Windows equivalent of the Unix [`dial`]. `penguin_ipc::dial_windows`
/// speaks to a single well-known named pipe rather than a caller-supplied
/// path, so `socket_path` goes unused here — an existing limitation of that
/// module (see its own doc comment), not something introduced by this file.
#[cfg(windows)]
async fn dial(_socket_path: &str) -> Result<Channel, tonic::transport::Error> {
    penguin_ipc::dial_windows::dial().await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A regression test for this file's own timeout/error mapping —
    /// `penguin_ipc::dial_unix` already covers the underlying "nothing is
    /// listening" case for the bare dial; this proves `connect` surfaces
    /// that failure as a `ConnectError` rather than hanging or panicking.
    #[tokio::test]
    async fn connect_fails_when_nothing_is_listening() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let socket_path = temp_dir.path().join("nobody-listening.sock");

        let result = connect(socket_path.to_str().expect("utf8 path")).await;

        assert!(result.is_err());
    }
}
