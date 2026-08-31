//! Unix daemon startup: config, telemetry, the single-instance lock, the
//! supervisor, and the gRPC server over the control socket. Ported from the
//! combined `go-client/cmd/penguind/main.go` + `service.go` startup path.
//!
//! Windows service-manager integration (`kardianos/service` in Go) is out of
//! scope here — see [`super::main`]'s `service` short-circuit — so this
//! module only ever runs the daemon in the Go reference's "interactive mode"
//! shape: serve directly, wait for a signal, shut down.

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Server;

use penguin_daemon::broker::EventBroker;
use penguin_daemon::config::{ConfigStore, DaemonConfig};
use penguin_daemon::external::{ExternalLoader, PluginDirLoader};
use penguin_daemon::host::{DaemonHostFactory, HostFactory, SecretStoreProvider};
use penguin_daemon::lock::{self, LockError};
use penguin_daemon::logring::LogRing;
use penguin_daemon::service::{DaemonService, OtelStatusSummary, UpdateClient};
use penguin_daemon::supervisor::{Supervisor, SupervisorConfig};
use penguin_ipc::groups_unix::SystemGroups;
use penguin_ipc::listen_unix::{self, ListenerConfig, PeerAuthInterceptor};
use penguin_ipc::{GroupResolver, IpcError};
use penguin_licensing::{LicenseClient, LicenseClientOptions};
use penguin_otel::{OtelConfig, OtelPipeline};
use penguin_proto::daemon::v1::daemon_server::DaemonServer;
use penguin_proto::desktop::v1::bridge_action_proxy_server::BridgeActionProxyServer;
use penguin_proto::desktop::v1::session_proxy_server::SessionProxyServer;
use penguin_sdk::{
    EventSink, LicenseChecker, LogLevel, ModuleTelemetry, NoopTelemetry, SecretError, SecretStore,
};
use penguin_secrets::{Backend as SecretsBackend, Config as SecretsConfig, Store as SecretsStore};
use penguin_selfprotect::{
    LocalFileSource, ManifestSource, NoopConsoleSink, is_armed, scan_heal_report,
};
use penguin_telemetry::{Telemetry, TelemetryError};
use tokio_util::sync::CancellationToken;

use crate::host_wiring::SecretsStoreProvider;
use crate::{VERSION, logging};
use penguin_daemon::service::{BridgeActionProxyService, SessionProxyService};

/// Log lines retained per source (a module name, or `""` for the daemon
/// itself) before the oldest are evicted. Generous but arbitrary — nothing
/// in the Go reference specifies a size, since Go never implemented
/// `TailLogs` at all.
const LOG_RING_CAPACITY: usize = 2000;

/// Events buffered per `WatchEvents` subscriber before the slowest one
/// starts lagging.
const EVENT_BROKER_CAPACITY: usize = 256;

/// How often a loaded module's health is polled. A Rust-only addition (see
/// `penguin_daemon`'s crate-level divergence list) with no Go equivalent to
/// match, so this is a reasonable starting default pending real tuning.
const HEALTH_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// How long a module must run before a subsequent failure resets its
/// restart budget. Same "no Go equivalent" caveat as
/// [`HEALTH_POLL_INTERVAL`].
const STABILITY_WINDOW: Duration = Duration::from_secs(5 * 60);

/// How often the license client re-validates against `license.penguintech.io`
/// in the background. Matches the Go client's default
/// `Options.RefreshInterval` (`go-client/internal/licensing/client.go`).
const LICENSE_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The GitHub repository self-updates are fetched from — matches
/// `go-client/.goreleaser.yaml`'s `release.github.{owner,name}`.
const RELEASE_REPO: &str = "penguintechinc/penguin";

/// The OTLP collector endpoint used when the daemon config has no `otel`
/// section of its own. `DaemonConfig` deliberately carries no `otel` field
/// yet (see the wiring comment in [`run_daemon`]) — the console-driven
/// override this constant would otherwise come from is the SP2 follow-up
/// [`OtelConfig::merge`] already has a hook for, so a hardcoded local
/// default is the minimal correct wiring for this milestone rather than
/// adding a config field with no writer yet.
const DEFAULT_OTEL_ENDPOINT: &str = "http://localhost:4318";

