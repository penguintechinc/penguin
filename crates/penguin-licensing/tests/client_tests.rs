//! Integration tests for [`LicenseClient`] against a hand-rolled local mock
//! server — no test ever contacts license.penguintech.io. `mock_server`
//! below understands just enough HTTP/1.1 to receive the one POST request
//! this client ever sends and hand back a canned response, so the suite
//! doesn't need a second HTTP-mocking dependency (and the risk that one
//! quietly drags in a second TLS/crypto stack alongside aws-lc-rs).

mod mock_server;

use std::os::unix::fs::PermissionsExt;

use mock_server::{MockResponse, MockServer};
use penguin_licensing::{LicenseClient, LicenseClientOptions};
use penguin_sdk::LicenseChecker;

fn options(license_key: &str, base_url: &str, cache_dir: &std::path::Path) -> LicenseClientOptions {
    LicenseClientOptions {
        license_key: license_key.to_string(),
        base_url: base_url.to_string(),
        cache_dir: Some(cache_dir.to_path_buf()),
        ..Default::default()
    }
}

/// A known-enabled flag, a known-disabled flag, and an unknown flag all
/// come back correctly after a successful fetch.
#[tokio::test]
async fn feature_enabled_matches_entitlement_and_defaults_unknown_to_false() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"tier":"enterprise","features":[{"name":"feature.ai","entitled":true},{"name":"feature.analytics","entitled":false}]}"#,
    )])
    .await;

    let client = LicenseClient::new(options("test_key", &server.base_url, dir.path()));
    client.refresh().await;

    assert!(client.feature_enabled("feature.ai"));
    assert!(!client.feature_enabled("feature.analytics"));
    assert!(!client.feature_enabled("feature.unknown"));
}

/// Without a license key, every feature reads disabled and no request is
/// ever sent (base_url points at a port nothing listens on; if `refresh`
/// tried to dial it, the test would still pass functionally but this pins
/// the short-circuit-before-network behavior documented on `refresh`).
#[tokio::test]
async fn feature_enabled_false_without_license_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let client = LicenseClient::new(options("", "http://127.0.0.1:1", dir.path()));

    client.refresh().await;

    assert!(!client.feature_enabled("any.feature"));
    assert_eq!(client.tier(), "");
}

/// `tier()` reports each tier the server can return.
#[tokio::test]
async fn tier_reports_each_known_tier() {
    for tier in ["community", "professional", "enterprise"] {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = format!(r#"{{"tier":"{tier}","features":[]}}"#);
        let server = MockServer::start(vec![MockResponse::json(200, &body)]).await;

        let client = LicenseClient::new(options("test_key", &server.base_url, dir.path()));
        client.refresh().await;

        assert_eq!(client.tier(), tier);
    }
}

/// A client that has never fetched or restored a cache reports an empty
/// tier and every feature disabled — plain sync `#[test]`, no runtime at
/// all, proving these calls need none.
#[test]
fn tier_and_features_are_empty_with_no_cache() {
    let client = LicenseClient::new(LicenseClientOptions {
        license_key: "test_key".to_string(),
        ..Default::default()
    });

    assert_eq!(client.tier(), "");
    assert!(!client.feature_enabled("anything"));
}

/// `feature_enabled`/`tier` are answerable with zero async runtime
/// involved: this test spins up no tokio runtime at all. A cache file is
/// seeded on disk by hand (no client, no network) and a fresh client reads
/// it back purely synchronously.
#[test]
fn sync_methods_answer_from_a_preloaded_cache_without_a_runtime() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("license-cache.json"),
        r#"{"tier":"professional","features":{"penguin.x":true,"penguin.y":false},"fetched_at":1700000000}"#,
    )
    .expect("seed cache file");

    let client = LicenseClient::new(LicenseClientOptions {
        license_key: "test_key".to_string(),
        cache_dir: Some(dir.path().to_path_buf()),
        ..Default::default()
    });

    assert_eq!(client.tier(), "professional");
    assert!(client.feature_enabled("penguin.x"));
    assert!(!client.feature_enabled("penguin.y"));
    assert!(!client.feature_enabled("penguin.unknown"));
}

