//! A tiny atomic-write helper shared by [`crate::cache`] (the denylist
//! snapshot) and [`crate::hooks`] (shim config edits and their backups).
//!
//! Mirrors `penguin_daemon::state::PersistedState::save`'s pattern exactly:
//! a uniquely-named temp file is written in the *same* directory as the
//! destination (so the rename is same-filesystem and therefore atomic), then
//! renamed over the destination — a reader (an editor with the file open, a
//! concurrent `penguin` invocation) never observes a half-written file.

use std::fs;
use std::io::{self, ErrorKind, Write as _};
use std::path::{Path, PathBuf};

use rand::Rng as _;

/// Writes `data` to `path` atomically, creating `path`'s parent directory
/// first if it does not already exist.
///
/// Permissions (Unix): a brand-new `path` is created owner-read/write-only
/// (mode 0600, see [`NEW_FILE_MODE`]) rather than left at whatever the
/// umask would otherwise produce — every caller here writes either this
/// module's own private state (the denylist cache, a shim's backup) or a
/// hook command line merged into a user's editor/agent config, neither of
/// which should default to a group/world-readable mode.
///
/// If `path` already exists, its current mode is read *before* the write
/// and re-applied to the replacement file. This matters because
/// `rename(2)` (the atomic step below) fully replaces the destination
/// inode rather than updating it in place — without re-applying the old
/// mode, an operator's own tighter permissions on an existing config file
/// (e.g. a `~/.claude/settings.json` they had locked down to 0600) would
/// silently be reset to the temp file's umask-derived mode the moment this
/// function merges a hook entry into it.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("{} has no parent directory", path.display()),
        )
    })?;
    fs::create_dir_all(dir)?;

    let existing_mode = existing_file_mode(path)?;

    let (mut file, tmp_path) = create_temp_file(dir)?;
    if let Err(err) = apply_mode(&file, existing_mode) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    if let Err(err) = file.write_all(data).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    drop(file);

    fs::rename(&tmp_path, path)
}

/// The mode [`write_atomic`] applies to a file it is creating for the first
/// time — owner read/write only.
#[cfg(unix)]
const NEW_FILE_MODE: u32 = 0o600;

/// `path`'s current permission bits — masked to the low 12 bits (setuid/
/// setgid/sticky + `rwxrwxrwx`), never the file-type bits `st_mode` also
/// carries — or `None` if `path` does not exist yet. Always `None` on
/// non-Unix targets, where there is no bit pattern to preserve.
#[cfg(unix)]
fn existing_file_mode(path: &Path) -> io::Result<Option<u32>> {
    use std::os::unix::fs::PermissionsExt as _;
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata.permissions().mode() & 0o7777)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

#[cfg(not(unix))]
fn existing_file_mode(_path: &Path) -> io::Result<Option<u32>> {
    Ok(None)
}

/// Sets `file`'s mode to `existing_mode` when `path` already existed
/// (preserving it across the rename below), or to [`NEW_FILE_MODE`] for a
/// brand-new file. A no-op on non-Unix targets.
#[cfg(unix)]
fn apply_mode(file: &fs::File, existing_mode: Option<u32>) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = existing_mode.unwrap_or(NEW_FILE_MODE);
    file.set_permissions(fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn apply_mode(_file: &fs::File, _existing_mode: Option<u32>) -> io::Result<()> {
    Ok(())
}

/// Creates a uniquely-named `.waddleai-tmp-*` file in `dir`, retrying on a
/// name collision — mirrors `penguin_daemon::state::create_temp_file`.
fn create_temp_file(dir: &Path) -> io::Result<(fs::File, PathBuf)> {
    const MAX_ATTEMPTS: u32 = 100;

    let mut attempt = 0;
    while attempt < MAX_ATTEMPTS {
        let path = dir.join(format!(".waddleai-tmp-{}", random_suffix()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((file, path)),
            Err(err) if err.kind() == ErrorKind::AlreadyExists => attempt += 1,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        ErrorKind::AlreadyExists,
        "could not create a unique temp file after 100 attempts",
    ))
}

fn random_suffix() -> String {
    let value: u64 = rand::rng().random();
    format!("{value:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_creates_the_file_with_expected_contents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("f.txt");
        write_atomic(&path, b"hello").expect("write");
        assert_eq!(fs::read(&path).expect("read"), b"hello");
    }

    #[test]
    fn write_atomic_overwrites_an_existing_file_in_one_step() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("f.txt");
        fs::write(&path, b"old").expect("seed");
        write_atomic(&path, b"new").expect("write");
        assert_eq!(fs::read(&path).expect("read"), b"new");
    }

    #[test]
    fn write_atomic_leaves_no_temp_file_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("f.txt");
        write_atomic(&path, b"hello").expect("write");
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .expect("read_dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains("waddleai-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file left behind: {leftovers:?}");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_creates_a_new_file_mode_0600_regardless_of_umask() {
        use std::os::unix::fs::MetadataExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("f.txt");
        write_atomic(&path, b"hello").expect("write");
        let mode = fs::metadata(&path).expect("stat").mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "new file must not inherit the umask's default mode"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_preserves_an_existing_files_tighter_mode() {
        use std::os::unix::fs::MetadataExt as _;
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        fs::write(&path, b"old").expect("seed");
        // Tighter than write_atomic's own 0600 default, and tighter than
        // whatever the process umask would otherwise leave a fresh file at
        // — proves the replacement isn't just falling back to a fixed
        // constant, it genuinely reads the pre-existing mode back.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).expect("chmod");

        write_atomic(&path, b"new").expect("write");

        let mode = fs::metadata(&path).expect("stat").mode() & 0o777;
        assert_eq!(
            mode, 0o400,
            "rename must not silently reset an existing file's mode"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_preserves_an_existing_files_looser_mode_too() {
        // The mirror case of the tighter-mode test above: write_atomic must
        // not clamp an existing file down to its own 0600 default either —
        // "preserve whatever was there" cuts both ways, this function does
        // not get to decide the file should be more restrictive than its
        // owner already made it.
        use std::os::unix::fs::MetadataExt as _;
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        fs::write(&path, b"old").expect("seed");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod");

        write_atomic(&path, b"new").expect("write");

        let mode = fs::metadata(&path).expect("stat").mode() & 0o777;
        assert_eq!(
            mode, 0o644,
            "an existing file's own mode must be preserved as-is"
        );
    }
}
