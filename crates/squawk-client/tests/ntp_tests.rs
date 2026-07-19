//! Integration tests for [`NtpClient`] against hand-rolled local UDP
//! servers — no test ever contacts a real NTP pool, and no test binds a
//! privileged port (every listener binds `127.0.0.1:0`).
//!
//! The offset/round-trip **formula** itself is tested at the unit level
//! inside `src/ntp.rs` (against synthetic timestamps, no network at all);
//! these tests instead prove the client can drive one real UDP round trip,
//! fail over, and time out.

use std::net::SocketAddr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use squawk_client::ntp::{ClientConfig, NtpClient, NtpError};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

const NTP_EPOCH_OFFSET: i64 = 2_208_988_800;

/// A minimal fake NTP server: replies to every 48-byte request with a
/// stratum-2 response whose receive/transmit timestamps are both "now" —
/// enough to prove the client parses a real response end to end without
/// asserting an exact offset (that's the unit tests' job).
async fn spawn_fake_ntp_server() -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = socket.local_addr().unwrap();

    tokio::spawn(async move {
        let mut buf = [0u8; 48];
        loop {
            let Ok((len, peer)) = socket.recv_from(&mut buf).await else {
                return;
            };
            if len < 48 {
                continue;
            }

            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            let ntp_secs = (now.as_secs() as i64 + NTP_EPOCH_OFFSET) as u32;
            let ntp_frac = ((now.subsec_nanos() as u64) << 32) / 1_000_000_000;

            let mut response = [0u8; 48];
            response[1] = 2; // stratum
            response[32..36].copy_from_slice(&ntp_secs.to_be_bytes()); // rx sec
            response[36..40].copy_from_slice(&(ntp_frac as u32).to_be_bytes()); // rx frac
            response[40..44].copy_from_slice(&ntp_secs.to_be_bytes()); // tx sec
            response[44..48].copy_from_slice(&(ntp_frac as u32).to_be_bytes()); // tx frac

            let _ = socket.send_to(&response, peer).await;
        }
    });

    addr
}

/// A UDP socket that receives a query and never answers it — models a
/// server that drops the request, for the timeout test.
async fn spawn_black_hole_udp_server() -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = socket.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 48];
        let _ = socket.recv_from(&mut buf).await;
        tokio::time::sleep(Duration::from_secs(10)).await;
    });
    addr
}

#[tokio::test]
async fn successful_query_against_a_real_udp_round_trip() {
    let addr = spawn_fake_ntp_server().await;
    let client = NtpClient::new(ClientConfig {
        server_urls: vec![addr.to_string()],
        timeout: 2,
        max_retries: 1,
        retry_delay: 0,
    });

    let cancel = CancellationToken::new();
    let response = client.query(&cancel).await.unwrap();

    assert_eq!(response.stratum, 2);
    // The fake server echoed back "now" as both rx and tx timestamps, so
    // the computed offset should be small (well under a second) rather
    // than wildly wrong — proves the encode/decode round trip is sane
    // without duplicating the exact-formula assertion the unit tests own.
    assert!(
        response.offset_nanos.abs() < Duration::from_secs(2).as_nanos() as i64,
        "offset {}ns is implausibly large for a same-host round trip",
        response.offset_nanos
    );

    let (synchronized, _, last_sync) = client.status();
    assert!(synchronized);
    assert!(last_sync.is_some());
}

#[tokio::test]
async fn round_robin_advances_past_a_dead_server() {
    let dead = spawn_black_hole_udp_server().await;
    let alive = spawn_fake_ntp_server().await;

    let client = NtpClient::new(ClientConfig {
        server_urls: vec![dead.to_string(), alive.to_string()],
        timeout: 1,
        max_retries: 2,
        retry_delay: 0,
    });

    let cancel = CancellationToken::new();
    let response = client.query(&cancel).await.unwrap();
    assert_eq!(response.stratum, 2);
}

#[tokio::test]
async fn timeout_path_returns_promptly_with_a_timeout_error() {
    let addr = spawn_black_hole_udp_server().await;
    let client = NtpClient::new(ClientConfig {
        server_urls: vec![addr.to_string()],
        timeout: 1,
        max_retries: 1,
        retry_delay: 0,
    });

    let cancel = CancellationToken::new();
    let started = Instant::now();
    let err = client.query(&cancel).await.unwrap_err();
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(3),
        "timeout path took {elapsed:?}, expected close to the 1s configured timeout"
    );
    match err {
        NtpError::AllServersFailed { attempts, errors } => {
            assert_eq!(attempts, 1);
            assert!(errors[0].contains("timed out"));
        }
        other => panic!("expected AllServersFailed wrapping a timeout, got {other:?}"),
    }

    let (synchronized, _, _) = client.status();
    assert!(!synchronized);
}
