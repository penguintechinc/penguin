//! Orchestrates the whole go-plugin host lifecycle: launch the child
//! process, complete the AutoMTLS handshake, connect the gRPC channel, wire
//! up the broker and stdio legs, verify health, and shut down cleanly.
//!
//! This is the one module in the crate allowed to touch a live process or
//! socket; `handshake.rs`, `mtls.rs`, and `adapter.rs` stay pure so their
//! decision logic is exhaustively testable without either.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use hyper_util::rt::TokioIo;
use rustls::pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};
use tokio::net::{TcpStream, UnixStream};
use tokio::process::{Child, Command};
use tokio::time::timeout;
use tonic::codegen::Service;
use tonic::transport::{Channel, Endpoint, Uri};

use penguin_sdk::Module;

use crate::adapter::ModuleAdapter;
use crate::broker::{self, Broker, HOST_SERVICE_BROKER_ID};
use crate::controller::Controller;
use crate::error::HostError;
use crate::handshake::Handshake;
use crate::mtls::{
    self, CERT_HOST, HostIdentity, PinnedClientCertVerifier, PinnedServerCertVerifier,
};
use crate::stdio::StdioClient;

/// go-plugin's magic-cookie env var this host sets on every launched child.
const MAGIC_COOKIE_KEY: &str = "PENGUIN_PLUGIN";
/// The magic-cookie value plugins must echo agreement with to prove they are
/// a real `penguin-sdk` plugin and not an unrelated executable.
const MAGIC_COOKIE_VALUE: &str = "penguin-sdk-v1";
const MIN_PORT: &str = "10000";
const MAX_PORT: &str = "25000";
const PROTOCOL_VERSIONS: &str = "1";

/// How long to wait for the handshake line before giving up.
const START_TIMEOUT: Duration = Duration::from_secs(60);
/// How long to wait for a graceful exit after `GRPCController.Shutdown`
/// before escalating to SIGKILL.
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// A launched, connected plugin: the running child process plus every
/// handle needed to use and later shut it down.
pub struct PluginProcess {
    child: Child,
    channel: Channel,
    controller: Controller,
    /// Kept alive for the connection's life — dropping it closes the
    /// broker's `StartStream`, which is step 1 of [`PluginProcess::shutdown`].
    broker: Broker,
    /// The parsed handshake line the plugin printed at startup. Nothing in
    /// the running connection needs it again once the channel is up; kept
    /// for diagnostics and so callers (including tests) can assert on the
    /// raw wire contract independently of whether the subsequent TLS
    /// handshake and health check also succeeded.
    handshake: Handshake,
}

