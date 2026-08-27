/// OpenTelemetry configuration.
///
/// Combines local defaults with console-provided overrides. Used in subsequent
/// crates to initialize the OpenTelemetry pipeline (Task 4+).
#[derive(Clone, Debug)]
pub struct OtelConfig {
    /// OpenTelemetry collector endpoint (HTTP/gRPC).
    pub endpoint: String,
    /// Sampling ratio: 0.0 (never) to 1.0 (always).
    pub sampling_ratio: f64,
    /// Whether to enable telemetry collection.
    pub enabled: bool,
}

impl OtelConfig {
    /// Merge local config with an optional console override.
    ///
    /// Any field from `console` (if `Some`) replaces the `local` value.
    /// If `console` is `None`, returns `local` unchanged.
    pub fn merge(local: OtelConfig, console: Option<OtelConfig>) -> OtelConfig {
        match console {
            Some(console_config) => console_config,
            None => local,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OtelConfig;

    #[test]
    fn console_overrides_local() {
        let local = OtelConfig {
            endpoint: "http://local:4317".into(),
            sampling_ratio: 0.1,
            enabled: false,
        };
        let console = OtelConfig {
            endpoint: "http://signoz:4317".into(),
            sampling_ratio: 1.0,
            enabled: true,
        };
        let merged = OtelConfig::merge(local, Some(console));
        assert_eq!(merged.endpoint, "http://signoz:4317");
        assert!(merged.enabled);
        assert!((merged.sampling_ratio - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn no_console_keeps_local() {
        let local = OtelConfig {
            endpoint: "http://local:4317".into(),
            sampling_ratio: 0.5,
            enabled: true,
        };
        let merged = OtelConfig::merge(local.clone(), None);
        assert_eq!(merged.endpoint, local.endpoint);
    }
}
