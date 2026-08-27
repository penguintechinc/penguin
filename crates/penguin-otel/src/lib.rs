//! OpenTelemetry crate for the penguin endpoint agent.
//!
//! Provides configuration structures and error types for OpenTelemetry pipeline
//! initialization. The actual pipeline setup (Task 4+) will use the types exported
//! here alongside OpenTelemetry SDK/OTLP crates.

pub mod config;
pub mod error;

pub use config::OtelConfig;
pub use error::OtelError;
