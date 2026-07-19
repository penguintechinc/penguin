//! Port of `pkg/time/ntp_client.go` only — the self-contained plain-UDP
//! SNTP client. The NTS/interceptor stack under `pkg/ntp/*` and
//! `pkg/time/forwarder.go`'s actual NTP-forwarding logic are out of scope
//! (the penguin agent never used them); only [`ForwarderConfig`]'s struct
//! shape is ported here, since [`crate::config`] needs it to exist as a
//! field type. This client is what makes the squawk module's `time`
//! command real instead of a hardcoded stub.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

const NTP_PACKET_SIZE: usize = 48;
const NTP_VERSION: u8 = 4;
const NTP_MODE_CLIENT: u8 = 3;
/// Seconds between the NTP epoch (1900-01-01) and the Unix epoch
/// (1970-01-01) — matches Go's `ntpEpochOffset`.
const NTP_EPOCH_OFFSET: i64 = 2_208_988_800;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_RETRY_DELAY: Duration = Duration::from_secs(1);
const DEFAULT_NTP_PORT: &str = "123";

/// The default NTP server pool, used when [`ClientConfig::server_urls`] is
/// empty — matches Go's `NewNTPClient` fallback exactly.
pub const DEFAULT_SERVERS: &[&str] = &[
    "pool.ntp.org:123",
    "time.google.com:123",
    "time.cloudflare.com:123",
];

/// Configuration for [`NtpClient`], ported from Go's `pkg/time.ClientConfig`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ClientConfig {
    #[serde(default)]
    pub server_urls: Vec<String>,
    /// Per-query timeout in seconds. `0` resolves to `5`.
    #[serde(default)]
    pub timeout: u64,
    /// Maximum attempts across all configured servers. `0` resolves to
    /// `server_urls.len() * 2`.
    #[serde(default)]
    pub max_retries: usize,
    /// Delay between retries in seconds. `0` resolves to `1`.
    #[serde(default)]
    pub retry_delay: u64,
}

/// The forwarder-side config struct's shape only — ported because
/// [`crate::config`]'s `TimeConfig` (out of this crate's scope, owned by
/// whatever composes the module's full config) references it. The actual
/// NTP-forwarding logic (`pkg/time/forwarder.go`) is not ported; this
/// crate's forwarding surface is DNS-only (see [`crate::forwarder`]).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ForwarderConfig {
    #[serde(default = "default_listen_address")]
    pub listen_address: String,
    /// Cache TTL in seconds.
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl: u64,
}

impl Default for ForwarderConfig {
    fn default() -> Self {
        ForwarderConfig {
            listen_address: default_listen_address(),
            cache_ttl: default_cache_ttl(),
        }
    }
}

fn default_listen_address() -> String {
    "127.0.0.1:123".to_string()
}

fn default_cache_ttl() -> u64 {
    60
}

/// The result of one successful NTP query. `offset_nanos`/`round_trip_nanos`
/// are signed nanosecond counts (Go's `time.Duration` is itself an `i64`
/// nanosecond count under the hood) rather than [`std::time::Duration`],
/// which cannot represent the negative offset a fast local clock produces.
#[derive(Debug, Clone, Copy)]
pub struct TimeResponse {
    pub server_time: SystemTime,
    pub local_time: SystemTime,
    pub offset_nanos: i64,
    pub round_trip_nanos: i64,
    pub stratum: u8,
}

/// Every way an [`NtpClient`] query can fail.
#[derive(Debug, thiserror::Error)]
pub enum NtpError {
    #[error("io error: {0}")]
    Io(String),
    #[error("query timed out")]
    Timeout,
    #[error("short NTP response: got {got} bytes, expected {expected}")]
    ShortResponse { got: usize, expected: usize },
    #[error("query cancelled")]
    Cancelled,
    #[error("all {attempts} NTP server attempts failed: {}", .errors.join("; "))]
    AllServersFailed {
        attempts: usize,
        errors: Vec<String>,
    },
}

struct SyncState {
    last_offset_nanos: i64,
    last_round_trip_nanos: i64,
    last_sync: Option<SystemTime>,
    synchronized: bool,
}

