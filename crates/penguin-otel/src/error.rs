/// OpenTelemetry pipeline errors.
#[derive(Debug, thiserror::Error)]
pub enum OtelError {
    /// Failed to build the OpenTelemetry pipeline.
    #[error("otel pipeline build failed: {0}")]
    Build(String),
}
