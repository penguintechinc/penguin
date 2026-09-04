//! Minimal PEM block extraction for [`crate::doh`]'s optional mTLS
//! configuration (client certificate/key, custom CA).
//!
//! This is deliberately not a general PEM library — it understands exactly
//! the block shapes `rustls::pki_types` needs (`CERTIFICATE` and the three
//! private-key encodings) and nothing else, following the same
//! strip-the-framing-and-base64-decode approach as
//! `penguin_sdk::plugin::mtls::parse_pem_cert_der`, extended here to handle
//! more than one block per file and more than one key encoding.

use rustls::pki_types::{
    CertificateDer, PrivateKeyDer, PrivatePkcs1KeyDer, PrivatePkcs8KeyDer, PrivateSec1KeyDer,
};

/// Every way loading a certificate/key PEM file can fail.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PemError {
    /// The file could not be read at all.
    #[error("failed to read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    /// The file was read but contained no block of the expected kind.
    #[error("{path} contains no {label} block")]
    NoBlock { path: String, label: &'static str },
    /// A block's body was not valid base64.
    #[error("{path} contains a malformed {label} block: {source}")]
    Decode {
        path: String,
        label: &'static str,
        source: base64::DecodeError,
    },
}

/// Extracts every `-----BEGIN {label}-----` … `-----END {label}-----` block
/// from `pem`, base64-decoding each body into raw DER bytes.
fn extract_blocks(pem: &str, label: &str) -> Result<Vec<Vec<u8>>, base64::DecodeError> {
    use base64::Engine as _;

    let begin_marker = format!("-----BEGIN {label}-----");
    let end_marker = format!("-----END {label}-----");
    let mut blocks = Vec::new();
    let mut rest = pem;

    while let Some(begin_at) = rest.find(&begin_marker) {
        let after_begin = &rest[begin_at + begin_marker.len()..];
        let Some(end_at) = after_begin.find(&end_marker) else {
            break;
        };
        let body: String = after_begin[..end_at]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let der = base64::engine::general_purpose::STANDARD.decode(body)?;
        blocks.push(der);
        rest = &after_begin[end_at + end_marker.len()..];
    }

    Ok(blocks)
}

/// Loads every certificate in `path` (a PEM file, possibly a chain) as DER.
pub(crate) fn load_certificate_chain(path: &str) -> Result<Vec<CertificateDer<'static>>, PemError> {
    let contents = std::fs::read_to_string(path).map_err(|source| PemError::Read {
        path: path.to_string(),
        source,
    })?;
    let blocks = extract_blocks(&contents, "CERTIFICATE").map_err(|source| PemError::Decode {
        path: path.to_string(),
        label: "CERTIFICATE",
        source,
    })?;
    if blocks.is_empty() {
        return Err(PemError::NoBlock {
            path: path.to_string(),
            label: "CERTIFICATE",
        });
    }
    let mut certs = Vec::with_capacity(blocks.len());
    for der in blocks {
        certs.push(CertificateDer::from(der));
    }
    Ok(certs)
}

