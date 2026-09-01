//! The telemetry handle a module receives via HostServices::telemetry(): a
//! stable, OpenTelemetry-agnostic surface for metrics/traces/logs. penguin-otel
//! implements it (ScopedTelemetry -> OTLP/SigNoz); NoopTelemetry is the safe
//! handle returned when the exporter is disabled.

use crate::LogLevel;

/// A per-module telemetry handle (metrics/traces/logs), scoped to one module.
pub trait ModuleTelemetry: Send + Sync {
    /// Adds `value` to a named counter, tagged with `attrs`.
    fn counter_add(&self, name: &str, value: u64, attrs: &[(&str, &str)]);
    /// Records a span with the given name and attributes.
    fn record_span(&self, name: &str, attrs: &[(&str, &str)]);
    /// Emits a log record at `level` with the given message and attributes.
    fn emit_log(&self, level: LogLevel, message: &str, attrs: &[(&str, &str)]);

    /// A short discriminator for the kind of sink this handle drives: "noop"
    /// when telemetry is disabled, "otel" when it exports to the collector.
    /// Lets callers and tests distinguish a live handle from the no-op fallback.
    fn kind(&self) -> &'static str {
        "noop"
    }
}

/// The no-op handle returned when telemetry is disabled — every method does nothing.
pub struct NoopTelemetry;

impl ModuleTelemetry for NoopTelemetry {
    fn counter_add(&self, _name: &str, _value: u64, _attrs: &[(&str, &str)]) {}
    fn record_span(&self, _name: &str, _attrs: &[(&str, &str)]) {}
    fn emit_log(&self, _level: LogLevel, _message: &str, _attrs: &[(&str, &str)]) {}
}

#[cfg(test)]
mod tests {
    use super::{ModuleTelemetry, NoopTelemetry};

    #[test]
    fn noop_records_without_panicking() {
        let t = NoopTelemetry;
        t.counter_add("skauswatch_events_total", 3, &[("kind", "scan")]);
        t.record_span("heartbeat", &[]);
        // no assertion beyond "did not panic"; NoopTelemetry is the flag-off handle.
    }
}
