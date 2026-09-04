//! Minisign signature verification: the sole trust mechanism between a
//! downloaded release archive and what this process is about to execute in
//! its own place.
//!
//! Thin wrapper over `minisign-verify` (verify-only) — matching
//! `penguin-extplugin`'s own choice, see that crate's `verify.rs` doc for
//! why signing never happens in production code here either. There is no
//! trust-on-first-use and no fallback: any failure below is refused, never
//! logged-and-continued.

use minisign_verify::{PublicKey, Signature};

/// Every way [`verify`] can fail to establish trust in `data`.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// `public_key_text` is not a well-formed minisign public key.
    #[error("release verification key is not a valid minisign public key")]
    MalformedPublicKey,
    /// `signature_text` is not a well-formed minisign signature.
    #[error("signature is not a valid minisign signature")]
    MalformedSignature,
    /// The signature decoded fine, but does not verify against `data` with
    /// `public_key_text` — the archive may be tampered, corrupted in
    /// transit, or simply was not signed by the expected key.
    #[error("signature verification failed: archive may be tampered")]
    SignatureMismatch,
}

/// Verifies `data` against `signature_text`, using `public_key_text` as the
/// sole trusted signer. `allow_legacy = true` (matching
/// `penguin-extplugin::verify`) accepts both prehashed and legacy-mode
/// minisign signatures, since real `minisign`/goreleaser CI output can be
/// either depending on tool version.
pub fn verify(data: &[u8], signature_text: &str, public_key_text: &str) -> Result<(), VerifyError> {
    let public_key =
        PublicKey::decode(public_key_text).map_err(|_| VerifyError::MalformedPublicKey)?;
    let signature =
        Signature::decode(signature_text).map_err(|_| VerifyError::MalformedSignature)?;

    public_key
        .verify(data, &signature, true)
        .map_err(|_| VerifyError::SignatureMismatch)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    /// Generates a throwaway minisign keypair and signs `data`, returning
    /// `(public_key_text, signature_text)` in the same wire format
    /// `minisign-verify` decodes — mirrors `penguin-daemon`'s own
    /// `tests/external_plugin.rs` fixture pattern (see that file's doc
    /// comment for why a *signing*-capable crate is only ever a
    /// dev-dependency).
    fn sign_fixture(data: &[u8]) -> (String, String) {
        let keypair =
            minisign::KeyPair::generate_unencrypted_keypair().expect("generate minisign keypair");
        let signature_box = minisign::sign(
            Some(&keypair.pk),
            &keypair.sk,
            Cursor::new(data),
            Some("penguin-update test fixture"),
            None,
        )
        .expect("sign fixture data");

        let public_key_text = keypair.pk.to_box().expect("public key box").into_string();
        (public_key_text, signature_box.to_string())
    }

    #[test]
    fn verify_accepts_a_correctly_signed_payload() {
        let data = b"a real penguind release archive";
        let (public_key, signature) = sign_fixture(data);

        assert!(verify(data, &signature, &public_key).is_ok());
    }

    #[test]
    fn verify_rejects_tampered_payload_bytes() {
        let data = b"a real penguind release archive";
        let (public_key, signature) = sign_fixture(data);

        let mut tampered = data.to_vec();
        tampered[0] ^= 0x01;

        let err = verify(&tampered, &signature, &public_key).expect_err("tampered data");
        assert!(matches!(err, VerifyError::SignatureMismatch));
    }

    #[test]
    fn verify_rejects_signature_from_an_unrelated_key() {
        let data = b"a real penguind release archive";
        let (_, signature) = sign_fixture(data);
        // A different keypair's public key never verifies this signature.
        let (unrelated_public_key, _) = sign_fixture(b"unrelated data");

        let err = verify(data, &signature, &unrelated_public_key).expect_err("wrong signer's key");
        assert!(matches!(err, VerifyError::SignatureMismatch));
    }

    #[test]
    fn verify_rejects_malformed_public_key_text() {
        let data = b"a real penguind release archive";
        let (_, signature) = sign_fixture(data);

        let err = verify(data, &signature, "not a minisign public key").expect_err("malformed key");
        assert!(matches!(err, VerifyError::MalformedPublicKey));
    }

    #[test]
    fn verify_rejects_malformed_signature_text() {
        let data = b"a real penguind release archive";
        let (public_key, _) = sign_fixture(data);

        let err = verify(data, "not a minisign signature", &public_key).expect_err("malformed sig");
        assert!(matches!(err, VerifyError::MalformedSignature));
    }

    #[test]
    fn verify_rejects_a_tampered_signature_that_still_parses() {
        let data = b"a real penguind release archive";
        let (public_key, signature) = sign_fixture(data);

        // Flip the first base64 character of the signature's global-
        // signature line (the 2nd line) so it still decodes structurally
        // but no longer verifies — same technique (and same reason for
        // targeting the first character rather than one near the end,
        // where base64 padding lives) as
        // `penguin-extplugin::verify::tests::tamper_signature_line`.
        let mut lines: Vec<&str> = signature.lines().collect();
        let target = lines[1];
        let mut chars: Vec<char> = target.chars().collect();
        let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let original = chars[0];
        chars[0] = alphabet
            .chars()
            .find(|candidate| *candidate != original)
            .expect("alphabet has more than one character");
        let tampered_line: String = chars.into_iter().collect();
        lines[1] = &tampered_line;
        let tampered_signature = format!("{}\n", lines.join("\n"));

        let err = verify(data, &tampered_signature, &public_key).expect_err("tampered signature");
        assert!(matches!(
            err,
            VerifyError::SignatureMismatch | VerifyError::MalformedSignature
        ));
    }
}
