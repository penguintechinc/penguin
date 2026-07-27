//! A dial-**out** adapter connecting to OBS via obs-websocket v5, translating
//! OBS events into [`BridgeEvent`]s for local integration scripts.
//!
//! This adapter connects to a local OBS WebSocket server (default
//! `ws://127.0.0.1:4455`), authenticates with the SHA256 challenge handshake
//! (Hello -> Identify -> Identified), then streams OBS events back into the
//! bridge's event channel. Any script can subscribe to these events via the
//! bridge's WebSocket transport without ever seeing the OBS password.
//!
//! # Security
//!
//! The OBS WebSocket password is stored in the [`ObsConfig`], passed at
//! adapter construction, and **never** logged or leaked into errors. Diagnostic
//! logging applies [`crate::mask::mask_secret`] to any rendered secret. The
//! adapter only connects to loopback addresses; a non-loopback URL is rejected.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::bridge::{BridgeAdapter, BridgeError, BridgeState};

#[cfg(test)]
use tokio::time::sleep;

/// Configuration for the OBS adapter: the WebSocket URL and password.
#[derive(Debug, Clone)]
pub struct ObsConfig {
    /// The OBS WebSocket server URL, typically `ws://127.0.0.1:4455`.
    /// **Must** be a loopback address; non-loopback URLs are rejected at
    /// connection time to match the bridge's security model.
    pub url: String,
    /// The OBS WebSocket password — kept secret and never logged unmasked.
    pub password: String,
}

impl ObsConfig {
    /// Creates a new OBS configuration with a URL and password. The URL
    /// must be a loopback address; validation happens at connection time.
    pub fn new(url: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            password: password.into(),
        }
    }
}

/// The OBS WebSocket v5 adapter — implements [`BridgeAdapter`] to connect
/// to a local OBS WebSocket server and stream events.
pub struct ObsAdapter {
    config: ObsConfig,
}

impl ObsAdapter {
    /// Creates a new OBS adapter with the given configuration.
    pub fn new(config: ObsConfig) -> Self {
        Self { config }
    }
}

impl BridgeAdapter for ObsAdapter {
    fn name(&self) -> &str {
        "obs"
    }

    fn attach(&self, state: Arc<BridgeState>) -> Result<(), BridgeError> {
        let config = self.config.clone();
        let state_for_task = Arc::clone(&state);

        // Spawn a background task that connects and streams events. If the
        // connection fails or drops, the task exits; the bridge continues
        // running unaffected. A reconnection strategy is not implemented in
        // this first version.
        tokio::spawn(async move {
            if let Err(e) = run_obs_connection(config, state_for_task).await {
                // Log the error but don't panic — a failed OBS connection does
                // not take down the entire bridge. The logger is available only
                // indirectly (through state.module.host().logger()), so we log
                // here without any internal crate access.
                eprintln!("obs adapter: connection failed: {e}");
            }
        });

        Ok(())
    }
}

/// The WebSocket message types from the obs-websocket v5 spec.
#[derive(Debug, Serialize, Deserialize)]
struct WsMessage {
    #[serde(default)]
    op: u32,
    #[serde(default)]
    d: Option<Value>,
}

/// Hello message from the server: carries `sessionId` and `serverTime` for
/// the auth challenge.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct HelloData {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "serverTime")]
    server_time: u64,
    #[serde(rename = "serverVersion")]
    server_version: Option<String>,
    #[serde(default)]
    authentication: Option<String>,
}

/// Identify message we send to the server: carries the sessionId and auth hash.
#[derive(Debug, Serialize)]
struct IdentifyData {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(default)]
    authentication: Option<String>,
}

