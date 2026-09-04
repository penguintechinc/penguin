//! The plugin-side entry point: [`serve`] is what a Rust plugin binary's
//! `main()` calls, mirroring the Go SDK's `sdk.Serve(module)`.
//!
//! ## Startup sequence (why the ordering is what it is)
//!
//! 1. [`crate::plugin::handshake::require_magic_cookie_or_exit`] — cheapest
//!    possible failure path, runs before a tokio runtime even exists.
//! 2. Generate our AutoMTLS identity and read the host's certificate from
//!    `PLUGIN_CLIENT_CERT`. Either failing is fatal: without them there is no
//!    way to complete the handshake at all.
//! 3. Create a short-path private socket directory and bind the unix
//!    listener. Binding (not accepting) is enough for the OS to start
//!    queuing the host's connection attempt.
//! 4. Print the handshake line and flush. From this instant the host may
//!    connect at any time.
//! 5. Start serving `ModuleService`, `grpc.health.v1.Health`,
//!    `GRPCController`, `GRPCStdio`, and `GRPCBroker` — and mark the health
//!    service `SERVING` immediately, matching go-plugin's own convention
//!    that a passing health check proves the gRPC server exists, not that
//!    the module behind it has finished initialising (see
//!    `penguin-goplugin-host::client`'s doc comment on the same point from
//!    the host's side). The single health check `PluginProcess::launch`
//!    performs would otherwise race step 6 below and could fail spuriously.
//! 6. Concurrently: dial the host's `HostService` over the broker's id=1
//!    leg and call [`Module::init`] with it — or, if that leg cannot be
//!    reached within a bounded timeout (as happens against the frozen Go
//!    daemon, which serves it in plaintext), degrade to
//!    [`crate::plugin::hostservices::NoopHostServices`] and call `init` with
//!    that instead. Either way, once this step finishes the readiness gate
//!    opens and `ModuleServiceImpl`'s `start`/`stop`/`status`/`health`/
//!    `dispatch` handlers — which were blocked waiting for it — proceed.
//!    This is the ordering guarantee: `Module::init` always completes before
//!    the module is usable, without delaying the health check that proves
//!    the plugin started at all.
//! 7. Wait for `GRPCController.Shutdown` (or the server future ending on its
//!    own) and exit.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rand::Rng as _;
use rustls::pki_types::CertificateDer;
use tokio::net::UnixListener;
use tokio::sync::{oneshot, watch};
use tonic::transport::Server;

use penguin_proto::goplugin;
use penguin_proto::sdk::v1::module_service_server::ModuleServiceServer;

use crate::host::HostServices;
use crate::module::Module;
use crate::plugin::broker::{self, BrokerService, BrokerState, HOST_SERVICE_BROKER_ID};
use crate::plugin::error::PluginError;
use crate::plugin::handshake;
use crate::plugin::hostservices::{NoopHostServices, RemoteHostServices};
use crate::plugin::mtls::{self, PluginIdentity};
use crate::plugin::services::{ControllerImpl, ModuleServiceImpl, StdioImpl};
use crate::plugin::tls_incoming::TlsIncoming;

/// Overall budget for "dial broker id 1, complete TLS, prefetch cached
/// `HostServices` state" before giving up and degrading to
/// [`NoopHostServices`]. Generous enough for a real host under load, short
/// enough that a plugin loaded by a host that never serves this leg (or
/// serves it in plaintext, like the frozen Go daemon) still becomes usable
/// quickly.
const HOST_SERVICES_TIMEOUT: Duration = Duration::from_secs(8);

/// The go-plugin server (plugin-process) entry point: builds a private
/// tokio runtime, serves `module` until the host tells us to stop, then
/// exits the process. Never returns.
///
/// Call this — and nothing else — from `main()`:
///
/// ```ignore
/// fn main() {
///     penguin_sdk::plugin::serve(Box::new(MyModule::default()));
/// }
/// ```
pub fn serve(module: Box<dyn Module>) -> ! {
    handshake::require_magic_cookie_or_exit();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build();
    let runtime = match runtime {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("penguin-sdk: failed to start tokio runtime: {e}");
            std::process::exit(1);
        }
    };

    let module: Arc<dyn Module> = Arc::from(module);
    runtime.block_on(serve_async(module));
    std::process::exit(0);
}

