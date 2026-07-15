//! The lifecycle and command contract every product module implements.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::command::{CommandResult, CommandSpec};
use crate::error::ModuleError;
use crate::host::HostServices;
use crate::status::{HealthReport, Status};

/// The contract the daemon supervisor drives every module through.
///
/// All methods must be safe for concurrent use, so the trait takes `&self`
/// throughout and modules use interior mutability (e.g. a `OnceLock` for the
/// host handle) rather than `&mut self`. `start` must return promptly: modules
/// own their background tasks and shut them down in `stop`.
#[async_trait]
pub trait Module: Send + Sync {
    /// Returns static identity metadata. Must be callable before `init`.
    fn info(&self) -> ModuleInfo;

    /// Prepares the module with host-provided services. Called exactly once
    /// before `start` and must not begin any background work.
    async fn init(&self, host: Arc<dyn HostServices>) -> Result<(), ModuleError>;

    /// Begins the module's work and returns promptly.
    async fn start(&self) -> Result<(), ModuleError>;

    /// Halts all work and restores any system state the module changed
    /// (resolver settings, tunnels, ...). Must be idempotent.
    async fn stop(&self) -> Result<(), ModuleError>;

    /// Reports the module's current operational state.
    async fn status(&self) -> Result<Status, ModuleError>;

    /// A cheap liveness/degradation probe used by the tray and `penguin status`.
    async fn health(&self) -> HealthReport;

    /// Declares the module's CLI command tree as pure data.
    fn commands(&self) -> Vec<CommandSpec>;

    /// Executes the command at `path` with parsed `flags` and positional
    /// `args`. The single entry point for all module CLI invocations.
    async fn dispatch(
        &self,
        path: &[String],
        flags: &HashMap<String, String>,
        args: &[String],
    ) -> Result<CommandResult, ModuleError>;

    /// Returns the JSON Schema the daemon validates the module's config file
    /// against, or `None` when the module takes no configuration.
    fn config_schema(&self) -> Option<Vec<u8>>;
}

/// Constructs a fresh, un-initialised [`Module`].
///
/// Registered in the built-in registry for compiled-in modules. A plain
/// function pointer suffices — built-in factories capture no state.
pub type Factory = fn() -> Box<dyn Module>;

/// Static module identity metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleInfo {
    /// The CLI-visible module name (`"tobogganing"`, `"squawk"`): lowercase, no
    /// spaces, used in `penguin <name> ...` and config paths.
    pub name: String,
    /// The module's own semantic version.
    pub version: String,
    /// A one-line human summary.
    pub description: String,
    /// The feature-flag / entitlement key gating this module
    /// (e.g. `"penguin.tobogganing"`); empty means ungated.
    pub license_feature: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_info_default_is_all_empty() {
        let info = ModuleInfo::default();
        assert!(info.name.is_empty());
        assert!(info.version.is_empty());
        assert!(info.description.is_empty());
        assert!(info.license_feature.is_empty());
    }

    #[test]
    fn module_info_equality_is_field_wise() {
        let a = ModuleInfo {
            name: "squawk".to_string(),
            version: "1.0.0".to_string(),
            description: "DoH".to_string(),
            license_feature: String::new(),
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
