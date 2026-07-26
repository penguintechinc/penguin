//! [`UpdateError`]: every way [`crate::Updater::check`] or
//! [`crate::Updater::apply`] can fail.

use crate::archive::ArchiveError;
use crate::release::SelectError;
use crate::verify::VerifyError;

/// Every failure mode of the self-update flow, from "no verification key
/// configured" through network, parsing, archive, and signature failures,
/// to the final binary swap.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    /// [`crate::UpdateConfig::public_key`] is `None`. This is the
    /// deliberate fail-closed default (see the crate doc): with no key,
    /// nothing downloaded could ever be verified, so [`crate::Updater::apply`]
    /// refuses before making a single network call.
    #[error("no release verification key configured — self-update is disabled until one is")]
    NoVerificationKey,
    /// The running process's OS is not one this workspace ships releases
    /// for (see `client.md`'s platform matrix).
    #[error("this OS is not one penguin ships releases for")]
    UnsupportedOs,
    /// The running process's CPU architecture is not one this workspace
    /// ships releases for.
    #[error("this CPU architecture is not one penguin ships releases for")]
    UnsupportedArch,
    /// The GitHub releases API request itself failed (DNS, TLS, connect,
    /// timeout, ...).
    #[error("failed to reach GitHub releases API: {0}")]
    Request(#[source] reqwest::Error),
    /// The request succeeded but its body could not be read.
    #[error("failed to read GitHub releases API response body: {0}")]
    ResponseBody(#[source] reqwest::Error),
    /// The GitHub releases API (or an asset download) returned a non-200
    /// status.
    #[error("GitHub returned HTTP {0}")]
    HttpStatus(u16),
    /// The releases API response was not the JSON shape
    /// [`crate::release::GithubRelease`] expects.
    #[error("failed to parse GitHub releases API response: {0}")]
    Decode(#[source] serde_json::Error),
    /// The downloaded `.minisig` asset was not valid UTF-8 text.
    #[error("signature asset is not valid UTF-8 text")]
    InvalidSignatureEncoding,
    /// No compatible asset (or its signature) was found in the release —
    /// see [`SelectError`] for which.
    #[error(transparent)]
    AssetSelection(#[from] SelectError),
    /// The downloaded archive could not be verified — see [`VerifyError`]
    /// for which check failed.
    #[error(transparent)]
    Verify(#[from] VerifyError),
    /// The downloaded archive could not be unpacked, or contained no entry
    /// matching the expected binary name — see [`ArchiveError`].
    #[error(transparent)]
    Archive(#[from] ArchiveError),
    /// The verified, extracted binary could not be staged on disk ahead of
    /// the swap.
    #[error("failed to stage the downloaded binary on disk: {0}")]
    StageBinary(#[source] std::io::Error),
    /// `self_replace::self_replace` itself failed — the running executable
    /// was not swapped.
    #[error("failed to swap the running binary: {0}")]
    SelfReplace(#[source] std::io::Error),
    /// The blocking install step (staging + `self_replace`, run via
    /// `tokio::task::spawn_blocking`) panicked or was cancelled.
    #[error("update install task failed to run to completion: {0}")]
    InstallTask(#[source] tokio::task::JoinError),
}
