//! The SkausWatch `penguin_sdk::Module` implementation: lifecycle glue,
//! agent enrollment, and the background heartbeat/report loop.
//!
//! Mirrors `penguin-module-tobogganing::module`'s pattern: an `Arc<Inner>`
//! shared into a background task via `Clone`, a `CancellationToken`
//! recreated fresh on every `start()` (so a `start` after a `stop` is never
//! driven by an already-cancelled token — see that crate's `start` doc for
//! the Go bug this avoids), and a cached `last_health` report so `health()`
//! never reads a silent "healthy by default" value before any real check has
//! run.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use penguin_sdk::{
    CommandResult, CommandSpec, HealthLevel, HealthReport, HostServices, Module, ModuleError,
    ModuleInfo, ModuleState, SecretError, Status,
};
use skauswatch_client::{
    AgentIdentity, ClientConfig, EndpointEvent, HeartbeatBody, SkausWatchClient,
};

use crate::config::ModuleConfig;
use crate::metrics::SkausWatchMetrics;

/// Secret-store key the enrolled [`AgentIdentity`] is persisted under, once
/// obtained — read back on every subsequent run so an agent registers with
/// the Manager at most once.
const AGENT_IDENTITY_SECRET_KEY: &str = "agent_identity";

/// Health degrades once the last successful heartbeat is older than this
/// multiple of the configured heartbeat interval — mirrors tobogganing's
/// `HANDSHAKE_DEGRADED_AGE` pattern (a fixed multiple of the check cadence,
/// not a fixed wall-clock duration, so a slower-configured interval doesn't
/// falsely read as degraded).
const DEGRADED_MULTIPLIER: u32 = 2;

/// The module's real state, held behind an `Arc` so [`SkausWatchModule::start`]
/// can clone a handle into its spawned background task.
struct Inner {
    host: OnceLock<Arc<dyn HostServices>>,
    client: OnceLock<Arc<SkausWatchClient>>,
    metrics: OnceLock<SkausWatchMetrics>,
    running: AtomicBool,
    /// Recreated fresh on every `start()` — see `start()`'s doc.
    cancel: StdMutex<Option<CancellationToken>>,
    heartbeat_interval_ms: AtomicU64,
    /// Cached once enrollment succeeds (either loaded from the secret store
    /// or freshly registered) — see `ensure_identity`. Read every tick so a
    /// steady-state loop never re-hits the secret store once warm.
    identity: StdMutex<Option<AgentIdentity>>,
    /// Set on every heartbeat the Manager acknowledges — the age of this is
    /// what `update_health_probe` grades against `DEGRADED_MULTIPLIER`.
    last_heartbeat_ok: StdMutex<Option<SystemTime>>,
    /// `None` until the first probe runs — see [`SkausWatchModule::health`]'s doc.
    last_health: StdMutex<Option<HealthReport>>,
    /// Events queued between ticks, drained and reported on the next tick —
    /// see `run_heartbeat_tick`. Nothing in production code populates this
    /// yet (Task 6's CLI/command surface is still a stub); test-only
    /// `queue_event_for_test` below exercises the drain path until then.
    pending_events: StdMutex<Vec<EndpointEvent>>,
    /// Test-only liveness counter — see the `#[cfg(test)]` accessors below.
    heartbeat_ticks: AtomicU64,
}

impl Inner {
    const UNINITIALISED: &'static str = "skauswatch module used before init";

    fn new() -> Inner {
        Inner {
            host: OnceLock::new(),
            client: OnceLock::new(),
            metrics: OnceLock::new(),
            running: AtomicBool::new(false),
            cancel: StdMutex::new(None),
            heartbeat_interval_ms: AtomicU64::new(
                ModuleConfig::default().heartbeat_interval * 1000,
            ),
            identity: StdMutex::new(None),
            last_heartbeat_ok: StdMutex::new(None),
            last_health: StdMutex::new(None),
            pending_events: StdMutex::new(Vec::new()),
            heartbeat_ticks: AtomicU64::new(0),
        }
    }
}

