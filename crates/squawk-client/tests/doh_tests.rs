//! Integration tests for [`DohClient`] against hand-rolled local mock
//! servers — no test ever contacts a real DoH provider, and no test binds
//! port 53 (every listener binds `127.0.0.1:0`, an ephemeral ports).

mod mock_http;

use mock_http::{MockResponse, MockServer};
use squawk_client::doh::{Config, DohClient};
use tokio_util::sync::CancellationToken;

fn config(server_urls: Vec<String>) -> Config {
    Config {
        server_urls,
        max_retries: 4,
        retry_delay: 0,
        ..Config::default()
    }
}

#[tokio::test]
async fn successful_query_parses() {
    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"Status":0,"Answer":[{"name":"example.com.","type":1,"TTL":300,"data":"192.0.2.1"}]}"#,
    )])
    .await;

    let client = DohClient::new(config(vec![format!("{}/dns-query", server.base_url)])).unwrap();
    let cancel = CancellationToken::new();
    let response = client.query(&cancel, "example.com", "A").await.unwrap();

    assert_eq!(response.status, 0);
    assert_eq!(response.answer.len(), 1);
    assert_eq!(response.answer[0].data, "192.0.2.1");
    assert_eq!(response.answer[0].kind.as_type_str(), "A");

    server.stop().await;
}

#[tokio::test]
async fn record_type_as_int_and_as_string_both_parse() {
    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"Status":0,"Answer":[
            {"name":"a.example.","type":1,"TTL":60,"data":"192.0.2.1"},
            {"name":"b.example.","type":"CNAME","TTL":60,"data":"target.example."}
        ]}"#,
    )])
    .await;

    let client = DohClient::new(config(vec![format!("{}/dns-query", server.base_url)])).unwrap();
    let cancel = CancellationToken::new();
    let response = client.query(&cancel, "example.com", "A").await.unwrap();

    assert_eq!(response.answer[0].kind.as_type_str(), "A");
    assert_eq!(response.answer[1].kind.as_type_str(), "CNAME");

    server.stop().await;
}

#[tokio::test]
async fn non_200_advances_to_the_next_server() {
    let failing = MockServer::start(vec![MockResponse::text(500, "internal error")]).await;
    let succeeding = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"Status":0,"Answer":[{"name":"example.com.","type":1,"TTL":60,"data":"192.0.2.9"}]}"#,
    )])
    .await;

    let client = DohClient::new(config(vec![
        format!("{}/dns-query", failing.base_url),
        format!("{}/dns-query", succeeding.base_url),
    ]))
    .unwrap();
    let cancel = CancellationToken::new();
    let response = client.query(&cancel, "example.com", "A").await.unwrap();

    assert_eq!(response.answer[0].data, "192.0.2.9");
    assert_eq!(failing.call_count(), 1);
    assert_eq!(succeeding.call_count(), 1);

    failing.stop().await;
    succeeding.stop().await;
}

#[tokio::test]
async fn all_servers_fail_surfaces_an_error_after_the_expected_retry_count() {
    let one = MockServer::start(vec![MockResponse::text(500, "down")]).await;
    let two = MockServer::start(vec![MockResponse::text(500, "down")]).await;

    let client = DohClient::new(config(vec![
        format!("{}/dns-query", one.base_url),
        format!("{}/dns-query", two.base_url),
    ]))
    .unwrap();
    let cancel = CancellationToken::new();
    let err = client.query(&cancel, "example.com", "A").await.unwrap_err();

    match err {
        squawk_client::doh::DohError::AllServersFailed { attempts, errors } => {
            assert_eq!(attempts, 4); // config() sets max_retries: 4
            assert_eq!(errors.len(), 4);
        }
        other => panic!("expected AllServersFailed, got {other:?}"),
    }

    one.stop().await;
    two.stop().await;
}

#[tokio::test]
async fn bearer_token_is_sent() {
    let server =
        MockServer::start(vec![MockResponse::json(200, r#"{"Status":0,"Answer":[]}"#)]).await;

    let mut cfg = config(vec![format!("{}/dns-query", server.base_url)]);
    cfg.auth_token = "secret-token".to_string();
    let client = DohClient::new(cfg).unwrap();
    let cancel = CancellationToken::new();
    client.query(&cancel, "example.com", "A").await.unwrap();

    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].header("authorization"),
        Some("Bearer secret-token")
    );
    assert!(requests[0].path.contains("name=example.com"));
    assert!(requests[0].path.contains("type=A"));

    server.stop().await;
}

#[tokio::test]
async fn no_auth_header_when_token_is_unset() {
    let server =
        MockServer::start(vec![MockResponse::json(200, r#"{"Status":0,"Answer":[]}"#)]).await;

    let client = DohClient::new(config(vec![format!("{}/dns-query", server.base_url)])).unwrap();
    let cancel = CancellationToken::new();
    client.query(&cancel, "example.com", "A").await.unwrap();

    let requests = server.requests().await;
    assert!(requests[0].header("authorization").is_none());

    server.stop().await;
}

#[tokio::test]
async fn nxdomain_status_is_preserved() {
    let server =
        MockServer::start(vec![MockResponse::json(200, r#"{"Status":3,"Answer":[]}"#)]).await;

    let client = DohClient::new(config(vec![format!("{}/dns-query", server.base_url)])).unwrap();
    let cancel = CancellationToken::new();
    let response = client
        .query(&cancel, "nonexistent.example", "A")
        .await
        .unwrap();
    assert_eq!(response.status, 3);

    server.stop().await;
}
