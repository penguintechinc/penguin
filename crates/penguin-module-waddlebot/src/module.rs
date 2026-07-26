//! The waddlebot `penguin_sdk::Module` implementation: lifecycle glue
//! wiring [`waddlebot_client::WaddlebotClient`] (the hub REST client — see
//! that crate, already built and tested) to the daemon supervisor and to
//! [`crate::commands`]'s CLI surface.
//!
//! # License gating
//!
//! `info().license_feature` is deliberately empty, matching
//! `penguin-module-squawk`/`penguin-module-tobogganing`: the module must
//! load with no license server reachable at all. Whether waddlebot itself
//! should eventually sit behind a PenguinTech entitlement tier is a
//! deliberate *future* decision this track does not make by default — see
//! `~/.claude/rules/general.md`'s License Tiers table for the tiers such a
//! decision would choose from.
//!
//! # The server-side CAT auth gap
//!
//! As documented on [`waddlebot_client::WaddlebotError::Auth`], the hub's
//! `requireAuth` middleware does not yet call its own CAT resolver
//! (`waddlebot#155`), so every CAT-authenticated request 401s today
//! regardless of token validity. This module is built to the endpoint's
//! intended contract anyway: [`Module::init`] never fails on account of the
//! hub being unreachable or rejecting credentials — it only builds the
//! client and registers metrics, both purely local operations — and
//! [`Module::health`]/[`Module::status`] degrade gracefully (reporting
//! `unauthorized`/`unreachable`) rather than treating a failed probe as a
//! module-level failure.
//!
//! # The local dial-in bridge
//!
//! [`WaddlebotModule::start_bridge`]/[`WaddlebotModule::stop_bridge`] wire
//! [`crate::bridge`] — a loopback TCP/WebSocket + unix-socket server local
//! integration scripts connect *into* — to this module's lifecycle. See
//! [`crate::bridge`]'s doc for the full security model; in short, a
//! connecting script never sees this module's CAT, only a narrow,
//! bridge-minted local credential. A dial-**out** adapter (e.g. driving an
//! OBS connection from the daemon) remains a separate, later track — see
//! [`crate::bridge::BridgeAdapter`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;

use penguin_sdk::{
    CommandResult, CommandSpec, HealthLevel, HealthReport, HostServices, Module, ModuleError,
    ModuleInfo, ModuleState, Status,
};

use waddlebot_client::{Config as HubConfig, WaddlebotClient, WaddlebotError};

use crate::commands;
use crate::config::ModuleConfig;
use crate::metrics::WaddlebotMetrics;

/// How long a cached auth probe (shared by [`Module::status`] and
/// [`Module::health`]) is trusted before a fresh one runs. Matches
/// `penguin-module-squawk`'s own health-cache TTL.
const AUTH_CACHE_TTL: Duration = Duration::from_secs(5);
/// Upper bound on a single auth probe's round trip.
const AUTH_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// The secret key the module's Community Access Token is read from. Never a
/// config field — see [`WaddlebotModule::init`]'s doc.
const CAT_SECRET_KEY: &str = "cat";

/// The coarse outcome of a live probe against the hub, distinguishing
/// "credentials rejected" from "couldn't even talk to the hub" — finer
/// grained than [`HealthLevel`], which [`Module::health`] collapses this
/// into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthState {
    Ok,
    Unauthorized,
    Unreachable,
}

impl AuthState {
    fn as_str(self) -> &'static str {
        match self {
            AuthState::Ok => "ok",
            AuthState::Unauthorized => "unauthorized",
            AuthState::Unreachable => "unreachable",
        }
    }
}

/// One cached probe outcome plus the instant it ran — reused by both
/// [`Module::status`] and [`Module::health`] so neither forces its own
/// network round trip on every call, and so a cached report's `checked_at`
/// is always the probe's real time rather than the calling instant (the
/// bug documented on [`Module::health`]).
#[derive(Debug, Clone, Copy)]
struct AuthProbe {
    state: AuthState,
    checked_at: SystemTime,
}

