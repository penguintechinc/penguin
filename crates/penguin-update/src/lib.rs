//! Signed self-update: GitHub releases + minisign verify + in-place replace.
//!
//! Greenfield against a known contract, not a port: the Go reference
//! (`go-client/internal/update`) hardcoded `GOOS`/`GOARCH` as test stubs and
//! was never wired into any `cmd/` binary (see that package's `getGOOS`/
//! `getGOARCH` doc comments), so there is no working implementation to
//! preserve behavior from — only the release-asset contract goreleaser
//! produces (`go-client/.goreleaser.yaml`) to build against.
//!
//! # Flow
//!
//! [`Updater::check`] fetches the latest GitHub release and reports whether
//! it differs from the running version — no verification key required.
//! [`Updater::apply`] does the real work: resolve this platform's expected
//! asset filename ([`platform`]), find it and its `.minisig` sibling in the
//! release ([`release`]), download both, verify the signature
//! ([`verify`]), extract the target binary from the archive ([`archive`] —
//! both tar.gz and zip are implemented, unlike the Go reference which only
//! ever handled tar.gz), and swap it in via `self_replace`.
//!
//! # No verification key means no update
//!
//! [`UpdateConfig::public_key`] is `Option<String>`, not a hardcoded
//! constant. With `None`, [`Updater::apply`] refuses before any network
//! call — see [`UpdateConfig`]'s doc for why baking a real PenguinTech
//! release key is a deliberate follow-up rather than something this crate
//! decides on its own.
//!
//! # What is (and isn't) unit tested
//!
//! [`platform`], [`release`], [`archive`], and [`verify`] are pure logic
//! over bytes/structs and are exhaustively unit tested with no network and
//! no real binary swap. [`Updater`] itself — the actual HTTP fetch and
//! `self_replace` call — is the one deliberately thin, untested boundary;
//! see `tests/integration_download.rs` for the gated, opt-in real-network
//! proof.

mod archive;
mod error;
mod platform;
mod release;
mod updater;
mod verify;

pub use archive::ArchiveError;
pub use error::UpdateError;
pub use platform::{Arch, ArchiveFormat, Os, asset_filename, binary_filename, normalize_version};
pub use release::{GithubAsset, GithubRelease, SelectError, SelectedAsset, select_asset};
pub use updater::{UpdateConfig, Updater};
pub use verify::VerifyError;
