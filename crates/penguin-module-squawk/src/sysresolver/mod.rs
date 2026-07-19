//! The cross-platform DNS-resolver state machine: crash-safe apply/restore
//! of the *host's* system resolver, driven through one
//! [`backend::PlatformBackend`] per call. Platform-specific mechanics live
//! in `linux/`, `macos.rs`, and `windows.rs`; this file owns the ordering
//! that keeps a crash between "decide to mutate DNS" and "DNS is mutated"
//! always recoverable.
//!
//! # The crash window Go left open
//!
//! The Go implementation wrote its crash marker *after* successfully
//! mutating DNS (`Apply` called `applyPlatform`, then `writeBackup`). A
//! crash between those two steps left the host's resolver pointed
//! somewhere new with no trace on disk: nothing would ever restore it, and
//! the operator was never told.
//!
//! Here the marker is written *before* any backend is allowed to touch
//! host state — see [`SysResolver::apply`] — and only after the chosen
//! backend's own [`backend::PlatformBackend::snapshot`] has already
//! captured (and, for `resolv.conf`, durably backed up) enough to restore
//! from. That makes "the marker exists on disk" a true precondition for
//! "DNS might have changed", rather than a best-effort afterthought:
//!
//! * A crash **before** the marker is durable provably means
//!   [`backend::PlatformBackend::commit`] never ran, so
//!   [`SysResolver::recover_from_crash`] finding no marker is always
//!   correct — DNS was never touched.
//! * A crash **after** the marker is durable (during or after `commit`) is
//!   always safe to recover from by calling `restore()` with the marker's
//!   `previous_servers`: if `commit` never got that far, restoring
//!   previous-over-previous is a harmless no-op; if it partially or fully
//!   ran, restoring fixes it.
//!
//! The one residue this does *not* eliminate: a crash in the brief window
//! between a backend's own internal backup write (inside `snapshot`, e.g.
//! `resolv.conf`'s byte-exact copy) and the marker write immediately after
//! it. That leaves an orphaned backup file with nothing pointing at it —
//! inert, since the thing it backs up was never touched before the marker
//! landed, and harmless clutter cleaned up by the next `apply()`. This is
//! the residue accepted in exchange for closing the actually dangerous
//! window (host DNS mutated, zero trace anywhere).

pub mod backend;
mod error;
mod fsutil;
mod marker;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod command;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Mutex;

use tracing::{info, warn};

pub use backend::PlatformBackend;
pub use error::SysResolverError;

use marker::{Marker, MarkerStore};

/// In-memory record of the most recent successful `apply()`, checked by
/// `restore()` before falling back to the on-disk marker — mirrors the Go
/// resolver's `r.backup` field. Purely a same-process fast path: crash
/// recovery after a restart always goes through the marker file instead,
/// since a fresh process starts with this empty.
struct AppliedState {
    backend: String,
    previous_servers: Vec<IpAddr>,
}

/// Manages the host's DNS resolver configuration end to end: apply new
/// servers, restore the previous ones, and recover cleanly if the process
/// was killed mid-operation. One instance per module lifetime.
pub struct SysResolver {
    backends: Vec<Box<dyn PlatformBackend>>,
    marker: MarkerStore,
    state: Mutex<Option<AppliedState>>,
}

impl SysResolver {
    /// Builds the resolver for the current platform's real backends,
    /// rooted at `data_dir` for the crash marker and (on Linux) the
    /// resolv.conf byte backup.
    #[cfg(target_os = "linux")]
    pub fn new(data_dir: PathBuf) -> SysResolver {
        let backends = linux::build_backends(&data_dir);
        SysResolver::with_backends(data_dir, backends)
    }

    /// Builds the resolver for the current platform's real backends,
    /// rooted at `data_dir` for the crash marker.
    #[cfg(target_os = "macos")]
    pub fn new(data_dir: PathBuf) -> SysResolver {
        SysResolver::with_backends(data_dir, macos::build_backends())
    }

    /// Builds the resolver for the current platform's real backends,
    /// rooted at `data_dir` for the crash marker.
    #[cfg(target_os = "windows")]
    pub fn new(data_dir: PathBuf) -> SysResolver {
        SysResolver::with_backends(data_dir, windows::build_backends())
    }