/// [`WaddlebotModule`]'s real state, held behind an `Arc` so the module
/// itself stays a cheap `Clone`.
struct Inner {
    host: OnceLock<Arc<dyn HostServices>>,
    config: OnceLock<ModuleConfig>,
    /// The active hub connection. Not a `OnceLock`: `community use <id>`
    /// (see [`WaddlebotModule::set_community`]) rebuilds and swaps it,
    /// since [`WaddlebotClient`] bakes its community id in at construction
    /// and has no per-call override.
    client: StdMutex<Option<Arc<WaddlebotClient>>>,
    /// The CAT `init` read from secrets, kept so [`WaddlebotModule::set_community`]
    /// can rebuild a client without going back to the secret store. Never
    /// logged or placed in any command's output.
    cat: StdMutex<String>,
    community_id: AtomicI64,
    metrics: OnceLock<WaddlebotMetrics>,
    running: AtomicBool,
    last_probe: StdMutex<Option<AuthProbe>>,
    /// The running bridge's transports, while `bridge.enabled` and the
    /// module is started. `None` whenever the bridge is stopped/disabled —
    /// see [`WaddlebotModule::start_bridge`]/[`WaddlebotModule::stop_bridge`].
    bridge: StdMutex<Option<crate::bridge::BridgeHandle>>,
}

impl Inner {
    const UNINITIALISED: &'static str = "waddlebot module used before init";

    fn new() -> Inner {
        Inner {
            host: OnceLock::new(),
            config: OnceLock::new(),
            client: StdMutex::new(None),
            cat: StdMutex::new(String::new()),
            community_id: AtomicI64::new(0),
            metrics: OnceLock::new(),
            running: AtomicBool::new(false),
            last_probe: StdMutex::new(None),
            bridge: StdMutex::new(None),
        }
    }
}

/// waddlebot: a CLI-over-API surface for one community's slice of the
/// waddlebot hub, plus — in a separate, later track — a local integration
/// bridge. See this module's doc for the bridge seam and the current
/// server-side CAT auth gap.
///
/// A cheap `Clone` (an `Arc` around its real state) so background work
/// (none today, but the seam in [`WaddlebotModule::start_bridge`] may need
/// it) can hold a handle without borrowing `&self` past `start`'s return.
#[derive(Clone)]
pub struct WaddlebotModule {
    inner: Arc<Inner>,
}

impl Default for WaddlebotModule {
    fn default() -> WaddlebotModule {
        WaddlebotModule::new()
    }
}

impl WaddlebotModule {
    /// Builds a fresh, un-initialised module — the shape every
    /// [`penguin_sdk::Factory`] invocation (including [`factory`]) produces.
    pub fn new() -> WaddlebotModule {
        WaddlebotModule {
            inner: Arc::new(Inner::new()),
        }
    }

    /// The panic message every post-init accessor shares: every `Module`
    /// method besides `info`/`init` runs only after `init` has already
    /// populated every field below, so a miss here means the supervisor
    /// violated that contract, not a recoverable runtime condition.
    pub(crate) fn host(&self) -> &Arc<dyn HostServices> {
        self.inner.host.get().expect(Inner::UNINITIALISED)
    }

    pub(crate) fn config(&self) -> &ModuleConfig {
        self.inner.config.get().expect(Inner::UNINITIALISED)
    }

    pub(crate) fn metrics(&self) -> &WaddlebotMetrics {
        self.inner.metrics.get().expect(Inner::UNINITIALISED)
    }

    /// The currently active hub connection. Cloning the `Arc` is cheap;
    /// call this fresh at each use rather than holding it across an await,
    /// since `community use` can swap it out from under a long-running
    /// caller.
    pub(crate) fn client(&self) -> Arc<WaddlebotClient> {
        self.inner
            .client
            .lock()
            .expect("client mutex poisoned")
            .clone()
            .expect(Inner::UNINITIALISED)
    }

