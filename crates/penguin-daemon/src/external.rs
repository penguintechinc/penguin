//! Resolves a module name the builtin registry does not recognise into a
//! live external plugin — the piece that lets `penguin load <name>` reach a
//! plugin that ships as its own signed binary instead of one compiled into
//! `penguind`.
//!
//! Two halves:
//!
//! * [`ExternalLoader`] / [`ExternalLoadError`] — the interface
//!   [`crate::supervisor::Supervisor`] drives, kept separate from any
//!   concrete implementation so the supervisor's tests can fake it (see
//!   `supervisor.rs`'s own test module) without spawning a process.
//! * [`PluginDirLoader`] / [`ExternalModule`] — the real implementation:
//!   `<plugins_dir>/<name>/plugin.json` → [`penguin_extplugin::Verifier`]'s
//!   fail-closed pipeline (ownership, world-writable, SHA256, minisign — no
//!   TOFU) → [`penguin_goplugin_host::client::PluginProcess::launch`] →
//!   `dispense()`. [`ExternalModule`] then owns that process for the
//!   module's whole lifetime and kills it in `stop()`, so the supervisor's
//!   existing stop/restart/shutdown paths — entirely unmodified — are what
//!   tear a plugin process down.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::Mutex as AsyncMutex;

use penguin_extplugin::{Verifier, load_manifest};
use penguin_goplugin_host::client::PluginProcess;
use penguin_sdk::{
    CommandResult, CommandSpec, HealthReport, HostServices, Module, ModuleError, ModuleInfo, Status,
};

/// Resolves a module name the builtin registry does not recognise into a
/// freshly-constructed, un-initialised [`Module`] — the external-plugin
/// analogue of a builtin [`penguin_sdk::Factory`]. An implementation owns
/// whatever it takes to produce that instance (plugin-directory resolution,
/// manifest parsing, signature verification, process launch); the
/// supervisor only ever sees this trait, so builtin and external modules
/// are instantiated through one uniform path.
#[async_trait]
pub trait ExternalLoader: Send + Sync {
    /// Resolves and constructs `name`. Called at most once per load or
    /// restart attempt, and only for a name the builtin registry did not
    /// already serve.
    async fn load(&self, name: &str) -> Result<Box<dyn Module>, ExternalLoadError>;
}

/// Every way [`ExternalLoader::load`] can fail, kept distinct so
/// [`crate::supervisor::Supervisor`] can map "this name doesn't exist
/// anywhere" identically for builtin and external names, while still
/// surfacing a real load failure (bad signature, launch failure, ...) as its
/// own error instead of a misleading "unknown module".
#[derive(Debug, Error)]
pub enum ExternalLoadError {
    /// No plugin exists under this name at all (e.g. its directory is
    /// absent). The supervisor folds this into the same
    /// `SupervisorError::UnknownModule` a builtin registry miss produces,
    /// so callers see one "no such module" shape regardless of where the
    /// name would have come from.
    #[error("no external plugin named {0:?}")]
    NotFound(String),
    /// The plugin exists but failed to load — a manifest, ownership,
    /// signature, or process-launch failure. A real, retryable error for a
    /// module that does exist, never conflated with [`ExternalLoadError::NotFound`].
    #[error("{0}")]
    Load(String),
}

/// What [`ExternalModule::stop`] tears down once the wire-level module has
/// been stopped: a running go-plugin child process in production, or a
/// call-counting fake in tests.
///
/// `PluginProcess` has no public constructor besides
/// [`PluginProcess::launch`], which spawns a real subprocess — so without
/// this seam, `ExternalModule`'s pure-delegation and idempotent-stop
/// behaviour could only be exercised by an integration test that launches
/// a real plugin binary. Boxing the teardown step behind a trait instead
/// lets a unit test substitute a fake that never touches a process.
#[async_trait]
trait ProcessTeardown: Send {
    /// Consumes the handle and shuts down whatever it holds.
    async fn shutdown(self: Box<Self>) -> Result<(), String>;
}

#[async_trait]
impl ProcessTeardown for PluginProcess {
    async fn shutdown(self: Box<Self>) -> Result<(), String> {
        PluginProcess::shutdown(*self)
            .await
            .map_err(|err| err.to_string())
    }
}

