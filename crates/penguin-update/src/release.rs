//! GitHub releases-list response shape, latest-release selection, and pure
//! asset-selection logic.
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

use crate::platform::normalize_version;

/// The subset of GitHub's `GET /repos/{owner}/{repo}/releases` (list)
/// response this crate reads — one element per release. Extra fields (body,
/// author, prerelease flag, ...) are ignored by serde — same "read only what
/// you use" convention as `penguin-licensing`'s `ValidateResponse`.
///
/// Deliberately NOT `/releases/latest`'s single-object shape: that endpoint
/// excludes drafts *and prereleases* by GitHub's own design, and this
/// repo's own real releases are published as prereleases — see
/// [`select_latest_release`]'s doc comment for why `Updater` reads the list
/// endpoint instead and picks the newest one itself.
#[derive(Debug, Deserialize)]
pub struct GithubRelease {
    /// The release's git tag, e.g. `"v0.2.0"`. Carries the `v` prefix; see
    /// [`crate::platform::normalize_version`] before using it in a filename
    /// or a version comparison.
    pub tag_name: String,
    /// `true` for a release that has been created but not published yet.
    /// GitHub only ever returns draft releases to callers with push access
    /// to the repo, so this is normally always `false` for the
    /// unauthenticated requests this crate makes — checked anyway in
    /// [`select_latest_release`] so a draft can never be selected if one
    /// somehow is returned.
    #[serde(default)]
    pub draft: bool,
    /// Every asset attached to the release, in whatever order GitHub
    /// returns them.
    #[serde(default)]
    pub assets: Vec<GithubAsset>,
}

/// Picks the newest release out of a `/releases` list response.
///
/// `/releases/latest` looks like the obvious endpoint for this, but GitHub
/// excludes both drafts AND prereleases from it by design (see
/// <https://docs.github.com/en/rest/releases/releases#get-the-latest-release>).
/// `penguintechinc/penguin`'s own releases are published with
/// `"prerelease": true` (see `docs/PARITY.md` / the goreleaser prerelease
/// workflow), so `/releases/latest` 404s against this repo's real state
/// today — self-update would never find a release at all. The list endpoint
/// returns every release regardless of its prerelease flag, so this
/// function does the "which one is latest" selection itself: excludes
/// drafts (never something to install — it isn't published), keeps
/// prereleases, and picks the highest semantic version among the rest.
///
/// A release whose tag doesn't parse as semver (via
/// [`crate::platform::normalize_version`]) is skipped rather than treated as
/// an error — a malformed or unrelated tag on the repo should not block
/// self-update from finding the real releases alongside it.
pub fn select_latest_release(releases: Vec<GithubRelease>) -> Option<GithubRelease> {
    releases
        .into_iter()
        .filter(|release| !release.draft)
        .filter_map(|release| {
            let version = semver::Version::parse(normalize_version(&release.tag_name)).ok()?;
            Some((version, release))
        })
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, release)| release)
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
        draft_release(tag_name, false, asset_names)
    }

    fn draft_release(tag_name: &str, draft: bool, asset_names: &[&str]) -> GithubRelease {
        GithubRelease {
            tag_name: tag_name.to_string(),
            draft,
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
        assert!(!parsed.draft);
    }

    #[test]
    fn deserializes_a_releases_list_response_with_a_prerelease_and_no_assets() {
        // The exact shape `GET /repos/penguintechinc/penguin/releases` returns
        // today: a single element, `"prerelease": true`, `"assets": []` — the
        // real state this fix exists to handle. `select_latest_release` must
        // still pick this release up.
        let body = r#"[{
            "tag_name": "v1.0.0",
            "draft": false,
            "prerelease": true,
            "assets": []
        }]"#;

        let parsed: Vec<GithubRelease> =
            serde_json::from_str(body).expect("valid GitHub releases-list JSON");

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].tag_name, "v1.0.0");
        assert!(!parsed[0].draft);
        assert!(parsed[0].assets.is_empty());
    }

    #[test]
    fn select_latest_release_picks_a_prerelease_when_it_is_the_only_release() {
        // The bug this whole module exists to fix: `/releases/latest` would
        // 404 here because GitHub excludes prereleases from it. The list
        // endpoint + this selection function must still find it.
        let releases = vec![draft_release("v1.0.0", false, &[])];

        let selected = select_latest_release(releases).expect("the prerelease should be found");

        assert_eq!(selected.tag_name, "v1.0.0");
    }

    #[test]
    fn select_latest_release_excludes_drafts() {
        let releases = vec![
            draft_release("v2.0.0", true, &[]),
            draft_release("v1.0.0", false, &[]),
        ];

        let selected = select_latest_release(releases).expect("the non-draft release remains");

        assert_eq!(selected.tag_name, "v1.0.0");
    }

    #[test]
    fn select_latest_release_picks_the_highest_semver_not_list_order() {
        let releases = vec![
            draft_release("v1.0.0", false, &[]),
            draft_release("v2.5.0", false, &[]),
            draft_release("v2.4.9", false, &[]),
        ];

        let selected = select_latest_release(releases).expect("a release is selected");

        assert_eq!(selected.tag_name, "v2.5.0");
    }

    #[test]
    fn select_latest_release_skips_tags_that_do_not_parse_as_semver() {
        let releases = vec![
            draft_release("nightly", false, &[]),
            draft_release("v1.0.0", false, &[]),
        ];

        let selected = select_latest_release(releases).expect("the valid-semver release remains");

        assert_eq!(selected.tag_name, "v1.0.0");
    }

    #[test]
    fn select_latest_release_returns_none_for_an_empty_list() {
        assert!(select_latest_release(Vec::new()).is_none());
    }

    #[test]
    fn select_latest_release_returns_none_when_only_drafts_or_unparseable_tags_exist() {
        let releases = vec![
            draft_release("v1.0.0", true, &[]),
            draft_release("not-a-version", false, &[]),
        ];

        assert!(select_latest_release(releases).is_none());
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
