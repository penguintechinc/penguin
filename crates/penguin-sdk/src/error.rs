//! Error types the contract surfaces.
//!
//! These stay deliberately thin: every module error collapses to its display
//! string when it crosses the external-plugin boundary (that is all go-plugin
//! carries), so a rich error taxonomy would not survive the trip anyway.

/// The error module lifecycle and dispatch methods return.
///
/// It wraps a human-readable message. Built-in modules could carry more, but
/// keeping it a message wrapper matches what the plugin boundary transmits and
/// keeps built-in and external modules behaving identically.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ModuleError {
    /// The human-readable failure description.
    pub message: String,
}

impl ModuleError {
    /// Builds a module error from anything string-like.
    pub fn new(message: impl Into<String>) -> ModuleError {
        ModuleError {
            message: message.into(),
        }
    }
}

impl From<&str> for ModuleError {
    fn from(message: &str) -> ModuleError {
        ModuleError::new(message)
    }
}

impl From<String> for ModuleError {
    fn from(message: String) -> ModuleError {
        ModuleError { message }
    }
}

/// The error a [`crate::SecretStore`] returns.
///
/// `NotFound` is distinguished because callers routinely branch on it (a
/// missing secret is normal, e.g. a module that has never authenticated).
/// Everything else is opaque. Mirrors the Go `ErrSecretNotFound` sentinel.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    /// The requested key does not exist in the store.
    #[error("secret not found")]
    NotFound,
    /// Any other backend failure, carrying its message.
    #[error("{0}")]
    Other(String),
}

/// The error a [`crate::Metrics`] handle returns when registration fails
/// (typically a duplicate collector). Mirrors prometheus.Registerer semantics.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct MetricsError(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_error_displays_its_message() {
        let err = ModuleError::new("boom");
        assert_eq!(err.to_string(), "boom");
        assert_eq!(err.message, "boom");
    }

    #[test]
    fn module_error_from_str_and_string_agree() {
        let from_str = ModuleError::from("bad");
        let from_string = ModuleError::from(String::from("bad"));
        assert_eq!(from_str, from_string);
        assert_eq!(from_str.to_string(), "bad");
    }

    #[test]
    fn secret_error_not_found_has_stable_text() {
        assert_eq!(SecretError::NotFound.to_string(), "secret not found");
    }

    #[test]
    fn secret_error_other_carries_message() {
        let err = SecretError::Other(String::from("keyring locked"));
        assert_eq!(err.to_string(), "keyring locked");
        assert_ne!(err, SecretError::NotFound);
    }

    #[test]
    fn metrics_error_displays_its_message() {
        let err = MetricsError(String::from("duplicate collector"));
        assert_eq!(err.to_string(), "duplicate collector");
    }
}
