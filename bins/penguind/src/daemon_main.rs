//! Unix daemon startup: config, telemetry, the single-instance lock, the
//! supervisor, and the gRPC server over the control socket. Ported from the
//! combined `go-client/cmd/penguind/main.go` + `service.go` startup path.
//!
//! Windows service-manager integration (`kardianos/service` in Go) is out of
//! scope here — see [`super::main`]'s `service` short-circuit — so this
//! module only ever runs the daemon in the Go reference's "interactive mode"
//! shape: serve directly, wait for a signal, shut down.

use std::collections::BTreeMap;
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
use penguin_daemon::host::{DaemonHostFactory, HostFactory};
use penguin_daemon::lock::{self, LockError};
use penguin_daemon::logring::LogRing;
use penguin_daemon::service::DaemonService;
use penguin_daemon::supervisor::{Supervisor, SupervisorConfig};
use penguin_ipc::groups_unix::SystemGroups;
use penguin_ipc::listen_unix::{self, ListenerConfig, PeerAuthInterceptor};
use penguin_ipc::{GroupResolver, IpcError};
use penguin_proto::daemon::v1::daemon_server::DaemonServer;
use penguin_sdk::{EventSink, LicenseChecker, SecretStore};
use penguin_telemetry::{Telemetry, TelemetryError};

use crate::stubs::{FreeTierLicenseChecker, InMemorySecretStore};
use crate::{VERSION, logging};

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

    let broker = Arc::new(EventBroker::new(EVENT_BROKER_CAPACITY));
    let events: Arc<dyn EventSink> = broker.clone();
    let secrets: Arc<dyn SecretStore> = Arc::new(InMemorySecretStore::new());
    let license: Arc<dyn LicenseChecker> = Arc::new(FreeTierLicenseChecker);
    let host_factory: Arc<dyn HostFactory> = Arc::new(DaemonHostFactory::new(
        telemetry,
        Arc::new(config_store),
        secrets,
        license,
        events,
        args.state_dir.clone(),
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
        registry: BTreeMap::new(), // no built-in modules until M5/M6
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

    let daemon_service = DaemonService::new(supervisor.clone(), broker, logs, VERSION, None);

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
    let health_svc = InterceptedService::new(health_service, peer_auth);

    let serve_result = Server::builder()
        .add_service(daemon_svc)
        .add_service(health_svc)
        .serve_with_incoming_shutdown(incoming, wait_for_shutdown_signal())
        .await;

    tracing::info!("shutting down");
    supervisor.shutdown().await;
    let _ = std::fs::remove_file(&daemon_cfg.socket_path);

    serve_result.map_err(DaemonBinError::Serve)
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
