//! Pure parsing and validation of the go-plugin handshake line.
//!
//! On startup a plugin prints exactly one line to stdout:
//! `CoreProtocolVersion|ProtocolVersion|Network|Address|ProtocolType|ServerCert`.
//! Nothing here touches a process or a socket — [`Handshake::parse`] is a
//! pure function over `&str`, so the wire format can be exhaustively tested
//! without spawning anything. `client.rs` owns reading the line off the
//! child's stdout and enforcing the 60s startup timeout; this module only
//! ever sees the trimmed line.

use base64::Engine as _;

/// The core go-plugin protocol version this host speaks. Bumping it would be
/// a breaking change to every existing plugin binary, so it is not
/// configurable.
pub const CORE_PROTOCOL_VERSION: u32 = 1;

/// The application protocol versions this host offers in
/// `PLUGIN_PROTOCOL_VERSIONS`. Only version 1 is defined today.
pub const OFFERED_PROTOCOL_VERSIONS: &[u32] = &[1];

/// The only wire protocol this host accepts. go-plugin also supports the
/// legacy `netrpc`, which nothing built against `penguin.sdk.v1` ever speaks.
const PROTOCOL_TYPE_GRPC: &str = "grpc";

/// A certificate field must be longer than this to be treated as real DER —
/// this rules out the unused "extra" data some older go-plugin server
/// implementations still emit in that slot.
const MIN_CERT_FIELD_LEN: usize = 50;

/// A parsed and validated go-plugin handshake line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    /// The protocol version the plugin negotiated; always a member of
    /// [`OFFERED_PROTOCOL_VERSIONS`].
    pub protocol_version: u32,
    /// `"unix"` on Linux/macOS, `"tcp"` on Windows.
    pub network: String,
    /// The unix socket path, or `host:port` for `tcp`, the plugin's gRPC
    /// server is listening on.
    pub address: String,
    /// The plugin's self-signed AutoMTLS leaf certificate, DER-encoded, if
    /// the line carried one. `None` means the plugin did not present a
    /// certificate (AutoMTLS is off, or the field was too short to be real).
    pub server_cert_der: Option<Vec<u8>>,
}

/// Everything that can go wrong parsing or validating a handshake line.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandshakeError {
    /// Fewer than the four mandatory `|`-separated fields were present.
    #[error("malformed handshake line (need at least 4 fields, got {0}): {1:?}")]
    TooFewFields(usize, String),

    /// The core protocol version field was not a valid non-negative integer.
    #[error("invalid core protocol version {0:?}: {1}")]
    InvalidCoreVersion(String, String),

    /// The core protocol version did not match [`CORE_PROTOCOL_VERSION`].
    #[error("unsupported core protocol version {0}, expected {CORE_PROTOCOL_VERSION}")]
    UnsupportedCoreVersion(u32),

    /// The negotiated protocol version field was not a valid non-negative
    /// integer.
    #[error("invalid protocol version {0:?}: {1}")]
    InvalidProtocolVersion(String, String),

    /// The negotiated protocol version was not one this host offered.
    #[error("unsupported protocol version {0}")]
    UnsupportedProtocolVersion(u32),

    /// The protocol type field was present and was not `"grpc"`, or was
    /// absent (implying the legacy `netrpc` default).
    #[error("unsupported protocol type {0:?}, only grpc is supported")]
    UnsupportedProtocolType(String),

    /// The certificate field was long enough to be real but was not valid
    /// base64.
    #[error("invalid server certificate encoding: {0}")]
    InvalidCertEncoding(String),
}

impl Handshake {
    /// Parses and validates one handshake line.
    ///
    /// `line` is expected to already be trimmed of surrounding whitespace —
    /// the caller reads it from a line-buffered scanner over the child's
    /// stdout, which strips the trailing newline but nothing else.
    pub fn parse(line: &str) -> Result<Handshake, HandshakeError> {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 4 {
            return Err(HandshakeError::TooFewFields(parts.len(), line.to_string()));
        }

        let core_version: u32 = parts[0].parse().map_err(|e: std::num::ParseIntError| {
            HandshakeError::InvalidCoreVersion(parts[0].to_string(), e.to_string())
        })?;
        if core_version != CORE_PROTOCOL_VERSION {
            return Err(HandshakeError::UnsupportedCoreVersion(core_version));
        }

        let protocol_version: u32 = parts[1].parse().map_err(|e: std::num::ParseIntError| {
            HandshakeError::InvalidProtocolVersion(parts[1].to_string(), e.to_string())
        })?;
        if !OFFERED_PROTOCOL_VERSIONS.contains(&protocol_version) {
            return Err(HandshakeError::UnsupportedProtocolVersion(protocol_version));
        }

        let network = parts[2].to_string();
        let address = parts[3].to_string();

        // A missing protocol-type field means the plugin defaulted to the
        // legacy net/rpc protocol, which this host never speaks.
        let protocol_type = parts.get(4).copied().unwrap_or("");
        if protocol_type != PROTOCOL_TYPE_GRPC {
            return Err(HandshakeError::UnsupportedProtocolType(
                protocol_type.to_string(),
            ));
        }

        let mut server_cert_der = None;
        if let Some(field) = parts.get(5)
            && field.len() > MIN_CERT_FIELD_LEN
        {
            server_cert_der = Some(decode_cert_der(field)?);
        }

        Ok(Handshake {
            protocol_version,
            network,
            address,
            server_cert_der,
        })
    }
}

