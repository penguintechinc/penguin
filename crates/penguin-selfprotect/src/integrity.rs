//! [`check`]: on-disk integrity verification against an [`crate::IntegrityManifest`].
//!
//! For each [`crate::ManifestEntry`], reads the file at `root.join(&entry.path)`,
//! computes its SHA-256 hash, and compares to the manifest's expected hash.
//! Missing or unreadable files produce a [`crate::TamperFinding`] with
//! [`crate::TamperKind::FileMissing`]; present but mismatched files are
//! classified as `BinaryModified`, `UnitModified`, or `ConfigModified` based
//! on the file name or path.

use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::{IntegrityManifest, SelfProtectError, TamperFinding, TamperKind};

/// Verifies that files referenced in a manifest exist on disk and match
/// their expected SHA-256 hashes. Returns a [`Vec`] of [`TamperFinding`]
/// for each file that failed the check; an empty vector means all files
/// passed.
///
/// Missing or unreadable files produce a `FileMissing` finding; present but
/// hash-mismatched files are classified by name: `.service` → `UnitModified`,
/// binary name (e.g. `penguind`) or no extension → `BinaryModified`, else
/// `ConfigModified`.
pub fn check(manifest: &IntegrityManifest, root: &Path) -> Vec<TamperFinding> {
    let mut findings: Vec<TamperFinding> = Vec::new();

    for entry in &manifest.entries {
        let file_path = root.join(&entry.path);

        // Try to read and hash the file.
        let actual_hash = match std::fs::read(&file_path) {
            Ok(contents) => {
                let mut hasher = Sha256::new();
                hasher.update(&contents);
                Some(format!("{:x}", hasher.finalize()))
            }
            Err(_) => None, // File missing or unreadable.
        };

        // If the hash matches, no finding.
        if let Some(ref hash) = actual_hash
            && hash == &entry.sha256
        {
            continue;
        }

        // Hash mismatch or file missing. Classify the tampering.
        let kind = if actual_hash.is_none() {
            TamperKind::FileMissing
        } else {
            classify_tamper(&entry.path)
        };

        findings.push(TamperFinding {
            path: entry.path.clone(),
            kind,
            expected_sha256: entry.sha256.clone(),
            actual_sha256: actual_hash,
        });
    }

    findings
}

/// Classifies a modified file based on its name/path.
///
/// - Ends with `.service` → `UnitModified`
/// - Matches a binary name (`penguind`) or has no extension → `BinaryModified`
/// - Otherwise → `ConfigModified`
fn classify_tamper(path: &str) -> TamperKind {
    if path.ends_with(".service") {
        return TamperKind::UnitModified;
    }

    // Check if the last path segment is the binary name or has no extension.
    if let Some(file_name) = path.split('/').next_back() {
        // Binary name check (e.g., "penguind" or "penguin").
        if file_name == "penguind" || file_name == "penguin" {
            return TamperKind::BinaryModified;
        }

        // No extension check.
        if !file_name.contains('.') {
            return TamperKind::BinaryModified;
        }
    }

    TamperKind::ConfigModified
}

