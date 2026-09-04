//! Logging and metrics for the daemon: the Go `telemetry` package ported, with
//! the PII-sanitisation stub completed for real.
//!
//! [`Telemetry`] installs a JSON tracing subscriber and a prometheus registry
//! and hands modules a redacting [`TracingLogger`] plus a namespaced metrics
//! handle. The redaction core ([`sanitize`]) is a small pure module the logger
//! applies to every field, so a module author cannot forget to mask a secret.

pub mod logger;
pub mod sanitize;
mod telemetry;

pub use logger::TracingLogger;
pub use sanitize::{is_sensitive_key, mask_secret, sanitize_value};
pub use telemetry::{Telemetry, TelemetryError};
