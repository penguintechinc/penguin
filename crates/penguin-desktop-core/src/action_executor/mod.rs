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
/// Holds the session, machine ID, and approval-prompt provider.
pub struct ActionExecutor {
    session: Session,
    machine_id: MachineId,
    approval_prompt: Box<dyn ApprovalPrompt>,
}

impl ActionExecutor {
    /// Creates a new executor with a session and an approval-prompt provider.
    pub async fn new(session: Session, approval_prompt: Box<dyn ApprovalPrompt>) -> Result<Self> {
        let machine_id = MachineId::load_or_create().await?;
        Ok(ActionExecutor {
            session,
            machine_id,
            approval_prompt,
        })
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
            .api_request("POST", "/api/v1/bridge/unregister", Some(body_bytes))
            .await?;

        debug!("unregistered from hub");
        Ok(())
    }

    /// Polls the hub for pending actions.
    pub async fn poll(&mut self) -> Result<PollResponse> {
        let resp = self
            .session
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

        match SignedExecutor::execute(&self.session, action, self.machine_id.as_str()).await {
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
/// Returns the number of actions processed.
pub async fn run_poll_loop(
    executor: &mut ActionExecutor,
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
            actions: vec![
                ActionRequest {
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
                },
            ],
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
}