/// A successful fetch persists the cache file with owner-only (0600)
/// permissions and the fetched data.
#[tokio::test]
async fn successful_fetch_persists_cache_with_mode_0600() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"tier":"enterprise","features":[{"name":"feature.ai","entitled":true}]}"#,
    )])
    .await;

    let client = LicenseClient::new(options("test_key", &server.base_url, dir.path()));
    client.refresh().await;

    let cache_path = dir.path().join("license-cache.json");
    let meta = std::fs::metadata(&cache_path).expect("cache file must exist");
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);

    let raw = std::fs::read_to_string(&cache_path).expect("read cache file");
    assert!(raw.contains("enterprise"));
    assert!(raw.contains("feature.ai"));
}

/// Once the server that answered a first successful fetch stops
/// listening entirely, a second `refresh()` neither panics nor clears the
/// cache — the last-known entitlements keep serving.
#[tokio::test]
async fn unreachable_server_after_success_serves_cached_value() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"tier":"enterprise","features":[{"name":"feature.ai","entitled":true}]}"#,
    )])
    .await;
    let base_url = server.base_url.clone();

    let client = LicenseClient::new(options("test_key", &base_url, dir.path()));
    client.refresh().await;
    assert_eq!(client.tier(), "enterprise");

    server.stop().await; // nothing is listening on that port anymore

    client.refresh().await; // must not panic and must not clear the cache
    assert_eq!(client.tier(), "enterprise");
    assert!(client.feature_enabled("feature.ai"));
}

/// A 5xx response following a successful fetch retains the previously
/// cached tier and features rather than clearing them.
#[tokio::test]
async fn server_5xx_after_success_retains_cached_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start(vec![
        MockResponse::json(
            200,
            r#"{"tier":"enterprise","features":[{"name":"feature.ai","entitled":true}]}"#,
        ),
        MockResponse::text(500, "internal server error"),
    ])
    .await;

    let client = LicenseClient::new(options("test_key", &server.base_url, dir.path()));

    client.refresh().await;
    assert_eq!(client.tier(), "enterprise");

    client.refresh().await; // second connection hits the 500 response
    assert_eq!(client.tier(), "enterprise", "tier must survive a 5xx");
    assert!(client.feature_enabled("feature.ai"));
}

/// Malformed JSON following a successful fetch retains the previously
/// cached state rather than clearing or crashing.
#[tokio::test]
async fn malformed_json_after_success_retains_cached_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start(vec![
        MockResponse::json(
            200,
            r#"{"tier":"professional","features":[{"name":"feature.sso","entitled":true}]}"#,
        ),
        MockResponse::text(200, "not json at all"),
    ])
    .await;

    let client = LicenseClient::new(options("test_key", &server.base_url, dir.path()));

    client.refresh().await;
    assert_eq!(client.tier(), "professional");

    client.refresh().await; // second connection returns malformed JSON
    assert_eq!(client.tier(), "professional");
    assert!(client.feature_enabled("feature.sso"));
}

/// A 200 response whose body is a JSON object entirely missing the fields
/// this client reads must not panic — `tier`/`features` fall back to their
/// serde defaults (empty string / empty vec) instead of erroring.
#[tokio::test]
async fn response_missing_known_fields_does_not_panic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start(vec![MockResponse::json(200, r#"{"valid":true}"#)]).await;

    let client = LicenseClient::new(options("test_key", &server.base_url, dir.path()));
    client.refresh().await; // must not panic

    assert_eq!(client.tier(), "");
    assert!(!client.feature_enabled("anything"));
}

/// A corrupt cache file on disk is treated as no cache at all — no panic,
/// no error surfaced anywhere reachable from the client's public API.
#[test]
fn corrupt_cache_file_is_ignored_not_panicked() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("license-cache.json"), b"{not valid json")
        .expect("seed corrupt cache");

    let client = LicenseClient::new(LicenseClientOptions {
        license_key: "test_key".to_string(),
        cache_dir: Some(dir.path().to_path_buf()),
        ..Default::default()
    });

    assert_eq!(client.tier(), "");
    assert!(!client.feature_enabled("anything"));
}

