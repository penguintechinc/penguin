//! The gRPC broker: hands out secondary unix-socket connections between host
//! and plugin by numeric ID, riding a single bidirectional `GRPCBroker`
//! stream opened once at connect time and held open for the connection's
//! life.
//!
//! This host never sets `PLUGIN_MULTIPLEX_GRPC`, so it always stays in
//! go-plugin's original, non-multiplexed broker mode: [`Broker::accept`]
//! binds a brand-new listener per ID and announces it over the stream;
//! [`Broker::dial`] waits for the peer's announcement and then opens a fresh
//! connection. The `Knock`/`Ack` fields on [`ConnInfo`] exist only for the
//! newer multiplexed mode and are never read or set here.

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, mpsc};
use tokio_stream::Stream;
use tonic::Streaming;
use tonic::service::Routes;
use tonic::transport::{Channel, Server};

use penguin_proto::goplugin::ConnInfo;
use penguin_proto::goplugin::grpc_broker_client::GrpcBrokerClient;

use crate::error::HostError;

/// The HostService broker ID, hard-coded on both sides of go-plugin. See the
/// crate-level doc comment for why this host — unlike the frozen Go one —
/// serves this leg properly, TLS-wrapped.
pub const HOST_SERVICE_BROKER_ID: u32 = 1;

/// How long [`Broker::dial`] waits for the peer to announce a connection
/// before giving up, matching go-plugin's own `5 * time.Second` broker
/// timeout.
const DIAL_TIMEOUT: Duration = Duration::from_secs(5);

/// A stream adapter over an `mpsc::Receiver`, used as the outbound half of
/// the `GRPCBroker.StartStream` bidirectional call. A hand-rolled `poll_next`
/// avoids pulling in `tokio-stream`'s `sync` feature for a single wrapper
/// type the crate would otherwise not need.
struct OutboundConnInfo {
    receiver: mpsc::Receiver<ConnInfo>,
}

impl Stream for OutboundConnInfo {
    type Item = ConnInfo;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<ConnInfo>> {
        self.receiver.poll_recv(cx)
    }
}

/// A per-ID slot in the broker's pending-connection table: the sender side
/// feeds `ConnInfo` announcements in as they arrive off the stream; the
/// receiver side is claimed by whichever [`Broker::dial`] call asks for that
/// ID first.
///
/// Mirrors go-plugin's `gRPCBrokerPending`, which is likewise created lazily
/// by whichever of the receive loop or `Dial` reaches a given ID first, so
/// an announcement that arrives before anyone dials it is not lost.
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

/// Brokers secondary unix-socket connections between host and plugin by
/// numeric ID, over the single `GRPCBroker.StartStream` stream opened by
/// [`Broker::connect`].
pub struct Broker {
    outbound: mpsc::Sender<ConnInfo>,
    pending: PendingTable,
}

impl Broker {
    /// Opens the broker's bidirectional stream and starts the background
    /// task that dispatches inbound `ConnInfo` announcements to whichever
    /// [`Broker::dial`] call is waiting for that ID. Returns immediately —
    /// the stream is kept alive for the lifetime of the returned [`Broker`].
    ///
    /// The `GRPCBroker.StartStream` call itself is spawned rather than
    /// awaited here, matching go-plugin's own Go host: `grpc_client.go`
    /// launches it with `go func() { _ = brokerGRPCClient.StartStream() }()`
    /// and never waits on it either. This is not a stylistic choice on
    /// either side — it is load-bearing. The plugin's `StartStream` handler
    /// (`grpc_broker.go`'s `gRPCBrokerServer.StartStream`) never proactively
    /// sends response headers; grpc-go defers them until the handler has an
    /// actual message to send, which for a plugin whose own broker never
    /// calls `Accept`/`Dial` (see the crate-level doc comment on the dead
    /// broker id=1 hook) is never. tonic's streaming client call blocks on
    /// `.await` until those headers arrive — unlike grpc-go's client, which
    /// returns a usable stream handle without any network round trip — so
    /// awaiting it here hangs forever against exactly the plugins this host
    /// must support. `accept`/`dial` below only need `outbound`/`pending`,
    /// both created before the spawn, so they work immediately regardless of
    /// whether the spawned call ever resolves.
    pub async fn connect(channel: Channel) -> Broker {
        let (outbound_tx, outbound_rx) = mpsc::channel::<ConnInfo>(8);
        let pending: PendingTable = Arc::new(Mutex::new(HashMap::new()));

        let mut client = GrpcBrokerClient::new(channel);
        let request = OutboundConnInfo {
            receiver: outbound_rx,
        };
        let dispatch_pending = Arc::clone(&pending);
        tokio::spawn(async move {
            let response = match client.start_stream(request).await {
                Ok(response) => response,
                Err(status) => {
                    tracing::debug!(error = %status, "GRPCBroker.StartStream failed");
                    return;
                }
            };
            dispatch_inbound(response.into_inner(), dispatch_pending).await;
        });

        Broker {
            outbound: outbound_tx,
            pending,
        }
    }

