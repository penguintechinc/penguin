//! Proves the headline claim of this whole task: a correctly-written Rust
//! plugin dials the host's `HostService` over the go-plugin broker's id=1
//! leg, `Module::init` actually receives working `HostServices`, and a
//! module built on top of them can log, publish an event, and round-trip a
//! secret — the exact leg the frozen Go SDK has never been able to
//! exercise (see `docs/PARITY.md` §1.10 and
//! `penguin-goplugin-host`'s crate-level doc comment on the same finding).
//!
//! This drives the real `penguin-goplugin-host` client against the real,
//! compiled `plugin-hello-rs` binary — `env!("CARGO_BIN_EXE_plugin-hello-rs")`
//! is set by Cargo automatically for integration tests of a package that
//! contains that binary target, so the binary is always built and up to
//! date before this test runs; no separate build step or path-guessing is
//! needed.
//!
//! Gated the same way as `penguin-goplugin-host`'s own compat suite:
//! `#[ignore]` *and* a runtime `PENGUIN_INTEGRATION=1` check, so neither a
//! plain `cargo test` nor a bare `cargo test -- --ignored` spawns a process.
//!
//! ```sh
//! PENGUIN_INTEGRATION=1 cargo test -p plugin-hello-rs \
//!     --test hostservice_roundtrip -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::mpsc;
use tonic::{Request, Response, Status};

use penguin_goplugin_host::client::PluginProcess;
use penguin_proto::sdk::v1 as pb;
use penguin_proto::sdk::v1::host_service_server::{HostService, HostServiceServer};

/// Skips the calling test (with a message) unless the integration tier is
/// explicitly opted into. Mirrors
/// `penguin-goplugin-host/tests/goplugin_compat.rs`'s identical macro.
macro_rules! require_integration {
    () => {
        if std::env::var("PENGUIN_INTEGRATION").as_deref() != Ok("1") {
            eprintln!("SKIP: set PENGUIN_INTEGRATION=1 to run hostservice_roundtrip tests");
            return;
        }
    };
}

/// A real `HostService` server, standing in for the daemon: records every
/// `Log`/`PublishEvent` call it receives and backs `Secrets*` with an
/// in-memory map, so the test can assert the plugin's `init` and
/// `hostcheck` actually reached it over the wire.
struct MockHostService {
    logs_tx: mpsc::UnboundedSender<pb::LogRequest>,
    events_tx: mpsc::UnboundedSender<pb::PublishEventRequest>,
    secrets: Mutex<HashMap<String, Vec<u8>>>,
}

#[tonic::async_trait]
impl HostService for MockHostService {
    async fn log(
        &self,
        request: Request<pb::LogRequest>,
    ) -> Result<Response<pb::LogResponse>, Status> {
        let _ = self.logs_tx.send(request.into_inner());
        Ok(Response::new(pb::LogResponse {}))
    }

    async fn secrets_get(
        &self,
        request: Request<pb::SecretsGetRequest>,
    ) -> Result<Response<pb::SecretsGetResponse>, Status> {
        let key = request.into_inner().key;
        let store = self.secrets.lock().unwrap_or_else(|e| e.into_inner());
        let response = match store.get(&key) {
            Some(value) => pb::SecretsGetResponse {
                value: value.clone(),
                error: String::new(),
            },
            None => pb::SecretsGetResponse {
                value: Vec::new(),
                error: "not found".to_string(),
            },
        };
        Ok(Response::new(response))
    }

    async fn secrets_set(
        &self,
        request: Request<pb::SecretsSetRequest>,
    ) -> Result<Response<pb::SecretsSetResponse>, Status> {
        let req = request.into_inner();
        let mut store = self.secrets.lock().unwrap_or_else(|e| e.into_inner());
        store.insert(req.key, req.value);
        Ok(Response::new(pb::SecretsSetResponse {
            error: String::new(),
        }))
    }

    async fn secrets_delete(
        &self,
        request: Request<pb::SecretsDeleteRequest>,
    ) -> Result<Response<pb::SecretsDeleteResponse>, Status> {
        let key = request.into_inner().key;
        let mut store = self.secrets.lock().unwrap_or_else(|e| e.into_inner());
        store.remove(&key);
        Ok(Response::new(pb::SecretsDeleteResponse {
            error: String::new(),
        }))
    }