/// SkausWatch: a monitoring and alerting endpoint client.
///
/// A cheap `Clone` (an `Arc` around its real state): [`Module::start`]
/// clones `self` into the background task it spawns, since that task must
/// be `'static` and a bare `&self` cannot outlive the call.
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

    pub(crate) fn client(&self) -> &Arc<SkausWatchClient> {
        self.inner.client.get().expect(Inner::UNINITIALISED)
    }

    pub(crate) fn metrics(&self) -> &SkausWatchMetrics {
        self.inner.metrics.get().expect(Inner::UNINITIALISED)
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

    /// Resolves config (defaults, then the host's raw YAML), builds the
    /// [`SkausWatchClient`], and registers all four metrics. Never begins
    /// background work or touches the network — that is [`Module::start`]'s
    /// job (specifically the loop it spawns; enrollment happens on that
    /// loop's first tick, not here — see `start`'s doc).
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
        if cfg.enrollment_token.is_empty() {
            return Err(ModuleError::new("enrollment_token is required"));
        }

        logger.info(
            "skauswatch config loaded",
            &[
                ("base_url", cfg.base_url.as_str()),
                ("heartbeat_interval", &cfg.heartbeat_interval.to_string()),
            ],
        );

        let interval = Duration::from_secs(cfg.heartbeat_interval.max(1));
        self.inner
            .heartbeat_interval_ms
            .store(interval.as_millis() as u64, Ordering::SeqCst);

        let client_cfg = ClientConfig::new(cfg.base_url.clone(), cfg.enrollment_token.clone());
        let client = Arc::new(SkausWatchClient::new(client_cfg).map_err(|err| {
            ModuleError::new(format!("failed to build skauswatch client: {err}"))
        })?);

        let metrics = SkausWatchMetrics::register(host.metrics().as_ref())
            .map_err(|err| ModuleError::new(format!("register metrics: {err}")))?;

        logger.info("skauswatch module initialized", &[]);

        // `OnceLock::set` returning `Err` would mean `init` ran twice —
        // impossible per the `Module::init` contract, so a violation here is
        // a supervisor bug, not a condition this method needs to handle.
        let _ = self.inner.host.set(host);
        let _ = self.inner.client.set(client);
        let _ = self.inner.metrics.set(metrics);

        Ok(())
    }

    /// Starts the background heartbeat/report loop and returns promptly.
    ///
    /// **Enrollment happens inside the loop, not here.** The loop's first
    /// tick is responsible for ensuring an [`AgentIdentity`] exists (loading
    /// one already persisted, or registering a fresh one — see
    /// `ensure_identity`), retrying on the next tick if that fails. This
    /// means `start()` never makes a network call and never blocks on an
    /// unreachable Manager — matching tobogganing's `start()`, whose own doc
    /// explains why a blocking `start` would wedge the whole daemon (the
    /// supervisor holds its lock across the call).
    ///
    /// The `CancellationToken` is recreated fresh on every call (not reused
    /// from a prior `start`), so a `start` following a `stop` is never
    /// driven by an already-cancelled token — see tobogganing's `start` doc
    /// for the Go bug (a permanently-closed `stopCh`) this pattern avoids.
    async fn start(&self) -> Result<(), ModuleError> {
        if self.inner.running.swap(true, Ordering::SeqCst) {
            return Err(ModuleError::new("module already running"));
        }

        self.host().logger().info("starting skauswatch module", &[]);

        let cancel = CancellationToken::new();
        *self.inner.cancel.lock().unwrap() = Some(cancel.clone());

        // A synchronous, purely-local probe so `health()` never reads the
        // "no health probe has run yet" fallback's silent alternative
        // (`HealthReport::default()` is `Healthy` — see its doc) before any
        // real check has run. Cheap: reads only cached local state, no
        // network I/O, so it's always safe to run inline here.
        update_health_probe(self);

        let loop_module = self.clone();
        let loop_cancel = cancel.clone();
        tokio::spawn(async move { heartbeat_loop(loop_module, loop_cancel).await });

        Ok(())
    }

    /// Cancels the background loop and marks the module stopped. Idempotent:
    /// a second call is a no-op `Ok(())`, matching tobogganing's `stop`.
    async fn stop(&self) -> Result<(), ModuleError> {
        if !self.inner.running.swap(false, Ordering::SeqCst) {
            return Ok(());
        }

        self.host().logger().info("stopping skauswatch module", &[]);

        if let Some(cancel) = self.inner.cancel.lock().unwrap().take() {
            cancel.cancel();
        }

        Ok(())
    }

    /// Reports the module's running state plus enrollment detail.
    async fn status(&self) -> Result<Status, ModuleError> {
        let state = if self.is_running() {
            ModuleState::Running
        } else {
            ModuleState::Disabled
        };

        let identity = self.inner.identity.lock().unwrap().clone();
        let mut detail = HashMap::new();
        detail.insert("enrolled".to_string(), identity.is_some().to_string());
        if let Some(identity) = identity {
            detail.insert("agent_id".to_string(), identity.agent_id);
        }

        Ok(Status { state, detail })
    }

    /// The last value [`update_health_probe`] computed, or — matching
    /// tobogganing's `health()` fix — an explicit "not yet probed"
    /// [`HealthLevel::Unhealthy`] report if no probe has run at all, rather
    /// than relying on [`HealthReport::default`]'s `Healthy` value. `start()`
    /// always runs one synchronous probe before returning, so the fallback
    /// here is only ever observable before the very first `start()`. Never
    /// errors — this method has no `Result` to fail with.
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

    /// No commands yet (scaffold — filled in Task 6).
    fn commands(&self) -> Vec<CommandSpec> {
        vec![]
    }

    /// Dispatch returns "unknown command" (scaffold — filled in Task 6).
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

