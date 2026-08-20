//! Everything a [`crate::client::WaddleAiClient`] request can fail with.
//!
//! The exact shape of a WaddleAI API error body isn't finalized (the
//! server-side agent-hooks feature is being built in parallel — see this
//! crate's top-level doc), so [`ErrorBody`] accepts a structured
//! `{"error": {"code": ..., "message": ...}}` body or a bare
//! `{"error": "..."}` string, and falls back to the raw bytes rather than
//! discarding a body that matched neither shape. Mirrors
//! `waddlebot_client::error::ErrorBody` exactly.

use serde_json::Value;

/// One decoded WaddleAI API error body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorBody {
    /// `{"error": {"code": "...", "message": "..."}}`.
    Structured { code: String, message: String },
    /// `{"error": "..."}`.
    Plain(String),
    /// The body wasn't JSON, or was JSON that matched neither known shape.
    Unparsed(String),
}

impl std::fmt::Display for ErrorBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorBody::Structured { code, message } => write!(f, "{code}: {message}"),
            ErrorBody::Plain(message) => write!(f, "{message}"),
            ErrorBody::Unparsed(raw) => write!(f, "{raw}"),
        }
    }
}

/// Parses a non-2xx response body into an [`ErrorBody`], trying each known
/// shape before falling back to the raw bytes.
pub(crate) fn parse_error_body(bytes: &[u8]) -> ErrorBody {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return ErrorBody::Unparsed(String::from_utf8_lossy(bytes).into_owned());
    };

    let Some(error_value) = value.get("error") else {
        return ErrorBody::Unparsed(String::from_utf8_lossy(bytes).into_owned());
    };

    if let Some(message) = error_value.as_str() {
        return ErrorBody::Plain(message.to_string());
    }

    if let Some(object) = error_value.as_object() {
        let code = object
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let message = object
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return ErrorBody::Structured {
            code: code.to_string(),
            message: message.to_string(),
        };
    }

    ErrorBody::Unparsed(String::from_utf8_lossy(bytes).into_owned())
}

/// Every way a [`crate::client::WaddleAiClient`] call can fail.
#[derive(Debug, thiserror::Error)]
pub enum WaddleAiError {
    /// Building the client's HTTP/TLS stack failed — before any request is
    /// sent.
    #[error("failed to set up HTTP client: {0}")]
    Setup(String),
    /// The request never got a usable response: DNS failure, connection
    /// refused, timeout, TLS handshake failure, or the response body
    /// couldn't be read off the wire. This is the outcome the offline
    /// fail-closed path (see `crate::cache`) treats as "no live decision
    /// available."
    #[error("transport error: {0}")]
    Transport(String),
    /// The server rejected the virtual key — HTTP 401 or 403.
    #[error("authentication rejected (HTTP {status}): {body}")]
    Auth { status: u16, body: ErrorBody },
    /// Any other non-2xx response.
    #[error("HTTP {status}: {body}")]
    Status { status: u16, body: ErrorBody },
    /// The response was 2xx but its body wasn't the JSON shape expected.
    #[error("failed to decode response: {0}")]
    Decode(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_body_reads_the_structured_shape() {
        let body = parse_error_body(br#"{"error":{"code":"NOT_FOUND","message":"missing"}}"#);
        assert_eq!(
            body,
            ErrorBody::Structured {
                code: "NOT_FOUND".to_string(),
                message: "missing".to_string(),
            }
        );
    }

    #[test]
    fn parse_error_body_reads_the_bare_string_shape() {
        let body = parse_error_body(br#"{"error":"virtual key required"}"#);
        assert_eq!(body, ErrorBody::Plain("virtual key required".to_string()));
    }

    #[test]
    fn parse_error_body_keeps_unparseable_bytes_verbatim() {
        let body = parse_error_body(b"<html>502 Bad Gateway</html>");
        assert_eq!(
            body,
            ErrorBody::Unparsed("<html>502 Bad Gateway</html>".to_string())
        );
    }

    #[test]
    fn parse_error_body_keeps_json_with_no_error_key_verbatim() {
        let body = parse_error_body(br#"{"message":"no error field here"}"#);
        assert!(matches!(body, ErrorBody::Unparsed(_)));
    }

    #[test]
    fn error_display_renders_each_variant() {
        assert_eq!(
            ErrorBody::Structured {
                code: "C".to_string(),
                message: "M".to_string()
            }
            .to_string(),
            "C: M"
        );
        assert_eq!(ErrorBody::Plain("p".to_string()).to_string(), "p");
        assert_eq!(ErrorBody::Unparsed("u".to_string()).to_string(), "u");
    }
}
