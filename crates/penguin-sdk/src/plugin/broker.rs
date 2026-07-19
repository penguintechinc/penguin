//! The plugin's side of the `GRPCBroker`: unlike `penguin-goplugin-host`
//! (which *calls* `StartStream` as a client), the plugin is the gRPC
//! *server* for this RPC — the host dials our main connection, then calls
//! `StartStream` on it to announce secondary connections such as
//! `HostService` on broker id 1. See `penguin-goplugin-host::broker` for the
//! host-side half of the same conversation; this module mirrors its pending-
//! table logic rather than importing it (see `mod.rs`'s doc comment on why).
//!
//! This is the piece of the protocol no Go-built plugin has ever
//! implemented (see `docs/PARITY.md` §1.10) — a correctly-written plugin
//! dialing id 1 here is what proves `HostService` callbacks actually work.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use hyper_util::rt::TokioIo;
use rustls::pki_types::{CertificateDer, ServerName};
use tokio::net::UnixStream;
use tokio::sync::{Mutex, mpsc};
use tokio_stream::Stream;
use tonic::Streaming;
use tonic::codegen::Service;
use tonic::transport::{Channel, Endpoint, Uri};
use tonic::{Request, Response, Status};

use penguin_proto::goplugin::ConnInfo;
use penguin_proto::goplugin::grpc_broker_server::GrpcBroker;

use crate::plugin::error::PluginError;
use crate::plugin::mtls::{self, PluginIdentity};

/// The HostService broker ID, hard-coded on both sides of go-plugin — see
/// `penguin-goplugin-host::broker::HOST_SERVICE_BROKER_ID`.
pub const HOST_SERVICE_BROKER_ID: u32 = 1;

/// How long [`BrokerState::dial`] waits for the host to announce a
/// connection before giving up. Matches go-plugin's own broker timeout, and
/// the value `penguin-goplugin-host` uses for the same wait in the other
/// direction.
const DIAL_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the TLS connect + `HostService` RPC prefetch step is allowed to
/// take once a socket address has been dialed. Bounded separately from
/// [`DIAL_TIMEOUT`] so a peer that accepts the TCP/unix connection but never
/// completes a TLS handshake (exactly what happens against the frozen Go
/// daemon, which serves this leg in plaintext) cannot hang plugin startup.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// A per-ID slot in the pending-connection table. Mirrors
/// `penguin-goplugin-host::broker::PendingSlot`.
struct PendingSlot {
    sender: mpsc::Sender<ConnInfo>,
    receiver: Option<mpsc::Receiver<ConnInfo>>,
}

fn new_pending_slot() -> PendingSlot {
    let (sender, receiver) = mpsc::channel(1);
    PendingSlot {
        sender,
        receiver: Some(receiver),
    }
}

type PendingTable = Arc<Mutex<HashMap<u32, PendingSlot>>>;

/// Shared state for the plugin's broker: the pending-connection table fed by
/// the host's inbound `ConnInfo` announcements, plus the outbound half of
/// our own `StartStream` response — kept open for the connection's life even
/// though this plugin never has anything of its own to announce, so the RPC
/// itself never prematurely completes (see [`BrokerService::start_stream`]).
pub struct BrokerState {
    outbound_rx: Mutex<Option<mpsc::Receiver<ConnInfo>>>,
    // Retained only to keep `outbound_rx`'s channel open; never sent on.
    _outbound_tx: mpsc::Sender<ConnInfo>,
    pending: PendingTable,
}