/// The async body of [`serve`], split out so the fatal-vs-recoverable error
/// handling described in the module doc comment reads as a straight line.
async fn serve_async(module: Arc<dyn Module>) {
    mtls::ensure_crypto_provider_installed();

    let identity = generate_identity_or_exit();
    let host_cert = read_host_cert_or_exit();
    let (listener, socket_path) = bind_listener_or_exit();

    let handshake_line = handshake::build_line(&socket_path, identity.cert_der.as_ref());
    if let Err(e) = handshake::print_and_flush(&handshake_line) {
        eprintln!("penguin-sdk: failed to print handshake line: {e}");
        std::process::exit(1);
    }

    let server_tls = match mtls::build_server_tls_config(&identity, host_cert.clone()) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("penguin-sdk: failed to build server TLS config: {e}");
            std::process::exit(1);
        }
    };
    let acceptor = tokio_rustls::TlsAcceptor::from(server_tls);
    let incoming = TlsIncoming { listener, acceptor };

    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    // go-plugin convention: SERVING reflects "the gRPC server is up", not
    // "the module finished initialising" — see the module doc comment.
    health_reporter
        .set_service_status("plugin", tonic_health::ServingStatus::Serving)
        .await;

    let (ready_tx, ready_rx) = watch::channel(false);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let broker_state = BrokerState::new();

    let module_service = ModuleServiceImpl {
        module: Arc::clone(&module),
        ready: ready_rx,
    };
    let controller_service = ControllerImpl::new(shutdown_tx);
    let broker_service = BrokerService(Arc::clone(&broker_state));

    let router = Server::builder()
        .add_service(ModuleServiceServer::new(module_service))
        .add_service(health_service)
        .add_service(goplugin::grpc_controller_server::GrpcControllerServer::new(
            controller_service,
        ))
        .add_service(goplugin::grpc_stdio_server::GrpcStdioServer::new(StdioImpl))
        .add_service(goplugin::grpc_broker_server::GrpcBrokerServer::new(
            broker_service,
        ));

    let shutdown_signal = async {
        let _ = shutdown_rx.await;
    };
    let server_handle =
        tokio::spawn(router.serve_with_incoming_shutdown(incoming, shutdown_signal));

    // Step 6: run concurrently with the server above so the health check in
    // step 5 is never delayed by it.
    tokio::spawn(run_init_sequence(
        module,
        broker_state,
        identity,
        host_cert,
        ready_tx,
    ));

    let _ = server_handle.await;
}

/// Dials the host's `HostService` (or degrades to a no-op), calls
/// [`Module::init`], and opens the readiness gate every `ModuleServiceImpl`
/// handler but `info`/`commands`/`config_schema` is waiting on.
async fn run_init_sequence(
    module: Arc<dyn Module>,
    broker_state: Arc<BrokerState>,
    identity: PluginIdentity,
    host_cert: CertificateDer<'static>,
    ready_tx: watch::Sender<bool>,
) {
    let host_services = build_host_services_or_noop(broker_state, &identity, host_cert).await;
    if let Err(e) = module.init(host_services).await {
        // A module whose init fails must still become "usable" — it is
        // responsible for reporting its own broken state through
        // `status()`/`health()`; the alternative (never opening the
        // readiness gate) would hang every RPC forever instead.
        tracing::warn!(error = %e, "Module::init returned an error; module will still be served");
    }
    let _ = ready_tx.send(true);
}

/// Attempts the broker id=1 dial, TLS connect, and `HostServices` prefetch
/// within [`HOST_SERVICES_TIMEOUT`]; on any failure, logs why and returns
/// [`NoopHostServices`] instead. This is the graceful-degradation path that
/// makes loading under the frozen Go daemon possible: that host serves this
/// leg in plaintext, so our TLS ClientHello is rejected — never a crash, and
/// never a hang past the timeout.
async fn build_host_services_or_noop(
    broker_state: Arc<BrokerState>,
    identity: &PluginIdentity,
    host_cert: CertificateDer<'static>,
) -> Arc<dyn HostServices> {
    let attempt = connect_host_services(broker_state, identity, host_cert);
    match tokio::time::timeout(HOST_SERVICES_TIMEOUT, attempt).await {
        Ok(Ok(host_services)) => host_services,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "HostService broker leg unavailable; degrading to no-op HostServices");
            Arc::new(NoopHostServices::new())
        }
        Err(_) => {
            tracing::warn!(
                "HostService broker leg dial timed out; degrading to no-op HostServices"
            );
            Arc::new(NoopHostServices::new())
        }
    }
}

