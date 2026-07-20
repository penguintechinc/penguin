//! The Tobogganing `penguin_sdk::Module` implementation: lifecycle glue
//! wiring [`crate::auth::AuthManager`] and [`crate::vpn::VpnManager`] to
//! the daemon supervisor.
//!
//! Ported from `go-client/internal/modules/tobogganing/module.go`, fixing
//! the bugs this milestone's brief calls out explicitly (documented at
//! each fix site) and otherwise preserving Go's behaviour and command
//! surface. See this crate's top-level doc for what in this file is a
//! genuine port vs. what [`crate::vpn::VpnManager`] had to implement for
//! the first time.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use penguin_sdk::{
    CommandResult, CommandSpec, HealthLevel, HealthReport, HostServices, Module, ModuleError,
    ModuleInfo, ModuleState, Status,
};

use crate::auth::AuthManager;
use crate::commands;
use crate::config::ModuleConfig;
use crate::metrics::TobogganingMetrics;
use crate::vpn::VpnManager;
use crate::wireguard::{WireGuardBackend, select_backend};

/// Matches Go's `time.NewTicker(1 * time.Minute)` in `authRefreshLoop`.
const AUTH_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
/// Matches Go's `m.authMgr.IsTokenExpired(5 * time.Minute)`.
const AUTH_REFRESH_THRESHOLD: Duration = Duration::from_secs(5 * 60);
/// Matches Go's `time.NewTicker(10 * time.Second)` in `monitorLoop`.
const MONITOR_INTERVAL: Duration = Duration::from_secs(10);
/// Matches Go's `2*time.Minute` degraded threshold in `updateHealthProbe`.
const HANDSHAKE_DEGRADED_AGE: Duration = Duration::from_secs(2 * 60);

/// The module's real state, held behind an `Arc` so [`TobogganingModule::start`]
/// can clone a handle into its spawned background tasks — see that
/// method's doc.
struct Inner {
    host: OnceLock<Arc<dyn HostServices>>,
    // The module's config is not separately stored here: `vpn.config()`
    // already holds the same `ModuleConfig`
    // ([`crate::vpn::VpnManager`] is constructed from it in `init`), and a
    // second copy would just be a second thing to keep in sync for no
    // reader anywhere in this crate.
    auth: OnceLock<Arc<AuthManager>>,
    vpn: OnceLock<Arc<VpnManager>>,
    metrics: OnceLock<TobogganingMetrics>,
    running: AtomicBool,
    /// Recreated fresh on every `start()` — see [`TobogganingModule::start`]'s doc.
    cancel: StdMutex<Option<CancellationToken>>,
    /// `None` until the first probe runs — see [`TobogganingModule::health`]'s doc.
    last_health: StdMutex<Option<HealthReport>>,
    monitor_interval_ms: AtomicU64,
    auth_refresh_interval_ms: AtomicU64,
    /// Test-only liveness counters — see the `#[cfg(test)]` accessors below.
    monitor_ticks: AtomicU64,
    auth_refresh_ticks: AtomicU64,
    /// Test-only backend override — see [`TobogganingModule::set_backend_for_test`].
    /// `init` always calls the real [`select_backend`] otherwise, which
    /// means no test could ever observe a *successful* connect (the real
    /// backends either need root/netlink or unconditionally report
    /// `Unsupported`). Always present (not `#[cfg(test)]`-gated) so `init`
    /// needs no conditional compilation of its own — only the setter below
    /// is test-only, so production code can never populate it.
    test_backend: StdMutex<Option<Arc<dyn WireGuardBackend>>>,
}

impl Inner {
    const UNINITIALISED: &'static str = "tobogganing module used before init";

    fn new() -> Inner {
        Inner {
            host: OnceLock::new(),
            auth: OnceLock::new(),
            vpn: OnceLock::new(),
            metrics: OnceLock::new(),
            running: AtomicBool::new(false),
            cancel: StdMutex::new(None),
            last_health: StdMutex::new(None),
            monitor_interval_ms: AtomicU64::new(MONITOR_INTERVAL.as_millis() as u64),
            auth_refresh_interval_ms: AtomicU64::new(AUTH_REFRESH_INTERVAL.as_millis() as u64),
            monitor_ticks: AtomicU64::new(0),
            auth_refresh_ticks: AtomicU64::new(0),
            test_backend: StdMutex::new(None),
        }
    }
}

/// Tobogganing: a WireGuard-based SASE/ZTNA endpoint client.
///
/// A cheap `Clone` (an `Arc` around its real state): [`Module::start`]
/// clones `self` into the background tasks it spawns, since those tasks
/// must be `'static` and a bare `&self` cannot outlive the call.
#[derive(Clone)]
pub struct TobogganingModule {
    inner: Arc<Inner>,
}

impl Default for TobogganingModule {
    fn default() -> TobogganingModule {
        TobogganingModule::new()
    }
}

impl TobogganingModule {
    /// Builds a fresh, un-initialised module — the shape every
    /// [`penguin_sdk::Factory`] invocation (including [`factory`]) produces.
    pub fn new() -> TobogganingModule {
        TobogganingModule {
            inner: Arc::new(Inner::new()),
        }
    }