    /// The community the active client is currently scoped to.
    pub(crate) fn community_id(&self) -> i64 {
        self.inner.community_id.load(Ordering::SeqCst)
    }

    /// Rebuilds the hub client against `community_id`, keeping the same hub
    /// URL and CAT — the backing primitive for `community use <id>`.
    /// [`WaddlebotClient`] has no per-call community override (every
    /// admin-scoped URL bakes it in at construction), so switching the
    /// active community means swapping the whole client.
    pub(crate) fn set_community(&self, community_id: i64) -> Result<(), ModuleError> {
        let cat = self.inner.cat.lock().expect("cat mutex poisoned").clone();
        let hub_config = HubConfig {
            base_url: self.config().hub.base_url.clone(),
            community_id,
            cat,
            ..HubConfig::default()
        };
        let client = WaddlebotClient::new(hub_config)
            .map_err(|err| ModuleError::new(format!("rebuild hub client: {err}")))?;
        *self.inner.client.lock().expect("client mutex poisoned") = Some(Arc::new(client));
        self.inner
            .community_id
            .store(community_id, Ordering::SeqCst);
        Ok(())
    }

    /// Runs `fut` (one hub call) and updates `waddlebot_api_requests_total`
    /// / `waddlebot_api_errors_total` around it — the single choke point
    /// every command handler and the auth probe route through, so no call
    /// site can forget to count itself.
    pub(crate) async fn call<T>(
        &self,
        fut: impl std::future::Future<Output = Result<T, WaddlebotError>>,
    ) -> Result<T, WaddlebotError> {
        self.metrics().api_requests_total.inc();
        let result = fut.await;
        if result.is_err() {
            self.metrics().api_errors_total.inc();
        }
        result
    }

    /// A cheap, cached liveness/auth probe shared by [`Module::status`] and
    /// [`Module::health`]. `list_my_communities` is the least destructive
    /// authenticated call the client has, so it doubles as the probe.
    async fn probe_auth(&self) -> AuthProbe {
        {
            let cached = self.inner.last_probe.lock().expect("probe mutex poisoned");
            if let Some(probe) = *cached
                && let Ok(age) = SystemTime::now().duration_since(probe.checked_at)
                && age < AUTH_CACHE_TTL
            {
                return probe;
            }
        }

        let client = self.client();
        let attempt = self.call(client.list_my_communities());
        let state = match tokio::time::timeout(AUTH_PROBE_TIMEOUT, attempt).await {
            Ok(Ok(_communities)) => AuthState::Ok,
            Ok(Err(WaddlebotError::Auth { .. })) => AuthState::Unauthorized,
            Ok(Err(_other)) => AuthState::Unreachable,
            Err(_elapsed) => AuthState::Unreachable,
        };
        let probe = AuthProbe {
            state,
            checked_at: SystemTime::now(),
        };
        *self.inner.last_probe.lock().expect("probe mutex poisoned") = Some(probe);
        probe
    }

    /// Starts [`crate::bridge`] when `bridge.enabled` is set; a clean no-op
    /// otherwise. A bind failure (bad address, port in use, unix-socket
    /// setup failure) fails `start` outright — unlike the hub client, which
    /// tolerates being unreachable, a bridge that cannot actually bind its
    /// configured transports is a real misconfiguration, not something to
    /// degrade gracefully from.
    async fn start_bridge(&self) -> Result<(), ModuleError> {
        if !self.config().bridge.enabled {
            return Ok(());
        }

        let cat = self.inner.cat.lock().expect("cat mutex poisoned").clone();
        let deps = crate::bridge::BridgeDeps {
            module: self.clone(),
            cat,
            adapters: Vec::new(),
        };
        let handle = crate::bridge::start(&self.config().bridge, deps)
            .await
            .map_err(|err| ModuleError::new(format!("start bridge: {err}")))?;

        self.host().logger().info(
            "waddlebot bridge started",
            &[
                (
                    "tcp",
                    &handle
                        .tcp_local_addr()
                        .map(|addr| addr.to_string())
                        .unwrap_or_default(),
                ),
                (
                    "unix",
                    &handle
                        .unix_local_path()
                        .map(|path| path.display().to_string())
                        .unwrap_or_default(),
                ),
            ],
        );
        *self.inner.bridge.lock().expect("bridge mutex poisoned") = Some(handle);
        Ok(())
    }

