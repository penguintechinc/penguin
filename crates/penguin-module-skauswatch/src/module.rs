//! The SkausWatch `penguin_sdk::Module` implementation: lifecycle glue.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;

use penguin_sdk::{
    CommandResult, CommandSpec, HealthLevel, HealthReport, HostServices, Module, ModuleError,
    ModuleInfo, ModuleState, Status,
};

use crate::config::ModuleConfig;
use crate::metrics::SkausWatchMetrics;

/// The module's real state, held behind an `Arc` so background tasks can clone a handle.
struct Inner {
    host: OnceLock<Arc<dyn HostServices>>,
    running: AtomicBool,
}

impl Inner {
    const UNINITIALISED: &'static str = "skauswatch module used before init";

    fn new() -> Inner {
        Inner {
            host: OnceLock::new(),
            running: AtomicBool::new(false),
        }
    }
}

/// SkausWatch: a monitoring and alerting endpoint client.
///
/// A cheap `Clone` (an `Arc` around its real state).
#[derive(Clone)]
pub struct SkausWatchModule {
    inner: Arc<Inner>,
}

impl Default for SkausWatchModule {
    fn default() -> SkausWatchModule {
        SkausWatchModule::new()
    }
}

impl SkausWatchModule {
    /// Builds a fresh, un-initialised module.
    pub fn new() -> SkausWatchModule {
        SkausWatchModule {
            inner: Arc::new(Inner::new()),
        }
    }

    pub(crate) fn host(&self) -> &Arc<dyn HostServices> {
        self.inner.host.get().expect(Inner::UNINITIALISED)
    }

    fn is_running(&self) -> bool {
        self.inner.running.load(Ordering::SeqCst)
    }
}

/// Builds a fresh, un-initialised [`SkausWatchModule`] — the
/// [`penguin_sdk::Factory`] registered for the built-in `"skauswatch"`
/// module.
pub fn factory() -> Box<dyn Module> {
    Box::new(SkausWatchModule::new())
}

#[async_trait]
impl Module for SkausWatchModule {
    /// SkausWatch is core product and ships in the Free tier.
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "skauswatch".to_string(),
            version: "1.0.0".to_string(),
            description: "Monitoring and alerting endpoint client".to_string(),
            license_feature: String::new(),
        }
    }

    /// Resolves config and registers metrics. No background work yet.
    async fn init(&self, host: Arc<dyn HostServices>) -> Result<(), ModuleError> {
        let logger = host.logger();

        let raw = host.config();
        let mut cfg = ModuleConfig::default();
        if !raw.is_empty() {
            cfg = serde_norway::from_slice(&raw)
                .map_err(|err| ModuleError::new(format!("failed to parse config: {err}")))?;
        }

        if cfg.base_url.is_empty() {
            return Err(ModuleError::new("base_url is required"));
        }

        logger.info(
            "skauswatch config loaded",
            &[("base_url", cfg.base_url.as_str())],
        );

        let _metrics = SkausWatchMetrics::register(host.metrics().as_ref())
            .map_err(|err| ModuleError::new(format!("register metrics: {err}")))?;

        logger.info("skauswatch module initialized", &[]);

        let _ = self.inner.host.set(host);

        Ok(())
    }

    /// Starts background work (stub for now).
    async fn start(&self) -> Result<(), ModuleError> {
        if self.inner.running.swap(true, Ordering::SeqCst) {
            return Err(ModuleError::new("module already running"));
        }

        self.host().logger().info("starting skauswatch module", &[]);

        Ok(())
    }

    /// Stops background work (stub for now).
    async fn stop(&self) -> Result<(), ModuleError> {
        if !self.inner.running.swap(false, Ordering::SeqCst) {
            return Ok(());
        }

        self.host().logger().info("stopping skauswatch module", &[]);

        Ok(())
    }

    /// Reports the module's running state.
    async fn status(&self) -> Result<Status, ModuleError> {
        let state = if self.is_running() {
            ModuleState::Running
        } else {
            ModuleState::Disabled
        };

        let detail = HashMap::new();

        Ok(Status { state, detail })
    }

    /// Returns a healthy status (scaffold).
    async fn health(&self) -> HealthReport {
        HealthReport {
            level: HealthLevel::Healthy,
            message: "module is healthy".to_string(),
            checked_at: std::time::SystemTime::now(),
        }
    }

    /// No commands yet (scaffold).
    fn commands(&self) -> Vec<CommandSpec> {
        vec![]
    }

    /// Dispatch returns "unknown command" (scaffold).
    async fn dispatch(
        &self,
        _path: &[String],
        _flags: &HashMap<String, String>,
        _args: &[String],
    ) -> Result<CommandResult, ModuleError> {
        Err(ModuleError::new("unknown command"))
    }

    /// Returns the config schema.
    fn config_schema(&self) -> Option<Vec<u8>> {
        Some(crate::config::CONFIG_SCHEMA.as_bytes().to_vec())
    }
}