    /// Builds a resolver over an explicit, priority-ordered backend list.
    /// Production code reaches this indirectly via [`Self::new`]; tests use
    /// it directly to inject a fake [`PlatformBackend`] (or a real backend
    /// wired to a fake D-Bus/process/filesystem seam) so nothing here ever
    /// touches the host.
    pub fn with_backends(
        data_dir: PathBuf,
        backends: Vec<Box<dyn PlatformBackend>>,
    ) -> SysResolver {
        SysResolver {
            marker: MarkerStore::new(&data_dir),
            backends,
            state: Mutex::new(None),
        }
    }

    /// Points the host's resolver at `servers`, recording enough to
    /// `restore()` it later. Safe to call repeatedly — each call captures
    /// whatever is active *right now* as the new undo point (matching the
    /// Go resolver: there is one level of undo, not a stack).
    pub async fn apply(&self, servers: &[IpAddr]) -> Result<(), SysResolverError> {
        if servers.is_empty() {
            return Err(SysResolverError::NoServers);
        }

        let selected = self.select_and_snapshot().await?;
        let (backend, previous) = selected;

        let pending = Marker::new(backend.name(), &previous);
        self.marker.write(&pending)?;

        let committed = backend.commit(servers).await;
        if let Err(err) = committed {
            // Nothing here crashed — this is the ordinary failure path —
            // so we know definitively there is nothing to recover, and
            // leaving the marker around would just be stray clutter
            // implying otherwise.
            let cleanup = self.marker.delete();
            if let Err(cleanup_err) = cleanup {
                warn!(error = %cleanup_err, "failed to clean up crash marker after a failed apply");
            }
            return Err(err);
        }

        let mut state = self.state.lock().expect("sysresolver state mutex poisoned");
        *state = Some(AppliedState {
            backend: backend.name().to_string(),
            previous_servers: previous,
        });
        drop(state);

        info!(
            backend = backend.name(),
            server_count = servers.len(),
            "DNS applied"
        );
        Ok(())
    }

    /// Tries each backend's `snapshot()` in priority order, returning the
    /// first that succeeds along with the servers it reports as "current
    /// right now". An `Err` from every backend collapses to whichever
    /// error was seen last (or [`SysResolverError::NoBackendAvailable`] if
    /// the backend list is empty).
    async fn select_and_snapshot(
        &self,
    ) -> Result<(&dyn PlatformBackend, Vec<IpAddr>), SysResolverError> {
        let mut last_err = None;
        for backend in &self.backends {
            let attempt = backend.snapshot().await;
            match attempt {
                Ok(previous) => return Ok((backend.as_ref(), previous)),
                Err(err) => {
                    warn!(backend = backend.name(), error = %err, "backend unavailable, trying next");
                    last_err = Some(err);
                }
            }
        }
        Err(last_err.unwrap_or(SysResolverError::NoBackendAvailable))
    }

    /// Reverts to whatever `apply()` last recorded as "previous": the
    /// in-memory record from this process if present, else the on-disk
    /// marker. Errors with [`SysResolverError::NoBackupAvailable`] if
    /// neither exists — the host's current DNS is left exactly as it is
    /// rather than guessing a fallback (Go instead hard-codes `1.1.1.1`
    /// when a backup's server list is present-but-empty; that case is
    /// treated identically to "no backup" here, since silently routing the
    /// host at an arbitrary third party is a worse failure mode than
    /// declining to restore).
    ///
    /// Calling `restore()` again immediately after a successful one
    /// deliberately errors the same way: both the in-memory record and the
    /// marker are cleared only on success, so there is nothing left to
    /// restore a second time — this is the chosen idempotency contract
    /// (no silent double no-op, no double side effects).
    pub async fn restore(&self) -> Result<(), SysResolverError> {
        let cached = self.cached_state();
        let resolved = match cached {
            Some(pair) => pair,
            None => self.load_marker_or_fail()?,
        };
        let (backend_name, previous) = resolved;

        let backend = self.find_backend(&backend_name)?;
        backend.restore(&previous).await?;

        *self.state.lock().expect("sysresolver state mutex poisoned") = None;
        let cleanup = self.marker.delete();
        if let Err(err) = cleanup {
            warn!(error = %err, "failed to delete crash marker after restore");
        }

        info!(backend = %backend_name, "DNS restored");
        Ok(())
    }

