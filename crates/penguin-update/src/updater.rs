//! [`Updater`]: the network + filesystem boundary that ties platform
//! detection, GitHub release lookup, minisign verification, archive
//! extraction, and the actual binary swap together.
//!
//! This module is deliberately the one piece of the crate NOT covered by
//! unit tests — every pure decision it delegates to ([`crate::platform`],
//! [`crate::release`], [`crate::archive`], [`crate::verify`]) is tested in
//! its own module instead. The one exception is
//! [`Updater::apply`]'s no-verification-key short-circuit, which is pure,
//! deterministic, and runs before any network call — see the test below.
//! The real network+swap path is only exercised by the gated integration
//! test in `tests/integration_download.rs`.

use std::io::Write as _;
use std::sync::OnceLock;
use std::time::Duration;

use crate::archive::extract_binary;
use crate::error::UpdateError;
use crate::platform::{self, Arch, Os};
use crate::release::{self, GithubRelease};
use crate::verify;

/// GitHub's REST API base URL. Not configurable — unlike the Go reference's
/// test-only `baseURL` field, this crate's HTTP fetch is deliberately
/// outside unit-test coverage (see the module doc), so there is no test
/// double that ever needs to override it.
const GITHUB_API_BASE: &str = "https://api.github.com";

/// GitHub's REST API rejects requests with no `User-Agent` header.
const USER_AGENT: &str = concat!("penguin-update/", env!("CARGO_PKG_VERSION"));

/// Per-request timeout, matching `client.md`'s "Request timeout: 30s
/// default" API-client standard.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Configures an [`Updater`].
///
/// `public_key` is the minisign public key text trusted to sign releases.
/// `None` means no key has been provisioned yet, and [`Updater::apply`]
/// refuses to proceed — fail closed: no key means no verification is
/// possible, which is strictly worse than not updating at all. Baking a
/// real PenguinTech release key into the daemon binary is a deliberate,
/// reviewed follow-up (see `docs/PARITY.md`), not something this crate
/// decides on its own initiative — see `bins/penguind/src/daemon_main.rs`'s
/// `RELEASE_PUBLIC_KEY` constant for where that decision lives.
pub struct UpdateConfig {
    /// GitHub repository in `"owner/name"` form, e.g. `"penguintechinc/penguin"`.
    pub repo: String,
    /// The running binary's own version string (with or without a leading
    /// `v` — both compare equal via [`platform::normalize_version`]).
    pub current_version: String,
    /// The binary's filename inside the release archive, without an OS
    /// suffix — e.g. `"penguind"` (this crate appends `.exe` on Windows
    /// itself via [`platform::binary_filename`]).
    pub binary_name: String,
    /// The minisign public key trusted to verify release signatures.
    pub public_key: Option<String>,
}

/// Checks for, and applies, penguin releases signed with
/// [`UpdateConfig::public_key`].
pub struct Updater {
    config: UpdateConfig,
    http: reqwest::Client,
}

impl Updater {
    /// Builds an updater. Does not touch the network or filesystem itself.
    pub fn new(config: UpdateConfig) -> Updater {
        Updater {
            config,
            http: build_http_client(),
        }
    }

    /// Fetches the latest GitHub release and reports whether it differs
    /// from [`UpdateConfig::current_version`]. Deliberately independent of
    /// [`UpdateConfig::public_key`] — a caller can always learn whether an
    /// update exists, even before a verification key has been provisioned;
    /// only [`Updater::apply`] refuses to act on that knowledge.
    pub async fn check(&self) -> Result<(bool, String), UpdateError> {
        let release = self.fetch_latest_release().await?;
        let available = platform::normalize_version(&release.tag_name)
            != platform::normalize_version(&self.config.current_version);
        Ok((available, release.tag_name))
    }

    /// Downloads, verifies, extracts, and installs the latest release over
    /// the running executable.
    ///
    /// Fails closed with [`UpdateError::NoVerificationKey`] — before any
    /// network call — when no verification key is configured. Otherwise:
    /// resolve this platform's expected asset name, find it (and its
    /// `.minisig` sibling) in the release, download both, verify the
    /// signature, extract the binary matching
    /// [`UpdateConfig::binary_name`], and swap it in via `self_replace`.
    pub async fn apply(&self) -> Result<(), UpdateError> {
        let Some(public_key) = self.config.public_key.as_deref() else {
            return Err(UpdateError::NoVerificationKey);
        };

        let os = Os::current().ok_or(UpdateError::UnsupportedOs)?;
        let arch = Arch::current().ok_or(UpdateError::UnsupportedArch)?;

        let release = self.fetch_latest_release().await?;
        let version = platform::normalize_version(&release.tag_name);
        let expected_filename = platform::asset_filename(version, os, arch);
        let selected = release::select_asset(&release, &expected_filename)?;

        let archive_bytes = self.download_bytes(&selected.binary_url).await?;
        let signature_text = self.download_text(&selected.signature_url).await?;

        verify::verify(&archive_bytes, &signature_text, public_key)?;

        let binary_filename = platform::binary_filename(&self.config.binary_name, os);
        let extracted = extract_binary(&archive_bytes, os.archive_format(), &binary_filename)?;

        install(extracted).await
    }

