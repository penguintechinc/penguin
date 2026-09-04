//! Windows named-pipe client connector.
//!
//! This file is not compiled or verified on Linux CI — `#[cfg(windows)]` on
//! its `pub mod` declaration in `lib.rs` excludes it entirely from a Linux
//! build. It is verified by the Windows job introduced in M7.
//!
//! DEPENDENCY GAP: this file needs `hyper_util::rt::TokioIo` to adapt a
//! plain Tokio `AsyncRead + AsyncWrite` named-pipe client into what tonic's
//! `Endpoint::connect_with_connector` requires (`hyper::rt::Read + Write`).
//! `hyper-util` is not currently a declared `cfg(windows)` dependency of
//! this crate (only `windows-sys` is, for `listen_windows`) — per this
//! crate's constraints the author of this file may not add dependencies, so
//! it is written against the API `hyper-util` (with its `tokio` feature)
//! provides and flagged here rather than silently added to `Cargo.toml`.
//! Add `hyper-util = { version = "...", features = ["tokio"] }` to the
//! `cfg(windows)` dependency block before the M7 Windows job attempts to
//! build this.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use hyper_util::rt::TokioIo;
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use tokio::time::sleep;
use tonic::codegen::Service;
use tonic::transport::{Channel, Endpoint, Uri};
use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

use crate::listen_windows::PIPE_PATH;

/// Connects to the daemon's named pipe.
///
/// Eager, like `dial_unix::dial` — see that function's doc comment for why
/// this crate always connects eagerly rather than lazily like the frozen Go
/// reference's `grpc.DialContext` (`go-client/internal/ipc/dial_windows.go`).
pub async fn dial() -> Result<Channel, tonic::transport::Error> {
    Endpoint::from_static("http://[::]:50051")
        .connect_with_connector(PipeConnector)
        .await
}

/// Opens a `NamedPipeClient` connection to [`PIPE_PATH`], retrying while the
/// server is busy. This is the client pattern `tokio`'s own
/// `named_pipe` documentation prescribes: `ClientOptions::open` returning
/// `ERROR_PIPE_BUSY` means a server exists but every instance is currently
/// occupied, which is transient and worth a short retry rather than an
/// immediate failure.
async fn open_pipe() -> std::io::Result<NamedPipeClient> {
    loop {
        match ClientOptions::new().open(PIPE_PATH) {
            Ok(client) => return Ok(client),
            Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {}
            Err(err) => return Err(err),
        }
        sleep(Duration::from_millis(50)).await;
    }
}

/// A `Service<Uri>` that ignores the URI — there is only ever one pipe —
/// and opens a fresh named-pipe connection per call. Named rather than
/// built from a closure so the `Service` impl below reads as ordinary
/// trait-implementing code rather than an inline adapter chain.
#[derive(Clone, Copy, Default)]
struct PipeConnector;

impl Service<Uri> for PipeConnector {
    type Response = TokioIo<NamedPipeClient>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = std::io::Result<Self::Response>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        Box::pin(async { open_pipe().await.map(TokioIo::new) })
    }
}
