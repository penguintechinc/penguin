//! The squawk `sdk::Module` implementation: lifecycle glue wiring
//! `squawk-client`'s DoH client, local forwarder, and [`crate::sysresolver`]
//! system-DNS state machine to the daemon supervisor.
//!
//! Ported from `go-client/internal/modules/squawk/module.go`, fixing the
//! bugs this milestone's brief calls out explicitly (documented at each
//! fix site) and otherwise preserving Go's behaviour and command surface.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use penguin_sdk::{
    CommandResult, CommandSpec, HealthLevel, HealthReport, HostServices, Module, ModuleError,
    ModuleInfo, ModuleState, Status,
};

use squawk_client::doh::DohClient;
use squawk_client::forwarder::Forwarder;

use crate::commands;
use crate::config::{DEFAULT_CACHE_MAX_ENTRIES, ModuleConfig};
use crate::metrics::SquawkMetrics;
use crate::sysresolver::SysResolver;

/// The IP squawk points the host's system resolver at when `system_dns.manage`
/// is enabled. Ported verbatim from the Go module, which hard-codes this
/// same address in `Start` — out of scope for this milestone's listed
/// bug-fixes, so preserved rather than silently changed. (Note for a future
/// pass: this bypasses squawk's own DoH forwarder entirely, which reads as
/// an oversight rather than an intentional design choice, but is not one of
/// the bugs this milestone was scoped to fix.)
const DEFAULT_MANAGED_DNS_SERVER: IpAddr = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));

/// How long a cached health probe is trusted before `health()` runs a fresh
/// one. Matches the Go module's `5 * time.Second`.
const HEALTH_CACHE_TTL: Duration = Duration::from_secs(5);

/// Upper bound on a single `health()` probe query. Matches Go's
/// `context.WithTimeout(ctx, 2*time.Second)`.
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The domain `health()` probes with — matches Go's hard-coded `"google.com"`.
const HEALTH_PROBE_DOMAIN: &str = "google.com";

/// Squawk: a DNS-over-HTTPS endpoint client with optional local `:53`
/// forwarding and system DNS resolver management.
///
/// Every field set during [`Module::init`] lives behind a [`OnceLock`] —
/// `init` runs exactly once per the `Module` contract, and every other
/// method only ever runs after it. Fields that change over the module's
/// running lifetime (the running/DNS-applied flags, the cached health
/// probe) use atomics/a plain mutex instead, since every `Module` method
/// takes `&self`.
pub struct SquawkModule {
    host: OnceLock<Arc<dyn HostServices>>,
    config: OnceLock<ModuleConfig>,
    doh: OnceLock<Arc<DohClient>>,
    /// `None` when `forwarder.enabled` is `false` in config.
    forwarder: OnceLock<Option<Arc<Forwarder>>>,
    resolver: OnceLock<SysResolver>,
    metrics: OnceLock<SquawkMetrics>,
    running: AtomicBool,
    dns_applied: AtomicBool,
    last_health: StdMutex<HealthReport>,
}

impl Default for SquawkModule {
    fn default() -> SquawkModule {
        SquawkModule::new()
    }
}

impl SquawkModule {
    /// Builds a fresh, un-initialised module — the shape every
    /// [`penguin_sdk::Factory`] invocation (including [`factory`]) produces.
    pub fn new() -> SquawkModule {
        SquawkModule {
            host: OnceLock::new(),
            config: OnceLock::new(),
            doh: OnceLock::new(),
            forwarder: OnceLock::new(),
            resolver: OnceLock::new(),
            metrics: OnceLock::new(),
            running: AtomicBool::new(false),
            dns_applied: AtomicBool::new(false),
            last_health: StdMutex::new(HealthReport::default()),
        }
    }

    /// The panic message every post-init accessor shares: every `Module`
    /// method besides `info`/`init` runs only after `init` has already set
    /// every `OnceLock` below, so a miss here means the supervisor violated
    /// that contract, not a recoverable runtime condition.
    const UNINITIALISED: &'static str = "squawk module used before init";

    pub(crate) fn host(&self) -> &Arc<dyn HostServices> {
        self.host.get().expect(Self::UNINITIALISED)
    }

    pub(crate) fn config(&self) -> &ModuleConfig {
        self.config.get().expect(Self::UNINITIALISED)
    }

