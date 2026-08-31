//! [`scan_heal_report`]: one full check→heal→report cycle over a signed
//! [`IntegrityManifest`] — the loop body `penguind`'s armed daemon spawns on
//! an interval (see that binary's `daemon_main.rs`).

use std::path::Path;

use crate::console::ConsoleSink;
use crate::event::TamperEvent;
use crate::integrity::{check, heal};
use crate::manifest::ManifestSource;

/// Loads a manifest from `source`, verifies it, and — only if verification
/// succeeds — checks `root` against it, heals any tampered/missing file from
/// `protected_dir`, and reports one [`TamperEvent`] per finding to `sink`.
/// Returns every event produced (empty if nothing was tampered, or if the
/// cycle safely no-opped).
///
/// # Trust boundary
///
/// A manifest that fails to load, or fails
/// [`IntegrityManifest::verify_signature`](crate::IntegrityManifest::verify_signature)
/// against `pubkey`, is never acted on: this function logs a warning and
/// returns an empty `Vec` immediately. Falling back to a last-known-good
/// manifest on failure is a real daemon-loop concern (tracked for the
/// caller, not here) — this function's job is only to never heal, restore,
/// or report anything against unverified data. Never panics.
///
/// # Timestamp
///
/// `ts_unix` is supplied by the caller rather than read from
/// [`std::time::SystemTime::now`] here, so this function stays pure and
/// deterministic for tests — the daemon passes the real wall-clock time.
pub fn scan_heal_report(
    source: &dyn ManifestSource,
    pubkey: &str,
    root: &Path,
    protected_dir: &Path,
    node_id: &str,
    ts_unix: i64,
    sink: &dyn ConsoleSink,
) -> Vec<TamperEvent> {
    let manifest = match source.load() {
        Ok(manifest) => manifest,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "selfprotect: failed to load integrity manifest; skipping this cycle"
            );
            return Vec::new();
        }
    };

    if let Err(err) = manifest.verify_signature(pubkey) {
        tracing::warn!(
            error = %err,
            "selfprotect: integrity manifest failed signature verification; skipping this cycle"
        );
        return Vec::new();
    }

    let findings = check(&manifest, root);
    let mut events = Vec::with_capacity(findings.len());

    for finding in &findings {
        let remediation = match heal(finding, protected_dir, root) {
            Ok(()) => "restored from protected copy".to_string(),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    path = %finding.path,
                    "selfprotect: failed to heal tampered file"
                );
                format!("heal failed: {err}")
            }
        };

        let event = TamperEvent::from_finding(finding, node_id, ts_unix, &remediation);
        sink.report_tamper(&event);
        events.push(event);
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::NoopConsoleSink;
    use crate::error::SelfProtectError;
    use crate::manifest::IntegrityManifest;

    /// A [`ManifestSource`] that always fails to load — proves
    /// `scan_heal_report` no-ops rather than panicking or acting on nothing.
    struct FailingSource;

    impl ManifestSource for FailingSource {
        fn load(&self) -> Result<IntegrityManifest, SelfProtectError> {
            Err(SelfProtectError::Io(std::io::Error::other("no manifest")))
        }
    }

    #[test]
    fn load_failure_returns_empty_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let events = scan_heal_report(
            &FailingSource,
            "irrelevant-pubkey",
            dir.path(),
            dir.path(),
            "node-1",
            0,
            &NoopConsoleSink,
        );
        assert!(events.is_empty());
    }
}
