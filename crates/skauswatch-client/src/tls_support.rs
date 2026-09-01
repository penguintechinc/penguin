//! Crypto-provider setup for [`crate::client::SkausWatchClient`]'s HTTP
//! client.
//!
//! Kept in its own tiny module rather than inlined, mirroring
//! `squawk-client`'s `tls_support.rs` — a small, obviously-reusable piece
//! rather than duplicated logic in [`crate::client`].

use std::sync::OnceLock;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_crypto_provider_installed_is_idempotent() {
        // Two calls in the same process must not panic or otherwise
        // conflict — the second call loses the install race and that is
        // documented as fine, not an error.
        ensure_crypto_provider_installed();
        ensure_crypto_provider_installed();
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }
}
