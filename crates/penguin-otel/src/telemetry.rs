//! `ScopedTelemetry`: the OTLP-backed implementation of
//! [`penguin_sdk::ModuleTelemetry`] handed out by [`crate::OtelPipeline::scoped`].
//!
//! Every metric/span/log recorded through it is automatically tagged with a
//! `module` attribute, so a fleet-wide SigNoz query can filter or group by
//! module without every call site remembering to add the tag itself.

use std::collections::HashMap;
use std::sync::Mutex;

use opentelemetry::KeyValue;
use opentelemetry::logs::{LogRecord as _, Logger as _, Severity};
use opentelemetry::metrics::{Counter, Meter};
use opentelemetry::trace::{Span as _, Tracer as _};
use opentelemetry_sdk::logs::SdkLogger;
use opentelemetry_sdk::trace::SdkTracer;
use penguin_sdk::{LogLevel, ModuleTelemetry};

/// A per-module OTLP telemetry handle. Holds one `Meter`/`Tracer`/`Logger`
/// each scoped (as their instrumentation name) to `module`, plus a cache of
/// already-built counters so repeated `counter_add` calls for the same
/// metric name reuse one instrument instead of re-registering it.
pub struct ScopedTelemetry {
    module: String,
    meter: Meter,
    tracer: SdkTracer,
    logger: SdkLogger,
    counters: Mutex<HashMap<String, Counter<u64>>>,
}

impl ScopedTelemetry {
    /// Builds a handle scoped to `module`, backed by provider instances the
    /// caller has already obtained with instrumentation scope = `module`.
    pub(crate) fn new(module: &str, meter: Meter, tracer: SdkTracer, logger: SdkLogger) -> Self {
        ScopedTelemetry {
            module: module.to_string(),
            meter,
            tracer,
            logger,
            counters: Mutex::new(HashMap::new()),
        }
    }

    /// `attrs` plus the `module` tag every record gets, as owned pairs
    /// (the OTel API takes owned/`'static'`-ish key-value types, so this
    /// crosses that boundary in one place rather than in every call site).
    fn attrs_with_module(&self, attrs: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut kvs = Vec::with_capacity(attrs.len() + 1);
        kvs.push(("module".to_string(), self.module.clone()));
        kvs.extend(
            attrs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string())),
        );
        kvs
    }
}

impl ModuleTelemetry for ScopedTelemetry {
    fn counter_add(&self, name: &str, value: u64, attrs: &[(&str, &str)]) {
        let kvs: Vec<KeyValue> = self
            .attrs_with_module(attrs)
            .into_iter()
            .map(|(k, v)| KeyValue::new(k, v))
            .collect();

        let mut counters = match self.counters.lock() {
            Ok(guard) => guard,
            // A poisoned lock still holds a perfectly usable cache; a panic
            // in some unrelated caller of counter_add must not take down
            // every other module's telemetry.
            Err(poisoned) => poisoned.into_inner(),
        };
        let counter = counters
            .entry(name.to_string())
            .or_insert_with(|| self.meter.u64_counter(name.to_string()).build());
        counter.add(value, &kvs);
    }

    fn record_span(&self, name: &str, attrs: &[(&str, &str)]) {
        let mut span = self.tracer.start(name.to_string());
        for (k, v) in self.attrs_with_module(attrs) {
            span.set_attribute(KeyValue::new(k, v));
        }
        span.end();
    }

    fn emit_log(&self, level: LogLevel, message: &str, attrs: &[(&str, &str)]) {
        let mut record = self.logger.create_log_record();
        record.set_severity_number(to_otel_severity(level));
        record.set_severity_text(level.as_str());
        record.set_body(message.to_string().into());
        for (k, v) in self.attrs_with_module(attrs) {
            record.add_attribute(k, v);
        }
        self.logger.emit(record);
    }
}

fn to_otel_severity(level: LogLevel) -> Severity {
    match level {
        LogLevel::Debug => Severity::Debug,
        LogLevel::Info => Severity::Info,
        LogLevel::Warn => Severity::Warn,
        LogLevel::Error => Severity::Error,
    }
}
