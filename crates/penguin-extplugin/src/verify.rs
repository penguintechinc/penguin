//! Plugin verification pipeline: ownership, world-writable refusal, SHA256
//! integrity, and minisign signature — in that order, failing closed at the
//! first violation. Mirrors go-client/internal/extplugin/verify.go.
//!
//! There is no trust-on-first-use: an unpinned publisher key is always a
//! hard failure ([`VerifyError::UntrustedSigner`]), never a signal to add
//! the key to the trusted set.

use std::fs;
use std::io;
use std::io::Read;
use std::path::{Path, PathBuf};

use minisign_verify::{PublicKey, Signature};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::manifest::Manifest;
use crate::os_stat::{OsStat, StatSource};

/// Default directory scanned for additional pinned publisher keys
/// (`*.pub` files), matching the Go daemon's system trust path.
pub const DEFAULT_TRUSTED_PUBLISHERS_DIR: &str = "/etc/penguin/trusted-publishers.d";

/// Placeholder embedded PenguinTech publisher key, ported verbatim from the
/// Go reference's `embeddedPublicKey`.
///
/// TODO: replace with the real production signing key before release. This
/// text decodes to 41 bytes, one short of a valid 42-byte minisign key, so
/// it can never itself verify a signature — the Go reference carries the
/// same placeholder with the same defect, and fixing it is out of scope for
/// this port (a real key rotation, not a parsing change).
const EMBEDDED_PUBLIC_KEY: &str = "untrusted comment: minisign public key\nRWQf7zLn5+DYjyZ8ZWIrasJVjMKWePWGVgvBvF40FmkT7K7VZV7EVwA=\n";