/// The minisign public key trusted to verify release archives.
///
/// `None` today: baking the real PenguinTech release-signing key into this
/// binary is a deliberate, reviewed follow-up (see `docs/PARITY.md` and the
/// M7.2 task), not something to fabricate here — a placeholder or made-up
/// key would be strictly worse than no key at all, since it would look
/// configured while verifying nothing real. With no key, `check_update`
/// still works and can report a release is available (read-only, harmless),
/// but `apply_update` fails closed with "no release verification key
/// configured" — see [`penguin_update::UpdateError::NoVerificationKey`].
const RELEASE_PUBLIC_KEY: Option<&str> = None;

/// Secret-store namespace the self-protection "enrolled" proxy reads the
/// tamper secret's hash from — must match `crate::service`'s own private
/// `SELFPROTECT_SECRET_NAMESPACE` (duplicated rather than shared, same
/// convention as `watchdog.rs`'s duplicated `DEFAULT_STATE_DIR`: that
/// module's constant is private to its own file, and there is no shared
/// `selfprotect`-constants module yet).
const SELFPROTECT_SECRET_NAMESPACE: &str = "selfprotect";

/// Secret-store key, within [`SELFPROTECT_SECRET_NAMESPACE`], the tamper-
/// protection secret's Argon2id PHC hash is stored under — must match
/// `crate::service::TAMPER_SECRET_KEY`.
const SELFPROTECT_TAMPER_SECRET_KEY: &str = "tamper_secret";

/// Where the (SP2-provisioned) signed integrity manifest is read from.
/// `LocalFileSource` only, for this milestone — a controller-fetched source
/// implementing the same `penguin_selfprotect::ManifestSource` trait is the
/// SP2 follow-up (see that trait's doc). No manifest ships at this path
/// yet, so `scan_heal_report` safely logs-and-no-ops every tick on a fresh
/// install rather than acting on a missing/unverified manifest.
const SELFPROTECT_MANIFEST_PATH: &str = "/etc/penguin/selfprotect-manifest.json";

/// Root the integrity manifest's relative entry paths (e.g.
/// `"bin/penguind"`) resolve against — this agent's install root. `/` is
/// deliberately conservative: no install-root convention has been settled
/// elsewhere in this codebase yet (packaging is still M7), so manifest
/// entries are assumed already relative to the filesystem root until SP2
/// settles a real value.
const SELFPROTECT_ROOT: &str = "/";

/// Directory holding the pristine "protected" copies
/// `penguin_selfprotect::heal` restores tampered/missing files from. SP2
/// provisions this out of band, alongside a real signed manifest; until
/// then it is expected to be empty on most installs, so any heal attempt
/// simply logs and is skipped rather than crashing the daemon (see
/// `penguin_selfprotect::heal`'s own error handling, and
/// `penguin_selfprotect::scan_heal_report`'s per-finding loop).
const SELFPROTECT_PROTECTED_DIR: &str = "/var/lib/penguind/selfprotect/protected";

/// Minisign public key trusted to verify the integrity manifest's
/// signature. Empty — not a real key — for the same reason
/// [`RELEASE_PUBLIC_KEY`]/`service::BREAK_GLASS_PUBKEY` are placeholders: a
/// made-up key would look configured while verifying nothing real. An empty
/// key fails to parse, so every `scan_heal_report` cycle safely no-ops
/// (never trusts an unverified manifest) until SP2 bakes in the real
/// PenguinTech integrity-manifest signing key.
const SELFPROTECT_PUBKEY: &str = "";

/// How often the armed self-protection loop runs one
/// `penguin_selfprotect::scan_heal_report` cycle. `tokio::time::interval`'s
/// first tick fires immediately, so an armed daemon's first scan happens at
/// startup, not after this delay.
const SELFPROTECT_SCAN_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Adapts [`penguin_update::Updater`] to [`UpdateClient`], the trait
/// [`DaemonService`]'s `CheckUpdate`/`ApplyUpdate` RPC handlers consult (see
/// that trait's doc — this file's construction below used to always pass
/// `None`). The only work this adapter does is flatten
/// `penguin_update::UpdateError` to the trait's plain `String` error type.
struct SelfUpdateClient {
    updater: penguin_update::Updater,
}

#[async_trait::async_trait]
impl UpdateClient for SelfUpdateClient {
    async fn check_update(&self) -> Result<(bool, String), String> {
        self.updater.check().await.map_err(|err| err.to_string())
    }

