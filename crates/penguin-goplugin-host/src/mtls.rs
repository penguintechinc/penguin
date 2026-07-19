//! AutoMTLS: our host identity, and the two byte-equality certificate
//! verifiers that replace normal chain validation.
//!
//! go-plugin's AutoMTLS is not a PKI: each side hands the other a single
//! self-signed leaf over an already-trusted channel (the handshake line for
//! the plugin's cert, the `PLUGIN_CLIENT_CERT` env var for ours) and from
//! then on TLS only has to prove "you are still holding the private key for
//! the exact certificate I was told about" — not "you chain to a CA I
//! trust". [`PinnedServerCertVerifier`] and [`PinnedClientCertVerifier`]
//! implement exactly that: a raw DER comparison, with real cryptographic
//! signature verification (via the installed [`rustls::crypto::CryptoProvider`])
//! still enforced for the handshake itself.
//!
//! Plugins present a self-signed **ECDSA P-521** certificate. rustls' default
//! `ring` provider cannot verify secp521r1 at all, so [`ensure_crypto_provider_installed`]
//! must run before any TLS connection is attempted — see the crate-level doc
//! comment for why this is the single most important line in the crate.

use std::sync::OnceLock;

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};
use rustls::DistinguishedName as RustlsDistinguishedName;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, Error as TlsError, SignatureScheme};

use crate::error::HostError;

/// The subject and SAN every AutoMTLS certificate in go-plugin uses,
/// including ours. TLS `ServerName` is hardcoded to this too, regardless of
/// which transport (unix socket or TCP) actually carries the connection —
/// `client.rs` reuses this constant for that purpose.
pub(crate) const CERT_HOST: &str = "localhost";

/// Installs the `aws-lc-rs` crypto provider as the process default, exactly
/// once. Idempotent: a second (or racing) call is a harmless no-op — losing
/// the install race is not an error, since it means another caller already
/// installed the same provider.
///
/// This must run before any [`rustls::ClientConfig`] or [`rustls::ServerConfig`]
/// is built. `client.rs` calls it at the top of every connection attempt;
/// tests that only exercise the byte-equality comparison in
/// [`PinnedServerCertVerifier::verify_server_cert`] /
/// [`PinnedClientCertVerifier::verify_client_cert`] don't need it, since
/// those paths never touch the crypto provider.
pub fn ensure_crypto_provider_installed() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        // install_default fails only when a provider is already installed
        // (by us or a racing caller) — either way the postcondition holds.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// Reads the signature verification algorithms off the installed default
/// provider. Panics if [`ensure_crypto_provider_installed`] has not run —
/// every call site in this crate is downstream of `client.rs` calling it
/// first.
fn supported_algorithms() -> WebPkiSupportedAlgorithms {
    CryptoProvider::get_default()
        .expect("rustls crypto provider must be installed before TLS use")
        .signature_verification_algorithms
}

/// Our self-generated AutoMTLS host identity: an ECDSA P-256 self-signed
/// certificate and its matching private key.
///
/// go-plugin's Go host generates a P-521 key for this role; nothing requires
/// us to match that. The plugin puts our certificate straight into an
/// `x509.CertPool` and never runs chain validation against it, so any
/// algorithm rustls can both sign and later verify (via our own
/// [`PinnedClientCertVerifier`]) works.
pub struct HostIdentity {
    /// The DER-encoded certificate, used directly when building our
    /// [`rustls::ServerConfig`]/[`rustls::ClientConfig`].
    pub cert_der: CertificateDer<'static>,
    /// The PEM-encoded certificate, sent to the plugin verbatim in the
    /// `PLUGIN_CLIENT_CERT` environment variable.
    pub cert_pem: String,
    /// The PKCS#8 DER-encoded private key, stored as raw bytes rather than a
    /// [`PrivateKeyDer`] because `client.rs` needs it twice — once for the
    /// client config on the main connection, once for the server config on
    /// the broker's id=1 leg — and `PrivateKeyDer` does not implement
    /// `Clone`. Use [`HostIdentity::private_key`] to materialise one.
    key_der_bytes: Vec<u8>,
}

impl HostIdentity {
    /// Materialises a fresh [`PrivateKeyDer`] from the stored PKCS#8 bytes.
    /// Every `rustls` config builder consumes its key by value, so this
    /// must be called once per config built from this identity.
    pub fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_der_bytes.clone()))
    }
}