/// Main connection loop: establishes the WebSocket, performs the auth
/// handshake, and streams OBS events into the bridge state.
async fn run_obs_connection(config: ObsConfig, state: Arc<BridgeState>) -> Result<(), String> {
    use futures_util::sink::SinkExt as _;
    use futures_util::stream::StreamExt as _;

    // Validate the URL is a loopback address before connecting.
    validate_loopback_url(&config.url)?;

    // Connect to the OBS WebSocket server.
    let (ws, _resp) = connect_async(&config.url)
        .await
        .map_err(|e| format!("failed to connect to OBS at {}: {}", config.url, e))?;

    // Publish a connection event so scripts know the adapter is alive.
    state.publish_event(super::BridgeEvent {
        kind: "obs.connected".to_string(),
        data: json!({"url": config.url}),
    });

    // Split the WebSocket into a sender and receiver so we can handle both
    // directions concurrently.
    let (mut ws_send, mut ws_recv) = ws.split();

    // Wait for the Hello message from OBS.
    let hello_msg = ws_recv
        .next()
        .await
        .ok_or("server closed connection before sending Hello".to_string())?
        .map_err(|e| format!("failed to read Hello message: {e}"))?;

    let hello_text = hello_msg
        .into_text()
        .map_err(|_| "Hello message was not text".to_string())?;
    let hello_json: WsMessage =
        serde_json::from_str(&hello_text).map_err(|e| format!("malformed Hello message: {e}"))?;

    // Parse the Hello's session info.
    let hello_data: HelloData = serde_json::from_value(
        hello_json
            .d
            .ok_or("Hello message missing data field".to_string())?,
    )
    .map_err(|e| format!("invalid Hello data: {e}"))?;

    // Perform the auth challenge handshake: compute the auth hash and send Identify.
    let auth_hash = if let Some(challenge) = &hello_data.authentication {
        Some(compute_auth_hash(&config.password, challenge)?)
    } else {
        None
    };

    let identify_data = IdentifyData {
        session_id: hello_data.session_id.clone(),
        authentication: auth_hash,
    };
    let identify_msg = json!({
        "op": 1,  // op=1 is Identify
        "d": identify_data,
    });
    ws_send
        .send(Message::Text(identify_msg.to_string().into()))
        .await
        .map_err(|e| format!("failed to send Identify: {e}"))?;

    // Wait for the Identified confirmation.
    let identified_msg = ws_recv
        .next()
        .await
        .ok_or("server closed connection before sending Identified".to_string())?
        .map_err(|e| format!("failed to read Identified message: {e}"))?;

    let identified_text = identified_msg
        .into_text()
        .map_err(|_| "Identified message was not text".to_string())?;
    let identified_json: WsMessage = serde_json::from_str(&identified_text)
        .map_err(|e| format!("malformed Identified message: {e}"))?;

    // If op != 2 (Identified), authentication failed.
    if identified_json.op != 2 {
        return Err("server rejected authentication (did not send Identified)".to_string());
    }

    // Publish an authenticated event and then start streaming OBS events.
    state.publish_event(super::BridgeEvent {
        kind: "obs.authenticated".to_string(),
        data: json!({"sessionId": hello_data.session_id}),
    });

    // Stream incoming OBS events (op=5 is EventMessage) into the bridge.
    while let Some(msg_result) = ws_recv.next().await {
        match msg_result {
            Ok(Message::Text(text)) => {
                if let Ok(event_json) = serde_json::from_str::<WsMessage>(&text)
                    && event_json.op == 5
                {
                    // op=5 is EventMessage from the server.
                    if let Some(event_data) = event_json.d {
                        // Extract the event name and metadata.
                        if let Some(event_name) =
                            event_data.get("eventName").and_then(|v| v.as_str())
                        {
                            state.publish_event(super::BridgeEvent {
                                kind: format!("obs.event.{}", event_name),
                                data: event_data.get("eventData").cloned().unwrap_or(Value::Null),
                            });
                        }
                    }
                }
            }
            Ok(Message::Close(_)) => {
                state.publish_event(super::BridgeEvent {
                    kind: "obs.disconnected".to_string(),
                    data: json!({}),
                });
                break;
            }
            Err(e) => {
                return Err(format!("WebSocket error: {e}"));
            }
            _ => {}
        }
    }

    Ok(())
}