    /// Binds a new unix listener under `id` inside `socket_dir` and
    /// announces its path to the plugin over the broker stream. Returns the
    /// listener without accepting on it — serving happens after, driven by
    /// the caller (see [`serve_tls`] for the broker id=1/HostService case).
    pub async fn accept(&self, id: u32, socket_dir: &Path) -> Result<UnixListener, HostError> {
        let path = socket_dir.join(format!("{id}.sock"));
        // A stale socket file left behind by a crashed previous run must not
        // block the bind.
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).map_err(|e| {
            HostError::Broker(format!("bind broker socket {}: {e}", path.display()))
        })?;

        let info = ConnInfo {
            service_id: id,
            network: "unix".to_string(),
            address: path.display().to_string(),
            knock: None,
        };
        self.outbound
            .send(info)
            .await
            .map_err(|_| HostError::Broker("broker stream closed".to_string()))?;

        Ok(listener)
    }

    /// Waits for the plugin to announce a connection for `id`, then dials
    /// it. Times out after [`DIAL_TIMEOUT`], matching go-plugin.
    pub async fn dial(&self, id: u32) -> Result<UnixStream, HostError> {
        let mut receiver = {
            let mut pending = self.pending.lock().await;
            let slot = pending.entry(id).or_insert_with(new_pending_slot);
            slot.receiver.take().ok_or_else(|| {
                HostError::Broker(format!("dial called more than once for broker id {id}"))
            })?
        };

        let info = tokio::time::timeout(DIAL_TIMEOUT, receiver.recv())
            .await
            .map_err(|_| {
                HostError::Broker(format!(
                    "timed out waiting for connection info for broker id {id}"
                ))
            })?
            .ok_or_else(|| {
                HostError::Broker(format!(
                    "broker stream closed before delivering connection info for id {id}"
                ))
            })?;

        UnixStream::connect(&info.address)
            .await
            .map_err(|e| HostError::Broker(format!("dial broker connection {}: {e}", info.address)))
    }
}