    /// `GET /repos/{repo}/releases/latest`, parsed into [`GithubRelease`].
    async fn fetch_latest_release(&self) -> Result<GithubRelease, UpdateError> {
        let url = format!(
            "{GITHUB_API_BASE}/repos/{repo}/releases/latest",
            repo = self.config.repo
        );
        let response = self
            .http
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await
            .map_err(UpdateError::Request)?;

        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(UpdateError::ResponseBody)?;
        if status != 200 {
            return Err(UpdateError::HttpStatus(status));
        }

        serde_json::from_slice(&body).map_err(UpdateError::Decode)
    }

    /// Downloads `url`'s raw bytes (the archive, or a signature file).
    async fn download_bytes(&self, url: &str) -> Result<Vec<u8>, UpdateError> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(UpdateError::Request)?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(UpdateError::HttpStatus(status));
        }
        let body = response.bytes().await.map_err(UpdateError::ResponseBody)?;
        Ok(body.to_vec())
    }

    /// Downloads `url` and decodes it as UTF-8 text — used for the
    /// `.minisig` signature asset, which is always ASCII/base64 text.
    async fn download_text(&self, url: &str) -> Result<String, UpdateError> {
        let bytes = self.download_bytes(url).await?;
        String::from_utf8(bytes).map_err(|_| UpdateError::InvalidSignatureEncoding)
    }
}

/// Stages `binary` in a temp file and swaps it in for the running
/// executable via `self_replace`. Blocking filesystem I/O throughout, so it
/// runs on a blocking-pool thread rather than an async worker thread.
async fn install(binary: Vec<u8>) -> Result<(), UpdateError> {
    tokio::task::spawn_blocking(move || install_blocking(&binary))
        .await
        .map_err(UpdateError::InstallTask)?
}

/// The actual staging + swap, synchronous. `self_replace::self_replace`
/// locates and restores the running executable's own permission bits onto
/// the staged file itself — see that crate's doc comment — so this
/// function does not need to (and must not try to) chmod anything.
fn install_blocking(binary: &[u8]) -> Result<(), UpdateError> {
    let mut staged = tempfile::NamedTempFile::new().map_err(UpdateError::StageBinary)?;
    staged.write_all(binary).map_err(UpdateError::StageBinary)?;
    staged.flush().map_err(UpdateError::StageBinary)?;

    self_replace::self_replace(staged.path()).map_err(UpdateError::SelfReplace)
}

/// Builds the reqwest client used for every GitHub request: rustls with the
/// aws-lc-rs crypto provider (installed once, process-wide) and root
/// certificates supplied manually from `webpki-roots`.
///
/// Deliberately manual rather than reqwest's default TLS setup, for the
/// same reason as `penguin-licensing::client::build_http_client` (see that
/// function's doc comment): the workspace is pinned to aws-lc-rs everywhere
/// for the go-plugin P-521 handshake, and reqwest's default TLS stack would
/// pull `ring` back in. `cargo tree -p penguin-update -i ring` staying
/// empty is the gate that proves this never regresses.
fn build_http_client() -> reqwest::Client {
    ensure_crypto_provider_installed();

    let root_store = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    // NOT wrapped in `Some(...)` — see `penguin-licensing`'s identical note:
    // `use_preconfigured_tls` wraps its argument in `Some(...)` itself.
    reqwest::Client::builder()
        .use_preconfigured_tls(tls_config)
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("penguin-update HTTP client config is static and always valid")
}

/// Installs the aws-lc-rs crypto provider as the process default, exactly
/// once. Idempotent: losing the install race to another initializer
/// elsewhere in the daemon (license client, go-plugin TLS, ...) is not an
/// error, since the whole workspace is pinned to aws-lc-rs.
fn ensure_crypto_provider_installed() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(public_key: Option<&str>) -> UpdateConfig {
        UpdateConfig {
            repo: "penguintechinc/penguin".to_string(),
            current_version: "0.2.0".to_string(),
            binary_name: "penguind".to_string(),
            public_key: public_key.map(str::to_string),
        }
    }

    /// The one behavior of [`Updater::apply`] this module tests directly:
    /// with no verification key configured, it must refuse before ever
    /// touching the network — this assertion holds with no mock server and
    /// no `PENGUIN_INTEGRATION` opt-in, because the short-circuit happens
    /// before the first `.await` that could reach out.
    #[tokio::test]
    async fn apply_fails_closed_with_no_verification_key_and_never_touches_the_network() {
        let updater = Updater::new(config(None));

        let err = updater.apply().await.expect_err("no key configured");

        assert!(matches!(err, UpdateError::NoVerificationKey));
    }
}