/// Builds the certificate parameters for our AutoMTLS host identity.
///
/// Split out from [`generate_host_identity`] so the parameters themselves —
/// subject, SAN, CA bit, extended key usage — can be asserted on directly in
/// tests without paying for a real keypair generation and signature in every
/// run.
fn host_cert_params() -> Result<CertificateParams, HostError> {
    let mut params = CertificateParams::new(vec![CERT_HOST.to_string()])
        .map_err(|e| HostError::Tls(e.to_string()))?;

    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, CERT_HOST);
    params.distinguished_name = name;

    // Mirrors the plugin-side template: a self-signed cert that is also its
    // own CA (go-plugin puts it straight into a CertPool as the sole trust
    // anchor) and carries both TLS roles, since this host is a TLS client on
    // the main connection but a TLS server on the broker's id=1 leg.
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ClientAuth,
        ExtendedKeyUsagePurpose::ServerAuth,
    ];
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
        KeyUsagePurpose::KeyAgreement,
        KeyUsagePurpose::KeyCertSign,
    ];

    Ok(params)
}

/// Generates a fresh AutoMTLS host identity: an ECDSA P-256 keypair and a
/// self-signed certificate over it, valid for `"localhost"`.
pub fn generate_host_identity() -> Result<HostIdentity, HostError> {
    let key_pair = KeyPair::generate().map_err(|e| HostError::Tls(e.to_string()))?;
    let params = host_cert_params()?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| HostError::Tls(e.to_string()))?;

    let cert_der = cert.der().clone();
    let cert_pem = cert.pem();
    let key_der_bytes = key_pair.serialize_der();

    Ok(HostIdentity {
        cert_der,
        cert_pem,
        key_der_bytes,
    })
}

/// Verifies the plugin's presented certificate byte-for-byte against the
/// single leaf DER pinned from the handshake line, instead of building a
/// certificate chain.
///
/// This is the Rust equivalent of Go's `loadServerCert`: Go puts the
/// handshake certificate in an `x509.CertPool` used as both `RootCAs` and
/// `ClientCAs`, and because the pool holds exactly one certificate, chain
/// verification degenerates to an equality check anyway. Comparing DER bytes
/// directly is the same check with less machinery and none of webpki's
/// chain-building assumptions, which don't apply to a self-signed leaf with
/// no issuer.
#[derive(Debug, Clone)]
pub struct PinnedServerCertVerifier {
    pinned: CertificateDer<'static>,
}

impl PinnedServerCertVerifier {
    /// Pins `cert` as the only certificate this verifier will accept.
    pub fn new(cert: CertificateDer<'static>) -> PinnedServerCertVerifier {
        PinnedServerCertVerifier { pinned: cert }
    }
}

impl ServerCertVerifier for PinnedServerCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        if end_entity.as_ref() == self.pinned.as_ref() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(TlsError::General(
                "presented server certificate does not match the pinned handshake certificate"
                    .to_string(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &supported_algorithms())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &supported_algorithms())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        supported_algorithms().supported_schemes()
    }
}

/// Verifies the plugin's presented client certificate byte-for-byte against
/// the pinned handshake leaf, for the broker's id=1 leg where this host acts
/// as the TLS server and the plugin as the TLS client.
///
/// See the crate-level doc comment: the frozen Go host serves this leg in
/// plaintext (a bug in its `Accept`/`AcceptAndServe` split), so this pinning
/// verifier only ever runs against a correctly-written plugin, never an
/// existing Go one.
#[derive(Debug, Clone)]
pub struct PinnedClientCertVerifier {
    pinned: CertificateDer<'static>,
}

impl PinnedClientCertVerifier {
    /// Pins `cert` as the only client certificate this verifier will accept.
    pub fn new(cert: CertificateDer<'static>) -> PinnedClientCertVerifier {
        PinnedClientCertVerifier { pinned: cert }
    }
}

