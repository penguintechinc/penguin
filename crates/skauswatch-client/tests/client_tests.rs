//! Integration tests for [`SkausWatchClient`] against a hand-rolled local
//! mock server — no test ever contacts a real SkausWatch Manager.

mod mock_server;

use skauswatch_client::{ClientConfig, SkausWatchClient};

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
    assert!(server.last_body_contains("enr-tok"));

    server.stop().await;
}