    /// The panic message every post-init accessor shares: every `Module`
    /// method besides `info`/`init` runs only after `init` has already set
    /// every `OnceLock` below, so a miss here means the supervisor
    /// violated that contract, not a recoverable runtime condition.
    pub(crate) fn host(&self) -> &Arc<dyn HostServices> {
        self.inner.host.get().expect(Inner::UNINITIALISED)
    }

    pub(crate) fn auth(&self) -> &Arc<AuthManager> {
        self.inner.auth.get().expect(Inner::UNINITIALISED)
    }

    pub(crate) fn vpn(&self) -> &Arc<VpnManager> {
        self.inner.vpn.get().expect(Inner::UNINITIALISED)
    }

    pub(crate) fn metrics(&self) -> &TobogganingMetrics {
        self.inner.metrics.get().expect(Inner::UNINITIALISED)
    }

    fn is_running(&self) -> bool {
        self.inner.running.load(Ordering::SeqCst)
    }
}

/// Builds a fresh, un-initialised [`TobogganingModule`] — the
/// [`penguin_sdk::Factory`] registered for the built-in `"tobogganing"`
/// module (see `penguin-registry`).
pub fn factory() -> Box<dyn Module> {
    Box::new(TobogganingModule::new())
}

#[async_trait]
impl Module for TobogganingModule {
    /// `license_feature` is deliberately empty: Tobogganing is core
    /// product and ships in the Free tier, so the module itself must load
    /// without a license server. Enterprise-only capabilities *inside*
    /// Tobogganing (none yet) would each be gated individually via
    /// `host.license().feature_enabled("penguin.<feature>")`.
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "tobogganing".to_string(),
            version: "1.0.0".to_string(),
            description: "WireGuard-compatible SASE/ZTNA endpoint client".to_string(),
            license_feature: String::new(),
        }
    }

    /// Resolves config (defaults, then the host's raw YAML — see
    /// `config.rs`'s doc for why `manager_url`/`node_id` are checked here
    /// even though the schema also marks them required), builds the auth
    /// manager, selects and wraps a [`WireGuardBackend`] via
    /// [`select_backend`], and registers all six metrics. Never begins
    /// background work — that is [`Module::start`]'s job.
    async fn init(&self, host: Arc<dyn HostServices>) -> Result<(), ModuleError> {
        let logger = host.logger();

        let raw = host.config();
        let mut cfg = ModuleConfig::default();
        if !raw.is_empty() {
            cfg = serde_norway::from_slice(&raw)
                .map_err(|err| ModuleError::new(format!("failed to parse config: {err}")))?;
        }

        if cfg.manager_url.is_empty() {
            return Err(ModuleError::new("manager_url is required"));
        }
        if cfg.node_id.is_empty() {
            return Err(ModuleError::new("node_id is required"));
        }

        logger.info(
            "tobogganing config loaded",
            &[
                ("manager_url", cfg.manager_url.as_str()),
                ("node_id", cfg.node_id.as_str()),
                ("interface", cfg.interface_name.as_str()),
            ],
        );

        let auth = Arc::new(AuthManager::new(cfg.manager_url.clone(), host.secrets()).await);

        // Tests inject a `FakeBackend` here (see `set_backend_for_test`) so
        // they can observe a genuine successful connect/reconnect — the
        // real backends either need root+netlink (`kernel`) or
        // unconditionally report `Unsupported` (`userspace`), so neither
        // can ever be driven to a successful `apply` from a test.
        let overridden_backend = self.inner.test_backend.lock().unwrap().clone();
        let backend: Arc<dyn WireGuardBackend> = if let Some(backend) = overridden_backend {
            backend
        } else {
            Arc::from(select_backend(cfg.embedded))
        };
        logger.info(
            "wireguard backend selected",
            &[
                ("embedded", &cfg.embedded.to_string()),
                ("backend", backend.kind().as_str()),
            ],
        );
        let vpn = Arc::new(VpnManager::new(cfg, host.data_dir(), backend));

        let metrics = TobogganingMetrics::register(host.metrics().as_ref())
            .map_err(|err| ModuleError::new(format!("register metrics: {err}")))?;

        logger.info("tobogganing module initialized", &[]);

        // `OnceLock::set` returning `Err` would mean `init` ran twice —
        // impossible per the `Module::init` contract, so a violation here
        // is a supervisor bug, not a condition this method needs to
        // handle gracefully.
        let _ = self.inner.host.set(host);
        let _ = self.inner.auth.set(auth);
        let _ = self.inner.vpn.set(vpn);
        let _ = self.inner.metrics.set(metrics);

        Ok(())
    }

    /// Starts the background refresh/monitor loops and kicks off an
    /// initial connect attempt, then returns promptly.
    ///
    /// Two Go fixes live here:
    ///
    /// 1. **The stop signal is recreated on every call.** Go's `stopCh`
    ///    was a single `chan struct{}` created once in `New` and `close`d
    ///    once by `Stop` — and a channel, once closed, stays closed
    ///    forever. A `Start` after a `Stop` still spawned fresh
    ///    goroutines, but they immediately saw the (permanently closed)
    ///    channel as ready and exited on their very first `select` call —
    ///    silently dead background work with no error anywhere. Here, a
    ///    brand new [`CancellationToken`] is created on every `start()`,
    ///    so loops spawned by a later `start` are driven by a token that
    ///    has never been cancelled.
    /// 2. **A real reconnect path.** Go's `monitorLoop` doc comment
    ///    claimed the monitor loop "retries" a failed initial connect, but
    ///    the loop only ever called `updateHealthProbe` — it never
    ///    attempted to reconnect anything. [`monitor_loop`] below actually
    ///    retries [`establish_tunnel`] whenever the tunnel is not
    ///    currently connected.
    async fn start(&self) -> Result<(), ModuleError> {
        if self.inner.running.swap(true, Ordering::SeqCst) {
            return Err(ModuleError::new("module already running"));
        }

        self.metrics().tunnel_up.set(0.0);
        self.host()
            .logger()
            .info("starting tobogganing module", &[]);

        let cancel = CancellationToken::new();
        *self.inner.cancel.lock().unwrap() = Some(cancel.clone());

        let monitor_module = self.clone();
        let monitor_cancel = cancel.clone();
        tokio::spawn(async move { monitor_loop(monitor_module, monitor_cancel).await });

        let refresh_module = self.clone();
        let refresh_cancel = cancel.clone();
        tokio::spawn(async move { auth_refresh_loop(refresh_module, refresh_cancel).await });

        // A first, synchronous, purely-local health probe so `health()`
        // never reads "Healthy" before any real check has run — see
        // `health`'s own doc for the Go bug this closes.
        update_health_probe(self).await;

        // Initial connect happens off the `start` path: `start` must
        // return promptly (the supervisor holds its lock across it; a
        // blocking `start` wedges the whole daemon), matching Go's `go
        // m.initialConnect(ctx)`. Fix #2 above means a failed initial
        // connect no longer strands the module — the monitor loop retries
        // it on every tick.
        let connect_module = self.clone();
        tokio::spawn(async move { initial_connect(&connect_module).await });

        Ok(())
    }

    /// Halts all background work and tears down the tunnel/token. Runs
    /// every teardown step even when an earlier one fails, and — fixing a
    /// Go bug — surfaces those failures instead of always reporting
    /// success: Go's `Stop` logged a failed `Disconnect`/`RevokeToken` and
    /// then unconditionally `return nil`, so neither a caller nor the
    /// supervisor could ever tell a clean shutdown from one that left the
    /// tunnel or token in an unknown state. Idempotent.
    async fn stop(&self) -> Result<(), ModuleError> {
        if !self.inner.running.swap(false, Ordering::SeqCst) {
            return Ok(());
        }

        self.host()
            .logger()
            .info("stopping tobogganing module", &[]);

        if let Some(cancel) = self.inner.cancel.lock().unwrap().take() {
            cancel.cancel();
        }

        let mut errors = Vec::new();

        if let Err(err) = self.vpn().disconnect().await {
            self.host().logger().error(
                "failed to disconnect tunnel",
                &[("error", &err.to_string())],
            );
            errors.push(format!("disconnect: {err}"));
        }

        if let Err(err) = self.auth().revoke_token().await {
            self.host()
                .logger()
                .warn("failed to revoke token", &[("error", &err.to_string())]);
            errors.push(format!("revoke: {err}"));
        }

        self.metrics().tunnel_up.set(0.0);
        self.host().logger().info("tobogganing module stopped", &[]);

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ModuleError::new(format!(
                "stop errors: {}",
                errors.join("; ")
            )))
        }
    }

    /// Reports the module's running state, the tunnel's up/down status,
    /// and the manager endpoint/node ID — matches Go's `Status` detail
    /// keys exactly.
    async fn status(&self) -> Result<Status, ModuleError> {
        let state = if self.is_running() {
            ModuleState::Running
        } else {
            ModuleState::Disabled
        };

        let mut detail = HashMap::new();
        detail.insert("tunnel".to_string(), "down".to_string());

        if let Some(vpn) = self.inner.vpn.get() {
            if vpn.is_connected().await {
                detail.insert("tunnel".to_string(), "up".to_string());
            }
            let cfg = vpn.config();
            detail.insert("endpoint".to_string(), cfg.manager_url.clone());
            detail.insert("node_id".to_string(), cfg.node_id.clone());
        }

        Ok(Status { state, detail })
    }

    /// A cheap liveness/degradation probe: the last value
    /// [`update_health_probe`] computed, or — fixing a Go bug — an
    /// explicit "not yet probed" [`HealthLevel::Unhealthy`] report if no
    /// probe has run at all. Go's `HealthProbe` zero value has
    /// `Level: sdk.HealthLevel(0)`, and `0` is `Healthy` — so a module
    /// that had never run a single check read as perfectly healthy, with
    /// a `CheckedAt` of the zero `time.Time` that nothing downstream
    /// double-checked. `start()` always runs one synchronous probe before
    /// returning, so the fallback here is only ever observable before the
    /// very first `start()`.
    async fn health(&self) -> HealthReport {
        let guard = self.inner.last_health.lock().unwrap();
        match &*guard {
            Some(report) => report.clone(),
            None => HealthReport {
                level: HealthLevel::Unhealthy,
                message: "no health probe has run yet".to_string(),
                checked_at: SystemTime::now(),
            },
        }
    }

    /// Declares Tobogganing's CLI command tree.
    fn commands(&self) -> Vec<CommandSpec> {
        commands::command_tree()
    }

    /// Executes one Tobogganing CLI command.
    async fn dispatch(
        &self,
        path: &[String],
        flags: &HashMap<String, String>,
        args: &[String],
    ) -> Result<CommandResult, ModuleError> {
        commands::dispatch(self, path, flags, args).await
    }

    /// Returns [`crate::config::CONFIG_SCHEMA`] for the daemon to validate
    /// `tobogganing.yaml` against before `init` ever sees it.
    fn config_schema(&self) -> Option<Vec<u8>> {
        Some(crate::config::CONFIG_SCHEMA.as_bytes().to_vec())
    }
}

