//! External plugin manifest parsing + security verification pipeline.
//!
//! Ports go-client/internal/extplugin's `manifest.go` and `verify.go`:
//! `plugin.json` parsing and the fail-closed ownership / world-writable /
//! SHA256 / minisign-signature pipeline, in that order. Process launching
//! and the go-plugin wire protocol are out of scope here — see the
//! `penguin-goplugin-host` crate for that half.

mod manifest;
mod os_stat;
mod verify;

pub use manifest::{Manifest, ManifestError, load_manifest};
pub use os_stat::{FileMeta, OsStat, StatSource};
pub use verify::{DEFAULT_TRUSTED_PUBLISHERS_DIR, Verifier, VerifyError};
