//! Thin adapter over OS file-metadata syscalls (permission bits + owning
//! uid).
//!
//! This is the *only* module in the crate that touches the real filesystem
//! for ownership decisions — every rule in [`crate::verify`] operates on the
//! [`FileMeta`] this module produces, never on a raw path, so the whole
//! decision matrix stays testable without root or a matching uid. Mirrors
//! how the Go reference's `StatInfo` interface isolates `os.Stat`.

use std::io;
use std::path::Path;

/// The subset of file metadata the verification pipeline needs: permission
/// bits and the owning uid (when the platform exposes one).
///
/// `uid: None` means "the platform has no uid concept for this file" (e.g.
/// non-Unix), which [`crate::verify`] treats the same way the Go reference's
/// failed `Sys()` type assertion does: skip the ownership check rather than
/// fail closed on a check that cannot be evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMeta {
    /// Raw permission bits (the low 9 bits are the rwxrwxrwx triplet the
    /// world-writable check inspects).
    pub mode: u32,
    /// The owning uid, if the platform has one.
    pub uid: Option<u32>,
}

/// Source of file metadata, injected into `Verifier` so tests can fake
/// ownership/permission bits without running the suite as another uid.
///
/// `Send + Sync` is a supertrait (not just a bound where `Verifier` uses it)
/// so every `dyn StatSource` trait object — including the boxed one
/// `Verifier` stores — is `Send + Sync` too, letting `Verifier` itself be
/// held across an `.await` point in async callers.
pub trait StatSource: Send + Sync {
    /// Stats `path`, returning its permission bits and owning uid.
    fn stat(&self, path: &Path) -> io::Result<FileMeta>;
}

/// Real OS-backed [`StatSource`], used outside of tests.
pub struct OsStat;

impl StatSource for OsStat {
    fn stat(&self, path: &Path) -> io::Result<FileMeta> {
        let meta = std::fs::metadata(path)?;
        Ok(real_file_meta(&meta))
    }
}

#[cfg(unix)]
fn real_file_meta(meta: &std::fs::Metadata) -> FileMeta {
    use std::os::unix::fs::MetadataExt;
    FileMeta {
        mode: meta.mode(),
        uid: Some(meta.uid()),
    }
}

// Non-Unix platforms have no uid concept; the ownership check degrades to
// "skip" for it, same as the Go reference's failed Sys() type assertion.
// World-writable bits do not exist in the same shape either, so mode reports
// as 0 (never world-writable) rather than guessing at an ACL translation.
#[cfg(not(unix))]
fn real_file_meta(_meta: &std::fs::Metadata) -> FileMeta {
    FileMeta { mode: 0, uid: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_stat_reads_real_metadata() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file_path = tmp.path().join("probe");
        std::fs::write(&file_path, b"probe").expect("write probe file");

        let meta = OsStat.stat(&file_path).expect("stat probe file");

        // Whatever the umask, a freshly-created regular file is never
        // world-writable, so this is a safe assertion in CI.
        assert_eq!(meta.mode & 0o002, 0);
    }

    #[test]
    fn os_stat_missing_path_is_an_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("does-not-exist");

        assert!(OsStat.stat(&missing).is_err());
    }
}