/// A plain-UDP SNTP client with round-robin failover across
/// [`ClientConfig::server_urls`].
pub struct NtpClient {
    server_urls: Vec<String>,
    timeout: Duration,
    max_retries: usize,
    retry_delay: Duration,
    current_index: AtomicUsize,
    state: Mutex<SyncState>,
}

impl NtpClient {
    pub fn new(config: ClientConfig) -> NtpClient {
        let mut server_urls = config.server_urls;
        if server_urls.is_empty() {
            server_urls = DEFAULT_SERVERS.iter().map(|s| s.to_string()).collect();
        }
        for server_url in &mut server_urls {
            *server_url = normalize_server_addr(server_url);
        }

        let timeout = if config.timeout > 0 {
            Duration::from_secs(config.timeout)
        } else {
            DEFAULT_TIMEOUT
        };
        let max_retries = if config.max_retries > 0 {
            config.max_retries
        } else {
            server_urls.len() * 2
        };
        let retry_delay = if config.retry_delay > 0 {
            Duration::from_secs(config.retry_delay)
        } else {
            DEFAULT_RETRY_DELAY
        };

        NtpClient {
            server_urls,
            timeout,
            max_retries,
            retry_delay,
            current_index: AtomicUsize::new(0),
            state: Mutex::new(SyncState {
                last_offset_nanos: 0,
                last_round_trip_nanos: 0,
                last_sync: None,
                synchronized: false,
            }),
        }
    }

    /// Queries the current server, retrying across the pool on any
    /// failure. The delay between attempts is cancellable via `cancel`.
    pub async fn query(&self, cancel: &CancellationToken) -> Result<TimeResponse, NtpError> {
        let mut errors = Vec::new();

        for attempt in 0..self.max_retries {
            let server_addr = self.current_server();
            match self.query_server(&server_addr).await {
                Ok(response) => {
                    let mut state = self.lock_state();
                    state.last_offset_nanos = response.offset_nanos;
                    state.last_round_trip_nanos = response.round_trip_nanos;
                    state.last_sync = Some(SystemTime::now());
                    state.synchronized = true;
                    return Ok(response);
                }
                Err(err) => {
                    errors.push(format!("{server_addr}: {err}"));
                    self.advance_server();
                    if attempt + 1 < self.max_retries {
                        let cancelled = tokio::select! {
                            _ = cancel.cancelled() => true,
                            _ = tokio::time::sleep(self.retry_delay) => false,
                        };
                        if cancelled {
                            return Err(NtpError::Cancelled);
                        }
                    }
                }
            }
        }

        self.lock_state().synchronized = false;
        Err(NtpError::AllServersFailed {
            attempts: self.max_retries,
            errors,
        })
    }

    /// Whether the last query succeeded, its offset, and when it happened.
    pub fn status(&self) -> (bool, i64, Option<SystemTime>) {
        let state = self.lock_state();
        (state.synchronized, state.last_offset_nanos, state.last_sync)
    }

    pub fn server_urls(&self) -> Vec<String> {
        self.server_urls.clone()
    }