/// A loaded external plugin: owns the child process for the module's whole
/// lifetime and tears it down in [`Module::stop`].
///
/// `process` is `None` once torn down — that is what makes a second `stop()`
/// call the no-op [`Module::stop`]'s contract requires ("must be
/// idempotent"), and it is also why ownership has to live behind a lock at
/// all: [`ProcessTeardown::shutdown`] consumes `self`, but `Module::stop`
/// only ever gets `&self`, so the process has to be `.take()`-able out of
/// shared storage rather than moved out directly.
struct ExternalModule {
    process: AsyncMutex<Option<Box<dyn ProcessTeardown>>>,
    inner: Box<dyn Module>,
}

impl ExternalModule {
    /// Production constructor: wraps a real launched [`PluginProcess`].
    fn new(process: PluginProcess, inner: Box<dyn Module>) -> ExternalModule {
        ExternalModule::from_teardown(Box::new(process), inner)
    }

    /// Builds from an already-boxed [`ProcessTeardown`] — the seam
    /// [`ExternalModule::new`] uses for a real process, and tests use to
    /// substitute a fake.
    fn from_teardown(process: Box<dyn ProcessTeardown>, inner: Box<dyn Module>) -> ExternalModule {
        ExternalModule {
            process: AsyncMutex::new(Some(process)),
            inner,
        }
    }
}

#[async_trait]
impl Module for ExternalModule {
    fn info(&self) -> ModuleInfo {
        self.inner.info()
    }

    async fn init(&self, host: Arc<dyn HostServices>) -> Result<(), ModuleError> {
        self.inner.init(host).await
    }

    async fn start(&self) -> Result<(), ModuleError> {
        self.inner.start().await
    }

    /// Stops the module at the wire level, then unconditionally tears down
    /// its child process — even if the wire-level stop failed or the
    /// process was never fully started, the process must not survive a
    /// `stop()` call. This is the sole place an external plugin's process
    /// is ever killed; the supervisor's unload/shutdown/restart paths reach
    /// it by calling `stop()` on whatever `Arc<dyn Module>` they are
    /// holding, exactly as they already do for a builtin module.
    async fn stop(&self) -> Result<(), ModuleError> {
        if let Err(err) = self.inner.stop().await {
            tracing::warn!(
                error = %err,
                "external plugin module-level stop failed; killing its process anyway"
            );
        }
        let mut guard = self.process.lock().await;
        let Some(process) = guard.take() else {
            return Ok(());
        };
        process.shutdown().await.map_err(ModuleError::new)
    }

    async fn status(&self) -> Result<Status, ModuleError> {
        self.inner.status().await
    }

    async fn health(&self) -> HealthReport {
        self.inner.health().await
    }

    fn commands(&self) -> Vec<CommandSpec> {
        self.inner.commands()
    }

    async fn dispatch(
        &self,
        path: &[String],
        flags: &HashMap<String, String>,
        args: &[String],
    ) -> Result<CommandResult, ModuleError> {
        self.inner.dispatch(path, flags, args).await
    }

    fn config_schema(&self) -> Option<Vec<u8>> {
        self.inner.config_schema()
    }
}

/// How [`PluginDirLoader`] builds the [`Verifier`] it verifies each plugin
/// with.
///
/// `penguin_extplugin::Verifier` is itself `Send + Sync` (it holds only a
/// `Box<dyn StatSource + Send + Sync>`), so nothing here is forced by a
/// missing auto-trait bound — a single long-lived `Verifier` field would
/// compile fine. This is kept as construction parameters plus a fresh
/// `Verifier` built synchronously inside every `load` call anyway, because
/// that has a real behavioural advantage for the `Default`/`TrustedDir`
/// cases: a freshly built `Verifier` re-scans the trusted-publishers
/// directory on every load, so a key pinned there after the daemon started
/// is picked up without a restart.
enum VerifierSource {
    /// [`Verifier::new`] — trusts only [`DEFAULT_TRUSTED_PUBLISHERS_DIR`]'s
    /// `*.pub` files; no publisher key is embedded.
    Default,
    /// [`Verifier::with_trusted_dir`].
    TrustedDir(PathBuf),
    /// [`Verifier::with_keys`] — exactly these keys, nothing implicit.
    Keys(Vec<String>),
}

impl VerifierSource {
    fn build(&self) -> Verifier {
        match self {
            VerifierSource::Default => Verifier::new(),
            VerifierSource::TrustedDir(dir) => Verifier::with_trusted_dir(dir),
            VerifierSource::Keys(keys) => Verifier::with_keys(keys.clone()),
        }
    }
}

