//! Error handling for Tauri commands.
//!
//! Converts penguin-desktop-core errors into Tauri-compatible error strings.

use penguin_desktop_core::DesktopError;

/// Tauri command result type: errors are serialized as strings.
pub type TauriResult<T> = Result<T, String>;

/// Convert DesktopError to a Tauri-friendly error string (internal helper).
pub fn desktop_error_to_string(err: DesktopError) -> String {
    match err {
        DesktopError::InvalidCredentials => "Invalid email or password".to_string(),
        DesktopError::NoSession => "No active session".to_string(),
        DesktopError::KeychainError(msg) => format!("Keychain error: {}", msg),
        DesktopError::GrpcError(msg) => format!("IPC error: {}", msg),
        DesktopError::OAuthError(msg) => format!("OAuth error: {}", msg),
        DesktopError::ApiRequest { status } => {
            format!("API request failed with status {}", status)
        }
        DesktopError::IpcConnection(msg) => format!("IPC connection error: {}", msg),
        DesktopError::InvalidCallback(msg) => format!("OAuth callback error: {}", msg),
        DesktopError::UrlError(msg) => format!("URL error: {}", msg),
        DesktopError::TokenRotationFailed(msg) => format!("Token rotation error: {}", msg),
        DesktopError::LuaError(msg) => format!("Lua execution error: {}", msg),
        DesktopError::JsonError(msg) => format!("JSON serialization error: {}", msg),
        DesktopError::Internal(msg) => format!("Internal error: {}", msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_conversion() {
        let err = DesktopError::InvalidCredentials;
        let s = desktop_error_to_string(err);
        assert_eq!(s, "Invalid email or password");
    }
}
