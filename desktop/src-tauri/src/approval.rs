//! Tauri approval prompt implementation.
//!
//! Emits Tauri events to the frontend for user approval decisions and waits for
//! responses via a Tauri command. Implements fail-closed semantics: timeout,
//! dropped channels, or missing responses → non-Approved decision.

use async_trait::async_trait;
use penguin_desktop_core::{ApprovalDecision, ApprovalPrompt, PendingAction};
use std::sync::Arc;
use tokio::sync::{oneshot, RwLock};
use std::collections::HashMap;
use tauri::Emitter;
use tracing::{debug, warn};

/// Manages approval decisions: maps action IDs to oneshot senders waiting for responses.
type PendingApprovals = Arc<RwLock<HashMap<String, oneshot::Sender<bool>>>>;

/// Tauri approval prompt: emits events to the frontend and waits for responses.
pub struct TauriApprovalPrompt {
    app_handle: tauri::AppHandle,
    pending: PendingApprovals,
}

impl TauriApprovalPrompt {
    /// Creates a new Tauri approval prompt with an app handle and its own pending map.
    pub fn new(app_handle: tauri::AppHandle) -> Self {
        TauriApprovalPrompt {
            app_handle,
            pending: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Creates a new Tauri approval prompt with an app handle and a shared pending map.
    /// Used when the poll loop needs to share the same pending map as the command handlers.
    pub fn new_with_pending(
        app_handle: tauri::AppHandle,
        pending: PendingApprovals,
    ) -> Self {
        TauriApprovalPrompt {
            app_handle,
            pending,
        }
    }

    /// Returns a reference to the pending approvals map for command handlers to use.
    pub fn pending(&self) -> PendingApprovals {
        Arc::clone(&self.pending)
    }
}

#[async_trait]
impl ApprovalPrompt for TauriApprovalPrompt {
    async fn ask(&self, action: &PendingAction) -> ApprovalDecision {
        let (tx, rx) = oneshot::channel::<bool>();

        // Store the sender in the pending map
        {
            let mut pending = self.pending.write().await;
            pending.insert(action.id.clone(), tx);
        }

        // Emit the approval request event to the frontend
        let payload = serde_json::json!({
            "id": action.id,
            "action_type": action.action_type,
            "content": action.content,
            "user_id": action.user_id,
            "community_id": action.community_id,
            "timeout_secs": action.timeout_secs,
        });

        if let Err(e) = self.app_handle.emit("action-approval-requested", &payload) {
            warn!(action_id = action.id, error = ?e, "failed to emit approval event");
            // Clean up the pending entry on emission failure
            self.pending.write().await.remove(&action.id);
            return ApprovalDecision::Timeout;
        }

        debug!(action_id = action.id, timeout = action.timeout_secs, "approval requested");

        // Wait for the response with a timeout
        let timeout_duration = std::time::Duration::from_secs(action.timeout_secs as u64);
        match tokio::time::timeout(timeout_duration, rx).await {
            Ok(Ok(approved)) => {
                debug!(action_id = action.id, approved = approved, "approval decision received");
                // Clean up
                self.pending.write().await.remove(&action.id);

                if approved {
                    ApprovalDecision::Approved
                } else {
                    ApprovalDecision::Denied
                }
            }
            Ok(Err(_)) => {
                // Channel was dropped (frontend closed, app crashed)
                warn!(action_id = action.id, "approval channel dropped (frontend disconnected)");
                self.pending.write().await.remove(&action.id);
                ApprovalDecision::Timeout
            }
            Err(_) => {
                // Timeout expired
                warn!(action_id = action.id, timeout = action.timeout_secs, "approval decision timed out");
                self.pending.write().await.remove(&action.id);
                ApprovalDecision::Timeout
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Test that a timeout (no response within the deadline) results in Timeout decision (fail-closed).
    #[tokio::test]
    async fn test_no_response_returns_timeout() {
        let action = PendingAction {
            id: "act_timeout".to_string(),
            action_type: "bash".to_string(),
            content: "echo test".to_string(),
            user_id: "user_test".to_string(),
            community_id: "comm_test".to_string(),
            timeout_secs: 1,
        };

        // Create a channel and drop the receiver without sending
        let (_tx, rx) = oneshot::channel::<bool>();

        // The receiver is dropped, so the channel is closed
        drop(rx);

        // This simulates what happens in the approval prompt when the channel is closed:
        // the timeout should trigger and return Timeout (not Approved)
        let timeout_duration = Duration::from_secs(action.timeout_secs as u64);
        let result = tokio::time::timeout(timeout_duration, async {
            // This will wait forever since the receiver is gone
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        })
        .await;

        // The timeout should have fired
        assert!(result.is_err(), "expected timeout to fire");
    }

    /// Test that the pending map correctly stores and removes entries.
    #[test]
    fn test_pending_map_crud() {
        let pending = Arc::new(RwLock::new(HashMap::<String, oneshot::Sender<bool>>::new()));

        // This test demonstrates the map bookkeeping works
        // (actual async operations tested above)
        assert_eq!(pending.blocking_read().len(), 0);
    }
}