    pub(crate) fn doh(&self) -> &Arc<DohClient> {
        self.doh.get().expect(Self::UNINITIALISED)
    }

    pub(crate) fn forwarder(&self) -> Option<&Arc<Forwarder>> {
        self.forwarder.get().expect(Self::UNINITIALISED).as_ref()
    }

    pub(crate) fn resolver(&self) -> &SysResolver {
        self.resolver.get().expect(Self::UNINITIALISED)
    }

    pub(crate) fn metrics(&self) -> &SquawkMetrics {
        self.metrics.get().expect(Self::UNINITIALISED)
    }
}

/// Builds a fresh, un-initialised [`SquawkModule`] — the [`penguin_sdk::Factory`]
/// registered for the built-in `"squawk"` module (see `penguin-registry`).
pub fn factory() -> Box<dyn Module> {
    Box::new(SquawkModule::new())
}

#[async_trait]
impl Module for SquawkModule {
    /// Identity metadata for the daemon's module registry and `penguin
    /// status`.
    ///
    /// `license_feature` is deliberately empty: squawk is core product and
    /// ships in the Free tier, so the module itself must load successfully
    /// even when no license server is reachable at all. Enterprise-only
    /// *capabilities inside* squawk (none exist yet) would each be gated
    /// individually via `host.license().feature_enabled("penguin.<feature>")`,
    /// not by refusing to load the module.
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "squawk".to_string(),
            version: "1.0.0".to_string(),
            description: "DNS-over-HTTPS endpoint client with system DNS management".to_string(),
            license_feature: String::new(),
        }
    }

    /// Prepares the module: recovers any interrupted DNS change, resolves
    /// config (defaults, then the host's validated YAML, then a best-effort
    /// secrets lookup for a missing auth token), builds the DoH client and
    /// (if enabled) the forwarder, and registers all five metrics. Never
    /// begins background work — that is [`Module::start`]'s job.
    async fn init(&self, host: Arc<dyn HostServices>) -> Result<(), ModuleError> {
        let logger = host.logger();

        let resolver = SysResolver::new(host.data_dir());
        if let Err(err) = resolver.recover_from_crash().await {
            logger.warn(
                "squawk DNS crash recovery failed; continuing with fresh resolver state",
                &[("error", &err.to_string())],
            );
        }

        let raw_config = host.config();
        let mut cfg: ModuleConfig = if raw_config.is_empty() {
            ModuleConfig::default()
        } else {
            match serde_norway::from_slice(&raw_config) {
                Ok(parsed) => parsed,
                Err(err) => return Err(ModuleError::new(format!("parse squawk config: {err}"))),
            }
        };

        if cfg.doh.auth_token.is_empty()
            && let Ok(token) = host.secrets().get("auth_token").await
        {
            cfg.doh.auth_token = String::from_utf8_lossy(&token).into_owned();
        }

        let doh_config = squawk_client::doh::Config {
            server_url: cfg.doh.server_url.clone(),
            server_urls: Vec::new(),
            auth_token: cfg.doh.auth_token.clone(),
            client_cert: cfg.doh.client_cert.clone(),
            client_key: cfg.doh.client_key.clone(),
            ca_cert: cfg.doh.ca_cert.clone(),
            verify_ssl: cfg.doh.verify_tls,
            max_retries: 0,
            retry_delay: 0,
        };
        let doh = match DohClient::new(doh_config) {
            Ok(client) => Arc::new(client),
            Err(err) => {
                logger.error(
                    "failed to create DoH client",
                    &[("error", &err.to_string())],
                );
                return Err(ModuleError::new(format!("create DoH client: {err}")));
            }
        };

        let forwarder = if cfg.forwarder.enabled {
            let fwd_config = squawk_client::forwarder::Config {
                udp_address: cfg.forwarder.udp_addr.clone(),
                tcp_address: cfg.forwarder.tcp_addr.clone(),
                listen_udp: true,
                listen_tcp: true,
            };
            let cache_config = squawk_client::forwarder::CacheConfig {
                enabled: cfg.cache.enabled,
                max_entries: DEFAULT_CACHE_MAX_ENTRIES,
            };
            Some(Arc::new(Forwarder::new(
                Arc::clone(&doh),
                fwd_config,
                cache_config,
            )))
        } else {
            None
        };

        let metrics = SquawkMetrics::register(host.metrics().as_ref())
            .map_err(|err| ModuleError::new(format!("register metrics: {err}")))?;

        logger.info(
            "squawk module initialized",
            &[
                ("server", cfg.doh.server_url.as_str()),
                ("forwarder_enabled", &cfg.forwarder.enabled.to_string()),
            ],
        );

        // `OnceLock::set` returning `Err` would mean `init` ran twice —
        // impossible per the `Module::init` contract ("called exactly
        // once"), so a violation here is a supervisor bug, not a condition
        // this method needs to handle gracefully.
        let _ = self.host.set(host);
        let _ = self.config.set(cfg);
        let _ = self.doh.set(doh);
        let _ = self.forwarder.set(forwarder);
        let _ = self.resolver.set(resolver);
        let _ = self.metrics.set(metrics);

        Ok(())
    }

    /// Starts the forwarder (if configured) and, if `system_dns.manage` is
    /// set, points the host's resolver at [`DEFAULT_MANAGED_DNS_SERVER`].
    /// Returns promptly: [`Forwarder::start`] binds synchronously and then
    /// spawns its serve loops rather than blocking on them. Idempotent — a
    /// second call while already running is a no-op, matching Go.
    async fn start(&self) -> Result<(), ModuleError> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        if let Some(forwarder) = self.forwarder() {
            if let Err(err) = forwarder.start().await {
                self.running.store(false, Ordering::SeqCst);
                self.host()
                    .logger()
                    .error("failed to start forwarder", &[("error", &err.to_string())]);
                return Err(ModuleError::new(format!("start forwarder: {err}")));
            }
            self.metrics().forwarder_up.set(1.0);
        }

        if self.config().system_dns.manage {
            match self.resolver().apply(&[DEFAULT_MANAGED_DNS_SERVER]).await {
                Ok(()) => {
                    self.dns_applied.store(true, Ordering::SeqCst);
                    self.metrics().dns_applied.set(1.0);
                }
                Err(err) => {
                    // Non-fatal: the forwarder (if any) is already up, and a
                    // DNS-apply failure must not take the whole module down.
                    self.host()
                        .logger()
                        .warn("failed to apply system DNS", &[("error", &err.to_string())]);
                }
            }
        }

        self.host().logger().info("squawk module started", &[]);
        Ok(())
    }

    /// Stops the forwarder and restores system DNS if it was being managed,
    /// running every teardown step even when an earlier one fails so a
    /// forwarder-stop error can never suppress a DNS restore (or vice
    /// versa). Idempotent: a second call while already stopped is a no-op.
    async fn stop(&self) -> Result<(), ModuleError> {
        if !self.running.swap(false, Ordering::SeqCst) {
            return Ok(());
        }

        let mut errors: Vec<String> = Vec::new();

        if let Some(forwarder) = self.forwarder() {
            if let Err(err) = forwarder.stop().await {
                self.host()
                    .logger()
                    .error("failed to stop forwarder", &[("error", &err.to_string())]);
                errors.push(format!("forwarder: {err}"));
            }
            self.metrics().forwarder_up.set(0.0);
        }

        if self.config().system_dns.manage {
            if let Err(err) = self.resolver().restore().await {
                self.host().logger().error(
                    "failed to restore system DNS",
                    &[("error", &err.to_string())],
                );
                errors.push(format!("resolver: {err}"));
            }
            self.dns_applied.store(false, Ordering::SeqCst);
            self.metrics().dns_applied.set(0.0);
        }

        self.host().logger().info("squawk module stopped", &[]);

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ModuleError::new(format!(
                "stop errors: {}",
                errors.join("; ")
            )))
        }
    }

    /// Reports the module's running state, its configured DoH server, and
    /// (when relevant) the forwarder's real bound addresses and the host
    /// resolver's current server. Also refreshes the live `cache_entries`
    /// gauge, so a `status`/`GetStatus` caller always sees a current count
    /// without having to separately run `cache stats`.
    async fn status(&self) -> Result<Status, ModuleError> {
        let running = self.running.load(Ordering::SeqCst);
        let state = if running {
            ModuleState::Running
        } else {
            ModuleState::Stopped
        };

        let mut detail = HashMap::new();
        detail.insert("server".to_string(), self.config().doh.server_url.clone());

        if let Some(forwarder) = self.forwarder() {
            detail.insert("forwarder".to_string(), forwarder_detail(forwarder));
            let stats = forwarder.cache().stats();
            self.metrics().cache_entries.set(stats.entries as f64);
        }

        if let Ok(current) = self.resolver().current().await
            && let Some(first) = current.first()
        {
            detail.insert("dns_servers".to_string(), first.to_string());
        }

        Ok(Status { state, detail })
    }

    /// A cheap liveness probe: a live DoH query for [`HEALTH_PROBE_DOMAIN`],
    /// cached for [`HEALTH_CACHE_TTL`]. Also updates the `health_status`
    /// metric on every fresh probe (not just at init — the Go module
    /// registered this gauge and never wrote to it at all).
    ///
    /// Fixes a real Go bug: on a fresh (non-cached) probe, Go stored the
    /// probe's timestamp into its cache but then built the *returned*
    /// report with a second, separate `time.Now()` call rather than the
    /// value it had just stored — the two could disagree. Here `checked_at`
    /// is computed once and reused for both the cached copy and the
    /// returned report, so they are always identical.
    async fn health(&self) -> HealthReport {
        {
            let cached = self.last_health.lock().expect("health mutex poisoned");
            if let Ok(age) = SystemTime::now().duration_since(cached.checked_at)
                && age < HEALTH_CACHE_TTL
            {
                return cached.clone();
            }
        }

        let doh = self.doh();
        let cancel = CancellationToken::new();
        self.metrics().queries_total.inc();
        let probe = doh.query(&cancel, HEALTH_PROBE_DOMAIN, "A");

        let (level, message) = match tokio::time::timeout(HEALTH_PROBE_TIMEOUT, probe).await {
            Ok(Ok(_response)) => (HealthLevel::Healthy, "OK".to_string()),
            Ok(Err(err)) => (HealthLevel::Degraded, format!("query error: {err}")),
            Err(_elapsed) => (HealthLevel::Degraded, "query error: timed out".to_string()),
        };

        let checked_at = SystemTime::now();
        let report = HealthReport {
            level,
            message,
            checked_at,
        };

        self.metrics().set_health(level);
        *self.last_health.lock().expect("health mutex poisoned") = report.clone();
        report
    }

    /// Declares squawk's CLI command tree.
    fn commands(&self) -> Vec<CommandSpec> {
        commands::command_tree()
    }

    /// Executes one squawk CLI command.
    async fn dispatch(
        &self,
        path: &[String],
        flags: &HashMap<String, String>,
        args: &[String],
    ) -> Result<CommandResult, ModuleError> {
        commands::dispatch(self, path, flags, args).await
    }

    /// Returns [`crate::config::CONFIG_SCHEMA`] for the daemon to validate
    /// `squawk.yaml` against before `init` ever sees it.
    fn config_schema(&self) -> Option<Vec<u8>> {
        Some(crate::config::CONFIG_SCHEMA.as_bytes().to_vec())
    }
}

