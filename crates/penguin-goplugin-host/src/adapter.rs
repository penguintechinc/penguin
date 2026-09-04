//! Adapts the `penguin.sdk.v1.ModuleService` gRPC client into the
//! [`Module`] trait the daemon supervisor drives every module through —
//! built-in and external plugins are indistinguishable from the
//! supervisor's point of view.
//!
//! ## Wire error convention
//!
//! Every `ModuleService` response carries a `string error` field alongside
//! its real payload: empty means success. [`ModuleAdapter`]'s methods check
//! the transport error first (a [`tonic::Status`], meaning the RPC itself
//! failed) and only then inspect `error` (meaning the RPC succeeded but the
//! module reported failure) via [`wire_result`]. [`secret_error_from_message`]
//! captures the one exact-string special case in this convention —
//! go-plugin's `HostService.SecretsGet` maps the sentinel `"not found"` back
//! to a typed [`SecretError::NotFound`] rather than an opaque message — as a
//! small reusable helper for whichever `HostService` server implementation
//! (outside this crate; see the crate-level doc comment on the broker id=1
//! divergence) needs to apply the same convention in the other direction.
//!
//! ## `commands`/`config_schema` are fetched once, not per call
//!
//! [`Module::commands`] and [`Module::config_schema`] are synchronous in the
//! trait, but retrieving either from a plugin is inherently an async gRPC
//! call. The Go adapter can get away with a fresh RPC on every call because
//! Go has no async/sync split; Rust does, and the trait signature (owned by
//! `penguin-sdk`) is fixed. Both proto RPCs take only `api_version` — no
//! dynamic input — which is itself a strong signal they describe static,
//! declared-once module metadata, so [`ModuleAdapter::connect`] fetches each
//! exactly once and caches it, exactly like [`Module::info`] already does.
//! An RPC failure during that one fetch degrades to an empty tree /
//! `None`, matching the Go adapter's own `return nil` fallback.
//!
//! ## `init` is a deliberate no-op
//!
//! The frozen Go host's `moduleClientAdapter.Init` always returns `nil`
//! without ever calling the wire `ModuleService.Init` RPC. A comment above
//! it claims `GRPCClient` calls `Init`, but that function
//! (`plugin_glue.go`) only wires up the broker's HostService leg and never
//! invokes anything named `Init` on the module — the comment is simply
//! wrong. So no plugin built against the frozen Go host has ever received an
//! `Init` RPC, and a plugin that requires one to function is already broken
//! against it. This port preserves that exact (bug-compatible) behaviour
//! rather than "fixing" it: [`ModuleAdapter::init`] stays a no-op so nothing
//! changes for existing plugins.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use tonic::transport::Channel;

use penguin_proto::sdk::v1 as pb;
use penguin_proto::sdk::v1::module_service_client::ModuleServiceClient;
use penguin_sdk::convert::{
    API_VERSION, command_result_from_proto, command_spec_from_proto, health_report_from_proto,
    status_from_proto,
};
use penguin_sdk::{
    CommandResult, CommandSpec, HealthLevel, HealthReport, HostServices, Module, ModuleError,
    ModuleInfo, SecretError, Status,
};

/// Adapts a connected `ModuleService` client into [`Module`].
pub struct ModuleAdapter {
    client: ModuleServiceClient<Channel>,
    /// Cached from the `Info` call made at construction — [`Module::info`]
    /// is synchronous and must be callable before `init`.
    info: ModuleInfo,
    /// Cached from the one `Commands` call made at construction. See the
    /// module-level doc comment.
    commands: Vec<CommandSpec>,
    /// Cached from the one `ConfigSchema` call made at construction. See the
    /// module-level doc comment.
    config_schema: Option<Vec<u8>>,
}

impl ModuleAdapter {
    /// Connects to the plugin's `ModuleService` and fetches its identity,
    /// command tree, and config schema.
    ///
    /// Only a failure of the `Info` call aborts this: a module without
    /// identity metadata cannot usefully exist, but a `Commands` or
    /// `ConfigSchema` failure degrades gracefully (see the module-level doc
    /// comment) rather than blocking the connection.
    pub async fn connect(channel: Channel) -> Result<ModuleAdapter, ModuleError> {
        let mut client = ModuleServiceClient::new(channel);

        let info_response = client
            .info(pb::InfoRequest {
                api_version: API_VERSION.to_string(),
            })
            .await
            .map_err(status_to_module_error)?
            .into_inner();
        let info = ModuleInfo {
            name: info_response.name,
            version: info_response.version,
            description: info_response.description,
            license_feature: info_response.license_feature,
        };

        let commands = fetch_commands(&mut client).await;
        let config_schema = fetch_config_schema(&mut client).await;

        Ok(ModuleAdapter {
            client,
            info,
            commands,
            config_schema,
        })
    }
}

#[async_trait]
impl Module for ModuleAdapter {
    fn info(&self) -> ModuleInfo {
        self.info.clone()
    }

    async fn init(&self, _host: Arc<dyn HostServices>) -> Result<(), ModuleError> {
        // Deliberate no-op — see the module-level doc comment on the Go
        // dead-Init finding.
        Ok(())
    }

