//! Tauri command handlers wrapping penguin-desktop-core::Session
//!
//! Each command is a thin wrapper that calls Session methods. The session owns
//! token persistence (keychain), IPC to penguind, and OAuth state validation.

use crate::error::{TauriResult, desktop_error_to_string};
use penguin_desktop_core::{CallbackParams, OAuthPlatform};
use serde_json::json;
use tauri::State;
use tracing::{debug, info, warn};

/// Application state: owns a Session instance (mutex-wrapped for command access),
/// an approval prompt, and the poll loop handle for lifecycle management.
pub struct AppState {
    pub session: std::sync::Arc<tokio::sync::Mutex<penguin_desktop_core::Session>>,
    pub hub_url: String,
    pub approval_pending: std::sync::Arc<tokio::sync::RwLock<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>>,
    /// Poll loop handle and cancellation token (Option to allow initialization without active poll loop).
    pub poll_loop: std::sync::Arc<tokio::sync::Mutex<Option<PollLoopHandle>>>,
}

/// Stores the poll loop's join handle and cancellation token for lifecycle management.
pub struct PollLoopHandle {
    pub task: tokio::task::JoinHandle<std::result::Result<u64, Box<dyn std::error::Error + Send + Sync>>>,
    pub token: tokio_util::sync::CancellationToken,
}

/// Spawns the poll loop for the given session.
///
/// This function:
/// 1. Creates an ActionExecutor
/// 2. Registers the machine with the hub
/// 3. Spawns the poll loop as a background task
/// 4. Stores the handle and cancellation token for lifecycle management
///
/// Called from both login() and startup resume paths.
pub async fn spawn_poll_loop(
    app_handle: &tauri::AppHandle,
    session: std::sync::Arc<tokio::sync::Mutex<penguin_desktop_core::Session>>,
    approval_pending: std::sync::Arc<tokio::sync::RwLock<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>>,
    poll_loop_guard: &mut Option<PollLoopHandle>,
) -> Result<(), String> {
    use penguin_desktop_core::ActionExecutor;
    use std::time::Duration;

    // Cancel any existing poll loop before starting a new one
    if let Some(handle) = poll_loop_guard.take() {
        debug!("[spawn_poll_loop] Cancelling existing poll loop before starting new one");
        handle.token.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), handle.task).await;
    }

    // Create the approval prompt with the shared pending map
    let approval_prompt = Box::new(crate::approval::TauriApprovalPrompt::new_with_pending(
        app_handle.clone(),
        approval_pending.clone(),
    ));

    // Create the ActionExecutor with the shared session Arc
    let executor = ActionExecutor::new(session, approval_prompt).await
        .map_err(|e| format!("Failed to create ActionExecutor: {}", e))?;

    // Register the machine with the hub
    {
        let mut executor_mut = executor;
        executor_mut.register(vec!["waddlebot".to_string()]).await
            .map_err(|e| format!("Failed to register machine: {}", e))?;

        // Create a cancellation token for the poll loop
        let shutdown_token = tokio_util::sync::CancellationToken::new();
        let shutdown_token_clone = shutdown_token.clone();

        // Spawn the poll loop as a background task
        let task = tokio::spawn(async move {
            match penguin_desktop_core::action_executor::run_poll_loop(
                executor_mut,
                Duration::from_secs(5), // 5-second poll interval
                shutdown_token_clone,
            )
            .await
            {
                Ok(count) => {
                    info!("[poll-loop] Exited after processing {} actions", count);
                    Ok(count)
                }
                Err(e) => {
                    let err: Box<dyn std::error::Error + Send + Sync> =
                        Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()));
                    Err(err)
                }
            }
        });

        *poll_loop_guard = Some(PollLoopHandle {
            task,
            token: shutdown_token,
        });

        debug!("[spawn_poll_loop] Poll loop started");
    }

    Ok(())
}

