//! The gRPC services a served plugin exposes on its main connection:
//! `penguin.sdk.v1.ModuleService` (delegating to the author's [`Module`]),
//! `plugin.GRPCController` (`Shutdown`), and `plugin.GRPCStdio`
//! (`StreamStdio`). `grpc.health.v1.Health` is wired up directly in
//! `serve.rs` via `tonic-health`, since it needs no [`Module`] access.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use tokio::sync::{oneshot, watch};
use tokio_stream::Stream;
use tonic::{Request, Response, Status};

use penguin_proto::goplugin;
use penguin_proto::goplugin::grpc_controller_server::GrpcController;
use penguin_proto::goplugin::grpc_stdio_server::GrpcStdio;
use penguin_proto::sdk::v1 as pb;
use penguin_proto::sdk::v1::module_service_server::ModuleService;

use crate::convert::{
    API_VERSION, command_result_to_proto, command_spec_to_proto, health_report_to_proto,
    status_to_proto,
};
use crate::module::Module;

/// Validates the wire `api_version` field per the org gRPC versioning
/// standard: an unknown or missing version is `UNIMPLEMENTED`, never a
/// silent fall-through to the current handler.
fn require_v1(api_version: &str) -> Result<(), Status> {
    if api_version == API_VERSION {
        Ok(())
    } else {
        Err(Status::unimplemented(format!(
            "api_version {api_version:?} not supported"
        )))
    }
}

/// The `ModuleService` implementation, delegating every RPC to the author's
/// [`Module`].
///
/// `ready` gates `start`/`stop`/`status`/`health`/`dispatch` — the RPCs that
/// plausibly depend on [`Module::init`] having already run — so a caller
/// that connects before `serve.rs`'s broker-dial-and-init sequence finishes
/// simply waits rather than racing it. `info`/`commands`/`config_schema` are
/// never gated: they are static metadata a module must be able to answer
/// before `init`, exactly like `penguin-goplugin-host::adapter::ModuleAdapter`
/// already assumes when it fetches `Info` first.
pub struct ModuleServiceImpl {
    pub module: Arc<dyn Module>,
    pub ready: watch::Receiver<bool>,
}

impl ModuleServiceImpl {
    /// Waits until `serve.rs` has finished the broker-dial-and-init sequence
    /// (successfully or degraded to a no-op `HostServices` — either way,
    /// `Module::init` has returned by the time this resolves).
    async fn wait_ready(&self) {
        let mut ready = self.ready.clone();
        let _ = ready.wait_for(|is_ready| *is_ready).await;
    }
}

#[tonic::async_trait]
impl ModuleService for ModuleServiceImpl {
    async fn info(
        &self,
        request: Request<pb::InfoRequest>,
    ) -> Result<Response<pb::InfoResponse>, Status> {
        require_v1(&request.into_inner().api_version)?;
        let info = self.module.info();
        Ok(Response::new(pb::InfoResponse {
            name: info.name,
            version: info.version,
            description: info.description,
            license_feature: info.license_feature,
        }))
    }

    async fn init(
        &self,
        request: Request<pb::InitRequest>,
    ) -> Result<Response<pb::InitResponse>, Status> {
        require_v1(&request.into_inner().api_version)?;
        // Deliberate no-op, bug-compatible with the frozen Go SDK: no
        // adapter on either side of the wire ever calls this RPC — the real
        // `Module::init` runs locally in `serve.rs` before this service
        // starts answering. See `penguin-goplugin-host::adapter`'s doc
        // comment on the same convention from the host's side.
        Ok(Response::new(pb::InitResponse {
            error: String::new(),
        }))
    }

    async fn start(
        &self,
        request: Request<pb::StartRequest>,
    ) -> Result<Response<pb::StartResponse>, Status> {
        require_v1(&request.into_inner().api_version)?;
        self.wait_ready().await;
        let error = match self.module.start().await {
            Ok(()) => String::new(),
            Err(e) => e.message,
        };
        Ok(Response::new(pb::StartResponse { error }))
    }

    async fn stop(
        &self,
        request: Request<pb::StopRequest>,
    ) -> Result<Response<pb::StopResponse>, Status> {
        require_v1(&request.into_inner().api_version)?;
        self.wait_ready().await;
        let error = match self.module.stop().await {
            Ok(()) => String::new(),
            Err(e) => e.message,
        };
        Ok(Response::new(pb::StopResponse { error }))
    }

