//! Integration test: drive the squawk built-in module through the REAL
//! daemon stack — real `HostServices` (telemetry, file-backed secrets,
//! config store), the real `DaemonService` gRPC handlers, no fakes for
//! anything the milestone brief calls out — proving the M1-M4 plugin
//! framework actually works end to end with a real module for the first
//! time (until now only test doubles and the trivial `hello` example
//! plugin ever exercised it).
//!
//! # Running
//!
//! ```sh
//! PENGUIN_INTEGRATION=1 cargo test -p penguind --test squawk_builtin -- --ignored --nocapture
//! ```
//!
//! Every test here is `#[ignore]` *and* separately checks
//! `PENGUIN_INTEGRATION=1` at runtime, so neither a plain `cargo test` nor a
//! bare `cargo test -- --ignored` ever binds a socket — same convention as
//! `penguin-daemon/tests/external_plugin.rs` and
//! `penguin-goplugin-host/tests/goplugin_compat.rs`.
//!
//! Nothing here touches a real network, a real DoH/NTP provider, or port
//! 53: the DoH client is pointed at a tiny local mock HTTP server this test
//! spins up on an ephemeral port, the NTP client at a tiny local mock UDP
//! responder (also ephemeral), the forwarder binds `127.0.0.1:0`, and
//! `system_dns.manage` stays `false` so the host's real resolver is never
//! touched.
//!
//! # Why this file re-implements a `SecretStoreProvider`
//!
//! `bins/penguind` is a binary-only crate (no `[lib]` target), so its
//! `tests/` binaries cannot `use crate::host_wiring::...` — there is no
//! compiled library artifact to link against, only the binary's own
//! `main.rs` and its private `mod` declarations. [`RealSecretsProvider`]
//! below is therefore a deliberate, minimal duplicate of
//! `src/host_wiring.rs`'s `SecretsStoreProvider`: same real
//! `penguin_secrets::Store::namespaced` backend, just redeclared so this
//! test can wire it without touching production code's module privacy.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio_stream::StreamExt;
use tonic::Request;

use penguin_daemon::broker::EventBroker;
use penguin_daemon::config::ConfigStore;
use penguin_daemon::host::{DaemonHostFactory, HostFactory, SecretStoreProvider};
use penguin_daemon::logring::LogRing;
use penguin_daemon::service::DaemonService;
use penguin_daemon::supervisor::{Supervisor, SupervisorConfig};
use penguin_proto::daemon::v1 as pb;
use penguin_proto::daemon::v1::daemon_server::Daemon;
use penguin_sdk::{EventSink, LicenseChecker, ModuleState, SecretStore};
use penguin_secrets::{Backend as SecretsBackend, Config as SecretsConfig, Store as SecretsStore};

/// Skips the calling test (with a message) unless the integration tier is
/// explicitly opted into. Kept separate from `#[ignore]`: a bare
/// `--ignored` run must still not spawn a real listener.
macro_rules! require_integration {
    () => {
        if std::env::var("PENGUIN_INTEGRATION").as_deref() != Ok("1") {
            eprintln!("SKIP: set PENGUIN_INTEGRATION=1 to run squawk_builtin tests");
            return;
        }
    };
}

/// See this file's module doc for why this exists instead of importing
/// `bins/penguind`'s own `host_wiring::SecretsStoreProvider`.
struct RealSecretsProvider {
    root: Arc<SecretsStore>,
}

impl SecretStoreProvider for RealSecretsProvider {
    fn store_for(&self, module: &str) -> Arc<dyn SecretStore> {
        Arc::new(self.root.namespaced(module))
    }
}

/// A [`LicenseChecker`] double for `HostServices::license()`. Unrelated to
/// squawk's own product license validator (`squawk_client::license`, which
/// the `license` command exercises directly against
/// `license.squawkdns.com`) — this is only the generic PenguinTech
/// entitlement surface every module's host carries, which squawk's `Module`
/// implementation never actually calls (its `license_feature` is empty).
struct FakeLicenseChecker;
impl LicenseChecker for FakeLicenseChecker {
    fn feature_enabled(&self, _key: &str) -> bool {
        true
    }
    fn tier(&self) -> String {
        "free".to_string()
    }
}

/// A minimal local DoH mock: answers every request with a fixed, valid DoH
/// JSON response, and records the `Authorization` header it last saw — the
/// concrete proof that `init` wired the DoH client using the token this
/// test seeded into the REAL secrets store, not a stub.
struct MockDohServer {
    base_url: String,
    last_authorization: Arc<StdMutex<Option<String>>>,
    _accept_loop: tokio::task::JoinHandle<()>,
}