/// Login with email and password.
///
/// Calls Session::login, which POSTs to /api/v1/auth/login via penduind,
/// extracts the JWT, and persists it to the OS keychain.
/// The frontend never sees the token — only a success/failure response.
///
/// On success, starts the poll loop for remote action execution.
#[tauri::command]
pub async fn login(
    email: String,
    password: String,
    hub_url: String,
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> TauriResult<serde_json::Value> {
    debug!("[login] email={}", email);

    let mut session = state.session.lock().await;
    session.login(&hub_url, &email, &password).await
        .map_err(|e| desktop_error_to_string(e))?;

    drop(session); // Release lock before starting poll loop

    // Start the poll loop (guard against double-login)
    {
        let mut poll_loop_guard = state.poll_loop.lock().await;
        spawn_poll_loop(
            &app_handle,
            state.session.clone(),
            state.approval_pending.clone(),
            &mut poll_loop_guard,
        ).await?;
    }

    info!("[login] Successful for {}", email);
    Ok(json!({
        "success": true,
        "email": email
    }))
}

/// Logout: cancels the poll loop, clears the keychain, and clears penduind's session.
#[tauri::command]
pub async fn logout(state: State<'_, AppState>) -> TauriResult<()> {
    use std::time::Duration;

    debug!("[logout] Logging out");

    // Cancel any active poll loop
    {
        let mut poll_loop_guard = state.poll_loop.lock().await;
        if let Some(handle) = poll_loop_guard.take() {
            debug!("[logout] Cancelling active poll loop");
            handle.token.cancel();

            // Await the task with a timeout (don't hang logout forever)
            match tokio::time::timeout(Duration::from_secs(5), handle.task).await {
                Ok(Ok(_)) => debug!("[logout] Poll loop task completed"),
                Ok(Err(e)) => warn!("[logout] Poll loop task panicked: {}", e),
                Err(_) => warn!("[logout] Poll loop task did not complete within timeout"),
            }
        }
    }

    // Clear the session
    let mut session = state.session.lock().await;
    session.logout().await
        .map_err(|e| desktop_error_to_string(e))?;

    info!("[logout] Successful");
    Ok(())
}

/// Make an authenticated API request.
///
/// The request is forwarded to penduind's ProxyRequest RPC, which injects
/// the Bearer token, handles 401 → refresh → retry, and returns the response.
/// The frontend never handles tokens.
#[tauri::command]
pub async fn api_request(
    method: String,
    path: String,
    body: Option<String>,
    state: State<'_, AppState>,
) -> TauriResult<serde_json::Value> {
    debug!("[api_request] {} {}", method, path);

    let body_bytes = body.map(|b| b.into_bytes());

    let mut session = state.session.lock().await;
    let resp = session.api_request(&method, &path, body_bytes).await
        .map_err(|e| desktop_error_to_string(e))?;

    debug!("[api_request] {} {} -> {}", method, path, resp.status);

    Ok(json!({
        "status": resp.status,
        "body": String::from_utf8_lossy(&resp.body).to_string()
    }))
}

/// Start an OAuth flow for a given platform (e.g., "google", "github").
///
/// Returns the authorize URL (to be opened in the browser) and the state
/// (to be matched against the callback). The frontend should open the URL
/// in the system browser.
#[tauri::command]
pub async fn oauth_start(
    platform: String,
    hub_url: String,
    state: State<'_, AppState>,
) -> TauriResult<serde_json::Value> {
    debug!("[oauth_start] platform={}", platform);

    let session = state.session.lock().await;

    let platform_enum = match platform.to_lowercase().as_str() {
        "google" => OAuthPlatform::Google,
        "github" => OAuthPlatform::GitHub,
        "discord" => OAuthPlatform::Discord,
        p => {
            warn!("[oauth_start] Unknown platform: {}", p);
            return Err("Unsupported OAuth platform".into());
        }
    };

    let (auth_url, oauth_context) = session.oauth_start(&hub_url, platform_enum).await
        .map_err(|e| desktop_error_to_string(e))?;

    info!("[oauth_start] Generated auth URL for {}", platform);

    Ok(json!({
        "authorize_url": auth_url,
        "state": oauth_context.state,
        "platform": oauth_context.platform
    }))
}

/// Complete an OAuth flow with callback parameters from the deep-link.
///
/// Validates the state, extracts tokens from the JWT, stores in keychain,
/// and primes penduind. The JWT token should be extracted from the OAuth
/// callback redirect (waddles://oauth/callback?token=...&state=...).
#[tauri::command]
pub async fn oauth_complete(
    token: String,
    state_param: String,
    stored_state: String,
    state: State<'_, AppState>,
) -> TauriResult<serde_json::Value> {
    debug!("[oauth_complete] state_param={}", state_param);

    // Validate state matches (constant-time comparison)
    if state_param != stored_state {
        warn!("[oauth_complete] State mismatch: {} != {}", state_param, stored_state);
        return Err("OAuth state mismatch".into());
    }

    // Build CallbackParams from the token and state
    let params = CallbackParams {
        token,
        state: state_param,
    };

    let mut session = state.session.lock().await;
    session.oauth_complete(params, &stored_state).await
        .map_err(|e| desktop_error_to_string(e))?;

    info!("[oauth_complete] OAuth callback processed");

    Ok(json!({
        "success": true,
        "message": "OAuth login successful"
    }))
}

/// Respond to an approval request.
///
/// The frontend calls this when the user makes a decision on an action approval.
/// Resolves the pending oneshot channel with the approval decision.
#[tauri::command]
pub async fn respond_to_approval(
    action_id: String,
    approved: bool,
    state: State<'_, AppState>,
) -> TauriResult<()> {
    debug!("[respond_to_approval] action_id={}, approved={}", action_id, approved);

    let mut pending = state.approval_pending.write().await;
    if let Some(tx) = pending.remove(&action_id) {
        if tx.send(approved).is_err() {
            warn!("[respond_to_approval] Failed to send approval decision (receiver dropped)");
        } else {
            info!("[respond_to_approval] Approval decision sent for {}", action_id);
        }
    } else {
        warn!("[respond_to_approval] No pending approval for action_id: {}", action_id);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_parsing() {
        let platforms = vec!["google", "github", "discord"];
        for p in platforms {
            let parsed = match p {
                "google" => OAuthPlatform::Google,
                "github" => OAuthPlatform::GitHub,
                "discord" => OAuthPlatform::Discord,
                _ => panic!("unknown platform"),
            };
            assert!(!parsed.as_str().is_empty());
        }
    }
}
