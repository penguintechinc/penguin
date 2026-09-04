//! The SkausWatch `penguin_sdk::Module` implementation: lifecycle glue and
//! the background check-in/heartbeat/report loop.
//!
//! Mirrors `penguin-module-tobogganing::module`'s pattern: an `Arc<Inner>`
//! shared into a background task via `Clone`, a `CancellationToken`
//! recreated fresh on every `start()` (so a `start` after a `stop` is never
//! driven by an already-cancelled token — see that crate's `start` doc for
//! the Go bug this avoids), and a cached `last_health` report so `health()`
//! never reads a silent "healthy by default" value before any real check has
//! run.
//!
//! # Identity is provisioned, not obtained
//!
//! Unlike a prior version of this module, the agent's `agent_id` (config)
//! and `api_key` (secret store) are both provisioned out-of-band before
//! `init` ever runs — the real Manager's `register()` handler is a
//! check-in/upsert against a known agent, not an identity-issuing call (see
//! `skauswatch_client::ClientConfig`'s doc). There is nothing to "acquire"
//! and nothing server-issued to persist.

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
use skauswatch_client::{ClientConfig, EndpointEvent, SkausWatchClient};

use crate::config::ModuleConfig;
use crate::metrics::SkausWatchMetrics;

/// Secret-store key the provisioned `api_key` credential is read from in
/// `init` — never written by this module; the Manager never issues one over
/// the wire (see [`SkausWatchModule::init`]'s doc).
pub(crate) const API_KEY_SECRET_KEY: &str = "api_key";

/// Health degrades once the last successful heartbeat is older than this
/// multiple of the configured heartbeat interval — mirrors tobogganing's
/// `HANDSHAKE_DEGRADED_AGE` pattern (a fixed multiple of the check cadence,
/// not a fixed wall-clock duration, so a slower-configured interval doesn't
/// falsely read as degraded).
const DEGRADED_MULTIPLIER: u32 = 2;

/// The module's real state, held behind an `Arc` so [`SkausWatchModule::start`]
/// can clone a handle into its spawned background task.
pub(crate) struct Inner {
    host: OnceLock<Arc<dyn HostServices>>,
    client: OnceLock<Arc<SkausWatchClient>>,
    metrics: OnceLock<SkausWatchMetrics>,
    /// This agent's provisioned identity, set once in `init` — read on
    /// every `status()` call. A `OnceLock` (not a `Mutex`) because, unlike
    /// the old server-issued identity, this never changes for the life of
    /// the process.
    agent_id: OnceLock<String>,
    running: AtomicBool,
    /// Recreated fresh on every `start()` — see `start()`'s doc.
    cancel: StdMutex<Option<CancellationToken>>,
    heartbeat_interval_ms: AtomicU64,
    /// Whether `register()` has succeeded at least once since this process
    /// started — see `ensure_checked_in`. `false` never blocks the
    /// heartbeat/report calls; a Manager that already has this agent's
    /// `endpoint_agents` row (the assumed deployment model) accepts both
    /// even before the first successful check-in. `pub(crate)` so
    /// `crate::commands::cmd_enroll` can force a re-check-in.
    pub(crate) checked_in: AtomicBool,
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
            agent_id: OnceLock::new(),
            running: AtomicBool::new(false),
            cancel: StdMutex::new(None),
            heartbeat_interval_ms: AtomicU64::new(
                ModuleConfig::default().heartbeat_interval * 1000,
            ),
            checked_in: AtomicBool::new(false),
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
    pub(crate) inner: Arc<Inner>,
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

    /// Resolves config (defaults, then the host's raw YAML), reads the
    /// provisioned `api_key` credential from the host secret store — never
    /// from the config document, matching every other built-in module's
    /// rule for its own credential — builds the [`SkausWatchClient`], and
    /// registers all four metrics. Never begins background work or touches
    /// the network — that is [`Module::start`]'s job.
    ///
    /// `base_url` and `agent_id` are required config fields; `api_key`
    /// must already be present in the secret store under
    /// [`API_KEY_SECRET_KEY`] (the daemon/operator provisions it before
    /// this module ever starts). All three fail `init` outright if
    /// missing — unlike a license/feature-flag check, there is no
    /// graceful-degradation path for a foundational credential this
    /// module cannot function without.
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
        if cfg.agent_id.is_empty() {
            return Err(ModuleError::new("agent_id is required"));
        }

