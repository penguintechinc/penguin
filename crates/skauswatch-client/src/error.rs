//! Error types for the SkausWatch agent API client.

use thiserror::Error;

/// Errors that may occur when using the SkausWatch agent API client.
#[derive(Error, Debug)]
pub enum ClientError {
    /// HTTP request error.
    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),
    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
    /// The Manager responded with a non-2xx status.
    #[error("SkausWatch Manager returned HTTP {status}")]
    Http {
        /// The response status code.
        status: u16,
    },
}