/// Restores a tampered or missing file from its protected copy.
///
/// Copies the pristine file at `protected_dir.join(&finding.path)` over the
/// file at `target_root.join(&finding.path)`, creating parent directories as
/// needed and preserving the protected copy's file mode. Returns an error if
/// the protected copy is missing, unreadable, if I/O operations fail, or if
/// the restored bytes do not match the manifest's expected hash (see the
/// re-verification step below).
///
/// # Defense in depth: re-verifying the restored bytes
///
/// After copying, the freshly-written file at `target_path` is re-read and
/// re-hashed (SHA-256), and compared against `finding.expected_sha256`.
/// Without this, a *poisoned protected copy* — planted alongside the
/// tampered target, or corrupted independently — would be trusted blindly:
/// `heal` would report success, and the next [`check`] cycle would find the
/// target now matches that poisoned content (nothing compares it against
/// the manifest at that point), so the poisoned bytes get "healed" in and
/// re-healed every cycle without ever being flagged again. Re-reading from
/// disk (rather than just re-hashing the in-memory bytes that were about to
/// be written) also catches corruption introduced by the write itself, not
/// only a bad protected copy. On mismatch this returns
/// [`SelfProtectError::HealVerificationFailed`] — the poisoned copy is
/// never claimed as healed, and `scan_heal_report` surfaces the failure via
/// its per-finding remediation string instead of silently looping.
pub fn heal(
    finding: &TamperFinding,
    protected_dir: &Path,
    target_root: &Path,
) -> Result<(), SelfProtectError> {
    let protected_path = protected_dir.join(&finding.path);
    let target_path = target_root.join(&finding.path);

    // Read the protected (pristine) file.
    let protected_contents = fs::read(&protected_path).map_err(SelfProtectError::Io)?;

    // Get the protected copy's metadata to preserve file mode.
    let protected_metadata = fs::metadata(&protected_path).map_err(SelfProtectError::Io)?;

    // Create parent directories if they don't exist.
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent).map_err(SelfProtectError::Io)?;
    }

    // Write the protected contents to the target.
    fs::write(&target_path, protected_contents).map_err(SelfProtectError::Io)?;

    // Restore the file mode.
    let permissions = protected_metadata.permissions();
    fs::set_permissions(&target_path, permissions).map_err(SelfProtectError::Io)?;

    // Re-verify: hash what actually landed at `target_path` and compare
    // against the manifest's expected hash before claiming success — see
    // the doc above.
    let restored_contents = fs::read(&target_path).map_err(SelfProtectError::Io)?;
    let mut restored_hasher = Sha256::new();
    restored_hasher.update(&restored_contents);
    let restored_hash = format!("{:x}", restored_hasher.finalize());
    if restored_hash != finding.expected_sha256 {
        return Err(SelfProtectError::HealVerificationFailed);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: compute SHA-256 of bytes in lowercase hex.
    fn sha256_hex(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
    }

    /// Test helper: build a manifest with entries for the given (path, hash) pairs.
    mod testfix {
        use crate::{IntegrityManifest, ManifestEntry};

        pub(crate) fn manifest_for(entries: &[(&str, &str)]) -> IntegrityManifest {
            IntegrityManifest {
                version: 1,
                entries: entries
                    .iter()
                    .map(|(path, hash)| ManifestEntry {
                        path: path.to_string(),
                        sha256: hash.to_string(),
                        mode: 0o644,
                    })
                    .collect(),
                signature: String::new(), // Not used in this test.
            }
        }
    }

    #[test]
    fn check_flags_modified_and_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("penguind");
        std::fs::write(&good, b"real-binary").unwrap();

        // Build manifest: one file present (penguind) with the correct hash,
        // and one file missing (missing.conf).
        let original_binary_hash = sha256_hex(b"real-binary");
        let manifest = testfix::manifest_for(&[
            ("penguind", &original_binary_hash),
            ("missing.conf", &sha256_hex(b"x")),
        ]);

        // Corrupt the binary on disk.
        std::fs::write(&good, b"corrupted").unwrap();

        // Run the check.
        let findings = check(&manifest, dir.path());

        // Assert we found both failures.
        assert!(
            findings
                .iter()
                .any(|f| f.path == "penguind" && f.kind == TamperKind::BinaryModified),
            "Expected BinaryModified finding for penguind; got findings: {:?}",
            findings
        );
        assert!(
            findings
                .iter()
                .any(|f| f.path == "missing.conf" && f.kind == TamperKind::FileMissing),
            "Expected FileMissing finding for missing.conf; got findings: {:?}",
            findings
        );
    }

    #[test]
    fn heal_restores_a_corrupted_file_from_protected_copy() {
        let target = tempfile::tempdir().unwrap();
        let protected = tempfile::tempdir().unwrap();
        std::fs::write(protected.path().join("penguind"), b"real-binary").unwrap();
        std::fs::write(target.path().join("penguind"), b"corrupted").unwrap();
        let finding = TamperFinding {
            path: "penguind".into(),
            kind: TamperKind::BinaryModified,
            expected_sha256: sha256_hex(b"real-binary"),
            actual_sha256: Some(sha256_hex(b"corrupted")),
        };
        heal(&finding, protected.path(), target.path()).unwrap();
        assert_eq!(
            std::fs::read(target.path().join("penguind")).unwrap(),
            b"real-binary"
        );

        // Test error path: protected copy absent.
        let empty_protected = tempfile::tempdir().unwrap();
        let finding_missing = TamperFinding {
            path: "nonexistent".into(),
            kind: TamperKind::FileMissing,
            expected_sha256: sha256_hex(b"content"),
            actual_sha256: None,
        };
        assert!(heal(&finding_missing, empty_protected.path(), target.path()).is_err());
    }

    /// Finding 2 regression: a protected copy whose on-disk bytes do NOT
    /// match `expected_sha256` (as if the protected copy itself had been
    /// poisoned/tampered) must never be trusted — `heal` must fail rather
    /// than restoring the poisoned bytes and reporting success. Without the
    /// post-heal re-verification, this poisoned copy would be "healed" in
    /// once and then match on every subsequent `check` cycle, since nothing
    /// would compare the restored bytes against the manifest again.
    #[test]
    fn heal_rejects_a_poisoned_protected_copy_that_does_not_match_the_manifest_hash() {
        let target = tempfile::tempdir().unwrap();
        let protected = tempfile::tempdir().unwrap();
        // Protected copy exists and is readable, but its content does not
        // hash to `expected_sha256` below.
        std::fs::write(protected.path().join("penguind"), b"poisoned-backup").unwrap();
        std::fs::write(target.path().join("penguind"), b"corrupted").unwrap();

        let finding = TamperFinding {
            path: "penguind".into(),
            kind: TamperKind::BinaryModified,
            expected_sha256: sha256_hex(b"real-binary"), // does NOT match "poisoned-backup"
            actual_sha256: Some(sha256_hex(b"corrupted")),
        };

        let result = heal(&finding, protected.path(), target.path());
        assert!(
            matches!(result, Err(SelfProtectError::HealVerificationFailed)),
            "expected HealVerificationFailed for a poisoned protected copy, got: {result:?}"
        );
    }

    #[test]
    fn check_produces_no_finding_for_an_untampered_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, b"untouched").unwrap();
        let manifest = testfix::manifest_for(&[("config.yaml", &sha256_hex(b"untouched"))]);

        let findings = check(&manifest, dir.path());
        assert!(
            findings.is_empty(),
            "matching hash must not produce a finding: {findings:?}"
        );
    }

    #[test]
    fn classify_tamper_covers_unit_binary_and_config_paths() {
        assert_eq!(
            classify_tamper("etc/penguind.service"),
            TamperKind::UnitModified
        );
        assert_eq!(classify_tamper("bin/penguind"), TamperKind::BinaryModified);
        assert_eq!(classify_tamper("bin/penguin"), TamperKind::BinaryModified);
        assert_eq!(
            classify_tamper("bin/some-no-ext-tool"),
            TamperKind::BinaryModified
        );
        assert_eq!(
            classify_tamper("etc/config.yaml"),
            TamperKind::ConfigModified
        );
    }
}
