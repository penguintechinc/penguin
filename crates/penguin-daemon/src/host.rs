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
    logger: Arc<dyn Logger>,
    secrets: Arc<dyn SecretStore>,
    license: Arc<dyn LicenseChecker>,
    metrics: Arc<dyn Metrics>,
    data_dir: PathBuf,
    events: Arc<dyn EventSink>,
    config: Vec<u8>,
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
}

/// Builds a [`DaemonHost`] for every module from one shared set of daemon
/// subsystems.
///
/// `secrets` and `license` are injected rather than constructed here: they
/// come from `penguin-secrets`/`penguin-licensing`, which land in M4, and the
/// `penguind` binary supplies whatever backs them (including test doubles).
/// `events` must be the same [`crate::broker::EventBroker`] instance the
/// daemon's gRPC `WatchEvents` subscribes to — that sharing is what fixes the
/// Go double-broker bug (see the `lib.rs` module doc).
pub struct DaemonHostFactory {
    telemetry: Arc<Telemetry>,
    config_store: Arc<ConfigStore>,
    secrets: Arc<dyn SecretStore>,
    license: Arc<dyn LicenseChecker>,
    events: Arc<dyn EventSink>,
    state_dir: PathBuf,
}

impl DaemonHostFactory {
    /// Builds a factory sharing `telemetry`, `config`, `secrets`, `license`,
    /// and `events` across every module it constructs a host for.
    pub fn new(
        telemetry: Arc<Telemetry>,
        config: Arc<ConfigStore>,
        secrets: Arc<dyn SecretStore>,
        license: Arc<dyn LicenseChecker>,
        events: Arc<dyn EventSink>,
        state_dir: PathBuf,
    ) -> DaemonHostFactory {
        DaemonHostFactory {
            telemetry,
            config_store: config,
            secrets,
            license,
            events,
            state_dir,
        }
    }
}

impl HostFactory for DaemonHostFactory {
    fn host_for(&self, module: &str, schema: Option<&[u8]>) -> Arc<dyn HostServices> {
        let logger = self.telemetry.module_logger(module);
        let config = resolve_config(&self.config_store, module, schema, logger.as_ref());
        let data_dir = ensure_data_dir(&self.state_dir, module, logger.as_ref());

        Arc::new(DaemonHost {
            logger: logger.clone(),
            secrets: self.secrets.clone(),
            license: self.license.clone(),
            metrics: self.telemetry.module_registerer(module),
            data_dir,
            events: self.events.clone(),
            config,
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

    use async_trait::async_trait;
    use penguin_sdk::SecretError;
    use tempfile::TempDir;

    use crate::broker::EventBroker;

    /// A [`SecretStore`] double that always reports "not found"; host tests
    /// only need to prove the same instance flows through unchanged, never
    /// exercise real secret storage.
    struct FakeSecretStore;

    #[async_trait]
    impl SecretStore for FakeSecretStore {
        async fn get(&self, _key: &str) -> Result<Vec<u8>, SecretError> {
            Err(SecretError::NotFound)
        }
        async fn set(&self, _key: &str, _value: &[u8]) -> Result<(), SecretError> {
            Ok(())
        }
        async fn delete(&self, _key: &str) -> Result<(), SecretError> {
            Ok(())
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
            Arc::new(FakeSecretStore),
            Arc::new(FakeLicenseChecker),
            broker,
            state_dir.path().to_path_buf(),
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
            Arc::new(FakeSecretStore),
            Arc::new(FakeLicenseChecker),
            broker,
            blocking_file.clone(),
        );

        let host = factory.host_for("squawk", None);
        assert_eq!(host.data_dir(), blocking_file.join("squawk"));
    }

    #[test]
    fn host_for_shares_the_same_injected_secrets_and_license_across_modules() {
        let (_config_dir, _state_dir, factory) = test_factory();
        let a = factory.host_for("a", None);
        let b = factory.host_for("b", None);

        assert_eq!(a.license().tier(), b.license().tier());
        assert!(Arc::ptr_eq(&a.secrets(), &b.secrets()));
        assert!(Arc::ptr_eq(&a.license(), &b.license()));
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
            Arc::new(FakeSecretStore),
            Arc::new(FakeLicenseChecker),
            events,
            state_dir.path().to_path_buf(),
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
}
