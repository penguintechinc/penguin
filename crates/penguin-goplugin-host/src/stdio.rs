//! The `GRPCStdio` client: drains the plugin's stdout/stderr over gRPC.
//!
//! A go-plugin plugin never writes to its real stdout/stderr after startup —
//! everything is funneled through this stream instead, specifically so the
//! host can multiplex a plugin's logs without the two processes racing on a
//! shared terminal. If nobody drains the stream, the plugin's own writes
//! eventually block on a full pipe, so `client.rs` connects and starts
//! [`StdioClient::drain`] in a background task immediately after the gRPC
//! channel is up — never synchronously in the connect path.

use penguin_proto::goplugin::StdioData;
use penguin_proto::goplugin::grpc_stdio_client::GrpcStdioClient;
use penguin_proto::goplugin::stdio_data::Channel as WireChannel;
use tonic::transport::Channel;
use tonic::{Code, Streaming};

/// A connected handle to the plugin's `GRPCStdio` service.
///
/// `StreamStdio` may only be called once per plugin connection, so this type
/// is consumed by [`StdioClient::drain`] rather than offering a repeatable
/// call.
pub struct StdioClient {
    stream: Streaming<StdioData>,
}

impl StdioClient {
    /// Opens the single permitted `StreamStdio` call.
    ///
    /// Returns `Ok(None)` — not an error — when the plugin doesn't implement
    /// the service (`Unimplemented`/`Unavailable`): matches upstream's
    /// fallback for plugins built against older go-plugin versions that
    /// predate stdio forwarding.
    pub async fn connect(channel: Channel) -> Result<Option<StdioClient>, tonic::Status> {
        let mut client = GrpcStdioClient::new(channel);
        match client.stream_stdio(()).await {
            Ok(response) => Ok(Some(StdioClient {
                stream: response.into_inner(),
            })),
            Err(status)
                if status.code() == Code::Unavailable || status.code() == Code::Unimplemented =>
            {
                Ok(None)
            }
            Err(status) => Err(status),
        }
    }

    /// Drains the stream until the plugin closes it, forwarding every chunk
    /// to `tracing`. Runs until EOF or a transport error — spawn this as its
    /// own task; it never returns while the plugin is alive and forwarding
    /// output.
    pub async fn drain(mut self) {
        loop {
            let next = self.stream.message().await;
            let chunk = match next {
                Ok(Some(chunk)) => chunk,
                Ok(None) => return,
                Err(status) => {
                    tracing::debug!(error = %status, "plugin stdio stream ended");
                    return;
                }
            };
            forward_chunk(&chunk);
        }
    }
}

/// Routes one chunk to the matching `tracing` level. Plugin stdout maps to
/// `info` and stderr to `warn`, mirroring go-plugin's own convention that a
/// plugin's stderr carries its structured log lines.
fn forward_chunk(chunk: &StdioData) {
    let text = String::from_utf8_lossy(&chunk.data);
    match WireChannel::try_from(chunk.channel) {
        Ok(WireChannel::Stdout) => tracing::info!(target: "goplugin::stdout", "{text}"),
        Ok(WireChannel::Stderr) => tracing::warn!(target: "goplugin::stderr", "{text}"),
        _ => tracing::debug!(target: "goplugin::stdio", channel = chunk.channel, "{text}"),
    }
}
