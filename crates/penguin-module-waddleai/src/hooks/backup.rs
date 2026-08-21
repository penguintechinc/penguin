//! Byte-for-byte backup/restore for one shim's config edit — the mechanism
//! that lets [`super::uninstall`] put a config file back exactly the way it
//! found it, unrelated content and all.
//!
//! Two files per ecosystem, both under the module's private state directory
//! (`host.data_dir()/hooks/`, never next to the config file itself, so a
//! stray edit to the target file can never also corrupt its own backup):
//!
//! - `<ecosystem>.meta.json` — a small manifest recording the target path
//!   and whether it existed before install.
//! - `<ecosystem>.orig` — the target file's exact original bytes, present
//!   only when the manifest's `existed` is `true`.
//!
//! Storing the original bytes directly (not JSON-embedded, not
//! base64-encoded) means [`restore`] never round-trips them through any
//! encoding — the file [`super::uninstall`] writes back is bit-identical to
//! what [`snapshot`] read.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fsutil;

/// The manifest recorded alongside a snapshot's raw-bytes sibling file.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    /// The config file path this backup was taken for, kept for diagnostics
    /// (not used to locate anything — the caller always supplies the
    /// current target path).
    target_path: String,
    /// Whether the target file existed before the first install.
    existed: bool,
}

fn meta_path(backup_dir: &Path, ecosystem: &str) -> PathBuf {
    backup_dir.join(format!("{ecosystem}.meta.json"))
}

fn orig_path(backup_dir: &Path, ecosystem: &str) -> PathBuf {
    backup_dir.join(format!("{ecosystem}.orig"))
}

/// Whether a backup has already been taken for `ecosystem` — install uses
/// this to decide whether to snapshot at all (never re-snapshot over an
/// existing backup; see [`snapshot`]'s doc).
pub fn exists(backup_dir: &Path, ecosystem: &str) -> bool {
    meta_path(backup_dir, ecosystem).is_file()
}

/// Snapshots `target`'s current bytes (or records that it did not exist)
/// for `ecosystem`. A no-op if a backup already exists (checked internally,
/// not left to the caller): a second `install` — e.g. after a version bump
/// changed the merged entry's shape — must never overwrite the *original*
/// pre-WaddleAI state with a state that already has WaddleAI's own merge
/// applied.
pub fn snapshot(backup_dir: &Path, ecosystem: &str, target: &Path) -> std::io::Result<()> {
    if exists(backup_dir, ecosystem) {
        return Ok(());
    }

    let original = match std::fs::read(target) {
        Ok(bytes) => Some(bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(err),
    };

    let manifest = Manifest {
        target_path: target.display().to_string(),
        existed: original.is_some(),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;

    if let Some(bytes) = &original {
        fsutil::write_atomic(&orig_path(backup_dir, ecosystem), bytes)?;
    }
    // The manifest is written last: its presence is what `exists` treats as
    // "a backup was taken", so a crash between the two writes above never
    // leaves a manifest pointing at bytes that were never actually saved.
    fsutil::write_atomic(&meta_path(backup_dir, ecosystem), &manifest_bytes)
}

/// One loaded backup: the original bytes, if the target existed before
/// install.
pub struct Backup {
    pub original: Option<Vec<u8>>,
}

/// Loads the backup for `ecosystem`, if one exists.
pub fn load(backup_dir: &Path, ecosystem: &str) -> std::io::Result<Option<Backup>> {
    let meta_bytes = match std::fs::read(meta_path(backup_dir, ecosystem)) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let manifest: Manifest = serde_json::from_slice(&meta_bytes)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;

    let original = if manifest.existed {
        Some(std::fs::read(orig_path(backup_dir, ecosystem))?)
    } else {
        None
    };

    Ok(Some(Backup { original }))
}

/// Removes the backup for `ecosystem` (both files, tolerating either being
/// already absent) — called once [`super::uninstall`] has finished
/// restoring the target.
pub fn clear(backup_dir: &Path, ecosystem: &str) -> std::io::Result<()> {
    remove_if_present(&meta_path(backup_dir, ecosystem))?;
    remove_if_present(&orig_path(backup_dir, ecosystem))
}

fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_of_a_missing_target_records_did_not_exist() {
        let backup_dir = tempfile::tempdir().unwrap();
        let target = backup_dir.path().join("does-not-exist.json");

        snapshot(backup_dir.path(), "claude", &target).expect("snapshot succeeds");
        assert!(exists(backup_dir.path(), "claude"));

        let backup = load(backup_dir.path(), "claude").unwrap().unwrap();
        assert!(backup.original.is_none());
    }

    #[test]
    fn snapshot_of_an_existing_target_preserves_its_exact_bytes() {
        let backup_dir = tempfile::tempdir().unwrap();
        let target = backup_dir.path().join("settings.json");
        let original_bytes = b"{\"unrelated\": true}\n";
        std::fs::write(&target, original_bytes).unwrap();

        snapshot(backup_dir.path(), "claude", &target).expect("snapshot succeeds");

        let backup = load(backup_dir.path(), "claude").unwrap().unwrap();
        assert_eq!(backup.original.as_deref(), Some(original_bytes.as_slice()));
    }

    #[test]
    fn a_second_snapshot_is_a_no_op_and_keeps_the_first_original() {
        let backup_dir = tempfile::tempdir().unwrap();
        let target = backup_dir.path().join("settings.json");
        std::fs::write(&target, b"original").unwrap();
        snapshot(backup_dir.path(), "claude", &target).unwrap();

        // Simulate the merged file now sitting at `target`, then a second
        // `install` call for the same ecosystem — `snapshot` must detect the
        // existing backup itself and leave the true original untouched.
        std::fs::write(&target, b"merged").unwrap();
        snapshot(backup_dir.path(), "claude", &target).unwrap();

        let backup = load(backup_dir.path(), "claude").unwrap().unwrap();
        assert_eq!(backup.original.as_deref(), Some(b"original".as_slice()));
    }

    #[test]
    fn load_of_a_missing_backup_is_none() {
        let backup_dir = tempfile::tempdir().unwrap();
        assert!(load(backup_dir.path(), "claude").unwrap().is_none());
        assert!(!exists(backup_dir.path(), "claude"));
    }

    #[test]
    fn clear_removes_both_backup_files() {
        let backup_dir = tempfile::tempdir().unwrap();
        let target = backup_dir.path().join("settings.json");
        std::fs::write(&target, b"original").unwrap();
        snapshot(backup_dir.path(), "claude", &target).unwrap();

        clear(backup_dir.path(), "claude").expect("clear succeeds");
        assert!(!exists(backup_dir.path(), "claude"));
        assert!(load(backup_dir.path(), "claude").unwrap().is_none());
    }

    #[test]
    fn clear_of_a_missing_backup_is_not_an_error() {
        let backup_dir = tempfile::tempdir().unwrap();
        clear(backup_dir.path(), "claude").expect("clearing a never-taken backup is a no-op");
    }
}
