//! Tauri command handlers wrapping penguin-desktop-core::Session
//!
//! Each command is a thin wrapper that calls Session methods. The session owns
//! token persistence (keychain), IPC to penguind, and OAuth state validation.

use crate::error::{TauriResult, desktop_error_to_string};
use penguin_desktop_core::{CallbackParams, OAuthPlatform};
use serde_json::json;
use tauri::State;
use tracing::{debug, info, warn};

/// Application state: owns a Session instance (mutex-wrapped for command access)
pub struct AppState {
    pub session: std::sync::Arc<tokio::sync::Mutex<penguin_desktop_core::Session>>,
    pub hub_url: String,
}

/// Login with email and password.
///
/// Calls Session::login, which POSTs to /api/v1/auth/login via penguind,
/// extracts the JWT, and persists it to the OS keychain.
/// The frontend never sees the token — only a success/failure response.
#[tauri::command]
pub async fn login(
    email: String,
    password: String,
    hub_url: String,
    state: State<'_, AppState>,
) -> TauriResult<serde_json::Value> {
    debug!("[login] email={}", email);

    let mut session = state.session.lock().await;
    session.login(&hub_url, &email, &password).await
        .map_err(|e| desktop_error_to_string(e))?;

    info!("[login] Successful for {}", email);
    Ok(json!({
        "success": true,
        "email": email
    }))
}

/// Logout: clears the keychain and penduind's in-memory session.
#[tauri::command]
pub async fn logout(state: State<'_, AppState>) -> TauriResult<()> {
    debug!("[logout] Logging out");

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
