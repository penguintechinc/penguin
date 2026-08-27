//! [`HostServices`] for built-in modules, ported from
//! `go-client/internal/daemon/host.go`.
//!
//! # Divergence: `host_for` takes the module's schema
//!
//! Go's `NewHost` receives already-resolved config bytes; the schema lookup
//! happens once in `cmd/penguind/service.go`, in a `hostFactory` closure that
//! closes over a precomputed `name -> schema` map built at startup. This crate
//! has no equivalent wiring file (that lives in the `penguind` binary, a later
//! milestone), so [`HostFactory::host_for`] takes the schema explicitly —
//! [`crate::supervisor::Supervisor`] has the live module instance at `load`
//! time and passes `module.config_schema()` straight through. This keeps
//! schema-validated config working without inventing an out-of-scope wiring
//! module just to precompute a map this crate can build on demand instead.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use penguin_sdk::{EventSink, HostServices, LicenseChecker, Logger, Metrics, SecretStore};
use penguin_telemetry::Telemetry;

use crate::config::ConfigStore;

/// Builds the [`HostServices`] handed to a module's `Init`.
///
/// One instance is shared by the whole daemon; [`HostFactory::host_for`] is
/// called once per module load and returns a handle scoped to that module.
pub trait HostFactory: Send + Sync {
    /// Builds the host services for `module`, validating its on-disk config
    /// against `schema` (the module's own `Module::config_schema()`).
    fn host_for(&self, module: &str, schema: Option<&[u8]>) -> Arc<dyn HostServices>;
}

/// The [`HostServices`] implementation for one loaded built-in module.
///
/// Every accessor is a cheap clone of a handle resolved once at construction
/// (see [`DaemonHostFactory::host_for`]) — mirrors the Go `HostImpl`, which is
/// likewise a bag of pre-resolved fields rather than doing work per call.
pub struct DaemonHost {
    module: String,
    logger: Arc<dyn Logger>,
    secrets: Arc<dyn SecretStore>,
    license: Arc<dyn LicenseChecker>,
    metrics: Arc<dyn Metrics>,
    data_dir: PathBuf,
    events: Arc<dyn EventSink>,
    config: Vec<u8>,
    otel: Option<Arc<penguin_otel::OtelPipeline>>,
}

impl HostServices for DaemonHost {
    fn logger(&self) -> Arc<dyn Logger> {
        self.logger.clone()
    }

    fn secrets(&self) -> Arc<dyn SecretStore> {
        self.secrets.clone()
    }

    fn license(&self) -> Arc<dyn LicenseChecker> {
        self.license.clone()
    }

    fn metrics(&self) -> Arc<dyn Metrics> {
        self.metrics.clone()
    }

    fn config(&self) -> Vec<u8> {
        self.config.clone()
    }

    fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    fn events(&self) -> Arc<dyn EventSink> {
        self.events.clone()
    }

    /// Returns a telemetry handle scoped to this module. When the daemon
    /// built a real [`penguin_otel::OtelPipeline`] (`penguin.otel` license
    /// flag on and pipeline construction succeeded — see
    /// `bins/penguind/src/daemon_main.rs`), this is a live OTLP-backed
    /// handle; otherwise it's the [`penguin_sdk::NoopTelemetry`] handle the
    /// trait default would already return, made explicit here so the branch
    /// is visible at this call site rather than only in the default body.
    fn telemetry(&self) -> Arc<dyn penguin_sdk::ModuleTelemetry> {
        match &self.otel {
            Some(pipeline) => pipeline.scoped(&self.module),
            None => Arc::new(penguin_sdk::NoopTelemetry),
        }
    }
}

/// Supplies the secret store a single module sees.
///
/// Implementations MUST return a view scoped to `module`; two different
/// module names must never be able to read or overwrite each other's
/// secrets. This is what the Go reference gets from downcasting to
/// `*secrets.Store` and calling `Namespaced(moduleName)`
/// (`go-client/internal/daemon/host.go`) — making isolation a trait
/// [`DaemonHostFactory`] must call through, rather than an `Arc<dyn
/// SecretStore>` it could simply share, closes that parity gap at the type
/// level instead of relying on every caller to remember to wrap one.
///
/// `penguin-daemon` deliberately depends on no concrete secrets backend
/// (see [`DaemonHostFactory`]'s doc), so this trait is expressed purely in
/// terms of [`SecretStore`] from `penguin-sdk`. The production
/// implementation, backed by `penguin_secrets::Store::namespaced`, lives in
/// `bins/penguind/src/host_wiring.rs`.
pub trait SecretStoreProvider: Send + Sync {
    /// Returns the secret store `module` is allowed to see.
    fn store_for(&self, module: &str) -> Arc<dyn SecretStore>;
}

