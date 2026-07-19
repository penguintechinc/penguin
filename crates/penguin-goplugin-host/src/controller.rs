//! The `GRPCController` client: tells the plugin's gRPC server to stop.
//!
//! This is one half of shutdown. `Shutdown` only tells the plugin's gRPC
//! server to stop serving — it does not kill the process. `client.rs` still
//! owns the bounded wait and SIGKILL fallback described in the crate-level
//! doc comment; never send SIGTERM/SIGINT instead of this RPC, since the
//! plugin installs a signal handler that permanently ignores SIGINT and
//! never reads stdin.

use penguin_proto::goplugin::Empty;
use penguin_proto::goplugin::grpc_controller_client::GrpcControllerClient;
use tonic::transport::Channel;

/// A connected handle to the plugin's `GRPCController` service.
pub struct Controller {
    client: GrpcControllerClient<Channel>,
}

impl Controller {
    /// Wraps an existing channel to the plugin.
    pub fn new(channel: Channel) -> Controller {
        Controller {
            client: GrpcControllerClient::new(channel),
        }
    }

    /// Calls `GRPCController.Shutdown` and waits for the plugin's response.
    pub async fn shutdown(&mut self) -> Result<(), tonic::Status> {
        self.client.shutdown(Empty::default()).await?;
        Ok(())
    }
}
