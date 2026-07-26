//! Integration test: `Updater::check` against the real GitHub releases API
//! for the real `penguintechinc/penguin` repository.
//!
//! # Running
//!
//! ```sh
//! PENGUIN_INTEGRATION=1 cargo test -p penguin-update --test integration_download -- --ignored --nocapture
//! ```
//!
//! Every test here is `#[ignore]` *and* separately checks
//! `PENGUIN_INTEGRATION=1` at runtime, so neither a plain `cargo test` nor a
//! bare `cargo test -- --ignored` ever reaches the real network — same
//! convention as `penguin-daemon/tests/external_plugin.rs` and
//! `penguin-goplugin-host/tests/goplugin_compat.rs`.
//!
//! This intentionally only exercises [`Updater::check`], never
//! [`Updater::apply`]: `apply` swaps the binary running the test process,
//! which is not something a CI job should ever do to itself even opt-in.
//! `apply`'s full pipeline (asset selection, minisign verification, archive
//! extraction) is already covered end-to-end by `src/updater.rs`'s sibling
//! modules' unit tests against synthetic data — this test's only job is to
//! prove the GitHub API request/response shape assumed by
//! [`crate::release::GithubRelease`] still matches reality.

use penguin_update::{UpdateConfig, Updater};

/// Skips the calling test (with a message) unless the integration tier is
/// explicitly opted into.
macro_rules! require_integration {
    () => {
        if std::env::var("PENGUIN_INTEGRATION").as_deref() != Ok("1") {
            eprintln!("SKIP: set PENGUIN_INTEGRATION=1 to run integration_download tests");
            return;
        }
    };
}

#[tokio::test]
#[ignore]
async fn check_against_the_real_penguin_repository_reports_a_release() {
    require_integration!();

    let updater = Updater::new(UpdateConfig {
        repo: "penguintechinc/penguin".to_string(),
        // Deliberately ancient so `available` is expected to read `true`
        // against whatever the real repository has actually published.
        current_version: "v0.0.0".to_string(),
        binary_name: "penguind".to_string(),
        public_key: None,
    });

    let (available, latest_version) = updater
        .check()
        .await
        .expect("GitHub releases API request should succeed");

    assert!(
        available,
        "v0.0.0 should read as older than any real release"
    );
    assert!(
        !latest_version.is_empty(),
        "latest_version should be a real tag name"
    );
}
