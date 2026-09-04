//! Tiny filesystem helpers shared by the crash marker and the resolv.conf
//! backend: create-directory-and-write-file with owner-only permissions.
//!
//! No `unsafe` anywhere in this crate — Unix mode bits go through the
//! `std::os::unix::fs` extension traits rather than raw libc calls.
//! Non-Unix targets get a mode-less fallback; the only non-Unix consumer
//! is the Windows `netsh` backend, which has no file of its own to protect
//! (it shells a command; there's nothing here for it to call into that a
//! Unix-only mode bit would matter for).

use std::fs;
use std::path::Path;

use crate::sysresolver::error::SysResolverError;

/// Creates `dir` (and any missing parents) if it doesn't already exist,
/// mode 0700 on Unix. An existing directory's mode is left untouched.
pub fn create_dir_all_owner_only(dir: &Path) -> Result<(), SysResolverError> {
    if dir.exists() {
        return Ok(());
    }
    create_dir_all_owner_only_impl(dir)
}

#[cfg(unix)]
fn create_dir_all_owner_only_impl(dir: &Path) -> Result<(), SysResolverError> {
    use std::os::unix::fs::DirBuilderExt as _;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(dir).map_err(|source| SysResolverError::Io {
        context: format!("create directory {}", dir.display()),
        source,
    })
}

#[cfg(not(unix))]
fn create_dir_all_owner_only_impl(dir: &Path) -> Result<(), SysResolverError> {
    fs::create_dir_all(dir).map_err(|source| SysResolverError::Io {
        context: format!("create directory {}", dir.display()),
        source,
    })
}

/// Writes `data` to `path`, mode 0600 on Unix. Not atomic via
/// temp-file-then-rename — this mirrors the Go implementation's
/// direct-write behaviour for both the crash marker and the resolv.conf
/// backup/live file. Crash safety here comes from the marker's
/// write-before-mutate ordering (see `sysresolver::mod`), not from
/// per-file atomicity.
///
/// The mode is enforced with an explicit `chmod` after writing, not just
/// applied at creation: `open(..., O_CREAT, 0o600)` only sets the mode
/// when it actually creates the file, and both `resolv.conf` and its
/// backup routinely already exist (e.g. system-created at 0644) before
/// this is first called — Go's `os.WriteFile(path, data, 0o600)` has the
/// identical gap (same POSIX `open()` semantics) and so silently leaves a
/// pre-existing file's looser permissions in place. That gap is closed
/// here rather than ported.
pub fn write_owner_only(path: &Path, data: &[u8]) -> Result<(), SysResolverError> {
    write_owner_only_impl(path, data)
}

#[cfg(unix)]
fn write_owner_only_impl(path: &Path, data: &[u8]) -> Result<(), SysResolverError> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    use std::os::unix::fs::PermissionsExt as _;
    let opened = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path);
    let mut file = opened.map_err(|source| SysResolverError::Io {
        context: format!("create {}", path.display()),
        source,
    })?;
    file.write_all(data)
        .map_err(|source| SysResolverError::Io {
            context: format!("write {}", path.display()),
            source,
        })?;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|source| SysResolverError::Io {
            context: format!("chmod {}", path.display()),
            source,
        })
}

#[cfg(not(unix))]
fn write_owner_only_impl(path: &Path, data: &[u8]) -> Result<(), SysResolverError> {
    fs::write(path, data).map_err(|source| SysResolverError::Io {
        context: format!("write {}", path.display()),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_owner_only_creates_file_with_expected_contents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("f.txt");
        write_owner_only(&path, b"hello").expect("write");
        assert_eq!(fs::read(&path).expect("read"), b"hello");
    }

    #[cfg(unix)]
    #[test]
    fn write_owner_only_sets_mode_0600() {
        use std::os::unix::fs::MetadataExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("f.txt");
        write_owner_only(&path, b"hello").expect("write");
        let mode = fs::metadata(&path).expect("stat").mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn create_dir_all_owner_only_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("a").join("b");
        create_dir_all_owner_only(&nested).expect("first create");
        create_dir_all_owner_only(&nested).expect("second create is a no-op");
        assert!(nested.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn create_dir_all_owner_only_sets_mode_0700() {
        use std::os::unix::fs::MetadataExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("a");
        create_dir_all_owner_only(&nested).expect("create");
        let mode = fs::metadata(&nested).expect("stat").mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
}
