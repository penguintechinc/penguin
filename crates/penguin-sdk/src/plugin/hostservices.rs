//! [`HostServices`] implementations available to a served [`Module`]:
//! [`RemoteHostServices`], backed by RPCs to the host's `HostService` over
//! the broker's id=1 leg, and [`NoopHostServices`], the graceful-degradation
//! fallback used when that leg cannot be reached (see `serve.rs`'s doc
//! comment on why that must never be fatal).
//!
//! [`Module`]: crate::Module
//!
//! ## Sync trait methods over an async transport
//!
//! [`HostServices::config`], [`HostServices::data_dir`], and
//! [`LicenseChecker::feature_enabled`]/[`LicenseChecker::tier`] are all
//! *synchronous* in the trait — `host.rs`'s doc comment on `LicenseChecker`
//! is explicit that this is deliberate: "they read a cache, never the wire."
//! [`RemoteHostServices::connect`] therefore prefetches `Config`, `DataDir`,
//! and the license tier exactly once, asynchronously, before the module ever
//! sees this type (the same one-shot-fetch-and-cache pattern
//! `penguin-goplugin-host::adapter::ModuleAdapter` already uses for
//! `commands`/`config_schema`). [`LicenseChecker::feature_enabled`] cannot be
//! prefetched (the flag key isn't known until called), so an unseen key
//! returns the house-standard safe default — `false` — and kicks off a
//! background refresh via `tokio::spawn` for next time, matching
//! `general.md`'s "new/never-seen flags default OFF" policy exactly.
//!
//! [`Logger::log`] and [`EventSink::publish`] are sync-and-fire-and-forget
//! for the same reason: logging and eventing are best-effort, so each call
//! spawns its RPC rather than blocking the caller on it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tonic::transport::Channel;

use penguin_proto::sdk::v1 as pb;
use penguin_proto::sdk::v1::host_service_client::HostServiceClient;

use crate::convert::API_VERSION;
use crate::error::{MetricsError, SecretError};
use crate::host::{Event, EventSink, HostServices, LicenseChecker, LogLevel, Logger, Metrics};

/// Maps a `SecretsGet` wire error message back to a typed [`SecretError`].
/// The host sends the exact string `"not found"` for a missing key — see
/// `penguin-goplugin-host::adapter::secret_error_from_message`, mirrored here
/// for the same reason every other file in this module duplicates rather
/// than imports.
fn secret_error_from_message(message: &str) -> SecretError {
    if message == "not found" {
        SecretError::NotFound
    } else {
        SecretError::Other(message.to_string())
    }
}

/// Registers a metrics collector into the process-wide default prometheus
/// registry. There is no `HostService` RPC for metrics — the Go SDK's own
/// `HostServicesProxy.Metrics()` returns `prometheus.DefaultRegisterer`
/// rather than proxying to the host — so [`RemoteHostServices`] and
/// [`NoopHostServices`] share this one implementation.
struct LocalMetrics;

impl Metrics for LocalMetrics {
    fn register(
        &self,
        collector: Box<dyn prometheus::core::Collector>,
    ) -> Result<(), MetricsError> {
        prometheus::register(collector).map_err(|e| MetricsError(e.to_string()))
    }
}

/// [`HostServices`] backed by RPCs to the host's `HostService`, dialed over
/// the broker's id=1 leg. See the module doc comment for the sync/async
/// boundary this type resolves.
pub struct RemoteHostServices {
    logger: Arc<RemoteLogger>,
    secrets: Arc<RemoteSecretStore>,
    license: Arc<RemoteLicenseChecker>,
    metrics: Arc<LocalMetrics>,
    events: Arc<RemoteEventSink>,
    config: Vec<u8>,
    data_dir: PathBuf,
}

impl RemoteHostServices {
    /// Connects to the host's `HostService` over `channel` and prefetches
    /// the values [`HostServices`]'s synchronous accessors need cached.
    /// Every prefetch degrades to an empty/default value on RPC failure
    /// rather than aborting the connection — a host that answers some calls
    /// but not others is still more useful than falling all the way back to
    /// [`NoopHostServices`].
    pub async fn connect(channel: Channel) -> RemoteHostServices {
        let client = HostServiceClient::new(channel);

        let config = fetch_config(client.clone()).await;
        let data_dir = fetch_data_dir(client.clone()).await;
        let tier = fetch_license_tier(client.clone()).await;

        RemoteHostServices {
            logger: Arc::new(RemoteLogger {
                client: client.clone(),
            }),
            secrets: Arc::new(RemoteSecretStore {
                client: client.clone(),
            }),
            license: Arc::new(RemoteLicenseChecker {
                client: client.clone(),
                tier: Mutex::new(tier),
                flags: Arc::new(Mutex::new(HashMap::new())),
            }),
            metrics: Arc::new(LocalMetrics),
            events: Arc::new(RemoteEventSink { client }),
            config,
            data_dir,
        }
    }
}