    async fn license_feature_enabled(
        &self,
        _request: Request<pb::LicenseFeatureEnabledRequest>,
    ) -> Result<Response<pb::LicenseFeatureEnabledResponse>, Status> {
        Ok(Response::new(pb::LicenseFeatureEnabledResponse {
            enabled: false,
        }))
    }

    async fn license_tier(
        &self,
        _request: Request<pb::LicenseTierRequest>,
    ) -> Result<Response<pb::LicenseTierResponse>, Status> {
        Ok(Response::new(pb::LicenseTierResponse {
            tier: "free".to_string(),
        }))
    }

    async fn data_dir(
        &self,
        _request: Request<pb::DataDirRequest>,
    ) -> Result<Response<pb::DataDirResponse>, Status> {
        Ok(Response::new(pb::DataDirResponse {
            path: "/tmp".to_string(),
        }))
    }

    async fn config(
        &self,
        _request: Request<pb::ConfigRequest>,
    ) -> Result<Response<pb::ConfigResponse>, Status> {
        Ok(Response::new(pb::ConfigResponse {
            config: b"hostservice-roundtrip-test-config".to_vec(),
        }))
    }

    async fn publish_event(
        &self,
        request: Request<pb::PublishEventRequest>,
    ) -> Result<Response<pb::PublishEventResponse>, Status> {
        let _ = self.events_tx.send(request.into_inner());
        Ok(Response::new(pb::PublishEventResponse {}))
    }
}

/// The path to the real, compiled `plugin-hello-rs` binary. Set by Cargo for
/// every integration test of a package that owns a binary target of the
/// same name — see the module doc comment.
fn plugin_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_plugin-hello-rs"))
}

#[tokio::test]
#[ignore]
async fn hostservice_callbacks_round_trip() {
    require_integration!();

    let (logs_tx, mut logs_rx) = mpsc::unbounded_channel();
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let mock = MockHostService {
        logs_tx,
        events_tx,
        secrets: Mutex::new(HashMap::new()),
    };
    let routes = tonic::service::Routes::new(HostServiceServer::new(mock));

    let bin_path = plugin_binary_path();
    let process = PluginProcess::launch(&bin_path, Path::new("/tmp"), Some(routes))
        .await
        .expect("plugin-hello-rs launch failed — see the HostError above");

    // Assertion: the log line from `HelloRsModule::init` arrived over the
    // broker's id=1 leg.
    let log_request = tokio::time::timeout(Duration::from_secs(5), logs_rx.recv())
        .await
        .expect("timed out waiting for HostService.Log — the broker leg never connected")
        .expect("log channel closed before a message arrived");
    assert_eq!(log_request.message, "hello-rs module initialised");
    assert_eq!(log_request.level, "info");

    // Assertion: the event from `HelloRsModule::init` arrived too.
    let event_request = tokio::time::timeout(Duration::from_secs(5), events_rx.recv())
        .await
        .expect("timed out waiting for HostService.PublishEvent")
        .expect("event channel closed before a message arrived");
    assert_eq!(event_request.module, "hello-rs");
    assert_eq!(event_request.message, "hello-rs initialised");

    // Assertion: `hostcheck` round-trips a secret through the same
    // HostServices instance — a NoopHostServices fallback would report a
    // secrets-store error here instead of "round-trip ok".
    let module = process.dispense().await.expect("dispense ModuleService");
    let path = vec!["hostcheck".to_string()];
    let flags = HashMap::new();
    let result = module
        .dispatch(&path, &flags, &[])
        .await
        .expect("hostcheck dispatch must succeed at the transport and module level");
    assert_eq!(
        result.exit_code, 0,
        "hostcheck did not report success: {}",
        result.output
    );
    assert!(
        result.output.contains("round-trip ok"),
        "unexpected hostcheck output: {}",
        result.output
    );

    process.shutdown().await.expect("plugin shutdown");
}
