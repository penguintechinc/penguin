//! OpenTelemetry crate for the penguin endpoint agent.
//!
//! Builds the real OTLP/HTTP export pipeline (metrics/traces/logs) and hands
//! out [`ScopedTelemetry`] handles implementing `penguin_sdk::ModuleTelemetry`
//! — the stable, OpenTelemetry-agnostic surface modules record telemetry
//! through. See [`pipeline::OtelPipeline`] for the entry point.

pub mod config;
pub mod error;
pub mod pipeline;
pub mod telemetry;

pub use config::OtelConfig;
pub use error::OtelError;
pub use pipeline::OtelPipeline;
pub use telemetry::ScopedTelemetry;