impl HostServices for RemoteHostServices {
    fn logger(&self) -> Arc<dyn Logger> {
        self.logger.clone()
    }
    fn secrets(&self) -> Arc<dyn crate::host::SecretStore> {
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

async fn fetch_config(mut client: HostServiceClient<Channel>) -> Vec<u8> {
    let request = pb::ConfigRequest {
        api_version: API_VERSION.to_string(),
    };
    let Ok(response) = client.config(request).await else {
        return Vec::new();
    };
    response.into_inner().config
}

async fn fetch_data_dir(mut client: HostServiceClient<Channel>) -> PathBuf {
    let request = pb::DataDirRequest {
        api_version: API_VERSION.to_string(),
    };
    let Ok(response) = client.data_dir(request).await else {
        return std::env::temp_dir();
    };
    let path = response.into_inner().path;
    if path.is_empty() {
        std::env::temp_dir()
    } else {
        PathBuf::from(path)
    }
}

async fn fetch_license_tier(mut client: HostServiceClient<Channel>) -> String {
    let request = pb::LicenseTierRequest {
        api_version: API_VERSION.to_string(),
    };
    let Ok(response) = client.license_tier(request).await else {
        return String::new();
    };
    response.into_inner().tier
}

/// [`Logger`] backed by `HostService.Log`. See the module doc comment on why
/// this fires and forgets rather than blocking `log()`'s sync caller.
struct RemoteLogger {
    client: HostServiceClient<Channel>,
}

impl Logger for RemoteLogger {
    fn log(&self, level: LogLevel, message: &str, fields: &[(&str, &str)]) {
        let mut client = self.client.clone();
        let request = pb::LogRequest {
            api_version: API_VERSION.to_string(),
            level: level.as_str().to_string(),
            message: message.to_string(),
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        tokio::spawn(async move {
            if let Err(status) = client.log(request).await {
                tracing::debug!(error = %status, "HostService.Log failed");
            }
        });
    }
}

/// [`SecretStore`](crate::host::SecretStore) backed by
/// `HostService.SecretsGet/Set/Delete`. These trait methods are genuinely
/// async, so unlike the logger/events sinks each call is a real RPC the
/// caller awaits.
struct RemoteSecretStore {
    client: HostServiceClient<Channel>,
}

#[async_trait]
impl crate::host::SecretStore for RemoteSecretStore {
    async fn get(&self, key: &str) -> Result<Vec<u8>, SecretError> {
        let request = pb::SecretsGetRequest {
            api_version: API_VERSION.to_string(),
            key: key.to_string(),
        };
        let mut client = self.client.clone();
        let response = client
            .secrets_get(request)
            .await
            .map_err(|status| SecretError::Other(status.to_string()))?
            .into_inner();
        if response.error.is_empty() {
            Ok(response.value)
        } else {
            Err(secret_error_from_message(&response.error))
        }
    }

    async fn set(&self, key: &str, value: &[u8]) -> Result<(), SecretError> {
        let request = pb::SecretsSetRequest {
            api_version: API_VERSION.to_string(),
            key: key.to_string(),
            value: value.to_vec(),
        };
        let mut client = self.client.clone();
        let response = client
            .secrets_set(request)
            .await
            .map_err(|status| SecretError::Other(status.to_string()))?
            .into_inner();
        if response.error.is_empty() {
            Ok(())
        } else {
            Err(SecretError::Other(response.error))
        }
    }

    async fn delete(&self, key: &str) -> Result<(), SecretError> {
        let request = pb::SecretsDeleteRequest {
            api_version: API_VERSION.to_string(),
            key: key.to_string(),
        };
        let mut client = self.client.clone();
        let response = client
            .secrets_delete(request)
            .await
            .map_err(|status| SecretError::Other(status.to_string()))?
            .into_inner();
        if response.error.is_empty() {
            Ok(())
        } else {
            Err(SecretError::Other(response.error))
        }
    }
}

/// [`LicenseChecker`] backed by `HostService.LicenseFeatureEnabled`/
/// `LicenseTier`, cached per the module doc comment's sync-trait contract.
/// `flags` is an `Arc` (not a bare `Mutex`) specifically so the background
/// refresh task spawned by [`RemoteLicenseChecker::feature_enabled`] can
/// share the exact same cache rather than populating an orphaned copy.
struct RemoteLicenseChecker {
    client: HostServiceClient<Channel>,
    tier: Mutex<String>,
    flags: Arc<Mutex<HashMap<String, bool>>>,
}

impl LicenseChecker for RemoteLicenseChecker {
    fn feature_enabled(&self, key: &str) -> bool {
        let cached = {
            let flags = self.flags.lock().unwrap_or_else(|e| e.into_inner());
            flags.get(key).copied()
        };
        if let Some(value) = cached {
            return value;
        }

        let client = self.client.clone();
        let key_owned = key.to_string();
        let flags = Arc::clone(&self.flags);
        tokio::spawn(async move {
            refresh_feature_flag(client, key_owned, flags).await;
        });
        // House policy (general.md, Feature Toggling & License Enforcement):
        // a never-seen flag defaults OFF while the background refresh above
        // populates the cache for the next call.
        false
    }

    fn tier(&self) -> String {
        self.tier.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Fetches `key`'s current value from the host and writes it into the shared
/// cache, so the *next* [`RemoteLicenseChecker::feature_enabled`] call for
/// the same key returns a real answer instead of the safe-default `false`.
async fn refresh_feature_flag(
    mut client: HostServiceClient<Channel>,
    key: String,
    flags: Arc<Mutex<HashMap<String, bool>>>,
) {
    let request = pb::LicenseFeatureEnabledRequest {
        api_version: API_VERSION.to_string(),
        key: key.clone(),
    };
    match client.license_feature_enabled(request).await {
        Ok(response) => {
            let enabled = response.into_inner().enabled;
            let mut flags = flags.lock().unwrap_or_else(|e| e.into_inner());
            flags.insert(key, enabled);
        }
        Err(status) => {
            tracing::debug!(error = %status, "HostService.LicenseFeatureEnabled failed");
        }
    }
}

/// [`EventSink`] backed by `HostService.PublishEvent`. Fire-and-forget, same
/// reasoning as [`RemoteLogger`].
struct RemoteEventSink {
    client: HostServiceClient<Channel>,
}

impl EventSink for RemoteEventSink {
    fn publish(&self, event: Event) {
        let mut client = self.client.clone();
        let request = crate::convert::event_to_proto(&event);
        tokio::spawn(async move {
            if let Err(status) = client.publish_event(request).await {
                tracing::debug!(error = %status, "HostService.PublishEvent failed");
            }
        });
    }
}

/// The graceful-degradation [`HostServices`] used when the broker's id=1
/// leg cannot be reached — an unreachable host, a host that never serves
/// `HostService` at all, or (as happens against the frozen Go daemon) a
/// host that serves it in plaintext and rejects our TLS ClientHello. See
/// `serve.rs`'s doc comment for the full decision.
pub struct NoopHostServices {
    logger: Arc<NoopLogger>,
    secrets: Arc<NoopSecretStore>,
    license: Arc<NoopLicenseChecker>,
    metrics: Arc<LocalMetrics>,
    events: Arc<NoopEventSink>,
}

impl NoopHostServices {
    /// Builds the no-op fallback.
    pub fn new() -> NoopHostServices {
        NoopHostServices {
            logger: Arc::new(NoopLogger),
            secrets: Arc::new(NoopSecretStore),
            license: Arc::new(NoopLicenseChecker),
            metrics: Arc::new(LocalMetrics),
            events: Arc::new(NoopEventSink),
        }
    }
}

impl Default for NoopHostServices {
    fn default() -> NoopHostServices {
        NoopHostServices::new()
    }
}

impl HostServices for NoopHostServices {
    fn logger(&self) -> Arc<dyn Logger> {
        self.logger.clone()
    }
    fn secrets(&self) -> Arc<dyn crate::host::SecretStore> {
        self.secrets.clone()
    }
    fn license(&self) -> Arc<dyn LicenseChecker> {
        self.license.clone()
    }
    fn metrics(&self) -> Arc<dyn Metrics> {
        self.metrics.clone()
    }
    fn config(&self) -> Vec<u8> {
        Vec::new()
    }
    fn data_dir(&self) -> PathBuf {
        std::env::temp_dir()
    }
    fn events(&self) -> Arc<dyn EventSink> {
        self.events.clone()
    }
}

/// Routes to local `tracing` output so diagnostics are not silently
/// dropped even without a reachable host.
struct NoopLogger;

impl Logger for NoopLogger {
    fn log(&self, level: LogLevel, message: &str, fields: &[(&str, &str)]) {
        tracing::event!(
            target: "penguin_sdk::plugin::noop_host",
            tracing::Level::INFO,
            level = level.as_str(),
            fields = ?fields,
            "{message}"
        );
    }
}

struct NoopSecretStore;

#[async_trait]
impl crate::host::SecretStore for NoopSecretStore {
    async fn get(&self, _key: &str) -> Result<Vec<u8>, SecretError> {
        Err(SecretError::Other(
            "host services unavailable: broker leg was not reachable".to_string(),
        ))
    }
    async fn set(&self, _key: &str, _value: &[u8]) -> Result<(), SecretError> {
        Err(SecretError::Other(
            "host services unavailable: broker leg was not reachable".to_string(),
        ))
    }
    async fn delete(&self, _key: &str) -> Result<(), SecretError> {
        Err(SecretError::Other(
            "host services unavailable: broker leg was not reachable".to_string(),
        ))
    }
}

struct NoopLicenseChecker;

impl LicenseChecker for NoopLicenseChecker {
    fn feature_enabled(&self, _key: &str) -> bool {
        false
    }
    fn tier(&self) -> String {
        String::new()
    }
}

struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn publish(&self, event: Event) {
        tracing::event!(
            target: "penguin_sdk::plugin::noop_host",
            tracing::Level::DEBUG,
            module = %event.module,
            event_type = event.event_type.as_str(),
            "dropped event (no host services): {}", event.message
        );
    }
}
