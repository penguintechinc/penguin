//! Integration tests for [`WaddlebotClient`]'s error handling: the four
//! [`WaddlebotError`] variants, and each of the three error-body shapes the
//! hub actually returns across its controllers (see `error.rs`'s module
//! doc comment for where each shape comes from).

mod mock_http;

use mock_http::{MockResponse, MockServer};
use waddlebot_client::{Config, ErrorBody, WaddlebotClient, WaddlebotError};

fn config(base_url: &str) -> Config {
    Config {
        base_url: base_url.to_string(),
        community_id: 1,
        cat: "wdl_c_test".to_string(),
        ..Config::default()
    }
}

#[tokio::test]
async fn a_401_response_is_the_auth_variant_with_the_structured_body() {
    // The global error handler's shape — what `errors.unauthorized()`
    // produces via `middleware/errorHandler.js`.
    let server = MockServer::start(vec![MockResponse::json(
        401,
        r#"{"success":false,"error":{"code":"UNAUTHORIZED","message":"Authentication required"}}"#,
    )])
    .await;

    let client = WaddlebotClient::new(config(&server.base_url)).unwrap();
    let err = client.list_my_communities().await.unwrap_err();

    let WaddlebotError::Auth { status, body } = err else {
        panic!("expected WaddlebotError::Auth, got {err:?}");
    };
    assert_eq!(status, 401);
    assert_eq!(
        body,
        ErrorBody::Structured {
            code: "UNAUTHORIZED".to_string(),
            message: "Authentication required".to_string(),
        }
    );

    server.stop().await;
}

#[tokio::test]
async fn a_403_response_is_the_auth_variant_with_the_plain_string_body() {
    // requireScope's inline shape — `middleware/auth.js`:
    // `res.status(403).json({ error: 'Missing scope: ...' })`.
    let server = MockServer::start(vec![MockResponse::json(
        403,
        r#"{"error":"Missing scope: music:write"}"#,
    )])
    .await;

    let client = WaddlebotClient::new(config(&server.base_url)).unwrap();
    let err = client.list_my_communities().await.unwrap_err();

    let WaddlebotError::Auth { status, body } = err else {
        panic!("expected WaddlebotError::Auth, got {err:?}");
    };
    assert_eq!(status, 403);
    assert_eq!(
        body,
        ErrorBody::Plain("Missing scope: music:write".to_string())
    );

    server.stop().await;
}

#[tokio::test]
async fn a_success_false_plain_string_body_decodes_as_plain() {
    // musicController.js's shape: `res.status(500).json({ success: false,
    // error: 'Failed to get music settings' })`.
    let server = MockServer::start(vec![MockResponse::json(
        500,
        r#"{"success":false,"error":"Failed to get music settings"}"#,
    )])
    .await;

    let client = WaddlebotClient::new(config(&server.base_url)).unwrap();
    let err = client.get_music_settings().await.unwrap_err();

    let WaddlebotError::Status { status, body } = err else {
        panic!("expected WaddlebotError::Status, got {err:?}");
    };
    assert_eq!(status, 500);
    assert_eq!(
        body,
        ErrorBody::Plain("Failed to get music settings".to_string())
    );

    server.stop().await;
}

#[tokio::test]
async fn a_non_json_non_2xx_body_is_status_with_an_unparsed_body() {
    let server = MockServer::start(vec![MockResponse::text(502, "<html>Bad Gateway</html>")]).await;

    let client = WaddlebotClient::new(config(&server.base_url)).unwrap();
    let err = client.list_my_communities().await.unwrap_err();

    let WaddlebotError::Status { status, body } = err else {
        panic!("expected WaddlebotError::Status, got {err:?}");
    };
    assert_eq!(status, 502);
    assert_eq!(
        body,
        ErrorBody::Unparsed("<html>Bad Gateway</html>".to_string())
    );

    server.stop().await;
}

#[tokio::test]
async fn a_2xx_response_with_a_malformed_body_is_the_decode_variant() {
    let server = MockServer::start(vec![MockResponse::text(200, "not json at all")]).await;

    let client = WaddlebotClient::new(config(&server.base_url)).unwrap();
    let err = client.list_my_communities().await.unwrap_err();

    assert!(
        matches!(err, WaddlebotError::Decode(_)),
        "expected WaddlebotError::Decode, got {err:?}"
    );

    server.stop().await;
}

#[tokio::test]
async fn a_2xx_response_missing_expected_fields_is_also_the_decode_variant() {
    // Valid JSON, wrong shape — `communities` is missing where the client
    // expects an array (or, here, is the wrong type entirely).
    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"success":true,"communities":"not-an-array"}"#,
    )])
    .await;

    let client = WaddlebotClient::new(config(&server.base_url)).unwrap();
    let err = client.list_my_communities().await.unwrap_err();

    assert!(matches!(err, WaddlebotError::Decode(_)));

    server.stop().await;
}

#[tokio::test]
async fn an_unreachable_server_is_the_transport_variant() {
    let unreachable_base_url = MockServer::unreachable_base_url().await;

    let client = WaddlebotClient::new(config(&unreachable_base_url)).unwrap();
    let err = client.list_my_communities().await.unwrap_err();

    assert!(
        matches!(err, WaddlebotError::Transport(_)),
        "expected WaddlebotError::Transport, got {err:?}"
    );
}

#[tokio::test]
async fn a_404_is_status_not_auth() {
    // Confirms 401/403 are the only statuses routed to Auth — anything
    // else (404, 409, 500, ...) is Status even though it's still an error
    // body in one of the same three shapes.
    let server = MockServer::start(vec![MockResponse::json(
        404,
        r#"{"error":"Token not found"}"#,
    )])
    .await;

    let client = WaddlebotClient::new(config(&server.base_url)).unwrap();
    let err = client.revoke_cat(1).await.unwrap_err();

    assert!(matches!(err, WaddlebotError::Status { status: 404, .. }));

    server.stop().await;
}
