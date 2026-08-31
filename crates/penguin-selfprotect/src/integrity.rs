//! [`check`]: on-disk integrity verification against an [`crate::IntegrityManifest`].
//!
//! For each [`crate::ManifestEntry`], reads the file at `root.join(&entry.path)`,
//! computes its SHA-256 hash, and compares to the manifest's expected hash.
//! Missing or unreadable files produce a [`crate::TamperFinding`] with
//! [`crate::TamperKind::FileMissing`]; present but mismatched files are
//! classified as `BinaryModified`, `UnitModified`, or `ConfigModified` based
//! on the file name or path.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::{IntegrityManifest, TamperFinding, TamperKind};

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
}
