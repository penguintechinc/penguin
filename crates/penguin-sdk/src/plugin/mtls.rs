//! AutoMTLS for the plugin side: our own self-signed identity, PEM parsing
//! of the host's certificate, and the two byte-equality verifiers that
//! replace normal chain validation.
//!
//! This mirrors `penguin-goplugin-host::mtls` deliberately — the two crates
//! cannot share code (`penguin-sdk` cannot depend on
//! `penguin-goplugin-host`, which already depends on `penguin-sdk`), but the
//! protocol on both ends of the same connection has to agree byte-for-byte,
//! so the cert template, verifier logic, and crypto-provider setup are kept
//! in lockstep by hand. See that crate's `mtls.rs` for the authoritative
//! wire-format reasoning.
//!
//! Unlike a real Go plugin (which presents ECDSA **P-521**), we generate an
//! ECDSA **P-256** identity: the host verifies whatever certificate we
//! present via a pinned byte-equality check, never a real chain build, so
//! nothing requires matching Go's curve choice. `aws-lc-rs` is still the
//! installed provider (rather than the default `ring`) purely for
//! consistency with the rest of the go-plugin implementation in this repo.

use std::sync::OnceLock;

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};
use rustls::DistinguishedName as RustlsDistinguishedName;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, Error as TlsError, SignatureScheme};

use crate::plugin::error::PluginError;

/// The subject and SAN every AutoMTLS certificate in go-plugin uses. TLS
/// `ServerName` is hardcoded to this too, regardless of which role (client on
/// the broker leg, server on the main leg) we are playing.
pub(crate) const CERT_HOST: &str = "localhost";

/// Installs the `aws-lc-rs` crypto provider as the process default, exactly
/// once. Must run before any [`rustls::ClientConfig`] or
/// [`rustls::ServerConfig`] is built. Idempotent: losing the install race to
/// another caller in the same process is not an error.
pub fn ensure_crypto_provider_installed() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// Reads the signature verification algorithms off the installed default
/// provider. Panics if [`ensure_crypto_provider_installed`] has not run.
fn supported_algorithms() -> WebPkiSupportedAlgorithms {
    CryptoProvider::get_default()
        .expect("rustls crypto provider must be installed before TLS use")
        .signature_verification_algorithms
}

/// Our self-generated AutoMTLS plugin identity: an ECDSA P-256 self-signed
/// certificate and its matching private key. Presented as the server
/// certificate on the main connection and as the client certificate on the
/// broker's id=1 leg — go-plugin's Go host uses one identity for both roles
/// too, so a plugin does the same.
pub struct PluginIdentity {
    /// The DER-encoded certificate.
    pub cert_der: CertificateDer<'static>,
    /// The PKCS#8 DER-encoded private key, stored as raw bytes because
    /// `rustls` config builders each consume a fresh [`PrivateKeyDer`] by
    /// value and this identity is used to build two configs (server and
    /// client) from the same key.
    key_der_bytes: Vec<u8>,
}

impl PluginIdentity {
    /// Materialises a fresh [`PrivateKeyDer`] from the stored PKCS#8 bytes.
    pub fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_der_bytes.clone()))
    }
}

/// Builds the certificate parameters for our AutoMTLS plugin identity,
/// mirroring the go-plugin template: self-signed, its own CA (the peer puts
/// it straight into a trust pool and never builds a real chain), both TLS
/// roles via EKU, and rcgen's long-default validity window (left untouched).
fn plugin_cert_params() -> Result<CertificateParams, PluginError> {
    let mut params = CertificateParams::new(vec![CERT_HOST.to_string()])
        .map_err(|e| PluginError::Tls(e.to_string()))?;

    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, CERT_HOST);
    params.distinguished_name = name;

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

/// Generates a fresh AutoMTLS plugin identity: an ECDSA P-256 keypair and a
/// self-signed certificate over it, valid for `"localhost"`.
pub fn generate_plugin_identity() -> Result<PluginIdentity, PluginError> {
    let key_pair = KeyPair::generate().map_err(|e| PluginError::Tls(e.to_string()))?;
    let params = plugin_cert_params()?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| PluginError::Tls(e.to_string()))?;

    let cert_der = cert.der().clone();
    let key_der_bytes = key_pair.serialize_der();

    Ok(PluginIdentity {
        cert_der,
        key_der_bytes,
    })
}

/// Parses the PEM certificate the host sends us in `PLUGIN_CLIENT_CERT` into
/// raw DER bytes, without pulling in a full X.509 parser: strip the
/// `-----BEGIN/END-----` framing lines and base64-decode what remains, using
/// the standard (padded) alphabet Go's `encoding/pem` + `encoding/base64`
/// produce.
pub fn parse_pem_cert_der(pem: &str) -> Result<Vec<u8>, PluginError> {
    use base64::Engine as _;
    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    if body.is_empty() {
        return Err(PluginError::HostCert(
            "PLUGIN_CLIENT_CERT contained no certificate body".to_string(),
        ));
    }
    base64::engine::general_purpose::STANDARD
        .decode(body)
        .map_err(|e| PluginError::HostCert(e.to_string()))
}

/// Reads and parses the host's certificate from the `PLUGIN_CLIENT_CERT`
/// environment variable go-plugin's host sets before launching us.
pub fn read_host_cert_from_env() -> Result<CertificateDer<'static>, PluginError> {
    let pem = std::env::var("PLUGIN_CLIENT_CERT").map_err(|_| {
        PluginError::HostCert("PLUGIN_CLIENT_CERT environment variable is not set".to_string())
    })?;
    let der = parse_pem_cert_der(&pem)?;
    Ok(CertificateDer::from(der))
}