    async fn apply_update(&self) -> Result<(), String> {
        self.updater.apply().await.map_err(|err| err.to_string())
    }
}

/// Command-line flags for normal daemon startup. The `version`/`--version`
/// and `service` short-circuits in [`super::main`] never reach this parser.
#[derive(Parser, Debug)]
#[command(name = "penguind", disable_version_flag = true)]
struct Args {
    /// Configuration directory (`config.yaml` + `modules.d/`).
    #[arg(long, default_value = "/etc/penguin")]
    config_dir: PathBuf,
    /// State directory: single-instance lock, persisted enabled-set, and
    /// each module's private data directory.
    #[arg(long, default_value = "/var/lib/penguind")]
    state_dir: PathBuf,
    /// Overrides `config.yaml`'s `socketPath` when non-empty.
    #[arg(long, default_value = "")]
    socket: String,
}

/// A fatal daemon-startup failure. [`run`] prints this and exits non-zero;
/// every variant corresponds to one of the "failure is fatal" steps in the
/// startup sequence.
#[derive(Debug, thiserror::Error)]
enum DaemonBinError {
    /// Creating `--state-dir` failed.
    #[error("create state dir: {0}")]
    StateDir(#[source] std::io::Error),
    /// Another `penguind` instance already holds the single-instance lock,
    /// or the lock file itself could not be opened.
    #[error(transparent)]
    Lock(#[from] LockError),
    /// The daemon's log level (from `config.yaml`, or its default) failed
    /// to validate.
    #[error(transparent)]
    Telemetry(#[from] TelemetryError),
    /// Opening the encrypted-file secret store failed — fatal, matching
    /// Go's `secrets.Open` failure path in `cmd/penguind/service.go` (it
    /// releases the single-instance lock and returns an error rather than
    /// starting with no secret storage at all).
    #[error("init secrets store: {0}")]
    Secrets(#[from] SecretError),
    /// Binding the control socket failed.
    #[error(transparent)]
    Listen(#[from] IpcError),
    /// The gRPC server itself failed (not a graceful shutdown).
    #[error("serve: {0}")]
    Serve(#[from] tonic::transport::Error),
}

/// Builds a tokio runtime and drives [`run_daemon`] to completion, mapping
/// any fatal error to a printed message and a non-zero exit code.
pub fn run() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("penguind: build async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    let Err(err) = runtime.block_on(run_daemon()) else {
        return ExitCode::SUCCESS;
    };
    eprintln!("penguind: {err}");
    ExitCode::FAILURE
}

/// The full startup sequence: load config, init telemetry, acquire the
/// single-instance lock, build the supervisor and gRPC service, serve until
/// a shutdown signal, then shut the supervisor down cleanly.
async fn run_daemon() -> Result<(), DaemonBinError> {
    let args = Args::parse();

    let config_store = ConfigStore::new(args.config_dir.clone());
    let mut daemon_cfg = match config_store.daemon() {
        Ok(cfg) => cfg,
        Err(err) => {
            // No logger exists yet at this point in the sequence — config
            // load happens before telemetry installs one.
            eprintln!("penguind: invalid daemon config, using defaults: {err}");
            DaemonConfig::defaults()
        }
    };
    if !args.socket.is_empty() {
        daemon_cfg.socket_path = args.socket.clone();
    }

    ensure_state_dir(&args.state_dir).map_err(DaemonBinError::StateDir)?;
    // Single-instance guard. Held for the rest of this function's lifetime;
    // dropping it (on any return path) releases the lock.
    let _lock_guard = lock::acquire(&args.state_dir)?;

    // The LogRing must exist before installing tracing (its layer needs a
    // handle to append into), so this precedes `Telemetry::new` — the one
    // step reordered from the brief's literal list, since EventBroker has
    // no such dependency and can stay wherever.
    let logs = Arc::new(LogRing::new(LOG_RING_CAPACITY));
    logging::install(logs.clone(), &daemon_cfg.log_level);
    let telemetry = Arc::new(Telemetry::new(&daemon_cfg.log_level)?);

    // Best-effort: spawn this daemon's mutual-supervision peer (see
    // `crate::watchdog`'s module doc). Never fatal — a spawn failure here
    // must not stop the daemon itself from starting, it only means the
    // daemon-keeps-watchdog-alive half of mutual supervision didn't start
    // this time (the watchdog side, once running, keeps trying to relaunch
    // the daemon regardless). Placed after the single-instance lock is
    // held, so a spawned watchdog's very first liveness check always finds
    // a live daemon rather than racing this process's own startup.
    spawn_watchdog_peer();

    let broker = Arc::new(EventBroker::new(EVENT_BROKER_CAPACITY));
    let events: Arc<dyn EventSink> = broker.clone();

    // Real M4 secret store: XChaCha20-Poly1305-encrypted files under
    // `<state_dir>/secrets`, with the master key alongside them (see
    // `penguin_secrets::file_backend`). `FileOnly` is deliberate, not a
    // placeholder — the daemon is headless, and `Backend::Auto` would probe
    // a platform keyring/Secret Service, which must never happen here (see
    // that crate's module doc, "Never let a test touch a real OS keyring" —
    // the same reasoning applies in production for a service with no
    // desktop session).
    let secrets_root = Arc::new(SecretsStore::open(SecretsConfig {
        service_name: String::new(),
        backend: SecretsBackend::FileOnly {
            file_dir: args.state_dir.join("secrets"),
        },
    })?);

    // Real M4 license client: license.penguintech.io with an offline cache
    // under `<state_dir>/license`. `LICENSE_KEY` matches the env var Go
    // reads in `cmd/penguind/service.go`. A missing/empty key or an
    // unreachable server both degrade gracefully — see `LicenseClient`'s own
    // doc — rather than stopping the daemon from starting.
    let license_client = Arc::new(LicenseClient::new(LicenseClientOptions {
        license_key: env::var("LICENSE_KEY").unwrap_or_default(),
        product: String::new(),
        base_url: String::new(),
        cache_dir: Some(args.state_dir.join("license")),
    }));
    let license_refresh = license_client.spawn_background_refresh(LICENSE_REFRESH_INTERVAL);
    let license: Arc<dyn LicenseChecker> = license_client;

    // OpenTelemetry pipeline: gated purely on the `penguin.otel` license
    // flag via `LicenseChecker`, never an env var/CLI override (see
    // `critical-rules.md` Feature Flags & License Tiers). Console-provided
    // config override (SP2) is not implemented yet, so `OtelConfig::merge`
    // always sees `None` here — a deliberate hook for that follow-up, not a
    // missing feature in this milestone. A build failure (bad endpoint,
    // etc.) degrades to no telemetry rather than aborting startup — modules
    // still get a safe `NoopTelemetry` handle either way, see
    // `penguin_daemon::host::DaemonHost::telemetry`.
    let node_id = nix::unistd::gethostname()
        .ok()
        .and_then(|name| name.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string());
    let otel_cfg = OtelConfig::merge(
        OtelConfig {
            endpoint: DEFAULT_OTEL_ENDPOINT.to_string(),
            sampling_ratio: 1.0,
            enabled: license.feature_enabled("penguin.otel"),
        },
        None,
    );
    // `GetStatus`'s `otel` field reports the daemon's configured exporter
    // state (`pdcli otel status`), independent of whether building the real
    // pipeline below actually succeeded — `enabled`/`endpoint` reflect
    // configuration intent, not pipeline build outcome.
    let otel_status = OtelStatusSummary {
        enabled: otel_cfg.enabled,
        endpoint: otel_cfg.endpoint.clone(),
    };
    let otel: Option<Arc<OtelPipeline>> = if otel_cfg.enabled {
        match OtelPipeline::build(
            &otel_cfg,
            &[("node_id", node_id.as_str()), ("service.version", VERSION)],
        ) {
            Ok(pipeline) => Some(Arc::new(pipeline)),
            Err(err) => {
                tracing::warn!(error = %err, "failed to build otel pipeline; telemetry export disabled");
                None
            }
        }
    } else {
        None
    };

    // Self-protection: arm only when enrolled AND the `penguin.self-protection`
    // feature flag is on (`penguin_selfprotect::is_armed`). "Enrolled" uses
    // the same interim proxy as `crate::service::resolve_teardown_ctx` (see
    // that function's doc): a tamper-protection secret already provisioned
    // in the secrets store is the best available enrollment signal until
    // SP2 adds real enrollment state. `secrets_root` is only borrowed here
    // (via `Store::namespaced`, which clones just the cheap namespace
    // prefix) — it is still moved into `secrets_provider` below.
    let selfprotect_flag_on = license.feature_enabled("penguin.self-protection");
    let selfprotect_enrolled = secrets_root
        .namespaced(SELFPROTECT_SECRET_NAMESPACE)
        .get(SELFPROTECT_TAMPER_SECRET_KEY)
        .await
        .is_ok();
    let selfprotect_loop = if is_armed(selfprotect_enrolled, selfprotect_flag_on) {
        let selfprotect_telemetry: Arc<dyn ModuleTelemetry> = match &otel {
            Some(pipeline) => pipeline.scoped("selfprotect"),
            None => Arc::new(NoopTelemetry),
        };
        tracing::info!("selfprotect: armed; integrity loop starting");
        Some(spawn_selfprotect_loop(
            node_id.clone(),
            selfprotect_telemetry,
        ))
    } else {
        tracing::debug!(
            enrolled = selfprotect_enrolled,
            flag_on = selfprotect_flag_on,
            "selfprotect: not armed; integrity loop not started"
        );
        None
    };

    // Per-module secret isolation is part of DaemonHostFactory's own
    // contract (see `penguin_daemon::host::SecretStoreProvider`):
    // `SecretsStoreProvider` gives every module its own
    // `secrets_root.namespaced(module)` view, so `host_for` never hands two
    // modules the same store.
    let secrets_provider: Arc<dyn SecretStoreProvider> =
        Arc::new(SecretsStoreProvider::new(secrets_root));
    let host_factory: Arc<dyn HostFactory> = Arc::new(DaemonHostFactory::new(
        telemetry,
        Arc::new(config_store),
        secrets_provider,
        license,
        events,
        args.state_dir.clone(),
        otel.clone(),
    ));

    // `daemon_cfg.plugins_dir` is scanned lazily, one `<name>/` at a time,
    // only when something actually tries to `load` that name — constructing
    // the loader here never touches the filesystem itself, so a plugins_dir
    // that doesn't exist yet (or never will) cannot stop the daemon from
    // starting: every `load` for a name the builtin registry doesn't know
    // just resolves to `PluginDirLoader`'s `NotFound`, exactly like an
    // unregistered builtin name always has.
    let plugin_socket_dir = args.state_dir.join("plugin-sockets");
    if let Err(err) = ensure_state_dir(&plugin_socket_dir) {
        tracing::warn!(
            error = %err,
            dir = %plugin_socket_dir.display(),
            "failed to create plugin socket dir; external plugin loading may be degraded"
        );
    }
    let external: Arc<dyn ExternalLoader> = Arc::new(PluginDirLoader::new(
        PathBuf::from(&daemon_cfg.plugins_dir),
        plugin_socket_dir,
        self_uid(),
    ));

    let supervisor = Supervisor::new(SupervisorConfig {
        // M5: squawk is the first real built-in module (penguin-tobogganing
        // lands in its own later milestone).
        registry: penguin_registry::builtin_modules(),
        host_factory,
        broker: broker.clone(),
        state_dir: args.state_dir.clone(),
        max_restarts: 0, // 0 = use the crate default (backoff::MAX_RESTARTS)
        health_interval: HEALTH_POLL_INTERVAL,
        stability_window: STABILITY_WINDOW,
        external: Some(external),
    });

    for (name, err) in supervisor.start_enabled().await {
        tracing::warn!(module = %name, error = %err, "failed to restore persisted module");
    }

    let update_client: Arc<dyn UpdateClient> = Arc::new(SelfUpdateClient {
        updater: penguin_update::Updater::new(penguin_update::UpdateConfig {
            repo: RELEASE_REPO.to_string(),
            current_version: VERSION.to_string(),
            binary_name: "penguind".to_string(),
            public_key: RELEASE_PUBLIC_KEY.map(str::to_string),
        }),
    });
    let daemon_service = DaemonService::new(
        supervisor.clone(),
        broker,
        logs,
        VERSION,
        Some(update_client),
        otel_status,
    );

    let listener_cfg = ListenerConfig {
        path: PathBuf::from(&daemon_cfg.socket_path),
        allowed_group: daemon_cfg.group.clone(),
    };
    let listener = listen_unix::listen(&listener_cfg)?;
    tracing::info!(socket = %daemon_cfg.socket_path, "listening on control socket");
    let incoming = listen_unix::incoming(listener);

    let resolver: Arc<dyn GroupResolver> = Arc::new(SystemGroups);
    let peer_auth = PeerAuthInterceptor::new(self_uid(), daemon_cfg.group.clone(), resolver);

    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<DaemonServer<DaemonService>>()
        .await;

    let daemon_svc = InterceptedService::new(DaemonServer::new(daemon_service), peer_auth.clone());
    let health_svc = InterceptedService::new(health_service, peer_auth.clone());

    // Register the session proxy service (Phase 4a/4b poll loop support).
    let session_proxy_svc = InterceptedService::new(
        SessionProxyServer::new(SessionProxyService::new(supervisor.clone())),
        peer_auth.clone(),
    );

    // Register the bridge action proxy service (Phase 4c OBS/webhook dispatch).
    let bridge_action_svc = InterceptedService::new(
        BridgeActionProxyServer::new(BridgeActionProxyService::new(supervisor.clone())),
        peer_auth,
    );

    let serve_result = Server::builder()
        .add_service(daemon_svc)
        .add_service(health_svc)
        .add_service(session_proxy_svc)
        .add_service(bridge_action_svc)
        .serve_with_incoming_shutdown(incoming, wait_for_shutdown_signal())
        .await;

    tracing::info!("shutting down");
    supervisor.shutdown().await;
    license_refresh.stop().await;
    // Stop the self-protection loop (if armed) *before* the otel flush
    // attempt below: `SelfProtectLoopHandle::stop` awaits the loop task's
    // actual exit, guaranteeing its `ModuleTelemetry` handle — and the
    // `Arc<OtelPipeline>` clone `OtelPipeline::scoped` hands back — is
    // dropped first, so `Arc::into_inner(pipeline)` below has its best
    // chance of finding this the sole surviving reference.
    if let Some(handle) = selfprotect_loop {
        handle.stop().await;
    }
    // Best-effort flush: succeeds only if this is the last surviving handle
    // on the pipeline. In practice every loaded module's `DaemonHost` (and
    // any still-unwinding health-poll/restart background task — see
    // `Supervisor`'s own "cheap to clone... background tasks hold their own
    // handle" doc) may still be holding a clone at this point, so losing this
    // race is expected and never treated as an error — a real network
    // exporter must never block process shutdown.
    if let Some(pipeline) = otel {
        match Arc::into_inner(pipeline) {
            Some(pipeline) => pipeline.shutdown(),
            None => tracing::debug!(
                "otel pipeline has other live references at shutdown; skipping final flush"
            ),
        }
    }
    let _ = std::fs::remove_file(&daemon_cfg.socket_path);

    serve_result.map_err(DaemonBinError::Serve)
}

/// A running self-protection loop started by [`spawn_selfprotect_loop`].
/// Same shape as `penguin_licensing::RefreshHandle`: dropping it leaves the
/// loop running, so [`stop`](Self::stop) is the only way to actually halt
/// it — used by `run_daemon`'s shutdown path (see that call site's doc for
/// why stopping this *before* the otel flush attempt matters).
struct SelfProtectLoopHandle {
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl SelfProtectLoopHandle {
    /// Signals the loop to stop and waits for its task to actually exit.
    async fn stop(self) {
        self.cancel.cancel();
        let _ = self.task.await;
    }
}

/// Spawns the armed self-protection loop: `tokio::time::interval`'s first
/// tick fires immediately, so the first
/// `penguin_selfprotect::scan_heal_report` cycle runs right at arm time,
/// then again every [`SELFPROTECT_SCAN_INTERVAL`] until
/// [`SelfProtectLoopHandle::stop`] is called.
///
/// Each cycle checks the manifest at [`SELFPROTECT_MANIFEST_PATH`] against
/// [`SELFPROTECT_ROOT`], heals any finding from [`SELFPROTECT_PROTECTED_DIR`],
/// reports it to [`NoopConsoleSink`] (see that type's doc — SP2 provisions
/// the real console-backed sink), and emits every returned `TamperEvent` to
/// `otel` (a `selfprotect_tamper_total` counter plus a warn-level log).
///
/// Never crashes the daemon: `scan_heal_report` already catches every
/// fallible step inside one cycle (manifest load, signature verification,
/// heal — see that function's doc), and this loop adds no further fallible
/// work of its own.
fn spawn_selfprotect_loop(
    node_id: String,
    otel: Arc<dyn ModuleTelemetry>,
) -> SelfProtectLoopHandle {
    let cancel = CancellationToken::new();
    let loop_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        let source = LocalFileSource {
            path: PathBuf::from(SELFPROTECT_MANIFEST_PATH),
        };
        let root = Path::new(SELFPROTECT_ROOT);
        let protected_dir = Path::new(SELFPROTECT_PROTECTED_DIR);
        let mut ticker = tokio::time::interval(SELFPROTECT_SCAN_INTERVAL);

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    run_selfprotect_cycle(&source, root, protected_dir, &node_id, &otel);
                }
                _ = loop_cancel.cancelled() => {
                    tracing::info!("selfprotect: integrity loop stopped");
                    break;
                }
            }
        }
    });
    SelfProtectLoopHandle { cancel, task }
}

/// Runs one `penguin_selfprotect::scan_heal_report` cycle — timestamped with
/// the current wall-clock time, never inside `scan_heal_report` itself, so
/// that function stays pure and deterministic for tests — and emits every
/// returned `TamperEvent` to `otel`: a `selfprotect_tamper_total` counter
/// increment tagged with the event's kind, plus a warn-level log naming the
/// path and kind.
fn run_selfprotect_cycle(
    source: &dyn ManifestSource,
    root: &Path,
    protected_dir: &Path,
    node_id: &str,
    otel: &Arc<dyn ModuleTelemetry>,
) {
    // A clock set before the Unix epoch is never expected in practice; the
    // `unwrap_or(0)` fallback exists so a broken system clock degrades this
    // cycle's timestamp rather than panicking the daemon.
    let ts_unix = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);

