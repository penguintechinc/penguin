//! Namespaced secure secret storage for penguin modules.
//!
//! Rust port of the Go `internal/secrets` package: [`Store`] implements
//! [`penguin_sdk::SecretStore`], backed by the OS keychain (Windows
//! Credential Manager, macOS Keychain, or Linux Secret Service, whichever
//! the `keyring` crate resolves to for this build target) with an
//! encrypted-file fallback for headless daemons that have no platform
//! credential store to talk to.
//!
//! # The file backend is not wire-compatible with Go's
//!
//! Go's file backend came from `99designs/keyring`, which encrypts records
//! as JWE under a password-derived key. The Rust `keyring` crate has no
//! file backend at all, so [`file_backend`] implements one from scratch:
//! XChaCha20-Poly1305 under a random 32-byte master key
//! ([`ensure_master_key`]), not a password-derived one, and not JWE.
//! **Files the Go daemon wrote are not readable here, and vice versa.**
//! This is a known, accepted migration gap — see `docs/PARITY.md`. Modules
//! do not carry secrets across the migration; they simply re-authenticate
//! from their `api_key` the first time they run under the Rust daemon.
//!
//! # Never let a test touch a real OS keyring
//!
//! [`Backend::FileOnly`] is the only selection that can never perform a
//! platform keyring syscall or IPC call — every test in this crate uses it
//! exclusively. [`Backend::Auto`] (the production default) is never
//! constructed anywhere under `#[cfg(test)]`.

mod file_backend;
mod master_key;
mod platform_backend;
mod store;

pub use master_key::{MasterKey, ensure_master_key};
pub use store::{Backend, Config, Store};