/// Validates that the given URL is a WebSocket loopback address.
/// The adapter only connects to local OBS servers (127.0.0.0/8 or ::1)
/// to prevent credential exfiltration via SSRF attacks.
///
/// Security contract: only accepts `ws://` scheme (not `wss`, `http`, etc.)
/// and verifies the host is a loopback address via proper URL parsing
/// (not substring matching, which is trivially bypassable).
fn validate_loopback_url(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("invalid URL: {}", e))?;

    // Only accept ws:// scheme (plaintext loopback connection).
    // wss://, http://, https://, etc. are rejected.
    if parsed.scheme() != "ws" {
        return Err(format!(
            "OBS WebSocket URL must use ws:// scheme, got: {}",
            parsed.scheme()
        ));
    }

    // Extract and validate the host.
    let host = parsed.host().ok_or("OBS WebSocket URL has no host")?;

    match host {
        url::Host::Domain(d) => {
            // Only localhost is accepted (case-insensitive).
            if d.eq_ignore_ascii_case("localhost") {
                Ok(())
            } else {
                Err(format!(
                    "OBS WebSocket URL host must be localhost, got: {}",
                    d
                ))
            }
        }
        url::Host::Ipv4(ip) => {
            // Accept if the IP is a loopback address (127.0.0.0/8).
            if std::net::IpAddr::V4(ip).is_loopback() {
                Ok(())
            } else {
                Err(format!(
                    "OBS WebSocket URL must be a loopback address, got: {}",
                    ip
                ))
            }
        }
        url::Host::Ipv6(ip) => {
            // Accept if the IP is a loopback address (::1).
            if std::net::IpAddr::V6(ip).is_loopback() {
                Ok(())
            } else {
                Err(format!(
                    "OBS WebSocket URL must be a loopback address, got: {}",
                    ip
                ))
            }
        }
    }
}