async fn start_mock_doh_server() -> MockDohServer {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock DoH listener");
    let addr = listener.local_addr().expect("mock DoH local addr");
    let base_url = format!("http://{addr}/dns-query");
    let last_authorization = Arc::new(StdMutex::new(None));
    let last_authorization_for_loop = Arc::clone(&last_authorization);

    let accept_loop = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let last_authorization = Arc::clone(&last_authorization_for_loop);
            tokio::spawn(serve_doh_once(stream, last_authorization));
        }
    });

    MockDohServer {
        base_url,
        last_authorization,
        _accept_loop: accept_loop,
    }
}

async fn serve_doh_once(stream: TcpStream, last_authorization: Arc<StdMutex<Option<String>>>) {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).await.unwrap_or(0) == 0 {
        return;
    }

    loop {
        let mut line = String::new();
        let Ok(bytes_read) = reader.read_line(&mut line).await else {
            break;
        };
        if bytes_read == 0 || line == "\r\n" {
            break;
        }
        let trimmed = line.trim_end();
        if let Some((name, value)) = trimmed.split_once(':')
            && name.trim().eq_ignore_ascii_case("authorization")
        {
            *last_authorization.lock().expect("mutex poisoned") = Some(value.trim().to_string());
        }
    }

    let body =
        r#"{"Status":0,"Answer":[{"name":"example.com.","type":1,"TTL":300,"data":"192.0.2.10"}]}"#;
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = writer.write_all(head.as_bytes()).await;
    let _ = writer.write_all(body.as_bytes()).await;
    let _ = writer.shutdown().await;
}

/// A minimal local NTP mock: replies to any 48-byte SNTP request with a
/// well-formed 48-byte response so `squawk time` gets a genuine (if
/// locally-synthesized) clock-offset measurement instead of ever touching
/// a real NTP pool.
fn start_mock_ntp_server() -> (String, tokio::task::JoinHandle<()>) {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind mock NTP socket");
    let addr = socket.local_addr().expect("mock NTP local addr");
    socket
        .set_nonblocking(true)
        .expect("set mock NTP socket nonblocking");
    let socket = UdpSocket::from_std(socket).expect("adopt mock NTP socket into tokio");

    let handle = tokio::spawn(async move {
        let mut buf = [0u8; 48];
        loop {
            let Ok((_len, peer)) = socket.recv_from(&mut buf).await else {
                return;
            };
            let mut response = [0u8; 48];
            response[1] = 2; // stratum
            response[40..44].copy_from_slice(&3_900_000_000u32.to_be_bytes());
            let _ = socket.send_to(&response, peer).await;
        }
    });

    (addr.to_string(), handle)
}

/// Everything one test needs: the real `DaemonService`, a handle to its
/// underlying `Supervisor`, the shared telemetry (for the metrics-registry
/// assertion), and every guard/task that must outlive the test.
struct TestStack {
    service: DaemonService,
    supervisor: Supervisor,
    telemetry: Arc<penguin_telemetry::Telemetry>,
    doh_server: MockDohServer,
    _state_dir: TempDir,
    _config_dir: TempDir,
    _secrets_dir: TempDir,
    _ntp_task: tokio::task::JoinHandle<()>,
}

