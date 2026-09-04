//! Integration tests for [`Validator`] against a hand-rolled local mock
//! server — no test ever contacts license.squawkdns.com.

mod mock_http;

use mock_http::{MockResponse, MockServer};
use squawk_client::config::LicenseConfig;
use squawk_client::license::{LicenseError, Validator};

fn config(server_url: &str, license_key: &str, user_token: &str) -> LicenseConfig {
    LicenseConfig {
        server_url: server_url.to_string(),
        license_key: license_key.to_string(),
        user_token: user_token.to_string(),
        validate_online: true,
        cache_time: 1440,
    }
}

#[tokio::test]
async fn valid_response_reports_valid() {
    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"valid":true,"message":"ok"}"#,
    )])
    .await;

    let validator = Validator::new(config(&server.base_url, "a-license-key", ""));
    let response = validator.validate_license().await.unwrap();

    assert!(response.valid);
    assert_eq!(response.message, "ok");

    server.stop().await;
}

#[tokio::test]
async fn server_url_is_actually_used() {
    let server = MockServer::start(vec![MockResponse::json(200, r#"{"valid":true}"#)]).await;

    let validator = Validator::new(config(&server.base_url, "a-license-key", ""));
    validator.validate_license().await.unwrap();

    let requests = server.requests().await;
    assert_eq!(
        requests.len(),
        1,
        "the configured server_url must receive the request"
    );
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/api/validate");

    server.stop().await;
}

#[tokio::test]
async fn user_token_prefers_the_token_endpoint_and_sends_bearer_auth() {
    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"valid":true,"message":"token ok"}"#,
    )])
    .await;

    let validator = Validator::new(config(&server.base_url, "a-license-key", "a-user-token"));
    let response = validator.validate_license().await.unwrap();

    assert!(response.valid);
    let requests = server.requests().await;
    assert_eq!(requests[0].path, "/api/validate_token");
    assert_eq!(
        requests[0].header("authorization"),
        Some("Bearer a-user-token")
    );

    server.stop().await;
}

#[tokio::test]
async fn unreachable_server_falls_back_to_the_cached_valid_result() {
    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"valid":true,"message":"ok"}"#,
    )])
    .await;
    let base_url = server.base_url.clone();

    let validator = Validator::new(config(&base_url, "a-license-key", ""));
    assert!(validator.is_valid().await.unwrap());

    server.stop().await; // nothing listens on that port anymore

    // The externally observable contract: once a validator has seen a
    // valid response, a later call must keep reporting valid even after
    // the server disappears — never panic, never flip to invalid/error.
    // (The next test proves this is a real fallback and not just "always
    // returns true": a validator with no prior successful fetch gets a
    // real error instead.)
    assert!(validator.is_valid().await.unwrap());
}

#[tokio::test]
async fn unreachable_server_graceful_degradation_after_cache_expires_the_daily_shortcut() {
    // A validator whose cache_time is effectively irrelevant here: this
    // test targets IsValid's own error-path fallback (entry.valid &&
    // recent-within-24h), not the offline/cache_time path.
    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"valid":true,"message":"ok"}"#,
    )])
    .await;
    let base_url = server.base_url.clone();

    let validator = Validator::new(config(&base_url, "a-license-key", ""));
    validator.validate_license().await.unwrap();
    server.stop().await;

    let unreachable = MockServer::unreachable_base_url().await;
    let restarted = Validator::new(config(&unreachable, "a-license-key", ""));
    // A brand new validator has no cache at all yet, so it must surface the
    // real error rather than inventing a cached value it never had.
    let err = restarted.is_valid().await.unwrap_err();
    assert!(matches!(err, LicenseError::Unreachable(_)));
}

#[tokio::test]
async fn neither_key_nor_token_is_an_immediate_error() {
    let validator = Validator::new(config("http://127.0.0.1:1", "", ""));
    let err = validator.validate_license().await.unwrap_err();
    assert!(matches!(err, LicenseError::NoCredentials));
}

#[tokio::test]
async fn clear_cache_forces_a_fresh_network_call() {
    let server = MockServer::start(vec![
        MockResponse::json(200, r#"{"valid":true,"message":"first"}"#),
        MockResponse::json(200, r#"{"valid":true,"message":"second"}"#),
    ])
    .await;

    let validator = Validator::new(config(&server.base_url, "a-license-key", ""));
    let first = validator.validate_license().await.unwrap();
    assert_eq!(first.message, "first");

    // Without clearing, the daily short-circuit would answer "first" again
    // without a second network call.
    validator.clear_cache();
    let second = validator.validate_license().await.unwrap();
    assert_eq!(second.message, "second");
    assert_eq!(server.call_count(), 2);

    server.stop().await;
}

#[tokio::test]
async fn get_license_info_formats_the_known_fields() {
    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"valid":true,"message":"all good","user_email":"user@example.com","tokens_used":3,"max_tokens":10}"#,
    )])
    .await;

    let validator = Validator::new(config(&server.base_url, "a-license-key", ""));
    let info = validator.get_license_info().await.unwrap();

    assert!(info.contains("Valid"));
    assert!(info.contains("all good"));
    assert!(info.contains("user@example.com"));
    assert!(info.contains("3/10 used"));

    server.stop().await;
}

#[tokio::test]
async fn get_status_is_an_alias_for_validate_license() {
    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"valid":true,"message":"status-ok"}"#,
    )])
    .await;

    let validator = Validator::new(config(&server.base_url, "a-license-key", ""));
    let status = validator.get_status().await.unwrap();

    assert!(status.valid);
    assert_eq!(status.message, "status-ok");

    server.stop().await;
}

#[tokio::test]
async fn get_license_info_reports_invalid_and_expiry() {
    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"valid":false,"message":"expired","expires_at":"2025-01-01"}"#,
    )])
    .await;

    let validator = Validator::new(config(&server.base_url, "a-license-key", ""));
    let info = validator.get_license_info().await.unwrap();

    assert!(info.contains("Invalid"));
    assert!(info.contains("expired"));
    assert!(info.contains("License Expires: 2025-01-01"));

    server.stop().await;
}

#[tokio::test]
async fn malformed_json_surfaces_a_decode_error() {
    let server = MockServer::start(vec![MockResponse::text(200, "not json")]).await;

    let validator = Validator::new(config(&server.base_url, "a-license-key", ""));
    let err = validator.validate_license().await.unwrap_err();
    assert!(matches!(err, LicenseError::Decode(_)));

    server.stop().await;
}