/// Decodes the base64 certificate field. go-plugin's Go host encodes it with
/// `base64.RawStdEncoding` — the standard alphabet with no `=` padding — so a
/// padded value must be rejected exactly as it would be on the Go side.
fn decode_cert_der(field: &str) -> Result<Vec<u8>, HandshakeError> {
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(field)
        .map_err(|e| HandshakeError::InvalidCertEncoding(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A syntactically valid 64-byte base64 (no padding) blob, long enough to
    /// clear [`MIN_CERT_FIELD_LEN`], standing in for a real DER certificate.
    const FAKE_CERT_B64: &str =
        "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVowMTIzNDU2Nzg5QUJDREVGR0hJSktMTU5PUA";

    fn valid_line() -> String {
        format!("1|1|unix|/tmp/plugin123/xxxx|grpc|{FAKE_CERT_B64}")
    }

    #[test]
    fn valid_line_with_all_six_fields_parses() {
        let handshake = Handshake::parse(&valid_line()).unwrap();
        assert_eq!(handshake.protocol_version, 1);
        assert_eq!(handshake.network, "unix");
        assert_eq!(handshake.address, "/tmp/plugin123/xxxx");
        assert!(handshake.server_cert_der.is_some());
    }

    #[test]
    fn four_field_minimum_is_enforced() {
        // Exactly 4 fields clears the length check itself (no
        // TooFewFields), but still fails validation because a missing
        // protocol-type field defaults to the unsupported legacy netrpc —
        // see missing_protocol_type_defaults_to_netrpc_and_is_rejected for
        // that specific case. 5 fields with an explicit "grpc" succeeds.
        assert!(Handshake::parse("1|1|unix|/tmp/sock|grpc").is_ok());
        let err = Handshake::parse("1|1|unix").unwrap_err();
        assert_eq!(err, HandshakeError::TooFewFields(3, "1|1|unix".to_string()));
    }

    #[test]
    fn missing_cert_field_yields_no_cert() {
        let handshake = Handshake::parse("1|1|unix|/tmp/sock|grpc").unwrap();
        assert_eq!(handshake.server_cert_der, None);
    }

    #[test]
    fn short_cert_field_yields_no_cert() {
        let line = "1|1|unix|/tmp/sock|grpc|dG9vc2hvcnQ";
        let handshake = Handshake::parse(line).unwrap();
        assert_eq!(handshake.server_cert_der, None);
    }

    #[test]
    fn wrong_core_version_is_rejected() {
        let line = format!("2|1|unix|/tmp/sock|grpc|{FAKE_CERT_B64}");
        let err = Handshake::parse(&line).unwrap_err();
        assert_eq!(err, HandshakeError::UnsupportedCoreVersion(2));
    }

    #[test]
    fn unoffered_protocol_version_is_rejected() {
        let line = format!("1|99|unix|/tmp/sock|grpc|{FAKE_CERT_B64}");
        let err = Handshake::parse(&line).unwrap_err();
        assert_eq!(err, HandshakeError::UnsupportedProtocolVersion(99));
    }

    #[test]
    fn non_grpc_protocol_type_is_rejected() {
        let line = "1|1|unix|/tmp/sock|netrpc";
        let err = Handshake::parse(line).unwrap_err();
        assert_eq!(
            err,
            HandshakeError::UnsupportedProtocolType("netrpc".to_string())
        );
    }

    #[test]
    fn missing_protocol_type_defaults_to_netrpc_and_is_rejected() {
        let err = Handshake::parse("1|1|unix|/tmp/sock").unwrap_err();
        assert_eq!(err, HandshakeError::UnsupportedProtocolType(String::new()));
    }

    #[test]
    fn malformed_garbage_line_is_rejected() {
        let err = Handshake::parse("not a handshake line at all").unwrap_err();
        assert_eq!(
            err,
            HandshakeError::TooFewFields(1, "not a handshake line at all".to_string())
        );
    }

    #[test]
    fn tcp_network_parses_like_unix() {
        let line = "1|1|tcp|127.0.0.1:54321|grpc";
        let handshake = Handshake::parse(line).unwrap();
        assert_eq!(handshake.network, "tcp");
        assert_eq!(handshake.address, "127.0.0.1:54321");
    }

    #[test]
    fn trailing_seventh_field_still_parses() {
        let line = format!("1|1|unix|/tmp/sock|grpc|{FAKE_CERT_B64}|true");
        let handshake = Handshake::parse(&line).unwrap();
        assert_eq!(handshake.address, "/tmp/sock");
        assert!(handshake.server_cert_der.is_some());
    }

    #[test]
    fn base64_cert_field_decodes_with_raw_std_encoding() {
        let der = decode_cert_der(FAKE_CERT_B64).unwrap();
        assert_eq!(
            der,
            base64::engine::general_purpose::STANDARD_NO_PAD
                .decode(FAKE_CERT_B64)
                .unwrap()
        );
        assert!(!der.is_empty());
    }

    #[test]
    fn padded_base64_cert_field_is_rejected() {
        // RawStdEncoding (no padding) rejects a value carrying `=` padding,
        // matching Go's base64.RawStdEncoding.DecodeString behaviour.
        let padded = format!("{FAKE_CERT_B64}==");
        assert!(decode_cert_der(&padded).is_err());
    }
}
