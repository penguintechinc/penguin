//! SkausWatch module metrics: Prometheus collectors registered via HostServices.

use penguin_sdk::{Metrics, MetricsError};

/// SkausWatch metrics placeholder.
#[derive(Debug, Clone)]
pub struct SkausWatchMetrics {
    // Placeholder for future metrics collectors
}

impl SkausWatchMetrics {
    /// Register metrics with the host's Prometheus registry.
    pub fn register(_metrics: &dyn Metrics) -> Result<Self, MetricsError> {
        // Minimal registration; no actual collectors for the scaffold.
        Ok(SkausWatchMetrics {})
    }
}
