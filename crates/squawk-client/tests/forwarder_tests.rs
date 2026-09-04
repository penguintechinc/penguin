//! Integration tests for [`Forwarder`] against a mock DoH JSON server and
//! real (ephemeral-port) UDP/TCP sockets — no test binds `:53`, and no test
//! ever contacts a real DNS-over-HTTPS provider.
//!
//! `start_returns_promptly` is the regression test for the Go deadlock
//! documented in `docs/PARITY.md` §1.15: Go's `Start(ctx)` blocks until the
//! context is cancelled, which deadlocks a caller (like the squawk module)
//! that invokes it synchronously under a mutex. This port's `start()` binds
//! synchronously and then returns, never blocking on serving.

mod mock_http;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use hickory_proto::op::{Message, Query, ResponseCode};
use hickory_proto::rr::{Name, RData, RecordType};
use mock_http::{MockResponse, MockServer};
use squawk_client::doh::{Config as DohConfig, DohClient};
use squawk_client::forwarder::{CacheConfig, Config as ForwarderConfig, Forwarder};
use tokio::net::UdpSocket;

/// A forwarder bound to ephemeral UDP-only ports, backed by a `DohClient`
/// pointed at `doh_base_url`.
fn build_forwarder(doh_base_url: &str, doh_max_retries: usize) -> Forwarder {
    let doh_config = DohConfig {
        server_urls: vec![format!("{doh_base_url}/dns-query")],
        max_retries: doh_max_retries,
        retry_delay: 1, // never hit: every test here uses max_retries: 1
        ..DohConfig::default()
    };
    let doh = Arc::new(DohClient::new(doh_config).unwrap());

    let forwarder_config = ForwarderConfig {
        udp_address: "127.0.0.1:0".to_string(),
        tcp_address: "127.0.0.1:0".to_string(),
        listen_udp: true,
        listen_tcp: false,
    };
    Forwarder::new(doh, forwarder_config, CacheConfig::default())
}

fn build_query(name: &str, rtype: RecordType) -> Vec<u8> {
    let mut msg = Message::query();
    msg.metadata.recursion_desired = true;
    msg.add_query(Query::query(Name::from_ascii(name).unwrap(), rtype));
    msg.to_vec().unwrap()
}

async fn send_and_receive(target: SocketAddr, request: &[u8]) -> Message {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket.send_to(request, target).await.unwrap();
    let mut buf = [0u8; 4096];
    let (len, _) = tokio::time::timeout(Duration::from_secs(5), socket.recv_from(&mut buf))
        .await
        .expect("forwarder did not respond in time")
        .unwrap();
    Message::from_vec(&buf[..len]).unwrap()
}