impl PluginProcess {
    /// Launches `binary_path` as a go-plugin plugin: spawns the process,
    /// completes the AutoMTLS handshake, connects the gRPC channel, starts
    /// the broker and stdio legs, and verifies the health service.
    ///
    /// `socket_dir` is where broker-announced unix sockets are created; the
    /// caller owns its lifecycle the same way go-plugin's
    /// `UnixSocketConfig.TempDir` does. `host_routes`, if given, is served
    /// TLS-wrapped over the broker's id=1 leg — see the crate-level doc
    /// comment on why this host, unlike the frozen Go one, can do this
    /// correctly; pass `None` when nothing needs to receive plugin callbacks.
    pub async fn launch(
        binary_path: &Path,
        socket_dir: &Path,
        host_routes: Option<tonic::service::Routes>,
    ) -> Result<PluginProcess, HostError> {
        mtls::ensure_crypto_provider_installed();
        let identity = mtls::generate_host_identity()?;

        let mut child = spawn_plugin(binary_path, &identity)?;
        let stdout = child.stdout.take().expect("stdout was piped at spawn");
        let handshake = match read_handshake_line(stdout).await {
            Ok(handshake) => handshake,
            Err(err) => {
                // The handshake never arrived; the child is either hung or
                // already dead. Either way it must not leak.
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(err);
            }
        };

        let peer_cert_der = handshake.server_cert_der.clone().ok_or_else(|| {
            HostError::Tls("plugin did not present an AutoMTLS certificate".to_string())
        })?;
        let peer_cert = CertificateDer::from(peer_cert_der);

        let target = Target::parse(&handshake.network, &handshake.address)?;
        let client_tls = build_client_tls_config(&identity, peer_cert.clone())?;

        let channel = connect_channel(target, client_tls).await?;

        // Both the broker and stdio legs are opened immediately after the
        // channel is up and kept running for the connection's life — never
        // deferred until first use. Neither is awaited here: `StreamStdio`
        // is server-streaming for the identical reason `Broker::connect`
        // isn't awaited (see its doc comment) — go-plugin's
        // `grpcStdioServer.StreamStdio` handler (`grpc_stdio.go`) never
        // sends response headers until the plugin actually writes to
        // stdout/stderr, which most plugins never do after startup. The Go
        // host gets away with awaiting its own `newGRPCStdioClient` call
        // synchronously only because grpc-go's client-side stub returns
        // once the request is queued locally, without waiting on the peer;
        // tonic's does not, so this must be spawned instead.
        let broker = Broker::connect(channel.clone()).await;
        let stdio_channel = channel.clone();
        tokio::spawn(async move {
            match StdioClient::connect(stdio_channel).await {
                Ok(Some(stdio)) => stdio.drain().await,
                Ok(None) => {}
                Err(status) => {
                    tracing::debug!(error = %status, "GRPCStdio.StreamStdio failed");
                }
            }
        });

        if let Some(routes) = host_routes {
            let server_tls = build_server_tls_config(&identity, peer_cert)?;
            let listener = broker.accept(HOST_SERVICE_BROKER_ID, socket_dir).await?;
            let acceptor = tokio_rustls::TlsAcceptor::from(server_tls);
            tokio::spawn(async move {
                if let Err(e) = broker::serve_tls(listener, acceptor, routes).await {
                    tracing::warn!(error = %e, "HostService broker leg exited");
                }
            });
        }

        // go-plugin's Go host sets SERVING on this check unconditionally
        // before it starts serving anything else, so a successful check only
        // proves the plugin's gRPC server exists — not that the module
        // behind it has finished initialising.
        wait_for_health(&channel).await?;

        let controller = Controller::new(channel.clone());

        Ok(PluginProcess {
            child,
            channel,
            controller,
            broker,
            handshake,
        })
    }

    /// Dispenses the plugin's `ModuleService` as a boxed [`Module`], the
    /// same type the daemon supervisor uses for built-in modules.
    pub async fn dispense(&self) -> Result<Box<dyn Module>, penguin_sdk::ModuleError> {
        let adapter = ModuleAdapter::connect(self.channel.clone()).await?;
        Ok(Box::new(adapter))
    }

    /// Returns the parsed handshake line the plugin printed at startup.
    pub fn handshake(&self) -> &Handshake {
        &self.handshake
    }

    /// Returns the plugin child process's OS PID. `None` only if the child
    /// has already been reaped, which never happens before [`PluginProcess::shutdown`].
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Shuts the plugin down: close the broker stream, call
    /// `GRPCController.Shutdown` and wait for its response, drop the
    /// channel, wait up to 2s for the child to exit gracefully, then
    /// SIGKILL.
    ///
    /// Never sends SIGTERM/SIGINT and never closes stdin as a shutdown
    /// signal: the plugin installs a handler that catches and permanently
    /// ignores SIGINT and never reads stdin, so either would be silently
    /// swallowed rather than triggering an exit.
    pub async fn shutdown(mut self) -> Result<(), HostError> {
        drop(self.broker);

        if let Err(e) = self.controller.shutdown().await {
            tracing::debug!(error = %e, "GRPCController.Shutdown failed; will still force-kill");
        }
        drop(self.channel);

        if timeout(GRACEFUL_SHUTDOWN_TIMEOUT, self.child.wait())
            .await
            .is_ok()
        {
            return Ok(());
        }

        self.child
            .start_kill()
            .map_err(|e| HostError::Broker(format!("SIGKILL failed: {e}")))?;
        let _ = self.child.wait().await;
        Ok(())
    }
}

