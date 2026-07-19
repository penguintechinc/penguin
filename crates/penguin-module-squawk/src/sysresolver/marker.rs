//! The crash-recovery marker: a small JSON file recording what to restore
//! and with which backend, kept at `<data_dir>/dns-applied.json`.
//!
//! Its lifetime is deliberately the *entire* time DNS might differ from
//! what it was before `apply()` — written before any backend is allowed to
//! mutate host state, deleted only after a successful `restore()`. See the
//! `sysresolver` module docs for why that ordering (not Go's
//! write-after-apply) is what actually closes the crash window.

use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::sysresolver::error::SysResolverError;

/// Filename the marker lives under, inside the module's data directory.
pub const MARKER_FILENAME: &str = "dns-applied.json";

/// The crash marker's on-disk shape.
///
/// Go additionally recorded a raw `previous_state` blob here (the full
/// resolv.conf/`scutil`/`netsh` output at apply time, populated by
/// `captureState()`) — but nothing in the Go source ever reads it back, in
/// `Restore` or in `RecoverFromCrash`. It is dropped here rather than
/// carried forward as dead data. Byte-exact fidelity for the one backend
/// where it actually matters (`resolv.conf`) comes instead from that
/// backend's own `resolv.conf.backup` file, which *is* read back (see
/// [`crate::sysresolver::linux::resolv_conf`]); the other backends don't
/// restore via literal previous-state replay at all — systemd-resolved
/// reverts through `RevertLink`, macOS/Windows reset to DHCP — so there
/// was nothing meaningful for the field to hold for them either.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    /// DNS servers active immediately before `apply()` changed them,
    /// stringified (`IpAddr::to_string`). Best-effort: empty if they could
    /// not be read at apply time.
    pub previous_servers: Vec<String>,
    /// Which [`super::backend::PlatformBackend::name`] applied the change
    /// and must be the one to reverse it.
    pub backend: String,
    /// Unix timestamp (seconds) the marker was written. Informational
    /// only — like Go's `AppliedAt`, never parsed back.
    pub applied_at: u64,
}

impl Marker {
    /// Builds a marker for a change that is about to be applied.
    pub fn new(backend: &str, previous_servers: &[IpAddr]) -> Marker {
        let applied_at = unix_now();
        let mut previous = Vec::with_capacity(previous_servers.len());
        for addr in previous_servers {
            previous.push(addr.to_string());
        }
        Marker {
            previous_servers: previous,
            backend: backend.to_string(),
            applied_at,
        }
    }

    /// Parses `previous_servers` back into addresses, skipping (and
    /// logging) any entry that no longer parses rather than failing the
    /// whole restore over one bad string — mirrors the Go resolver's
    /// per-address `continue` in both `Restore` and `RecoverFromCrash`.
    pub fn parse_servers(&self) -> Vec<IpAddr> {
        let mut servers = Vec::with_capacity(self.previous_servers.len());
        for raw in &self.previous_servers {
            let parsed: Result<IpAddr, _> = raw.parse();
            let Ok(addr) = parsed else {
                tracing::warn!(value = %raw, "skipping unparseable address in crash marker");
                continue;
            };
            servers.push(addr);
        }
        servers
    }
}

fn unix_now() -> u64 {
    let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH);
    let Ok(duration) = since_epoch else {
        return 0;
    };
    duration.as_secs()
}

/// Reads, writes, deletes, and quarantines the marker file at
/// `<data_dir>/dns-applied.json`.
pub struct MarkerStore {
    path: PathBuf,
}

impl MarkerStore {
    /// Points the store at `<data_dir>/dns-applied.json`.
    pub fn new(data_dir: &Path) -> MarkerStore {
        MarkerStore {
            path: data_dir.join(MARKER_FILENAME),
        }
    }

    /// Durably writes `marker` at mode 0600, creating the data directory
    /// first (mode 0700 on Unix) if it doesn't exist yet. Must complete
    /// before any backend mutates host DNS — see module docs.
    pub fn write(&self, marker: &Marker) -> Result<(), SysResolverError> {
        let dir = self.path.parent().unwrap_or_else(|| Path::new("."));
        crate::sysresolver::fsutil::create_dir_all_owner_only(dir)?;

        let data = serde_json::to_vec_pretty(marker).map_err(|source| {
            SysResolverError::Backend(format!("failed to serialize crash marker: {source}"))
        })?;
        crate::sysresolver::fsutil::write_owner_only(&self.path, &data)
    }

    /// Loads the marker, quarantining it first if it exists but fails to
    /// parse.
    ///
    /// `Ok(None)` means genuinely absent — a clean no-op for callers.
    /// `Err(CorruptMarker)` means it existed, was unreadable as JSON, and
    /// has already been renamed out of the way; the caller should surface
    /// that loudly rather than treat it as "nothing to do" (the bug this
    /// type exists to prevent: a corrupt marker used to be indistinguishable
    /// from no marker at all, silently wedging recovery forever).
    pub fn load(&self) -> Result<Option<Marker>, SysResolverError> {
        let read = fs::read(&self.path);
        let data = match read {
            Ok(data) => data,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(SysResolverError::Io {
                    context: format!("read crash marker {}", self.path.display()),
                    source,
                });
            }
        };