        let api_key = match host.secrets().get(API_KEY_SECRET_KEY).await {
            Ok(bytes) => String::from_utf8(bytes).map_err(|err| {
                ModuleError::new(format!("api_key secret is not valid UTF-8: {err}"))
            })?,
            Err(SecretError::NotFound) => {
                return Err(ModuleError::new(
                    "api_key secret is required (provision it via the host secret store before starting this module)",
                ));
            }
            Err(err) => {
                return Err(ModuleError::new(format!(
                    "failed to read api_key secret: {err}"
                )));
            }
        };
        if api_key.is_empty() {
            return Err(ModuleError::new(
                "api_key secret is required (provision it via the host secret store before starting this module)",
            ));
        }

        logger.info(
            "skauswatch config loaded",
            &[
                ("base_url", cfg.base_url.as_str()),
                ("agent_id", cfg.agent_id.as_str()),
                ("heartbeat_interval", &cfg.heartbeat_interval.to_string()),
            ],
        );

        let interval = Duration::from_secs(cfg.heartbeat_interval.max(1));
        self.inner
            .heartbeat_interval_ms
            .store(interval.as_millis() as u64, Ordering::SeqCst);

        let client_cfg = ClientConfig::new(
            cfg.base_url.clone(),
            cfg.agent_id.clone(),
            api_key,
            cfg.enrollment_token.clone(),
        );
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
        let _ = self.inner.agent_id.set(cfg.agent_id);

        Ok(())
    }

    /// Starts the background check-in/heartbeat/report loop and returns
    /// promptly.
    ///
    /// **Check-in happens inside the loop, not here.** The loop's first
    /// tick attempts a best-effort `register()` check-in (see
    /// `ensure_checked_in`), retrying on later ticks if it fails — this
    /// means `start()` never makes a network call and never blocks on an
    /// unreachable Manager, matching tobogganing's `start()`, whose own doc
    /// explains why a blocking `start` would wedge the whole daemon (the
    /// supervisor holds its lock across the call). Heartbeats and event
    /// reports proceed on every tick regardless of check-in outcome — the
    /// agent's identity is already provisioned, so a Manager that already
    /// holds this agent's `endpoint_agents` row accepts both even before
    /// the first successful check-in.
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

    /// Reports the module's running state plus this agent's provisioned
    /// identity and check-in status.
    async fn status(&self) -> Result<Status, ModuleError> {
        let state = if self.is_running() {
            ModuleState::Running
        } else {
            ModuleState::Disabled
        };

        let mut detail = HashMap::new();
        if let Some(agent_id) = self.inner.agent_id.get() {
            detail.insert("agent_id".to_string(), agent_id.clone());
        }
        detail.insert(
            "checked_in".to_string(),
            self.inner.checked_in.load(Ordering::SeqCst).to_string(),
        );

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

    /// Returns the SkausWatch CLI command tree: `status` and `enroll`.
    fn commands(&self) -> Vec<CommandSpec> {
        crate::commands::command_tree()
    }

    /// Dispatches a command path to its handler.
    async fn dispatch(
        &self,
        path: &[String],
        flags: &HashMap<String, String>,
        args: &[String],
    ) -> Result<CommandResult, ModuleError> {
        crate::commands::dispatch(self, path, flags, args).await
    }

    /// Returns the config schema.
    fn config_schema(&self) -> Option<Vec<u8>> {
        Some(crate::config::CONFIG_SCHEMA.as_bytes().to_vec())
    }
}

/// Best-effort check-in: calls `register()` once and latches
/// [`Inner::checked_in`] on success. A no-op once already checked in this
/// run. Called from every heartbeat tick (never from `start()` itself — see
/// that method's doc), so a Manager that's unreachable at startup is
/// retried automatically without ever blocking `start()`. Never panics and
/// never aborts the loop on failure — only logs and bumps `errors_total`.
async fn ensure_checked_in(module: &SkausWatchModule) {
    if module.inner.checked_in.load(Ordering::SeqCst) {
        return;
    }

    match module.client().register().await {
        Ok(response) => {
            module.host().logger().info(
                "agent checked in",
                &[
                    ("agent_id", response.agent_id.as_str()),
                    ("status", response.status.as_str()),
                ],
            );
            module.inner.checked_in.store(true, Ordering::SeqCst);
            module.metrics().checked_in.set(1.0);
        }
        Err(err) => {
            module.host().logger().warn(
                "check-in failed, will retry next tick",
                &[("error", &err.to_string())],
            );
            module.metrics().errors_total.inc();
        }
    }
}