/// Every distinct way plugin verification can fail, so callers (and tests)
/// can branch on the reason rather than parse error text.
#[derive(Debug, Error)]
pub enum VerifyError {
    /// `stat()` on the plugin directory or binary failed (missing,
    /// permission denied, etc).
    #[error("stat {path}: {source}")]
    Stat {
        /// The path that could not be stat'd.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The directory or file has world-writable permission bits set.
    #[error("{path} is world-writable (mode {mode:o})")]
    WorldWritable {
        /// The offending path.
        path: PathBuf,
        /// The full mode bits, for the error message and logs.
        mode: u32,
    },
    /// The owning uid is neither root (0) nor the expected daemon uid.
    #[error("{path} owned by uid {uid}, expected root (0) or daemon uid {expected_uid}")]
    WrongOwner {
        /// The offending path.
        path: PathBuf,
        /// The actual owning uid.
        uid: u32,
        /// The uid the caller expects to own plugin files.
        expected_uid: u32,
    },
    /// The binary could not be opened or read to compute its hash.
    #[error("read {path} for hashing: {source}")]
    ReadBinary {
        /// The binary path.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// The binary's SHA256 does not match the manifest.
    #[error("sha256 mismatch: expected {expected}, got {actual}")]
    Sha256Mismatch {
        /// The hash the manifest declared.
        expected: String,
        /// The hash actually computed from the binary.
        actual: String,
    },
    /// The `.minisig` signature file could not be read.
    #[error("read signature {path}: {source}")]
    ReadSignature {
        /// The signature file path.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// The signature file's content is not a well-formed minisign
    /// signature (corrupt, truncated, or garbage bytes).
    #[error("{path} is not a valid minisign signature")]
    MalformedSignature {
        /// The signature file path.
        path: PathBuf,
    },
    /// The signature decoded fine, but no pinned publisher key verifies it
    /// — either it was signed by an unknown key, or the trusted set is
    /// empty. There is no trust-on-first-use fallback for this case.
    #[error("no pinned publisher key verifies this signature")]
    UntrustedSigner,
}

/// Verifies a plugin directory against its manifest: directory ownership,
/// binary ownership, SHA256, then minisign signature — in that order,
/// stopping at the first failing check.
pub struct Verifier {
    stat: Box<dyn StatSource>,
    trusted_public_keys: Vec<String>,
}

impl Verifier {
    /// Production verifier: the embedded PenguinTech key plus any `*.pub`
    /// files found under [`DEFAULT_TRUSTED_PUBLISHERS_DIR`].
    pub fn new() -> Self {
        Self::with_trusted_dir(Path::new(DEFAULT_TRUSTED_PUBLISHERS_DIR))
    }

    /// Same as [`Verifier::new`], but scans `trusted_dir` for pinned
    /// publisher keys instead of the hardcoded system path — lets tests
    /// point at a tempdir instead of touching `/etc`.
    pub fn with_trusted_dir(trusted_dir: &Path) -> Self {
        let mut keys = vec![EMBEDDED_PUBLIC_KEY.to_string()];
        keys.extend(load_trusted_keys(trusted_dir));
        Verifier {
            stat: Box::new(OsStat),
            trusted_public_keys: keys,
        }
    }

    /// Test-only verifier: exactly the given trusted keys, no embedded key,
    /// no directory scan. Mirrors the Go reference's `NewVerifierWithKeys`
    /// test helper.
    pub fn with_keys(trusted_public_keys: Vec<String>) -> Self {
        Verifier {
            stat: Box::new(OsStat),
            trusted_public_keys,
        }
    }

    /// Swaps in a fake [`StatSource`] so ownership/permission checks can be
    /// exercised without running the suite as another uid.
    #[cfg(test)]
    fn set_stat_for_testing(&mut self, stat: impl StatSource + 'static) {
        self.stat = Box::new(stat);
    }

    /// Runs the full pipeline against `plugin_dir` / `manifest`. `expected_uid`
    /// is the uid plugin files must be owned by (besides root) — the caller
    /// supplies it (typically its own uid) rather than this crate reading it
    /// off the OS, so the whole pipeline stays privilege-free to test.
    pub fn verify(
        &self,
        plugin_dir: &Path,
        manifest: &Manifest,
        expected_uid: u32,
    ) -> Result<(), VerifyError> {
        self.verify_ownership(plugin_dir, expected_uid)?;

        let binary_path = manifest.binary_path(plugin_dir);
        self.verify_ownership(&binary_path, expected_uid)?;

        self.verify_sha256(&binary_path, &manifest.sha256)?;

        let sig_path = manifest.signature_path(plugin_dir);
        self.verify_signature(&binary_path, &sig_path)?;

        Ok(())
    }

    /// Checks that `path` is not world-writable and is owned by root or
    /// `expected_uid`. Shared by the directory and binary ownership checks
    /// (Go has two near-identical functions for this; one suffices here
    /// since both operate on the same [`FileMeta`] shape).
    fn verify_ownership(&self, path: &Path, expected_uid: u32) -> Result<(), VerifyError> {
        let meta = self.stat.stat(path).map_err(|source| VerifyError::Stat {
            path: path.to_path_buf(),
            source,
        })?;

        if meta.mode & 0o002 != 0 {
            return Err(VerifyError::WorldWritable {
                path: path.to_path_buf(),
                mode: meta.mode,
            });
        }

        let Some(uid) = meta.uid else {
            // No uid concept on this platform (e.g. non-Unix) — skip the
            // ownership check rather than fail closed on an unanswerable
            // question, matching the Go reference's failed Sys() assertion.
            return Ok(());
        };
        if uid != 0 && uid != expected_uid {
            return Err(VerifyError::WrongOwner {
                path: path.to_path_buf(),
                uid,
                expected_uid,
            });
        }

        Ok(())
    }

    /// Streams the binary through SHA256 and compares against the
    /// manifest's hex-encoded expectation.
    fn verify_sha256(&self, binary_path: &Path, expected_hex: &str) -> Result<(), VerifyError> {
        let mut file = fs::File::open(binary_path).map_err(|source| VerifyError::ReadBinary {
            path: binary_path.to_path_buf(),
            source,
        })?;

        let mut hasher = Sha256::new();
        let mut buf = [0u8; 8192];
        loop {
            let n = file
                .read(&mut buf)
                .map_err(|source| VerifyError::ReadBinary {
                    path: binary_path.to_path_buf(),
                    source,
                })?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }

        let actual = hex_encode(&hasher.finalize());
        if actual != expected_hex {
            return Err(VerifyError::Sha256Mismatch {
                expected: expected_hex.to_string(),
                actual,
            });
        }

        Ok(())
    }

    /// Reads the `.minisig` file and the binary, then tries every trusted
    /// key in turn until one verifies. No key matching means no trust —
    /// there is no fallback that pins an unrecognized signer on the fly.
    fn verify_signature(&self, binary_path: &Path, sig_path: &Path) -> Result<(), VerifyError> {
        let sig_text =
            fs::read_to_string(sig_path).map_err(|source| VerifyError::ReadSignature {
                path: sig_path.to_path_buf(),
                source,
            })?;
        let signature =
            Signature::decode(&sig_text).map_err(|_| VerifyError::MalformedSignature {
                path: sig_path.to_path_buf(),
            })?;

        let binary_data = fs::read(binary_path).map_err(|source| VerifyError::ReadBinary {
            path: binary_path.to_path_buf(),
            source,
        })?;

        for key_text in &self.trusted_public_keys {
            let Ok(public_key) = PublicKey::decode(key_text) else {
                // Malformed trusted key (e.g. the placeholder embedded key)
                // — skip it and keep trying the rest, same as the Go
                // reference's UnmarshalText-fails-continue loop.
                continue;
            };
            // allow_legacy=true accepts both prehashed and legacy-mode
            // signatures, matching the Go aead.dev/minisign library's
            // automatic mode detection.
            if public_key.verify(&binary_data, &signature, true).is_ok() {
                return Ok(());
            }
        }

        Err(VerifyError::UntrustedSigner)
    }
}

impl Default for Verifier {
    fn default() -> Self {
        Self::new()
    }
}

/// Best-effort scan of `trusted_dir` for `*.pub` minisign key files. A
/// missing directory is not an error — the system trust directory is
/// optional and most hosts will not have one.
fn load_trusted_keys(trusted_dir: &Path) -> Vec<String> {
    let mut keys = Vec::new();

    let Ok(entries) = fs::read_dir(trusted_dir) else {
        return keys;
    };

    for entry in entries.flatten() {
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if is_dir || !name.ends_with(".pub") {
            continue;
        }
        if let Ok(contents) = fs::read_to_string(entry.path()) {
            keys.push(contents);
        }
    }

    keys
}

/// Lowercase hex encoding, avoiding a dependency for a 32-byte digest.
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::os_stat::FileMeta;

    /// Fixture binary content matching the committed minisign fixtures
    /// (`tests/fixtures/test-binary.pub` / `test-binary.minisig`) — the
    /// signature was generated once by the real `minisign` CLI over exactly
    /// these bytes and is not reproducible in-test since `minisign-verify`
    /// is verify-only.
    const FIXTURE_BINARY: &[u8] = b"test binary content";
    const FIXTURE_SHA256: &str = "56681959d2de970a2dbee51710bb02862bec0a603b725443b92063c02b5f0a0c";
    const FIXTURE_PUBLIC_KEY: &str = include_str!("../tests/fixtures/test-binary.pub");
    const FIXTURE_SIGNATURE: &str = include_str!("../tests/fixtures/test-binary.minisig");

    /// Deterministic in-memory [`StatSource`] for exercising ownership
    /// rules without touching real file metadata.
    struct FakeStat {
        entries: Vec<(PathBuf, FileMeta)>,
    }

    impl FakeStat {
        fn new() -> Self {
            FakeStat {
                entries: Vec::new(),
            }
        }

        fn with(mut self, path: &Path, mode: u32, uid: Option<u32>) -> Self {
            self.entries
                .push((path.to_path_buf(), FileMeta { mode, uid }));
            self
        }
    }

    impl StatSource for FakeStat {
        fn stat(&self, path: &Path) -> io::Result<FileMeta> {
            for (entry_path, meta) in &self.entries {
                if entry_path == path {
                    return Ok(*meta);
                }
            }
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no such file or directory",
            ))
        }
    }