/// The production [`ExternalLoader`]: resolves `<plugins_dir>/<name>/`,
/// verifies it, launches it, and wraps the result so its process is torn
/// down on `stop()`.
pub struct PluginDirLoader {
    plugins_dir: PathBuf,
    socket_dir: PathBuf,
    verifier_source: VerifierSource,
    daemon_uid: u32,
}

impl PluginDirLoader {
    /// Builds a loader scanning `plugins_dir` for `<name>/plugin.json`
    /// directories, verifying each against [`Verifier::new`]'s system
    /// trusted-publishers directory scan — the production configuration.
    /// `daemon_uid` is the uid plugin files must be owned by
    /// (besides root); production wiring passes the daemon's own uid (see
    /// `bins/penguind`), keeping this loader itself privilege-free to
    /// construct.
    ///
    /// `socket_dir` is where the go-plugin broker would create secondary
    /// unix sockets — unused today since every `load` passes `host_routes:
    /// None` (no built-in module currently needs to receive callbacks from
    /// an external plugin), but required by
    /// [`PluginProcess::launch`]'s signature regardless.
    pub fn new(plugins_dir: PathBuf, socket_dir: PathBuf, daemon_uid: u32) -> PluginDirLoader {
        PluginDirLoader {
            plugins_dir,
            socket_dir,
            verifier_source: VerifierSource::Default,
            daemon_uid,
        }
    }

    /// Same as [`PluginDirLoader::new`], but verifies against
    /// [`Verifier::with_trusted_dir`] instead of the system path — lets a
    /// deployment point at its own trusted-publishers directory.
    pub fn with_trusted_dir(
        plugins_dir: PathBuf,
        socket_dir: PathBuf,
        daemon_uid: u32,
        trusted_dir: PathBuf,
    ) -> PluginDirLoader {
        PluginDirLoader {
            plugins_dir,
            socket_dir,
            verifier_source: VerifierSource::TrustedDir(trusted_dir),
            daemon_uid,
        }
    }

    /// Same as [`PluginDirLoader::new`], but verifies against exactly
    /// `trusted_public_keys` via [`Verifier::with_keys`] — no directory
    /// scan, nothing implicit. For tests that inject a fixture's own
    /// keypair.
    pub fn with_keys(
        plugins_dir: PathBuf,
        socket_dir: PathBuf,
        daemon_uid: u32,
        trusted_public_keys: Vec<String>,
    ) -> PluginDirLoader {
        PluginDirLoader {
            plugins_dir,
            socket_dir,
            verifier_source: VerifierSource::Keys(trusted_public_keys),
            daemon_uid,
        }
    }
}

