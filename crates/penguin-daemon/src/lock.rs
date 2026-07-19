//! The single-instance guard, ported from
//! `go-client/internal/daemon/lock_unix.go` / `lock_windows.go`.
//!
//! Both Go files are collapsed into one implementation here because [`fs4`]
//! already provides a cross-platform non-blocking exclusive file lock
//! (`flock` on Unix, `LockFileEx` on Windows), so there is no reason to
//! hand-roll the platform split the Go code needed.
//!
//! # Divergence from Go: the lock is released on drop
//!
//! Go's `AcquireLock` returns a release closure that the daemon never calls,
//! relying on process exit to drop the `flock`. Here [`acquire`] returns a
//! [`LockGuard`] whose [`Drop`] releases the lock deterministically. Drop
//! only unlocks — it never deletes the lock file — matching the flock-daemon
//! convention that the file itself is a permanent fixture of the state
//! directory, not per-run state.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;

/// An error acquiring the single-instance lock.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// Another process already holds the lock.
    #[error("penguind already running: lock file {path:?} is locked")]
    AlreadyRunning {
        /// The lock file path, echoed for operator diagnostics.
        path: PathBuf,
    },
    /// Opening or locking the file failed for a reason other than contention
    /// (missing parent directory, permissions, etc).
    #[error("acquire lock on {path:?}: {source}")]
    Acquire {
        /// The lock file path.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// Holds the daemon's single-instance lock for as long as it is alive.
///
/// Dropping the guard unlocks the file (see the module doc for why this is a
/// deliberate change from the Go reference).
#[derive(Debug)]
pub struct LockGuard {
    file: File,
    path: PathBuf,
}

impl LockGuard {
    /// The path of the lock file this guard holds.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // Best-effort: there is nothing actionable to do with an unlock
        // failure during drop, and the OS releases the flock on process exit
        // regardless of whether this call succeeds.
        let _ = self.file.unlock();
    }
}

/// Acquires the single-instance lock at `<state_dir>/penguind.lock`.
///
/// Opens (creating if absent) the lock file with mode 0600, then takes a
/// non-blocking exclusive lock on it. If another process already holds the
/// lock this returns [`LockError::AlreadyRunning`] immediately rather than
/// waiting for it to be released.
pub fn acquire(state_dir: &Path) -> Result<LockGuard, LockError> {
    let path = state_dir.join("penguind.lock");
    let file = open_lock_file(&path)?;

    let acquired = file
        .try_lock_exclusive()
        .map_err(|source| LockError::Acquire {
            path: path.clone(),
            source,
        })?;
    if !acquired {
        return Err(LockError::AlreadyRunning { path });
    }

    Ok(LockGuard { file, path })
}

/// Opens `path` for writing, creating it with mode 0600 if it does not exist.
/// An already-existing file keeps whatever mode it had (matching Go's
/// `os.OpenFile` semantics, where the mode argument only applies at creation).
fn open_lock_file(path: &Path) -> Result<File, LockError> {
    let mut options = OpenOptions::new();
    options.create(true).write(true);
    set_owner_only_create_mode(&mut options);
    options.open(path).map_err(|source| LockError::Acquire {
        path: path.to_path_buf(),
        source,
    })
}

/// Applies the 0600 owner-only creation mode to `options`. A no-op on
/// non-Unix targets, where this bit pattern has no equivalent.
#[cfg(unix)]
fn set_owner_only_create_mode(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

/// Non-Unix stub for [`set_owner_only_create_mode`]; see its doc.
#[cfg(not(unix))]
fn set_owner_only_create_mode(_options: &mut OpenOptions) {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn acquiring_a_fresh_lock_succeeds() {
        let dir = TempDir::new().unwrap();
        let guard = acquire(dir.path()).unwrap();
        assert_eq!(guard.path(), dir.path().join("penguind.lock"));
    }

    #[test]
    fn second_acquire_while_held_fails_with_the_exact_message() {
        let dir = TempDir::new().unwrap();
        let guard = acquire(dir.path()).unwrap();

        let err = acquire(dir.path()).unwrap_err();
        let path = dir.path().join("penguind.lock");
        assert_eq!(
            err.to_string(),
            format!("penguind already running: lock file {path:?} is locked")
        );

        drop(guard);
    }

    #[test]
    fn acquiring_again_after_the_guard_is_dropped_succeeds() {
        let dir = TempDir::new().unwrap();
        let first = acquire(dir.path()).unwrap();
        drop(first);

        let second = acquire(dir.path()).unwrap();
        drop(second);
    }

    #[test]
    fn the_lock_file_still_exists_after_the_guard_is_dropped() {
        let dir = TempDir::new().unwrap();
        let guard = acquire(dir.path()).unwrap();
        let path = guard.path().to_path_buf();
        drop(guard);

        assert!(std::fs::metadata(&path).is_ok());
    }
}