/// Builds the real stack: real telemetry, a real file-backed secrets store
/// (seeded with a real `auth_token` secret), a real schema-validating
/// config store (seeded with a squawk config pointed at the mock DoH/NTP
/// servers), and the squawk builtin registered exactly as
/// `bins/penguind/src/daemon_main.rs` registers it in production.
async fn build_stack() -> TestStack {
    let state_dir = TempDir::new().expect("state dir");
    let config_dir = TempDir::new().expect("config dir");
    let secrets_dir = TempDir::new().expect("secrets dir");
    std::fs::create_dir_all(config_dir.path().join("modules.d")).expect("modules.d");

    let doh_server = start_mock_doh_server().await;
    let (ntp_addr, ntp_task) = start_mock_ntp_server();

    let config_yaml = format!(
        "doh:\n  server_url: \"{doh_url}\"\n  verify_tls: false\nforwarder:\n  enabled: true\n  udp_addr: \"127.0.0.1:0\"\n  tcp_addr: \"127.0.0.1:0\"\nsystem_dns:\n  manage: false\ncache:\n  enabled: true\nntp:\n  server_urls:\n    - \"{ntp_addr}\"\n",
        doh_url = doh_server.base_url,
    );
    std::fs::write(
        config_dir.path().join("modules.d").join("squawk.yaml"),
        config_yaml,
    )
    .expect("write squawk config");

    let telemetry = Arc::new(penguin_telemetry::Telemetry::new("error").expect("telemetry"));
    let config_store = Arc::new(ConfigStore::new(config_dir.path()));
    let broker = Arc::new(EventBroker::new(64));
    let events: Arc<dyn EventSink> = broker.clone();

    let secrets_root = Arc::new(
        SecretsStore::open(SecretsConfig {
            service_name: String::new(),
            backend: SecretsBackend::FileOnly {
                file_dir: secrets_dir.path().to_path_buf(),
            },
        })
        .expect("open real file-backed secret store"),
    );
    // Seeds a real secret so `init`'s best-effort `secrets().get("auth_token")`
    // fallback (config leaves `doh.auth_token` empty) has something real to
    // find — and the mock DoH server records the Authorization header it
    // produces, proving the real secrets store actually reached the DoH
    // client, not a double.
    secrets_root
        .namespaced("squawk")
        .set("auth_token", b"itest-auth-token")
        .await
        .expect("seed real auth_token secret");

    let secrets_provider: Arc<dyn SecretStoreProvider> =
        Arc::new(RealSecretsProvider { root: secrets_root });
    let license: Arc<dyn LicenseChecker> = Arc::new(FakeLicenseChecker);

    let host_factory: Arc<dyn HostFactory> = Arc::new(DaemonHostFactory::new(
        telemetry.clone(),
        config_store,
        secrets_provider,
        license,
        events,
        state_dir.path().to_path_buf(),
    ));

    let supervisor = Supervisor::new(SupervisorConfig {
        registry: penguin_registry::builtin_modules(),
        host_factory,
        broker: broker.clone(),
        state_dir: state_dir.path().to_path_buf(),
        max_restarts: 5,
        health_interval: Duration::from_secs(3600),
        stability_window: Duration::from_secs(3600),
        external: None,
    });

    let logs = Arc::new(LogRing::new(64));
    let service = DaemonService::new(supervisor.clone(), broker, logs, "itest", None);

    TestStack {
        service,
        supervisor,
        telemetry,
        doh_server,
        _state_dir: state_dir,
        _config_dir: config_dir,
        _secrets_dir: secrets_dir,
        _ntp_task: ntp_task,
    }
}

/// Dispatches one command through the real `DaemonService` RPC and returns
/// its single final chunk.
async fn dispatch_once(
    service: &DaemonService,
    module: &str,
    path: Vec<String>,
) -> pb::DispatchChunk {
    let mut stream = service
        .dispatch(Request::new(pb::DispatchRequest {
            api_version: String::new(),
            module: module.to_string(),
            path,
            flags: HashMap::new(),
            args: Vec::new(),
        }))
        .await
        .expect("dispatch rpc")
        .into_inner();
    stream
        .next()
        .await
        .expect("dispatch stream yields one chunk")
        .expect("chunk ok")
}

