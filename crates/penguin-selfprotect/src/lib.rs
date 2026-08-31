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
//! Later tasks: periodic re-verification and tamper response — see
//! `.superpowers/sdd/2026-08-22-agent-self-protection/`.

mod error;
mod event;
mod integrity;
mod manifest;

pub use error::SelfProtectError;
pub use event::{TamperFinding, TamperKind};
pub use integrity::check;
pub use manifest::{IntegrityManifest, LocalFileSource, ManifestEntry, ManifestSource};
