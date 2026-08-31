//! [`IntegrityManifest`]: the controller-signed list of files an agent
//! install is supposed to contain, plus [`ManifestSource`] for loading one.
//!
//! Verification only — [`IntegrityManifest::verify_signature`] is a thin
//! wrapper over `penguin_update::verify` (see that crate's `verify.rs` for
//! why minisign and why signing never happens in production code here
//! either).

use std::path::PathBuf;

use crate::error::SelfProtectError;

/// One file this agent's self-protection subsystem should trust: its
/// path relative to the install root, expected SHA-256 content hash (lower-
/// case hex), and expected Unix mode bits.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManifestEntry {
    /// Path relative to the agent install root, e.g. `"bin/penguind"`.
    pub path: String,
    /// Expected SHA-256 hash of the file's contents, as lower-case hex.
    pub sha256: String,
    /// Expected Unix mode bits (e.g. `0o755`).
    pub mode: u32,
}

/// A signed set of [`ManifestEntry`] records plus the manifest schema
/// version. [`IntegrityManifest::verify_signature`] is the sole way to
/// establish trust in `entries` — nothing in this crate acts on an
/// unverified manifest.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IntegrityManifest {
    /// Manifest schema version, bumped on incompatible layout changes.
    pub version: u32,
    /// The files this manifest attests to.
    pub entries: Vec<ManifestEntry>,
    /// Minisign signature (wire-format text) over
    /// [`IntegrityManifest::canonical_bytes`] — never over the whole
    /// manifest including this field, which would be self-referential.
    pub signature: String,
}

/// The subset of [`IntegrityManifest`] that is actually signed. `signature`
/// is excluded (self-referential), and `entries` is a `path`-sorted copy so
/// byte-for-byte agreement between signer and verifier does not depend on
/// the order a manifest happens to be constructed or deserialized in.
#[derive(serde::Serialize)]
struct CanonicalManifest<'a> {
    version: u32,
    entries: Vec<&'a ManifestEntry>,
}

impl IntegrityManifest {
    /// Deterministic JSON bytes over `{version, entries}` — never
    /// `signature` — with `entries` sorted by `path`. This is what
    /// [`IntegrityManifest::verify_signature`] checks the signature
    /// against, and what a signer must sign to produce a valid
    /// `signature`; signer and verifier must always compute this the same
    /// way for a legitimately-signed manifest to verify.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut entries: Vec<&ManifestEntry> = self.entries.iter().collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let canonical = CanonicalManifest {
            version: self.version,
            entries,
        };
        // `CanonicalManifest`'s fields are plain owned/borrowed String, u32,
        // and Vec — serde_json::to_vec over this shape cannot fail.
        serde_json::to_vec(&canonical).expect("CanonicalManifest serialization is infallible")
    }

    /// Verifies `self.signature` against
    /// [`IntegrityManifest::canonical_bytes`] using `public_key_text` as the
    /// sole trusted signer. Any failure — malformed key, malformed
    /// signature, or a signature that decodes fine but does not match —
    /// maps to [`SelfProtectError::Signature`], since a caller deciding
    /// whether to trust this manifest only ever needs "verified or not".
    pub fn verify_signature(&self, public_key_text: &str) -> Result<(), SelfProtectError> {
        penguin_update::verify(&self.canonical_bytes(), &self.signature, public_key_text)
            .map_err(|_| SelfProtectError::Signature)
    }
}

/// Where an [`IntegrityManifest`] comes from — abstracts local-file loading
/// today so a future controller-fetched source can implement the same
/// trait without callers changing.
pub trait ManifestSource {
    /// Loads and deserializes a manifest. Does not verify its signature —
    /// callers must call [`IntegrityManifest::verify_signature`] before
    /// trusting the result.
    fn load(&self) -> Result<IntegrityManifest, SelfProtectError>;
}

/// Loads an [`IntegrityManifest`] from a JSON file on local disk.
pub struct LocalFileSource {
    /// Path to the manifest JSON file.
    pub path: PathBuf,
}

impl ManifestSource for LocalFileSource {
    fn load(&self) -> Result<IntegrityManifest, SelfProtectError> {
        let bytes = std::fs::read(&self.path).map_err(SelfProtectError::Io)?;
        let manifest = serde_json::from_slice(&bytes).map_err(SelfProtectError::Parse)?;
        Ok(manifest)
    }
}

/// Test-only fixture: builds and signs an [`IntegrityManifest`] with a
/// throwaway minisign keypair, mirroring `penguin_update::verify`'s own
/// `sign_fixture` test helper (see that crate's `verify.rs`) — `minisign`
/// (signing-capable) is a dev-dependency only, never a production one; see
/// this crate's `Cargo.toml` for why.
#[cfg(test)]
mod testfix {
    use std::io::Cursor;

    use super::{IntegrityManifest, ManifestEntry};

    /// Returns `(public_key_text, manifest)`: a two-entry manifest signed
    /// over its own canonical bytes, ready for `verify_signature`.
    pub(crate) fn signed_manifest() -> (String, IntegrityManifest) {
        let mut manifest = IntegrityManifest {
            version: 1,
            entries: vec![
                ManifestEntry {
                    path: "bin/penguind".to_string(),
                    sha256: "a".repeat(64),
                    mode: 0o755,
                },
                ManifestEntry {
                    path: "bin/penguin".to_string(),
                    sha256: "b".repeat(64),
                    mode: 0o755,
                },
            ],
            signature: String::new(),
        };

        let keypair =
            minisign::KeyPair::generate_unencrypted_keypair().expect("generate minisign keypair");
        let data_reader = Cursor::new(manifest.canonical_bytes());
        let signature_box =
            minisign::sign(Some(&keypair.pk), &keypair.sk, data_reader, None, None).expect("sign");
        manifest.signature = signature_box.into_string();
        let public_key_text = keypair.pk.to_box().expect("public key box").into_string();

        (public_key_text, manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tampered_manifest_body_fails_signature() {
        // Fixture signed at test-setup time with a throwaway ed25519 key (see testdata helper).
        let (pubkey, mut manifest) = testfix::signed_manifest();
        assert!(manifest.verify_signature(&pubkey).is_ok());
        manifest.entries[0].sha256 = "0".repeat(64); // tamper the body, keep old signature
        assert!(matches!(
            manifest.verify_signature(&pubkey),
            Err(SelfProtectError::Signature)
        ));
    }
}