    async fn query_server(&self, server_addr: &str) -> Result<TimeResponse, NtpError> {
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(|err| NtpError::Io(err.to_string()))?;
        socket
            .connect(server_addr)
            .await
            .map_err(|err| NtpError::Io(err.to_string()))?;

        let mut request = [0u8; NTP_PACKET_SIZE];
        request[0] = (NTP_VERSION << 3) | NTP_MODE_CLIENT;

        let t1 = SystemTime::now();
        let (t1_sec, t1_frac) = system_time_to_ntp(t1);
        request[24..28].copy_from_slice(&t1_sec.to_be_bytes());
        request[28..32].copy_from_slice(&t1_frac.to_be_bytes());
        request[40..44].copy_from_slice(&t1_sec.to_be_bytes());
        request[44..48].copy_from_slice(&t1_frac.to_be_bytes());

        let send_result = tokio::time::timeout(self.timeout, socket.send(&request)).await;
        let Ok(send_result) = send_result else {
            return Err(NtpError::Timeout);
        };
        send_result.map_err(|err| NtpError::Io(err.to_string()))?;

        let mut response = [0u8; NTP_PACKET_SIZE];
        let recv_result = tokio::time::timeout(self.timeout, socket.recv(&mut response)).await;
        let Ok(recv_result) = recv_result else {
            return Err(NtpError::Timeout);
        };
        let received = recv_result.map_err(|err| NtpError::Io(err.to_string()))?;
        let t4 = SystemTime::now();

        if received < NTP_PACKET_SIZE {
            return Err(NtpError::ShortResponse {
                got: received,
                expected: NTP_PACKET_SIZE,
            });
        }

        let stratum = response[1];
        let rx_sec = u32::from_be_bytes(response[32..36].try_into().unwrap());
        let rx_frac = u32::from_be_bytes(response[36..40].try_into().unwrap());
        let tx_sec = u32::from_be_bytes(response[40..44].try_into().unwrap());
        let tx_frac = u32::from_be_bytes(response[44..48].try_into().unwrap());

        let t2 = ntp_to_system_time(rx_sec, rx_frac);
        let t3 = ntp_to_system_time(tx_sec, tx_frac);

        Ok(TimeResponse {
            server_time: t3,
            local_time: t4,
            offset_nanos: ntp_offset_nanos(t1, t2, t3, t4),
            round_trip_nanos: ntp_round_trip_nanos(t1, t2, t3, t4),
            stratum,
        })
    }

    fn current_server(&self) -> String {
        let index = self.current_index.load(Ordering::Relaxed) % self.server_urls.len();
        self.server_urls[index].clone()
    }

    fn advance_server(&self) {
        self.current_index.fetch_add(1, Ordering::Relaxed);
    }

    fn lock_state(&self) -> MutexGuard<'_, SyncState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Appends the default NTP port when `addr` doesn't already end in one —
/// handles the common `"host"`/`"host:123"` cases Go's server list (and
/// most real configs) actually uses. Bare, unbracketed IPv6 literals are a
/// known limitation shared with the Go source, which has the same gap (Go's
/// `net.SplitHostPort` also requires brackets for IPv6 and falls back to
/// blindly appending `:123` on its error path).
fn normalize_server_addr(addr: &str) -> String {
    if let Some((_, port)) = addr.rsplit_once(':') {
        let is_numeric_port = !port.is_empty() && port.chars().all(|c| c.is_ascii_digit());
        if is_numeric_port {
            return addr.to_string();
        }
    }
    format!("{addr}:{DEFAULT_NTP_PORT}")
}

/// Nanoseconds since the Unix epoch, signed — negative for any instant
/// before 1970 (never expected in practice, but keeps the subtraction in
/// [`ntp_offset_nanos`]/[`ntp_round_trip_nanos`] exact instead of saturating).
fn nanos_since_epoch(t: SystemTime) -> i128 {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(err) => -(err.duration().as_nanos() as i128),
    }
}

/// The NTP clock-offset formula: `((T2-T1)+(T3-T4))/2` — matches Go exactly.
fn ntp_offset_nanos(t1: SystemTime, t2: SystemTime, t3: SystemTime, t4: SystemTime) -> i64 {
    let (t1n, t2n, t3n, t4n) = (
        nanos_since_epoch(t1),
        nanos_since_epoch(t2),
        nanos_since_epoch(t3),
        nanos_since_epoch(t4),
    );
    (((t2n - t1n) + (t3n - t4n)) / 2) as i64
}

/// The NTP round-trip-delay formula: `(T4-T1)-(T3-T2)` — matches Go exactly.
fn ntp_round_trip_nanos(t1: SystemTime, t2: SystemTime, t3: SystemTime, t4: SystemTime) -> i64 {
    let (t1n, t2n, t3n, t4n) = (
        nanos_since_epoch(t1),
        nanos_since_epoch(t2),
        nanos_since_epoch(t3),
        nanos_since_epoch(t4),
    );
    ((t4n - t1n) - (t3n - t2n)) as i64
}

