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
//! Later tasks: periodic re-verification and tamper response — see
//! `.superpowers/sdd/2026-08-22-agent-self-protection/`.

mod authz;
mod error;
mod event;
mod integrity;
mod manifest;
mod state;

pub use authz::{
    TeardownAuthz, TeardownCtx, TeardownInput, authorize, hash_secret, verify_break_glass,
    verify_secret,
};
pub use error::SelfProtectError;
pub use event::{TamperFinding, TamperKind};
pub use integrity::{check, heal};
pub use manifest::{IntegrityManifest, LocalFileSource, ManifestEntry, ManifestSource};
pub use state::{ProtectionState, is_armed};
