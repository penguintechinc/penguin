//! IPC client wrapper for the desktop shell to dial penguind and call ProxyRequest.
//!
//! Wraps penguin_ipc's dial/probe + the gRPC DaemonClient to forward `/api/v1/**`
//! requests to the hub through the penguind module.
//!
//! Reuses the proven tray/CLI pattern: dial over UDS, probe with Version RPC,
//! then forward ProxyRequest calls.

use std::time::Duration;
use tokio::time::timeout;
use tonic::transport::Channel;
use tracing::{debug, info};

use penguin_proto::daemon::v1::VersionRequest;
use penguin_proto::daemon::v1::daemon_client::DaemonClient;
use penguin_proto::desktop::v1::session_proxy_client::SessionProxyClient;
use penguin_proto::desktop::v1::{Header as ProtoHeader, ProxyHttpRequest, UserSession};

use crate::error::{DesktopError, Result};
use crate::ipc_dial::dial_unix;

const API_VERSION: &str = "v1";
const PROBE_TIMEOUT_SECS: u64 = 3;
const DEFAULT_SOCKET_PATH: &str = "/run/penguin/penguind.sock";

/// An HTTP header pair (name, value).
#[derive(Clone, Debug)]
pub struct Header {
    pub name: String,
    pub value: String,
}

/// An HTTP request to forward to the hub via penguind's ProxyRequest RPC.
#[derive(Clone, Debug)]
pub struct ApiRequest {
    /// HTTP method (GET, POST, etc.).
    pub method: String,
    /// API path (e.g., `/api/v1/auth/login`).
    pub path: String,
    /// Request headers.
    pub headers: Vec<Header>,
    /// Request body (JSON-encoded bytes).
    pub body: Vec<u8>,
}

/// An HTTP response from the hub via penguind's ProxyResponse RPC.
#[derive(Clone, Debug)]
pub struct ApiResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers.
    pub headers: Vec<Header>,
    /// Response body (JSON-encoded bytes).
    pub body: Vec<u8>,
}

/// The desktop IPC client: wraps the connection to penguind and forwards
/// API requests and session commands.
pub struct IpcClient {
    session_client: SessionProxyClient<Channel>,
}

impl IpcClient {
    /// Creates a new IPC client by dialing penguind on the default socket and
    /// probing with Version to confirm liveness.
    ///
    /// Uses `DEFAULT_SOCKET_PATH` (`/run/penguin/penguind.sock`).
    /// Probe timeout is 3 seconds.
    pub async fn connect() -> Result<Self> {
        Self::connect_with_socket(DEFAULT_SOCKET_PATH).await
    }

    /// Creates a new IPC client on a custom socket path.
    pub async fn connect_with_socket(socket_path: &str) -> Result<Self> {
        Self::connect_internal(socket_path, true).await
    }

    /// Creates a new IPC client without probing (for tests).
    #[cfg(test)]
    pub async fn connect_with_socket_no_probe(socket_path: &str) -> Result<Self> {
        Self::connect_internal(socket_path, false).await
    }

    /// Internal connection logic with probe toggle.
    async fn connect_internal(socket_path: &str, probe: bool) -> Result<Self> {
        // Dial penguind over UDS (unix domain socket). Reuse the proven pattern
        // from penguin-tray's connection.rs.
        let channel = dial_unix(socket_path).await?;

        if probe {
            // Probe with Version RPC to confirm liveness.
            let mut daemon_client = DaemonClient::new(channel.clone());
            let probe_req = VersionRequest {
                api_version: API_VERSION.to_string(),
            };

            let probe_result = timeout(
                Duration::from_secs(PROBE_TIMEOUT_SECS),
                daemon_client.version(probe_req),
            )
            .await;

            match probe_result {
                Ok(Ok(_)) => {
                    info!("IPC connection to penguind established and probed");
                }
                Ok(Err(e)) => {
                    return Err(DesktopError::IpcConnection(format!(
                        "probe RPC failed: {}",
                        e
                    )));
                }
                Err(_) => {
                    return Err(DesktopError::IpcConnection("probe timeout".to_string()));
                }
            }
        }

        let session_client = SessionProxyClient::new(channel.clone());

        Ok(IpcClient { session_client })
    }

    /// Forwards an HTTP request to the hub via penguind's ProxyRequest RPC.
    ///
    /// The module (waddlebot-desktop) holds the in-memory session tokens and
    /// handles Bearer auth + refresh logic server-side.
    pub async fn proxy_request(&mut self, req: ApiRequest) -> Result<ApiResponse> {
        let proto_headers = req
            .headers
            .into_iter()
            .map(|h| ProtoHeader {
                name: h.name,
                value: h.value,
            })
            .collect();

        let proto_req = ProxyHttpRequest {
            api_version: API_VERSION.to_string(),
            method: req.method.clone(),
            path: req.path.clone(),
            headers: proto_headers,
            body: req.body,
        };

        let resp = self
            .session_client
            .proxy_request(proto_req)
            .await
            .map_err(|e| {
                debug!("ProxyRequest RPC failed: {}", e);
                DesktopError::GrpcError(format!("ProxyRequest failed: {}", e))
            })?
            .into_inner();

        Ok(ApiResponse {
            status: resp.status as u16,
            headers: resp
                .headers
                .into_iter()
                .map(|h| Header {
                    name: h.name,
                    value: h.value,
                })
                .collect(),
            body: resp.body,
        })
    }

    /// Primes the penguind module with the user's session tokens.
    ///
    /// Called after login or OAuth token exchange. The module holds the tokens
    /// in-memory and uses them for subsequent ProxyRequest calls.
    pub async fn set_user_session(
        &mut self,
        access_token: String,
        refresh_token: Option<String>,
        hub_base_url: String,
    ) -> Result<()> {
        let proto_req = UserSession {
            api_version: API_VERSION.to_string(),
            access_token,
            refresh_token: refresh_token.unwrap_or_default(),
            hub_base_url,
        };

        self.session_client
            .set_user_session(proto_req)
            .await
            .map_err(|e| {
                debug!("SetUserSession RPC failed: {}", e);
                DesktopError::GrpcError(format!("SetUserSession failed: {}", e))
            })?;

        debug!("user session primed in penguind");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_request_creation() {
        let req = ApiRequest {
            method: "POST".to_string(),
            path: "/api/v1/test".to_string(),
            headers: vec![Header {
                name: "Content-Type".to_string(),
                value: "application/json".to_string(),
            }],
            body: vec![1, 2, 3],
        };

        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/api/v1/test");
        assert_eq!(req.headers.len(), 1);
        assert_eq!(req.body, vec![1, 2, 3]);
    }

    #[test]
    fn test_api_response_creation() {
        let resp = ApiResponse {
            status: 200,
            headers: vec![Header {
                name: "Content-Type".to_string(),
                value: "application/json".to_string(),
            }],
            body: vec![4, 5, 6],
        };

        assert_eq!(resp.status, 200);
        assert_eq!(resp.headers.len(), 1);
        assert_eq!(resp.body, vec![4, 5, 6]);
    }

    #[test]
    fn test_header_structure() {
        let header = Header {
            name: "X-Custom".to_string(),
            value: "test-value".to_string(),
        };

        assert_eq!(header.name, "X-Custom");
        assert_eq!(header.value, "test-value");
    }
}