/// Ensures a persisted [`AgentIdentity`] exists: returns the cached one if
/// this process has already resolved it this run, else tries the host's
/// secret store, else registers a fresh one against the Manager and
/// persists it. Called from every heartbeat tick (never from `start()`
/// itself — see that method's doc), so a Manager that's unreachable at
/// startup is retried automatically without ever blocking `start()`.
async fn ensure_identity(module: &SkausWatchModule) -> Result<AgentIdentity, ModuleError> {
    if let Some(identity) = module.inner.identity.lock().unwrap().clone() {
        return Ok(identity);
    }

    match module.host().secrets().get(AGENT_IDENTITY_SECRET_KEY).await {
        Ok(bytes) => {
            let identity: AgentIdentity = serde_json::from_slice(&bytes).map_err(|err| {
                ModuleError::new(format!("failed to parse stored agent identity: {err}"))
            })?;
            cache_identity(module, identity.clone());
            return Ok(identity);
        }
        Err(SecretError::NotFound) => {}
        Err(err) => {
            return Err(ModuleError::new(format!(
                "failed to read stored agent identity: {err}"
            )));
        }
    }

    let identity = module
        .client()
        .register()
        .await
        .map_err(|err| ModuleError::new(format!("registration failed: {err}")))?;

    let payload = serde_json::to_vec(&identity)
        .map_err(|err| ModuleError::new(format!("failed to serialize agent identity: {err}")))?;
    module
        .host()
        .secrets()
        .set(AGENT_IDENTITY_SECRET_KEY, &payload)
        .await
        .map_err(|err| ModuleError::new(format!("failed to persist agent identity: {err}")))?;

    module.host().logger().info(
        "agent enrolled",
        &[("agent_id", identity.agent_id.as_str())],
    );
    cache_identity(module, identity.clone());

    Ok(identity)
}

fn cache_identity(module: &SkausWatchModule, identity: AgentIdentity) {
    *module.inner.identity.lock().unwrap() = Some(identity);
    module.metrics().enrolled.set(1.0);
}