/// Encodes a [`SystemTime`] as an NTP (seconds, fraction) pair. Out-of-range
/// (pre-1900 or post-2036) inputs clamp to `0` rather than wrapping,
/// matching Go's overflow-fallback comment (Go falls back to
/// `time.Now()`; this falls back to the NTP epoch itself — either way the
/// request's origin timestamp is not meaningfully used by a compliant
/// server, only echoed back).
fn system_time_to_ntp(t: SystemTime) -> (u32, u32) {
    let (unix_secs, nanos) = match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64, d.subsec_nanos()),
        Err(_) => (-NTP_EPOCH_OFFSET, 0),
    };
    let ntp_secs = unix_secs + NTP_EPOCH_OFFSET;
    let sec = if (0..=u32::MAX as i64).contains(&ntp_secs) {
        ntp_secs as u32
    } else {
        0
    };
    let frac = ((nanos as u64) << 32) / 1_000_000_000;
    (sec, frac as u32)
}

/// Decodes an NTP (seconds, fraction) pair back to a [`SystemTime`].
fn ntp_to_system_time(sec: u32, frac: u32) -> SystemTime {
    let unix_secs = sec as i64 - NTP_EPOCH_OFFSET;
    let nanos = ((frac as u64) * 1_000_000_000) >> 32;
    if unix_secs >= 0 {
        SystemTime::UNIX_EPOCH + Duration::new(unix_secs as u64, nanos as u32)
    } else {
        SystemTime::UNIX_EPOCH - Duration::new((-unix_secs) as u64, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_server_addr_appends_default_port() {
        assert_eq!(normalize_server_addr("pool.ntp.org"), "pool.ntp.org:123");
    }

    #[test]
    fn normalize_server_addr_leaves_an_explicit_port_alone() {
        assert_eq!(
            normalize_server_addr("pool.ntp.org:123"),
            "pool.ntp.org:123"
        );
        assert_eq!(normalize_server_addr("192.0.2.1:8123"), "192.0.2.1:8123");
    }

    #[test]
    fn ntp_round_trip_time_conversion_is_exact_to_the_second() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let (sec, frac) = system_time_to_ntp(now);
        let back = ntp_to_system_time(sec, frac);
        assert_eq!(
            back.duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            1_700_000_000
        );
    }

    /// Builds the four NTP handshake timestamps and asserts the textbook
    /// formula directly, independent of any real network round trip.
    #[test]
    fn offset_formula_matches_a_synthetic_packet() {
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        // Client sends at T1=base; server's clock is 500ms ahead and
        // receives at T2=base+10ms+500ms, transmits at T3=T2+5ms; client
        // receives at T4=base+30ms. Expected offset = ((T2-T1)+(T3-T4))/2.
        let t1 = base;
        let t2 = base + Duration::from_millis(510);
        let t3 = base + Duration::from_millis(515);
        let t4 = base + Duration::from_millis(30);

        let offset = ntp_offset_nanos(t1, t2, t3, t4);
        // t1 is the zero baseline, so (T2-T1) is just T2's offset from base.
        let expected = (510_000_000i64 + (515_000_000i64 - 30_000_000i64)) / 2;
        assert_eq!(offset, expected);
        assert_eq!(offset, 497_500_000); // server clock ~497.5ms ahead

        let round_trip = ntp_round_trip_nanos(t1, t2, t3, t4);
        let expected_rt = 30_000_000i64 - (515_000_000i64 - 510_000_000i64);
        assert_eq!(round_trip, expected_rt);
        assert_eq!(round_trip, 25_000_000); // 30ms total minus 5ms server-side processing
    }

    #[test]
    fn offset_formula_handles_a_slow_local_clock() {
        // Server is behind the client: negative offset must round-trip
        // correctly through the signed i128 intermediate.
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let t1 = base;
        let t2 = base - Duration::from_millis(200) + Duration::from_millis(5);
        let t3 = t2 + Duration::from_millis(1);
        let t4 = base + Duration::from_millis(10);

        let offset = ntp_offset_nanos(t1, t2, t3, t4);
        assert!(offset < 0, "expected a negative offset, got {offset}");
    }
}