    async fn status(
        &self,
        request: Request<pb::StatusRequest>,
    ) -> Result<Response<pb::StatusResponse>, Status> {
        require_v1(&request.into_inner().api_version)?;
        self.wait_ready().await;
        let response = match self.module.status().await {
            Ok(status) => status_to_proto(&status),
            Err(e) => pb::StatusResponse {
                state: String::new(),
                detail: std::collections::HashMap::new(),
                error: e.message,
            },
        };
        Ok(Response::new(response))
    }

    async fn health(
        &self,
        request: Request<pb::HealthRequest>,
    ) -> Result<Response<pb::HealthResponse>, Status> {
        require_v1(&request.into_inner().api_version)?;
        self.wait_ready().await;
        let report = self.module.health().await;
        Ok(Response::new(health_report_to_proto(&report)))
    }

    async fn commands(
        &self,
        request: Request<pb::CommandsRequest>,
    ) -> Result<Response<pb::CommandsResponse>, Status> {
        require_v1(&request.into_inner().api_version)?;
        let mut commands: Vec<pb::CommandSpec> = Vec::new();
        for spec in self.module.commands() {
            commands.push(command_spec_to_proto(&spec));
        }
        Ok(Response::new(pb::CommandsResponse { commands }))
    }

    async fn dispatch(
        &self,
        request: Request<pb::DispatchRequest>,
    ) -> Result<Response<pb::DispatchResponse>, Status> {
        let req = request.into_inner();
        require_v1(&req.api_version)?;
        self.wait_ready().await;
        let outcome = self.module.dispatch(&req.path, &req.flags, &req.args).await;
        let response = match outcome {
            Ok(result) => command_result_to_proto(&result),
            Err(e) => pb::DispatchResponse {
                output: String::new(),
                json: Vec::new(),
                exit_code: 0,
                error: e.message,
            },
        };
        Ok(Response::new(response))
    }

    async fn config_schema(
        &self,
        request: Request<pb::ConfigSchemaRequest>,
    ) -> Result<Response<pb::ConfigSchemaResponse>, Status> {
        require_v1(&request.into_inner().api_version)?;
        let schema = self.module.config_schema().unwrap_or_default();
        Ok(Response::new(pb::ConfigSchemaResponse { schema }))
    }
}

/// The `GRPCController` implementation: `Shutdown` fires the one-shot signal
/// `serve.rs` passed to `serve_with_incoming_shutdown`, so the gRPC server
/// stops accepting new work and the process can exit promptly — the host
/// waits only 2s before escalating to SIGKILL.
pub struct ControllerImpl {
    shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
}

impl ControllerImpl {
    /// Wraps the one-shot shutdown sender.
    pub fn new(shutdown_tx: oneshot::Sender<()>) -> ControllerImpl {
        ControllerImpl {
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
        }
    }
}

#[tonic::async_trait]
impl GrpcController for ControllerImpl {
    async fn shutdown(
        &self,
        _request: Request<goplugin::Empty>,
    ) -> Result<Response<goplugin::Empty>, Status> {
        let sender = {
            let mut guard = self.shutdown_tx.lock().unwrap_or_else(|e| e.into_inner());
            guard.take()
        };
        if let Some(sender) = sender {
            // A send failure only means the server future already ended on
            // its own; either way shutdown is in progress.
            let _ = sender.send(());
        }
        Ok(Response::new(goplugin::Empty {}))
    }
}

/// The `GRPCStdio` implementation. This SDK never redirects the plugin
/// process's real stdout/stderr into this stream (diagnostic logging goes
/// through `tracing` to stderr directly instead), so `StreamStdio` reports
/// `Unimplemented` — `penguin-goplugin-host::stdio::StdioClient::connect`
/// already treats that as "no stdio to drain" rather than an error, so this
/// cannot hang or fail a caller.
pub struct StdioImpl;

#[tonic::async_trait]
impl GrpcStdio for StdioImpl {
    type StreamStdioStream =
        Pin<Box<dyn Stream<Item = Result<goplugin::StdioData, Status>> + Send + 'static>>;

    async fn stream_stdio(
        &self,
        _request: Request<()>,
    ) -> Result<Response<Self::StreamStdioStream>, Status> {
        Err(Status::unimplemented("stdio forwarding not implemented"))
    }
}