#[async_trait]
impl ExternalLoader for PluginDirLoader {
    async fn load(&self, name: &str) -> Result<Box<dyn Module>, ExternalLoadError> {
        let plugin_dir = self.plugins_dir.join(name);
        if !plugin_dir.is_dir() {
            return Err(ExternalLoadError::NotFound(name.to_string()));
        }

        let manifest =
            load_manifest(&plugin_dir).map_err(|err| ExternalLoadError::Load(err.to_string()))?;
        // Scoped to a block so the `Verifier` (needed only for this one
        // check) is dropped before the process launch below. Not required
        // for `Send` purposes — `Verifier` is `Send + Sync` itself — just
        // keeps its lifetime visibly tied to the check it exists for.
        {
            let verifier = self.verifier_source.build();
            verifier
                .verify(&plugin_dir, &manifest, self.daemon_uid)
                .map_err(|err| ExternalLoadError::Load(err.to_string()))?;
        }

        let binary_path = manifest.binary_path(&plugin_dir);
        let process = PluginProcess::launch(&binary_path, &self.socket_dir, None)
            .await
            .map_err(|err| ExternalLoadError::Load(err.to_string()))?;

        let inner = match process.dispense().await {
            Ok(inner) => inner,
            Err(err) => {
                // The process launched and passed its health check, but its
                // ModuleService could not be dispensed — it must not be
                // left running with nothing left to supervise it.
                let _ = process.shutdown().await;
                return Err(ExternalLoadError::Load(err.to_string()));
            }
        };

        Ok(Box::new(ExternalModule::new(process, inner)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use penguin_sdk::{
        Event, EventSink, LicenseChecker, Logger, Metrics, SecretError, SecretStore,
    };
    use penguin_telemetry::Telemetry;
    use tempfile::tempdir;

    use super::*;

    /// The `(path, flags, args)` a fake `dispatch` call recorded.
    type DispatchArgs = (Vec<String>, HashMap<String, String>, Vec<String>);

    /// Shared call counters for [`FakeInnerModule`], read by a test after
    /// the module itself has been moved into a `Box<dyn Module>` and is no
    /// longer directly reachable.
    struct FakeInnerCalls {
        init: Arc<AtomicUsize>,
        start: Arc<AtomicUsize>,
        stop: Arc<AtomicUsize>,
        status: Arc<AtomicUsize>,
        health: Arc<AtomicUsize>,
        dispatch: Arc<AtomicUsize>,
        last_dispatch: Arc<Mutex<Option<DispatchArgs>>>,
    }

    /// Fake [`Module`] used to test [`ExternalModule`]'s pure-delegation
    /// behaviour without a real plugin process. Every method increments a
    /// counter (shared with the test via [`FakeInnerCalls`]) and returns a
    /// plain configured field — never a closure — so a test can assert both
    /// "delegated exactly once" and "the result/error came back unchanged".
    struct FakeInnerModule {
        info: ModuleInfo,
        init_result: Result<(), ModuleError>,
        start_result: Result<(), ModuleError>,
        stop_result: Result<(), ModuleError>,
        status_result: Result<Status, ModuleError>,
        health_report: HealthReport,
        commands: Vec<CommandSpec>,
        dispatch_result: Result<CommandResult, ModuleError>,
        config_schema: Option<Vec<u8>>,
        calls: FakeInnerCalls,
    }

    impl FakeInnerModule {
        /// Builds a fake that succeeds everywhere with empty/default
        /// values, plus the counters handle a test keeps after boxing it.
        fn new() -> (FakeInnerModule, FakeInnerCalls) {
            let calls = FakeInnerCalls {
                init: Arc::new(AtomicUsize::new(0)),
                start: Arc::new(AtomicUsize::new(0)),
                stop: Arc::new(AtomicUsize::new(0)),
                status: Arc::new(AtomicUsize::new(0)),
                health: Arc::new(AtomicUsize::new(0)),
                dispatch: Arc::new(AtomicUsize::new(0)),
                last_dispatch: Arc::new(Mutex::new(None)),
            };
            let handle = FakeInnerCalls {
                init: calls.init.clone(),
                start: calls.start.clone(),
                stop: calls.stop.clone(),
                status: calls.status.clone(),
                health: calls.health.clone(),
                dispatch: calls.dispatch.clone(),
                last_dispatch: calls.last_dispatch.clone(),
            };
            let module = FakeInnerModule {
                info: ModuleInfo::default(),
                init_result: Ok(()),
                start_result: Ok(()),
                stop_result: Ok(()),
                status_result: Ok(Status::default()),
                health_report: HealthReport::default(),
                commands: Vec::new(),
                dispatch_result: Ok(CommandResult::default()),
                config_schema: None,
                calls,
            };
            (module, handle)
        }

        fn with_info(mut self, info: ModuleInfo) -> FakeInnerModule {
            self.info = info;
            self
        }

        fn with_init_result(mut self, result: Result<(), ModuleError>) -> FakeInnerModule {
            self.init_result = result;
            self
        }

        fn with_start_result(mut self, result: Result<(), ModuleError>) -> FakeInnerModule {
            self.start_result = result;
            self
        }

        fn with_stop_result(mut self, result: Result<(), ModuleError>) -> FakeInnerModule {
            self.stop_result = result;
            self
        }

        fn with_status_result(mut self, result: Result<Status, ModuleError>) -> FakeInnerModule {
            self.status_result = result;
            self
        }

        fn with_health_report(mut self, report: HealthReport) -> FakeInnerModule {
            self.health_report = report;
            self
        }

        fn with_commands(mut self, commands: Vec<CommandSpec>) -> FakeInnerModule {
            self.commands = commands;
            self
        }

        fn with_dispatch_result(
            mut self,
            result: Result<CommandResult, ModuleError>,
        ) -> FakeInnerModule {
            self.dispatch_result = result;
            self
        }

        fn with_config_schema(mut self, schema: Option<Vec<u8>>) -> FakeInnerModule {
            self.config_schema = schema;
            self
        }
    }

    #[async_trait]
    impl Module for FakeInnerModule {
        fn info(&self) -> ModuleInfo {
            self.info.clone()
        }

        async fn init(&self, _host: Arc<dyn HostServices>) -> Result<(), ModuleError> {
            self.calls.init.fetch_add(1, Ordering::SeqCst);
            self.init_result.clone()
        }

        async fn start(&self) -> Result<(), ModuleError> {
            self.calls.start.fetch_add(1, Ordering::SeqCst);
            self.start_result.clone()
        }

        async fn stop(&self) -> Result<(), ModuleError> {
            self.calls.stop.fetch_add(1, Ordering::SeqCst);
            self.stop_result.clone()
        }

        async fn status(&self) -> Result<Status, ModuleError> {
            self.calls.status.fetch_add(1, Ordering::SeqCst);
            self.status_result.clone()
        }

        async fn health(&self) -> HealthReport {
            self.calls.health.fetch_add(1, Ordering::SeqCst);
            self.health_report.clone()
        }

        fn commands(&self) -> Vec<CommandSpec> {
            self.commands.clone()
        }

        async fn dispatch(
            &self,
            path: &[String],
            flags: &HashMap<String, String>,
            args: &[String],
        ) -> Result<CommandResult, ModuleError> {
            self.calls.dispatch.fetch_add(1, Ordering::SeqCst);
            let mut last = self.calls.last_dispatch.lock().unwrap();
            *last = Some((path.to_vec(), flags.clone(), args.to_vec()));
            self.dispatch_result.clone()
        }

        fn config_schema(&self) -> Option<Vec<u8>> {
            self.config_schema.clone()
        }
    }

    /// Fake [`ProcessTeardown`]: records how many times `shutdown` ran
    /// (shared with the test via the cloned `Arc`) and returns a
    /// configured result, without touching any real process.
    struct FakeProcessTeardown {
        calls: Arc<AtomicUsize>,
        result: Result<(), String>,
    }

    #[async_trait]
    impl ProcessTeardown for FakeProcessTeardown {
        async fn shutdown(self: Box<Self>) -> Result<(), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.clone()
        }
    }

    /// A no-op [`SecretStore`]; [`FakeInnerModule::init`] never calls it —
    /// it only needs to type-check as the `Arc<dyn HostServices>` argument.
    struct NoopSecretStore;
    #[async_trait]
    impl SecretStore for NoopSecretStore {
        async fn get(&self, _key: &str) -> Result<Vec<u8>, SecretError> {
            Err(SecretError::NotFound)
        }
        async fn set(&self, _key: &str, _value: &[u8]) -> Result<(), SecretError> {
            Ok(())
        }
        async fn delete(&self, _key: &str) -> Result<(), SecretError> {
            Err(SecretError::NotFound)
        }
    }

    /// A no-op [`LicenseChecker`]; unused by [`FakeInnerModule::init`].
    struct NoopLicenseChecker;
    impl LicenseChecker for NoopLicenseChecker {
        fn feature_enabled(&self, _key: &str) -> bool {
            false
        }
        fn tier(&self) -> String {
            "free".to_string()
        }
    }

    /// A no-op [`EventSink`]; unused by [`FakeInnerModule::init`].
    struct NoopEventSink;
    impl EventSink for NoopEventSink {
        fn publish(&self, _event: Event) {}
    }

    /// Minimal [`HostServices`] double: enough to satisfy
    /// `ExternalModule::init`'s signature. [`FakeInnerModule::init`] never
    /// calls any of these accessors, so most of them just need to
    /// type-check; `logger`/`metrics` are borrowed from a real
    /// [`Telemetry`] instance instead of hand-rolled, since a real `Metrics`
    /// impl would otherwise require this crate to depend on `prometheus`
    /// directly just for a test double.
    struct FakeHostServices {
        logger: Arc<dyn Logger>,
        metrics: Arc<dyn Metrics>,
    }

    /// Builds a [`FakeHostServices`] behind the `Arc<dyn HostServices>`
    /// shape `ExternalModule::init` expects. A free function rather than an
    /// associated `fn new` returning something other than `Self`.
    fn fake_host_services() -> Arc<dyn HostServices> {
        let telemetry = Telemetry::new("error").expect("telemetry");
        Arc::new(FakeHostServices {
            logger: telemetry.module_logger("external-module-test"),
            metrics: telemetry.module_registerer("external-module-test"),
        })
    }

    impl HostServices for FakeHostServices {
        fn logger(&self) -> Arc<dyn Logger> {
            self.logger.clone()
        }
        fn secrets(&self) -> Arc<dyn SecretStore> {
            Arc::new(NoopSecretStore)
        }
        fn license(&self) -> Arc<dyn LicenseChecker> {
            Arc::new(NoopLicenseChecker)
        }
        fn metrics(&self) -> Arc<dyn Metrics> {
            self.metrics.clone()
        }
        fn config(&self) -> Vec<u8> {
            Vec::new()
        }
        fn data_dir(&self) -> PathBuf {
            PathBuf::new()
        }
        fn events(&self) -> Arc<dyn EventSink> {
            Arc::new(NoopEventSink)
        }
    }

    /// Builds an [`ExternalModule`] from a fresh, always-succeeding
    /// [`FakeProcessTeardown`] and the given fake inner module — the
    /// common case for delegation tests that don't care about teardown.
    fn module_with_teardown_calls(inner: FakeInnerModule) -> (ExternalModule, Arc<AtomicUsize>) {
        let teardown_calls = Arc::new(AtomicUsize::new(0));
        let teardown = FakeProcessTeardown {
            calls: teardown_calls.clone(),
            result: Ok(()),
        };
        let module = ExternalModule::from_teardown(Box::new(teardown), Box::new(inner));
        (module, teardown_calls)
    }

    #[test]
    fn info_delegates_to_inner() {
        let info = ModuleInfo {
            name: "widget".to_string(),
            version: "1.2.3".to_string(),
            description: "a fake widget module".to_string(),
            license_feature: "penguin.widget".to_string(),
        };
        let (inner, _calls) = FakeInnerModule::new();
        let (module, _teardown) = module_with_teardown_calls(inner.with_info(info.clone()));

        assert_eq!(module.info(), info);
    }

    #[tokio::test]
    async fn init_delegates_to_inner_and_forwards_ok() {
        let (inner, calls) = FakeInnerModule::new();
        let (module, _teardown) = module_with_teardown_calls(inner);

        let result = module.init(fake_host_services()).await;

        assert!(result.is_ok());
        assert_eq!(calls.init.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn init_delegates_to_inner_and_forwards_error() {
        let (inner, calls) = FakeInnerModule::new();
        let inner = inner.with_init_result(Err(ModuleError::new("init boom")));
        let (module, _teardown) = module_with_teardown_calls(inner);

        let err = module
            .init(fake_host_services())
            .await
            .expect_err("init error must be forwarded");

        assert_eq!(err, ModuleError::new("init boom"));
        assert_eq!(calls.init.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn start_delegates_to_inner_and_forwards_ok() {
        let (inner, calls) = FakeInnerModule::new();
        let (module, _teardown) = module_with_teardown_calls(inner);

        let result = module.start().await;

        assert!(result.is_ok());
        assert_eq!(calls.start.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn start_delegates_to_inner_and_forwards_error() {
        let (inner, calls) = FakeInnerModule::new();
        let inner = inner.with_start_result(Err(ModuleError::new("start boom")));
        let (module, _teardown) = module_with_teardown_calls(inner);

        let err = module.start().await.expect_err("start error must forward");

        assert_eq!(err, ModuleError::new("start boom"));
        assert_eq!(calls.start.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn status_delegates_to_inner_and_forwards_result() {
        let mut detail = HashMap::new();
        detail.insert("endpoint".to_string(), "us-east".to_string());
        let status = Status {
            state: penguin_sdk::ModuleState::Running,
            detail,
        };
        let (inner, calls) = FakeInnerModule::new();
        let inner = inner.with_status_result(Ok(status.clone()));
        let (module, _teardown) = module_with_teardown_calls(inner);

        let got = module.status().await.expect("status forwarded");

        assert_eq!(got, status);
        assert_eq!(calls.status.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn health_delegates_to_inner() {
        let report = HealthReport {
            level: penguin_sdk::HealthLevel::Degraded,
            message: "half broken".to_string(),
            checked_at: std::time::SystemTime::UNIX_EPOCH,
        };
        let (inner, calls) = FakeInnerModule::new();
        let inner = inner.with_health_report(report.clone());
        let (module, _teardown) = module_with_teardown_calls(inner);

        let got = module.health().await;

        assert_eq!(got, report);
        assert_eq!(calls.health.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn commands_delegates_to_inner() {
        let commands = vec![CommandSpec {
            name: "connect".to_string(),
            ..Default::default()
        }];
        let (inner, _calls) = FakeInnerModule::new();
        let (module, _teardown) = module_with_teardown_calls(inner.with_commands(commands.clone()));

        assert_eq!(module.commands(), commands);
    }

    #[test]
    fn config_schema_delegates_to_inner() {
        let schema = Some(br#"{"type":"object"}"#.to_vec());
        let (inner, _calls) = FakeInnerModule::new();
        let (module, _teardown) =
            module_with_teardown_calls(inner.with_config_schema(schema.clone()));

        assert_eq!(module.config_schema(), schema);
    }

    #[tokio::test]
    async fn dispatch_delegates_to_inner_with_path_flags_and_args() {
        let result = CommandResult {
            output: "connected".to_string(),
            json: Vec::new(),
            exit_code: 0,
        };
        let (inner, calls) = FakeInnerModule::new();
        let inner = inner.with_dispatch_result(Ok(result.clone()));
        let (module, _teardown) = module_with_teardown_calls(inner);

        let path = vec!["squawk".to_string(), "connect".to_string()];
        let mut flags = HashMap::new();
        flags.insert("endpoint".to_string(), "us-east".to_string());
        let args = vec!["extra-arg".to_string()];

        let got = module
            .dispatch(&path, &flags, &args)
            .await
            .expect("dispatch forwarded");

        assert_eq!(got, result);
        assert_eq!(calls.dispatch.load(Ordering::SeqCst), 1);
        let recorded = calls.last_dispatch.lock().unwrap().clone().unwrap();
        assert_eq!(recorded, (path, flags, args));
    }

    #[tokio::test]
    async fn stop_calls_inner_stop_then_tears_down_process_exactly_once() {
        let (inner, calls) = FakeInnerModule::new();
        let (module, teardown_calls) = module_with_teardown_calls(inner);

        let result = module.stop().await;

        assert!(result.is_ok());
        assert_eq!(calls.stop.load(Ordering::SeqCst), 1);
        assert_eq!(teardown_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stop_tears_down_process_even_when_inner_stop_errors() {
        // The wire-level stop failing must not leave the process running —
        // only logged, per `Module::stop`'s doc comment on `ExternalModule`.
        let (inner, calls) = FakeInnerModule::new();
        let inner = inner.with_stop_result(Err(ModuleError::new("wire stop failed")));
        let (module, teardown_calls) = module_with_teardown_calls(inner);

        let result = module.stop().await;

        assert!(result.is_ok(), "inner stop error must not fail stop()");
        assert_eq!(calls.stop.load(Ordering::SeqCst), 1);
        assert_eq!(teardown_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn second_stop_call_is_a_noop_process_teardown_happens_once() {
        let (inner, calls) = FakeInnerModule::new();
        let (module, teardown_calls) = module_with_teardown_calls(inner);

        let first = module.stop().await;
        let second = module.stop().await;

        assert!(first.is_ok());
        assert!(second.is_ok(), "a second stop() must still succeed");
        // The wire-level inner.stop() is still invoked each call (its own
        // idempotency is the inner module's concern) — what must be
        // idempotent here is process teardown, exactly once.
        assert_eq!(calls.stop.load(Ordering::SeqCst), 2);
        assert_eq!(
            teardown_calls.load(Ordering::SeqCst),
            1,
            "process teardown must not run twice"
        );
    }

    #[tokio::test]
    async fn stop_propagates_a_teardown_error() {
        let (inner, _calls) = FakeInnerModule::new();
        let teardown_calls = Arc::new(AtomicUsize::new(0));
        let teardown = FakeProcessTeardown {
            calls: teardown_calls.clone(),
            result: Err("kill failed".to_string()),
        };
        let module = ExternalModule::from_teardown(Box::new(teardown), Box::new(inner));

        let err = module
            .stop()
            .await
            .expect_err("teardown error must propagate");

        assert_eq!(err, ModuleError::new("kill failed"));
        assert_eq!(teardown_calls.load(Ordering::SeqCst), 1);
    }

    /// The uid that owns freshly-created files under `path` — matches
    /// `PluginDirLoader`'s `daemon_uid` parameter to the test process's own
    /// uid, since the ownership check in `penguin_extplugin::Verifier` runs
    /// against the real filesystem here (no `StatSource` fake is plumbed
    /// through `PluginDirLoader`).
    #[cfg(unix)]
    fn owner_uid_of(path: &std::path::Path) -> u32 {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).expect("stat for uid").uid()
    }

    #[tokio::test]
    async fn load_returns_not_found_when_plugin_dir_is_missing() {
        let tmp = tempdir().expect("tempdir");
        let loader = PluginDirLoader::with_keys(
            tmp.path().to_path_buf(),
            tmp.path().join("sockets"),
            0,
            Vec::new(),
        );

        // `.expect_err()`/`.unwrap_err()` both require the `Ok` type to be
        // `Debug` (to print it if the result was unexpectedly `Ok`) — but
        // `Box<dyn Module>` isn't, so every test in this group matches the
        // `Result` directly instead.
        let Err(err) = loader.load("does-not-exist").await else {
            panic!("missing plugin dir must be NotFound");
        };

        let ExternalLoadError::NotFound(name) = err else {
            panic!("expected NotFound, got {err:?}");
        };
        assert_eq!(name, "does-not-exist");
    }

    #[tokio::test]
    async fn load_returns_load_error_when_manifest_file_is_absent() {
        let tmp = tempdir().expect("tempdir");
        let plugin_dir = tmp.path().join("myplugin");
        std::fs::create_dir(&plugin_dir).expect("mkdir plugin dir");
        // Deliberately no plugin.json written.
        let uid = owner_uid_of(&plugin_dir);
        let loader = PluginDirLoader::with_keys(
            tmp.path().to_path_buf(),
            tmp.path().join("sockets"),
            uid,
            Vec::new(),
        );

        let Err(err) = loader.load("myplugin").await else {
            panic!("absent manifest must fail to load");
        };

        let ExternalLoadError::Load(message) = err else {
            panic!("expected Load, got {err:?}");
        };
        assert!(message.contains("plugin.json"), "message was: {message}");
    }

    #[tokio::test]
    async fn load_returns_load_error_when_manifest_is_missing_a_required_field() {
        let tmp = tempdir().expect("tempdir");
        let plugin_dir = tmp.path().join("myplugin");
        std::fs::create_dir(&plugin_dir).expect("mkdir plugin dir");
        std::fs::write(
            plugin_dir.join("plugin.json"),
            r#"{"name":"myplugin","binary":"bin"}"#,
        )
        .expect("write plugin.json missing sha256");
        let uid = owner_uid_of(&plugin_dir);
        let loader = PluginDirLoader::with_keys(
            tmp.path().to_path_buf(),
            tmp.path().join("sockets"),
            uid,
            Vec::new(),
        );

        let Err(err) = loader.load("myplugin").await else {
            panic!("manifest missing sha256 must fail to load");
        };

        let ExternalLoadError::Load(message) = err else {
            panic!("expected Load, got {err:?}");
        };
        assert!(message.contains("sha256"), "message was: {message}");
    }

    #[tokio::test]
    async fn load_refuses_binary_before_launch_when_sha256_mismatches() {
        let tmp = tempdir().expect("tempdir");
        let plugin_dir = tmp.path().join("myplugin");
        std::fs::create_dir(&plugin_dir).expect("mkdir plugin dir");
        std::fs::write(
            plugin_dir.join("plugin.json"),
            r#"{"name":"myplugin","binary":"bin","sha256":"0000000000000000000000000000000000000000000000000000000000000000"}"#,
        )
        .expect("write plugin.json");
        std::fs::write(plugin_dir.join("bin"), b"actual binary content").expect("write binary");
        // Deliberately no .minisig file: a sha256 mismatch must be caught
        // before verification ever reaches the signature step, so its
        // absence never matters here.
        let uid = owner_uid_of(&plugin_dir);
        let loader = PluginDirLoader::with_keys(
            tmp.path().to_path_buf(),
            tmp.path().join("sockets"),
            uid,
            Vec::new(),
        );

        let Err(err) = loader.load("myplugin").await else {
            panic!("sha256 mismatch must refuse the plugin before launch");
        };

        let ExternalLoadError::Load(message) = err else {
            panic!("expected Load, got {err:?}");
        };
        assert!(
            message.contains("sha256 mismatch"),
            "message was: {message}"
        );
    }
}