/// Renders the forwarder's live status for [`Module::status`]'s `detail`
/// map: its real bound addresses while running, or a plain "configured"
/// note otherwise. Unlike the Go module (which hard-codes `"listening
/// :53"` regardless of the forwarder's actual configured address), this
/// reports the forwarder's genuine bound port — relevant since a squawk
/// deployment may bind an address other than `:53` (as this milestone's own
/// integration test does, to stay off privileged ports).
fn forwarder_detail(forwarder: &Forwarder) -> String {
    if !forwarder.is_running() {
        return "configured, not running".to_string();
    }
    match (forwarder.local_udp_addr(), forwarder.local_tcp_addr()) {
        (Some(udp), Some(tcp)) => format!("listening udp {udp}, tcp {tcp}"),
        (Some(udp), None) => format!("listening udp {udp}"),
        (None, Some(tcp)) => format!("listening tcp {tcp}"),
        (None, None) => "listening".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_reports_squawk_identity_with_no_license_gate() {
        let module = SquawkModule::new();
        let info = module.info();
        assert_eq!(info.name, "squawk");
        assert_eq!(info.version, "1.0.0");
        assert!(info.license_feature.is_empty());
    }

    #[test]
    fn factory_builds_a_fresh_uninitialised_module() {
        let module = factory();
        assert_eq!(module.info().name, "squawk");
    }

    #[test]
    #[should_panic(expected = "squawk module used before init")]
    fn accessors_panic_before_init() {
        let module = SquawkModule::new();
        let _ = module.config();
    }
}