/// Ensures a valid token, then connects the tunnel — the single path
/// [`Module::start`]'s initial connect, [`monitor_loop`]'s reconnect
/// retries, and the `connect` CLI command ([`crate::commands::dispatch`])
/// all share. Requires the module to already be running, matching Go's
/// `establishTunnel`.
pub(crate) async fn establish_tunnel(module: &TobogganingModule) -> Result<(), ModuleError> {
    if !module.is_running() {
        return Err(ModuleError::new("module not running"));
    }

    module
        .auth()
        .ensure_valid_token()
        .await
        .map_err(|err| ModuleError::new(format!("failed to obtain token: {err}")))?;

    if let Err(err) = module.vpn().connect(module.auth()).await {
        module.metrics().conn_errors.inc();
        return Err(ModuleError::new(format!(
            "failed to establish tunnel: {err}"
        )));
    }

    module.metrics().tunnel_up.set(1.0);
    module.host().logger().info("tunnel established", &[]);
    Ok(())
}

/// The `start()` background task performing the first connect attempt.
/// Failures are logged and left for [`monitor_loop`] to retry — matches
/// Go's `initialConnect`, which likewise never propagates its error
/// anywhere but the log (the whole point of running it off the `start`
/// path).
async fn initial_connect(module: &TobogganingModule) {
    if let Err(err) = establish_tunnel(module).await {
        module.host().logger().error(
            "failed to establish initial tunnel",
            &[("error", &err.to_string())],
        );
    }
}

