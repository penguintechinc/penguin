//! Platform-specific socket dial for connecting to penguind.
//!
//! This module wraps the OS-level socket connection logic. Unit tests
//! cannot inject a real penguind, so this boundary is excluded from
//! coverage. The `connect_internal(probe=true)` path is exercised
//! in integration tests.

use tonic::transport::Channel;

use crate::error::{DesktopError, Result};

/// Dials penguind over Unix domain socket (Linux/macOS).
#[cfg(unix)]
pub async fn dial_unix(socket_path: &str) -> Result<Channel> {
    use penguin_ipc::dial_unix::dial;

    dial(socket_path)
        .await
        .map_err(|e| DesktopError::IpcConnection(format!("dial failed: {}", e)))
}

/// Placeholder for Windows (not yet implemented).
#[cfg(windows)]
pub async fn dial_unix(socket_path: &str) -> Result<Channel> {
    // Windows uses dial_windows; desktop shell phase (2b) will implement this.
    Err(DesktopError::IpcConnection(
        "Windows desktop support not yet implemented".to_string(),
    ))
}
