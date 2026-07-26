//! GitHub "latest release" response shape and pure asset-selection logic.
//!
//! [`select_asset`] is deliberately an exact-filename match, tightened from
//! the Go reference's `strings.Contains(asset.Name, goos) &&
//! strings.Contains(asset.Name, goarch)` — a loose substring match can pick
//! the wrong platform's archive by accident (e.g. an `arm64` host matching
//! a `"...windows_amd64..."` asset name because it happens to contain
//! `"64"` somewhere convenient). Exact equality against the filename
//! [`crate::platform::asset_filename`] builds is the only match that can
//! never do that.

use serde::Deserialize;

/// The subset of GitHub's `GET /repos/{owner}/{repo}/releases/latest`
/// response this crate reads. Extra fields (body, author, prerelease flag,
/// ...) are ignored by serde — same "read only what you use" convention as
/// `penguin-licensing`'s `ValidateResponse`.
#[derive(Debug, Deserialize)]
pub struct GithubRelease {
    /// The release's git tag, e.g. `"v0.2.0"`. Carries the `v` prefix; see
    /// [`crate::platform::normalize_version`] before using it in a filename
    /// or a version comparison.
    pub tag_name: String,
    /// Every asset attached to the release, in whatever order GitHub
    /// returns them.
    #[serde(default)]
    pub assets: Vec<GithubAsset>,
}

/// One release asset: a filename and the URL to download its raw bytes.
#[derive(Debug, Deserialize)]
pub struct GithubAsset {
    pub name: String,
    pub browser_download_url: String,
}

/// A release's binary archive asset, paired with its `.minisig` sibling —
/// the two URLs [`crate::Updater::apply`] downloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedAsset {
    pub binary_url: String,
    pub signature_url: String,
}

/// Every way [`select_asset`] can fail to find what it's looking for.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelectError {
    /// No asset in the release is named exactly `expected_filename` — this
    /// platform's archive was not published (or was published under a
    /// different name than expected).
    #[error("no asset named {0:?} in this release")]
    AssetNotFound(String),
    /// The binary archive itself was found, but no `<name>.minisig`
    /// sibling asset exists — there is nothing to verify it against, so it
    /// can never be trusted enough to install.
    #[error("asset {0:?} has no .minisig signature asset alongside it")]
    SignatureNotFound(String),
}

/// Finds the asset named exactly `expected_filename` in `release`, plus its
/// `<expected_filename>.minisig` sibling. Returns both download URLs, or
/// the specific reason neither could be assembled.
pub fn select_asset(
    release: &GithubRelease,
    expected_filename: &str,
) -> Result<SelectedAsset, SelectError> {
    let binary = find_asset(release, expected_filename)
        .ok_or_else(|| SelectError::AssetNotFound(expected_filename.to_string()))?;

    let expected_signature_name = format!("{expected_filename}.minisig");
    let signature = find_asset(release, &expected_signature_name)
        .ok_or_else(|| SelectError::SignatureNotFound(expected_filename.to_string()))?;

    Ok(SelectedAsset {
        binary_url: binary.browser_download_url.clone(),
        signature_url: signature.browser_download_url.clone(),
    })
}

