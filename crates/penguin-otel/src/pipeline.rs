//! The real OTLP pipeline: builds metric/trace/log providers that export to
//! a collector (SigNoz) over OTLP/HTTP, and hands out module-scoped
//! [`crate::ScopedTelemetry`] handles implementing
//! [`penguin_sdk::ModuleTelemetry`].
//!
//! Transport is OTLP/HTTP, never `grpc-tonic` — see the dependency comment
//! in `Cargo.toml` for why gRPC is off the table in this workspace.

use std::sync::Arc;

use opentelemetry::logs::LoggerProvider as _;
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{InstrumentationScope, KeyValue};
use opentelemetry_otlp::{LogExporter, MetricExporter, Protocol, SpanExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use penguin_sdk::{ModuleTelemetry, NoopTelemetry};

use crate::telemetry::ScopedTelemetry;
use crate::{OtelConfig, OtelError};

/// Owns the metric/trace/log providers built for one process. `None` when
/// `OtelConfig::enabled` was false at build time — `scoped()` then hands out
/// [`NoopTelemetry`] handles and `shutdown()` is a no-op, so callers never
/// need to branch on whether telemetry is actually enabled.
pub struct OtelPipeline {
    inner: Option<Providers>,
}

struct Providers {
    tracer_provider: SdkTracerProvider,
    meter_provider: SdkMeterProvider,
    logger_provider: SdkLoggerProvider,
}

impl OtelPipeline {
    /// Builds the OTLP/HTTP pipeline described by `cfg`, tagging every
    /// exported record with `resource_attrs` (e.g. `node_id`). An exporter
    /// that fails to construct (bad endpoint URL, etc.) is a build error;
    /// once running, per-export failures are logged and swallowed — see
    /// `telemetry.rs`/the OTLP exporters' own retry/log-and-continue
    /// behavior — never a panic and never a blocked caller.
    ///
    /// # Note: requires a multi-thread Tokio runtime
    ///
    /// The returned pipeline's metric/span/log export (`scoped(...)`'s
    /// handles, and `shutdown()`) MUST run on a multi-thread Tokio runtime
    /// (`#[tokio::main]`'s default, or `Builder::new_multi_thread()`) — the
    /// same requirement applies to whichever thread called `build`, since
    /// that's ordinarily the same runtime. Under a `current_thread` runtime
    /// (`#[tokio::test]`'s default, or `Builder::new_current_thread()`),
    /// the OTLP/HTTP exporter's blocking reqwest client reliably hangs for
    /// its full ~10s (`force_flush`) / ~5s (`shutdown`) internal timeouts
    /// and then fails — reproduced against the bare `opentelemetry_sdk` +
    /// `opentelemetry-otlp` crates outside this module, so it is not
    /// specific to this crate's glue code. `penguind` (this pipeline's only
    /// caller today) already runs multi-thread
    /// (`bins/penguind/src/daemon_main.rs`), so this is a non-issue there —
    /// but `penguin-otel` is a reusable library, so any future caller must
    /// do the same.
    pub fn build(
        cfg: &OtelConfig,
        resource_attrs: &[(&str, &str)],
    ) -> Result<OtelPipeline, OtelError> {
        if !cfg.enabled {
            return Ok(OtelPipeline { inner: None });
        }

        let resource = build_resource(resource_attrs);
        let base = cfg.endpoint.trim_end_matches('/');

        let span_exporter = SpanExporter::builder()
            .with_http()
            .with_endpoint(format!("{base}/v1/traces"))
            .with_protocol(Protocol::HttpBinary)
            .build()
            .map_err(|e| OtelError::Build(format!("span exporter: {e}")))?;

        let tracer_provider = SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .with_sampler(Sampler::TraceIdRatioBased(cfg.sampling_ratio))
            .with_batch_exporter(span_exporter)
            .build();

        let metric_exporter = MetricExporter::builder()
            .with_http()
            .with_endpoint(format!("{base}/v1/metrics"))
            .with_protocol(Protocol::HttpBinary)
            .build()
            .map_err(|e| OtelError::Build(format!("metric exporter: {e}")))?;

        // Bounded export queue: PeriodicReader's default background-thread
        // channel is bounded, so a stalled/unreachable collector applies
        // backpressure and drops rather than growing memory without limit.
        let reader = PeriodicReader::builder(metric_exporter).build();
        let meter_provider = SdkMeterProvider::builder()
            .with_resource(resource.clone())
            .with_reader(reader)
            .build();

        let log_exporter = LogExporter::builder()
            .with_http()
            .with_endpoint(format!("{base}/v1/logs"))
            .with_protocol(Protocol::HttpBinary)
            .build()
            .map_err(|e| OtelError::Build(format!("log exporter: {e}")))?;

        let logger_provider = SdkLoggerProvider::builder()
            .with_resource(resource)
            .with_batch_exporter(log_exporter)
            .build();

        Ok(OtelPipeline {
            inner: Some(Providers {
                tracer_provider,
                meter_provider,
                logger_provider,
            }),
        })
    }

    /// Returns a telemetry handle scoped to `module` — every metric/span/log
    /// recorded through it is tagged with a `module` attribute. When the
    /// pipeline was built with telemetry disabled, returns a
    /// [`NoopTelemetry`] handle instead so callers don't need to branch.
    pub fn scoped(&self, module: &str) -> Arc<dyn ModuleTelemetry> {
        let Some(inner) = &self.inner else {
            return Arc::new(NoopTelemetry);
        };
        let scope = InstrumentationScope::builder(module.to_string()).build();
        let meter = inner.meter_provider.meter_with_scope(scope.clone());
        let tracer = inner.tracer_provider.tracer_with_scope(scope.clone());
        let logger = inner.logger_provider.logger_with_scope(scope);
        Arc::new(ScopedTelemetry::new(module, meter, tracer, logger))
    }

    /// Flushes every provider and shuts its exporter down. A shutdown/flush
    /// error on one signal is logged and does not prevent the others from
    /// shutting down — a broken exporter must never block process exit.
    pub fn shutdown(self) {
        let Some(inner) = self.inner else {
            return;
        };
        if let Err(e) = inner.tracer_provider.force_flush() {
            tracing::warn!(error = %e, "otel: trace force_flush failed");
        }
        if let Err(e) = inner.meter_provider.force_flush() {
            tracing::warn!(error = %e, "otel: metrics force_flush failed");
        }
        if let Err(e) = inner.logger_provider.force_flush() {
            tracing::warn!(error = %e, "otel: logs force_flush failed");
        }
        if let Err(e) = inner.tracer_provider.shutdown() {
            tracing::warn!(error = %e, "otel: trace provider shutdown failed");
        }
        if let Err(e) = inner.meter_provider.shutdown() {
            tracing::warn!(error = %e, "otel: meter provider shutdown failed");
        }
        if let Err(e) = inner.logger_provider.shutdown() {
            tracing::warn!(error = %e, "otel: logger provider shutdown failed");
        }
    }
}

fn build_resource(attrs: &[(&str, &str)]) -> Resource {
    let kvs: Vec<KeyValue> = attrs
        .iter()
        .map(|(k, v)| KeyValue::new((*k).to_string(), (*v).to_string()))
        .collect();
    Resource::builder().with_attributes(kvs).build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use penguin_sdk::LogLevel;

    /// `enabled: false` must build a pure no-op pipeline: no exporter is
    /// constructed (the endpoint below is never dialed — a bogus port would
    /// fail loudly if this test were somehow reaching the network), `scoped`
    /// hands back a handle whose methods are safe to call unconditionally,
    /// and `shutdown` returns immediately with nothing to flush.
    #[test]
    fn disabled_config_builds_a_pure_noop_pipeline() {
        let cfg = OtelConfig {
            endpoint: "http://127.0.0.1:1".to_string(),
            sampling_ratio: 1.0,
            enabled: false,
        };
        let pipe = OtelPipeline::build(&cfg, &[("node_id", "n-1")]).expect("build");

        let t = pipe.scoped("skauswatch");
        t.counter_add("events_total", 5, &[("kind", "scan")]);
        t.record_span("heartbeat", &[]);
        t.emit_log(LogLevel::Info, "hello", &[]);

        pipe.shutdown();
    }
}