/// Delivers each inbound `ConnInfo` to the pending slot for its `service_id`,
/// creating the slot if the receive loop reaches that ID before any `dial`
/// call does. Runs for the life of the broker stream; returns when the
/// stream ends.
async fn dispatch_inbound(mut inbound: Streaming<ConnInfo>, pending: PendingTable) {
    loop {
        let next = inbound.message().await;
        let info = match next {
            Ok(Some(info)) => info,
            Ok(None) => return,
            Err(status) => {
                tracing::debug!(error = %status, "broker stream ended");
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
        // The channel has capacity 1 and Accept is documented as not being
        // called twice for the same ID concurrently, so this never blocks in
        // practice; a full channel just means a duplicate announcement,
        // which is dropped rather than stalling the receive loop.
        let _ = sender.try_send(info);
    }
}

/// Serves `routes` (built by the caller via `tonic::service::Routes::new`)
/// over every connection accepted on `listener`, wrapping each one in TLS
/// with the host acting as the TLS server.
///
/// This is the deliberate fix described in the crate-level doc comment: the
/// frozen Go host serves the id=1/HostService leg in plaintext because it
/// bypasses go-plugin's own `AcceptAndServe`. This host does not have that
/// bug — every broker leg it serves is TLS-wrapped, pinning the plugin's
/// handshake leaf as the only acceptable client certificate.
pub async fn serve_tls(
    listener: UnixListener,
    tls_acceptor: tokio_rustls::TlsAcceptor,
    routes: Routes,
) -> Result<(), HostError> {
    let incoming = TlsIncoming {
        listener,
        acceptor: tls_acceptor,
    };
    Server::builder()
        .add_routes(routes)
        .serve_with_incoming(incoming)
        .await
        .map_err(|e| HostError::Broker(format!("broker TLS server error: {e}")))
}

/// A [`Stream`] of TLS-wrapped connections accepted from a [`UnixListener`],
/// suitable for [`tonic::transport::Server::serve_with_incoming`].
struct TlsIncoming {
    listener: UnixListener,
    acceptor: tokio_rustls::TlsAcceptor,
}

impl Stream for TlsIncoming {
    type Item = std::io::Result<TlsUnixStream>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.listener.poll_accept(cx) {
            // The TLS handshake is asynchronous, so it cannot be finished
            // here; TlsUnixStream defers it to its own first poll instead of
            // blocking this listener's accept loop on it.
            Poll::Ready(Ok((stream, _addr))) => {
                let acceptor = this.acceptor.clone();
                Poll::Ready(Some(Ok(TlsUnixStream::new(acceptor, stream))))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// A unix-socket connection mid- or post-TLS-handshake.
///
/// tonic's `serve_with_incoming` requires each stream item to already
/// implement [`tokio::io::AsyncRead`]/[`AsyncWrite`], but the TLS handshake
/// is itself asynchronous. This type starts as a pending handshake future
/// and switches to the established [`tokio_rustls::server::TlsStream`] the
/// first time it is polled to completion, so accepting a new connection
/// never blocks [`TlsIncoming`]'s poll loop on a full TLS handshake.
///
/// Both variants are `Unpin` (a boxed future and a plain tokio stream), so
/// every method below works with an ordinary `&mut self` under the hood
/// rather than juggling `Pin::set`.
enum TlsUnixStream {
    Handshaking(Pin<Box<tokio_rustls::Accept<UnixStream>>>),
    // Boxed so this variant doesn't dwarf `Handshaking`'s: a `TlsStream`
    // embeds rustls' connection state directly, over a kilobyte, versus a
    // bare pointer for the still-pending future above.
    Established(Box<tokio_rustls::server::TlsStream<UnixStream>>),
}

impl TlsUnixStream {
    fn new(acceptor: tokio_rustls::TlsAcceptor, stream: UnixStream) -> TlsUnixStream {
        TlsUnixStream::Handshaking(Box::pin(acceptor.accept(stream)))
    }

    /// Drives a pending handshake to completion in place, transitioning to
    /// `Established` on success. A no-op once already `Established`.
    fn poll_established(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        loop {
            match self {
                TlsUnixStream::Established(_) => return Poll::Ready(Ok(())),
                TlsUnixStream::Handshaking(accept) => match accept.as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Ready(Ok(stream)) => {
                        *self = TlsUnixStream::Established(Box::new(stream));
                    }
                },
            }
        }
    }
}

impl tokio::io::AsyncRead for TlsUnixStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        match this.poll_established(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {}
        }
        match this {
            TlsUnixStream::Established(stream) => Pin::new(stream).poll_read(cx, buf),
            TlsUnixStream::Handshaking(_) => {
                unreachable!("poll_established guarantees Established")
            }
        }
    }
}

impl tokio::io::AsyncWrite for TlsUnixStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        match this.poll_established(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {}
        }
        match this {
            TlsUnixStream::Established(stream) => Pin::new(stream).poll_write(cx, data),
            TlsUnixStream::Handshaking(_) => {
                unreachable!("poll_established guarantees Established")
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        match this.poll_established(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {}
        }
        match this {
            TlsUnixStream::Established(stream) => Pin::new(stream).poll_flush(cx),
            TlsUnixStream::Handshaking(_) => {
                unreachable!("poll_established guarantees Established")
            }
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        match this.poll_established(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {}
        }
        match this {
            TlsUnixStream::Established(stream) => Pin::new(stream).poll_shutdown(cx),
            TlsUnixStream::Handshaking(_) => {
                unreachable!("poll_established guarantees Established")
            }
        }
    }
}

impl tonic::transport::server::Connected for TlsUnixStream {
    type ConnectInfo = ();

    fn connect_info(&self) -> Self::ConnectInfo {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn accept_binds_a_socket_and_announces_it() {
        let dir = tempfile::tempdir().unwrap();
        let (outbound_tx, mut outbound_rx) = mpsc::channel::<ConnInfo>(1);
        let broker = Broker {
            outbound: outbound_tx,
            pending: Arc::new(Mutex::new(HashMap::new())),
        };

        let listener = broker.accept(7, dir.path()).await.unwrap();
        drop(listener);

        let info = outbound_rx.recv().await.unwrap();
        assert_eq!(info.service_id, 7);
        assert_eq!(info.network, "unix");
        assert!(info.address.ends_with("7.sock"));
    }

    #[tokio::test]
    async fn dial_delivers_info_that_arrived_before_the_dial_call() {
        let pending: PendingTable = Arc::new(Mutex::new(HashMap::new()));
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("early.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        // Simulate dispatch_inbound() having already delivered an
        // announcement before dial() is ever called for this ID.
        {
            let mut table = pending.lock().await;
            let slot = table.entry(3).or_insert_with(new_pending_slot);
            slot.sender
                .try_send(ConnInfo {
                    service_id: 3,
                    network: "unix".to_string(),
                    address: sock_path.display().to_string(),
                    knock: None,
                })
                .unwrap();
        }

        let broker = Broker {
            outbound: mpsc::channel(1).0,
            pending,
        };

        let (dialed, _accepted) = tokio::join!(broker.dial(3), listener.accept());
        assert!(dialed.is_ok());
    }

    #[tokio::test]
    async fn dial_rejects_a_second_call_for_an_id_already_claimed() {
        // Simulate a first dial() already in flight for this ID: its
        // receiver has been taken and not yet returned. A second call must
        // fail immediately rather than block — this test never waits on the
        // real DIAL_TIMEOUT.
        let pending: PendingTable = Arc::new(Mutex::new(HashMap::new()));
        {
            let mut table = pending.lock().await;
            let mut slot = new_pending_slot();
            slot.receiver = None;
            table.insert(9, slot);
        }
        let broker = Broker {
            outbound: mpsc::channel(1).0,
            pending,
        };

        let err = broker.dial(9).await.unwrap_err();
        let message = err.to_string();
        assert!(message.contains("more than once"), "{message}");
    }
}