/// Builds a [`DaemonHost`] for every module from one shared set of daemon
/// subsystems.
///
/// `secrets` is a [`SecretStoreProvider`] rather than a bare `Arc<dyn
/// SecretStore>`: [`host_for`](DaemonHostFactory::host_for) calls
/// [`SecretStoreProvider::store_for`] once per module, so per-module secret
/// isolation is part of this type's contract instead of something a caller
/// could forget to layer on afterward. `license`, by contrast, has no
/// per-module wrapping — one shared licensing client is correct here,
/// matching the Go reference. Neither is constructed in this crate: they
/// come from `penguin-secrets`/`penguin-licensing`, and the `penguind`
/// binary supplies whatever backs them (including test doubles). `events`
/// must be the same [`crate::broker::EventBroker`] instance the daemon's
/// gRPC `WatchEvents` subscribes to — that sharing is what fixes the Go
/// double-broker bug (see the `lib.rs` module doc).
pub struct DaemonHostFactory {
    telemetry: Arc<Telemetry>,
    config_store: Arc<ConfigStore>,
    secrets: Arc<dyn SecretStoreProvider>,
    license: Arc<dyn LicenseChecker>,
    events: Arc<dyn EventSink>,
    state_dir: PathBuf,
    otel: Option<Arc<penguin_otel::OtelPipeline>>,
}

impl DaemonHostFactory {
    /// Builds a factory sharing `telemetry`, `config`, `secrets`, `license`,
    /// `events`, and `otel` across every module it constructs a host for.
    /// `secrets` still yields each module its own isolated view — see
    /// [`SecretStoreProvider`]. `otel` is `None` when telemetry export is
    /// disabled (`penguin.otel` license flag off, or pipeline construction
    /// failed) — every host built from this factory then falls back to
    /// [`penguin_sdk::NoopTelemetry`] from [`DaemonHost::telemetry`].
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        telemetry: Arc<Telemetry>,
        config: Arc<ConfigStore>,
        secrets: Arc<dyn SecretStoreProvider>,
        license: Arc<dyn LicenseChecker>,
        events: Arc<dyn EventSink>,
        state_dir: PathBuf,
        otel: Option<Arc<penguin_otel::OtelPipeline>>,
    ) -> DaemonHostFactory {
        DaemonHostFactory {
            telemetry,
            config_store: config,
            secrets,
            license,
            events,
            state_dir,
            otel,
        }
    }
}

impl HostFactory for DaemonHostFactory {
    fn host_for(&self, module: &str, schema: Option<&[u8]>) -> Arc<dyn HostServices> {
        let logger = self.telemetry.module_logger(module);
        let config = resolve_config(&self.config_store, module, schema, logger.as_ref());
        let data_dir = ensure_data_dir(&self.state_dir, module, logger.as_ref());

        Arc::new(DaemonHost {
            module: module.to_string(),
            logger: logger.clone(),
            secrets: self.secrets.store_for(module),
            license: self.license.clone(),
            metrics: self.telemetry.module_registerer(module),
            data_dir,
            events: self.events.clone(),
            config,
            otel: self.otel.clone(),
        })
    }
}

/// Resolves and schema-validates `module`'s on-disk config.
///
/// A read, parse, or validation failure is logged and treated as "no config"
/// — matching Go's `hostFactory` closure in `service.go`, which never fails
/// module construction over a bad config file; the module falls back to its
/// own defaults instead.
fn resolve_config(
    store: &ConfigStore,
    module: &str,
    schema: Option<&[u8]>,
    logger: &dyn Logger,
) -> Vec<u8> {
    match store.module_raw(module, schema) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => Vec::new(),
        Err(err) => {
            logger.warn(
                "invalid module config; module will start with defaults",
                &[("module", module), ("error", &err.to_string())],
            );
            Vec::new()
        }
    }
}

/// Creates the module's private data directory (`<state_dir>/<module>`, mode
/// 0700 on Unix).
///
/// A creation failure is logged and non-fatal, matching Go's `NewHost`: a
/// module that never touches `DataDir` is unaffected, and one that does gets a
/// clear error at first use rather than a failed load over an unrelated path.
fn ensure_data_dir(state_dir: &Path, module: &str, logger: &dyn Logger) -> PathBuf {
    let dir = state_dir.join(module);

    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    set_dir_builder_mode(&mut builder);

    if let Err(err) = builder.create(&dir) {
        logger.warn(
            "could not create module data dir",
            &[
                ("module", module),
                ("dir", &dir.display().to_string()),
                ("error", &err.to_string()),
            ],
        );
    }
    dir
}

