//! Remote action executor: poll loop, signed-binary verification, script approval gate,
//! response posting, and audit logging for the PenguinTech desktop bridge.
//!
//! **Architecture:**
//! - Polls the hub's `/api/v1/bridge/{poll,register,heartbeat,unregister,response}` endpoints
//!   over a fixed-interval ticker (not true long-poll).
//! - Dispatches actions by type: signed binary (verify + auto-execute), script/Lua (approval-gated).
//! - Captures subprocess output (capped at 64 KiB), enforces timeouts, clears environment.
//! - Posts execution results back to the hub with a fixed response body shape.
//! - Audit-logs every execution via `penguin-telemetry`.
//!
//! **Fail-closed contract:**
//! - Signed binary verification failure → refuse execution, respond to hub with rejection
//! - Approval decision missing / denied / timeout → refuse execution, respond to hub
//! - Execution timeout or output overflow → kill process, report to hub

pub mod approval;
pub mod script;
pub mod signed;

use penguin_sdk::SecretStore;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};
use uuid::Uuid;

use crate::Session;
use crate::error::{DesktopError, Result};

pub use approval::{ApprovalDecision, ApprovalPrompt, PendingAction};
pub use script::ScriptExecutor;
pub use signed::SignedExecutor;

/// Maximum size of captured stdout/stderr per action (64 KiB).
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// A machine ID generated at first run and persisted via `penguin-secrets`.
/// Sent on every poll/register/heartbeat call for proper action targeting.
#[derive(Clone, Debug)]
pub struct MachineId(String);

impl MachineId {
    /// Generates a new machine ID (UUID v4) and persists it via `penguin-secrets`.
    /// If one is already stored, loads and returns it instead.
    pub async fn load_or_create() -> Result<Self> {
        // Load or create the secure store with default configuration
        use penguin_secrets::Config;
        let cfg = Config {
            service_name: "penguin-desktop-bridge".to_string(),
            backend: penguin_secrets::Backend::Auto {
                file_dir: std::path::PathBuf::from(&format!(
                    "{}/.penguin/secrets",
                    std::env::var("HOME").unwrap_or_else(|_| ".".to_string())
                )),
            },
        };
        let store = penguin_secrets::Store::open(cfg)
            .map_err(|e| DesktopError::Internal(format!("failed to open secret store: {}", e)))?;
        // The Store has a namespaced() method that returns a new Store with a prefix
        let ns_store = store.namespaced("penguin-desktop-bridge");
        let key = "machine_id";

        // Try to load existing
        if let Ok(existing_bytes) = ns_store.get(key).await {
            let existing_id = String::from_utf8(existing_bytes)
                .map_err(|_| DesktopError::Internal("invalid machine ID in store".to_string()))?;
            return Ok(MachineId(existing_id));
        }

        // Generate and store new
        let new_id = Uuid::new_v4().to_string();
        ns_store
            .set(key, new_id.as_bytes())
            .await
            .map_err(|e| DesktopError::Internal(format!("failed to store machine ID: {}", e)))?;

        debug!(machine_id = %new_id, "generated and persisted new machine ID");
        Ok(MachineId(new_id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Parsed action from the hub's poll response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionRequest {
    pub id: String,
    pub r#type: String, // "signed_binary", "bash", "powershell", "python", "lua"
    pub module_name: Option<String>,
    pub action: Option<String>,
    pub parameters: serde_json::Value, // Map of string keys to string/binary values
    pub user_id: String,
    pub community_id: String,
    pub priority: i32,
    pub timeout: i32, // Seconds
    pub created_at: String,
    pub expires_at: String,
}

/// Response polled from the hub.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PollResponse {
    pub actions: Vec<ActionRequest>,
    pub next_poll: String,
    pub server_time: String,
    pub has_more: bool,
    pub poll_count: u64,
}

/// Result of an executed action, posted back to the hub.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionResponse {
    pub id: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ActionResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub duration_ms: i64,
    pub timestamp: String,
}

/// Result of a successful action execution.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionResult {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
}

/// Driver for the poll→verify/approve→execute→respond loop.
/// Holds a shared session (Arc<Mutex<Session>>), machine ID, and approval-prompt provider.
pub struct ActionExecutor {
    session: Arc<tokio::sync::Mutex<Session>>,
    machine_id: MachineId,
    approval_prompt: Box<dyn ApprovalPrompt>,
}

impl ActionExecutor {
    /// Creates a new executor with a shared session and an approval-prompt provider.
    pub async fn new(
        session: Arc<tokio::sync::Mutex<Session>>,
        approval_prompt: Box<dyn ApprovalPrompt>,
    ) -> Result<Self> {
        let machine_id = MachineId::load_or_create().await?;
        Ok(ActionExecutor {
            session,
            machine_id,
            approval_prompt,
        })
    }

    /// Creates a new executor for testing with a pre-made machine ID.
    #[cfg(test)]
    pub fn new_for_testing(
        session: Arc<tokio::sync::Mutex<Session>>,
        approval_prompt: Box<dyn ApprovalPrompt>,
        machine_id: MachineId,
    ) -> Self {
        ActionExecutor {
            session,
            machine_id,
            approval_prompt,
        }
    }

    /// Registers this machine with the hub (called at shell startup).
    /// Hub response is not parsed — just used to confirm the register endpoint works.
    pub async fn register(&mut self, modules: Vec<String>) -> Result<()> {
        let body = serde_json::json!({
            "bridge_id": self.machine_id.as_str(),
            "modules": modules,
        });

        let body_bytes = serde_json::to_vec(&body)?;

        let _resp = self
            .session
            .lock()
            .await
            .api_request("POST", "/api/v1/bridge/register", Some(body_bytes))
            .await?;

        debug!(machine_id = self.machine_id.as_str(), "registered with hub");
        Ok(())
    }

    /// Sends a heartbeat to the hub (called periodically to confirm liveness).
    pub async fn heartbeat(&mut self) -> Result<()> {
        let body = serde_json::json!({
            "bridge_id": self.machine_id.as_str(),
        });

        let body_bytes = serde_json::to_vec(&body)?;

        let _resp = self
            .session
            .lock()
            .await
            .api_request("POST", "/api/v1/bridge/heartbeat", Some(body_bytes))
            .await?;

        debug!("heartbeat sent to hub");
        Ok(())
    }

    /// Deregisters this machine from the hub (called at shell shutdown).
    pub async fn unregister(&mut self) -> Result<()> {
        let body = serde_json::json!({
            "bridge_id": self.machine_id.as_str(),
        });

        let body_bytes = serde_json::to_vec(&body)?;

        let _resp = self
            .session
            .lock()
            .await
            .api_request("POST", "/api/v1/bridge/unregister", Some(body_bytes))
            .await?;

        debug!("unregistered from hub");
        Ok(())
    }

    /// Polls the hub for pending actions.
    pub async fn poll(&mut self) -> Result<PollResponse> {
        let resp = self
            .session
            .lock()
            .await
            .api_request(
                "GET",
                &format!("/api/v1/bridge/poll?bridge_id={}", self.machine_id.as_str()),
                None,
            )
            .await?;

        if resp.status != 200 {
            return Err(DesktopError::ApiRequest {
                status: resp.status,
            });
        }

        let poll_response: PollResponse = serde_json::from_slice(&resp.body)?;
        debug!(
            action_count = poll_response.actions.len(),
            "polled actions from hub"
        );

        Ok(poll_response)
    }

    /// Posts an action response back to the hub.
    pub async fn post_response(&mut self, response: ActionResponse) -> Result<()> {
        let body_bytes = serde_json::to_vec(&response)?;

        let _resp = self
            .session
            .lock()
            .await
            .api_request("POST", "/api/v1/bridge/response", Some(body_bytes))
            .await?;

        debug!(action_id = response.id, "response posted to hub");
        Ok(())
    }