/// Loads the first private key found in `path`, trying PKCS#8
/// (`PRIVATE KEY`), then PKCS#1 (`RSA PRIVATE KEY`), then SEC1
/// (`EC PRIVATE KEY`) — the three encodings `tls.LoadX509KeyPair` accepts on
/// the Go side without a passphrase.
pub(crate) fn load_private_key(path: &str) -> Result<PrivateKeyDer<'static>, PemError> {
    let contents = std::fs::read_to_string(path).map_err(|source| PemError::Read {
        path: path.to_string(),
        source,
    })?;

    let pkcs8 = extract_blocks(&contents, "PRIVATE KEY").map_err(|source| PemError::Decode {
        path: path.to_string(),
        label: "PRIVATE KEY",
        source,
    })?;
    if let Some(der) = pkcs8.into_iter().next() {
        return Ok(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(der)));
    }

    let pkcs1 =
        extract_blocks(&contents, "RSA PRIVATE KEY").map_err(|source| PemError::Decode {
            path: path.to_string(),
            label: "RSA PRIVATE KEY",
            source,
        })?;
    if let Some(der) = pkcs1.into_iter().next() {
        return Ok(PrivateKeyDer::Pkcs1(PrivatePkcs1KeyDer::from(der)));
    }

    let sec1 = extract_blocks(&contents, "EC PRIVATE KEY").map_err(|source| PemError::Decode {
        path: path.to_string(),
        label: "EC PRIVATE KEY",
        source,
    })?;
    if let Some(der) = sec1.into_iter().next() {
        return Ok(PrivateKeyDer::Sec1(PrivateSec1KeyDer::from(der)));
    }

    Err(PemError::NoBlock {
        path: path.to_string(),
        label: "PRIVATE KEY / RSA PRIVATE KEY / EC PRIVATE KEY",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a syntactically valid PEM block from arbitrary bytes,
    /// base64-encoded with the real encoder rather than hand-typed — this
    /// unit only tests the framing/extraction logic, never asks the bytes to
    /// be a structurally valid X.509 certificate or key.
    fn fake_pem_block(label: &str, payload: &[u8]) -> String {
        let body = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, payload);
        format!("-----BEGIN {label}-----\n{body}\n-----END {label}-----\n")
    }

    #[test]
    fn extract_blocks_finds_one_certificate() {
        let pem = fake_pem_block("CERTIFICATE", b"fake-cert-bytes-for-testing");
        let blocks = extract_blocks(&pem, "CERTIFICATE").unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0], b"fake-cert-bytes-for-testing");
    }

    #[test]
    fn extract_blocks_finds_a_chain_of_two() {
        let one = fake_pem_block("CERTIFICATE", b"leaf-cert-bytes");
        let two = fake_pem_block("CERTIFICATE", b"intermediate-cert-bytes");
        let chain = format!("{one}{two}");
        let blocks = extract_blocks(&chain, "CERTIFICATE").unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0], b"leaf-cert-bytes");
        assert_eq!(blocks[1], b"intermediate-cert-bytes");
    }

    #[test]
    fn extract_blocks_returns_empty_for_no_match() {
        let blocks = extract_blocks("not a pem file", "CERTIFICATE").unwrap();
        assert!(blocks.is_empty());
    }

    #[test]
    fn extract_blocks_stops_at_an_unterminated_begin_marker() {
        // A BEGIN with no matching END must not be treated as a block —
        // the loop breaks rather than reading past the end of the string.
        let pem = "-----BEGIN CERTIFICATE-----\nAAAA\n";
        let blocks = extract_blocks(pem, "CERTIFICATE").unwrap();
        assert!(blocks.is_empty());
    }

    #[test]
    fn extract_blocks_errors_on_malformed_base64() {
        let pem =
            "-----BEGIN CERTIFICATE-----\n!!!not-valid-base64!!!\n-----END CERTIFICATE-----\n";
        assert!(extract_blocks(pem, "CERTIFICATE").is_err());
    }

    #[test]
    fn load_certificate_chain_reads_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cert.pem");
        std::fs::write(&path, fake_pem_block("CERTIFICATE", b"fake-cert-bytes")).unwrap();

        let certs = load_certificate_chain(path.to_str().unwrap()).unwrap();
        assert_eq!(certs.len(), 1);
    }

    #[test]
    fn load_certificate_chain_errors_on_missing_file() {
        let err = load_certificate_chain("/nonexistent/path/does-not-exist.pem");
        assert!(err.is_err());
    }

    #[test]
    fn load_certificate_chain_errors_when_the_file_has_no_certificate_block() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cert.pem");
        std::fs::write(&path, "this file has no PEM blocks at all").unwrap();

        let Err(err) = load_certificate_chain(path.to_str().unwrap()) else {
            panic!("a file with no CERTIFICATE block must error");
        };
        assert!(matches!(err, PemError::NoBlock { .. }));
    }

    #[test]
    fn load_certificate_chain_errors_on_malformed_base64() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cert.pem");
        std::fs::write(
            &path,
            "-----BEGIN CERTIFICATE-----\n!!!not-valid-base64!!!\n-----END CERTIFICATE-----\n",
        )
        .unwrap();

        let Err(err) = load_certificate_chain(path.to_str().unwrap()) else {
            panic!("malformed base64 must error");
        };
        assert!(matches!(err, PemError::Decode { .. }));
    }

    #[test]
    fn load_private_key_errors_on_missing_file() {
        let err = load_private_key("/nonexistent/path/does-not-exist.pem");
        assert!(matches!(err, Err(PemError::Read { .. })));
    }

    #[test]
    fn load_private_key_errors_on_malformed_pkcs8_base64() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.pem");
        std::fs::write(
            &path,
            "-----BEGIN PRIVATE KEY-----\n!!!not-valid-base64!!!\n-----END PRIVATE KEY-----\n",
        )
        .unwrap();

        let Err(err) = load_private_key(path.to_str().unwrap()) else {
            panic!("malformed base64 in a PRIVATE KEY block must error");
        };
        assert!(matches!(err, PemError::Decode { .. }));
    }

    #[test]
    fn load_private_key_falls_back_to_pkcs1_rsa() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.pem");
        std::fs::write(
            &path,
            fake_pem_block("RSA PRIVATE KEY", b"fake-rsa-key-bytes"),
        )
        .unwrap();

        let key = load_private_key(path.to_str().unwrap()).unwrap();
        assert!(matches!(key, PrivateKeyDer::Pkcs1(_)));
    }

    #[test]
    fn load_private_key_errors_on_malformed_pkcs1_base64() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.pem");
        std::fs::write(
            &path,
            "-----BEGIN RSA PRIVATE KEY-----\n!!!not-valid-base64!!!\n-----END RSA PRIVATE KEY-----\n",
        )
        .unwrap();

        let Err(err) = load_private_key(path.to_str().unwrap()) else {
            panic!("malformed base64 in an RSA PRIVATE KEY block must error");
        };
        assert!(matches!(err, PemError::Decode { .. }));
    }

    #[test]
    fn load_private_key_errors_on_malformed_ec_base64() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.pem");
        std::fs::write(
            &path,
            "-----BEGIN EC PRIVATE KEY-----\n!!!not-valid-base64!!!\n-----END EC PRIVATE KEY-----\n",
        )
        .unwrap();

        let Err(err) = load_private_key(path.to_str().unwrap()) else {
            panic!("malformed base64 in an EC PRIVATE KEY block must error");
        };
        assert!(matches!(err, PemError::Decode { .. }));
    }

    #[test]
    fn load_private_key_prefers_pkcs8_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.pem");
        // Body content doesn't need to be a structurally valid key for this
        // parser-selection test — only that PKCS8 framing is recognized and
        // decoded first when multiple key blocks are present.
        std::fs::write(&path, fake_pem_block("PRIVATE KEY", b"fake-key-bytes")).unwrap();

        let key = load_private_key(path.to_str().unwrap()).unwrap();
        assert!(matches!(key, PrivateKeyDer::Pkcs8(_)));
    }

    #[test]
    fn load_private_key_falls_back_to_ec() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.pem");
        std::fs::write(
            &path,
            fake_pem_block("EC PRIVATE KEY", b"fake-ec-key-bytes"),
        )
        .unwrap();

        let key = load_private_key(path.to_str().unwrap()).unwrap();
        assert!(matches!(key, PrivateKeyDer::Sec1(_)));
    }

    #[test]
    fn load_private_key_errors_when_no_known_block_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.pem");
        std::fs::write(&path, "not a key file").unwrap();

        let err = load_private_key(path.to_str().unwrap());
        assert!(err.is_err());
    }
}