/// Applies the 0700 owner-only mode to directories a [`std::fs::DirBuilder`]
/// creates. A no-op on non-Unix targets, where this bit pattern has no
/// equivalent (mirrors the same helper in `state.rs`).
#[cfg(unix)]
fn set_dir_builder_mode(builder: &mut std::fs::DirBuilder) {
    use std::os::unix::fs::DirBuilderExt;
    builder.mode(0o700);
}

/// Non-Unix stub for [`set_dir_builder_mode`]; see its doc.
#[cfg(not(unix))]
fn set_dir_builder_mode(_builder: &mut std::fs::DirBuilder) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use penguin_sdk::SecretError;
    use tempfile::TempDir;

    use crate::broker::EventBroker;

    /// A trivial in-memory [`SecretStore`]; used only through
    /// [`FakeSecretStoreProvider`] below, never constructed directly.
    #[derive(Default)]
    struct InMemorySecretStore {
        values: Mutex<HashMap<String, Vec<u8>>>,
    }

    #[async_trait]
    impl SecretStore for InMemorySecretStore {
        async fn get(&self, key: &str) -> Result<Vec<u8>, SecretError> {
            let values = self.values.lock().unwrap();
            values.get(key).cloned().ok_or(SecretError::NotFound)
        }
        async fn set(&self, key: &str, value: &[u8]) -> Result<(), SecretError> {
            let mut values = self.values.lock().unwrap();
            values.insert(key.to_string(), value.to_vec());
            Ok(())
        }
        async fn delete(&self, key: &str) -> Result<(), SecretError> {
            let mut values = self.values.lock().unwrap();
            let Some(_removed) = values.remove(key) else {
                return Err(SecretError::NotFound);
            };
            Ok(())
        }
    }

    /// A [`SecretStoreProvider`] test double that hands each module name its
    /// own [`InMemorySecretStore`] — modeling the same per-module isolation
    /// contract the production implementation provides over
    /// `penguin_secrets::Store::namespaced` (see `bins/penguind`'s
    /// `host_wiring.rs`), without pulling that concrete backend into this
    /// library crate's own tests.
    #[derive(Default)]
    struct FakeSecretStoreProvider {
        modules: Mutex<HashMap<String, Arc<InMemorySecretStore>>>,
    }

    impl SecretStoreProvider for FakeSecretStoreProvider {
        fn store_for(&self, module: &str) -> Arc<dyn SecretStore> {
            let mut modules = self.modules.lock().unwrap();
            let store = modules
                .entry(module.to_string())
                .or_insert_with(|| Arc::new(InMemorySecretStore::default()));
            store.clone()
        }
    }

    /// A [`LicenseChecker`] double that reports everything enabled at the
    /// free tier; host tests only care that the same instance is shared.
    struct FakeLicenseChecker;

    impl LicenseChecker for FakeLicenseChecker {
        fn feature_enabled(&self, _key: &str) -> bool {
            true
        }
        fn tier(&self) -> String {
            "free".to_string()
        }
    }

    /// Builds a factory over a fresh temp config dir and temp state dir.
    fn test_factory() -> (TempDir, TempDir, DaemonHostFactory) {
        let config_dir = TempDir::new().unwrap();
        let state_dir = TempDir::new().unwrap();
        let telemetry = Arc::new(Telemetry::new("error").unwrap());
        let config_store = Arc::new(ConfigStore::new(config_dir.path()));
        let broker: Arc<dyn EventSink> = Arc::new(EventBroker::new(4));

        let factory = DaemonHostFactory::new(
            telemetry,
            config_store,
            Arc::new(FakeSecretStoreProvider::default()),
            Arc::new(FakeLicenseChecker),
            broker,
            state_dir.path().to_path_buf(),
            None,
        );
        (config_dir, state_dir, factory)
    }

    #[test]
    fn host_for_creates_the_module_data_dir_with_owner_only_mode() {
        let (_config_dir, state_dir, factory) = test_factory();
        let host = factory.host_for("squawk", None);

        let expected = state_dir.path().join("squawk");
        assert_eq!(host.data_dir(), expected);
        assert!(expected.is_dir());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&expected).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[test]
    fn host_for_returns_empty_config_when_no_module_file_exists() {
        let (_config_dir, _state_dir, factory) = test_factory();
        let host = factory.host_for("squawk", None);
        assert!(host.config().is_empty());
    }

    #[test]
    fn host_for_returns_raw_bytes_for_a_present_unvalidated_config() {
        let (config_dir, _state_dir, factory) = test_factory();
        let modules_dir = config_dir.path().join("modules.d");
        std::fs::create_dir_all(&modules_dir).unwrap();
        std::fs::write(modules_dir.join("squawk.yaml"), "endpoint: us-east\n").unwrap();

        let host = factory.host_for("squawk", None);
        assert_eq!(host.config(), b"endpoint: us-east\n");
    }

    #[test]
    fn host_for_returns_empty_config_when_schema_validation_fails() {
        let (config_dir, _state_dir, factory) = test_factory();
        let modules_dir = config_dir.path().join("modules.d");
        std::fs::create_dir_all(&modules_dir).unwrap();
        std::fs::write(modules_dir.join("squawk.yaml"), "port: 53\n").unwrap();

        let schema = br#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"endpoint":{"type":"string"}},"required":["endpoint"]}"#;
        let host = factory.host_for("squawk", Some(schema));

        // Validation failed (missing required "endpoint"); the module falls
        // back to its own defaults rather than the load failing outright.
        assert!(host.config().is_empty());
    }

    #[test]
    fn host_for_accepts_config_matching_its_schema() {
        let (config_dir, _state_dir, factory) = test_factory();
        let modules_dir = config_dir.path().join("modules.d");
        std::fs::create_dir_all(&modules_dir).unwrap();
        std::fs::write(modules_dir.join("squawk.yaml"), "endpoint: us-east\n").unwrap();

        let schema = br#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"endpoint":{"type":"string"}},"required":["endpoint"]}"#;
        let host = factory.host_for("squawk", Some(schema));
        assert_eq!(host.config(), b"endpoint: us-east\n");
    }

    #[test]
    fn host_for_is_non_fatal_when_the_data_dir_cannot_be_created() {
        // Point state_dir at a plain file, not a directory: MkdirAll-equivalent
        // creation must fail, but host_for must still return a usable host
        // rather than panicking or erroring the whole call.
        let root = TempDir::new().unwrap();
        let blocking_file = root.path().join("not-a-dir");
        std::fs::write(&blocking_file, b"x").unwrap();

        let config_dir = TempDir::new().unwrap();
        let telemetry = Arc::new(Telemetry::new("error").unwrap());
        let config_store = Arc::new(ConfigStore::new(config_dir.path()));
        let broker: Arc<dyn EventSink> = Arc::new(EventBroker::new(4));
        let factory = DaemonHostFactory::new(
            telemetry,
            config_store,
            Arc::new(FakeSecretStoreProvider::default()),
            Arc::new(FakeLicenseChecker),
            broker,
            blocking_file.clone(),
            None,
        );

        let host = factory.host_for("squawk", None);
        assert_eq!(host.data_dir(), blocking_file.join("squawk"));
    }

    #[test]
    fn host_for_shares_the_same_injected_license_across_modules() {
        // One shared licensing client is correct here — unlike secrets, this
        // matches the Go reference, which never namespaces license checks
        // per module.
        let (_config_dir, _state_dir, factory) = test_factory();
        let host_a = factory.host_for("module-a", None);
        let host_b = factory.host_for("module-b", None);

        assert_eq!(host_a.license().tier(), host_b.license().tier());
        assert!(Arc::ptr_eq(&host_a.license(), &host_b.license()));
    }

    #[tokio::test]
    async fn host_for_gives_two_modules_isolated_secret_values_for_the_same_key() {
        let (_config_dir, _state_dir, factory) = test_factory();
        let host_a = factory.host_for("module-a", None);
        let host_b = factory.host_for("module-b", None);

        host_a
            .secrets()
            .set("api_key", b"secret-a")
            .await
            .expect("a set");
        host_b
            .secrets()
            .set("api_key", b"secret-b")
            .await
            .expect("b set");

        assert_eq!(
            host_a.secrets().get("api_key").await.expect("a get"),
            b"secret-a"
        );
        assert_eq!(
            host_b.secrets().get("api_key").await.expect("b get"),
            b"secret-b"
        );
    }

    #[tokio::test]
    async fn host_for_deleting_one_modules_secret_leaves_anothers_intact() {
        let (_config_dir, _state_dir, factory) = test_factory();
        let host_a = factory.host_for("module-a", None);
        let host_b = factory.host_for("module-b", None);

        host_a
            .secrets()
            .set("api_key", b"secret-a")
            .await
            .expect("a set");
        host_b
            .secrets()
            .set("api_key", b"secret-b")
            .await
            .expect("b set");

        host_a.secrets().delete("api_key").await.expect("a delete");

        let after_delete = host_a.secrets().get("api_key").await.unwrap_err();
        assert!(matches!(after_delete, SecretError::NotFound));
        assert_eq!(
            host_b
                .secrets()
                .get("api_key")
                .await
                .expect("b still present"),
            b"secret-b"
        );
    }

    #[tokio::test]
    async fn host_for_a_module_cannot_read_a_key_written_under_another_modules_namespace() {
        let (_config_dir, _state_dir, factory) = test_factory();
        let host_a = factory.host_for("module-a", None);
        let host_b = factory.host_for("module-b", None);

        host_b
            .secrets()
            .set("only_in_b", b"secret-b")
            .await
            .expect("b set");

        let err = host_a.secrets().get("only_in_b").await.unwrap_err();
        assert!(matches!(err, SecretError::NotFound));
    }

    #[tokio::test]
    async fn host_for_wires_events_into_the_same_shared_broker() {
        let config_dir = TempDir::new().unwrap();
        let state_dir = TempDir::new().unwrap();
        let telemetry = Arc::new(Telemetry::new("error").unwrap());
        let config_store = Arc::new(ConfigStore::new(config_dir.path()));
        let broker = Arc::new(EventBroker::new(4));
        let events: Arc<dyn EventSink> = broker.clone();

        let factory = DaemonHostFactory::new(
            telemetry,
            config_store,
            Arc::new(FakeSecretStoreProvider::default()),
            Arc::new(FakeLicenseChecker),
            events,
            state_dir.path().to_path_buf(),
            None,
        );

        let mut subscriber = broker.subscribe();
        let host = factory.host_for("squawk", None);
        host.events().publish(penguin_sdk::Event {
            module: "squawk".to_string(),
            event_type: penguin_sdk::EventType::Info,
            message: "via host".to_string(),
            at: std::time::SystemTime::now(),
            fields: HashMap::new(),
        });

        let received = subscriber.recv().await.unwrap();
        assert_eq!(received.message, "via host");
    }

    /// When the factory holds a real (enabled) [`penguin_otel::OtelPipeline`],
    /// `telemetry()` must return a handle scoped to the host's module rather
    /// than the trait's `NoopTelemetry` default. Building an *enabled*
    /// pipeline spins up OTLP/HTTP exporters that require a multi-thread
    /// Tokio runtime (see `OtelPipeline::build`'s doc) — the endpoint points
    /// at an unused localhost port, so this asserts the handle is real and
    /// safe to call, not that anything is actually delivered.
    #[tokio::test(flavor = "multi_thread")]
    async fn host_returns_scoped_telemetry_when_pipeline_present() {
        let cfg = penguin_otel::OtelConfig {
            endpoint: "http://127.0.0.1:1".to_string(),
            sampling_ratio: 1.0,
            enabled: true,
        };
        let pipeline =
            Arc::new(penguin_otel::OtelPipeline::build(&cfg, &[("node_id", "test-node")]).unwrap());

        let telemetry = Arc::new(Telemetry::new("error").unwrap());
        let config_dir = TempDir::new().unwrap();
        let state_dir = TempDir::new().unwrap();
        let config_store = Arc::new(ConfigStore::new(config_dir.path()));
        let broker: Arc<dyn EventSink> = Arc::new(EventBroker::new(4));
        let factory = DaemonHostFactory::new(
            telemetry,
            config_store,
            Arc::new(FakeSecretStoreProvider::default()),
            Arc::new(FakeLicenseChecker),
            broker,
            state_dir.path().to_path_buf(),
            Some(pipeline),
        );

        let host = factory.host_for("probe-module", None);
        let handle = host.telemetry();
        handle.counter_add("probe", 1, &[]);
        // The real assertion: proves `host_for` actually threaded `otel`
        // into this `DaemonHost` rather than silently falling back to
        // `NoopTelemetry` — `counter_add` not panicking alone can't
        // distinguish the two, since both handles are safe to call.
        assert_eq!(handle.kind(), "otel");
    }

    /// With no pipeline (`otel: None` — the license flag off, or pipeline
    /// build failed), `telemetry()` must fall back to the safe
    /// [`penguin_sdk::NoopTelemetry`] handle: calling it must never panic.
    #[test]
    fn host_returns_noop_when_flag_off() {
        let (_config_dir, _state_dir, factory) = test_factory();
        let host = factory.host_for("probe-module", None);
        let handle = host.telemetry();
        handle.record_span("x", &[]);
        assert_eq!(handle.kind(), "noop");
    }
}