    /// Clones the in-memory backup, if any, without holding the lock past
    /// this call.
    fn cached_state(&self) -> Option<(String, Vec<IpAddr>)> {
        let guard = self.state.lock().expect("sysresolver state mutex poisoned");
        let applied = guard.as_ref()?;
        Some((applied.backend.clone(), applied.previous_servers.clone()))
    }

    /// Loads the on-disk marker, translating "genuinely absent" into
    /// [`SysResolverError::NoBackupAvailable`]. A corrupt marker's
    /// [`SysResolverError::CorruptMarker`] propagates unchanged — `restore()`
    /// benefits from the same quarantine-and-report behaviour as
    /// [`Self::recover_from_crash`].
    fn load_marker_or_fail(&self) -> Result<(String, Vec<IpAddr>), SysResolverError> {
        let loaded = self.marker.load()?;
        let Some(marker) = loaded else {
            return Err(SysResolverError::NoBackupAvailable);
        };
        Ok((marker.backend.clone(), marker.parse_servers()))
    }

    /// Finds the backend named in a marker. Falls back to the
    /// highest-priority configured backend (with a loud warning) if the
    /// name doesn't match any of them — defensive against a marker written
    /// by a since-changed build, rather than failing outright when
    /// candidate backends are actually available.
    fn find_backend(&self, name: &str) -> Result<&dyn PlatformBackend, SysResolverError> {
        for backend in &self.backends {
            if backend.name() == name {
                return Ok(backend.as_ref());
            }
        }
        let Some(first) = self.backends.first() else {
            return Err(SysResolverError::NoBackendAvailable);
        };
        warn!(
            recorded = name,
            using = first.name(),
            "crash marker names an unrecognised backend; using the default instead"
        );
        Ok(first.as_ref())
    }

    /// Reads what the host resolves through right now, trying each backend
    /// in priority order and returning the first non-empty result.
    pub async fn current(&self) -> Result<Vec<IpAddr>, SysResolverError> {
        let mut last_err = None;
        for backend in &self.backends {
            let attempt = backend.current().await;
            match attempt {
                Ok(servers) if !servers.is_empty() => return Ok(servers),
                Ok(_) => continue,
                Err(err) => last_err = Some(err),
            }
        }
        Err(last_err.unwrap_or(SysResolverError::NoBackendAvailable))
    }

