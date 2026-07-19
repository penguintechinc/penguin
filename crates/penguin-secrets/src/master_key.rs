//! The 32-byte AEAD key that encrypts every record in [`crate::file_backend`].
//!
//! Exact port of the Go `EnsureMasterKey`: idempotent, rejects a
//! world-writable parent directory outright, and never silently accepts a
//! key file of the wrong size.

use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::Path;

use rand::RngCore as _;
use zeroize::{Zeroize, ZeroizeOnDrop};

use penguin_sdk::SecretError;

/// A 32-byte XChaCha20-Poly1305 key, wiped from memory on drop.
///
/// [`ensure_master_key`] is the only supported way to obtain one, so "where
/// does the key come from" stays a single, auditable function. Deliberately
/// does not derive `Debug` — a key must never end up in a log line.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct MasterKey([u8; 32]);

impl MasterKey {
    /// Borrows the raw key bytes for use as an AEAD key.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Creates or reads the 32-byte master key at `path`.
///
/// The parent directory must not be world-writable: that is a deliberate
/// hard error, not a nicety. A writable data directory lets any local user
/// swap in their own key file and silently intercept every secret this
/// store ever encrypts. A missing key file is generated fresh (32
/// cryptographically random bytes) and written back with mode 0600; an
/// existing file must be exactly 32 bytes, or the read fails naming the
/// actual size found.
pub fn ensure_master_key(path: &Path) -> Result<MasterKey, SecretError> {
    let dir = path.parent().ok_or_else(|| {
        SecretError::Other(format!(
            "master key path {} has no parent directory",
            path.display()
        ))
    })?;
    reject_world_writable(dir)?;

    match std::fs::read(path) {
        Ok(bytes) => key_from_existing_bytes(path, bytes),
        Err(err) if err.kind() == io::ErrorKind::NotFound => generate_and_write_key(path),
        Err(err) => Err(SecretError::Other(format!(
            "failed to read master key at {}: {err}",
            path.display()
        ))),
    }
}

/// Converts a freshly-read key file's bytes into a [`MasterKey`], or an
/// error naming both the expected and actual size if it is not 32 bytes.
fn key_from_existing_bytes(path: &Path, bytes: Vec<u8>) -> Result<MasterKey, SecretError> {
    if bytes.len() != 32 {
        return Err(SecretError::Other(format!(
            "master key at {} has wrong size (expected 32, got {})",
            path.display(),
            bytes.len()
        )));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(MasterKey(key))
}

/// Generates a fresh 32-byte key and writes it to `path` with mode 0600 set
/// at creation time, before any key bytes are written.
fn generate_and_write_key(path: &Path) -> Result<MasterKey, SecretError> {
    let mut key = [0u8; 32];
    rand::rng().fill_bytes(&mut key);

    let mut file = create_owner_only_file(path).map_err(|err| {
        SecretError::Other(format!(
            "failed to create master key file {}: {err}",
            path.display()
        ))
    })?;
    file.write_all(&key).map_err(|err| {
        SecretError::Other(format!(
            "failed to write master key to {}: {err}",
            path.display()
        ))
    })?;

    Ok(MasterKey(key))
}

/// Errors if `dir` is stat-able and world-writable. Mirrors the Go check:
/// `info.Mode()&0o002 != 0`.
#[cfg(unix)]
fn reject_world_writable(dir: &Path) -> Result<(), SecretError> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = std::fs::metadata(dir).map_err(|err| {
        SecretError::Other(format!("failed to stat directory {}: {err}", dir.display()))
    })?;
    if meta.mode() & 0o002 != 0 {
        return Err(SecretError::Other(format!(
            "directory {} is world-writable (security risk)",
            dir.display()
        )));
    }
    Ok(())
}

/// Non-Unix stub for [`reject_world_writable`]. Windows ACLs have no direct
/// equivalent of the POSIX "other-writable" bit, so this only confirms the
/// directory exists; a real ACL check is a follow-up, not attempted here.
#[cfg(not(unix))]
fn reject_world_writable(dir: &Path) -> Result<(), SecretError> {
    std::fs::metadata(dir).map(|_| ()).map_err(|err| {
        SecretError::Other(format!("failed to stat directory {}: {err}", dir.display()))
    })
}

/// Creates `path` with mode 0600 applied at creation, before any bytes are
/// written. A no-op mode on non-Unix targets, where this bit pattern has no
/// equivalent.
#[cfg(unix)]
fn create_owner_only_file(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

/// Non-Unix stub for [`create_owner_only_file`]; see its doc.
#[cfg(not(unix))]
fn create_owner_only_file(path: &Path) -> io::Result<std::fs::File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn mode_bits(path: &Path) -> u32 {
        use std::os::unix::fs::MetadataExt as _;
        std::fs::metadata(path).expect("stat").mode() & 0o777
    }

    #[test]
    fn creates_new_key_with_0600_perms_when_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("master.key");

        let key = ensure_master_key(&path).expect("ensure_master_key");
        assert_eq!(key.as_bytes().len(), 32);

        #[cfg(unix)]
        assert_eq!(mode_bits(&path), 0o600);
    }

    #[test]
    fn rereading_an_existing_key_returns_the_same_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("master.key");

        let first = ensure_master_key(&path).expect("first ensure_master_key");
        let second = ensure_master_key(&path).expect("second ensure_master_key");

        assert_eq!(first.as_bytes(), second.as_bytes());
    }

    #[test]
    fn wrong_size_file_is_rejected_with_size_in_message() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("master.key");
        std::fs::write(&path, vec![0u8; 16]).expect("write short key");

        // `MasterKey` deliberately has no `Debug` impl, so `expect_err`
        // (which would need to print an unexpected `Ok` value) can't be
        // used here; `.err()` sidesteps that without needing one.
        let err = ensure_master_key(&path)
            .err()
            .expect("wrong-sized key should error");
        let message = err.to_string();
        assert!(
            message.contains("16") && message.contains("32"),
            "error should mention both actual and expected size, got: {message}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn world_writable_parent_directory_is_rejected() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777))
            .expect("chmod 777");
        let path = dir.path().join("master.key");

        let err = ensure_master_key(&path)
            .err()
            .expect("world-writable dir should be rejected");
        assert!(err.to_string().contains("world-writable"), "got: {err}");
    }

    #[test]
    fn missing_parent_directory_is_an_error() {
        let path = Path::new("/nonexistent/parent/dir/master.key");
        assert!(ensure_master_key(path).is_err());
    }

    #[test]
    fn unreadable_key_path_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("master.key");
        std::fs::create_dir(&path).expect("mkdir where key file is expected");

        assert!(ensure_master_key(&path).is_err());
    }
}