/// Headline test: loads squawk as a builtin against the REAL stack, proves
/// every one of the seven M5 assertions, then unloads it cleanly.
#[tokio::test]
#[ignore]
async fn squawk_builtin_loads_and_operates_through_the_real_daemon_stack() {
    require_integration!();
    let stack = build_stack().await;

    // Subscribe to WatchEvents *before* loading, so a lifecycle event
    // published during load is not missed (the broker never replays past
    // events to a new subscriber) — proves assertion 6.
    let mut event_stream = stack
        .service
        .watch_events(Request::new(pb::WatchEventsRequest {
            api_version: String::new(),
            module: "squawk".to_string(),
        }))
        .await
        .expect("watch_events rpc")
        .into_inner();

    // --- Assertion 1: supervisor loads squawk as a builtin module. -------
    let state = stack
        .supervisor
        .load("squawk")
        .await
        .expect("squawk (a real, non-fake module) must load through the supervisor");
    assert_eq!(state, ModuleState::Running);

    // --- Assertion 6: a lifecycle event reaches a WatchEvents subscriber. -
    let mut saw_squawk_event = false;
    for _ in 0..8 {
        let Ok(Some(Ok(event))) =
            tokio::time::timeout(Duration::from_secs(2), event_stream.next()).await
        else {
            break;
        };
        if event.module == "squawk" {
            saw_squawk_event = true;
            break;
        }
    }
    assert!(
        saw_squawk_event,
        "no module lifecycle event from squawk reached the WatchEvents subscriber"
    );

    // --- Assertion 3: squawk's commands appear in ListCommands. ----------
    let list_commands = stack
        .service
        .list_commands(Request::new(pb::ListCommandsRequest {
            api_version: String::new(),
        }))
        .await
        .expect("list_commands rpc")
        .into_inner();
    let squawk_commands = list_commands
        .modules
        .iter()
        .find(|m| m.module == "squawk")
        .expect("squawk must appear in ListCommands");
    let command_names: Vec<&str> = squawk_commands
        .commands
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    for expected in ["query", "forward", "config", "cache", "license", "time"] {
        assert!(
            command_names.contains(&expected),
            "ListCommands missing squawk command {expected:?}: got {command_names:?}"
        );
    }

    // --- Assertion 2 (concrete proof): a real DoH query through `query`
    // carries the auth token this test seeded into the REAL file-backed
    // secrets store — impossible unless `init` actually read `HostServices`
    // for real, not a double.
    let query_chunk = {
        let mut stream = stack
            .service
            .dispatch(Request::new(pb::DispatchRequest {
                api_version: String::new(),
                module: "squawk".to_string(),
                path: vec!["query".to_string()],
                flags: HashMap::new(),
                args: vec!["example.com".to_string()],
            }))
            .await
            .expect("dispatch rpc")
            .into_inner();
        stream.next().await.expect("one chunk").expect("chunk ok")
    };
    assert_eq!(
        query_chunk.exit_code, 0,
        "query dispatch failed: {}",
        query_chunk.output
    );
    let seen_auth = stack
        .doh_server
        .last_authorization
        .lock()
        .expect("mutex poisoned")
        .clone();
    assert_eq!(
        seen_auth.as_deref(),
        Some("Bearer itest-auth-token"),
        "DoH client did not use the auth token seeded into the real secrets store"
    );

    // --- Assertion 4: `cache stats` and `time` are real, not canned text. -
    let cache_stats = dispatch_once(
        &stack.service,
        "squawk",
        vec!["cache".to_string(), "stats".to_string()],
    )
    .await;
    assert_eq!(cache_stats.exit_code, 0);
    assert!(
        cache_stats.output.contains("entries"),
        "unexpected cache stats output: {}",
        cache_stats.output
    );
    assert!(
        !cache_stats.output.contains("not currently exposed")
            && !cache_stats.output.contains("not directly accessible"),
        "cache stats still returns the old Go canned text: {}",
        cache_stats.output
    );

    let cache_flush = dispatch_once(
        &stack.service,
        "squawk",
        vec!["cache".to_string(), "flush".to_string()],
    )
    .await;
    assert_eq!(cache_flush.exit_code, 0);

    let time_chunk = dispatch_once(&stack.service, "squawk", vec!["time".to_string()]).await;
    assert_eq!(
        time_chunk.exit_code, 0,
        "time dispatch failed against the mock NTP server: {}",
        time_chunk.output
    );
    assert!(
        !time_chunk.output.contains("not currently exposed")
            && !time_chunk.output.contains("not configured"),
        "time still returns the old Go canned text: {}",
        time_chunk.output
    );
    assert!(
        time_chunk.output.contains("stratum"),
        "time output does not look like a real NTP measurement: {}",
        time_chunk.output
    );

    // --- Assertion 5: all five metrics are in the shared registry. -------
    let families = stack.telemetry.registry().gather();
    let names: Vec<&str> = families.iter().map(|f| f.name()).collect();
    for expected in [
        "penguin_module_squawk_squawk_queries_total",
        "penguin_module_squawk_squawk_forwarder_up",
        "penguin_module_squawk_squawk_cache_entries",
        "penguin_module_squawk_squawk_dns_applied",
        "penguin_module_squawk_squawk_health_status",
    ] {
        assert!(
            names.contains(&expected),
            "shared Prometheus registry missing {expected:?}; got {names:?}"
        );
    }

    // --- Assertion 7: unload cleanly. -------------------------------------
    stack
        .supervisor
        .unload("squawk")
        .await
        .expect("squawk must unload cleanly");
    let after_unload = stack.service.get_status(Request::new(pb::GetStatusRequest {
        api_version: String::new(),
        name: "squawk".to_string(),
    }));
    assert!(
        after_unload.await.is_err(),
        "squawk must no longer report status once unloaded"
    );
}