/// Periodically refreshes the token before it expires.
///
/// Calls [`AuthManager::ensure_valid_token`], not a bare
/// `AuthManager::refresh_token` — this is the fix for the Go bug this
/// milestone's brief calls out explicitly: Go's loop called only
/// `RefreshToken`, which fails outright once the refresh token itself is
/// rejected (or absent), with no fallback — so a single bad refresh token
/// made every subsequent tick fail forever, and the module never
/// re-authenticated. `ensure_valid_token` already contains the
/// refresh-then-fall-back-to-API-key logic Go's own `EnsureValidToken` had
/// (see `auth.rs`); this loop only needed to call it instead.
async fn auth_refresh_loop(module: TobogganingModule, cancel: CancellationToken) {
    let interval =
        Duration::from_millis(module.inner.auth_refresh_interval_ms.load(Ordering::SeqCst));
    let mut ticker = tokio::time::interval(interval);
    // Tokio's first tick fires immediately; Go's `time.NewTicker` only
    // fires after the first full interval elapses. Discarding this first
    // tick matches that.
    ticker.tick().await;

    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            _ = ticker.tick() => {
                module.inner.auth_refresh_ticks.fetch_add(1, Ordering::SeqCst);
                if !module.auth().is_token_expired(AUTH_REFRESH_THRESHOLD).await {
                    continue;
                }
                module.host().logger().debug("token expiring soon, refreshing...", &[]);
                if let Err(err) = module.auth().ensure_valid_token().await {
                    module.host().logger().error("token refresh failed", &[("error", &err.to_string())]);
                    module.metrics().conn_errors.inc();
                    continue;
                }
                module.metrics().token_refreshes.inc();
                module.host().logger().debug("token refreshed", &[]);
            }
        }
    }
}

/// Periodically checks tunnel health, updates metrics, and — fixing the
/// Go gap documented on [`Module::start`] — actually retries a
/// failed/dropped connection.
async fn monitor_loop(module: TobogganingModule, cancel: CancellationToken) {
    let interval = Duration::from_millis(module.inner.monitor_interval_ms.load(Ordering::SeqCst));
    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await;

    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            _ = ticker.tick() => {
                module.inner.monitor_ticks.fetch_add(1, Ordering::SeqCst);
                update_health_probe(&module).await;

                if !module.vpn().is_connected().await
                    && let Err(err) = establish_tunnel(&module).await
                {
                    module.host().logger().warn("reconnect attempt failed", &[("error", &err.to_string())]);
                }
            }
        }
    }
}

