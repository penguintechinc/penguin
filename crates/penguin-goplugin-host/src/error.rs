//! The error type surfaced across the go-plugin host boundary.
//!
//! Every fallible step in loading a plugin — spawning the process, parsing
//! the handshake, standing up mTLS, connecting the gRPC channel, brokering
//! secondary connections, and shutting down — collapses into one flat
//! [`HostError`] so `client.rs` has a single type to propagate through its
//! orchestration. Module-lifecycle errors seen through the [`crate::adapter`]
//! boundary are a different type ([`penguin_sdk::ModuleError`]) by design:
//! that boundary only ever carries a message, matching what go-plugin's wire
//! format actually transmits.

use crate::handshake::HandshakeError;

/// Everything that can go wrong launching and speaking to a go-plugin binary.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The plugin process could not be spawned at all (bad path, permission
    /// denied, ...).
    #[error("failed to launch plugin process: {0}")]
    Spawn(std::io::Error),

    /// The plugin exited before printing a handshake line.
    #[error("plugin process exited before completing the handshake")]
    ExitedBeforeHandshake,

    /// No handshake line arrived within the 60s startup timeout.
    #[error("timed out waiting for plugin handshake")]
    HandshakeTimeout,

    /// A handshake line was read but failed to parse or validate.
    #[error("invalid plugin handshake: {0}")]
    Handshake(#[from] HandshakeError),

    /// Certificate generation or TLS configuration failed.
    #[error("mTLS setup failed: {0}")]
    Tls(String),

    /// Establishing the main gRPC channel to the plugin failed.
    #[error("failed to connect to plugin: {0}")]
    Connect(String),

    /// A `GRPCBroker` Accept/Dial operation failed.
    #[error("broker error: {0}")]
    Broker(String),

    /// The `grpc.health.v1` check never reported `SERVING`.
    #[error("plugin health check failed: {0}")]
    Health(String),

    /// A `GRPCController.Shutdown` call failed at the transport level.
    #[error("controller shutdown failed: {0}")]
    Shutdown(String),
}