/// Cache state fetched by one client instance survives a simulated daemon
/// restart: a brand-new `LicenseClient` over the same cache directory,
/// pointed at a server that will never answer, still reports the
/// previously fetched entitlements.
#[tokio::test]
async fn cache_survives_simulated_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"tier":"enterprise","features":[{"name":"penguin.squawk","entitled":true},{"name":"penguin.off","entitled":false}]}"#,
    )])
    .await;

    let first_run = LicenseClient::new(options("test_key", &server.base_url, dir.path()));
    first_run.refresh().await;
    server.stop().await;

    let unreachable_url = MockServer::unreachable_base_url().await;
    let restarted = LicenseClient::new(options("test_key", &unreachable_url, dir.path()));

    assert_eq!(restarted.tier(), "enterprise");
    assert!(restarted.feature_enabled("penguin.squawk"));
    assert!(!restarted.feature_enabled("penguin.off"));
}

/// The Go client (`go-client/internal/licensing`) has no domain-based
/// bypass at all — it never inspects `base_url` or any deployment-domain
/// concept, only the license key. This pins that the port didn't grow one:
/// pointing at a well-known "license bypass" domain string does not
/// short-circuit validation. The only way features come on is a
/// successful, real fetch.
#[test]
fn no_domain_based_bypass_exists() {
    let client = LicenseClient::new(LicenseClientOptions {
        license_key: String::new(),
        base_url: "https://license.penguintech.cloud".to_string(),
        ..Default::default()
    });

    assert!(!client.feature_enabled("anything"));
    assert_eq!(client.tier(), "");
}

/// `refresh` starts and stops cleanly, and the background loop actually
/// calls the server more than once before being stopped.
///
/// This polls for the "at least 2 calls" condition instead of sleeping a
/// fixed real duration and then asserting once: the old version slept a
/// fixed 120ms and asserted on however many of the interval's real 20ms
/// ticks happened to land in that window — a margin a loaded/slow CI runner
/// can blow through (that's exactly what CI hit: only 1 completed refresh in
/// 120ms instead of the expected 6+).
///
/// `tokio::time::pause`'s auto-advancing virtual clock
/// (`#[tokio::test(start_paused = true)]`) is the deterministic fix of
/// choice per house style, but it does not work for this loop: it was tried
/// first and reproducibly stopped after exactly 1 call. The reason is
/// structural, not a test bug — `spawn_background_refresh` awaits the first
/// `refresh()`'s real, variable-latency network I/O *before* creating the
/// `interval`, so that interval's periodic-tick timer is only registered in
/// tokio's timer wheel after that I/O completes. Meanwhile this test's own
/// wait registers its timer up front. Tokio's paused-clock auto-advance
/// jumps straight to the nearest timer *currently in the wheel* whenever the
/// runtime goes idle — with only the outer wait's (later, already-registered)
/// deadline present, it fast-forwards straight past where the interval's
/// tick would have landed, skipping every tick but the one immediate
/// call already made. Restructuring `spawn_background_refresh` to register
/// the interval before the first fetch wouldn't help either: `Interval`
/// arms each tick's wheel entry lazily on `.tick()`, not all up front, so
/// the second tick's entry still cannot exist until the first `.tick()`
/// (consumed synchronously right after the immediate refresh, per the
/// existing comment below) has been awaited.
///
/// So this test instead polls real elapsed time, bounded by a 5s ceiling —
/// far beyond any plausible CI scheduling delay for 2 loopback round trips
/// 20ms apart — rather than a single fixed-sleep-then-assert margin. It
/// still resolves in low tens of milliseconds under normal conditions and
/// only pays the full 5s if the loop has genuinely stopped ticking.
#[tokio::test]
async fn background_refresh_starts_and_stops_and_ticks_more_than_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"tier":"professional","features":[]}"#,
    )])
    .await;

    let client = std::sync::Arc::new(LicenseClient::new(options(
        "test_key",
        &server.base_url,
        dir.path(),
    )));
    let handle = client.spawn_background_refresh(std::time::Duration::from_millis(20));

    let poll_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while server.call_count() < 2 && std::time::Instant::now() < poll_deadline {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    handle.stop().await;

    assert_eq!(client.tier(), "professional");
    assert!(
        server.call_count() >= 2,
        "expected at least 2 background refreshes within 5s, got {}",
        server.call_count()
    );
}