impl BrokerState {
    /// Builds a fresh, not-yet-served broker state.
    pub fn new() -> Arc<BrokerState> {
        let (outbound_tx, outbound_rx) = mpsc::channel::<ConnInfo>(1);
        Arc::new(BrokerState {
            outbound_rx: Mutex::new(Some(outbound_rx)),
            _outbound_tx: outbound_tx,
            pending: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Waits for the host to announce a connection for `id`, returning the
    /// unix socket path it published. Times out after [`DIAL_TIMEOUT`].
    pub async fn dial(&self, id: u32) -> Result<PathBuf, PluginError> {
        let mut receiver = {
            let mut pending = self.pending.lock().await;
            let slot = pending.entry(id).or_insert_with(new_pending_slot);
            slot.receiver.take().ok_or_else(|| {
                PluginError::Broker(format!("dial called more than once for broker id {id}"))
            })?
        };

        let received = tokio::time::timeout(DIAL_TIMEOUT, receiver.recv())
            .await
            .map_err(|_| {
                PluginError::Broker(format!(
                    "timed out waiting for connection info for broker id {id}"
                ))
            })?;
        let info = received.ok_or_else(|| {
            PluginError::Broker(format!(
                "broker stream closed before delivering connection info for id {id}"
            ))
        })?;

        if info.network != "unix" {
            return Err(PluginError::Broker(format!(
                "unsupported broker network {:?} for id {id} (only unix is supported)",
                info.network
            )));
        }
        Ok(PathBuf::from(info.address))
    }
}

/// Delivers each inbound `ConnInfo` to the pending slot for its `service_id`.
/// Runs for the life of the `StartStream` call; returns when the host closes
/// its side of the stream. Mirrors
/// `penguin-goplugin-host::broker::dispatch_inbound`.
async fn dispatch_inbound(mut inbound: Streaming<ConnInfo>, pending: PendingTable) {
    loop {
        let next = inbound.message().await;
        let info = match next {
            Ok(Some(info)) => info,
            Ok(None) => return,
            Err(status) => {
                tracing::debug!(error = %status, "broker stream from host ended");
                return;
            }
        };

        let sender = {
            let mut pending = pending.lock().await;
            let slot = pending
                .entry(info.service_id)
                .or_insert_with(new_pending_slot);
            slot.sender.clone()
        };
        let _ = sender.try_send(info);
    }
}

/// A [`Stream`] adapter over an `mpsc::Receiver`, used as the outbound half
/// of our `StartStream` response. Never yields an item in practice (we have
/// nothing to announce), but stays alive rather than ending immediately —
/// letting the response stream finish early would end the whole bidi RPC,
/// closing the inbound direction we still need for [`BrokerState::dial`].
struct OutboundConnInfo {
    receiver: mpsc::Receiver<ConnInfo>,
}

impl Stream for OutboundConnInfo {
    type Item = Result<ConnInfo, Status>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.receiver.poll_recv(cx) {
            Poll::Ready(Some(info)) => Poll::Ready(Some(Ok(info))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// The `GRPCBroker` service we register on our main gRPC server, wrapping
/// [`BrokerState`].
pub struct BrokerService(pub Arc<BrokerState>);

#[tonic::async_trait]
impl GrpcBroker for BrokerService {
    type StartStreamStream = Pin<Box<dyn Stream<Item = Result<ConnInfo, Status>> + Send + 'static>>;

    async fn start_stream(
        &self,
        request: Request<Streaming<ConnInfo>>,
    ) -> Result<Response<Self::StartStreamStream>, Status> {
        let inbound = request.into_inner();
        let pending = Arc::clone(&self.0.pending);
        tokio::spawn(dispatch_inbound(inbound, pending));

        let mut outbound_rx = self.0.outbound_rx.lock().await;
        let receiver = outbound_rx
            .take()
            .ok_or_else(|| Status::failed_precondition("StartStream called more than once"))?;
        let stream = OutboundConnInfo { receiver };
        Ok(Response::new(Box::pin(stream)))
    }
}

/// Dials `host_cert`-pinned TLS on `target` (the unix socket path
/// [`BrokerState::dial`] returned) and returns a connected channel, ready to
/// build a `HostServiceClient` (or any other client) from. We are the TLS
/// client here; the host is the TLS server — the inverse of the main
/// connection's roles.
pub async fn connect_host_channel(
    target: PathBuf,
    identity: &PluginIdentity,
    host_cert: CertificateDer<'static>,
) -> Result<Channel, PluginError> {
    let tls_config = mtls::build_client_tls_config(identity, host_cert)?;
    let connector = HostConnector { target, tls_config };
    Endpoint::from_static("http://host.invalid")
        .connect_with_connector(connector)
        .await
        .map_err(|e| PluginError::HostConnect(e.to_string()))
}

/// The bound this module needs from the pre-TLS unix stream. `tonic`'s
/// connector trait only requires `AsyncRead + AsyncWrite`; a named alias
/// keeps [`HostConnector::call`]'s return type readable.
trait AsyncIo: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> AsyncIo for T {}

/// A [`tower_service::Service`] (via `tonic::codegen::Service`) that dials
/// the fixed unix socket `target` and wraps the resulting stream in TLS.
/// Mirrors `penguin-goplugin-host::client::PluginConnector`.
struct HostConnector {
    target: PathBuf,
    tls_config: Arc<rustls::ClientConfig>,
}

impl Service<Uri> for HostConnector {
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
            let plain = UnixStream::connect(&target).await?;
            let connector = tokio_rustls::TlsConnector::from(tls_config);
            let server_name = ServerName::try_from(mtls::CERT_HOST)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
            let tls_stream = connector.connect(server_name, plain).await?;
            let boxed: Box<dyn AsyncIo> = Box::new(tls_stream);
            Ok(TokioIo::new(boxed))
        })
    }
}
