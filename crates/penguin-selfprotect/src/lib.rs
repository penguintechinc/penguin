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
//! This crate currently covers Task 1 of the agent self-protection plan:
//! the manifest type, its signature check, and [`ManifestSource`] for
//! loading one. Hashing the actual on-disk files against the manifest,
//! periodic re-verification, and tamper response are later tasks — see
//! `.superpowers/sdd/2026-08-22-agent-self-protection/`.

mod error;
mod manifest;

pub use error::SelfProtectError;
pub use manifest::{IntegrityManifest, LocalFileSource, ManifestEntry, ManifestSource};