impl ClientCertVerifier for PinnedClientCertVerifier {
    fn root_hint_subjects(&self) -> &[RustlsDistinguishedName] {
        // Nothing to hint: the plugin has exactly one certificate to offer
        // and no chain-building logic on either side to steer.
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, TlsError> {
        if end_entity.as_ref() == self.pinned.as_ref() {
            Ok(ClientCertVerified::assertion())
        } else {
            Err(TlsError::General(
                "presented client certificate does not match the pinned plugin certificate"
                    .to_string(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &supported_algorithms())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &supported_algorithms())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        supported_algorithms().supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes a PEM block's base64 body back to DER, using nothing but the
    /// standard alphabet decoder this crate already depends on — enough to
    /// prove [`generate_host_identity`]'s PEM and DER outputs agree without
    /// pulling in an X.509 parser just for a test.
    fn pem_to_der(pem: &str) -> Vec<u8> {
        use base64::Engine as _;
        let body: String = pem
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();
        base64::engine::general_purpose::STANDARD
            .decode(body)
            .expect("valid PEM body")
    }

    #[test]
    fn host_cert_params_identify_localhost() {
        let params = host_cert_params().unwrap();
        assert_eq!(
            params.distinguished_name.get(&DnType::CommonName),
            Some(&rcgen::DnValue::Utf8String(CERT_HOST.to_string()))
        );
        let has_localhost_san = params.subject_alt_names.iter().any(|san| match san {
            rcgen::SanType::DnsName(name) => name.as_str() == CERT_HOST,
            _ => false,
        });
        assert!(has_localhost_san);
    }

    #[test]
    fn host_cert_params_are_a_ca() {
        let params = host_cert_params().unwrap();
        assert_eq!(params.is_ca, IsCa::Ca(BasicConstraints::Unconstrained));
    }

    #[test]
    fn host_cert_params_carry_both_extended_key_usages() {
        let params = host_cert_params().unwrap();
        assert!(
            params
                .extended_key_usages
                .contains(&ExtendedKeyUsagePurpose::ClientAuth)
        );
        assert!(
            params
                .extended_key_usages
                .contains(&ExtendedKeyUsagePurpose::ServerAuth)
        );
    }

    #[test]
    fn generated_host_identity_round_trips_der_to_pem() {
        let identity = generate_host_identity().unwrap();
        assert!(identity.cert_pem.starts_with("-----BEGIN CERTIFICATE-----"));
        let decoded = pem_to_der(&identity.cert_pem);
        assert_eq!(decoded, identity.cert_der.as_ref());
    }

    #[test]
    fn server_verifier_accepts_the_exact_pinned_der() {
        let pinned = CertificateDer::from(vec![1, 2, 3, 4]);
        let verifier = PinnedServerCertVerifier::new(pinned.clone());
        let server_name = ServerName::try_from(CERT_HOST).unwrap();
        let result = verifier.verify_server_cert(&pinned, &[], &server_name, &[], UnixTime::now());
        assert!(result.is_ok());
    }

    #[test]
    fn server_verifier_rejects_any_other_der() {
        let pinned = CertificateDer::from(vec![1, 2, 3, 4]);
        let other = CertificateDer::from(vec![9, 9, 9, 9]);
        let verifier = PinnedServerCertVerifier::new(pinned);
        let server_name = ServerName::try_from(CERT_HOST).unwrap();
        let result = verifier.verify_server_cert(&other, &[], &server_name, &[], UnixTime::now());
        assert!(result.is_err());
    }

    #[test]
    fn client_verifier_accepts_the_exact_pinned_der() {
        let pinned = CertificateDer::from(vec![5, 6, 7, 8]);
        let verifier = PinnedClientCertVerifier::new(pinned.clone());
        let result = verifier.verify_client_cert(&pinned, &[], UnixTime::now());
        assert!(result.is_ok());
    }

    #[test]
    fn client_verifier_rejects_any_other_der() {
        let pinned = CertificateDer::from(vec![5, 6, 7, 8]);
        let other = CertificateDer::from(vec![0, 0, 0, 0]);
        let verifier = PinnedClientCertVerifier::new(pinned);
        let result = verifier.verify_client_cert(&other, &[], UnixTime::now());
        assert!(result.is_err());
    }

    #[test]
    fn client_verifier_offers_no_root_hint_subjects() {
        let verifier = PinnedClientCertVerifier::new(CertificateDer::from(vec![1]));
        assert!(verifier.root_hint_subjects().is_empty());
    }
}
