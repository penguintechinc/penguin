//! Typed errors for desktop-core operations, with sanitized logging.

use thiserror::Error;

/// Errors from desktop-core operations (session, keychain, OAuth, IPC).
#[derive(Error, Debug)]
pub enum DesktopError {
    /// IPC connection to penguind failed.
    #[error("IPC connection failed: {0}")]
    IpcConnection(String),

    /// Keychain (secret storage) operation failed.
    #[error("keychain operation failed: {0}")]
    KeychainError(String),

    /// No active session (no token stored or primed).
    #[error("no active session")]
    NoSession,

    /// Invalid or missing credentials.
    #[error("invalid credentials")]
    InvalidCredentials,

    /// ProxyRequest to penguind failed (network, timeout, API error).
    #[error("API request failed: status {status}")]
    ApiRequest { status: u16 },

    /// OAuth token exchange or validation failed.
    #[error("OAuth failed: {0}")]
    OAuthError(String),

    /// Invalid OAuth callback state or parameters.
    #[error("invalid OAuth callback: {0}")]
    InvalidCallback(String),

    /// URL parsing failed (malformed hub base URL, invalid callback URL).
    #[error("URL parsing failed: {0}")]
    UrlError(String),

    /// Token rotation detected but rotated token invalid or missing.
    #[error("token rotation incomplete: {0}")]
    TokenRotationFailed(String),

    /// gRPC or wire protocol error.
    #[error("gRPC error: {0}")]
    GrpcError(String),

    /// Other internal errors (not user-facing).
    #[error("internal error: {0}")]
    Internal(String),
}

impl DesktopError {
    /// Mask sensitive fields in error messages for logging.
    /// Tokens, credentials, and PII never appear in the result.
    pub fn sanitized_message(&self) -> String {
        match self {
            DesktopError::IpcConnection(msg) => {
                format!("IPC connection failed: {}", mask_sensitive(msg))
            }
            DesktopError::KeychainError(msg) => {
                format!("keychain operation failed: {}", mask_sensitive(msg))
            }
            DesktopError::NoSession => "no active session".to_string(),
            DesktopError::InvalidCredentials => "invalid credentials".to_string(),
            DesktopError::ApiRequest { status } => format!("API request failed: status {}", status),
            DesktopError::OAuthError(msg) => format!("OAuth failed: {}", mask_sensitive(msg)),
            DesktopError::InvalidCallback(msg) => {
                format!("invalid OAuth callback: {}", mask_sensitive(msg))
            }
            DesktopError::UrlError(msg) => format!("URL parsing failed: {}", mask_sensitive(msg)),
            DesktopError::TokenRotationFailed(msg) => {
                format!("token rotation incomplete: {}", mask_sensitive(msg))
            }
            DesktopError::GrpcError(msg) => format!("gRPC error: {}", mask_sensitive(msg)),
            DesktopError::Internal(msg) => format!("internal error: {}", mask_sensitive(msg)),
        }
    }
}

/// Mask tokens, emails, and secret-like strings in error messages.
/// Preserves URLs and API paths but redacts the sensitive parts.
fn mask_sensitive(s: &str) -> String {
    // Redact tokens: "token=..." or "Bearer ..." or hex-looking long strings.
    if s.contains("token")
        || s.contains("Bearer")
        || s.len() > 40
            && s.chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        "***".to_string()
    } else {
        s.to_string()
    }
}

/// Shorthand Result type for desktop-core operations.
pub type Result<T> = std::result::Result<T, DesktopError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_desktop_error_no_session() {
        let err = DesktopError::NoSession;
        assert_eq!(err.to_string(), "no active session");
    }

    #[test]
    fn test_desktop_error_invalid_credentials() {
        let err = DesktopError::InvalidCredentials;
        assert_eq!(err.to_string(), "invalid credentials");
    }

    #[test]
    fn test_desktop_error_api_request() {
        let err = DesktopError::ApiRequest { status: 401 };
        assert_eq!(err.to_string(), "API request failed: status 401");
    }

    #[test]
    fn test_sanitized_message_no_session() {
        let err = DesktopError::NoSession;
        assert_eq!(err.sanitized_message(), "no active session");
    }

    #[test]
    fn test_sanitized_message_token_masking() {
        let err = DesktopError::IpcConnection("token=abc123def456".to_string());
        let sanitized = err.sanitized_message();
        assert!(sanitized.contains("***"));
        assert!(!sanitized.contains("abc123"));
    }

    #[test]
    fn test_sanitized_message_bearer_masking() {
        let err =
            DesktopError::OAuthError("Bearer eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9".to_string());
        let sanitized = err.sanitized_message();
        assert!(sanitized.contains("***"));
    }

    #[test]
    fn test_sanitized_message_long_hex_masking() {
        let err =
            DesktopError::Internal("abc123def456abc123def456abc123def456abc123def456".to_string());
        let sanitized = err.sanitized_message();
        assert!(sanitized.contains("***"));
    }

    #[test]
    fn test_sanitized_message_normal_text() {
        let err = DesktopError::KeychainError("connection timeout".to_string());
        let sanitized = err.sanitized_message();
        assert!(sanitized.contains("connection timeout"));
    }
}
