//! Agent self-protection: signed integrity manifests the running agent
//! verifies before trusting its own binary, modules, or config on disk.
//!
//! [`IntegrityManifest`] is the controller-signed record of what an agent
//! install is supposed to contain (path, SHA-256, mode per file). The
//! agent only ever verifies — [`IntegrityManifest::verify_signature`] is a
//! thin wrapper over `penguin_update::verify` — signing happens out of
//! band, once, by the controller when a manifest is issued. See that
//! crate's `verify.rs` for why minisign and why verify-only.
//!
//! Task 1: the manifest type, its signature check, and [`ManifestSource`]
//! for loading one.
//!
//! Task 2: on-disk integrity verification via [`check`] — hashes files on
//! disk, compares to the manifest, and produces [`TamperFinding`] for any
//! mismatches or missing files.
//!
//! Task 4: the tamper-protection secret's hash/verify pair,
//! [`hash_secret`]/[`verify_secret`] — Argon2id, never plaintext at rest or
//! in logs.
//!
//! Task 5: the teardown/uninstall authorization decision,
//! [`authorize`] — precedence across a console-recorded deauthorization, a
//! node-bound break-glass token ([`verify_break_glass`]), and the local
//! secret from Task 4.
//!
//! Task 10: the daemon-facing loop body, [`scan_heal_report`] — load,
//! verify, [`check`], [`heal`], and report a [`TamperEvent`] per finding to
//! a [`ConsoleSink`] — plus [`TamperEvent`]/[`TamperEventKind`] themselves.
//! [`NoopConsoleSink`] is the only [`ConsoleSink`] implementation until SP2
//! (see `console.rs`'s module doc). The daemon's own arming/wiring
//! (`penguind::daemon_main`) is outside this crate.

mod authz;
mod console;
mod error;
mod event;
mod fleetdm;
mod integrity;
mod manifest;
mod monitor;
mod state;

pub use authz::{
    TeardownAuthz, TeardownCtx, TeardownInput, authorize, hash_secret, verify_break_glass,
    verify_secret,
};
pub use console::{ConsoleSink, NoopConsoleSink};
pub use error::SelfProtectError;
pub use event::{TamperEvent, TamperEventKind, TamperFinding, TamperKind};
pub use fleetdm::{FleetProbe, FleetStatus, RealFleetProbe, detect, fleet_resource_attrs};
pub use integrity::{check, heal};
pub use manifest::{IntegrityManifest, LocalFileSource, ManifestEntry, ManifestSource};
pub use monitor::scan_heal_report;
pub use state::{ProtectionState, is_armed};