/// Verifies a presented certificate byte-for-byte against a single pinned
/// leaf DER, instead of building a certificate chain — the AutoMTLS model
/// has no chain to build, since every certificate is self-signed. Used as a
/// [`ServerCertVerifier`] when we dial the host on the broker's id=1 leg
/// (pinning the host's `PLUGIN_CLIENT_CERT` value).
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
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        if end_entity.as_ref() == self.pinned.as_ref() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(TlsError::General(
                "presented server certificate does not match the pinned host certificate"
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

/// The client-certificate counterpart of [`PinnedServerCertVerifier`], used
/// when we serve the main connection (we are the TLS server, the host is the
/// TLS client presenting its `PLUGIN_CLIENT_CERT` identity back to us).
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
                "presented client certificate does not match the pinned host certificate"
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

/// Builds the `rustls::ServerConfig` for the main connection: we are the TLS
/// server, presenting our own identity and requiring + pinning the host's
/// certificate as the only acceptable client certificate. ALPN `h2` is
/// mandatory — see the crate-level doc comment on why grpc-go closes the
/// connection immediately after an otherwise-successful handshake without it.
pub fn build_server_tls_config(
    identity: &PluginIdentity,
    host_cert: CertificateDer<'static>,
) -> Result<std::sync::Arc<rustls::ServerConfig>, PluginError> {
    let verifier = std::sync::Arc::new(PinnedClientCertVerifier::new(host_cert));
    let cert_chain = vec![identity.cert_der.clone()];
    let mut config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(cert_chain, identity.private_key())
        .map_err(|e| PluginError::Tls(e.to_string()))?;
    config.alpn_protocols = vec![b"h2".to_vec()];
    Ok(std::sync::Arc::new(config))
}

/// Builds the `rustls::ClientConfig` for the broker's id=1 leg: we are the
/// TLS client dialing the host's `HostService`, presenting our own identity
/// (the host requires and verifies a client certificate there) and pinning
/// the host's certificate as the only acceptable server certificate. Also
/// mandates ALPN `h2`, matching [`build_server_tls_config`].
pub fn build_client_tls_config(
    identity: &PluginIdentity,
    host_cert: CertificateDer<'static>,
) -> Result<std::sync::Arc<rustls::ClientConfig>, PluginError> {
    let verifier = std::sync::Arc::new(PinnedServerCertVerifier::new(host_cert));
    let cert_chain = vec![identity.cert_der.clone()];
    let mut config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(cert_chain, identity.private_key())
        .map_err(|e| PluginError::Tls(e.to_string()))?;
    config.alpn_protocols = vec![b"h2".to_vec()];
    Ok(std::sync::Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_cert_params_identify_localhost() {
        let params = plugin_cert_params().unwrap();
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
    fn plugin_cert_params_are_a_ca_with_both_ekus() {
        let params = plugin_cert_params().unwrap();
        assert_eq!(params.is_ca, IsCa::Ca(BasicConstraints::Unconstrained));
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
    fn generated_identity_produces_a_real_der_cert() {
        let identity = generate_plugin_identity().unwrap();
        assert!(identity.cert_der.len() > 50);
    }

    #[test]
    fn pem_round_trips_to_the_same_der_generate_wrote() {
        let identity = generate_plugin_identity().unwrap();
        // rcgen's own PEM encoder round-tripped through our hand-rolled
        // parser must reproduce the exact DER — this is the same parsing
        // path used on the host's PLUGIN_CLIENT_CERT PEM at runtime.
        let cert = rcgen::CertificateParams::new(vec![CERT_HOST.to_string()])
            .unwrap()
            .self_signed(&KeyPair::generate().unwrap())
            .unwrap();
        let der = parse_pem_cert_der(&cert.pem()).unwrap();
        assert_eq!(der, cert.der().as_ref());
        // identity.cert_der itself is never re-encoded to PEM by this crate
        // (only DER goes on the wire), so this just proves the parser is
        // correct against rcgen's own PEM output shape.
        let _ = &identity;
    }

    #[test]
    fn parse_pem_cert_der_rejects_empty_body() {
        let err = parse_pem_cert_der("-----BEGIN CERTIFICATE-----\n-----END CERTIFICATE-----\n");
        assert!(err.is_err());
    }

    #[test]
    fn server_verifier_accepts_only_the_pinned_der() {
        let pinned = CertificateDer::from(vec![1, 2, 3, 4]);
        let other = CertificateDer::from(vec![9, 9, 9, 9]);
        let verifier = PinnedServerCertVerifier::new(pinned.clone());
        let server_name = rustls::pki_types::ServerName::try_from(CERT_HOST).unwrap();
        assert!(
            verifier
                .verify_server_cert(&pinned, &[], &server_name, &[], UnixTime::now())
                .is_ok()
        );
        assert!(
            verifier
                .verify_server_cert(&other, &[], &server_name, &[], UnixTime::now())
                .is_err()
        );
    }

    #[test]
    fn client_verifier_accepts_only_the_pinned_der() {
        let pinned = CertificateDer::from(vec![5, 6, 7, 8]);
        let other = CertificateDer::from(vec![0, 0, 0, 0]);
        let verifier = PinnedClientCertVerifier::new(pinned.clone());
        assert!(
            verifier
                .verify_client_cert(&pinned, &[], UnixTime::now())
                .is_ok()
        );
        assert!(
            verifier
                .verify_client_cert(&other, &[], UnixTime::now())
                .is_err()
        );
        assert!(verifier.root_hint_subjects().is_empty());
    }
}