    async fn start(&self) -> Result<(), ModuleError> {
        let mut client = self.client.clone();
        let response = client
            .start(pb::StartRequest {
                api_version: API_VERSION.to_string(),
            })
            .await
            .map_err(status_to_module_error)?
            .into_inner();
        wire_result((), response.error)
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        let mut client = self.client.clone();
        let response = client
            .stop(pb::StopRequest {
                api_version: API_VERSION.to_string(),
            })
            .await
            .map_err(status_to_module_error)?
            .into_inner();
        wire_result((), response.error)
    }

    async fn status(&self) -> Result<Status, ModuleError> {
        let mut client = self.client.clone();
        let response = client
            .status(pb::StatusRequest {
                api_version: API_VERSION.to_string(),
            })
            .await
            .map_err(status_to_module_error)?
            .into_inner();
        let error = response.error.clone();
        wire_result(status_from_proto(&response), error)
    }

    async fn health(&self) -> HealthReport {
        let mut client = self.client.clone();
        let result = client
            .health(pb::HealthRequest {
                api_version: API_VERSION.to_string(),
            })
            .await;
        match result {
            Ok(response) => health_report_from_proto(&response.into_inner()),
            Err(status) => HealthReport {
                level: HealthLevel::Unhealthy,
                message: status.to_string(),
                checked_at: SystemTime::now(),
            },
        }
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
        let mut client = self.client.clone();
        let response = client
            .dispatch(pb::DispatchRequest {
                api_version: API_VERSION.to_string(),
                path: path.to_vec(),
                flags: flags.clone(),
                args: args.to_vec(),
            })
            .await
            .map_err(status_to_module_error)?
            .into_inner();
        let error = response.error.clone();
        wire_result(command_result_from_proto(&response), error)
    }

    fn config_schema(&self) -> Option<Vec<u8>> {
        self.config_schema.clone()
    }
}

/// Fetches the module's command tree, degrading to an empty tree on any RPC
/// failure — matches the Go adapter's `return nil` fallback for `Commands`.
async fn fetch_commands(client: &mut ModuleServiceClient<Channel>) -> Vec<CommandSpec> {
    let result = client
        .commands(pb::CommandsRequest {
            api_version: API_VERSION.to_string(),
        })
        .await;
    let mut specs = Vec::new();
    if let Ok(response) = result {
        for pb_spec in &response.into_inner().commands {
            specs.push(command_spec_from_proto(pb_spec));
        }
    }
    specs
}

/// Fetches the module's config schema, degrading to `None` on any RPC
/// failure or an empty payload — matches the Go adapter's `return nil`
/// fallback for `ConfigSchema` and the proto's own documented convention
/// that an empty `schema` means "no configuration".
async fn fetch_config_schema(client: &mut ModuleServiceClient<Channel>) -> Option<Vec<u8>> {
    let result = client
        .config_schema(pb::ConfigSchemaRequest {
            api_version: API_VERSION.to_string(),
        })
        .await;
    let Ok(response) = result else {
        return None;
    };
    let schema = response.into_inner().schema;
    if schema.is_empty() {
        None
    } else {
        Some(schema)
    }
}

/// Converts a transport-level failure into a [`ModuleError`]. Only the
/// message survives the trip — see the module-level doc comment on the wire
/// error convention.
fn status_to_module_error(status: tonic::Status) -> ModuleError {
    ModuleError::new(status.to_string())
}

/// Applies the wire error convention: `error` is only meaningful once the
/// RPC has already succeeded at the transport level. An empty `error` means
/// the call succeeded and `value` is returned; anything else becomes a
/// [`ModuleError`] carrying that message verbatim.
fn wire_result<T>(value: T, error: String) -> Result<T, ModuleError> {
    if error.is_empty() {
        Ok(value)
    } else {
        Err(ModuleError::new(error))
    }
}

/// Maps a `SecretsGet` wire error message back to a typed [`SecretError`].
/// go-plugin's `HostService.SecretsGet` sends the exact string `"not found"`
/// for a missing key (see `go-client/internal/extplugin/host.go`); every
/// other non-empty message is an opaque backend failure. Exposed here so a
/// `HostService` server implementation (outside this crate — see the
/// crate-level doc comment on the broker id=1 divergence) applies the same
/// convention this crate already uses everywhere else on the wire, instead
/// of redefining it.
pub fn secret_error_from_message(message: &str) -> SecretError {
    if message == "not found" {
        SecretError::NotFound
    } else {
        SecretError::Other(message.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_result_empty_error_is_success() {
        assert_eq!(wire_result(42, String::new()), Ok(42));
    }

    #[test]
    fn wire_result_nonempty_error_becomes_module_error() {
        let err = wire_result((), "boom".to_string()).unwrap_err();
        assert_eq!(err, ModuleError::new("boom"));
    }

    #[test]
    fn secret_error_not_found_is_the_exact_go_sentinel() {
        assert_eq!(
            secret_error_from_message("not found"),
            SecretError::NotFound
        );
    }

    #[test]
    fn secret_error_anything_else_is_a_flat_message() {
        assert_eq!(
            secret_error_from_message("keyring locked"),
            SecretError::Other("keyring locked".to_string())
        );
    }

    #[test]
    fn secret_error_sentinel_match_is_exact() {
        // Only the precise lowercase sentinel matches; a near-miss is still
        // an opaque message, matching Go's plain `==` comparison.
        assert_eq!(
            secret_error_from_message("Not Found"),
            SecretError::Other("Not Found".to_string())
        );
    }

    #[test]
    fn empty_error_message_is_not_a_secret_error() {
        assert_eq!(
            secret_error_from_message(""),
            SecretError::Other(String::new())
        );
    }
}
