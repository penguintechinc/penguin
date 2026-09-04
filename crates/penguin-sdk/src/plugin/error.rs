//! The error type the `plugin` module's internals use.
//!
//! Two very different failure classes share this enum, matching the pattern
//! `penguin-goplugin-host::HostError` already uses on the other side of the
//! wire: startup errors ([`serve::serve`] cannot recover from a failed
//! listener bind or certificate generation, so these are logged and the
//! process exits) and best-effort errors from the broker/`HostServices` leg
//! (these are always caught and degrade to [`crate::plugin::hostservices::NoopHostServices`]
//! rather than aborting the plugin — see that module's doc comment).

/// Everything that can go wrong inside the plugin-side go-plugin runtime.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    /// Certificate generation or TLS configuration failed.
    #[error("mTLS setup failed: {0}")]
    Tls(String),

    /// The `PLUGIN_CLIENT_CERT` environment variable was missing or not
    /// valid PEM.
    #[error("invalid PLUGIN_CLIENT_CERT: {0}")]
    HostCert(String),

    /// A private temp directory or unix socket could not be created.
    #[error("failed to create plugin listener: {0}")]
    Listener(String),

    /// Writing or flushing the handshake line to stdout failed.
    #[error("failed to write handshake line: {0}")]
    Handshake(String),

    /// A `GRPCBroker` accept/dial operation failed.
    #[error("broker error: {0}")]
    Broker(String),

    /// Connecting to the host's `HostService` (over the broker's id=1 leg)
    /// failed.
    #[error("failed to connect to host services: {0}")]
    HostConnect(String),
}