/// Exact (never substring) name match against `release.assets`.
fn find_asset<'a>(release: &'a GithubRelease, name: &str) -> Option<&'a GithubAsset> {
    release.assets.iter().find(|asset| asset.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> GithubAsset {
        GithubAsset {
            name: name.to_string(),
            browser_download_url: format!("https://example.invalid/{name}"),
        }
    }

    fn release(tag_name: &str, asset_names: &[&str]) -> GithubRelease {
        GithubRelease {
            tag_name: tag_name.to_string(),
            assets: asset_names.iter().map(|n| asset(n)).collect(),
        }
    }

    #[test]
    fn deserializes_the_fields_this_crate_reads_from_real_github_json() {
        let body = r#"{
            "tag_name": "v1.2.3",
            "assets": [
                {
                    "name": "penguin_1.2.3_linux_amd64.tar.gz",
                    "browser_download_url": "https://github.com/owner/repo/releases/download/v1.2.3/penguin_1.2.3_linux_amd64.tar.gz",
                    "id": 12345,
                    "content_type": "application/gzip"
                }
            ],
            "body": "release notes",
            "prerelease": false
        }"#;

        let parsed: GithubRelease = serde_json::from_str(body).expect("valid GitHub release JSON");

        assert_eq!(parsed.tag_name, "v1.2.3");
        assert_eq!(parsed.assets.len(), 1);
        assert_eq!(parsed.assets[0].name, "penguin_1.2.3_linux_amd64.tar.gz");
        assert_eq!(
            parsed.assets[0].browser_download_url,
            "https://github.com/owner/repo/releases/download/v1.2.3/penguin_1.2.3_linux_amd64.tar.gz"
        );
    }

    #[test]
    fn deserializes_a_release_with_no_assets_field_at_all() {
        let parsed: GithubRelease =
            serde_json::from_str(r#"{"tag_name": "v1.2.3"}"#).expect("assets defaults to empty");
        assert!(parsed.assets.is_empty());
    }

    #[test]
    fn select_asset_finds_the_binary_and_its_signature() {
        let rel = release(
            "v1.2.3",
            &[
                "penguin_1.2.3_linux_amd64.tar.gz",
                "penguin_1.2.3_linux_amd64.tar.gz.minisig",
                "penguin_1.2.3_windows_amd64.zip",
                "penguin_1.2.3_windows_amd64.zip.minisig",
            ],
        );

        let selected =
            select_asset(&rel, "penguin_1.2.3_linux_amd64.tar.gz").expect("asset present");

        assert_eq!(
            selected.binary_url,
            "https://example.invalid/penguin_1.2.3_linux_amd64.tar.gz"
        );
        assert_eq!(
            selected.signature_url,
            "https://example.invalid/penguin_1.2.3_linux_amd64.tar.gz.minisig"
        );
    }

    #[test]
    fn select_asset_rejects_when_only_a_different_platform_is_present() {
        // Only darwin/arm64 is published — a linux/amd64 host must get a
        // clean "not found", never an accidental substring match.
        let rel = release(
            "v1.2.3",
            &[
                "penguin_1.2.3_darwin_arm64.tar.gz",
                "penguin_1.2.3_darwin_arm64.tar.gz.minisig",
            ],
        );

        let err = select_asset(&rel, "penguin_1.2.3_linux_amd64.tar.gz")
            .expect_err("no matching asset for this platform");

        assert_eq!(
            err,
            SelectError::AssetNotFound("penguin_1.2.3_linux_amd64.tar.gz".to_string())
        );
    }

    #[test]
    fn select_asset_never_substring_matches_a_similarly_named_asset() {
        // Go's `strings.Contains` matching would have accepted this: the
        // asset name contains both "linux" and "amd64" as substrings, but
        // it is not an exact match for the expected filename.
        let rel = release(
            "v1.2.3",
            &["penguin-extra_1.2.3_linux_amd64_debug.tar.gz.bak"],
        );

        let err = select_asset(&rel, "penguin_1.2.3_linux_amd64.tar.gz")
            .expect_err("substring-similar name must not match");

        assert_eq!(
            err,
            SelectError::AssetNotFound("penguin_1.2.3_linux_amd64.tar.gz".to_string())
        );
    }

    #[test]
    fn select_asset_rejects_when_signature_asset_is_missing() {
        let rel = release("v1.2.3", &["penguin_1.2.3_linux_amd64.tar.gz"]);

        let err = select_asset(&rel, "penguin_1.2.3_linux_amd64.tar.gz")
            .expect_err("binary present but no .minisig sibling");

        assert_eq!(
            err,
            SelectError::SignatureNotFound("penguin_1.2.3_linux_amd64.tar.gz".to_string())
        );
    }

    #[test]
    fn select_asset_rejects_on_an_empty_release() {
        let rel = release("v1.2.3", &[]);

        let err =
            select_asset(&rel, "penguin_1.2.3_linux_amd64.tar.gz").expect_err("empty asset list");

        assert_eq!(
            err,
            SelectError::AssetNotFound("penguin_1.2.3_linux_amd64.tar.gz".to_string())
        );
    }
}
