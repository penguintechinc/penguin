//! The plugin side of the go-plugin handshake: the magic-cookie sanity check
//! every plugin must fail closed on, and the single handshake line printed
//! to stdout once the listener is ready.
//!
//! `penguin-goplugin-host::handshake` parses this exact line; see that
//! module's doc comment for the wire format
//! (`CoreProtocolVersion|ProtocolVersion|Network|Address|ProtocolType|ServerCert`).
//! This module only ever builds it — parsing is not needed on this side.

use std::io::Write as _;
use std::path::Path;

use base64::Engine as _;

use crate::plugin::error::PluginError;

/// The env var the host sets to prove the child it launched agrees this is a
/// real `penguin-sdk` plugin, not an unrelated executable run by accident.
const MAGIC_COOKIE_KEY: &str = "PENGUIN_PLUGIN";
/// The value a plugin must see in [`MAGIC_COOKIE_KEY`] to proceed.
const MAGIC_COOKIE_VALUE: &str = "penguin-sdk-v1";

/// The core go-plugin protocol version this SDK speaks.
const CORE_PROTOCOL_VERSION: u32 = 1;
/// The application protocol version this SDK negotiates. Only version 1 is
/// defined today, so it is not negotiated against `PLUGIN_PROTOCOL_VERSIONS`
/// — we only ever support unix-socket gRPC transport, which every host that
/// can launch us already offers at version 1.
const PROTOCOL_VERSION: u32 = 1;

/// Checks the magic-cookie environment variable and exits the process
/// immediately if it is missing or wrong — the standard go-plugin behaviour
/// for a binary that was executed directly instead of launched as a plugin.
///
/// Must run before anything else in [`crate::plugin::serve::serve`]: it is
/// the cheapest possible failure path and must not wait on a tokio runtime,
/// a generated certificate, or a bound socket first.
pub fn require_magic_cookie_or_exit() {
    let cookie = std::env::var(MAGIC_COOKIE_KEY).unwrap_or_default();
    if cookie == MAGIC_COOKIE_VALUE {
        return;
    }
    eprintln!(
        "This binary is a plugin. These are not meant to be executed directly.\n\
         Please execute the program that consumes these plugins, which will\n\
         load any plugins automatically"
    );
    std::process::exit(1);
}

/// Builds the handshake line for a unix-socket listener at `socket_path`,
/// presenting `leaf_cert_der` as our AutoMTLS certificate.
///
/// The certificate field is base64 with `RawStdEncoding` — the standard
/// alphabet, no `=` padding — matching Go's `base64.RawStdEncoding` exactly,
/// since `penguin-goplugin-host::handshake` decodes it the same way.
pub fn build_line(socket_path: &Path, leaf_cert_der: &[u8]) -> String {
    let cert_b64 = base64::engine::general_purpose::STANDARD_NO_PAD.encode(leaf_cert_der);
    format!(
        "{CORE_PROTOCOL_VERSION}|{PROTOCOL_VERSION}|unix|{}|grpc|{cert_b64}",
        socket_path.display()
    )
}

/// Writes `line` to stdout followed by a newline and flushes immediately.
///
/// The host reads exactly one line off our stdout with a 60s timeout before
/// attempting anything else, so this must be written once, promptly, and
/// with no buffering left un-flushed — a plugin that panics before this
/// point or fails to flush looks identical to a hung plugin from the host's
/// side.
pub fn print_and_flush(line: &str) -> Result<(), PluginError> {
    let mut stdout = std::io::stdout();
    writeln!(stdout, "{line}").map_err(|e| PluginError::Handshake(e.to_string()))?;
    stdout
        .flush()
        .map_err(|e| PluginError::Handshake(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn build_line_has_the_expected_shape() {
        let path = PathBuf::from("/tmp/pgp-abcd/s");
        let line = build_line(&path, &[1, 2, 3, 4, 5]);
        let parts: Vec<&str> = line.split('|').collect();
        assert_eq!(parts.len(), 6);
        assert_eq!(parts[0], "1");
        assert_eq!(parts[1], "1");
        assert_eq!(parts[2], "unix");
        assert_eq!(parts[3], "/tmp/pgp-abcd/s");
        assert_eq!(parts[4], "grpc");
        assert!(!parts[5].contains('='), "cert field must not be padded");
    }

    #[test]
    fn print_and_flush_writes_the_line_to_stdout() {
        // Writes to the test process's real (captured-by-cargo-test) stdout
        // — no mock needed, and nothing this crate's "no real network/OS
        // mutation" rule is concerned with: this is the exact call
        // `crate::plugin::serve::serve` makes once the listener is ready.
        let result = print_and_flush("1|1|unix|/tmp/example|grpc|");
        assert!(result.is_ok());
    }

    #[test]
    fn build_line_cert_field_decodes_back_to_the_same_der() {
        let path = PathBuf::from("/tmp/pgp-xyz/s");
        let der = vec![9_u8, 8, 7, 6, 5, 4, 3, 2, 1, 0];
        let line = build_line(&path, &der);
        let cert_field = line.split('|').nth(5).unwrap();
        let decoded = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(cert_field)
            .unwrap();
        assert_eq!(decoded, der);
    }
}