#[tokio::test]
async fn start_returns_promptly() {
    let server =
        MockServer::start(vec![MockResponse::json(200, r#"{"Status":0,"Answer":[]}"#)]).await;
    let forwarder = build_forwarder(&server.base_url, 1);

    let result = tokio::time::timeout(Duration::from_millis(500), forwarder.start()).await;
    assert!(
        result.is_ok(),
        "start() must return well under 500ms — a hang here is exactly the Go deadlock this port fixes"
    );
    result.unwrap().unwrap();

    assert!(forwarder.is_running());
    forwarder.stop().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn start_on_an_already_bound_port_fails_immediately() {
    let reserved = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = reserved.local_addr().unwrap();

    let doh_config = DohConfig {
        server_urls: vec!["https://127.0.0.1:1/dns-query".to_string()],
        max_retries: 1,
        ..DohConfig::default()
    };
    let doh = Arc::new(DohClient::new(doh_config).unwrap());
    let forwarder_config = ForwarderConfig {
        udp_address: addr.to_string(),
        tcp_address: "127.0.0.1:0".to_string(),
        listen_udp: true,
        listen_tcp: false,
    };
    let forwarder = Forwarder::new(doh, forwarder_config, CacheConfig::default());

    let result = tokio::time::timeout(Duration::from_millis(500), forwarder.start()).await;
    assert!(
        result.is_ok(),
        "a bind failure must surface immediately, not hang"
    );
    assert!(
        result.unwrap().is_err(),
        "binding an already-used port must fail"
    );

    drop(reserved);
}

#[tokio::test]
async fn query_is_served_end_to_end_against_a_mock_doh_server() {
    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"Status":0,"Answer":[{"name":"example.com.","type":1,"TTL":300,"data":"192.0.2.42"}]}"#,
    )])
    .await;
    let forwarder = build_forwarder(&server.base_url, 1);
    forwarder.start().await.unwrap();
    let addr = forwarder.local_udp_addr().unwrap();

    let request = build_query("example.com.", RecordType::A);
    let response = send_and_receive(addr, &request).await;

    assert_eq!(response.metadata.response_code, ResponseCode::NoError);
    assert_eq!(response.answers.len(), 1);
    let RData::A(ip) = &response.answers[0].data else {
        panic!("expected an A record, got {:?}", response.answers[0].data);
    };
    assert_eq!(ip.0.to_string(), "192.0.2.42");

    forwarder.stop().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn servfail_mapping() {
    let server = MockServer::start(vec![MockResponse::text(500, "upstream down")]).await;
    let forwarder = build_forwarder(&server.base_url, 1);
    forwarder.start().await.unwrap();
    let addr = forwarder.local_udp_addr().unwrap();

    let request = build_query("example.com.", RecordType::A);
    let response = send_and_receive(addr, &request).await;

    assert_eq!(response.metadata.response_code, ResponseCode::ServFail);

    forwarder.stop().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn nxdomain_mapping() {
    let server =
        MockServer::start(vec![MockResponse::json(200, r#"{"Status":3,"Answer":[]}"#)]).await;
    let forwarder = build_forwarder(&server.base_url, 1);
    forwarder.start().await.unwrap();
    let addr = forwarder.local_udp_addr().unwrap();

    let request = build_query("nonexistent.example.", RecordType::A);
    let response = send_and_receive(addr, &request).await;

    assert_eq!(response.metadata.response_code, ResponseCode::NXDomain);

    forwarder.stop().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn mx_priority_round_trips() {
    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"Status":0,"Answer":[{"name":"example.com.","type":15,"TTL":300,"data":"10 mail.example.com."}]}"#,
    )])
    .await;
    let forwarder = build_forwarder(&server.base_url, 1);
    forwarder.start().await.unwrap();
    let addr = forwarder.local_udp_addr().unwrap();

    let request = build_query("example.com.", RecordType::MX);
    let response = send_and_receive(addr, &request).await;

    assert_eq!(response.answers.len(), 1);
    let RData::MX(mx) = &response.answers[0].data else {
        panic!("expected an MX record, got {:?}", response.answers[0].data);
    };
    assert_eq!(
        mx.preference, 10,
        "Go's forwarder drops MX priority — this port must not"
    );
    assert_eq!(mx.exchange.to_ascii(), "mail.example.com.");

    forwarder.stop().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn unsupported_query_type_returns_notimp() {
    let server =
        MockServer::start(vec![MockResponse::json(200, r#"{"Status":0,"Answer":[]}"#)]).await;
    let forwarder = build_forwarder(&server.base_url, 1);
    forwarder.start().await.unwrap();
    let addr = forwarder.local_udp_addr().unwrap();

    let request = build_query("example.com.", RecordType::SOA);
    let response = send_and_receive(addr, &request).await;

    assert_eq!(response.metadata.response_code, ResponseCode::NotImp);

    forwarder.stop().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn stop_terminates_the_tasks_and_the_port_is_released() {
    let server =
        MockServer::start(vec![MockResponse::json(200, r#"{"Status":0,"Answer":[]}"#)]).await;
    let forwarder = build_forwarder(&server.base_url, 1);
    forwarder.start().await.unwrap();
    let addr = forwarder.local_udp_addr().unwrap();

    forwarder.stop().await.unwrap();
    assert!(!forwarder.is_running());

    // The port must be free immediately — no lingering listener holding it.
    let rebind = UdpSocket::bind(addr).await;
    assert!(rebind.is_ok(), "port {addr} was not released after stop()");

    server.stop().await;
}

#[tokio::test]
async fn stop_without_a_prior_start_is_an_error() {
    let server =
        MockServer::start(vec![MockResponse::json(200, r#"{"Status":0,"Answer":[]}"#)]).await;
    let forwarder = build_forwarder(&server.base_url, 1);
    assert!(forwarder.stop().await.is_err());
    server.stop().await;
}

#[tokio::test]
async fn tcp_serves_a_length_prefixed_query_end_to_end() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"Status":0,"Answer":[{"name":"example.com.","type":1,"TTL":300,"data":"192.0.2.99"}]}"#,
    )])
    .await;

    let doh_config = DohConfig {
        server_urls: vec![format!("{}/dns-query", server.base_url)],
        max_retries: 1,
        ..DohConfig::default()
    };
    let doh = Arc::new(DohClient::new(doh_config).unwrap());
    let forwarder_config = ForwarderConfig {
        udp_address: "127.0.0.1:0".to_string(),
        tcp_address: "127.0.0.1:0".to_string(),
        listen_udp: false,
        listen_tcp: true,
    };
    let forwarder = Forwarder::new(doh, forwarder_config, CacheConfig::default());
    forwarder.start().await.unwrap();
    let addr = forwarder.local_tcp_addr().unwrap();

    let request = build_query("example.com.", RecordType::A);
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let len_prefix = (request.len() as u16).to_be_bytes();
    stream.write_all(&len_prefix).await.unwrap();
    stream.write_all(&request).await.unwrap();

    let mut response_len = [0u8; 2];
    stream.read_exact(&mut response_len).await.unwrap();
    let mut response_buf = vec![0u8; u16::from_be_bytes(response_len) as usize];
    stream.read_exact(&mut response_buf).await.unwrap();
    let response = Message::from_vec(&response_buf).unwrap();

    assert_eq!(response.metadata.response_code, ResponseCode::NoError);
    assert_eq!(response.answers.len(), 1);
    let RData::A(ip) = &response.answers[0].data else {
        panic!("expected an A record, got {:?}", response.answers[0].data);
    };
    assert_eq!(ip.0.to_string(), "192.0.2.99");

    drop(stream);
    forwarder.stop().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn cache_hit_avoids_a_second_upstream_call() {
    let server = MockServer::start(vec![MockResponse::json(
        200,
        r#"{"Status":0,"Answer":[{"name":"example.com.","type":1,"TTL":300,"data":"192.0.2.7"}]}"#,
    )])
    .await;
    let forwarder = build_forwarder(&server.base_url, 1);
    forwarder.start().await.unwrap();
    let addr = forwarder.local_udp_addr().unwrap();

    let request = build_query("example.com.", RecordType::A);
    send_and_receive(addr, &request).await;
    send_and_receive(addr, &request).await;

    assert_eq!(
        server.call_count(),
        1,
        "second query should be served from cache"
    );
    let stats = forwarder.cache().stats();
    assert_eq!(stats.hits, 1);
    assert_eq!(stats.misses, 1);

    forwarder.stop().await.unwrap();
    server.stop().await;
}