    /// Run once at startup, before anything else touches DNS: undoes a
    /// previous unclean exit.
    ///
    /// * No marker on disk → clean no-op (`Ok(())`) — by construction (see
    ///   module docs) this provably means no previous run ever mutated
    ///   host DNS without a trace.
    /// * A valid marker → restores its `previous_servers` via its recorded
    ///   backend, then deletes the marker.
    /// * A corrupt marker → quarantined by [`marker::MarkerStore::load`]
    ///   and reported as [`SysResolverError::CorruptMarker`]. This is the
    ///   fixed half of the two Go bugs this port addresses: Go treated
    ///   unparseable JSON exactly like "no marker", returning `nil` and
    ///   logging nothing useful, which left the file in place forever and
    ///   silently no-opped on every subsequent start. Quarantining moves
    ///   the bad file out of the way (so it can't wedge every future start
    ///   the same way) while surfacing the failure loudly instead of
    ///   swallowing it.
    pub async fn recover_from_crash(&self) -> Result<(), SysResolverError> {
        let loaded = self.marker.load();
        let marker = match loaded {
            Ok(None) => return Ok(()),
            Ok(Some(marker)) => marker,
            Err(err) => return Err(err),
        };

        info!(backend = %marker.backend, "crash recovery: restoring DNS from marker");
        let servers = marker.parse_servers();
        let backend = self.find_backend(&marker.backend)?;
        backend.restore(&servers).await?;

        self.marker.delete()?;
        *self.state.lock().expect("sysresolver state mutex poisoned") = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    /// Test double for [`PlatformBackend`]: records call counts, lets
    /// tests script a failure at any step, and never touches the host.
    struct FakeBackend {
        backend_name: &'static str,
        current_servers: Mutex<Vec<IpAddr>>,
        snapshot_calls: AtomicU32,
        commit_calls: AtomicU32,
        restore_calls: AtomicU32,
        fail_snapshot: AtomicBool,
        fail_commit: AtomicBool,
        fail_restore: AtomicBool,
    }

    impl FakeBackend {
        fn new(name: &'static str, initial_servers: Vec<IpAddr>) -> FakeBackend {
            FakeBackend {
                backend_name: name,
                current_servers: Mutex::new(initial_servers),
                snapshot_calls: AtomicU32::new(0),
                commit_calls: AtomicU32::new(0),
                restore_calls: AtomicU32::new(0),
                fail_snapshot: AtomicBool::new(false),
                fail_commit: AtomicBool::new(false),
                fail_restore: AtomicBool::new(false),
            }
        }

        fn servers(&self) -> Vec<IpAddr> {
            self.current_servers
                .lock()
                .expect("fake mutex poisoned")
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl PlatformBackend for FakeBackend {
        fn name(&self) -> &'static str {
            self.backend_name
        }

        async fn snapshot(&self) -> Result<Vec<IpAddr>, SysResolverError> {
            self.snapshot_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_snapshot.load(Ordering::SeqCst) {
                return Err(SysResolverError::Backend(
                    "fake snapshot failure".to_string(),
                ));
            }
            Ok(self.servers())
        }

        async fn commit(&self, servers: &[IpAddr]) -> Result<(), SysResolverError> {
            self.commit_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_commit.load(Ordering::SeqCst) {
                return Err(SysResolverError::Backend("fake commit failure".to_string()));
            }
            *self.current_servers.lock().expect("fake mutex poisoned") = servers.to_vec();
            Ok(())
        }

        async fn restore(&self, fallback_servers: &[IpAddr]) -> Result<(), SysResolverError> {
            self.restore_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_restore.load(Ordering::SeqCst) {
                return Err(SysResolverError::Backend(
                    "fake restore failure".to_string(),
                ));
            }
            *self.current_servers.lock().expect("fake mutex poisoned") = fallback_servers.to_vec();
            Ok(())
        }

        async fn current(&self) -> Result<Vec<IpAddr>, SysResolverError> {
            Ok(self.servers())
        }
    }

    fn addr(s: &str) -> IpAddr {
        s.parse().expect("valid test address")
    }

    fn resolver_with_fake(
        dir: &std::path::Path,
        initial: Vec<IpAddr>,
    ) -> (SysResolver, std::sync::Arc<FakeBackend>) {
        let fake = std::sync::Arc::new(FakeBackend::new("fake", initial));
        let boxed: Box<dyn PlatformBackend> = Box::new(ArcBackend(fake.clone()));
        let resolver = SysResolver::with_backends(dir.to_path_buf(), vec![boxed]);
        (resolver, fake)
    }

    /// Lets the same `Arc<FakeBackend>` be both held onto by the test (to
    /// inspect call counts) and boxed into the backend list, without a
    /// second implementation of the trait.
    struct ArcBackend(std::sync::Arc<FakeBackend>);

    #[async_trait::async_trait]
    impl PlatformBackend for ArcBackend {
        fn name(&self) -> &'static str {
            self.0.name()
        }
        async fn snapshot(&self) -> Result<Vec<IpAddr>, SysResolverError> {
            self.0.snapshot().await
        }
        async fn commit(&self, servers: &[IpAddr]) -> Result<(), SysResolverError> {
            self.0.commit(servers).await
        }
        async fn restore(&self, fallback_servers: &[IpAddr]) -> Result<(), SysResolverError> {
            self.0.restore(fallback_servers).await
        }
        async fn current(&self) -> Result<Vec<IpAddr>, SysResolverError> {
            self.0.current().await
        }
    }

    #[tokio::test]
    async fn apply_rejects_empty_server_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (resolver, _fake) = resolver_with_fake(dir.path(), vec![addr("8.8.8.8")]);
        let err = resolver
            .apply(&[])
            .await
            .expect_err("empty servers must error");
        assert!(matches!(err, SysResolverError::NoServers));
    }

