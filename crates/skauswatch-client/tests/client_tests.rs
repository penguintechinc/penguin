//! Integration tests for [`SkausWatchClient`] against a hand-rolled local
//! mock server — no test ever contacts a real SkausWatch Manager.

mod mock_server;

use skauswatch_client::{
    AgentIdentity, ClientConfig, ClientError, EndpointEvent, HeartbeatBody, SkausWatchClient,
};

#[tokio::test]
async fn register_posts_enrollment_token_and_returns_identity() {
    let server = mock_server::start_register_ok("agent-42", "key-abc").await;
    let cfg = ClientConfig {
        base_url: server.base_url(),
        enrollment_token: "enr-tok".to_string(),
    };
    let client = SkausWatchClient::new(cfg).expect("client builds");
    let id = client.register().await.expect("register ok");
    assert_eq!(id.agent_id, "agent-42");
    assert_eq!(id.api_key, "key-abc");
    assert_eq!(server.last_path(), "/api/v1/endpoint/register");
    assert_eq!(server.last_method().as_deref(), Some("POST"));
    assert!(server.last_body_contains("enr-tok"));

    server.stop().await;
}

#[tokio::test]
async fn register_returns_http_error_on_non_2xx_response() {
    let server = mock_server::start_error(500).await;
    let cfg = ClientConfig {
        base_url: server.base_url(),
        enrollment_token: "enr-tok".to_string(),
    };
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
async fn heartbeat_sends_hmac_headers() {
    let server = mock_server::start_auth_echo().await; // 200 iff x-agent-id + valid x-api-key present
    let client = mock_server::client_for(&server, "agent-9", "k9");
    let id = AgentIdentity {
        agent_id: "agent-9".into(),
        api_key: "k9".into(),
    };
    let body = HeartbeatBody {
        healthy: true,
        module_version: "0.2.0".into(),
    };
    client.heartbeat(&id, &body).await.expect("heartbeat ok");
    assert_eq!(server.last_path(), "/api/v1/endpoint/heartbeat");
    assert_eq!(server.last_method().as_deref(), Some("POST"));
    assert_eq!(server.last_header("x-agent-id").as_deref(), Some("agent-9"));
    assert_eq!(server.last_header("x-api-key").map(|s| s.len()), Some(64));

    server.stop().await;
}

#[tokio::test]
async fn heartbeat_returns_http_error_on_non_2xx_response() {
    let server = mock_server::start_error(500).await;
    let client = mock_server::client_for(&server, "agent-99", "k99");
    let id = AgentIdentity {
        agent_id: "agent-99".into(),
        api_key: "k99".into(),
    };
    let body = HeartbeatBody {
        healthy: true,
        module_version: "0.2.0".into(),
    };
    let err = client
        .heartbeat(&id, &body)
        .await
        .expect_err("heartbeat should fail on a non-2xx response");
    match err {
        ClientError::Http { status } => assert_eq!(status, 500),
        other => panic!("expected ClientError::Http, got {other:?}"),
    }

    server.stop().await;
}

#[tokio::test]
async fn report_events_sends_hmac_headers_and_serialized_events() {
    let server = mock_server::start_auth_echo().await;
    let client = mock_server::client_for(&server, "agent-11", "k11");
    let id = AgentIdentity {
        agent_id: "agent-11".into(),
        api_key: "k11".into(),
    };
    let events = vec![EndpointEvent {
        kind: "module_fault".into(),
        severity: "critical".into(),
        detail: serde_json::json!({"module": "sp1"}),
        ts_unix: 1_724_000_000,
    }];
    client
        .report_events(&id, &events)
        .await
        .expect("report_events ok");
    assert_eq!(server.last_path(), "/api/v1/endpoint/events");
    assert_eq!(server.last_method().as_deref(), Some("POST"));
    assert_eq!(
        server.last_header("x-agent-id").as_deref(),
        Some("agent-11")
    );
    assert_eq!(server.last_header("x-api-key").map(|s| s.len()), Some(64));
    assert!(server.last_body_contains("module_fault"));

    server.stop().await;
}

#[tokio::test]
async fn fetch_config_signs_empty_body_and_parses_response() {
    let server = mock_server::start_auth_echo().await;
    let client = mock_server::client_for(&server, "agent-13", "k13");
    let id = AgentIdentity {
        agent_id: "agent-13".into(),
        api_key: "k13".into(),
    };
    let config = client.fetch_config(&id).await.expect("fetch_config ok");
    assert_eq!(config.heartbeat_secs, 30);
    assert_eq!(server.last_path(), "/api/v1/endpoint/config");
    assert_eq!(server.last_method().as_deref(), Some("GET"));
    assert_eq!(
        server.last_header("x-agent-id").as_deref(),
        Some("agent-13")
    );
    assert_eq!(server.last_header("x-api-key").map(|s| s.len()), Some(64));

    server.stop().await;
}
