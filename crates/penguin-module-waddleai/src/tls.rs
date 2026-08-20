//! Minimal TLS bootstrap for [`crate::client::WaddleAiClient`]'s HTTP
//! client: installs the workspace's pinned `aws-lc-rs` crypto provider and
//! builds a default webpki root store. Mirrors
//! `waddlebot_client::tls`/`squawk_client::tls_support` exactly.

use std::sync::OnceLock;

use rustls::{ClientConfig, RootCertStore};

/// Installs the `aws-lc-rs` crypto provider as the process default, exactly
/// once. The workspace is pinned to aws-lc-rs everywhere else (rustls'
/// default `ring` provider cannot verify the go-plugin host's P-521
/// certificates at all), so losing the install race to another initializer
/// elsewhere in the daemon is fine: every caller wants the same provider.
fn ensure_crypto_provider_installed() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// Builds the rustls client config used for every WaddleAI API request: the
/// installed aws-lc-rs provider, the default webpki root store, no client
/// certificate (the API authenticates callers via the virtual key, not
/// mTLS).
pub(crate) fn build_tls_config() -> ClientConfig {
    ensure_crypto_provider_installed();
    let root_store = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_tls_config_can_be_called_more_than_once() {
        build_tls_config();
        build_tls_config();
    }
}