/// Maps this module's locally-computed [`HealthLevel`] to the heartbeat
/// `status` string the Manager's `AGENT_STATUSES` accepts (`"active"`,
/// `"inactive"`, `"disconnected"` — `services/manager/src/routes/endpoint.rs`
/// ~line 31). A straight 1:1 mapping of the three grades:
///
/// - [`HealthLevel::Healthy`] (fully operational, a fresh heartbeat has
///   landed) reports `"active"`.
/// - [`HealthLevel::Degraded`] (operational, but the last successful
///   heartbeat is aging past the degraded threshold) reports `"inactive"`.
/// - [`HealthLevel::Unhealthy`] (no successful heartbeat has ever landed
///   this run) reports `"disconnected"` — the most conservative signal
///   available in the Manager's three-value enum.
///
/// This is a real, locally-computed grade sent every tick — never a
/// hardcoded `"active"`.
fn health_to_status(level: HealthLevel) -> &'static str {
    match level {
        HealthLevel::Healthy => "active",
        HealthLevel::Degraded => "inactive",
        HealthLevel::Unhealthy => "disconnected",
    }
}

/// One check-in/heartbeat/report cycle: best-effort check-in, a heartbeat
/// carrying this module's real computed health as its `status`, and
/// draining+reporting any queued events. Every fallible step logs, bumps
/// `errors_total`, and continues — this function never panics and never
/// aborts the loop on a transient error, so a failed Manager call is
/// retried on the next tick, never fatal.
async fn run_heartbeat_tick(module: &SkausWatchModule) {
    ensure_checked_in(module).await;

    // Read the health grade computed as of the end of the previous tick (or
    // `start()`'s synchronous probe, for the very first tick) — this is
    // what actually gets sent as this heartbeat's `status`, not a hardcoded
    // value.
    let status = health_to_status(module.health().await.level);

    match module.client().heartbeat(status).await {
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

    report_pending_events(module).await;
    update_health_probe(module);
}

/// Drains [`Inner::pending_events`] and reports the batch via
/// [`SkausWatchClient::report_events`]. On failure the drained events are
/// pushed back onto the front of the queue (ahead of anything queued while
/// the report was in flight) so a transient failure never silently drops an
/// observed event — it's retried on the next tick instead.
async fn report_pending_events(module: &SkausWatchModule) {
    let events = {
        let mut pending = module.inner.pending_events.lock().unwrap();
        std::mem::take(&mut *pending)
    };
    if events.is_empty() {
        return;
    }

    let event_count = events.len();
    if let Err(err) = module.client().report_events(&events).await {
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
/// call synchronously from `start()` as well as from every loop tick): no
/// heartbeat has ever succeeded is `Unhealthy`, a heartbeat older than
/// `DEGRADED_MULTIPLIER` intervals is `Degraded`, otherwise `Healthy`. This
/// agent's identity is always provisioned (never "unenrolled"), so unlike a
/// prior version of this probe there is no separate enrollment gate.
fn update_health_probe(module: &SkausWatchModule) {
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
    use skauswatch_client::Severity;

    use crate::testutil::{self, FakeHost, MockManager, MockResponse};

    use super::*;

    fn valid_config_bytes(base_url: &str, agent_id: &str, interval_secs: u64) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "base_url": base_url,
            "agent_id": agent_id,
            "heartbeat_interval": interval_secs,
        }))
        .unwrap()
    }

    async fn init_module() -> SkausWatchModule {
        let host = testutil::fake_host("http://127.0.0.1:1", "agent-init", "test-key", 60).await;
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

    async fn seed_register_route(manager: &MockManager, agent_id: &str, status: &str) {
        manager
            .respond(
                "POST",
                "/api/v1/endpoint/register",
                MockResponse::json(
                    200,
                    format!(r#"{{"message":"ok","agent_id":"{agent_id}","status":"{status}"}}"#),
                ),
            )
            .await;
    }

    async fn seed_heartbeat_route(manager: &MockManager) {
        manager
            .respond(
                "POST",
                "/api/v1/endpoint/heartbeat",
                MockResponse::json(
                    200,
                    r#"{"status":"active","agent_id":"a","timestamp":"2026-01-01T00:00:00Z"}"#,
                ),
            )
            .await;
    }

    async fn seed_events_route(manager: &MockManager) {
        manager
            .respond(
                "POST",
                "/api/v1/endpoint/events",
                MockResponse::json(
                    200,
                    r#"{"status":"accepted","events_received":1,"events_stored":1,"errors":[]}"#,
                ),
            )
            .await;
    }

    /// Builds and initializes a module wired to `manager_url` with a short,
    /// test-overridden heartbeat interval.
    async fn init_module_with_manager(manager_url: &str, agent_id: &str) -> SkausWatchModule {
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = valid_config_bytes(manager_url, agent_id, 60);
        host.secrets
            .set(API_KEY_SECRET_KEY, b"test-key")
            .await
            .unwrap();
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
        host.config = serde_json::to_vec(&serde_json::json!({"agent_id": "a"})).unwrap();
        let module = SkausWatchModule::new();
        let err = module.init(Arc::new(host)).await.unwrap_err();
        assert!(err.to_string().contains("base_url"));
    }

    #[tokio::test]
    async fn init_requires_agent_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = serde_json::to_vec(&serde_json::json!({"base_url": "http://x"})).unwrap();
        let module = SkausWatchModule::new();
        let err = module.init(Arc::new(host)).await.unwrap_err();
        assert!(err.to_string().contains("agent_id"));
    }

    /// The `api_key` credential must come from the secret store, not the
    /// config document — `init` fails outright when it's absent, since
    /// unlike a license/feature-flag check there is no graceful fallback
    /// for a foundational credential.
    #[tokio::test]
    async fn init_requires_api_key_secret() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = FakeHost::new(dir.path().to_path_buf());
        host.config = valid_config_bytes("http://127.0.0.1:1", "agent-1", 60);
        let module = SkausWatchModule::new();
        let err = module.init(Arc::new(host)).await.unwrap_err();
        assert!(err.to_string().contains("api_key"));
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
        host.config = valid_config_bytes("http://127.0.0.1:1", "agent-1", 60);
        host.secrets.set(API_KEY_SECRET_KEY, b"k").await.unwrap();
        let host: Arc<dyn HostServices> = Arc::new(host);

        let first = SkausWatchModule::new();
        first.init(host.clone()).await.expect("first init succeeds");

        let second = SkausWatchModule::new();
        let err = second.init(host).await.unwrap_err();
        assert!(err.to_string().contains("register metrics"));
    }

    /// Proves the lifecycle is clean (start returns promptly with no live
    /// endpoint required — check-in happens inside the loop, not on the
    /// `start()` path) and `stop()` is idempotent.
    #[tokio::test(start_paused = true)]
    async fn start_then_stop_is_clean_and_idempotent() {
        let module = SkausWatchModule::new();
        let host = testutil::fake_host("http://127.0.0.1:1", "agent-1", "k", 30).await;
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
        seed_register_route(&manager, "agent-1", "active").await;
        seed_heartbeat_route(&manager).await;

        let module = init_module_with_manager(&manager.base_url, "agent-1").await;

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
    async fn status_reports_not_checked_in_before_any_tick() {
        let module = init_module().await;
        let status = module.status().await.unwrap();
        assert_eq!(
            status.detail.get("checked_in").map(String::as_str),
            Some("false")
        );
        assert_eq!(
            status.detail.get("agent_id").map(String::as_str),
            Some("agent-init")
        );
    }

    /// End-to-end: the loop checks in (no prior check-in this run), and
    /// sends a heartbeat using the provisioned identity — proving
    /// `ensure_checked_in`'s register path and `run_heartbeat_tick`'s
    /// heartbeat call both work against a real (mocked) Manager. No
    /// identity is persisted anywhere: the Manager never issues one.
    #[tokio::test]
    async fn heartbeat_loop_checks_in_and_heartbeats() {
        let manager = MockManager::start().await;
        seed_register_route(&manager, "agent-42", "active").await;
        seed_heartbeat_route(&manager).await;

        let module = init_module_with_manager(&manager.base_url, "agent-42").await;
        module.start().await.expect("start succeeds");

        wait_for(|| module.metrics().heartbeats_total.get() >= 1.0).await;

        assert_eq!(
            manager
                .request_count("POST", "/api/v1/endpoint/register")
                .await,
            1
        );
        assert_eq!(module.metrics().checked_in.get(), 1.0);

        let status = module.status().await.unwrap();
        assert_eq!(
            status.detail.get("agent_id").map(String::as_str),
            Some("agent-42")
        );
        assert_eq!(
            status.detail.get("checked_in").map(String::as_str),
            Some("true")
        );

        let health = module.health().await;
        assert_eq!(health.level, HealthLevel::Healthy);

        module.stop().await.ok();
        manager.stop().await;
    }

    /// Once check-in succeeds it must not repeat on every tick — proves
    /// `ensure_checked_in`'s short-circuit, not just that it eventually
    /// succeeds once.
    #[tokio::test]
    async fn heartbeat_loop_only_checks_in_once_across_multiple_ticks() {
        let manager = MockManager::start().await;
        seed_register_route(&manager, "agent-2", "active").await;
        seed_heartbeat_route(&manager).await;

        let module = init_module_with_manager(&manager.base_url, "agent-2").await;
        module.start().await.expect("start succeeds");

        wait_for(|| module.metrics().heartbeats_total.get() >= 3.0).await;

        assert_eq!(
            manager
                .request_count("POST", "/api/v1/endpoint/register")
                .await,
            1,
            "register must only be called once check-in has succeeded"
        );

        module.stop().await.ok();
        manager.stop().await;
    }

    /// A failed check-in is retried on the next tick rather than wedging
    /// the loop — proves the "never panic, never exit the loop"
    /// requirement for the check-in path specifically.
    #[tokio::test]
    async fn heartbeat_loop_retries_check_in_after_a_failure() {
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
                MockResponse::json(
                    200,
                    r#"{"message":"ok","agent_id":"agent-7","status":"active"}"#,
                ),
            )
            .await;
        seed_heartbeat_route(&manager).await;

        let module = init_module_with_manager(&manager.base_url, "agent-7").await;
        module.start().await.expect("start succeeds");

        wait_for(|| module.metrics().errors_total.get() >= 1.0).await;
        wait_for(|| module.metrics().checked_in.get() >= 1.0).await;
        wait_for(|| module.metrics().heartbeats_total.get() >= 1.0).await;

        module.stop().await.ok();
        manager.stop().await;
    }

    /// Regression for "Health -> status (real, not hardcoded)": the first
    /// heartbeat of a fresh process fires before any heartbeat has ever
    /// succeeded, so `update_health_probe` grades it `Unhealthy` and this
    /// tick must send `"disconnected"` (see `health_to_status`'s doc); once
    /// a heartbeat has landed the *next* tick must send `"active"`. A
    /// hardcoded status string would send the same value both times.
    #[tokio::test]
    async fn heartbeat_status_reflects_computed_health_not_a_hardcoded_value() {
        let manager = MockManager::start().await;
        seed_register_route(&manager, "agent-55", "active").await;
        seed_heartbeat_route(&manager).await;

        let module = init_module_with_manager(&manager.base_url, "agent-55").await;
        module.start().await.expect("start succeeds");

        wait_for(|| module.metrics().heartbeats_total.get() >= 2.0).await;

        let requests = manager.requests().await;
        let heartbeats: Vec<_> = requests
            .iter()
            .filter(|req| {
                req.method == "POST" && req.path.starts_with("/api/v1/endpoint/heartbeat")
            })
            .collect();
        assert!(
            heartbeats.len() >= 2,
            "expected at least two recorded heartbeat requests"
        );
        assert!(
            heartbeats[0].body.contains(r#""status":"disconnected""#),
            "first heartbeat must report the pre-heartbeat Unhealthy grade; body: {}",
            heartbeats[0].body
        );
        assert!(
            heartbeats[1].body.contains(r#""status":"active""#),
            "second heartbeat must report the Healthy grade set after the first succeeded; body: {}",
            heartbeats[1].body
        );

        module.stop().await.ok();
        manager.stop().await;
    }

    /// Events queued between ticks are drained and reported on the next
    /// tick via `report_events`.
    #[tokio::test]
    async fn heartbeat_loop_drains_queued_events_via_report_events() {
        let manager = MockManager::start().await;
        seed_register_route(&manager, "agent-9", "active").await;
        seed_heartbeat_route(&manager).await;
        seed_events_route(&manager).await;

        let module = init_module_with_manager(&manager.base_url, "agent-9").await;
        module.queue_event_for_test(EndpointEvent {
            agent_id: "agent-9".to_string(),
            event_type: "module_fault".to_string(),
            severity: Some(Severity::Medium),
            process_name: None,
            process_path: None,
            process_hash: None,
            parent_process: None,
            command_line: None,
            network_connections: None,
            file_operations: None,
            registry_operations: None,
            details: Some(serde_json::json!({"module": "squawk"})),
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
        seed_register_route(&manager, "agent-11", "active").await;
        seed_heartbeat_route(&manager).await;
        manager
            .respond(
                "POST",
                "/api/v1/endpoint/events",
                MockResponse::json(500, "{}"),
            )
            .await;
        manager
            .respond(
                "POST",
                "/api/v1/endpoint/events",
                MockResponse::json(
                    200,
                    r#"{"status":"accepted","events_received":1,"events_stored":1,"errors":[]}"#,
                ),
            )
            .await;

        let module = init_module_with_manager(&manager.base_url, "agent-11").await;
        module.queue_event_for_test(EndpointEvent {
            agent_id: "agent-11".to_string(),
            event_type: "module_fault".to_string(),
            severity: Some(Severity::Critical),
            process_name: None,
            process_path: None,
            process_hash: None,
            parent_process: None,
            command_line: None,
            network_connections: None,
            file_operations: None,
            registry_operations: None,
            details: None,
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
    async fn start_makes_a_pre_heartbeat_health_report_available_immediately() {
        let module = init_module().await;
        module.start().await.expect("start succeeds");
        let health = module.health().await;
        assert_eq!(health.level, HealthLevel::Unhealthy);
        assert!(health.message.contains("no successful heartbeat"));
        module.stop().await.ok();
    }

    /// Drives `update_health_probe` directly (not through the loop) so
    /// degraded-age grading is deterministic rather than racing a ticker.
    #[tokio::test]
    async fn health_probe_reports_degraded_once_the_last_heartbeat_goes_stale() {
        let module = init_module().await;
        module.set_heartbeat_interval_for_test(Duration::from_millis(10));
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
        *module.inner.last_heartbeat_ok.lock().unwrap() = Some(SystemTime::now());

        update_health_probe(&module);

        let health = module.health().await;
        assert_eq!(health.level, HealthLevel::Healthy);
    }

    #[test]
    fn health_to_status_maps_the_three_grades_to_agent_statuses() {
        assert_eq!(health_to_status(HealthLevel::Healthy), "active");
        assert_eq!(health_to_status(HealthLevel::Degraded), "inactive");
        assert_eq!(health_to_status(HealthLevel::Unhealthy), "disconnected");
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
    async fn dispatch_unknown_command_returns_error_result() {
        let module = init_module().await;
        let result = module.dispatch(&[], &HashMap::new(), &[]).await.unwrap();
        assert_eq!(result.exit_code, 1);
        assert!(result.output.contains("no command specified"));
    }

    #[tokio::test]
    async fn dispatch_status_json_returns_structured_result() {
        let module = SkausWatchModule::new();
        module
            .init(testutil::fake_host_default().await)
            .await
            .unwrap();
        let flags = HashMap::from([("json".to_string(), "true".to_string())]);
        let out = module
            .dispatch(&["status".to_string()], &flags, &[])
            .await
            .expect("dispatch ok");
        assert!(out.output.contains("checked_in") || out.output.contains("agent_id"));
    }
}