    /// Tears down whatever [`WaddlebotModule::start_bridge`] stood up, if
    /// anything — a clean no-op when the bridge was never started
    /// (disabled, or `start` never ran).
    async fn stop_bridge(&self) -> Result<(), ModuleError> {
        let handle = self
            .inner
            .bridge
            .lock()
            .expect("bridge mutex poisoned")
            .take();
        if let Some(handle) = handle {
            handle.stop().await;
            self.host().logger().info("waddlebot bridge stopped", &[]);
        }
        Ok(())
    }
}

/// Builds a fresh, un-initialised [`WaddlebotModule`] — the
/// [`penguin_sdk::Factory`] registered for the built-in `"waddlebot"`
/// module (see `penguin-registry`).
pub fn factory() -> Box<dyn Module> {
    Box::new(WaddlebotModule::new())
}

#[async_trait]
impl Module for WaddlebotModule {
    /// Identity metadata for the daemon's module registry and `penguin
    /// status`. See this module's top-level doc for why `license_feature`
    /// is empty.
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "waddlebot".to_string(),
            version: "1.0.0".to_string(),
            description: "waddlebot local integration and CLI over the waddles hub".to_string(),
            license_feature: String::new(),
        }
    }

    /// Resolves config (defaults, then the host's validated YAML), reads
    /// the Community Access Token from the secret store — **never** from
    /// config, even if a document happened to carry one: a token is a
    /// secret, and routing it through `host.secrets()` is what the rest of
    /// this workspace relies on for credential handling — builds the hub
    /// client, and registers all three metrics.
    ///
    /// Deliberately never fails because the hub is unreachable or rejects
    /// the CAT: [`WaddlebotClient::new`] only builds an HTTP/TLS stack, it
    /// never touches the network, so nothing here *can* observe a
    /// connectivity or auth problem. The module loads either way; a bad hub
    /// URL or an invalid/absent CAT instead shows up through
    /// [`Module::health`]/[`Module::status`] once it starts trying to
    /// actually talk to the hub. See this module's top-level doc for the
    /// (currently 401-everything) server-side CAT auth gap this is built
    /// to tolerate.
    async fn init(&self, host: Arc<dyn HostServices>) -> Result<(), ModuleError> {
        let logger = host.logger();

        let raw = host.config();
        let cfg: ModuleConfig = if raw.is_empty() {
            ModuleConfig::default()
        } else {
            serde_norway::from_slice(&raw)
                .map_err(|err| ModuleError::new(format!("parse waddlebot config: {err}")))?
        };

        let cat = match host.secrets().get(CAT_SECRET_KEY).await {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_not_found_or_other) => String::new(),
        };

        let hub_config = HubConfig {
            base_url: cfg.hub.base_url.clone(),
            community_id: cfg.community_id,
            cat: cat.clone(),
            ..HubConfig::default()
        };
        let client = WaddlebotClient::new(hub_config)
            .map_err(|err| ModuleError::new(format!("create hub client: {err}")))?;

        let metrics = WaddlebotMetrics::register(host.metrics().as_ref())
            .map_err(|err| ModuleError::new(format!("register metrics: {err}")))?;

        logger.info(
            "waddlebot module initialized",
            &[
                ("hub", cfg.hub.base_url.as_str()),
                ("community_id", &cfg.community_id.to_string()),
                ("cat_present", &(!cat.is_empty()).to_string()),
            ],
        );

        *self.inner.client.lock().expect("client mutex poisoned") = Some(Arc::new(client));
        *self.inner.cat.lock().expect("cat mutex poisoned") = cat;
        self.inner
            .community_id
            .store(cfg.community_id, Ordering::SeqCst);
        // `OnceLock::set` returning `Err` would mean `init` ran twice —
        // impossible per the `Module::init` contract ("called exactly
        // once"), so a violation here is a supervisor bug, not a condition
        // this method needs to handle gracefully.
        let _ = self.inner.host.set(host);
        let _ = self.inner.config.set(cfg);
        let _ = self.inner.metrics.set(metrics);

        Ok(())
    }

    /// Marks the module running, flips the `waddlebot_up` gauge, and runs
    /// the (currently no-op) bridge seam. Returns promptly. Idempotent — a
    /// second call while already running is a no-op.
    async fn start(&self) -> Result<(), ModuleError> {
        if self.inner.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.metrics().up.set(1.0);
        self.start_bridge().await?;
        self.host().logger().info("waddlebot module started", &[]);
        Ok(())
    }

    /// Stops the (currently no-op) bridge seam and flips `waddlebot_up`
    /// back to 0. Idempotent.
    async fn stop(&self) -> Result<(), ModuleError> {
        if !self.inner.running.swap(false, Ordering::SeqCst) {
            return Ok(());
        }
        self.stop_bridge().await?;
        self.metrics().up.set(0.0);
        self.host().logger().info("waddlebot module stopped", &[]);
        Ok(())
    }

    /// Reports the module's running state plus the active hub URL,
    /// community id, and a cached auth probe outcome (`ok` / `unauthorized`
    /// / `unreachable`) — see [`WaddlebotModule::probe_auth`].
    async fn status(&self) -> Result<Status, ModuleError> {
        let running = self.inner.running.load(Ordering::SeqCst);
        let state = if running {
            ModuleState::Running
        } else {
            ModuleState::Stopped
        };

        let probe = self.probe_auth().await;
        let mut detail = HashMap::new();
        detail.insert("hub".to_string(), self.config().hub.base_url.clone());
        detail.insert("community".to_string(), self.community_id().to_string());
        detail.insert("auth".to_string(), probe.state.as_str().to_string());

        Ok(Status { state, detail })
    }

    /// A cheap, cached liveness/auth probe: [`HealthLevel::Healthy`] when
    /// the hub accepts the current CAT, [`HealthLevel::Degraded`] when it
    /// rejects it or can't be reached at all — never
    /// [`HealthLevel::Unhealthy`], since a module that simply can't reach
    /// its remote hub yet is still itself fully operational. `checked_at`
    /// is always the probe's own timestamp (from [`WaddlebotModule::probe_auth`]),
    /// including on a cache hit — never `SystemTime::now()` at the point of
    /// the `health()` call, which is the bug `penguin-module-squawk`'s own
    /// `health()` doc documents fixing.
    async fn health(&self) -> HealthReport {
        let probe = self.probe_auth().await;
        let (level, message) = match probe.state {
            AuthState::Ok => (HealthLevel::Healthy, "OK".to_string()),
            AuthState::Unauthorized => (
                HealthLevel::Degraded,
                "hub rejected credentials".to_string(),
            ),
            AuthState::Unreachable => (HealthLevel::Degraded, "hub unreachable".to_string()),
        };
        HealthReport {
            level,
            message,
            checked_at: probe.checked_at,
        }
    }

    /// Declares waddlebot's CLI command tree.
    fn commands(&self) -> Vec<CommandSpec> {
        commands::command_tree()
    }

    /// Executes one waddlebot CLI command.
    async fn dispatch(
        &self,
        path: &[String],
        flags: &HashMap<String, String>,
        args: &[String],
    ) -> Result<CommandResult, ModuleError> {
        commands::dispatch(self, path, flags, args).await
    }

    /// Returns [`crate::config::CONFIG_SCHEMA`] for the daemon to validate
    /// `waddlebot.yaml` against before `init` ever sees it.
    fn config_schema(&self) -> Option<Vec<u8>> {
        Some(crate::config::CONFIG_SCHEMA.as_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{FakeHost, MockHub, MockResponse};
    use penguin_sdk::SecretStore;

    fn config_bytes(hub_base_url: &str, community_id: i64) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "hub": {"base_url": hub_base_url},
            "community_id": community_id,
        }))
        .unwrap()
    }

    async fn init_module_against(hub: &MockHub, community_id: i64) -> WaddlebotModule {
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.secrets.set("cat", b"wdl_c_testtoken").await.unwrap();
        host.config = config_bytes(&hub.base_url, community_id);
        let module = WaddlebotModule::new();
        module.init(Arc::new(host)).await.expect("init succeeds");
        module
    }

    #[test]
    fn info_reports_waddlebot_identity_with_no_license_gate() {
        let module = WaddlebotModule::new();
        let info = module.info();
        assert_eq!(info.name, "waddlebot");
        assert_eq!(info.version, "1.0.0");
        assert!(info.license_feature.is_empty());
        assert!(!info.description.is_empty());
    }

    #[test]
    fn factory_builds_a_fresh_uninitialised_module() {
        let module = factory();
        assert_eq!(module.info().name, "waddlebot");
    }

    #[test]
    #[should_panic(expected = "waddlebot module used before init")]
    fn accessors_panic_before_init() {
        let module = WaddlebotModule::new();
        let _ = module.config();
    }

    #[tokio::test]
    async fn init_never_fails_when_the_hub_is_unreachable() {
        let unreachable = MockHub::unreachable_base_url().await;
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = config_bytes(&unreachable, 1);
        let module = WaddlebotModule::new();
        module
            .init(Arc::new(host))
            .await
            .expect("init must succeed even when the hub cannot be reached");
    }

    #[tokio::test]
    async fn init_reads_the_cat_from_secrets_not_config() {
        let hub = MockHub::start().await;
        hub.respond(
            "GET",
            "/communities/my",
            MockResponse::json(200, r#"{"success":true,"communities":[]}"#),
        )
        .await;

        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.secrets.set("cat", b"wdl_c_from_secret").await.unwrap();
        host.config = config_bytes(&hub.base_url, 7);
        let module = WaddlebotModule::new();
        module.init(Arc::new(host)).await.expect("init succeeds");

        module.client().list_my_communities().await.ok();
        let requests = hub.requests().await;
        assert_eq!(
            requests[0].header("authorization"),
            Some("Bearer wdl_c_from_secret")
        );

        hub.stop().await;
    }

    #[tokio::test]
    async fn init_succeeds_with_no_cat_present() {
        let hub = MockHub::start().await;
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = config_bytes(&hub.base_url, 1);
        let module = WaddlebotModule::new();
        module
            .init(Arc::new(host))
            .await
            .expect("init succeeds even with no CAT secret set");
        hub.stop().await;
    }

    #[tokio::test]
    async fn health_maps_a_successful_probe_to_healthy() {
        let hub = MockHub::start().await;
        hub.respond(
            "GET",
            "/communities/my",
            MockResponse::json(200, r#"{"success":true,"communities":[]}"#),
        )
        .await;
        let module = init_module_against(&hub, 1).await;

        let health = module.health().await;
        assert_eq!(health.level, HealthLevel::Healthy);

        hub.stop().await;
    }

    #[tokio::test]
    async fn health_maps_a_401_to_degraded() {
        let hub = MockHub::start().await;
        hub.respond(
            "GET",
            "/communities/my",
            MockResponse::json(401, r#"{"error":"unauthorized"}"#),
        )
        .await;
        let module = init_module_against(&hub, 1).await;

        let health = module.health().await;
        assert_eq!(health.level, HealthLevel::Degraded);
        assert!(health.message.contains("rejected"));

        hub.stop().await;
    }

    #[tokio::test]
    async fn health_maps_an_unreachable_hub_to_degraded() {
        let unreachable = MockHub::unreachable_base_url().await;
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = config_bytes(&unreachable, 1);
        let module = WaddlebotModule::new();
        module.init(Arc::new(host)).await.expect("init succeeds");

        let health = module.health().await;
        assert_eq!(health.level, HealthLevel::Degraded);
        assert!(health.message.contains("unreachable"));
    }

    /// Regression-style: proves the probe is genuinely cached, not just
    /// that two calls happen to agree — only one hub request must have
    /// gone out, and both reports must carry the exact same `checked_at`.
    #[tokio::test]
    async fn health_caches_the_probe_instead_of_hitting_the_hub_every_call() {
        let hub = MockHub::start().await;
        hub.respond(
            "GET",
            "/communities/my",
            MockResponse::json(200, r#"{"success":true,"communities":[]}"#),
        )
        .await;
        let module = init_module_against(&hub, 1).await;

        let first = module.health().await;
        let second = module.health().await;
        assert_eq!(first.checked_at, second.checked_at);
        assert_eq!(hub.request_count("GET", "/communities/my").await, 1);

        hub.stop().await;
    }

    #[tokio::test]
    async fn status_reports_hub_community_and_auth_detail() {
        let hub = MockHub::start().await;
        hub.respond(
            "GET",
            "/communities/my",
            MockResponse::json(200, r#"{"success":true,"communities":[]}"#),
        )
        .await;
        let module = init_module_against(&hub, 42).await;

        let status = module.status().await.unwrap();
        assert_eq!(status.state, ModuleState::Stopped);
        assert_eq!(status.detail.get("hub"), Some(&hub.base_url));
        assert_eq!(status.detail.get("community"), Some(&"42".to_string()));
        assert_eq!(status.detail.get("auth"), Some(&"ok".to_string()));

        hub.stop().await;
    }

    #[tokio::test]
    async fn start_sets_the_up_gauge_and_is_idempotent() {
        let hub = MockHub::start().await;
        let module = init_module_against(&hub, 1).await;

        module.start().await.expect("start succeeds");
        assert_eq!(module.metrics().up.get(), 1.0);
        module.start().await.expect("second start is a no-op");
        assert_eq!(module.metrics().up.get(), 1.0);

        module.stop().await.ok();
        hub.stop().await;
    }

    #[tokio::test]
    async fn stop_without_start_is_idempotent() {
        let hub = MockHub::start().await;
        let module = init_module_against(&hub, 1).await;
        module.stop().await.expect("stop without start is a no-op");
        hub.stop().await;
    }

    #[tokio::test]
    async fn start_with_bridge_disabled_never_binds_anything() {
        let hub = MockHub::start().await;
        let module = init_module_against(&hub, 1).await;

        module
            .start()
            .await
            .expect("start succeeds with the bridge disabled");
        assert!(
            module
                .inner
                .bridge
                .lock()
                .expect("bridge mutex poisoned")
                .is_none(),
            "bridge.enabled defaults to false — nothing should be bound"
        );

        module.stop().await.ok();
        hub.stop().await;
    }

    #[tokio::test]
    async fn start_with_bridge_enabled_but_no_addresses_configured_binds_nothing_and_still_succeeds()
     {
        let hub = MockHub::start().await;
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = serde_json::to_vec(&serde_json::json!({
            "hub": {"base_url": hub.base_url},
            "community_id": 1,
            "bridge": {"enabled": true},
        }))
        .unwrap();
        let module = WaddlebotModule::new();
        module.init(Arc::new(host)).await.expect("init succeeds");

        module
            .start()
            .await
            .expect("bridge.enabled with empty addresses is a degenerate, not a failing, case");

        module.stop().await.ok();
        hub.stop().await;
    }

    /// The full lifecycle end to end, through `Module::start`/`stop` rather
    /// than calling `bridge::start` directly (already covered in
    /// `crate::bridge`'s own tests): both transports come up, and
    /// `Module::stop` releases both.
    #[tokio::test]
    async fn start_with_bridge_enabled_binds_both_transports_and_stop_releases_both() {
        let hub = MockHub::start().await;
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("bridge.sock");
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = serde_json::to_vec(&serde_json::json!({
            "hub": {"base_url": hub.base_url},
            "community_id": 1,
            "bridge": {
                "enabled": true,
                "listen_tcp": "127.0.0.1:0",
                "listen_unix": socket_path.to_string_lossy(),
            },
        }))
        .unwrap();
        let module = WaddlebotModule::new();
        module.init(Arc::new(host)).await.expect("init succeeds");

        module.start().await.expect("start binds both transports");
        let tcp_addr = {
            let bridge = module.inner.bridge.lock().expect("bridge mutex poisoned");
            bridge
                .as_ref()
                .expect("bridge handle stored")
                .tcp_local_addr()
                .expect("tcp bound")
        };
        assert!(tokio::net::TcpStream::connect(tcp_addr).await.is_ok());
        assert!(socket_path.exists());

        module.stop().await.expect("stop tears the bridge down");
        assert!(tokio::net::TcpStream::connect(tcp_addr).await.is_err());
        assert!(
            module
                .inner
                .bridge
                .lock()
                .expect("bridge mutex poisoned")
                .is_none()
        );

        hub.stop().await;
    }

    #[tokio::test]
    async fn start_fails_when_the_bridge_tcp_address_is_non_loopback() {
        let hub = MockHub::start().await;
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = serde_json::to_vec(&serde_json::json!({
            "hub": {"base_url": hub.base_url},
            "community_id": 1,
            "bridge": {"enabled": true, "listen_tcp": "0.0.0.0:0"},
        }))
        .unwrap();
        let module = WaddlebotModule::new();
        module.init(Arc::new(host)).await.expect("init succeeds");

        let Err(err) = module.start().await else {
            panic!("a non-loopback bridge address must fail start");
        };
        assert!(err.to_string().contains("loopback"));

        hub.stop().await;
    }

    #[tokio::test]
    async fn set_community_rebuilds_the_client_and_status_reflects_it() {
        let hub = MockHub::start().await;
        hub.respond(
            "GET",
            "/communities/my",
            MockResponse::json(200, r#"{"success":true,"communities":[]}"#),
        )
        .await;
        let module = init_module_against(&hub, 1).await;
        assert_eq!(module.community_id(), 1);

        module.set_community(99).expect("switch succeeds");
        assert_eq!(module.community_id(), 99);

        let status = module.status().await.unwrap();
        assert_eq!(status.detail.get("community"), Some(&"99".to_string()));

        hub.stop().await;
    }

    #[tokio::test]
    async fn dispatch_no_command_is_a_nonzero_exit() {
        let hub = MockHub::start().await;
        let module = init_module_against(&hub, 1).await;
        let result = module.dispatch(&[], &HashMap::new(), &[]).await.unwrap();
        assert_ne!(result.exit_code, 0);
        hub.stop().await;
    }

    #[tokio::test]
    async fn dispatch_unknown_command_is_a_nonzero_exit() {
        let hub = MockHub::start().await;
        let module = init_module_against(&hub, 1).await;
        let result = module
            .dispatch(&["bogus".to_string()], &HashMap::new(), &[])
            .await
            .unwrap();
        assert_ne!(result.exit_code, 0);
        hub.stop().await;
    }

    #[test]
    fn config_schema_is_present_and_valid_json() {
        let module = WaddlebotModule::new();
        let schema = module.config_schema().expect("schema present");
        let _: serde_json::Value = serde_json::from_slice(&schema).expect("valid JSON");
    }

    #[tokio::test]
    async fn default_impl_builds_a_fresh_module() {
        let module = WaddlebotModule::default();
        assert_eq!(module.info().name, "waddlebot");
    }
}