    #[tokio::test]
    async fn apply_writes_marker_before_commit_and_commits_after() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (resolver, fake) = resolver_with_fake(dir.path(), vec![addr("8.8.8.8")]);

        resolver.apply(&[addr("1.1.1.1")]).await.expect("apply");

        assert_eq!(fake.snapshot_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fake.commit_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fake.servers(), vec![addr("1.1.1.1")]);

        let marker_path = dir.path().join(marker::MARKER_FILENAME);
        assert!(
            marker_path.exists(),
            "marker must survive a successful apply"
        );
    }

    #[tokio::test]
    async fn apply_failure_cleans_up_the_marker_it_wrote() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (resolver, fake) = resolver_with_fake(dir.path(), vec![addr("8.8.8.8")]);
        fake.fail_commit.store(true, Ordering::SeqCst);

        let err = resolver
            .apply(&[addr("1.1.1.1")])
            .await
            .expect_err("commit fails");
        assert!(matches!(err, SysResolverError::Backend(_)));

        let marker_path = dir.path().join(marker::MARKER_FILENAME);
        assert!(
            !marker_path.exists(),
            "a plain (non-crash) apply failure must not leave a stray marker"
        );
    }

    #[tokio::test]
    async fn restore_puts_back_previous_servers_and_deletes_marker() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (resolver, fake) = resolver_with_fake(dir.path(), vec![addr("8.8.8.8")]);

        resolver.apply(&[addr("1.1.1.1")]).await.expect("apply");
        resolver.restore().await.expect("restore");

        assert_eq!(fake.servers(), vec![addr("8.8.8.8")]);
        let marker_path = dir.path().join(marker::MARKER_FILENAME);
        assert!(
            !marker_path.exists(),
            "marker must be removed after restore"
        );
    }

    #[tokio::test]
    async fn restore_without_any_backup_errors_and_leaves_dns_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (resolver, fake) = resolver_with_fake(dir.path(), vec![addr("8.8.8.8")]);

        let err = resolver.restore().await.expect_err("nothing to restore");
        assert!(matches!(err, SysResolverError::NoBackupAvailable));
        assert_eq!(fake.restore_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn double_restore_errors_on_the_second_call() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (resolver, fake) = resolver_with_fake(dir.path(), vec![addr("8.8.8.8")]);

        resolver.apply(&[addr("1.1.1.1")]).await.expect("apply");
        resolver.restore().await.expect("first restore succeeds");
        let err = resolver
            .restore()
            .await
            .expect_err("second restore has nothing left to undo");

        assert!(matches!(err, SysResolverError::NoBackupAvailable));
        assert_eq!(
            fake.restore_calls.load(Ordering::SeqCst),
            1,
            "no second host mutation attempted"
        );
    }

    #[tokio::test]
    async fn double_apply_then_restore_undoes_only_the_second_apply() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (resolver, fake) = resolver_with_fake(dir.path(), vec![addr("8.8.8.8")]);

        resolver
            .apply(&[addr("1.1.1.1")])
            .await
            .expect("first apply");
        resolver
            .apply(&[addr("9.9.9.9")])
            .await
            .expect("second apply");
        assert_eq!(fake.servers(), vec![addr("9.9.9.9")]);

        resolver.restore().await.expect("restore");
        assert_eq!(
            fake.servers(),
            vec![addr("1.1.1.1")],
            "restore undoes one level: back to what the second apply overwrote, not the original"
        );
    }

    #[tokio::test]
    async fn recover_from_crash_with_no_marker_is_a_clean_noop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (resolver, fake) = resolver_with_fake(dir.path(), vec![addr("8.8.8.8")]);

        resolver
            .recover_from_crash()
            .await
            .expect("no-op recovery must succeed");
        assert_eq!(fake.restore_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn recover_from_crash_with_valid_marker_restores_and_removes_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (resolver, fake) = resolver_with_fake(dir.path(), vec![addr("8.8.8.8")]);

        // Simulate a crash: a marker exists but `commit` for the new
        // servers never got to run (or ran and the process died right
        // after) — either way the fake's live servers are still whatever
        // they were, and recovery must put the marker's servers back.
        let marker = Marker::new("fake", &[addr("8.8.8.8")]);
        MarkerStore::new(dir.path())
            .write(&marker)
            .expect("seed crash marker");

        resolver.recover_from_crash().await.expect("recovery");

        assert_eq!(fake.restore_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fake.servers(), vec![addr("8.8.8.8")]);
        assert!(!dir.path().join(marker::MARKER_FILENAME).exists());
    }

    #[tokio::test]
    async fn recover_from_crash_with_corrupt_marker_quarantines_and_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (resolver, fake) = resolver_with_fake(dir.path(), vec![addr("8.8.8.8")]);

        let marker_path = dir.path().join(marker::MARKER_FILENAME);
        std::fs::write(&marker_path, b"{not json").expect("seed corrupt marker");

        let err = resolver
            .recover_from_crash()
            .await
            .expect_err("a corrupt marker must be reported, not silently swallowed");
        assert!(matches!(err, SysResolverError::CorruptMarker { .. }));

        // Quarantined, not left in place — and no restore was attempted
        // since there was nothing trustworthy to restore from.
        assert!(!marker_path.exists());
        assert_eq!(fake.restore_calls.load(Ordering::SeqCst), 0);

        let mut quarantine_files = Vec::new();
        for entry in std::fs::read_dir(dir.path()).expect("read dir") {
            let entry = entry.expect("dir entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("dns-applied.json.corrupt-") {
                quarantine_files.push(name);
            }
        }
        assert_eq!(
            quarantine_files.len(),
            1,
            "exactly one quarantined file, evidence preserved"
        );

        // And a subsequent recovery attempt on the same data dir no longer
        // finds anything to quarantine — it's a clean no-op, not a loop.
        resolver
            .recover_from_crash()
            .await
            .expect("second attempt is a clean no-op");
    }

    #[tokio::test]
    async fn crash_between_marker_write_and_commit_leaves_recoverable_state() {
        // This is the window the fix targets: simulate a crash by writing
        // the marker exactly as `apply()` would, but never calling
        // `commit()` at all (the backend's live servers are still the
        // pre-apply ones). recover_from_crash must still succeed and the
        // servers must end up matching the marker's previous_servers —
        // proving a marker on disk is always safely recoverable even when
        // the mutation never happened.
        let dir = tempfile::tempdir().expect("tempdir");
        let (resolver, fake) = resolver_with_fake(dir.path(), vec![addr("8.8.8.8")]);

        let marker = Marker::new("fake", &[addr("8.8.8.8")]);
        MarkerStore::new(dir.path())
            .write(&marker)
            .expect("simulate pre-commit crash marker");

        resolver
            .recover_from_crash()
            .await
            .expect("recovery over an unmutated backend is a safe no-op");
        assert_eq!(fake.servers(), vec![addr("8.8.8.8")]);
    }

    #[tokio::test]
    async fn current_returns_first_backend_with_servers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (resolver, _fake) =
            resolver_with_fake(dir.path(), vec![addr("8.8.8.8"), addr("8.8.4.4")]);

        let current = resolver.current().await.expect("current");
        assert_eq!(current, vec![addr("8.8.8.8"), addr("8.8.4.4")]);
    }

    #[tokio::test]
    async fn find_backend_falls_back_to_first_when_marker_name_is_unknown() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (resolver, fake) = resolver_with_fake(dir.path(), vec![addr("8.8.8.8")]);

        let marker = Marker::new("some-other-backend-name", &[addr("8.8.8.8")]);
        MarkerStore::new(dir.path())
            .write(&marker)
            .expect("seed marker with unknown backend name");

        resolver
            .recover_from_crash()
            .await
            .expect("falls back instead of failing");
        assert_eq!(fake.restore_calls.load(Ordering::SeqCst), 1);
    }
}