    let events = scan_heal_report(
        source,
        SELFPROTECT_PUBKEY,
        root,
        protected_dir,
        node_id,
        ts_unix,
        &NoopConsoleSink,
    );

    for event in events {
        let kind = format!("{:?}", event.kind);
        otel.counter_add("selfprotect_tamper_total", 1, &[("kind", kind.as_str())]);
        otel.emit_log(
            LogLevel::Warn,
            "selfprotect: tamper detected and healed",
            &[("path", event.path.as_str()), ("kind", kind.as_str())],
        );
    }
}

/// Creates `path` (and any missing parents) with mode `0700` if it does not
/// already exist, matching Go's `os.MkdirAll(stateDir, 0o700)`.
fn ensure_state_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    builder.mode(0o700);
    builder.create(path)
}

/// The current process's uid, for [`PeerAuthInterceptor`]'s "the daemon's
/// own uid is always authorized" rule.
fn self_uid() -> u32 {
    nix::unistd::Uid::current().as_raw()
}

/// Best-effort spawn of `penguind watchdog` — the daemon-keeps-watchdog-
/// alive half of mutual supervision (see `crate::watchdog`'s module doc for
/// the full picture, including how the watchdog side guards against
/// accumulating a duplicate on every crash-restart cycle). Logs and returns
/// on any failure; never treated as fatal to daemon startup — an endpoint
/// agent that failed to start because its self-protection helper couldn't
/// spawn would be strictly worse than one that starts without it.
fn spawn_watchdog_peer() {
    let exe = match env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "failed to resolve own executable path; watchdog peer not started"
            );
            return;
        }
    };

    match std::process::Command::new(exe).arg("watchdog").spawn() {
        Ok(_child) => tracing::info!("spawned watchdog peer"),
        Err(err) => tracing::warn!(error = %err, "failed to spawn watchdog peer"),
    }
}

/// Resolves once SIGINT or SIGTERM arrives, driving
/// `Server::serve_with_incoming_shutdown`'s graceful drain.
async fn wait_for_shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    // Registering a signal handler only fails if the underlying syscall
    // setup fails, which would indicate a broken process environment this
    // daemon cannot run in regardless — panicking here is no worse than the
    // process being unable to shut down cleanly at all.
    let mut sigint = signal(SignalKind::interrupt()).expect("register SIGINT handler");
    let mut sigterm = signal(SignalKind::terminate()).expect("register SIGTERM handler");

    tokio::select! {
        _ = sigint.recv() => tracing::info!(signal = "SIGINT", "received shutdown signal"),
        _ = sigterm.recv() => tracing::info!(signal = "SIGTERM", "received shutdown signal"),
    }
}
