//! A [`Stream`] of TLS-wrapped unix-socket connections, suitable for
//! `tonic::transport::Server::serve_with_incoming_shutdown`.
//!
//! Mirrors `penguin-goplugin-host::broker::TlsIncoming`/`TlsUnixStream`
//! verbatim (see that module's doc comment for the full reasoning); it is
//! duplicated here rather than imported because `penguin-sdk` cannot depend
//! on `penguin-goplugin-host` — that crate already depends on this one.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::net::{UnixListener, UnixStream};
use tokio_stream::Stream;

/// Accepts connections from `listener` and wraps each one in TLS via
/// `acceptor`, deferring the handshake itself to [`TlsUnixStream`]'s first
/// poll so accepting a new connection never blocks on a slow peer.
pub struct TlsIncoming {
    pub listener: UnixListener,
    pub acceptor: tokio_rustls::TlsAcceptor,
}

impl Stream for TlsIncoming {
    type Item = std::io::Result<TlsUnixStream>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.listener.poll_accept(cx) {
            Poll::Ready(Ok((stream, _addr))) => {
                let acceptor = this.acceptor.clone();
                Poll::Ready(Some(Ok(TlsUnixStream::new(acceptor, stream))))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// A unix-socket connection mid- or post-TLS-handshake. Starts as a pending
/// handshake future and switches to the established stream the first time
/// it is polled to completion.
pub enum TlsUnixStream {
    Handshaking(Pin<Box<tokio_rustls::Accept<UnixStream>>>),
    // Boxed so this variant doesn't dwarf `Handshaking`'s.
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
