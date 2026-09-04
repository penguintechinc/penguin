//! On-disk cache persistence.
//!
//! The whole point of caching license state to disk is graceful
//! degradation across a daemon restart: if the license server is
//! unreachable when the daemon comes back up, it should still enforce the
//! entitlements it last confirmed rather than falling back to "everything
//! off". Every function here is deliberately infallible-in-effect — a
//! missing, corrupt, or unwritable cache is a condition the caller logs and
//! moves on from, never a reason to stop the daemon.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The persisted shape of the cache file: current tier, the flag map, and
/// when it was fetched. This is a private on-disk format read only by this
/// crate, so field names don't need to track any external wire contract.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct CacheFile {
    #[serde(default)]
    pub(crate) tier: String,
    #[serde(default)]
    pub(crate) features: HashMap<String, bool>,
    /// Unix seconds the cache was fetched. Plain integer rather than a
    /// formatted timestamp so staleness is observable without pulling in a
    /// date-time crate just to store one number.
    #[serde(default)]
    pub(crate) fetched_at: i64,
}

/// Returns the on-disk cache file path inside `cache_dir`.
fn cache_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("license-cache.json")
}

/// Converts a [`SystemTime`] to Unix seconds, clamping to `0` instead of
/// panicking if the clock is somehow before the epoch — this function is
/// part of the no-panic surface, so it has no failure mode.
pub(crate) fn unix_seconds(at: SystemTime) -> i64 {
    let Ok(since_epoch) = at.duration_since(UNIX_EPOCH) else {
        return 0;
    };
    since_epoch.as_secs() as i64
}

/// The inverse of [`unix_seconds`]: rebuilds a [`SystemTime`] from a stored
/// value, treating a negative or corrupt value as the epoch rather than
/// panicking on the cast.
pub(crate) fn system_time(unix_secs: i64) -> SystemTime {
    if unix_secs < 0 {
        return UNIX_EPOCH;
    }
    UNIX_EPOCH + Duration::from_secs(unix_secs as u64)
}

/// Writes `data` to `cache_dir`'s cache file atomically: a temp file in the
/// same directory, permissions locked to owner-only (0600) *before* any
/// bytes are written, then an atomic rename over the real path. Writing
/// through a same-directory temp file means a concurrent reader never sees
/// a half-written cache, and setting the mode before writing means the
/// entitlement data is never briefly world-readable.
pub(crate) fn persist(cache_dir: &Path, data: &CacheFile) -> std::io::Result<()> {
    fs::create_dir_all(cache_dir)?;

    let raw = serde_json::to_vec_pretty(data)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;

    let tmp_path = unique_tmp_path(cache_dir);
    let mut tmp = File::create(&tmp_path)?;
    if let Err(err) = lock_down_permissions(&tmp) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    if let Err(err) = tmp.write_all(&raw) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    if let Err(err) = tmp.sync_all() {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    drop(tmp);

    fs::rename(&tmp_path, cache_path(cache_dir))
}

/// Sets the temp file's permissions to owner-read/write only (0600) on
/// Unix. There is no equivalent single-bit "owner only" ACL concept on
/// Windows, so this is a deliberate no-op there — the daemon's non-Unix
/// build already restricts the containing directory via its own ACLs.
#[cfg(unix)]
fn lock_down_permissions(file: &File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn lock_down_permissions(_file: &File) -> std::io::Result<()> {
    Ok(())
}

/// Builds a temp-file path that's unique for the lifetime of this process,
/// so concurrent [`persist`] calls (the license client allows concurrent
/// `refresh` calls) never collide on the same temp name. Not `O_EXCL`-safe
/// against a *different* process racing on the same counter value, but two
/// independent processes sharing one cache directory is not a supported
/// configuration to begin with.
fn unique_tmp_path(cache_dir: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    cache_dir.join(format!(".license-cache-{pid}-{sequence}.tmp"))
}

/// Reads and parses the cache file. Any failure — missing file, permission
/// error, corrupt or truncated JSON — is reported as `None` ("no cache"),
/// never as an error: a bad cache on disk must not stop the daemon from
/// starting, only make it start cold.
pub(crate) fn load(cache_dir: &Path) -> Option<CacheFile> {
    let raw = fs::read(cache_path(cache_dir)).ok()?;
    serde_json::from_slice(&raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persist_then_load_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut features = HashMap::new();
        features.insert("penguin.squawk".to_string(), true);
        features.insert("penguin.off".to_string(), false);
        let data = CacheFile {
            tier: "enterprise".to_string(),
            features,
            fetched_at: 1_700_000_000,
        };

        persist(dir.path(), &data).expect("persist");
        let loaded = load(dir.path()).expect("load");

        assert_eq!(loaded.tier, "enterprise");
        assert_eq!(loaded.features.get("penguin.squawk"), Some(&true));
        assert_eq!(loaded.features.get("penguin.off"), Some(&false));
        assert_eq!(loaded.fetched_at, 1_700_000_000);
    }

    #[test]
    fn persist_writes_owner_only_permissions() {
        let dir = tempfile::tempdir().expect("tempdir");
        persist(dir.path(), &CacheFile::default()).expect("persist");

        let meta = fs::metadata(cache_path(dir.path())).expect("metadata");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn load_missing_file_returns_none_not_a_panic() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(load(dir.path()).is_none());
    }

    #[test]
    fn load_corrupt_file_returns_none_not_a_panic() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(cache_path(dir.path()), b"{not valid json").expect("write corrupt cache");
        assert!(load(dir.path()).is_none());
    }

    #[test]
    fn load_truncated_file_returns_none_not_a_panic() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A syntactically valid JSON prefix cut off mid-object.
        fs::write(cache_path(dir.path()), b"{\"tier\": \"enterprise\", \"fea")
            .expect("write truncated cache");
        assert!(load(dir.path()).is_none());
    }

    #[test]
    fn unix_seconds_round_trips_through_system_time() {
        let now = system_time(1_700_000_000);
        assert_eq!(unix_seconds(now), 1_700_000_000);
    }

    #[test]
    fn unix_seconds_before_epoch_clamps_to_zero() {
        let before_epoch = UNIX_EPOCH - Duration::from_secs(10);
        assert_eq!(unix_seconds(before_epoch), 0);
    }

    #[test]
    fn concurrent_persist_calls_do_not_collide() {
        use std::thread;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_path_buf();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                thread::spawn(move || {
                    let data = CacheFile {
                        tier: format!("tier-{i}"),
                        features: HashMap::new(),
                        fetched_at: i,
                    };
                    persist(&path, &data).expect("persist");
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("thread panicked");
        }

        // Whichever write landed last, the cache file must be valid and
        // free of any leftover temp files.
        assert!(load(&path).is_some());
        let leftover_tmp = fs::read_dir(&path)
            .expect("read_dir")
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains(".tmp"));
        assert!(!leftover_tmp, "a temp file was left behind");
    }
}