    /// Lays out a plugin directory with a real binary + real minisig
    /// signature from the committed fixtures, and a manifest whose sha256
    /// matches. Ownership stays real-filesystem (owned by the test
    /// process), so `expected_uid` must be the caller's own uid.
    fn signed_plugin_dir() -> (tempfile::TempDir, Manifest) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let plugin_dir = tmp.path().join("test-plugin");
        fs::create_dir(&plugin_dir).expect("mkdir plugin dir");
        fs::write(plugin_dir.join("test-binary"), FIXTURE_BINARY).expect("write binary");
        fs::write(plugin_dir.join("test-binary.minisig"), FIXTURE_SIGNATURE).expect("write sig");

        let manifest = Manifest {
            name: String::from("test-plugin"),
            version: String::from("1.0.0"),
            sdk_version: String::from("v1"),
            binary: String::from("test-binary"),
            sha256: String::from(FIXTURE_SHA256),
            publisher: String::from("test"),
        };

        (tmp, manifest)
    }

    #[cfg(unix)]
    fn owner_uid_of(path: &Path) -> u32 {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).expect("stat for uid").uid()
    }

    #[test]
    fn verify_happy_path_accepts_a_correctly_signed_plugin() {
        let (tmp, manifest) = signed_plugin_dir();
        let plugin_dir = tmp.path().join("test-plugin");
        let verifier = Verifier::with_keys(vec![FIXTURE_PUBLIC_KEY.to_string()]);

        let expected_uid = owner_uid_of(&plugin_dir);
        let result = verifier.verify(&plugin_dir, &manifest, expected_uid);

        assert!(result.is_ok(), "verify failed: {:?}", result.err());
    }

    #[test]
    fn verify_rejects_tampered_binary_via_sha256_mismatch() {
        let (tmp, manifest) = signed_plugin_dir();
        let plugin_dir = tmp.path().join("test-plugin");
        // Flip one byte of the binary after the manifest hash was fixed.
        let mut tampered = FIXTURE_BINARY.to_vec();
        tampered[0] ^= 0x01;
        fs::write(plugin_dir.join("test-binary"), &tampered).expect("write tampered binary");
        let verifier = Verifier::with_keys(vec![FIXTURE_PUBLIC_KEY.to_string()]);

        let expected_uid = owner_uid_of(&plugin_dir);
        let err = verifier
            .verify(&plugin_dir, &manifest, expected_uid)
            .expect_err("tampered binary must be rejected");

        assert!(matches!(err, VerifyError::Sha256Mismatch { .. }));
    }

    #[test]
    fn verify_rejects_unpinned_publisher_key_no_tofu() {
        let (tmp, manifest) = signed_plugin_dir();
        let plugin_dir = tmp.path().join("test-plugin");
        // The verifier trusts a *different* key than the one that actually
        // signed the fixture — this must be a hard failure, never an
        // implicit "first use" acceptance of the real signer.
        let unrelated_key = "untrusted comment: minisign public key\nRWTAgXjsPtrZsGKGN22DI2bk4CMTapwGY8J1Z0EqdwujDamAAAAAAAA\n";
        let verifier = Verifier::with_keys(vec![unrelated_key.to_string()]);

        let expected_uid = owner_uid_of(&plugin_dir);
        let err = verifier
            .verify(&plugin_dir, &manifest, expected_uid)
            .expect_err("unpinned signer must be rejected, never trust-on-first-use");

        assert!(matches!(err, VerifyError::UntrustedSigner));
    }

    #[test]
    fn verify_rejects_when_trusted_key_set_is_empty() {
        let (tmp, manifest) = signed_plugin_dir();
        let plugin_dir = tmp.path().join("test-plugin");
        let verifier = Verifier::with_keys(Vec::new());

        let expected_uid = owner_uid_of(&plugin_dir);
        let err = verifier
            .verify(&plugin_dir, &manifest, expected_uid)
            .expect_err("no trusted keys must be rejected");

        assert!(matches!(err, VerifyError::UntrustedSigner));
    }

    #[test]
    fn verify_rejects_tampered_signature_bytes() {
        let (tmp, manifest) = signed_plugin_dir();
        let plugin_dir = tmp.path().join("test-plugin");
        let tampered_sig = tamper_signature_line(FIXTURE_SIGNATURE);
        fs::write(plugin_dir.join("test-binary.minisig"), tampered_sig)
            .expect("write tampered sig");
        let verifier = Verifier::with_keys(vec![FIXTURE_PUBLIC_KEY.to_string()]);

        let expected_uid = owner_uid_of(&plugin_dir);
        let err = verifier
            .verify(&plugin_dir, &manifest, expected_uid)
            .expect_err("tampered signature must be rejected");

        assert!(matches!(
            err,
            VerifyError::UntrustedSigner | VerifyError::MalformedSignature { .. }
        ));
    }

    #[test]
    fn verify_rejects_missing_signature_file() {
        let (tmp, manifest) = signed_plugin_dir();
        let plugin_dir = tmp.path().join("test-plugin");
        fs::remove_file(plugin_dir.join("test-binary.minisig")).expect("remove sig");
        let verifier = Verifier::with_keys(vec![FIXTURE_PUBLIC_KEY.to_string()]);

        let expected_uid = owner_uid_of(&plugin_dir);
        let err = verifier
            .verify(&plugin_dir, &manifest, expected_uid)
            .expect_err("missing signature file must be rejected");

        assert!(matches!(err, VerifyError::ReadSignature { .. }));
    }

    #[test]
    fn verify_rejects_malformed_signature_file() {
        let (tmp, manifest) = signed_plugin_dir();
        let plugin_dir = tmp.path().join("test-plugin");
        fs::write(
            plugin_dir.join("test-binary.minisig"),
            b"not a valid signature",
        )
        .expect("write garbage sig");
        let verifier = Verifier::with_keys(vec![FIXTURE_PUBLIC_KEY.to_string()]);

        let expected_uid = owner_uid_of(&plugin_dir);
        let err = verifier
            .verify(&plugin_dir, &manifest, expected_uid)
            .expect_err("malformed signature file must be rejected");

        assert!(matches!(err, VerifyError::MalformedSignature { .. }));
    }

    #[test]
    fn verify_rejects_world_writable_directory() {
        let plugin_dir = PathBuf::from("/fake/test-plugin");
        let manifest = Manifest {
            name: String::from("test-plugin"),
            version: String::new(),
            sdk_version: String::new(),
            binary: String::from("test-binary"),
            sha256: String::from(FIXTURE_SHA256),
            publisher: String::new(),
        };
        let mut verifier = Verifier::with_keys(Vec::new());
        let fake = FakeStat::new().with(&plugin_dir, 0o777, Some(0));
        verifier.set_stat_for_testing(fake);

        let err = verifier
            .verify(&plugin_dir, &manifest, 1000)
            .expect_err("world-writable dir must be rejected");

        assert!(matches!(err, VerifyError::WorldWritable { .. }));
    }

    #[test]
    fn verify_rejects_world_writable_binary() {
        let plugin_dir = PathBuf::from("/fake/test-plugin");
        let binary_path = plugin_dir.join("test-binary");
        let manifest = Manifest {
            name: String::from("test-plugin"),
            version: String::new(),
            sdk_version: String::new(),
            binary: String::from("test-binary"),
            sha256: String::from(FIXTURE_SHA256),
            publisher: String::new(),
        };
        let mut verifier = Verifier::with_keys(Vec::new());
        let fake =
            FakeStat::new()
                .with(&plugin_dir, 0o750, Some(0))
                .with(&binary_path, 0o777, Some(0));
        verifier.set_stat_for_testing(fake);

        let err = verifier
            .verify(&plugin_dir, &manifest, 1000)
            .expect_err("world-writable binary must be rejected");

        assert!(matches!(err, VerifyError::WorldWritable { .. }));
    }

    #[test]
    fn verify_rejects_directory_owned_by_unrelated_uid() {
        let plugin_dir = PathBuf::from("/fake/test-plugin");
        let manifest = Manifest {
            name: String::from("test-plugin"),
            version: String::new(),
            sdk_version: String::new(),
            binary: String::from("test-binary"),
            sha256: String::from(FIXTURE_SHA256),
            publisher: String::new(),
        };
        let mut verifier = Verifier::with_keys(Vec::new());
        let fake = FakeStat::new().with(&plugin_dir, 0o750, Some(9999));
        verifier.set_stat_for_testing(fake);

        let err = verifier
            .verify(&plugin_dir, &manifest, 1000)
            .expect_err("dir owned by an unrelated uid must be rejected");

        assert!(matches!(
            err,
            VerifyError::WrongOwner {
                uid: 9999,
                expected_uid: 1000,
                ..
            }
        ));
    }

    #[test]
    fn verify_rejects_binary_owned_by_unrelated_uid() {
        let plugin_dir = PathBuf::from("/fake/test-plugin");
        let binary_path = plugin_dir.join("test-binary");
        let manifest = Manifest {
            name: String::from("test-plugin"),
            version: String::new(),
            sdk_version: String::new(),
            binary: String::from("test-binary"),
            sha256: String::from(FIXTURE_SHA256),
            publisher: String::new(),
        };
        let mut verifier = Verifier::with_keys(Vec::new());
        let fake =
            FakeStat::new()
                .with(&plugin_dir, 0o750, Some(0))
                .with(&binary_path, 0o600, Some(9999));
        verifier.set_stat_for_testing(fake);

        let err = verifier
            .verify(&plugin_dir, &manifest, 1000)
            .expect_err("binary owned by an unrelated uid must be rejected");

        assert!(matches!(
            err,
            VerifyError::WrongOwner {
                uid: 9999,
                expected_uid: 1000,
                ..
            }
        ));
    }

    #[test]
    fn verify_accepts_root_owned_paths_regardless_of_expected_uid() {
        let plugin_dir = PathBuf::from("/fake/test-plugin");
        let binary_path = plugin_dir.join("test-binary");
        let manifest = Manifest {
            name: String::from("test-plugin"),
            version: String::new(),
            sdk_version: String::new(),
            binary: String::from("test-binary"),
            sha256: String::from(FIXTURE_SHA256),
            publisher: String::new(),
        };
        let mut verifier = Verifier::with_keys(Vec::new());
        // Root-owned (uid 0) must pass ownership even though expected_uid
        // is something else entirely — but the pipeline still fails later,
        // at the (unfaked) real sha256 read of a nonexistent binary.
        let fake =
            FakeStat::new()
                .with(&plugin_dir, 0o750, Some(0))
                .with(&binary_path, 0o600, Some(0));
        verifier.set_stat_for_testing(fake);

        let err = verifier
            .verify(&plugin_dir, &manifest, 1000)
            .expect_err("nonexistent binary must fail at the read-for-hash step");

        assert!(matches!(err, VerifyError::ReadBinary { .. }));
    }

    #[test]
    fn verify_dir_stat_error_is_surfaced() {
        let plugin_dir = PathBuf::from("/fake/test-plugin");
        let manifest = Manifest {
            name: String::from("test-plugin"),
            version: String::new(),
            sdk_version: String::new(),
            binary: String::from("test-binary"),
            sha256: String::new(),
            publisher: String::new(),
        };
        let mut verifier = Verifier::with_keys(Vec::new());
        verifier.set_stat_for_testing(FakeStat::new());

        let err = verifier
            .verify(&plugin_dir, &manifest, 1000)
            .expect_err("stat error must be surfaced");

        assert!(matches!(err, VerifyError::Stat { .. }));
    }

    #[test]
    fn ownership_check_skips_uid_when_platform_has_none() {
        let plugin_dir = PathBuf::from("/fake/test-plugin");
        let mut verifier = Verifier::with_keys(Vec::new());
        let fake = FakeStat::new().with(&plugin_dir, 0o750, None);
        verifier.set_stat_for_testing(fake);

        // No uid on this entry — ownership must be treated as satisfied,
        // never rejected for a question the platform cannot answer.
        assert!(verifier.verify_ownership(&plugin_dir, 1000).is_ok());
    }

    #[test]
    fn load_trusted_keys_from_missing_directory_returns_empty() {
        let missing = PathBuf::from("/definitely/does/not/exist/trusted-publishers.d");
        assert!(load_trusted_keys(&missing).is_empty());
    }

    #[test]
    fn with_trusted_dir_loads_pub_files_and_skips_others() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::write(tmp.path().join("acme.pub"), FIXTURE_PUBLIC_KEY).expect("write pub file");
        fs::write(tmp.path().join("readme.txt"), "not a key").expect("write non-pub file");

        let verifier = Verifier::with_trusted_dir(tmp.path());

        assert!(
            verifier
                .trusted_public_keys
                .contains(&FIXTURE_PUBLIC_KEY.to_string())
        );
        // Only the embedded placeholder key plus the one *.pub file — the
        // non-.pub file must not have been picked up.
        assert_eq!(verifier.trusted_public_keys.len(), 2);
    }

    #[test]
    fn default_verifier_matches_new() {
        let verifier = Verifier::default();
        assert!(!verifier.trusted_public_keys.is_empty());
    }

    /// Flips one base64 character (to a different, still-valid base64
    /// character) in the fixture's global-signature line, so the file
    /// still decodes structurally but no longer verifies cryptographically.
    fn tamper_signature_line(sig_text: &str) -> String {
        let mut lines: Vec<&str> = sig_text.lines().collect();
        let target_line = lines[3];
        let mut chars: Vec<char> = target_line.chars().collect();
        let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let original = chars[0];
        for candidate in alphabet.chars() {
            if candidate != original {
                chars[0] = candidate;
                break;
            }
        }
        let tampered_line: String = chars.into_iter().collect();
        lines[3] = &tampered_line;
        let joined = lines.join("\n");
        format!("{joined}\n")
    }
}
