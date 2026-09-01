//! Integration tests for [`SkausWatchClient`] against a hand-rolled local
//! mock server — no test ever contacts a real SkausWatch Manager.
//!
//! Every assertion here is checked against
//! `~/code/skauswatch/services/manager/src/routes/endpoint.rs` (the real
//! Manager handler) rather than a fabricated contract — see `crate::model`
//! doc comments for the exact lines each shape mirrors.

mod mock_server;

use skauswatch_client::{ClientConfig, ClientError, EndpointEvent, Severity, SkausWatchClient};

/// The steady-state check-in path: an already-provisioned `agent_id`,
/// `enrollment_token` left `None`. The real Manager ignores
/// `enrollment_token` on this branch even if present (see
/// `RegisterRequest`'s doc) — this test proves the client doesn't send the
/// field at all when it has none to send.
#[tokio::test]
async fn register_omits_enrollment_token_when_none_configured() {
    let server = mock_server::start_register_ok("agent-42", "Agent re-registered", "active").await;
    let cfg = ClientConfig::new(
        server.base_url(),
        "agent-42".to_string(),
        "static-key".to_string(),
        None,
    );
    let client = SkausWatchClient::new(cfg).expect("client builds");
    let resp = client.register().await.expect("register ok");

    // No api_key in the response -- it's provisioned out-of-band, never
    // issued by this call (RegisterResponse has no such field at all).
    assert_eq!(resp.agent_id, "agent-42");
    assert_eq!(resp.status, "active");
    assert_eq!(resp.message, "Agent re-registered");

    assert_eq!(server.last_path(), "/api/v1/endpoint/register");
    assert_eq!(server.last_method().as_deref(), Some("POST"));
    // The body's own agent_id field carries the identity -- not just the
    // x-agent-id header.
    assert!(server.last_body_contains(r#""agent_id":"agent-42""#));
    // No enrollment_token is sent when ClientConfig carries none.
    assert!(!server.last_body_contains("enrollment_token"));
    assert_eq!(
        server.last_header("x-agent-id").as_deref(),
        Some("agent-42")
    );
    assert_eq!(
        server.last_header("x-api-key").as_deref(),
        Some("static-key")
    );

    server.stop().await;
}

/// The first-contact enrollment path: a brand-new `agent_id` whose tenant
/// the real Manager resolves from `enrollment_token`
/// (`RegisterBody::enrollment_token`, endpoint.rs ~line 315-328;
/// `register_agent`'s branch split ~line 461-471). Proves the client sends
/// the field verbatim in the request body when `ClientConfig` carries one.
#[tokio::test]
async fn register_includes_enrollment_token_when_configured() {
    let server = mock_server::start_register_ok("agent-99", "Agent registered", "active").await;
    let cfg = ClientConfig::new(
        server.base_url(),
        "agent-99".to_string(),
        "static-key".to_string(),
        Some("enroll-tok-abc".to_string()),
    );
    let client = SkausWatchClient::new(cfg).expect("client builds");
    client.register().await.expect("register ok");

    assert!(server.last_body_contains(r#""enrollment_token":"enroll-tok-abc""#));

    server.stop().await;
}

#[tokio::test]
async fn register_returns_http_error_on_non_2xx_response() {
    let server = mock_server::start_error(500).await;
    let cfg = ClientConfig::new(
        server.base_url(),
        "agent-x".to_string(),
        "key-x".to_string(),
        None,
    );
    let client = SkausWatchClient::new(cfg).expect("client builds");
    let err = client
        .register()
        .await
        .expect_err("register should fail on a non-2xx response");
    match err {
        ClientError::Http { status } => assert_eq!(status, 500),
        other => panic!("expected ClientError::Http, got {other:?}"),
    }

    server.stop().await;
}

#[tokio::test]
async fn heartbeat_sends_the_static_api_key_verbatim_and_correct_body() {
    let server = mock_server::start_auth_echo().await; // 200 iff x-agent-id + x-api-key present
    let client = mock_server::client_for(&server, "agent-9", "provisioned-key-9");

    client.heartbeat("active").await.expect("heartbeat ok");

    assert_eq!(server.last_path(), "/api/v1/endpoint/heartbeat");
    assert_eq!(server.last_method().as_deref(), Some("POST"));
    assert_eq!(server.last_header("x-agent-id").as_deref(), Some("agent-9"));
    // The api_key is sent verbatim -- not a computed hash of anything.
    assert_eq!(
        server.last_header("x-api-key").as_deref(),
        Some("provisioned-key-9")
    );
    assert!(server.last_body_contains(r#""agent_id":"agent-9""#));
    assert!(server.last_body_contains(r#""status":"active""#));

    server.stop().await;
}

#[tokio::test]
async fn heartbeat_returns_http_error_on_non_2xx_response() {
    let server = mock_server::start_error(500).await;
    let client = mock_server::client_for(&server, "agent-99", "k99");
    let err = client
        .heartbeat("active")
        .await
        .expect_err("heartbeat should fail on a non-2xx response");
    match err {
        ClientError::Http { status } => assert_eq!(status, 500),
        other => panic!("expected ClientError::Http, got {other:?}"),
    }

    server.stop().await;
}

/// An unregistered `agent_id` gets 404 from the real `heartbeat` handler
/// (endpoint.rs ~line 561-568) -- proves that maps through the same
/// `ClientError::Http` path as any other non-2xx status, not a special
/// case.
#[tokio::test]
async fn heartbeat_returns_http_error_404_for_an_unregistered_agent() {
    let server = mock_server::start_error(404).await;
    let client = mock_server::client_for(&server, "never-registered", "k");
    let err = client
        .heartbeat("active")
        .await
        .expect_err("heartbeat should fail for an unregistered agent");
    match err {
        ClientError::Http { status } => assert_eq!(status, 404),
        other => panic!("expected ClientError::Http, got {other:?}"),
    }

    server.stop().await;
}

#[tokio::test]
async fn report_events_sends_a_bare_array_with_agent_id_on_each_event() {
    let server = mock_server::start_auth_echo().await;
    let client = mock_server::client_for(&server, "agent-11", "k11");

    let events = vec![EndpointEvent {
        agent_id: "agent-11".to_string(),
        event_type: "module_fault".to_string(),
        severity: Some(Severity::Critical),
        process_name: None,
        process_path: None,
        process_hash: None,
        parent_process: None,
        command_line: None,
        network_connections: None,
        file_operations: None,
        registry_operations: None,
        details: Some(serde_json::json!({"module": "sp1"})),
    }];
    client
        .report_events(&events)
        .await
        .expect("report_events ok");

    assert_eq!(server.last_path(), "/api/v1/endpoint/events");
    assert_eq!(server.last_method().as_deref(), Some("POST"));
    assert_eq!(
        server.last_header("x-agent-id").as_deref(),
        Some("agent-11")
    );
    assert_eq!(server.last_header("x-api-key").as_deref(), Some("k11"));

    // Each event carries its own agent_id -- there is no top-level
    // {"agent_id": ..., "events": [...]} wrapper on the real wire.
    assert!(server.last_body_contains(r#""agent_id":"agent-11""#));
    assert!(server.last_body_contains(r#""event_type":"module_fault""#));
    assert!(server.last_body_contains(r#""severity":"critical""#));
    assert!(server.last_body_contains("module_fault"));
    assert!(
        !server.last_body_contains(r#""events":["#),
        "must not wrap the batch in an {{agent_id, events: [...]}} envelope"
    );
    // A bare JSON array, not a single object.
    assert!(server.last_body_contains(r#"[{"agent_id""#));

    server.stop().await;
}

#[tokio::test]
async fn report_events_returns_http_error_on_non_2xx_response() {
    let server = mock_server::start_error(500).await;
    let client = mock_server::client_for(&server, "agent-77", "k77");
    let events = vec![EndpointEvent {
        agent_id: "agent-77".to_string(),
        event_type: "module_fault".to_string(),
        severity: None,
        process_name: None,
        process_path: None,
        process_hash: None,
        parent_process: None,
        command_line: None,
        network_connections: None,
        file_operations: None,
        registry_operations: None,
        details: None,
    }];
    let err = client
        .report_events(&events)
        .await
        .expect_err("report_events should fail on a non-2xx response");
    match err {
        ClientError::Http { status } => assert_eq!(status, 500),
        other => panic!("expected ClientError::Http, got {other:?}"),
    }

    server.stop().await;
}

#[tokio::test]
async fn fetch_config_sends_static_headers_and_parses_the_nested_config() {
    let server = mock_server::start_auth_echo().await;
    let client = mock_server::client_for(&server, "agent-13", "k13");

    let config = client.fetch_config().await.expect("fetch_config ok");

    // NOT a top-level heartbeat_secs -- nested under `config`.
    assert_eq!(config.config.heartbeat_interval, serde_json::json!(30));
    assert_eq!(config.config.reporting_interval, serde_json::json!(60));

    assert_eq!(server.last_path(), "/api/v1/endpoint/config");
    assert_eq!(server.last_method().as_deref(), Some("GET"));
    assert_eq!(
        server.last_header("x-agent-id").as_deref(),
        Some("agent-13")
    );
    assert_eq!(server.last_header("x-api-key").as_deref(), Some("k13"));

    server.stop().await;
}

#[tokio::test]
async fn fetch_config_returns_http_error_on_non_2xx_response() {
    let server = mock_server::start_error(404).await;
    let client = mock_server::client_for(&server, "agent-14", "k14");
    let err = client
        .fetch_config()
        .await
        .expect_err("fetch_config should fail on a non-2xx response");
    match err {
        ClientError::Http { status } => assert_eq!(status, 404),
        other => panic!("expected ClientError::Http, got {other:?}"),
    }

    server.stop().await;
}

/// `Severity` must serialize to the exact lowercase strings the Manager's
/// `THREAT_LEVELS` accepts (endpoint.rs ~line 33) -- not Rust's default
/// PascalCase variant names.
#[test]
fn severity_serializes_to_the_exact_lowercase_threat_level_strings() {
    assert_eq!(
        serde_json::to_string(&Severity::Critical).unwrap(),
        "\"critical\""
    );
    assert_eq!(serde_json::to_string(&Severity::High).unwrap(), "\"high\"");
    assert_eq!(
        serde_json::to_string(&Severity::Medium).unwrap(),
        "\"medium\""
    );
    assert_eq!(serde_json::to_string(&Severity::Low).unwrap(), "\"low\"");
    assert_eq!(serde_json::to_string(&Severity::Info).unwrap(), "\"info\"");
}
