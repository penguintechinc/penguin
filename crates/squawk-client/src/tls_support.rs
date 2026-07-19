//! Crypto-provider setup shared by [`crate::doh`] and [`crate::license`]'s
//! HTTP clients.
//!
//! Kept in its own tiny module rather than duplicated in each (the way
//! `penguin-sdk`/`penguin-goplugin-host` intentionally duplicate their own
//! copies across crate boundaries) because both call sites live in this same
//! crate — one shared `OnceLock` install is simpler than two, with no
//! cross-crate coupling cost to avoid.

use std::sync::OnceLock;

use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};

/// Installs the `aws-lc-rs` crypto provider as the process default, exactly
/// once. The workspace is pinned to aws-lc-rs everywhere (the go-plugin
/// P-521 certificate work cannot use rustls' default `ring` provider at
/// all), so losing the install race to another initializer elsewhere in the
/// daemon is not an error — every caller wants the same provider.
pub(crate) fn ensure_crypto_provider_installed() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// Reads the signature verification algorithms off the installed default
/// provider, for building custom [`rustls::client::danger::ServerCertVerifier`]
/// implementations. Panics if [`ensure_crypto_provider_installed`] has not
/// run yet — every call site here runs it first.
pub(crate) fn supported_algorithms() -> WebPkiSupportedAlgorithms {
    CryptoProvider::get_default()
        .expect("rustls crypto provider must be installed before TLS use")
        .signature_verification_algorithms
}