        let parsed: Result<Marker, serde_json::Error> = serde_json::from_slice(&data);
        let marker = match parsed {
            Ok(marker) => marker,
            Err(parse_err) => return Err(self.quarantine_and_report(parse_err)),
        };
        Ok(Some(marker))
    }

    /// Renames the marker to `dns-applied.json.corrupt-<unix_ts>` in the
    /// same directory (preserving the bad content as evidence instead of
    /// deleting it), logs loudly, and returns the resulting error.
    fn quarantine_and_report(&self, parse_err: serde_json::Error) -> SysResolverError {
        let quarantined = self.quarantine();
        tracing::error!(
            original = %self.path.display(),
            quarantined = %quarantined.display(),
            error = %parse_err,
            "crash marker is corrupt; quarantined so recovery is retried instead of silently skipped forever"
        );
        SysResolverError::CorruptMarker {
            original: self.path.display().to_string(),
            quarantined: quarantined.display().to_string(),
            source: parse_err,
        }
    }

    /// Performs the actual rename. Best-effort: if the rename itself fails
    /// (e.g. a read-only data directory), the corrupt file is left where it
    /// was — still reported loudly by the caller — rather than panicking.
    fn quarantine(&self) -> PathBuf {
        let ts = unix_now();
        let mut quarantined = self.path.clone();
        quarantined.set_file_name(format!("{MARKER_FILENAME}.corrupt-{ts}"));
        let renamed = fs::rename(&self.path, &quarantined);
        if let Err(err) = renamed {
            tracing::error!(error = %err, "failed to quarantine corrupt crash marker; leaving it in place");
            return self.path.clone();
        }
        quarantined
    }

    /// Deletes the marker; a missing file is not an error (mirrors Go's
    /// `deleteBackup`).
    pub fn delete(&self) -> Result<(), SysResolverError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(SysResolverError::Io {
                context: format!("delete crash marker {}", self.path.display()),
                source,
            }),
        }
    }

    /// The marker's own path — exposed for tests that assert on file
    /// presence/permissions directly.
    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> IpAddr {
        s.parse().expect("valid test address")
    }

    #[test]
    fn write_then_load_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = MarkerStore::new(dir.path());
        let marker = Marker::new("resolv.conf", &[addr("8.8.8.8"), addr("8.8.4.4")]);

        store.write(&marker).expect("write");
        let loaded = store.load().expect("load").expect("marker present");

        assert_eq!(loaded, marker);
    }

    #[test]
    fn load_absent_marker_is_ok_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = MarkerStore::new(dir.path());
        let loaded = store.load().expect("load should not error when absent");
        assert!(loaded.is_none());
    }

    #[test]
    fn load_corrupt_marker_quarantines_and_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = MarkerStore::new(dir.path());
        fs::write(store.path(), b"not valid json").expect("seed corrupt marker");

        let err = store.load().expect_err("corrupt marker must error");
        assert!(matches!(err, SysResolverError::CorruptMarker { .. }));

        // The original path is gone...
        assert!(!store.path().exists());
        // ...and exactly one quarantine file with the expected prefix exists.
        let mut quarantine_files = Vec::new();
        for entry in fs::read_dir(dir.path()).expect("read dir") {
            let entry = entry.expect("dir entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("dns-applied.json.corrupt-") {
                quarantine_files.push(name);
            }
        }
        assert_eq!(quarantine_files.len(), 1);
    }

    #[test]
    fn load_after_quarantine_is_absent_again() {
        // A second `load()` after quarantine must not loop forever finding
        // the same corrupt file — the quarantined copy has a different
        // name, so the store sees a clean "absent" marker going forward.
        let dir = tempfile::tempdir().expect("tempdir");
        let store = MarkerStore::new(dir.path());
        fs::write(store.path(), b"not valid json").expect("seed corrupt marker");

        let _ = store.load().expect_err("first load quarantines");
        let second = store.load().expect("second load must not error");
        assert!(second.is_none());
    }

    #[test]
    fn delete_missing_marker_is_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = MarkerStore::new(dir.path());
        store
            .delete()
            .expect("deleting an absent marker is a no-op");
    }

    #[test]
    fn write_creates_missing_data_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("nested").join("data");
        let store = MarkerStore::new(&nested);
        let marker = Marker::new("resolv.conf", &[]);
        store.write(&marker).expect("write should create parents");
        assert!(nested.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn marker_file_has_mode_0600() {
        use std::os::unix::fs::MetadataExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let store = MarkerStore::new(dir.path());
        store
            .write(&Marker::new("resolv.conf", &[]))
            .expect("write");

        let mode = fs::metadata(store.path()).expect("stat").mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn parse_servers_skips_invalid_entries() {
        let marker = Marker {
            previous_servers: vec!["not-an-ip".to_string(), "1.1.1.1".to_string()],
            backend: "resolv.conf".to_string(),
            applied_at: 0,
        };
        let servers = marker.parse_servers();
        assert_eq!(servers, vec![addr("1.1.1.1")]);
    }

    #[test]
    fn new_stringifies_addresses() {
        let marker = Marker::new("resolv.conf", &[addr("::1")]);
        assert_eq!(marker.previous_servers, vec!["::1".to_string()]);
        assert_eq!(marker.backend, "resolv.conf");
    }
}