/// One heartbeat/report cycle: ensures enrollment, sends a heartbeat, and
/// drains+reports any queued events. Every fallible step logs, bumps
/// `errors_total`, and returns — this function never panics and never
/// aborts the loop on a transient error, matching the brief's requirement
/// that a failed Manager call is retried on the next tick, not fatal.
async fn run_heartbeat_tick(module: &SkausWatchModule) {
    let identity = match ensure_identity(module).await {
        Ok(identity) => identity,
        Err(err) => {
            module.host().logger().error(
                "failed to obtain agent identity",
                &[("error", &err.to_string())],
            );
            module.metrics().errors_total.inc();
            update_health_probe(module);
            return;
        }
    };

    let body = HeartbeatBody {
        healthy: true,
        module_version: env!("CARGO_PKG_VERSION").to_string(),
    };

    match module.client().heartbeat(&identity, &body).await {
        Ok(()) => {
            *module.inner.last_heartbeat_ok.lock().unwrap() = Some(SystemTime::now());
            module.metrics().heartbeats_total.inc();
        }
        Err(err) => {
            module
                .host()
                .logger()
                .warn("heartbeat failed", &[("error", &err.to_string())]);
            module.metrics().errors_total.inc();
        }
    }

    report_pending_events(module, &identity).await;
    update_health_probe(module);
}

/// Drains [`Inner::pending_events`] and reports the batch via
/// [`SkausWatchClient::report_events`]. On failure the drained events are
/// pushed back onto the front of the queue (ahead of anything queued while
/// the report was in flight) so a transient failure never silently drops an
/// observed event — it's retried on the next tick instead.
async fn report_pending_events(module: &SkausWatchModule, identity: &AgentIdentity) {
    let events = {
        let mut pending = module.inner.pending_events.lock().unwrap();
        std::mem::take(&mut *pending)
    };
    if events.is_empty() {
        return;
    }

    let event_count = events.len();
    if let Err(err) = module.client().report_events(identity, &events).await {
        module
            .host()
            .logger()
            .warn("event report failed", &[("error", &err.to_string())]);
        module.metrics().errors_total.inc();

        let mut pending = module.inner.pending_events.lock().unwrap();
        let mut requeued = events;
        requeued.append(&mut pending);
        *pending = requeued;
        return;
    }

    module
        .metrics()
        .events_reported_total
        .inc_by(event_count as f64);
}

/// The `start()`-spawned background task: ticks every configured heartbeat
/// interval (default from `ModuleConfig`, overridable for tests via
/// `set_heartbeat_interval_for_test`) until `cancel` fires.
async fn heartbeat_loop(module: SkausWatchModule, cancel: CancellationToken) {
    let interval = Duration::from_millis(module.inner.heartbeat_interval_ms.load(Ordering::SeqCst));
    let mut ticker = tokio::time::interval(interval);
    // Tokio's first tick fires immediately; discarding it here means the
    // loop's real cadence starts counting from `start()`, matching
    // tobogganing's identical `ticker.tick().await` discard.
    ticker.tick().await;

    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            _ = ticker.tick() => {
                module.inner.heartbeat_ticks.fetch_add(1, Ordering::SeqCst);
                run_heartbeat_tick(&module).await;
            }
        }
    }
}

/// Grades health from local cached state only (no network I/O — safe to
/// call synchronously from `start()` as well as from every loop tick):
/// unenrolled is `Unhealthy`, enrolled-but-never-heartbeat is `Unhealthy`,
/// a heartbeat older than `DEGRADED_MULTIPLIER` intervals is `Degraded`,
/// otherwise `Healthy`.
fn update_health_probe(module: &SkausWatchModule) {
    let enrolled = module.inner.identity.lock().unwrap().is_some();
    if !enrolled {
        set_health(module, HealthLevel::Unhealthy, "agent not yet enrolled");
        return;
    }

    let last = *module.inner.last_heartbeat_ok.lock().unwrap();
    let Some(last) = last else {
        set_health(
            module,
            HealthLevel::Unhealthy,
            "no successful heartbeat yet",
        );
        return;
    };

    let interval = Duration::from_millis(module.inner.heartbeat_interval_ms.load(Ordering::SeqCst));
    let degraded_age = interval.saturating_mul(DEGRADED_MULTIPLIER);
    let age = SystemTime::now()
        .duration_since(last)
        .unwrap_or(Duration::ZERO);

    if age > degraded_age {
        set_health(
            module,
            HealthLevel::Degraded,
            format!("last heartbeat was {}s ago", age.as_secs()),
        );
        return;
    }

    set_health(module, HealthLevel::Healthy, "heartbeat is current");
}

