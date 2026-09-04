//! Mock SessionProxy gRPC server for testing.
//!
//! Implements the `penguin.desktop.v1.SessionProxy` server trait with recording
//! of all RPC calls for assertion in tests.

use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::{Request, Response, Status};

use penguin_proto::desktop::v1::{
    ProxyHttpRequest, ProxyHttpResponse, SetUserSessionResponse, UserSession,
    session_proxy_server::SessionProxy,
};

/// Mock SessionProxy server for testing choreography.
#[derive(Clone)]
pub struct MockSessionProxy {
    /// Recorded SetUserSession calls: (access_token, refresh_token, hub_base_url)
    pub recorded_sessions: Arc<Mutex<Vec<(String, String, String)>>>,
    /// Recorded ProxyRequest calls
    pub recorded_requests: Arc<Mutex<Vec<(String, String)>>>,
    /// Response to return from ProxyRequest (status, body)
    pub proxy_response: Arc<Mutex<(u32, Vec<u8>)>>,
    /// Whether to return an error from proxy_request
    pub proxy_error: Arc<Mutex<bool>>,
}

impl MockSessionProxy {
    /// Creates a new mock server.
    pub fn new() -> Self {
        MockSessionProxy {
            recorded_sessions: Arc::new(Mutex::new(Vec::new())),
            recorded_requests: Arc::new(Mutex::new(Vec::new())),
            proxy_response: Arc::new(Mutex::new((200, b"{}".to_vec()))),
            proxy_error: Arc::new(Mutex::new(false)),
        }
    }

    /// Sets the response to return from ProxyRequest.
    pub async fn set_proxy_response(&self, status: u32, body: Vec<u8>) {
        *self.proxy_response.lock().await = (status, body);
    }

    /// Sets whether to return an error from proxy_request.
    pub async fn set_proxy_error(&self, error: bool) {
        *self.proxy_error.lock().await = error;
    }

    /// Gets the recorded sessions.
    pub async fn recorded_sessions(&self) -> Vec<(String, String, String)> {
        self.recorded_sessions.lock().await.clone()
    }

    /// Gets the recorded requests.
    pub async fn recorded_requests(&self) -> Vec<(String, String)> {
        self.recorded_requests.lock().await.clone()
    }
}

impl Default for MockSessionProxy {
    fn default() -> Self {
        Self::new()
    }
}

#[tonic::async_trait]
impl SessionProxy for MockSessionProxy {
    async fn proxy_request(
        &self,
        request: Request<ProxyHttpRequest>,
    ) -> Result<Response<ProxyHttpResponse>, Status> {
        // Check if we should return an error
        if *self.proxy_error.lock().await {
            return Err(Status::internal("ProxyRequest error (mocked)"));
        }

        let req = request.into_inner();

        // Record the request
        self.recorded_requests
            .lock()
            .await
            .push((req.method.clone(), req.path.clone()));

        // Return the configured response
        let (status, body) = self.proxy_response.lock().await.clone();

        Ok(Response::new(ProxyHttpResponse {
            api_version: "v1".to_string(),
            status,
            headers: vec![],
            body,
        }))
    }

    async fn set_user_session(
        &self,
        request: Request<UserSession>,
    ) -> Result<Response<SetUserSessionResponse>, Status> {
        let req = request.into_inner();

        // Record the session
        self.recorded_sessions.lock().await.push((
            req.access_token.clone(),
            req.refresh_token.clone(),
            req.hub_base_url.clone(),
        ));

        Ok(Response::new(SetUserSessionResponse {
            api_version: "v1".to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_server_records_session() {
        let server = MockSessionProxy::new();

        // Simulate a SetUserSession call
        let req = UserSession {
            api_version: "v1".to_string(),
            access_token: "token123".to_string(),
            refresh_token: "refresh456".to_string(),
            hub_base_url: "https://hub.example.com".to_string(),
        };

        let _resp = server.set_user_session(Request::new(req)).await.unwrap();

        // Verify recording
        let sessions = server.recorded_sessions().await;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].0, "token123");
        assert_eq!(sessions[0].1, "refresh456");
        assert_eq!(sessions[0].2, "https://hub.example.com");
    }

    #[tokio::test]
    async fn test_mock_server_returns_proxy_response() {
        let server = MockSessionProxy::new();
        server
            .set_proxy_response(200, b"test response".to_vec())
            .await;

        let req = ProxyHttpRequest {
            api_version: "v1".to_string(),
            method: "GET".to_string(),
            path: "/api/v1/test".to_string(),
            headers: vec![],
            body: vec![],
        };

        let resp = server.proxy_request(Request::new(req)).await.unwrap();
        let inner = resp.into_inner();

        assert_eq!(inner.status, 200);
        assert_eq!(inner.body, b"test response".to_vec());
    }
}