/// Spawns the plugin binary with the launch environment go-plugin requires.
/// The parent's own environment is inherited (never cleared) — only these
/// variables are added on top of it.
fn spawn_plugin(binary_path: &Path, identity: &HostIdentity) -> Result<Child, HostError> {
    let mut command = Command::new(binary_path);
    command
        .env(MAGIC_COOKIE_KEY, MAGIC_COOKIE_VALUE)
        .env("PLUGIN_MIN_PORT", MIN_PORT)
        .env("PLUGIN_MAX_PORT", MAX_PORT)
        .env("PLUGIN_PROTOCOL_VERSIONS", PROTOCOL_VERSIONS)
        .env("PLUGIN_CLIENT_CERT", &identity.cert_pem)
        // Must stay absent: setting this would opt the plugin into
        // multiplexed broker mode, which this host never implements.
        .env_remove("PLUGIN_MULTIPLEX_GRPC")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    command.spawn().map_err(HostError::Spawn)
}

/// Reads the single handshake line off the child's stdout.
///
/// A 60s timeout guards a plugin that never prints anything. A plugin that
/// exits immediately closes its stdout pipe, which unblocks the read with
/// EOF (`Ok(None)`) well before the timeout — that is how this reports "the
/// process exited before handshake" distinctly from "the process is hung".
async fn read_handshake_line(stdout: tokio::process::ChildStdout) -> Result<Handshake, HostError> {
    let mut lines = BufReader::new(stdout).lines();
    let read = timeout(START_TIMEOUT, lines.next_line())
        .await
        .map_err(|_| HostError::HandshakeTimeout)?;
    let line = read
        .map_err(HostError::Spawn)?
        .ok_or(HostError::ExitedBeforeHandshake)?;
    Handshake::parse(line.trim()).map_err(HostError::from)
}

/// Where the plugin's gRPC server is listening, parsed from the handshake's
/// network/address fields.
#[derive(Clone)]
enum Target {
    Unix(PathBuf),
    Tcp(String),
}

impl Target {
    fn parse(network: &str, address: &str) -> Result<Target, HostError> {
        match network {
            "unix" => Ok(Target::Unix(PathBuf::from(address))),
            "tcp" => Ok(Target::Tcp(address.to_string())),
            other => Err(HostError::Connect(format!(
                "unsupported handshake network {other:?}"
            ))),
        }
    }
}

/// The bound this crate needs from any plain (pre-TLS) transport: unix
/// sockets and TCP sockets are otherwise unrelated types, so `dial_plain`
/// returns a boxed trait object over this instead of duplicating the dial
/// and TLS-wrap logic per transport.
trait AsyncIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncIo for T {}

/// Opens the plain (pre-TLS) connection to `target`.
async fn dial_plain(target: &Target) -> std::io::Result<Box<dyn AsyncIo>> {
    match target {
        Target::Unix(path) => {
            let stream = UnixStream::connect(path).await?;
            Ok(Box::new(stream))
        }
        Target::Tcp(addr) => {
            let stream = TcpStream::connect(addr).await?;
            Ok(Box::new(stream))
        }
    }
}

/// A [`tower_service::Service`] (via `tonic::codegen::Service`) that dials
/// `target` and wraps the resulting stream in TLS, for use with
/// [`Endpoint::connect_with_connector`]. tonic's connector bound requires
/// [`hyper::rt::Read`]/[`hyper::rt::Write`], not `tokio`'s traits, hence the
/// [`TokioIo`] wrapper on the response.
struct PluginConnector {
    target: Target,
    tls_config: Arc<rustls::ClientConfig>,
}

impl Service<Uri> for PluginConnector {
    type Response = TokioIo<Box<dyn AsyncIo>>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        let target = self.target.clone();
        let tls_config = Arc::clone(&self.tls_config);
        Box::pin(async move {
            let plain = dial_plain(&target).await?;
            let connector = tokio_rustls::TlsConnector::from(tls_config);
            let server_name = ServerName::try_from(CERT_HOST)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
            let tls_stream = connector.connect(server_name, plain).await?;
            let boxed: Box<dyn AsyncIo> = Box::new(tls_stream);
            Ok(TokioIo::new(boxed))
        })
    }
}

