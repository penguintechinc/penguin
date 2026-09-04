//! The `reqwest` client builder shared by [`crate::auth::AuthManager`] (JWT
//! lifecycle) and [`crate::vpn::VpnManager`] (tunnel config fetch).
//!
//! Mirrors `penguin-licensing`'s `build_http_client` exactly: rustls with
//! the aws-lc-rs crypto provider installed once process-wide, and root
//! certificates supplied manually from `webpki-roots`. This is deliberate,
//! not incidental — the workspace is pinned to aws-lc-rs everywhere because
//! the go-plugin P-521 certificate work cannot use rustls' default `ring`
//! provider at all, so every `reqwest::Client` in this workspace is built
//! this same way rather than pulling in reqwest's stock TLS setup.

use std::sync::OnceLock;
use std::time::Duration;

/// Builds a `reqwest::Client` with the given per-request timeout, using the
/// workspace-standard aws-lc-rs + `webpki-roots` TLS setup.
pub fn build_client(timeout: Duration) -> reqwest::Client {
    ensure_crypto_provider_installed();

    let root_store = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    // NOT wrapped in `Some(...)` — see `penguin-licensing::client::build_http_client`'s
    // identical comment: `use_preconfigured_tls` wraps its argument itself,
    // so pre-wrapping here would make the downcast target
    // `Option<Option<ClientConfig>>` and silently fall through to "Unknown
    // TLS backend".
    reqwest::Client::builder()
        .use_preconfigured_tls(tls_config)
        .timeout(timeout)
        .build()
        .expect("tobogganing HTTP client config is static and always valid")
}

/// Installs the aws-lc-rs crypto provider as the process default, exactly
/// once. Idempotent: losing the install race to another initializer
/// elsewhere in the daemon is not an error, since the whole workspace is
/// pinned to aws-lc-rs.
fn ensure_crypto_provider_installed() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}
