//! Approval-prompt trait and types for script/Lua execution gates.
//!
//! The `ApprovalPrompt` trait is the seam between the executor core (which has no
//! Tauri dependency) and the shell (which implements this trait with a real GUI dialog).
//!
//! Tests use a mock implementation that can be configured to approve/deny/timeout.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::ActionRequest;

/// A pending action awaiting user approval (presented to the approval prompt).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingAction {
    pub id: String,
    pub action_type: String, // "bash", "powershell", "python", "lua"
    pub content: String,     // Full script/Lua source to display to the user
    pub user_id: String,
    pub community_id: String,
    pub timeout_secs: i32,
}

impl PendingAction {
    /// Constructs a pending action from a hub action request.
    /// Extracts the full script/Lua source from `parameters["source"]` or `parameters["content"]`.
    pub fn from_request(action: &ActionRequest) -> Self {
        let content = action
            .parameters
            .get("source")
            .or_else(|| action.parameters.get("content"))
            .and_then(|v| v.as_str())
            .unwrap_or("<script content not found>")
            .to_string();

        PendingAction {
            id: action.id.clone(),
            action_type: action.r#type.clone(),
            content,
            user_id: action.user_id.clone(),
            community_id: action.community_id.clone(),
            timeout_secs: action.timeout,
        }
    }
}

/// User's decision on whether to approve an action.
#[derive(Clone, Debug)]
pub enum ApprovalDecision {
    /// User explicitly approved the action.
    Approved,
    /// User explicitly denied the action.
    Denied,
    /// User did not respond before the deadline.
    Timeout,
}

/// Trait for prompting the user to approve or deny a pending action.
/// The shell (Tauri) implements this with a real dialog;
/// tests use a mock implementation.
#[async_trait]
pub trait ApprovalPrompt: Send + Sync {
    /// Asks the user whether to approve the given action.
    /// Must fail closed: no response, dismissal, or timeout → Timeout decision (never auto-approve).
    async fn ask(&self, action: &PendingAction) -> ApprovalDecision;
}

/// Mock approval prompt for testing: always approves (or can be configured to deny).
#[cfg(test)]
pub struct MockApprovalPrompt {
    mode: std::sync::Arc<tokio::sync::Mutex<ApprovalMode>>,
}

#[cfg(test)]
pub enum ApprovalMode {
    AlwaysApprove,
    AlwaysDeny,
    Timeout,
}

#[cfg(test)]
impl MockApprovalPrompt {
    pub fn new(mode: ApprovalMode) -> Self {
        MockApprovalPrompt {
            mode: std::sync::Arc::new(tokio::sync::Mutex::new(mode)),
        }
    }

    pub async fn set_mode(&self, mode: ApprovalMode) {
        *self.mode.lock().await = mode;
    }
}

#[cfg(test)]
#[async_trait]
impl ApprovalPrompt for MockApprovalPrompt {
    async fn ask(&self, _action: &PendingAction) -> ApprovalDecision {
        match *self.mode.lock().await {
            ApprovalMode::AlwaysApprove => ApprovalDecision::Approved,
            ApprovalMode::AlwaysDeny => ApprovalDecision::Denied,
            ApprovalMode::Timeout => ApprovalDecision::Timeout,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pending_action_from_request() {
        let action = ActionRequest {
            id: "act_123".to_string(),
            r#type: "bash".to_string(),
            module_name: None,
            action: None,
            parameters: serde_json::json!({
                "source": "echo hello",
            }),
            user_id: "user_456".to_string(),
            community_id: "comm_789".to_string(),
            priority: 0,
            timeout: 30,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            expires_at: "2025-01-01T00:05:00Z".to_string(),
        };

        let pending = PendingAction::from_request(&action);
        assert_eq!(pending.id, "act_123");
        assert_eq!(pending.action_type, "bash");
        assert_eq!(pending.content, "echo hello");
        assert_eq!(pending.timeout_secs, 30);
    }

    #[tokio::test]
    async fn test_mock_approval_prompt_approve() {
        let mock = MockApprovalPrompt::new(ApprovalMode::AlwaysApprove);
        let action = PendingAction {
            id: "act_test".to_string(),
            action_type: "bash".to_string(),
            content: "echo test".to_string(),
            user_id: "user_test".to_string(),
            community_id: "comm_test".to_string(),
            timeout_secs: 30,
        };

        let decision = mock.ask(&action).await;
        assert!(matches!(decision, ApprovalDecision::Approved));
    }

    #[tokio::test]
    async fn test_mock_approval_prompt_deny() {
        let mock = MockApprovalPrompt::new(ApprovalMode::AlwaysDeny);
        let action = PendingAction {
            id: "act_test".to_string(),
            action_type: "bash".to_string(),
            content: "echo test".to_string(),
            user_id: "user_test".to_string(),
            community_id: "comm_test".to_string(),
            timeout_secs: 30,
        };

        let decision = mock.ask(&action).await;
        assert!(matches!(decision, ApprovalDecision::Denied));
    }

    #[tokio::test]
    async fn test_mock_approval_prompt_timeout() {
        let mock = MockApprovalPrompt::new(ApprovalMode::Timeout);
        let action = PendingAction {
            id: "act_test".to_string(),
            action_type: "lua".to_string(),
            content: "print('test')".to_string(),
            user_id: "user_test".to_string(),
            community_id: "comm_test".to_string(),
            timeout_secs: 5,
        };

        let decision = mock.ask(&action).await;
        assert!(matches!(decision, ApprovalDecision::Timeout));
    }

    #[tokio::test]
    async fn test_mock_approval_prompt_mode_switch() {
        let mock = MockApprovalPrompt::new(ApprovalMode::AlwaysApprove);
        let action = PendingAction {
            id: "act_test".to_string(),
            action_type: "bash".to_string(),
            content: "echo test".to_string(),
            user_id: "user_test".to_string(),
            community_id: "comm_test".to_string(),
            timeout_secs: 30,
        };

        // Start in approve mode
        let decision = mock.ask(&action).await;
        assert!(matches!(decision, ApprovalDecision::Approved));

        // Switch to deny mode
        mock.set_mode(ApprovalMode::AlwaysDeny).await;
        let decision = mock.ask(&action).await;
        assert!(matches!(decision, ApprovalDecision::Denied));
    }
}