/// Connects the main gRPC channel to the plugin over `target`, TLS-wrapped
/// with `tls_config`. The URI passed to [`Endpoint::from_static`] is never
/// actually dialed — [`PluginConnector`] ignores it and dials `target`
/// directly — it exists only because `Endpoint` requires a well-formed one.
async fn connect_channel(
    target: Target,
    tls_config: Arc<rustls::ClientConfig>,
) -> Result<Channel, HostError> {
    let connector = PluginConnector { target, tls_config };
    Endpoint::from_static("http://plugin.invalid")
        .connect_with_connector(connector)
        .await
        .map_err(|e| HostError::Connect(e.to_string()))
}

/// Builds the `rustls::ClientConfig` for the main connection: this host is
/// the TLS client, presenting its own identity (the plugin requires and
/// verifies a client certificate) and pinning the plugin's handshake leaf as
/// the only acceptable server certificate.
///
/// Must advertise `h2` via ALPN: grpc-go v1.67+ enforces ALPN negotiation by
/// default (`GRPC_ENFORCE_ALPN_ENABLED` defaults to true — see
/// `google.golang.org/grpc/credentials.tlsCreds.ServerHandshake`) and closes
/// the connection immediately after any TLS handshake that didn't negotiate
/// a protocol. Without this, the raw TLS handshake still succeeds — the
/// plugin only rejects it one layer up, inside its own gRPC server — so the
/// failure surfaces downstream as a broken pipe on the first RPC rather than
/// as a TLS error, which is what made it non-obvious.
fn build_client_tls_config(
    identity: &HostIdentity,
    peer_cert: CertificateDer<'static>,
) -> Result<Arc<rustls::ClientConfig>, HostError> {
    let verifier = Arc::new(PinnedServerCertVerifier::new(peer_cert));
    let cert_chain = vec![identity.cert_der.clone()];
    let mut config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(cert_chain, identity.private_key())
        .map_err(|e| HostError::Tls(e.to_string()))?;
    config.alpn_protocols = vec![b"h2".to_vec()];
    Ok(Arc::new(config))
}

/// Builds the `rustls::ServerConfig` for the broker's id=1 leg: this host is
/// the TLS server there, presenting its own identity and pinning the
/// plugin's handshake leaf as the only acceptable client certificate. See
/// the crate-level doc comment for why this leg is TLS-wrapped at all,
/// unlike the frozen Go host's plaintext handling of the same leg.
///
/// Also advertises `h2` via ALPN, matching [`build_client_tls_config`]: a
/// correctly-written plugin dialing in as a grpc-go client enforces the same
/// ALPN requirement on its side of the handshake.
fn build_server_tls_config(
    identity: &HostIdentity,
    peer_cert: CertificateDer<'static>,
) -> Result<Arc<rustls::ServerConfig>, HostError> {
    let verifier = Arc::new(PinnedClientCertVerifier::new(peer_cert));
    let cert_chain = vec![identity.cert_der.clone()];
    let mut config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(cert_chain, identity.private_key())
        .map_err(|e| HostError::Tls(e.to_string()))?;
    config.alpn_protocols = vec![b"h2".to_vec()];
    Ok(Arc::new(config))
}

/// Checks `grpc.health.v1` once. See [`PluginProcess::launch`]'s doc comment
/// on why this is a single check rather than a poll loop.
async fn wait_for_health(channel: &Channel) -> Result<(), HostError> {
    let mut client = tonic_health::pb::health_client::HealthClient::new(channel.clone());
    let response = client
        .check(tonic_health::pb::HealthCheckRequest {
            service: "plugin".to_string(),
        })
        .await
        .map_err(|status| HostError::Health(status.to_string()))?
        .into_inner();

    let status = tonic_health::pb::health_check_response::ServingStatus::try_from(response.status)
        .unwrap_or(tonic_health::pb::health_check_response::ServingStatus::Unknown);
    if status == tonic_health::pb::health_check_response::ServingStatus::Serving {
        Ok(())
    } else {
        Err(HostError::Health(format!(
            "plugin health check returned {status:?} instead of SERVING"
        )))
    }
}