/// Checks tunnel handshake freshness via a real device read
/// ([`crate::vpn::VpnManager::peer_stats`], never a value cached at
/// connect time) and updates both the cached [`HealthReport`] and the
/// `handshake_age`/`rx_bytes`/`tx_bytes` metrics. Cheap and
/// backend-read-only — safe to run synchronously from `start()` as well
/// as from [`monitor_loop`]'s ticker.
async fn update_health_probe(module: &TobogganingModule) {
    if !module.vpn().is_connected().await {
        set_health(module, HealthLevel::Unhealthy, "tunnel is not connected");
        return;
    }

    let stats = match module.vpn().peer_stats().await {
        Ok(stats) => stats,
        Err(err) => {
            set_health(
                module,
                HealthLevel::Unhealthy,
                format!("failed to read tunnel stats: {err}"),
            );
            return;
        }
    };

    module
        .metrics()
        .record_bytes(stats.rx_bytes, stats.tx_bytes);

    let Some(last_handshake) = stats.last_handshake else {
        set_health(module, HealthLevel::Degraded, "no handshake yet");
        return;
    };

    let age = SystemTime::now()
        .duration_since(last_handshake)
        .unwrap_or(Duration::ZERO);
    module.metrics().handshake_age.set(age.as_secs_f64());

    if age > HANDSHAKE_DEGRADED_AGE {
        set_health(
            module,
            HealthLevel::Degraded,
            format!("last handshake was {}s ago", age.as_secs()),
        );
        return;
    }

    set_health(
        module,
        HealthLevel::Healthy,
        "tunnel is connected and healthy",
    );
}

fn set_health(module: &TobogganingModule, level: HealthLevel, message: impl Into<String>) {
    let mut guard = module.inner.last_health.lock().unwrap();
    *guard = Some(HealthReport {
        level,
        message: message.into(),
        checked_at: SystemTime::now(),
    });
}

/// Test-only hooks: overridable loop intervals (production defaults are
/// far too slow for a test to wait on) and liveness counters proving the
/// background loops are genuinely still ticking — see
/// `start_stop_start_leaves_refresh_and_monitor_loops_alive` below.
#[cfg(test)]
impl TobogganingModule {
    /// Injects `backend` in place of [`select_backend`]'s real choice — the
    /// one seam this crate needed to add for tests: `init` otherwise always
    /// calls `select_backend` itself, with no way for a test to substitute
    /// a [`crate::wireguard::fake::FakeBackend`]. Must be called before
    /// `init`, since that is where the override is read.
    pub(crate) fn set_backend_for_test(&self, backend: Arc<dyn WireGuardBackend>) {
        *self.inner.test_backend.lock().unwrap() = Some(backend);
    }

    pub(crate) fn set_monitor_interval_for_test(&self, interval: Duration) {
        self.inner
            .monitor_interval_ms
            .store(interval.as_millis() as u64, Ordering::SeqCst);
    }

    pub(crate) fn set_auth_refresh_interval_for_test(&self, interval: Duration) {
        self.inner
            .auth_refresh_interval_ms
            .store(interval.as_millis() as u64, Ordering::SeqCst);
    }

    pub(crate) fn monitor_tick_count(&self) -> u64 {
        self.inner.monitor_ticks.load(Ordering::SeqCst)
    }