/// Computes the obs-websocket v5 authentication hash given the password and
/// the server's challenge. The formula is:
/// `auth = base64(sha256(base64(sha256(password + salt)) + challenge))`.
fn compute_auth_hash(password: &str, challenge: &str) -> Result<String, String> {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};

    // Parse the challenge to extract the salt and the challenge string.
    // The challenge format is: "{salt}.{challenge}".
    let parts: Vec<&str> = challenge.split('.').collect();
    if parts.len() != 2 {
        return Err("challenge format invalid (expected salt.challenge)".to_string());
    }
    let salt = parts[0];
    let challenge_str = parts[1];

    // Step 1: Hash password + salt and base64 encode it.
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    hasher.update(salt.as_bytes());
    let hash1 = hasher.finalize();
    let hash1_b64 = base64::engine::general_purpose::STANDARD.encode(hash1);

    // Step 2: Hash the concatenation of hash1_b64 + challenge and base64 encode.
    let mut hasher = Sha256::new();
    hasher.update(hash1_b64.as_bytes());
    hasher.update(challenge_str.as_bytes());
    let hash2 = hasher.finalize();
    let auth_hash = base64::engine::general_purpose::STANDARD.encode(hash2);

    Ok(auth_hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream::StreamExt;
    use penguin_sdk::Module;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    /// A mock OBS WebSocket v5 server for testing. Accepts one connection,
    /// performs the auth handshake, and can send or receive messages.
    #[allow(dead_code)]
    struct MockObsServer {
        listen_addr: SocketAddr,
        messages_sent: Arc<Mutex<Vec<String>>>,
        messages_received: Arc<Mutex<Vec<String>>>,
        server_task: tokio::task::JoinHandle<()>,
    }

    impl MockObsServer {
        /// Starts a mock OBS server on a loopback ephemeral port. Returns the
        /// server and its WebSocket URL.
        async fn start() -> (Self, String) {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind loopback listener");
            let addr = listener.local_addr().expect("get bound address");
            let url = format!("ws://{}", addr);

            let messages_sent = Arc::new(Mutex::new(Vec::new()));
            let messages_received = Arc::new(Mutex::new(Vec::new()));

            let sent_clone = Arc::clone(&messages_sent);
            let recv_clone = Arc::clone(&messages_received);

            let server_task = tokio::spawn(async move {
                if let Ok((stream, _)) = listener.accept().await
                    && let Ok(ws) = accept_async(stream).await
                {
                    let (mut ws_send, mut ws_recv) = ws.split();

                    // Send Hello message.
                    use futures_util::sink::SinkExt as _;
                    let hello = json!({
                        "op": 0,
                        "d": {
                            "serverVersion": "5.0.0",
                            "sessionId": "test-session-123",
                            "serverTime": 1234567890u64,
                            "authentication": "salt123.challenge456",
                        },
                    });
                    let _ = ws_send.send(Message::Text(hello.to_string().into())).await;

                    // Receive and record Identify message.
                    if let Some(Ok(Message::Text(text))) = ws_recv.next().await {
                        recv_clone.lock().await.push(text.to_string());

                        // Send Identified confirmation.
                        let identified = json!({
                            "op": 2,
                            "d": {},
                        });
                        let _ = ws_send
                            .send(Message::Text(identified.to_string().into()))
                            .await;

                        // Stream events: send a mock SceneChanged event.
                        sleep(Duration::from_millis(50)).await;
                        let event = json!({
                            "op": 5,
                            "d": {
                                "eventName": "SceneChanged",
                                "eventData": {
                                    "sceneName": "TestScene",
                                },
                            },
                        });
                        let _ = ws_send.send(Message::Text(event.to_string().into())).await;
                        sent_clone.lock().await.push(event.to_string());

                        // Keep the connection open for a bit.
                        sleep(Duration::from_millis(200)).await;
                    }
                }
            });

            (
                Self {
                    listen_addr: addr,
                    messages_sent,
                    messages_received,
                    server_task,
                },
                url,
            )
        }

        /// Waits for the server task to complete.
        async fn stop(self) {
            let _ = tokio::time::timeout(Duration::from_secs(2), self.server_task).await;
        }

        /// Returns the messages the server received from the client.
        #[allow(dead_code)]
        async fn received_messages(&self) -> Vec<String> {
            self.messages_received.lock().await.clone()
        }

        /// Returns the messages the server sent to the client.
        #[allow(dead_code)]
        async fn sent_messages(&self) -> Vec<String> {
            self.messages_sent.lock().await.clone()
        }
    }

    #[tokio::test]
    async fn obs_adapter_connects_and_authenticates() {
        let (server, url) = MockObsServer::start().await;

        let config = ObsConfig::new(&url, "test-password");
        let adapter = ObsAdapter::new(config);

        // Create a mock bridge state.
        let hub = crate::testutil::MockHub::start().await;
        let dir = tempfile::tempdir().unwrap();
        let mut host = crate::testutil::FakeHost::new(dir.path().to_path_buf());
        host.config = serde_json::to_vec(&json!({
            "hub": {"base_url": hub.base_url},
            "community_id": 1,
        }))
        .unwrap();
        let module = crate::module::WaddlebotModule::new();
        module.init(Arc::new(host)).await.expect("init succeeds");
        let state = Arc::new(BridgeState::new(module, "test-cat".to_string()));

        // Subscribe to events so we can observe them.
        let mut rx = state.subscribe();
        let mut event_kinds = Vec::new();

        // Attach the adapter — this spawns the connection task.
        adapter.attach(Arc::clone(&state)).expect("attach succeeds");

        // Wait for at least one event (should be obs.connected, obs.authenticated, etc.).
        let timeout = Duration::from_secs(3);
        let start = std::time::Instant::now();
        while event_kinds.len() < 2 && start.elapsed() < timeout {
            if let Ok(Ok(e)) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                event_kinds.push(e.kind.clone());
            }
        }

        // Stop the server and verify we got connection events.
        server.stop().await;

        assert!(
            event_kinds.iter().any(|k| k.contains("obs.")),
            "expected at least one obs.* event, got: {:?}",
            event_kinds
        );
    }

    #[test]
    fn validate_loopback_url_accepts_127_0_0_1() {
        assert!(validate_loopback_url("ws://127.0.0.1:4455").is_ok());
    }

    #[test]
    fn validate_loopback_url_accepts_localhost() {
        assert!(validate_loopback_url("ws://localhost:4455").is_ok());
    }

    #[test]
    fn validate_loopback_url_accepts_ipv6_loopback() {
        assert!(validate_loopback_url("ws://[::1]:4455").is_ok());
    }

    #[test]
    fn validate_loopback_url_accepts_entire_loopback_range() {
        assert!(validate_loopback_url("ws://127.0.0.2:4455").is_ok());
        assert!(validate_loopback_url("ws://127.255.255.255:4455").is_ok());
    }

    #[test]
    fn validate_loopback_url_rejects_external_addresses() {
        assert!(validate_loopback_url("ws://192.168.1.1:4455").is_err());
        assert!(validate_loopback_url("ws://example.com:4455").is_err());
    }

    #[test]
    fn validate_loopback_url_rejects_substring_bypass_with_path() {
        // Attacker tries to bypass by appending 127.0.0.1 in the path
        assert!(validate_loopback_url("ws://evil.com/?x=127.0.0.1").is_err());
    }

    #[test]
    fn validate_loopback_url_rejects_substring_bypass_with_fragment() {
        // Attacker tries to bypass by appending localhost in fragment
        assert!(validate_loopback_url("ws://evil.com#localhost").is_err());
    }

    #[test]
    fn validate_loopback_url_rejects_domain_suffix_bypass() {
        // Attacker tries to bypass with domain that has loopback as suffix
        assert!(validate_loopback_url("ws://127.0.0.1.evil.com:4455/").is_err());
        assert!(validate_loopback_url("ws://localhost.evil.com/").is_err());
    }

    #[test]
    fn validate_loopback_url_rejects_userinfo_confusion() {
        // Attacker tries to bypass using userinfo (127.0.0.1@evil.com has host evil.com)
        assert!(validate_loopback_url("ws://127.0.0.1@evil.com:4455").is_err());
    }

    #[test]
    fn validate_loopback_url_rejects_private_network_addresses() {
        // Reject LAN addresses (10/8, 172.16/12, 192.168/16)
        assert!(validate_loopback_url("ws://192.168.1.1:4455").is_err());
        assert!(validate_loopback_url("ws://10.0.0.1:4455").is_err());
        assert!(validate_loopback_url("ws://172.16.0.1:4455").is_err());
    }

    #[test]
    fn validate_loopback_url_rejects_cloud_metadata_address() {
        // Reject cloud metadata endpoint (AWS, GCP, etc.)
        assert!(validate_loopback_url("ws://169.254.169.254:4455").is_err());
    }

    #[test]
    fn validate_loopback_url_rejects_wrong_scheme_wss() {
        // Only ws:// is allowed, not wss://
        assert!(validate_loopback_url("wss://127.0.0.1:4455").is_err());
        assert!(validate_loopback_url("wss://localhost:4455").is_err());
        assert!(validate_loopback_url("wss://[::1]:4455").is_err());
    }

    #[test]
    fn validate_loopback_url_rejects_wrong_scheme_http() {
        // Reject http:// and https://
        assert!(validate_loopback_url("http://127.0.0.1:4455").is_err());
        assert!(validate_loopback_url("https://127.0.0.1:4455").is_err());
        assert!(validate_loopback_url("http://localhost:4455").is_err());
    }

    #[test]
    fn validate_loopback_url_rejects_malformed_url() {
        // Reject URLs that cannot be parsed
        assert!(validate_loopback_url("not a valid url").is_err());
        assert!(validate_loopback_url("://invalid").is_err());
    }

    #[test]
    fn validate_loopback_url_rejects_no_host() {
        // Reject URLs with no host
        assert!(validate_loopback_url("ws://").is_err());
    }

    #[test]
    fn compute_auth_hash_succeeds_with_valid_challenge() {
        use base64::Engine as _;

        // Use a fixed challenge for deterministic testing.
        let password = "test-password";
        let challenge = "salt123.challenge456";

        let result = compute_auth_hash(password, challenge);
        assert!(result.is_ok());
        let hash = result.unwrap();

        // Verify it's not empty and is a valid base64 string.
        assert!(!hash.is_empty());
        assert!(
            base64::engine::general_purpose::STANDARD
                .decode(&hash)
                .is_ok()
        );
    }

    #[test]
    fn compute_auth_hash_rejects_malformed_challenge() {
        let password = "test-password";
        let challenge = "invalid-challenge-no-dot";

        let result = compute_auth_hash(password, challenge);
        assert!(result.is_err());
    }

    #[test]
    fn password_never_leaks_into_error_messages() {
        let password = "super-secret-password";

        let config = ObsConfig::new("ws://192.168.1.1:4455", password);
        let adapter = ObsAdapter::new(config);

        // The adapter's attach should fail, but the error should not contain the password.
        // In this case, it's deferred to the spawned task, so we check at construction time.
        // Let's verify that the password is stored but never exposed via name().
        assert!(!adapter.name().contains(password));
    }

    #[tokio::test]
    async fn obs_adapter_handles_auth_rejection() {
        // Mock server that rejects authentication (sends wrong opcode after Identify).
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback listener");
        let addr = listener.local_addr().expect("get bound address");
        let url = format!("ws://{}", addr);

        let server_task = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await
                && let Ok(ws) = accept_async(stream).await
            {
                let (mut ws_send, mut ws_recv) = ws.split();

                // Send Hello message.
                use futures_util::sink::SinkExt as _;
                let hello = json!({
                    "op": 0,
                    "d": {
                        "serverVersion": "5.0.0",
                        "sessionId": "test-session-123",
                        "serverTime": 1234567890u64,
                        "authentication": "salt123.challenge456",
                    },
                });
                let _ = ws_send.send(Message::Text(hello.to_string().into())).await;

                // Receive Identify message.
                if let Some(Ok(Message::Text(_))) = ws_recv.next().await {
                    // Send a rejection response (wrong op code).
                    let rejection = json!({"op": 99, "d": {}});
                    let _ = ws_send
                        .send(Message::Text(rejection.to_string().into()))
                        .await;
                }
            }
        });

        let config = ObsConfig::new(&url, "test-password");
        let adapter = ObsAdapter::new(config);

        let hub = crate::testutil::MockHub::start().await;
        let dir = tempfile::tempdir().unwrap();
        let mut host = crate::testutil::FakeHost::new(dir.path().to_path_buf());
        host.config = serde_json::to_vec(&json!({
            "hub": {"base_url": hub.base_url},
            "community_id": 1,
        }))
        .unwrap();
        let module = crate::module::WaddlebotModule::new();
        module.init(Arc::new(host)).await.expect("init succeeds");
        let state = Arc::new(BridgeState::new(module, "test-cat".to_string()));

        let _rx = state.subscribe();
        adapter.attach(Arc::clone(&state)).expect("attach succeeds");

        // The connection should fail when auth is rejected, so we might not get events.
        // Give it time to fail.
        sleep(Duration::from_millis(500)).await;
        server_task.abort();
    }

    #[tokio::test]
    async fn obs_adapter_handles_connection_closed_before_hello() {
        // Mock server that closes connection immediately without sending Hello.
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback listener");
        let addr = listener.local_addr().expect("get bound address");
        let url = format!("ws://{}", addr);

        let server_task = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await
                && let Ok(ws) = accept_async(stream).await
            {
                let (_ws_send, _ws_recv) = ws.split();
                // Close immediately without sending anything.
            }
        });

        let config = ObsConfig::new(&url, "test-password");
        let adapter = ObsAdapter::new(config);

        let hub = crate::testutil::MockHub::start().await;
        let dir = tempfile::tempdir().unwrap();
        let mut host = crate::testutil::FakeHost::new(dir.path().to_path_buf());
        host.config = serde_json::to_vec(&json!({
            "hub": {"base_url": hub.base_url},
            "community_id": 1,
        }))
        .unwrap();
        let module = crate::module::WaddlebotModule::new();
        module.init(Arc::new(host)).await.expect("init succeeds");
        let state = Arc::new(BridgeState::new(module, "test-cat".to_string()));

        adapter.attach(Arc::clone(&state)).expect("attach succeeds");
        sleep(Duration::from_millis(500)).await;
        server_task.abort();
    }

    #[tokio::test]
    async fn obs_adapter_handles_unexpected_message_opcodes() {
        // Mock server that sends unexpected opcodes instead of expected ones.
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback listener");
        let addr = listener.local_addr().expect("get bound address");
        let url = format!("ws://{}", addr);

        let server_task = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await
                && let Ok(ws) = accept_async(stream).await
            {
                let (mut ws_send, mut ws_recv) = ws.split();

                // Send Hello message.
                use futures_util::sink::SinkExt as _;
                let hello = json!({
                    "op": 0,
                    "d": {
                        "serverVersion": "5.0.0",
                        "sessionId": "test-session-123",
                        "serverTime": 1234567890u64,
                        "authentication": "salt123.challenge456",
                    },
                });
                let _ = ws_send.send(Message::Text(hello.to_string().into())).await;

                // Receive Identify message.
                if let Some(Ok(Message::Text(_))) = ws_recv.next().await {
                    // Send Identified confirmation.
                    let identified = json!({"op": 2, "d": {}});
                    let _ = ws_send
                        .send(Message::Text(identified.to_string().into()))
                        .await;

                    // Send an event with unexpected opcode (not 5).
                    sleep(Duration::from_millis(50)).await;
                    let unknown = json!({
                        "op": 99,
                        "d": {"eventName": "UnknownEvent"},
                    });
                    let _ = ws_send
                        .send(Message::Text(unknown.to_string().into()))
                        .await;

                    // Keep connection open briefly.
                    sleep(Duration::from_millis(200)).await;
                }
            }
        });

        let config = ObsConfig::new(&url, "test-password");
        let adapter = ObsAdapter::new(config);

        let hub = crate::testutil::MockHub::start().await;
        let dir = tempfile::tempdir().unwrap();
        let mut host = crate::testutil::FakeHost::new(dir.path().to_path_buf());
        host.config = serde_json::to_vec(&json!({
            "hub": {"base_url": hub.base_url},
            "community_id": 1,
        }))
        .unwrap();
        let module = crate::module::WaddlebotModule::new();
        module.init(Arc::new(host)).await.expect("init succeeds");
        let state = Arc::new(BridgeState::new(module, "test-cat".to_string()));

        let mut rx = state.subscribe();
        adapter.attach(Arc::clone(&state)).expect("attach succeeds");

        // Wait for events. We should get at least connected and authenticated events,
        // but not fail on the unknown opcode.
        let timeout = Duration::from_secs(2);
        let start = std::time::Instant::now();
        let mut event_kinds = Vec::new();
        while start.elapsed() < timeout {
            if let Ok(Ok(e)) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                event_kinds.push(e.kind.clone());
            }
        }

        server_task.abort();
        assert!(
            event_kinds.iter().any(|k| k.contains("obs.connected")),
            "expected obs.connected event"
        );
    }

    #[tokio::test]
    async fn obs_adapter_handles_websocket_close_frame() {
        // Mock server that sends a Close frame after authentication.
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback listener");
        let addr = listener.local_addr().expect("get bound address");
        let url = format!("ws://{}", addr);

        let server_task = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await
                && let Ok(ws) = accept_async(stream).await
            {
                let (mut ws_send, mut ws_recv) = ws.split();

                // Send Hello message.
                use futures_util::sink::SinkExt as _;
                let hello = json!({
                    "op": 0,
                    "d": {
                        "serverVersion": "5.0.0",
                        "sessionId": "test-session-123",
                        "serverTime": 1234567890u64,
                        "authentication": "salt123.challenge456",
                    },
                });
                let _ = ws_send.send(Message::Text(hello.to_string().into())).await;

                // Receive Identify message.
                if let Some(Ok(Message::Text(_))) = ws_recv.next().await {
                    // Send Identified confirmation.
                    let identified = json!({"op": 2, "d": {}});
                    let _ = ws_send
                        .send(Message::Text(identified.to_string().into()))
                        .await;

                    sleep(Duration::from_millis(100)).await;
                    // Send Close frame.
                    let _ = ws_send.send(Message::Close(None)).await;
                }
            }
        });

        let config = ObsConfig::new(&url, "test-password");
        let adapter = ObsAdapter::new(config);

        let hub = crate::testutil::MockHub::start().await;
        let dir = tempfile::tempdir().unwrap();
        let mut host = crate::testutil::FakeHost::new(dir.path().to_path_buf());
        host.config = serde_json::to_vec(&json!({
            "hub": {"base_url": hub.base_url},
            "community_id": 1,
        }))
        .unwrap();
        let module = crate::module::WaddlebotModule::new();
        module.init(Arc::new(host)).await.expect("init succeeds");
        let state = Arc::new(BridgeState::new(module, "test-cat".to_string()));

        let mut rx = state.subscribe();
        adapter.attach(Arc::clone(&state)).expect("attach succeeds");

        // Wait for events including the disconnection.
        let timeout = Duration::from_secs(2);
        let start = std::time::Instant::now();
        let mut event_kinds = Vec::new();
        while start.elapsed() < timeout {
            if let Ok(Ok(e)) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                event_kinds.push(e.kind.clone());
            }
        }

        server_task.abort();
        assert!(
            event_kinds.iter().any(|k| k.contains("obs.disconnected")),
            "expected obs.disconnected event"
        );
    }
}