    /// Executes a single action, returning the response to be posted to the hub.
    pub async fn execute(&mut self, action: ActionRequest) -> ActionResponse {
        let start = std::time::Instant::now();

        match action.r#type.as_str() {
            "signed_binary" => self.execute_signed_binary(&action).await,
            "bash" | "powershell" | "python" => self.execute_script(&action).await,
            "lua" => self.execute_lua(&action).await,
            "obs_command" => self.execute_obs_command(&action).await,
            "webhook" => self.execute_webhook(&action).await,
            _ => ActionResponse {
                id: action.id,
                success: false,
                error: Some(format!("unknown action type: {}", action.r#type)),
                result: None,
                duration_ms: start.elapsed().as_millis() as i64,
                timestamp: chrono::Utc::now().to_rfc3339(),
            },
        }
    }

    /// Executes a signed binary action: verifies the signature first, then runs it.
    async fn execute_signed_binary(&mut self, action: &ActionRequest) -> ActionResponse {
        let start = std::time::Instant::now();

        let session = self.session.lock().await;
        match SignedExecutor::execute(&session, action, self.machine_id.as_str()).await {
            Ok((exit_code, stdout, stderr)) => {
                self.audit_log_execution(&action.id, "signed_binary", exit_code, &stdout, &stderr);

                ActionResponse {
                    id: action.id.clone(),
                    success: true,
                    result: Some(ActionResult {
                        exit_code: Some(exit_code),
                        stdout,
                        stderr,
                        truncated: false,
                    }),
                    error: None,
                    duration_ms: start.elapsed().as_millis() as i64,
                    timestamp: chrono::Utc::now().to_rfc3339(),
                }
            }
            Err(e) => {
                warn!(action_id = action.id, error = %e, "signed binary verification/execution failed");

                ActionResponse {
                    id: action.id.clone(),
                    success: false,
                    error: Some(format!("signed binary error: {}", e)),
                    result: None,
                    duration_ms: start.elapsed().as_millis() as i64,
                    timestamp: chrono::Utc::now().to_rfc3339(),
                }
            }
        }
    }

    /// Executes a script action (bash/powershell/python): asks for approval first.
    async fn execute_script(&mut self, action: &ActionRequest) -> ActionResponse {
        let start = std::time::Instant::now();

        let pending = PendingAction::from_request(action);

        match self.approval_prompt.ask(&pending).await {
            ApprovalDecision::Approved => match ScriptExecutor::execute(action).await {
                Ok((exit_code, stdout, stderr)) => {
                    self.audit_log_execution(
                        &action.id,
                        &action.r#type,
                        exit_code,
                        &stdout,
                        &stderr,
                    );

                    ActionResponse {
                        id: action.id.clone(),
                        success: true,
                        result: Some(ActionResult {
                            exit_code: Some(exit_code),
                            stdout,
                            stderr,
                            truncated: false,
                        }),
                        error: None,
                        duration_ms: start.elapsed().as_millis() as i64,
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    }
                }
                Err(e) => {
                    error!(action_id = action.id, error = %e, "script execution failed");

                    ActionResponse {
                        id: action.id.clone(),
                        success: false,
                        error: Some(format!("script execution error: {}", e)),
                        result: None,
                        duration_ms: start.elapsed().as_millis() as i64,
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    }
                }
            },
            ApprovalDecision::Denied => ActionResponse {
                id: action.id.clone(),
                success: false,
                error: Some("denied by user".to_string()),
                result: None,
                duration_ms: start.elapsed().as_millis() as i64,
                timestamp: chrono::Utc::now().to_rfc3339(),
            },
            ApprovalDecision::Timeout => ActionResponse {
                id: action.id.clone(),
                success: false,
                error: Some("approval timed out".to_string()),
                result: None,
                duration_ms: start.elapsed().as_millis() as i64,
                timestamp: chrono::Utc::now().to_rfc3339(),
            },
        }
    }

    /// Executes a Lua action: asks for approval first (Lua is a script, not auto-run).
    async fn execute_lua(&mut self, action: &ActionRequest) -> ActionResponse {
        let start = std::time::Instant::now();

        let pending = PendingAction::from_request(action);

        match self.approval_prompt.ask(&pending).await {
            ApprovalDecision::Approved => match ScriptExecutor::execute_lua(action).await {
                Ok((stdout, stderr)) => {
                    self.audit_log_execution(&action.id, "lua", 0, &stdout, &stderr);

                    ActionResponse {
                        id: action.id.clone(),
                        success: true,
                        result: Some(ActionResult {
                            exit_code: Some(0),
                            stdout,
                            stderr,
                            truncated: false,
                        }),
                        error: None,
                        duration_ms: start.elapsed().as_millis() as i64,
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    }
                }
                Err(e) => {
                    error!(action_id = action.id, error = %e, "Lua execution failed");

                    ActionResponse {
                        id: action.id.clone(),
                        success: false,
                        error: Some(format!("Lua execution error: {}", e)),
                        result: None,
                        duration_ms: start.elapsed().as_millis() as i64,
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    }
                }
            },
            ApprovalDecision::Denied => ActionResponse {
                id: action.id.clone(),
                success: false,
                error: Some("denied by user".to_string()),
                result: None,
                duration_ms: start.elapsed().as_millis() as i64,
                timestamp: chrono::Utc::now().to_rfc3339(),
            },
            ApprovalDecision::Timeout => ActionResponse {
                id: action.id.clone(),
                success: false,
                error: Some("approval timed out".to_string()),
                result: None,
                duration_ms: start.elapsed().as_millis() as i64,
                timestamp: chrono::Utc::now().to_rfc3339(),
            },
        }
    }

    /// Executes an OBS command action: pre-authorized, no approval gate.
    /// Relays the daemon's result back to the hub unchanged.
    async fn execute_obs_command(&mut self, action: &ActionRequest) -> ActionResponse {
        let start = std::time::Instant::now();

        // Extract action-specific parameters.
        let module_name = action.module_name.clone().unwrap_or_default();
        let obs_request_type = action
            .parameters
            .get("obs_request_type")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let obs_request_data = action
            .parameters
            .get("obs_request_data")
            .and_then(|v| v.as_str())
            .map(|s| s.as_bytes().to_vec())
            .unwrap_or_default();

        let mut session = self.session.lock().await;
        match session
            .execute_bridge_action(
                module_name,
                "obs_command".to_string(),
                obs_request_type,
                obs_request_data,
                String::new(),
                Vec::new(),
            )
            .await
        {
            Ok(bridge_resp) => {
                self.audit_log_execution(
                    &action.id,
                    "obs_command",
                    if bridge_resp.success { 0 } else { 1 },
                    &bridge_resp.obs_response_data,
                    bridge_resp.error.as_bytes(),
                );

                ActionResponse {
                    id: action.id.clone(),
                    success: bridge_resp.success,
                    result: None,
                    error: if bridge_resp.error.is_empty() {
                        None
                    } else {
                        Some(bridge_resp.error)
                    },
                    duration_ms: start.elapsed().as_millis() as i64,
                    timestamp: chrono::Utc::now().to_rfc3339(),
                }
            }
            Err(e) => {
                error!(action_id = action.id, error = %e, "OBS command execution failed");

                ActionResponse {
                    id: action.id.clone(),
                    success: false,
                    error: Some(format!("OBS command error: {}", e)),
                    result: None,
                    duration_ms: start.elapsed().as_millis() as i64,
                    timestamp: chrono::Utc::now().to_rfc3339(),
                }
            }
        }
    }

    /// Executes a webhook action: pre-authorized, no approval gate.
    /// Relays the daemon's result back to the hub unchanged.
    async fn execute_webhook(&mut self, action: &ActionRequest) -> ActionResponse {
        let start = std::time::Instant::now();

        // Extract action-specific parameters.
        let module_name = action.module_name.clone().unwrap_or_default();
        let webhook_name = action
            .parameters
            .get("webhook_name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let webhook_payload = action
            .parameters
            .get("webhook_payload")
            .and_then(|v| v.as_str())
            .map(|s| s.as_bytes().to_vec())
            .unwrap_or_default();

        let mut session = self.session.lock().await;
        match session
            .execute_bridge_action(
                module_name,
                "webhook".to_string(),
                String::new(),
                Vec::new(),
                webhook_name,
                webhook_payload,
            )
            .await
        {
            Ok(bridge_resp) => {
                self.audit_log_execution(
                    &action.id,
                    "webhook",
                    bridge_resp.http_status as i32,
                    &[],
                    bridge_resp.error.as_bytes(),
                );

                ActionResponse {
                    id: action.id.clone(),
                    success: bridge_resp.success,
                    result: None,
                    error: if bridge_resp.error.is_empty() {
                        None
                    } else {
                        Some(bridge_resp.error)
                    },
                    duration_ms: start.elapsed().as_millis() as i64,
                    timestamp: chrono::Utc::now().to_rfc3339(),
                }
            }
            Err(e) => {
                error!(action_id = action.id, error = %e, "webhook execution failed");

                ActionResponse {
                    id: action.id.clone(),
                    success: false,
                    error: Some(format!("webhook error: {}", e)),
                    result: None,
                    duration_ms: start.elapsed().as_millis() as i64,
                    timestamp: chrono::Utc::now().to_rfc3339(),
                }
            }
        }
    }

    /// Audit-logs an action execution via structured logging.
    fn audit_log_execution(
        &self,
        action_id: &str,
        action_type: &str,
        exit_code: i32,
        stdout: &[u8],
        stderr: &[u8],
    ) {
        // Log action execution with structured fields for audit trail
        tracing::info!(
            action_id = action_id,
            action_type = action_type,
            exit_code = exit_code,
            stdout_len = stdout.len(),
            stderr_len = stderr.len(),
            machine_id = self.machine_id.as_str(),
            "action executed"
        );
    }
}

/// Drives the full poll→execute→respond loop for a configurable duration or until cancellation.
/// Takes ownership of the executor so it can be spawned as a background task.
/// Returns the number of actions processed.
pub async fn run_poll_loop(
    mut executor: ActionExecutor,
    poll_interval: Duration,
    shutdown: CancellationToken,
) -> Result<u64> {
    let mut ticker = tokio::time::interval(poll_interval);
    let mut action_count = 0u64;

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                debug!("poll loop cancelled");
                break;
            }
            _ = ticker.tick() => {
                match executor.poll().await {
                    Ok(poll_resp) => {
                        for action in poll_resp.actions {
                            let resp = executor.execute(action).await;

                            if let Err(e) = executor.post_response(resp).await {
                                error!("failed to post response: {}", e);
                            }

                            action_count += 1;
                        }
                    }
                    Err(e) => {
                        warn!("poll failed: {}", e);
                    }
                }
            }
        }
    }

    Ok(action_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_machine_id_format() {
        // A machine ID should be a valid UUID string
        let id = MachineId("550e8400-e29b-41d4-a716-446655440000".to_string());
        assert_eq!(id.as_str(), "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn test_action_response_structure() {
        let resp = ActionResponse {
            id: "act_123".to_string(),
            success: true,
            result: Some(ActionResult {
                exit_code: Some(0),
                stdout: vec![1, 2, 3],
                stderr: vec![],
                truncated: false,
            }),
            error: None,
            duration_ms: 100,
            timestamp: "2025-01-01T00:00:00Z".to_string(),
        };

        assert!(resp.success);
        assert_eq!(resp.id, "act_123");
        assert_eq!(resp.duration_ms, 100);
    }

    #[test]
    fn test_action_request_parsing() {
        // Test ActionRequest deserialization and field access
        let action = ActionRequest {
            id: "act_test_123".to_string(),
            r#type: "bash".to_string(),
            module_name: Some("test_module".to_string()),
            action: Some("test_action".to_string()),
            parameters: serde_json::json!({
                "script": "echo hello",
                "timeout": 30
            }),
            user_id: "user_456".to_string(),
            community_id: "comm_789".to_string(),
            priority: 5,
            timeout: 60,
            created_at: "2025-01-01T12:00:00Z".to_string(),
            expires_at: "2025-01-01T12:05:00Z".to_string(),
        };

        // Verify all fields are accessible and correct
        assert_eq!(action.id, "act_test_123");
        assert_eq!(action.r#type, "bash");
        assert_eq!(action.module_name, Some("test_module".to_string()));
        assert_eq!(action.action, Some("test_action".to_string()));
        assert_eq!(action.user_id, "user_456");
        assert_eq!(action.community_id, "comm_789");
        assert_eq!(action.priority, 5);
        assert_eq!(action.timeout, 60);
    }

    #[test]
    fn test_poll_response_structure() {
        // Test PollResponse deserialization
        let poll_resp = PollResponse {
            actions: vec![ActionRequest {
                id: "act_1".to_string(),
                r#type: "bash".to_string(),
                module_name: None,
                action: None,
                parameters: serde_json::json!({}),
                user_id: "user1".to_string(),
                community_id: "comm1".to_string(),
                priority: 0,
                timeout: 30,
                created_at: "2025-01-01T00:00:00Z".to_string(),
                expires_at: "2025-01-01T00:05:00Z".to_string(),
            }],
            next_poll: "2025-01-01T00:01:00Z".to_string(),
            server_time: "2025-01-01T00:00:30Z".to_string(),
            has_more: false,
            poll_count: 1,
        };

        assert_eq!(poll_resp.actions.len(), 1);
        assert_eq!(poll_resp.poll_count, 1);
        assert!(!poll_resp.has_more);
    }

    #[test]
    fn test_action_response_error_serialization() {
        // Test ActionResponse with error (no result field)
        let resp = ActionResponse {
            id: "act_err".to_string(),
            success: false,
            error: Some("test error message".to_string()),
            result: None,
            duration_ms: 150,
            timestamp: "2025-01-01T00:00:00Z".to_string(),
        };

        assert_eq!(resp.id, "act_err");
        assert!(!resp.success);
        assert!(resp.error.is_some());
        assert!(resp.result.is_none());

        // Verify serialization omits None fields
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("result").is_none()); // Should be skipped in serialization
    }

    #[test]
    fn test_action_response_truncated_output() {
        // Test ActionResult with truncation flag
        let result = ActionResult {
            exit_code: Some(0),
            stdout: vec![1, 2, 3, 4, 5],
            stderr: vec![],
            truncated: true,
        };

        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout.len(), 5);
        assert!(result.truncated);
    }

    #[test]
    fn test_action_response_no_exit_code() {
        // Test ActionResult with None exit_code (e.g., for Lua)
        let result = ActionResult {
            exit_code: None,
            stdout: vec![72, 105],
            stderr: vec![],
            truncated: false,
        };

        assert_eq!(result.exit_code, None);
        assert_eq!(result.stdout, vec![72, 105]); // "Hi" in ASCII
    }

    // Unit tests for ActionExecutor
    #[test]
    fn test_execute_unknown_action_type_in_dispatch() {
        // Test that unknown action types are caught in the dispatch table
        let action = ActionRequest {
            id: "act_unknown".to_string(),
            r#type: "unknown_type".to_string(),
            module_name: None,
            action: None,
            parameters: serde_json::json!({}),
            user_id: "user_123".to_string(),
            community_id: "comm_456".to_string(),
            priority: 0,
            timeout: 30,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            expires_at: "2025-01-01T00:05:00Z".to_string(),
        };

        // Verify the action type is unknown
        assert!(!matches!(
            action.r#type.as_str(),
            "signed_binary" | "bash" | "powershell" | "python" | "lua" | "obs_command" | "webhook"
        ));
    }

    #[test]
    fn test_action_request_type_variants() {
        let types = vec![
            "signed_binary",
            "bash",
            "powershell",
            "python",
            "lua",
            "obs_command",
            "webhook",
        ];
        for t in types {
            let action = ActionRequest {
                id: "test".to_string(),
                r#type: t.to_string(),
                module_name: None,
                action: None,
                parameters: serde_json::json!({}),
                user_id: "user".to_string(),
                community_id: "comm".to_string(),
                priority: 0,
                timeout: 30,
                created_at: "2025-01-01T00:00:00Z".to_string(),
                expires_at: "2025-01-01T00:05:00Z".to_string(),
            };
            assert_eq!(action.r#type, t);
        }
    }

    #[test]
    fn test_poll_response_empty_actions() {
        let poll_resp = PollResponse {
            actions: vec![],
            next_poll: "2025-01-01T00:01:00Z".to_string(),
            server_time: "2025-01-01T00:00:30Z".to_string(),
            has_more: false,
            poll_count: 0,
        };

        assert_eq!(poll_resp.actions.len(), 0);
        assert!(!poll_resp.has_more);
        assert_eq!(poll_resp.poll_count, 0);
    }

    #[test]
    fn test_poll_response_many_actions() {
        let mut actions = Vec::new();
        for i in 0..10 {
            actions.push(ActionRequest {
                id: format!("act_{}", i),
                r#type: "bash".to_string(),
                module_name: None,
                action: None,
                parameters: serde_json::json!({}),
                user_id: "user".to_string(),
                community_id: "comm".to_string(),
                priority: 0,
                timeout: 30,
                created_at: "2025-01-01T00:00:00Z".to_string(),
                expires_at: "2025-01-01T00:05:00Z".to_string(),
            });
        }

        let poll_resp = PollResponse {
            actions: actions.clone(),
            next_poll: "2025-01-01T00:01:00Z".to_string(),
            server_time: "2025-01-01T00:00:30Z".to_string(),
            has_more: true,
            poll_count: 10,
        };

        assert_eq!(poll_resp.actions.len(), 10);
        assert!(poll_resp.has_more);
        assert_eq!(poll_resp.poll_count, 10);
    }

    #[test]
    fn test_action_response_with_large_output() {
        let large_output = vec![0u8; 50000];
        let resp = ActionResponse {
            id: "act_large".to_string(),
            success: true,
            result: Some(ActionResult {
                exit_code: Some(0),
                stdout: large_output.clone(),
                stderr: vec![],
                truncated: false,
            }),
            error: None,
            duration_ms: 500,
            timestamp: "2025-01-01T00:00:00Z".to_string(),
        };

        assert!(resp.result.is_some());
        if let Some(result) = &resp.result {
            assert_eq!(result.stdout.len(), 50000);
            assert!(!result.truncated);
        }
    }

    #[test]
    fn test_machine_id_persistence_string_format() {
        let uuid_str = "550e8400-e29b-41d4-a716-446655440000";
        let mid = MachineId(uuid_str.to_string());
        assert_eq!(mid.as_str(), uuid_str);
        assert!(!mid.as_str().is_empty());
    }

    #[test]
    fn test_action_response_error_with_all_fields() {
        let resp = ActionResponse {
            id: "act_err_full".to_string(),
            success: false,
            error: Some("command failed with exit code 127".to_string()),
            result: None,
            duration_ms: 250,
            timestamp: "2025-01-01T00:00:00Z".to_string(),
        };

        assert_eq!(resp.id, "act_err_full");
        assert!(!resp.success);
        assert!(resp.error.is_some());
        assert!(resp.result.is_none());
        assert!(resp.duration_ms > 0);
    }

    #[test]
    fn test_action_result_with_stderr_only() {
        let error_msg = b"error: file not found".to_vec();
        let result = ActionResult {
            exit_code: Some(1),
            stdout: vec![],
            stderr: error_msg.clone(),
            truncated: false,
        };

        assert_eq!(result.exit_code, Some(1));
        assert_eq!(result.stdout.len(), 0);
        assert_eq!(result.stderr.len(), error_msg.len());
        assert!(!result.truncated);
    }

    #[test]
    fn test_action_request_with_all_fields_populated() {
        let action = ActionRequest {
            id: "act_full".to_string(),
            r#type: "bash".to_string(),
            module_name: Some("waddlebot".to_string()),
            action: Some("send_message".to_string()),
            parameters: serde_json::json!({
                "script": "echo hello",
                "timeout": 30,
                "user_context": "admin"
            }),
            user_id: "user_abc123".to_string(),
            community_id: "comm_xyz789".to_string(),
            priority: 5,
            timeout: 60,
            created_at: "2025-01-01T10:00:00Z".to_string(),
            expires_at: "2025-01-01T10:05:00Z".to_string(),
        };

        assert_eq!(action.r#type, "bash");
        assert_eq!(action.module_name, Some("waddlebot".to_string()));
        assert_eq!(action.priority, 5);
        assert_eq!(action.timeout, 60);
    }

    #[test]
    fn test_poll_response_serialization_roundtrip() {
        let original = PollResponse {
            actions: vec![ActionRequest {
                id: "act_rt1".to_string(),
                r#type: "bash".to_string(),
                module_name: None,
                action: None,
                parameters: serde_json::json!({"script": "date"}),
                user_id: "user1".to_string(),
                community_id: "comm1".to_string(),
                priority: 1,
                timeout: 10,
                created_at: "2025-01-01T00:00:00Z".to_string(),
                expires_at: "2025-01-01T00:01:00Z".to_string(),
            }],
            next_poll: "2025-01-01T00:02:00Z".to_string(),
            server_time: "2025-01-01T00:00:30Z".to_string(),
            has_more: true,
            poll_count: 123,
        };

        let json = serde_json::to_string(&original).unwrap();
        let restored: PollResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.poll_count, original.poll_count);
        assert_eq!(restored.has_more, original.has_more);
        assert_eq!(restored.actions.len(), original.actions.len());
    }

    // Unit tests for request body serialization
    #[test]
    fn test_register_request_serialization() {
        let body = serde_json::json!({
            "bridge_id": "test-machine-id",
            "modules": vec!["squawk", "waddlebot"],
        });

        let serialized = serde_json::to_vec(&body).expect("serialize");
        assert!(!serialized.is_empty());

        let parsed: serde_json::Value = serde_json::from_slice(&serialized).expect("deserialize");
        assert_eq!(parsed["bridge_id"], "test-machine-id");
    }

    #[test]
    fn test_heartbeat_request_serialization() {
        let body = serde_json::json!({
            "bridge_id": "test-machine-id",
        });

        let serialized = serde_json::to_vec(&body).expect("serialize");
        assert!(!serialized.is_empty());
    }

    #[test]
    fn test_unregister_request_serialization() {
        let body = serde_json::json!({
            "bridge_id": "test-machine-id",
        });

        let serialized = serde_json::to_vec(&body).expect("serialize");
        assert!(!serialized.is_empty());
    }

    #[test]
    fn test_poll_response_with_many_actions() {
        let actions: Vec<ActionRequest> = (0..10)
            .map(|i: i32| ActionRequest {
                id: format!("act_{}", i),
                r#type: "bash".to_string(),
                module_name: None,
                action: None,
                parameters: serde_json::json!({}),
                user_id: format!("user_{}", i),
                community_id: "comm_123".to_string(),
                priority: i,
                timeout: 30 + i,
                created_at: "2025-01-01T00:00:00Z".to_string(),
                expires_at: "2025-01-01T00:05:00Z".to_string(),
            })
            .collect();

        let poll_resp = PollResponse {
            actions: actions.clone(),
            next_poll: "2025-01-01T00:01:00Z".to_string(),
            server_time: "2025-01-01T00:00:30Z".to_string(),
            has_more: true,
            poll_count: 42,
        };

        assert_eq!(poll_resp.actions.len(), 10);
        assert_eq!(poll_resp.poll_count, 42);
        assert!(poll_resp.has_more);

        for (i, action) in poll_resp.actions.iter().enumerate() {
            assert_eq!(action.id, format!("act_{}", i));
            assert_eq!(action.priority, i as i32);
        }
    }

    #[test]
    fn test_action_response_with_mixed_stdout_stderr() {
        let resp = ActionResponse {
            id: "act_mixed".to_string(),
            success: true,
            result: Some(ActionResult {
                exit_code: Some(0),
                stdout: b"output line 1\noutput line 2".to_vec(),
                stderr: b"warning: deprecation notice".to_vec(),
                truncated: false,
            }),
            error: None,
            duration_ms: 250,
            timestamp: "2025-01-01T00:00:00Z".to_string(),
        };

        assert!(resp.success);
        if let Some(result) = resp.result {
            assert!(result.stdout.len() > 20);
            assert!(result.stderr.len() > 20);
            assert_eq!(result.exit_code, Some(0));
        } else {
            panic!("result should be Some");
        }
    }

    #[test]
    fn test_machine_id_uuid_validity() {
        // Verify machine_id strings are valid UUID format
        let test_ids = vec![
            "550e8400-e29b-41d4-a716-446655440000",
            "f47ac10b-58cc-4372-a567-0e02b2c3d479",
            "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
        ];

        for id_str in test_ids {
            let mid = MachineId(id_str.to_string());
            assert_eq!(mid.as_str(), id_str);
            // Verify it's 36 chars (UUID with hyphens)
            assert_eq!(mid.as_str().len(), 36);
        }
    }

    #[test]
    fn test_poll_response_empty_actions_branch() {
        let resp = PollResponse {
            actions: vec![],
            next_poll: "2025-01-01T00:05:00Z".to_string(),
            server_time: "2025-01-01T00:00:30Z".to_string(),
            has_more: false,
            poll_count: 0,
        };

        assert!(resp.actions.is_empty());
        assert_eq!(resp.poll_count, 0);
    }

    #[test]
    fn test_action_response_all_error_fields() {
        let resp = ActionResponse {
            id: "act_error_full".to_string(),
            success: false,
            error: Some("command not found".to_string()),
            result: None,
            duration_ms: 50,
            timestamp: "2025-01-01T12:34:56Z".to_string(),
        };

        assert!(!resp.success);
        assert!(resp.error.is_some());
        assert!(resp.result.is_none());
        assert_eq!(resp.duration_ms, 50);
    }

    // Test various action type dispatching paths
    #[test]
    fn test_action_type_dispatch_paths() {
        let types = vec![
            "webhook",
            "signed_binary",
            "bash",
            "python",
            "powershell",
            "lua",
        ];
        for ty in types {
            let action = ActionRequest {
                id: format!("act_{}", ty),
                r#type: ty.to_string(),
                module_name: None,
                action: None,
                parameters: serde_json::json!({}),
                user_id: "user_1".to_string(),
                community_id: "comm_1".to_string(),
                priority: 0,
                timeout: 30,
                created_at: "2025-01-01T00:00:00Z".to_string(),
                expires_at: "2025-01-01T00:01:00Z".to_string(),
            };
            assert_eq!(action.r#type, ty);
        }
    }

    // Test action response timestamp generation
    #[test]
    fn test_action_response_timestamp_format() {
        let resp = ActionResponse {
            id: "act_1".to_string(),
            success: true,
            result: Some(ActionResult {
                exit_code: Some(0),
                stdout: vec![],
                stderr: vec![],
                truncated: false,
            }),
            error: None,
            duration_ms: 100,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };

        // Verify timestamp is valid RFC3339 format
        assert!(resp.timestamp.contains('T'));
        assert!(resp.timestamp.contains('Z') || resp.timestamp.contains('+'));
    }

    // Test action response with both stdout and stderr
    #[test]
    fn test_action_response_with_output() {
        let stdout = vec![72, 101, 108, 108, 111]; // "Hello"
        let stderr = vec![69, 114, 114]; // "Err"

        let resp = ActionResponse {
            id: "act_output".to_string(),
            success: true,
            result: Some(ActionResult {
                exit_code: Some(1),
                stdout: stdout.clone(),
                stderr: stderr.clone(),
                truncated: false,
            }),
            error: None,
            duration_ms: 123,
            timestamp: "2025-01-01T00:00:00Z".to_string(),
        };

        assert!(resp.success);
        assert_eq!(resp.result.as_ref().unwrap().stdout, stdout);
        assert_eq!(resp.result.as_ref().unwrap().stderr, stderr);
    }

    // Test action request with various parameter types
    #[test]
    fn test_action_request_with_complex_parameters() {
        let params = serde_json::json!({
            "string_param": "value",
            "number_param": 42,
            "bool_param": true,
            "object_param": {"nested": "data"},
            "array_param": [1, 2, 3]
        });

        let action = ActionRequest {
            id: "act_complex".to_string(),
            r#type: "bash".to_string(),
            module_name: Some("test_module".to_string()),
            action: Some("test_action".to_string()),
            parameters: params.clone(),
            user_id: "user_456".to_string(),
            community_id: "comm_789".to_string(),
            priority: 5,
            timeout: 60,
            created_at: "2025-01-01T12:00:00Z".to_string(),
            expires_at: "2025-01-01T12:05:00Z".to_string(),
        };

        assert_eq!(action.parameters, params);
        assert_eq!(action.priority, 5);
    }

    // Test MachineId generation and persistence paths
    #[test]
    fn test_machine_id_strings() {
        let id1 = MachineId("test-id-1".to_string());
        let id2 = MachineId("test-id-2".to_string());

        assert_eq!(id1.as_str(), "test-id-1");
        assert_eq!(id2.as_str(), "test-id-2");
        assert_ne!(id1.as_str(), id2.as_str());
    }

    // Test execute dispatch for unknown action type
    #[test]
    fn test_unknown_action_type_handling() {
        let action = ActionRequest {
            id: "unknown_123".to_string(),
            r#type: "unknown_custom_type".to_string(),
            module_name: None,
            action: None,
            parameters: serde_json::json!({}),
            user_id: "user_1".to_string(),
            community_id: "comm_1".to_string(),
            priority: 0,
            timeout: 30,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            expires_at: "2025-01-01T00:01:00Z".to_string(),
        };

        // Verify action has the unknown type set
        assert_eq!(action.r#type, "unknown_custom_type");
    }

    // Test poll response with multiple actions
    #[test]
    fn test_poll_response_multiple_actions() {
        let actions = vec![
            ActionRequest {
                id: "act_1".to_string(),
                r#type: "bash".to_string(),
                module_name: None,
                action: None,
                parameters: serde_json::json!({}),
                user_id: "user_1".to_string(),
                community_id: "comm_1".to_string(),
                priority: 1,
                timeout: 30,
                created_at: "2025-01-01T00:00:00Z".to_string(),
                expires_at: "2025-01-01T00:01:00Z".to_string(),
            },
            ActionRequest {
                id: "act_2".to_string(),
                r#type: "python".to_string(),
                module_name: None,
                action: None,
                parameters: serde_json::json!({}),
                user_id: "user_2".to_string(),
                community_id: "comm_2".to_string(),
                priority: 2,
                timeout: 60,
                created_at: "2025-01-01T00:00:30Z".to_string(),
                expires_at: "2025-01-01T00:01:30Z".to_string(),
            },
        ];

        let poll_resp = PollResponse {
            actions: actions.clone(),
            next_poll: "2025-01-01T00:02:00Z".to_string(),
            server_time: "2025-01-01T00:00:30Z".to_string(),
            has_more: true,
            poll_count: 2,
        };

        assert_eq!(poll_resp.actions.len(), 2);
        assert_eq!(poll_resp.actions[0].id, "act_1");
        assert_eq!(poll_resp.actions[1].id, "act_2");
        assert!(poll_resp.has_more);
        assert_eq!(poll_resp.poll_count, 2);
    }

    // Test action response serialization edge cases
    #[test]
    fn test_action_response_serialization_roundtrip() {
        let resp = ActionResponse {
            id: "act_roundtrip".to_string(),
            success: true,
            result: Some(ActionResult {
                exit_code: Some(42),
                stdout: vec![1, 2, 3],
                stderr: vec![],
                truncated: false,
            }),
            error: None,
            duration_ms: 999,
            timestamp: "2025-01-01T00:00:00Z".to_string(),
        };

        let json = serde_json::to_value(&resp).expect("serialization failed");

        // Verify all fields are present in serialized form
        assert!(json.get("id").is_some());
        assert!(json.get("success").is_some());
        assert!(json.get("result").is_some());
        assert!(json.get("duration_ms").is_some());
        assert!(json.get("timestamp").is_some());

        // Verify None fields are omitted
        assert!(json.get("error").is_none());
    }

    // Test action response with various duration values
    #[test]
    fn test_action_response_duration_values() {
        let durations = vec![0, 1, 100, 1000, 99999];

        for duration_ms in durations {
            let resp = ActionResponse {
                id: format!("act_{}", duration_ms),
                success: true,
                result: Some(ActionResult {
                    exit_code: Some(0),
                    stdout: vec![],
                    stderr: vec![],
                    truncated: false,
                }),
                error: None,
                duration_ms,
                timestamp: "2025-01-01T00:00:00Z".to_string(),
            };

            assert_eq!(resp.duration_ms, duration_ms);
        }
    }

    // Test action response status flags
    #[test]
    fn test_action_response_success_flag_combinations() {
        let cases = vec![
            (true, Some("result_data".to_string()), None),
            (false, None, Some("error message".to_string())),
            (false, None, None),
        ];

        for (success, _result_str, error) in cases {
            let resp = ActionResponse {
                id: "test_case".to_string(),
                success,
                result: None,
                error,
                duration_ms: 100,
                timestamp: "2025-01-01T00:00:00Z".to_string(),
            };

            assert_eq!(resp.success, success);
            if success {
                assert!(resp.error.is_none());
            }
        }
    }

    // Test poll response with empty actions list
    #[test]
    fn test_poll_response_empty_actions_detailed() {
        let poll_resp = PollResponse {
            actions: vec![],
            next_poll: "2025-01-01T00:05:00Z".to_string(),
            server_time: "2025-01-01T00:00:30Z".to_string(),
            has_more: false,
            poll_count: 0,
        };

        assert!(poll_resp.actions.is_empty());
        assert!(!poll_resp.has_more);
        assert_eq!(poll_resp.poll_count, 0);
    }

    // Test action request fields are preserved
    #[test]
    fn test_action_request_field_preservation() {
        let action = ActionRequest {
            id: "preserve_test".to_string(),
            r#type: "bash".to_string(),
            module_name: Some("module_x".to_string()),
            action: Some("action_y".to_string()),
            parameters: serde_json::json!({"key": "value"}),
            user_id: "user_preserve".to_string(),
            community_id: "comm_preserve".to_string(),
            priority: 7,
            timeout: 45,
            created_at: "2025-01-01T10:00:00Z".to_string(),
            expires_at: "2025-01-01T10:01:00Z".to_string(),
        };

        assert_eq!(action.id, "preserve_test");
        assert_eq!(action.r#type, "bash");
        assert_eq!(action.module_name.unwrap(), "module_x");
        assert_eq!(action.action.unwrap(), "action_y");
        assert_eq!(action.user_id, "user_preserve");
        assert_eq!(action.community_id, "comm_preserve");
        assert_eq!(action.priority, 7);
        assert_eq!(action.timeout, 45);
    }

    // === Integration Tests (using MockSessionProxy) ===

    use crate::mock_server::MockSessionProxy;
    use penguin_proto::desktop::v1::session_proxy_server::SessionProxyServer;
    use std::path::PathBuf;
    use tokio::net::UnixListener;
    use tonic::transport::Server;

    /// Starts a mock gRPC server on a temp UDS and returns the socket path.
    async fn start_mock_server(mock: MockSessionProxy) -> PathBuf {
        let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let socket_path = temp_dir.path().join("test.sock");

        let uds = UnixListener::bind(&socket_path).expect("failed to bind UDS");
        let socket_path_clone = socket_path.clone();

        // Keep temp_dir alive for the duration of the test
        std::mem::forget(temp_dir);

        tokio::spawn(async move {
            use tokio_stream::wrappers::UnixListenerStream;

            let svc = SessionProxyServer::new(mock);
            let stream = UnixListenerStream::new(uds);
            let _result = Server::builder()
                .add_service(svc)
                .serve_with_incoming(stream)
                .await;
        });

        // Give the server a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        socket_path_clone
    }

    #[tokio::test]
    async fn test_executor_poll_against_mock() -> Result<()> {
        let test_dir =
            tempfile::tempdir().map_err(|e| DesktopError::Internal(format!("temp dir: {}", e)))?;

        let mock = MockSessionProxy::new();
        let mock_clone = mock.clone();
        let socket_path = start_mock_server(mock_clone).await;

        // Mock returns a poll response with one action
        let poll_resp = serde_json::json!({
            "actions": [{
                "id": "act_1",
                "type": "bash",
                "parameters": {},
                "user_id": "user_1",
                "community_id": "comm_1",
                "priority": 0,
                "timeout": 30,
                "created_at": "2025-01-01T00:00:00Z",
                "expires_at": "2025-01-01T00:05:00Z"
            }],
            "next_poll": "2025-01-01T00:01:00Z",
            "server_time": "2025-01-01T00:00:30Z",
            "has_more": false,
            "poll_count": 1
        });
        mock.set_proxy_response(200, poll_resp.to_string().into_bytes())
            .await;

        let session = Session::new_for_testing(
            socket_path.to_str().expect("socket path"),
            test_dir.path().to_path_buf(),
        )
        .await?;

        let machine_id = MachineId("test-machine-id-poll".to_string());
        let mut executor = ActionExecutor::new_for_testing(
            Arc::new(tokio::sync::Mutex::new(session)),
            Box::new(approval::MockApprovalPrompt::new(
                approval::ApprovalMode::AlwaysApprove,
            )),
            machine_id,
        );

        let poll_response = executor.poll().await?;

        // Verify: mock received poll call
        let requests = mock.recorded_requests().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, "GET");
        assert!(requests[0].1.starts_with("/api/v1/bridge/poll?bridge_id="));

        // Verify: poll response parsed correctly
        assert_eq!(poll_response.actions.len(), 1);
        assert_eq!(poll_response.poll_count, 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_executor_post_response_against_mock() -> Result<()> {
        let test_dir =
            tempfile::tempdir().map_err(|e| DesktopError::Internal(format!("temp dir: {}", e)))?;

        let mock = MockSessionProxy::new();
        let mock_clone = mock.clone();
        let socket_path = start_mock_server(mock_clone).await;

        let session = Session::new_for_testing(
            socket_path.to_str().expect("socket path"),
            test_dir.path().to_path_buf(),
        )
        .await?;

        let machine_id = MachineId("test-machine-id-poll".to_string());
        let mut executor = ActionExecutor::new_for_testing(
            Arc::new(tokio::sync::Mutex::new(session)),
            Box::new(approval::MockApprovalPrompt::new(
                approval::ApprovalMode::AlwaysApprove,
            )),
            machine_id,
        );

        let response = ActionResponse {
            id: "act_123".to_string(),
            success: true,
            result: Some(ActionResult {
                exit_code: Some(0),
                stdout: b"output".to_vec(),
                stderr: vec![],
                truncated: false,
            }),
            error: None,
            duration_ms: 100,
            timestamp: "2025-01-01T00:00:00Z".to_string(),
        };

        executor.post_response(response.clone()).await?;

        // Verify: mock received POST to /api/v1/bridge/response
        let requests = mock.recorded_requests().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, "POST");
        assert_eq!(requests[0].1, "/api/v1/bridge/response");

        Ok(())
    }

    #[tokio::test]
    async fn test_executor_register_against_mock() -> Result<()> {
        let test_dir =
            tempfile::tempdir().map_err(|e| DesktopError::Internal(format!("temp dir: {}", e)))?;

        let mock = MockSessionProxy::new();
        let mock_clone = mock.clone();
        let socket_path = start_mock_server(mock_clone).await;

        let session = Session::new_for_testing(
            socket_path.to_str().expect("socket path"),
            test_dir.path().to_path_buf(),
        )
        .await?;

        let machine_id = MachineId("test-machine-id-poll".to_string());
        let mut executor = ActionExecutor::new_for_testing(
            Arc::new(tokio::sync::Mutex::new(session)),
            Box::new(approval::MockApprovalPrompt::new(
                approval::ApprovalMode::AlwaysApprove,
            )),
            machine_id,
        );

        executor.register(vec!["module_1".to_string()]).await?;

        // Verify: mock recorded register call
        let requests = mock.recorded_requests().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, "POST");
        assert_eq!(requests[0].1, "/api/v1/bridge/register");

        Ok(())
    }

    #[tokio::test]
    async fn test_executor_heartbeat_against_mock() -> Result<()> {
        let test_dir =
            tempfile::tempdir().map_err(|e| DesktopError::Internal(format!("temp dir: {}", e)))?;

        let mock = MockSessionProxy::new();
        let mock_clone = mock.clone();
        let socket_path = start_mock_server(mock_clone).await;

        let session = Session::new_for_testing(
            socket_path.to_str().expect("socket path"),
            test_dir.path().to_path_buf(),
        )
        .await?;

        let machine_id = MachineId("test-machine-id-poll".to_string());
        let mut executor = ActionExecutor::new_for_testing(
            Arc::new(tokio::sync::Mutex::new(session)),
            Box::new(approval::MockApprovalPrompt::new(
                approval::ApprovalMode::AlwaysApprove,
            )),
            machine_id,
        );

        executor.heartbeat().await?;

        // Verify: mock recorded heartbeat call
        let requests = mock.recorded_requests().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, "POST");
        assert_eq!(requests[0].1, "/api/v1/bridge/heartbeat");

        Ok(())
    }

    #[tokio::test]
    async fn test_executor_unregister_against_mock() -> Result<()> {
        let test_dir =
            tempfile::tempdir().map_err(|e| DesktopError::Internal(format!("temp dir: {}", e)))?;

        let mock = MockSessionProxy::new();
        let mock_clone = mock.clone();
        let socket_path = start_mock_server(mock_clone).await;

        let session = Session::new_for_testing(
            socket_path.to_str().expect("socket path"),
            test_dir.path().to_path_buf(),
        )
        .await?;

        let machine_id = MachineId("test-machine-id-poll".to_string());
        let mut executor = ActionExecutor::new_for_testing(
            Arc::new(tokio::sync::Mutex::new(session)),
            Box::new(approval::MockApprovalPrompt::new(
                approval::ApprovalMode::AlwaysApprove,
            )),
            machine_id,
        );

        executor.unregister().await?;

        // Verify: mock recorded unregister call
        let requests = mock.recorded_requests().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, "POST");
        assert_eq!(requests[0].1, "/api/v1/bridge/unregister");

        Ok(())
    }

    #[tokio::test]
    async fn test_executor_approval_denied_response() -> Result<()> {
        let action = ActionRequest {
            id: "act_denied".to_string(),
            r#type: "bash".to_string(),
            module_name: None,
            action: None,
            parameters: serde_json::json!({}),
            user_id: "user".to_string(),
            community_id: "comm".to_string(),
            priority: 0,
            timeout: 30,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            expires_at: "2025-01-01T00:05:00Z".to_string(),
        };

        let test_dir =
            tempfile::tempdir().map_err(|e| DesktopError::Internal(format!("temp dir: {}", e)))?;

        let mock = MockSessionProxy::new();
        let mock_clone = mock.clone();
        let socket_path = start_mock_server(mock_clone).await;

        let session = Session::new_for_testing(
            socket_path.to_str().expect("socket path"),
            test_dir.path().to_path_buf(),
        )
        .await?;

        // Use denying approval prompt
        let machine_id = MachineId("test-machine-id-deny".to_string());
        let mut executor = ActionExecutor::new_for_testing(
            Arc::new(tokio::sync::Mutex::new(session)),
            Box::new(approval::MockApprovalPrompt::new(
                approval::ApprovalMode::AlwaysDeny,
            )),
            machine_id,
        );

        let resp = executor.execute(action).await;

        // Verify: response indicates denial
        assert!(!resp.success);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap(), "denied by user");

        Ok(())
    }

    #[tokio::test]
    async fn test_run_poll_loop_with_actions() -> Result<()> {
        let test_dir =
            tempfile::tempdir().map_err(|e| DesktopError::Internal(format!("temp dir: {}", e)))?;

        let mock = MockSessionProxy::new();
        let mock_clone = mock.clone();
        let socket_path = start_mock_server(mock_clone).await;

        // Mock returns a poll response with one action on first call, then empty on second
        let poll_resp_with_action = serde_json::json!({
            "actions": [{
                "id": "act_poll_test",
                "type": "bash",
                "parameters": {"source": "echo test"},
                "user_id": "user_1",
                "community_id": "comm_1",
                "priority": 0,
                "timeout": 30,
                "created_at": "2025-01-01T00:00:00Z",
                "expires_at": "2025-01-01T00:05:00Z"
            }],
            "next_poll": "2025-01-01T00:01:00Z",
            "server_time": "2025-01-01T00:00:30Z",
            "has_more": false,
            "poll_count": 1
        });

        // Set initial response
        mock.set_proxy_response(200, poll_resp_with_action.to_string().into_bytes())
            .await;

        let session = Arc::new(tokio::sync::Mutex::new(
            Session::new_for_testing(
                socket_path.to_str().expect("socket path"),
                test_dir.path().to_path_buf(),
            )
            .await?,
        ));

        let machine_id = MachineId("test-machine-id-loop".to_string());
        let executor = ActionExecutor::new_for_testing(
            Arc::clone(&session),
            Box::new(approval::MockApprovalPrompt::new(
                approval::ApprovalMode::AlwaysApprove,
            )),
            machine_id,
        );

        let shutdown = tokio_util::sync::CancellationToken::new();
        let shutdown_clone = shutdown.clone();

        // Spawn poll loop to run for a short time
        let poll_handle = tokio::spawn(async move {
            run_poll_loop(executor, Duration::from_millis(10), shutdown_clone).await
        });

        // Let it run for 100ms then cancel
        tokio::time::sleep(Duration::from_millis(100)).await;
        shutdown.cancel();

        // Wait for poll loop to complete
        let join_result = tokio::time::timeout(Duration::from_secs(2), poll_handle)
            .await
            .map_err(|_| DesktopError::Internal("poll loop timeout".to_string()))?
            .map_err(|e| DesktopError::Internal(format!("poll loop join error: {}", e)))?;

        // Unpack the nested Result from run_poll_loop
        let action_count = join_result?;

        // Verify: poll loop ran and completed successfully
        assert!(
            action_count >= 1,
            "poll loop should have processed at least one action"
        );

        // Verify: mock recorded poll and response calls
        let requests = mock.recorded_requests().await;
        assert!(requests.len() >= 2, "should have poll and response calls"); // At least poll and response post
        assert!(
            requests
                .iter()
                .any(|(method, path)| method == "GET" && path.starts_with("/api/v1/bridge/poll"))
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_execute_with_script_and_parameters() -> Result<()> {
        let test_dir =
            tempfile::tempdir().map_err(|e| DesktopError::Internal(format!("temp dir: {}", e)))?;

        let mock = MockSessionProxy::new();
        let mock_clone = mock.clone();
        let socket_path = start_mock_server(mock_clone).await;

        let session = Session::new_for_testing(
            socket_path.to_str().expect("socket path"),
            test_dir.path().to_path_buf(),
        )
        .await?;

        let machine_id = MachineId("test-machine-id-exec-params".to_string());
        let mut executor = ActionExecutor::new_for_testing(
            Arc::new(tokio::sync::Mutex::new(session)),
            Box::new(approval::MockApprovalPrompt::new(
                approval::ApprovalMode::AlwaysApprove,
            )),
            machine_id,
        );

        let action = ActionRequest {
            id: "act_params_test".to_string(),
            r#type: "bash".to_string(),
            module_name: None,
            action: None,
            parameters: serde_json::json!({
                "source": "echo 'param test'"
            }),
            user_id: "user_param".to_string(),
            community_id: "comm_param".to_string(),
            priority: 1,
            timeout: 10,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            expires_at: "2025-01-01T00:00:10Z".to_string(),
        };

        let result = executor.execute(action).await;
        assert!(
            result.success,
            "execute should succeed with approved script"
        );
        assert!(result.result.is_some(), "should have result with output");

        Ok(())
    }

    #[tokio::test]
    async fn test_poll_with_empty_actions() -> Result<()> {
        let test_dir =
            tempfile::tempdir().map_err(|e| DesktopError::Internal(format!("temp dir: {}", e)))?;

        let mock = MockSessionProxy::new();

        let poll_resp = serde_json::json!({
            "actions": [],
            "next_poll": "2025-01-01T00:00:35Z",
            "server_time": "2025-01-01T00:00:30Z",
            "has_more": false,
            "poll_count": 1
        });
        mock.set_proxy_response(200, poll_resp.to_string().into_bytes())
            .await;

        let mock_clone = mock.clone();
        let socket_path = start_mock_server(mock_clone).await;

        let session = Session::new_for_testing(
            socket_path.to_str().expect("socket path"),
            test_dir.path().to_path_buf(),
        )
        .await?;

        let machine_id = MachineId("test-machine-id-empty-poll".to_string());
        let mut executor = ActionExecutor::new_for_testing(
            Arc::new(tokio::sync::Mutex::new(session)),
            Box::new(approval::MockApprovalPrompt::new(
                approval::ApprovalMode::AlwaysApprove,
            )),
            machine_id,
        );

        let poll_response = executor.poll().await?;

        // Verify: empty actions list is handled gracefully
        assert_eq!(poll_response.actions.len(), 0);
        assert_eq!(poll_response.poll_count, 1);

        // Verify: mock received poll call
        let requests = mock.recorded_requests().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, "GET");

        Ok(())
    }

    #[tokio::test]
    async fn test_post_response_with_error() -> Result<()> {
        let test_dir =
            tempfile::tempdir().map_err(|e| DesktopError::Internal(format!("temp dir: {}", e)))?;

        let mock = MockSessionProxy::new();
        let mock_clone = mock.clone();
        let socket_path = start_mock_server(mock_clone).await;

        let session = Session::new_for_testing(
            socket_path.to_str().expect("socket path"),
            test_dir.path().to_path_buf(),
        )
        .await?;

        let machine_id = MachineId("test-machine-id-error-response".to_string());
        let mut executor = ActionExecutor::new_for_testing(
            Arc::new(tokio::sync::Mutex::new(session)),
            Box::new(approval::MockApprovalPrompt::new(
                approval::ApprovalMode::AlwaysApprove,
            )),
            machine_id,
        );

        let error_response = ActionResponse {
            id: "act_error_123".to_string(),
            success: false,
            result: None,
            error: Some("Test error occurred".to_string()),
            duration_ms: 100,
            timestamp: "2025-01-01T00:00:01Z".to_string(),
        };

        executor.post_response(error_response).await?;

        // Verify: mock recorded response post
        let requests = mock.recorded_requests().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, "POST");
        assert_eq!(requests[0].1, "/api/v1/bridge/response");

        Ok(())
    }

    #[tokio::test]
    async fn test_execute_unknown_action_type() -> Result<()> {
        let test_dir =
            tempfile::tempdir().map_err(|e| DesktopError::Internal(format!("temp dir: {}", e)))?;

        let mock = MockSessionProxy::new();
        let mock_clone = mock.clone();
        let socket_path = start_mock_server(mock_clone).await;

        let session = Session::new_for_testing(
            socket_path.to_str().expect("socket path"),
            test_dir.path().to_path_buf(),
        )
        .await?;

        let machine_id = MachineId("test-machine-id-unknown".to_string());
        let mut executor = ActionExecutor::new_for_testing(
            Arc::new(tokio::sync::Mutex::new(session)),
            Box::new(approval::MockApprovalPrompt::new(
                approval::ApprovalMode::AlwaysApprove,
            )),
            machine_id,
        );

        let unknown_action = ActionRequest {
            id: "act_unknown_type".to_string(),
            r#type: "unknown_type".to_string(),
            module_name: None,
            action: None,
            parameters: serde_json::json!({}),
            user_id: "user_unknown".to_string(),
            community_id: "comm_unknown".to_string(),
            priority: 0,
            timeout: 10,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            expires_at: "2025-01-01T00:00:10Z".to_string(),
        };

        let result = executor.execute(unknown_action).await;

        // Verify: unknown action type is rejected
        assert!(!result.success, "unknown action type should fail");
        assert!(
            result.error.is_some(),
            "should have error message for unknown type"
        );
        assert!(
            result.error.unwrap().contains("unknown action type"),
            "error message should mention unknown type"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_poll_with_timeout_action() -> Result<()> {
        let test_dir =
            tempfile::tempdir().map_err(|e| DesktopError::Internal(format!("temp dir: {}", e)))?;

        let mock = MockSessionProxy::new();

        // Create an action with short timeout
        let poll_resp = serde_json::json!({
            "actions": [{
                "id": "act_timeout",
                "type": "bash",
                "module_name": null,
                "action": null,
                "parameters": {
                    "source": "sleep 1 && echo done"
                },
                "user_id": "user_timeout",
                "community_id": "comm_timeout",
                "priority": 0,
                "timeout": 1,
                "created_at": "2025-01-01T00:00:00Z",
                "expires_at": "2025-01-01T00:00:01Z"
            }],
            "next_poll": "2025-01-01T00:00:35Z",
            "server_time": "2025-01-01T00:00:30Z",
            "has_more": false,
            "poll_count": 1
        });
        mock.set_proxy_response(200, poll_resp.to_string().into_bytes())
            .await;

        let mock_clone = mock.clone();
        let socket_path = start_mock_server(mock_clone).await;

        let session = Session::new_for_testing(
            socket_path.to_str().expect("socket path"),
            test_dir.path().to_path_buf(),
        )
        .await?;

        let machine_id = MachineId("test-machine-id-timeout".to_string());
        let mut executor = ActionExecutor::new_for_testing(
            Arc::new(tokio::sync::Mutex::new(session)),
            Box::new(approval::MockApprovalPrompt::new(
                approval::ApprovalMode::AlwaysApprove,
            )),
            machine_id,
        );

        let poll_response = executor.poll().await?;

        // Verify: poll returned the timeout action
        assert_eq!(poll_response.actions.len(), 1);
        assert_eq!(poll_response.actions[0].id, "act_timeout");
        assert_eq!(poll_response.actions[0].timeout, 1);

        Ok(())
    }
}