async fn connect_host_services(
    broker_state: Arc<BrokerState>,
    identity: &PluginIdentity,
    host_cert: CertificateDer<'static>,
) -> Result<Arc<dyn HostServices>, PluginError> {
    let socket_path = broker_state.dial(HOST_SERVICE_BROKER_ID).await?;
    let connect = broker::connect_host_channel(socket_path, identity, host_cert);
    let channel = tokio::time::timeout(broker::CONNECT_TIMEOUT, connect)
        .await
        .map_err(|_| {
            PluginError::HostConnect("TLS connect to HostService timed out".to_string())
        })??;
    let remote = RemoteHostServices::connect(channel).await;
    Ok(Arc::new(remote))
}

/// Generates the plugin's AutoMTLS identity, exiting the process on failure
/// — there is no degraded mode for "cannot generate our own certificate".
fn generate_identity_or_exit() -> PluginIdentity {
    match mtls::generate_plugin_identity() {
        Ok(identity) => identity,
        Err(e) => {
            eprintln!("penguin-sdk: failed to generate plugin identity: {e}");
            std::process::exit(1);
        }
    }
}

/// Reads and parses the host's certificate from `PLUGIN_CLIENT_CERT`,
/// exiting the process on failure — without it neither TLS role can be
/// configured.
fn read_host_cert_or_exit() -> CertificateDer<'static> {
    match mtls::read_host_cert_from_env() {
        Ok(cert) => cert,
        Err(e) => {
            eprintln!("penguin-sdk: {e}");
            std::process::exit(1);
        }
    }
}

/// Creates a private (mode 0700) temp directory and binds a unix listener
/// inside it at a deliberately short path — the `sun_path` limit is well
/// under 103 bytes on the platforms this SDK targets, so the directory name
/// itself must stay short too.
fn bind_listener_or_exit() -> (UnixListener, PathBuf) {
    match make_socket_dir() {
        Ok(dir) => {
            let socket_path = dir.join("s");
            match UnixListener::bind(&socket_path) {
                Ok(listener) => (listener, socket_path),
                Err(e) => {
                    eprintln!("penguin-sdk: failed to bind plugin listener: {e}");
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("penguin-sdk: {e}");
            std::process::exit(1);
        }
    }
}

/// Creates a private, short-path temp directory under the platform temp dir
/// (mode 0700) to hold this plugin's listening socket.
fn make_socket_dir() -> Result<PathBuf, PluginError> {
    let base = std::env::temp_dir();
    let pid = std::process::id();
    let mut last_error: Option<std::io::Error> = None;
    for _attempt in 0..5 {
        let suffix: u16 = rand::rng().random();
        let dir = base.join(format!("pgp-{pid:x}-{suffix:x}"));
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                set_private_permissions(&dir)?;
                return Ok(dir);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                last_error = Some(e);
                continue;
            }
            Err(e) => return Err(PluginError::Listener(e.to_string())),
        }
    }
    let message = last_error.map(|e| e.to_string()).unwrap_or_default();
    Err(PluginError::Listener(format!(
        "could not create a unique socket directory under {}: {message}",
        base.display()
    )))
}

/// Restricts `dir` to owner-only access (mode 0700), matching go-plugin's
/// own `ioutil.TempDir` + `os.Chmod` convention for the socket directory.
fn set_private_permissions(dir: &std::path::Path) -> Result<(), PluginError> {
    use std::os::unix::fs::PermissionsExt as _;
    let permissions = std::fs::Permissions::from_mode(0o700);
    std::fs::set_permissions(dir, permissions).map_err(|e| PluginError::Listener(e.to_string()))
}
