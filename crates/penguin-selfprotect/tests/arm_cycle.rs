//! Integration test for Task 10's full arm cycle: a fake [`ManifestSource`]
//! serving a signed manifest, [`scan_heal_report`] driving one
//! check→heal→report pass against a corrupted temp file, and a fake
//! [`ConsoleSink`] capturing what got reported.
//!
//! Mirrors `src/manifest.rs`'s own `testfix::signed_manifest` pattern
//! (ephemeral minisign keypair, `minisign` is a dev-dependency only — see
//! that module's doc for why) rather than reusing it directly: `testfix` is
//! `pub(crate)`, unreachable from an external integration-test crate.

use std::io::Cursor;
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use penguin_selfprotect::{
    ConsoleSink, IntegrityManifest, ManifestEntry, ManifestSource, SelfProtectError, TamperEvent,
    TamperEventKind, scan_heal_report,
};

/// Test helper: SHA-256 of bytes as lowercase hex.
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// A [`ManifestSource`] that always returns a clone of a fixed, already
/// signed manifest — no filesystem or network involved.
struct FakeSource(IntegrityManifest);

impl ManifestSource for FakeSource {
    fn load(&self) -> Result<IntegrityManifest, SelfProtectError> {
        Ok(self.0.clone())
    }
}

/// A [`ConsoleSink`] that records every reported [`TamperEvent`] in-memory
/// instead of talking to a real console — `Mutex` rather than `RefCell`
/// since the trait requires `Send + Sync`.
#[derive(Default)]
struct FakeSink {
    events: Mutex<Vec<TamperEvent>>,
}

impl ConsoleSink for FakeSink {
    fn report_tamper(&self, event: &TamperEvent) {
        self.events
            .lock()
            .expect("fake sink lock")
            .push(event.clone());
    }

    fn poll_deauthorized(&self, _node_id: &str) -> bool {
        false
    }
}

/// Signs `manifest`'s canonical bytes with a fresh throwaway minisign
/// keypair, returning `(public_key_text, signed_manifest)` — same shape as
/// `src/manifest.rs`'s private `testfix::signed_manifest`.
fn sign(mut manifest: IntegrityManifest) -> (String, IntegrityManifest) {
    let keypair =
        minisign::KeyPair::generate_unencrypted_keypair().expect("generate minisign keypair");
    let data_reader = Cursor::new(manifest.canonical_bytes());
    let signature_box =
        minisign::sign(Some(&keypair.pk), &keypair.sk, data_reader, None, None).expect("sign");
    manifest.signature = signature_box.into_string();
    let public_key_text = keypair.pk.to_box().expect("public key box").into_string();
    (public_key_text, manifest)
}

/// One full arm cycle: a corrupted `bin/penguind` under `root`, its pristine
/// copy under `protected`, and a signed manifest attesting to the pristine
/// hash. `scan_heal_report` must heal the file back to the pristine bytes
/// and report exactly one `BinaryModified` event, to both its return value
/// and the `ConsoleSink`.
#[test]
fn scan_heal_report_heals_and_reports_exactly_one_tamper_event() {
    let root = tempfile::tempdir().expect("root tempdir");
    let protected = tempfile::tempdir().expect("protected tempdir");

    let pristine = b"real-penguind-binary-bytes";
    std::fs::create_dir_all(protected.path().join("bin")).expect("mkdir protected/bin");
    std::fs::write(protected.path().join("bin/penguind"), pristine).expect("write protected copy");

    std::fs::create_dir_all(root.path().join("bin")).expect("mkdir root/bin");
    std::fs::write(
        root.path().join("bin/penguind"),
        b"corrupted-bytes-from-attacker",
    )
    .expect("write corrupted target");

    let manifest = IntegrityManifest {
        version: 1,
        entries: vec![ManifestEntry {
            path: "bin/penguind".to_string(),
            sha256: sha256_hex(pristine),
            mode: 0o755,
        }],
        signature: String::new(),
    };
    let (pubkey, signed_manifest) = sign(manifest);
    let source = FakeSource(signed_manifest);
    let sink = FakeSink::default();

    let events = scan_heal_report(
        &source,
        &pubkey,
        root.path(),
        protected.path(),
        "node-1",
        1_700_000_000,
        &sink,
    );

    // The corrupted file was healed back to the pristine bytes.
    let healed = std::fs::read(root.path().join("bin/penguind")).expect("read healed file");
    assert_eq!(
        healed, pristine,
        "expected the corrupted file to be restored from the protected copy"
    );

    // Exactly one TamperEvent, of kind BinaryModified, came back from the call...
    assert_eq!(
        events.len(),
        1,
        "expected exactly one TamperEvent; got: {events:?}"
    );
    assert_eq!(events[0].kind, TamperEventKind::BinaryModified);
    assert_eq!(events[0].path, "bin/penguind");
    assert_eq!(events[0].node_id, "node-1");
    assert_eq!(events[0].ts_unix, 1_700_000_000);

    // ...and exactly one was pushed to the ConsoleSink, matching it.
    let sink_events = sink.events.lock().expect("fake sink lock");
    assert_eq!(
        sink_events.len(),
        1,
        "expected exactly one event pushed to the sink"
    );
    assert_eq!(sink_events[0].kind, TamperEventKind::BinaryModified);
    assert_eq!(sink_events[0].path, "bin/penguind");
}

/// An unsigned/tampered manifest (wrong signature) must never be acted on —
/// `scan_heal_report` returns an empty vector and never touches disk or the
/// sink, rather than healing off unverified data.
#[test]
fn scan_heal_report_no_ops_on_signature_failure() {
    let root = tempfile::tempdir().expect("root tempdir");
    let protected = tempfile::tempdir().expect("protected tempdir");
    std::fs::write(root.path().join("bin_missing_marker"), b"unused").expect("touch marker");

    let manifest = IntegrityManifest {
        version: 1,
        entries: vec![ManifestEntry {
            path: "bin/penguind".to_string(),
            sha256: sha256_hex(b"whatever"),
            mode: 0o755,
        }],
        signature: String::new(),
    };
    // Sign with one keypair, then verify with an unrelated second keypair's
    // public key — the signature must fail to verify.
    let (_correct_pubkey, signed_manifest) = sign(manifest);
    let wrong_keypair =
        minisign::KeyPair::generate_unencrypted_keypair().expect("generate minisign keypair");
    let wrong_pubkey = wrong_keypair
        .pk
        .to_box()
        .expect("public key box")
        .into_string();

    let source = FakeSource(signed_manifest);
    let sink = FakeSink::default();

    let events = scan_heal_report(
        &source,
        &wrong_pubkey,
        root.path(),
        protected.path(),
        "node-1",
        1_700_000_000,
        &sink,
    );

    assert!(events.is_empty(), "expected no events on signature failure");
    assert!(
        sink.events.lock().expect("fake sink lock").is_empty(),
        "expected nothing reported to the sink on signature failure"
    );
    assert!(
        !root.path().join("bin").exists(),
        "expected no directories/files created by a no-op cycle"
    );
}