    pub(crate) fn auth_refresh_tick_count(&self) -> u64 {
        self.inner.auth_refresh_ticks.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use penguin_sdk::SecretStore;

    use crate::testutil::{FakeHost, MockManager, MockResponse};
    use crate::wireguard::PeerStats;
    use crate::wireguard::fake::{FakeBackend, RecordedCall};

    use super::*;

    fn valid_config_bytes() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "manager_url": "http://127.0.0.1:1",
            "node_id": "test-node",
        }))
        .unwrap()
    }

    async fn init_module() -> TobogganingModule {
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = valid_config_bytes();
        let module = TobogganingModule::new();
        module.init(Arc::new(host)).await.expect("init succeeds");
        module
    }

    async fn wait_for(mut condition: impl FnMut() -> bool) {
        for _ in 0..300 {
            if condition() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("condition not met within timeout");
    }

    #[tokio::test]
    async fn info_reports_tobogganing_identity_with_no_license_gate() {
        let module = TobogganingModule::new();
        let info = module.info();
        assert_eq!(info.name, "tobogganing");
        assert!(info.license_feature.is_empty());
        assert!(!info.description.is_empty());
    }

    #[test]
    #[should_panic(expected = "tobogganing module used before init")]
    fn accessors_panic_before_init() {
        let module = TobogganingModule::new();
        let _ = module.vpn();
    }

    #[tokio::test]
    async fn init_requires_manager_url() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = serde_json::to_vec(&serde_json::json!({})).unwrap();
        let module = TobogganingModule::new();
        let err = module.init(Arc::new(host)).await.unwrap_err();
        assert!(err.to_string().contains("manager_url"));
    }

    #[tokio::test]
    async fn init_requires_node_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = serde_json::to_vec(&serde_json::json!({"manager_url": "http://x"})).unwrap();
        let module = TobogganingModule::new();
        let err = module.init(Arc::new(host)).await.unwrap_err();
        assert!(err.to_string().contains("node_id"));
    }

    /// Regression: Go's `HealthProbe` zero value read as `Healthy` before
    /// any check had ever run.
    #[tokio::test]
    async fn health_before_any_probe_is_never_healthy() {
        let module = init_module().await;
        let health = module.health().await;
        assert_ne!(health.level, HealthLevel::Healthy);
    }

    #[tokio::test]
    async fn start_makes_a_real_not_connected_health_report_available_immediately() {
        let module = init_module().await;
        module.start().await.expect("start succeeds");
        let health = module.health().await;
        assert_eq!(health.level, HealthLevel::Unhealthy);
        assert!(health.message.contains("not connected"));
        module.stop().await.ok();
    }

    #[tokio::test]
    async fn start_returns_promptly_with_an_unreachable_manager() {
        let module = init_module().await;
        let started = std::time::Instant::now();
        module.start().await.expect("start succeeds");
        assert!(started.elapsed() < Duration::from_secs(1));
        module.stop().await.ok();
    }

    #[tokio::test]
    async fn start_twice_without_stop_errors() {
        let module = init_module().await;
        module.start().await.unwrap();
        let err = module.start().await.unwrap_err();
        assert!(err.to_string().contains("already running"));
        module.stop().await.ok();
    }

    #[tokio::test]
    async fn stop_without_start_is_idempotent() {
        let module = init_module().await;
        module.stop().await.expect("stop without start is a no-op");
    }

    #[tokio::test]
    async fn stop_twice_is_idempotent() {
        let module = init_module().await;
        module.start().await.unwrap();
        module.stop().await.ok();
        module.stop().await.expect("second stop is a no-op");
    }

    /// Regression for the Go bug this milestone's brief calls out: Go's
    /// `stopCh` was created once and closed once, so a `Start` after a
    /// `Stop` spawned loop goroutines that immediately saw it as
    /// already-closed and exited on their first `select` — silently dead
    /// background work. This proves both loops are genuinely alive (still
    /// ticking) after a second `start()`, not just that `start()` itself
    /// returns without error.
    #[tokio::test]
    async fn start_stop_start_leaves_refresh_and_monitor_loops_alive() {
        let module = init_module().await;
        module.set_monitor_interval_for_test(Duration::from_millis(15));
        module.set_auth_refresh_interval_for_test(Duration::from_millis(15));

        module.start().await.unwrap();
        wait_for(|| module.monitor_tick_count() > 0).await;
        wait_for(|| module.auth_refresh_tick_count() > 0).await;
        module.stop().await.ok();

        module.start().await.unwrap();
        let monitor_before = module.monitor_tick_count();
        let refresh_before = module.auth_refresh_tick_count();
        wait_for(|| module.monitor_tick_count() > monitor_before).await;
        wait_for(|| module.auth_refresh_tick_count() > refresh_before).await;

        module.stop().await.ok();
    }

    #[tokio::test]
    async fn status_reports_tunnel_down_and_manager_detail_before_connect() {
        let module = init_module().await;
        let status = module.status().await.unwrap();
        assert_eq!(
            status.detail.get("tunnel").map(String::as_str),
            Some("down")
        );
        assert_eq!(
            status.detail.get("node_id").map(String::as_str),
            Some("test-node")
        );
    }

    #[tokio::test]
    async fn dispatch_no_command_is_a_nonzero_exit() {
        let module = init_module().await;
        let result = module.dispatch(&[], &HashMap::new(), &[]).await.unwrap();
        assert_ne!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn dispatch_unknown_command_is_a_nonzero_exit() {
        let module = init_module().await;
        let result = module
            .dispatch(&["bogus".to_string()], &HashMap::new(), &[])
            .await
            .unwrap();
        assert_ne!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn dispatch_status_json_reports_state() {
        let module = init_module().await;
        let mut flags = HashMap::new();
        flags.insert("json".to_string(), "true".to_string());
        let result = module
            .dispatch(&["status".to_string()], &flags, &[])
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(!result.json.is_empty());
    }

    #[tokio::test]
    async fn dispatch_connect_before_start_fails_module_not_running() {
        let module = init_module().await;
        let result = module
            .dispatch(&["connect".to_string()], &HashMap::new(), &[])
            .await
            .unwrap();
        assert_ne!(result.exit_code, 0);
        assert!(result.output.contains("module not running"));
    }

    #[tokio::test]
    async fn dispatch_disconnect_when_not_connected_succeeds() {
        let module = init_module().await;
        let result = module
            .dispatch(&["disconnect".to_string()], &HashMap::new(), &[])
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn dispatch_rotate_before_connect_fails() {
        let module = init_module().await;
        let result = module
            .dispatch(&["rotate".to_string()], &HashMap::new(), &[])
            .await
            .unwrap();
        assert_ne!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn config_schema_is_present_and_valid_json() {
        let module = TobogganingModule::new();
        let schema = module.config_schema().expect("schema present");
        let _: serde_json::Value = serde_json::from_slice(&schema).expect("valid JSON");
    }

    #[tokio::test]
    async fn default_impl_builds_a_fresh_unrunning_module() {
        let module = TobogganingModule::default();
        assert!(!module.is_running());
    }

    #[tokio::test]
    async fn init_surfaces_a_config_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = b"{not valid yaml or json".to_vec();
        let module = TobogganingModule::new();
        let err = module.init(Arc::new(host)).await.unwrap_err();
        assert!(err.to_string().contains("failed to parse config"));
    }

    /// Regression for a class of bug this module never had a test to catch:
    /// a second module sharing the same host's Prometheus registry must
    /// surface the resulting duplicate-collector error from `init`, not
    /// panic or silently drop the metrics.
    #[tokio::test]
    async fn init_surfaces_a_metrics_registration_failure() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = valid_config_bytes();
        let host: Arc<dyn HostServices> = Arc::new(host);

        let first = TobogganingModule::new();
        first.init(host.clone()).await.expect("first init succeeds");

        let second = TobogganingModule::new();
        let err = second.init(host).await.unwrap_err();
        assert!(err.to_string().contains("register metrics"));
    }

    fn reconnect_test_config_bytes(manager_url: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "manager_url": manager_url,
            "node_id": "test-node",
        }))
        .unwrap()
    }

    async fn seed_token_route(manager: &MockManager) {
        manager
            .respond(
                "POST",
                "/api/v1/auth/token",
                MockResponse::json(200, r#"{"access_token":"tok","expires_at":9999999999}"#),
            )
            .await;
    }

    async fn seed_config_route(manager: &MockManager) {
        manager
            .respond(
                "GET",
                "/api/v1/config",
                MockResponse::json(
                    200,
                    r#"{"tunnel_address":"10.0.0.2/32","server_public_key":"sMlWwt2d4gkKsPl6gWAGqtEgp2Xo2S4xyJ1wFjNsFEs=","server_endpoint":"203.0.113.1:51820","allowed_ips":["10.0.0.0/24"],"dns":[]}"#,
                ),
            )
            .await;
    }

    async fn seed_token_and_config_routes(manager: &MockManager) {
        seed_token_route(manager).await;
        seed_config_route(manager).await;
    }

    /// Builds and initializes a module wired to `backend` — see
    /// `set_backend_for_test`'s doc for why a test needs this seam at all.
    async fn init_module_with_backend(
        manager_url: &str,
        backend: Arc<dyn WireGuardBackend>,
    ) -> TobogganingModule {
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.secrets.set("api_key", b"k").await.unwrap();
        host.config = reconnect_test_config_bytes(manager_url);
        let module = TobogganingModule::new();
        module.set_backend_for_test(backend);
        module.init(Arc::new(host)).await.expect("init succeeds");
        module
    }

    /// Obtains a token and connects the tunnel directly (bypassing
    /// `start()`'s background loops entirely), so health-probe tests can
    /// drive `update_health_probe` deterministically instead of racing a
    /// ticker.
    async fn connect_directly(module: &TobogganingModule) {
        module
            .auth()
            .ensure_valid_token()
            .await
            .expect("token obtained");
        module
            .vpn()
            .connect(module.auth())
            .await
            .expect("connect succeeds");
    }

    /// Regression for the Go bug this milestone's brief calls out
    /// explicitly: Go's `monitorLoop` doc comment claimed it "retries" a
    /// failed initial connect, but the loop only ever updated the health
    /// probe — it never actually reconnected anything. This proves the
    /// fix end-to-end: a failed initial connect is followed by a real,
    /// successful reconnect driven by `monitor_loop`, with no test-only
    /// shortcut into `establish_tunnel` itself.
    #[tokio::test]
    async fn monitor_loop_reconnects_after_a_failed_initial_connect() {
        let manager = MockManager::start().await;
        seed_token_and_config_routes(&manager).await;

        let backend = Arc::new(FakeBackend::new());
        backend.fail_next_apply();

        let module = init_module_with_backend(&manager.base_url, backend.clone()).await;
        module.set_monitor_interval_for_test(Duration::from_millis(15));
        module.set_auth_refresh_interval_for_test(Duration::from_millis(15));

        module.start().await.expect("start succeeds");

        // start()'s own initial-connect attempt hits the injected failure;
        // only monitor_loop's reconnect retry can bring the interface up.
        wait_for(|| backend.is_configured("wg0")).await;
        assert!(module.vpn().is_connected().await);

        let apply_calls = backend
            .calls()
            .into_iter()
            .filter(|c| matches!(c, RecordedCall::Apply { .. }))
            .count();
        assert!(
            apply_calls >= 2,
            "expected a failed initial attempt plus a successful monitor-loop retry, got {apply_calls}"
        );

        // The token was already valid (far-future expiry) at the auth
        // refresh loop's first tick, so it must have taken the "skip"
        // branch rather than attempting a refresh.
        wait_for(|| module.auth_refresh_tick_count() > 0).await;
        assert_eq!(module.auth().token().await, "tok");

        module.stop().await.ok();
        manager.stop().await;
    }

    /// `VpnManager::disconnect` propagates a failing teardown, and `stop`
    /// collects it — but nothing previously drove a failing teardown
    /// through either, so this Go-parity fix (see `stop`'s doc: Go always
    /// `return nil`, even after a failed `Disconnect`) had no proof it
    /// actually worked.
    #[tokio::test]
    async fn stop_surfaces_a_failing_disconnect_instead_of_silently_succeeding() {
        let manager = MockManager::start().await;
        seed_token_and_config_routes(&manager).await;
        manager
            .respond("POST", "/api/v1/auth/revoke", MockResponse::empty(200))
            .await;

        let backend = Arc::new(FakeBackend::new());
        let module = init_module_with_backend(&manager.base_url, backend.clone()).await;
        module.start().await.expect("start succeeds");

        wait_for(|| backend.is_configured("wg0")).await;

        backend.fail_next_teardown();
        let err = module.stop().await.unwrap_err();
        assert!(
            err.to_string().contains("disconnect:"),
            "stop must surface the teardown failure, got: {err}"
        );

        manager.stop().await;
    }

    fn unix_seconds_from_now(delta: i64) -> i64 {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        now + delta
    }

    /// Regression for the Go bug this milestone's brief calls out: the
    /// refresh loop must call `ensure_valid_token` (refresh-then-fall-back)
    /// rather than a bare refresh, and must genuinely install the result.
    /// Drives `auth_refresh_loop` directly (not through `start()`) so this
    /// is a controlled test of that loop alone, not entangled with
    /// `monitor_loop`/`initial_connect` also racing for the same token.
    ///
    /// The primed token's `expires_at` is already in the past:
    /// `ensure_valid_token` itself only ever attempts a real refresh/obtain
    /// once a token has actually expired (its own check is a zero
    /// threshold) — merely being within `AUTH_REFRESH_THRESHOLD` is what
    /// makes the *loop* attempt the call, not what makes `ensure_valid_token`
    /// do real work once it's in there.
    #[tokio::test]
    async fn auth_refresh_loop_reobtains_a_token_once_it_has_actually_expired() {
        let manager = MockManager::start().await;
        let already_expired = unix_seconds_from_now(-5);
        manager
            .respond(
                "POST",
                "/api/v1/auth/token",
                MockResponse::json(
                    200,
                    format!(r#"{{"access_token":"tok-1","expires_at":{already_expired}}}"#),
                ),
            )
            .await;
        manager
            .respond(
                "POST",
                "/api/v1/auth/token",
                MockResponse::json(200, r#"{"access_token":"tok-2","expires_at":9999999999}"#),
            )
            .await;

        let backend = Arc::new(FakeBackend::new());
        let module = init_module_with_backend(&manager.base_url, backend).await;
        module
            .auth()
            .ensure_valid_token()
            .await
            .expect("primes an already-expired cached token");
        assert_eq!(module.auth().token().await, "tok-1");

        module.set_auth_refresh_interval_for_test(Duration::from_millis(15));
        let cancel = CancellationToken::new();
        let loop_module = module.clone();
        let loop_cancel = cancel.clone();
        let handle = tokio::spawn(async move { auth_refresh_loop(loop_module, loop_cancel).await });

        let mut refreshed = false;
        for _ in 0..300 {
            if module.auth().token().await == "tok-2" {
                refreshed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        cancel.cancel();
        handle.await.ok();
        manager.stop().await;

        assert!(
            refreshed,
            "auth_refresh_loop must re-obtain a token that has actually expired"
        );
    }

    #[tokio::test]
    async fn health_probe_reports_degraded_when_connected_with_no_handshake_yet() {
        let manager = MockManager::start().await;
        seed_token_and_config_routes(&manager).await;
        let backend = Arc::new(FakeBackend::new());
        let module = init_module_with_backend(&manager.base_url, backend).await;
        connect_directly(&module).await;

        update_health_probe(&module).await;

        let health = module.health().await;
        assert_eq!(health.level, HealthLevel::Degraded);
        assert!(health.message.contains("no handshake"));

        manager.stop().await;
    }

    #[tokio::test]
    async fn health_probe_reports_healthy_with_a_fresh_handshake() {
        let manager = MockManager::start().await;
        seed_token_and_config_routes(&manager).await;
        let backend = Arc::new(FakeBackend::new());
        let module = init_module_with_backend(&manager.base_url, backend.clone()).await;
        connect_directly(&module).await;

        backend.set_peer_stats(PeerStats {
            last_handshake: Some(SystemTime::now()),
            rx_bytes: 4096,
            tx_bytes: 2048,
        });
        update_health_probe(&module).await;

        let health = module.health().await;
        assert_eq!(health.level, HealthLevel::Healthy);
        assert_eq!(module.metrics().rx_bytes.get(), 4096.0);
        assert_eq!(module.metrics().tx_bytes.get(), 2048.0);

        manager.stop().await;
    }

    #[tokio::test]
    async fn health_probe_reports_degraded_when_the_last_handshake_is_stale() {
        let manager = MockManager::start().await;
        seed_token_and_config_routes(&manager).await;
        let backend = Arc::new(FakeBackend::new());
        let module = init_module_with_backend(&manager.base_url, backend.clone()).await;
        connect_directly(&module).await;

        let stale = SystemTime::now() - HANDSHAKE_DEGRADED_AGE - Duration::from_secs(30);
        backend.set_peer_stats(PeerStats {
            last_handshake: Some(stale),
            rx_bytes: 0,
            tx_bytes: 0,
        });
        update_health_probe(&module).await;

        let health = module.health().await;
        assert_eq!(health.level, HealthLevel::Degraded);
        assert!(health.message.contains("ago"));

        manager.stop().await;
    }

    /// Fixes the Go bug documented on [`PeerStats`]: stats must come from a
    /// real device read on every probe, which means a read that fails must
    /// be surfaced as `Unhealthy`, not silently keep the previous report.
    #[tokio::test]
    async fn health_probe_reports_unhealthy_when_the_backend_read_fails() {
        let manager = MockManager::start().await;
        seed_token_and_config_routes(&manager).await;
        let backend = Arc::new(FakeBackend::new());
        let module = init_module_with_backend(&manager.base_url, backend.clone()).await;
        connect_directly(&module).await;

        backend.fail_next_peer_stats();
        update_health_probe(&module).await;

        let health = module.health().await;
        assert_eq!(health.level, HealthLevel::Unhealthy);
        assert!(health.message.contains("failed to read tunnel stats"));

        manager.stop().await;
    }
}