fn set_health(module: &SkausWatchModule, level: HealthLevel, message: impl Into<String>) {
    let mut guard = module.inner.last_health.lock().unwrap();
    *guard = Some(HealthReport {
        level,
        message: message.into(),
        checked_at: SystemTime::now(),
    });
}

/// Test-only hooks: overridable heartbeat interval (production defaults are
/// far too slow for a test to wait on) and a liveness counter proving the
/// background loop is genuinely still ticking.
#[cfg(test)]
impl SkausWatchModule {
    pub(crate) fn set_heartbeat_interval_for_test(&self, interval: Duration) {
        self.inner
            .heartbeat_interval_ms
            .store(interval.as_millis() as u64, Ordering::SeqCst);
    }

    pub(crate) fn heartbeat_tick_count(&self) -> u64 {
        self.inner.heartbeat_ticks.load(Ordering::SeqCst)
    }

    /// Queues `event` for the next heartbeat tick's `report_events` call —
    /// stands in for the real production entry point Task 6's command/event
    /// surface will add once it exists.
    pub(crate) fn queue_event_for_test(&self, event: EndpointEvent) {
        self.inner.pending_events.lock().unwrap().push(event);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use penguin_sdk::SecretStore;

    use crate::testutil::{self, FakeHost, MockManager, MockResponse};

    use super::*;

    fn valid_config_bytes(base_url: &str, enrollment_token: &str, interval_secs: u64) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "base_url": base_url,
            "enrollment_token": enrollment_token,
            "heartbeat_interval": interval_secs,
        }))
        .unwrap()
    }

    async fn init_module() -> SkausWatchModule {
        let host = testutil::fake_host("http://127.0.0.1:1", "enroll-tok", 60);
        let module = SkausWatchModule::new();
        module.init(host).await.expect("init succeeds");
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

    async fn seed_register_route(manager: &MockManager, agent_id: &str, api_key: &str) {
        manager
            .respond(
                "POST",
                "/api/v1/endpoint/register",
                MockResponse::json(
                    200,
                    format!(r#"{{"agent_id":"{agent_id}","api_key":"{api_key}"}}"#),
                ),
            )
            .await;
    }

    async fn seed_heartbeat_route(manager: &MockManager) {
        manager
            .respond(
                "POST",
                "/api/v1/endpoint/heartbeat",
                MockResponse::empty(200),
            )
            .await;
    }

    async fn seed_events_route(manager: &MockManager) {
        manager
            .respond("POST", "/api/v1/endpoint/events", MockResponse::empty(200))
            .await;
    }

    /// Builds and initializes a module wired to `manager_url` with a short,
    /// test-overridden heartbeat interval.
    async fn init_module_with_manager(manager_url: &str) -> SkausWatchModule {
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = valid_config_bytes(manager_url, "enroll-tok", 60);
        let module = SkausWatchModule::new();
        module.init(Arc::new(host)).await.expect("init succeeds");
        module.set_heartbeat_interval_for_test(Duration::from_millis(15));
        module
    }

    #[tokio::test]
    async fn info_reports_skauswatch_identity_with_no_license_gate() {
        let module = SkausWatchModule::new();
        let info = module.info();
        assert_eq!(info.name, "skauswatch");
        assert!(info.license_feature.is_empty());
        assert!(!info.description.is_empty());
    }

    #[test]
    #[should_panic(expected = "skauswatch module used before init")]
    fn accessors_panic_before_init() {
        let module = SkausWatchModule::new();
        let _ = module.client();
    }

    #[tokio::test]
    async fn init_requires_base_url() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = serde_json::to_vec(&serde_json::json!({"enrollment_token": "tok"})).unwrap();
        let module = SkausWatchModule::new();
        let err = module.init(Arc::new(host)).await.unwrap_err();
        assert!(err.to_string().contains("base_url"));
    }

    #[tokio::test]
    async fn init_requires_enrollment_token() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = serde_json::to_vec(&serde_json::json!({"base_url": "http://x"})).unwrap();
        let module = SkausWatchModule::new();
        let err = module.init(Arc::new(host)).await.unwrap_err();
        assert!(err.to_string().contains("enrollment_token"));
    }

    #[tokio::test]
    async fn init_surfaces_a_config_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = b"{not valid yaml or json".to_vec();
        let module = SkausWatchModule::new();
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
        host.config = valid_config_bytes("http://127.0.0.1:1", "tok", 60);
        let host: Arc<dyn HostServices> = Arc::new(host);

        let first = SkausWatchModule::new();
        first.init(host.clone()).await.expect("first init succeeds");

        let second = SkausWatchModule::new();
        let err = second.init(host).await.unwrap_err();
        assert!(err.to_string().contains("register metrics"));
    }

    /// TDD RED for Task 5: proves the lifecycle is clean (start returns
    /// promptly with no live endpoint required — enrollment happens inside
    /// the loop, not on the `start()` path) and `stop()` is idempotent.
    #[tokio::test(start_paused = true)]
    async fn start_then_stop_is_clean_and_idempotent() {
        let module = SkausWatchModule::new();
        let host = testutil::fake_host("http://127.0.0.1:1", "enroll-tok", 30);
        module.init(host).await.expect("init");
        module.start().await.expect("start returns promptly");
        assert!(module.is_running());
        module.stop().await.expect("stop");
        module.stop().await.expect("stop is idempotent");
        assert!(!module.is_running());
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

    /// Regression, mirroring tobogganing: Go's `stopCh` pattern created a
    /// stop signal once and closed it once, so a `start` after a `stop`
    /// spawned a loop that immediately saw an already-cancelled signal and
    /// exited on its first `select` — silently dead background work. This
    /// proves the loop is genuinely alive (still ticking) after a second
    /// `start()`, not just that `start()` itself returns without error.
    #[tokio::test]
    async fn start_stop_start_leaves_the_heartbeat_loop_alive() {
        let manager = MockManager::start().await;
        seed_register_route(&manager, "agent-1", "key-1").await;
        seed_heartbeat_route(&manager).await;

        let module = init_module_with_manager(&manager.base_url).await;

        module.start().await.unwrap();
        wait_for(|| module.heartbeat_tick_count() > 0).await;
        module.stop().await.ok();

        module.start().await.unwrap();
        let before = module.heartbeat_tick_count();
        wait_for(|| module.heartbeat_tick_count() > before).await;

        module.stop().await.ok();
        manager.stop().await;
    }

    #[tokio::test]
    async fn status_reports_not_enrolled_before_any_tick() {
        let module = init_module().await;
        let status = module.status().await.unwrap();
        assert_eq!(
            status.detail.get("enrolled").map(String::as_str),
            Some("false")
        );
    }

    /// End-to-end: the loop registers (no persisted identity yet), persists
    /// the resulting `AgentIdentity` to the host's secret store, and sends a
    /// heartbeat using it — proving `ensure_identity`'s register-then-persist
    /// path and `run_heartbeat_tick`'s heartbeat call both work against a
    /// real (mocked) Manager.
    #[tokio::test]
    async fn heartbeat_loop_registers_persists_and_heartbeats() {
        let manager = MockManager::start().await;
        seed_register_route(&manager, "agent-42", "secret-key").await;
        seed_heartbeat_route(&manager).await;

        let module = init_module_with_manager(&manager.base_url).await;
        module.start().await.expect("start succeeds");

        wait_for(|| module.metrics().heartbeats_total.get() >= 1.0).await;

        assert_eq!(
            manager
                .request_count("POST", "/api/v1/endpoint/register")
                .await,
            1
        );
        assert_eq!(module.metrics().enrolled.get(), 1.0);

        let status = module.status().await.unwrap();
        assert_eq!(
            status.detail.get("agent_id").map(String::as_str),
            Some("agent-42")
        );

        let stored = module
            .host()
            .secrets()
            .get(AGENT_IDENTITY_SECRET_KEY)
            .await
            .expect("identity persisted");
        let identity: AgentIdentity = serde_json::from_slice(&stored).unwrap();
        assert_eq!(identity.agent_id, "agent-42");
        assert_eq!(identity.api_key, "secret-key");

        let health = module.health().await;
        assert_eq!(health.level, HealthLevel::Healthy);

        module.stop().await.ok();
        manager.stop().await;
    }

    /// A previously-persisted identity is reused without re-registering —
    /// exercises `ensure_identity`'s secret-store-hit path directly, not
    /// just the register-then-persist path the other loop tests cover.
    #[tokio::test]
    async fn heartbeat_loop_reuses_a_persisted_identity_without_re_registering() {
        let manager = MockManager::start().await;
        seed_heartbeat_route(&manager).await;

        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = valid_config_bytes(&manager.base_url, "enroll-tok", 60);
        let identity_json =
            serde_json::to_vec(&serde_json::json!({"agent_id": "existing-agent", "api_key": "k"}))
                .unwrap();
        host.secrets
            .set(AGENT_IDENTITY_SECRET_KEY, &identity_json)
            .await
            .unwrap();

        let module = SkausWatchModule::new();
        module.init(Arc::new(host)).await.expect("init succeeds");
        module.set_heartbeat_interval_for_test(Duration::from_millis(15));
        module.start().await.expect("start succeeds");

        wait_for(|| module.metrics().heartbeats_total.get() >= 1.0).await;

        assert_eq!(
            manager
                .request_count("POST", "/api/v1/endpoint/register")
                .await,
            0
        );

        module.stop().await.ok();
        manager.stop().await;
    }

    /// A failed registration is retried on the next tick rather than
    /// wedging the loop — proves the "never panic, never exit the loop"
    /// requirement for the enrollment path specifically.
    #[tokio::test]
    async fn heartbeat_loop_retries_registration_after_a_failure() {
        let manager = MockManager::start().await;
        manager
            .respond(
                "POST",
                "/api/v1/endpoint/register",
                MockResponse::json(500, "{}"),
            )
            .await;
        manager
            .respond(
                "POST",
                "/api/v1/endpoint/register",
                MockResponse::json(200, r#"{"agent_id":"agent-7","api_key":"k7"}"#),
            )
            .await;
        seed_heartbeat_route(&manager).await;

        let module = init_module_with_manager(&manager.base_url).await;
        module.start().await.expect("start succeeds");

        wait_for(|| module.metrics().errors_total.get() >= 1.0).await;
        wait_for(|| module.metrics().enrolled.get() >= 1.0).await;
        wait_for(|| module.metrics().heartbeats_total.get() >= 1.0).await;

        module.stop().await.ok();
        manager.stop().await;
    }

    /// Events queued between ticks are drained and reported on the next
    /// tick via `report_events`.
    #[tokio::test]
    async fn heartbeat_loop_drains_queued_events_via_report_events() {
        let manager = MockManager::start().await;
        seed_register_route(&manager, "agent-9", "key-9").await;
        seed_heartbeat_route(&manager).await;
        seed_events_route(&manager).await;

        let module = init_module_with_manager(&manager.base_url).await;
        module.queue_event_for_test(EndpointEvent {
            kind: "module_fault".to_string(),
            severity: "warning".to_string(),
            detail: serde_json::json!({"module": "squawk"}),
            ts_unix: 1_700_000_000,
        });

        module.start().await.expect("start succeeds");

        wait_for(|| module.metrics().events_reported_total.get() >= 1.0).await;
        assert_eq!(
            manager
                .request_count("POST", "/api/v1/endpoint/events")
                .await,
            1
        );

        module.stop().await.ok();
        manager.stop().await;
    }

    /// A failed event report re-queues the batch instead of dropping it —
    /// proves events survive a transient Manager failure.
    #[tokio::test]
    async fn heartbeat_loop_requeues_events_after_a_failed_report() {
        let manager = MockManager::start().await;
        seed_register_route(&manager, "agent-11", "key-11").await;
        seed_heartbeat_route(&manager).await;
        manager
            .respond(
                "POST",
                "/api/v1/endpoint/events",
                MockResponse::json(500, "{}"),
            )
            .await;
        manager
            .respond("POST", "/api/v1/endpoint/events", MockResponse::empty(200))
            .await;

        let module = init_module_with_manager(&manager.base_url).await;
        module.queue_event_for_test(EndpointEvent {
            kind: "module_fault".to_string(),
            severity: "critical".to_string(),
            detail: serde_json::Value::Null,
            ts_unix: 1_700_000_001,
        });

        module.start().await.expect("start succeeds");

        wait_for(|| module.metrics().events_reported_total.get() >= 1.0).await;
        assert!(
            manager
                .request_count("POST", "/api/v1/endpoint/events")
                .await
                >= 2,
            "expected a failed attempt plus a successful retry"
        );

        module.stop().await.ok();
        manager.stop().await;
    }

    #[tokio::test]
    async fn health_before_any_probe_is_never_healthy() {
        let module = init_module().await;
        let health = module.health().await;
        assert_ne!(health.level, HealthLevel::Healthy);
    }

    #[tokio::test]
    async fn start_makes_an_unenrolled_health_report_available_immediately() {
        let module = init_module().await;
        module.start().await.expect("start succeeds");
        let health = module.health().await;
        assert_eq!(health.level, HealthLevel::Unhealthy);
        assert!(health.message.contains("not yet enrolled"));
        module.stop().await.ok();
    }

    /// Drives `update_health_probe` directly (not through the loop) so
    /// degraded-age grading is deterministic rather than racing a ticker.
    #[tokio::test]
    async fn health_probe_reports_degraded_once_the_last_heartbeat_goes_stale() {
        let module = init_module().await;
        module.set_heartbeat_interval_for_test(Duration::from_millis(10));
        *module.inner.identity.lock().unwrap() = Some(AgentIdentity {
            agent_id: "a".to_string(),
            api_key: "k".to_string(),
        });
        let stale = SystemTime::now() - Duration::from_secs(1);
        *module.inner.last_heartbeat_ok.lock().unwrap() = Some(stale);

        update_health_probe(&module);

        let health = module.health().await;
        assert_eq!(health.level, HealthLevel::Degraded);
        assert!(health.message.contains("ago"));
    }

    #[tokio::test]
    async fn health_probe_reports_healthy_with_a_fresh_heartbeat() {
        let module = init_module().await;
        module.set_heartbeat_interval_for_test(Duration::from_secs(60));
        *module.inner.identity.lock().unwrap() = Some(AgentIdentity {
            agent_id: "a".to_string(),
            api_key: "k".to_string(),
        });
        *module.inner.last_heartbeat_ok.lock().unwrap() = Some(SystemTime::now());

        update_health_probe(&module);

        let health = module.health().await;
        assert_eq!(health.level, HealthLevel::Healthy);
    }

    #[tokio::test]
    async fn config_schema_is_present_and_valid_json() {
        let module = SkausWatchModule::new();
        let schema = module.config_schema().expect("schema present");
        let _: serde_json::Value = serde_json::from_slice(&schema).expect("valid JSON");
    }

    #[tokio::test]
    async fn default_impl_builds_a_fresh_unrunning_module() {
        let module = SkausWatchModule::default();
        assert!(!module.is_running());
    }

    #[tokio::test]
    async fn dispatch_unknown_command_is_an_error() {
        let module = init_module().await;
        let err = module
            .dispatch(&[], &HashMap::new(), &[])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown command"));
    }
}
