//! SkausWatch module metrics: Prometheus collectors registered once in
//! `init` and updated from the heartbeat/report loop in `module.rs`.

use penguin_sdk::{Metrics, MetricsError};
use prometheus::{Counter, Gauge, Opts};

/// Namespace/subsystem shared by every collector — combines into fully
/// qualified names like `penguin_module_skauswatch_heartbeats_total`,
/// matching `penguin-module-tobogganing::metrics`'s scheme.
const NAMESPACE: &str = "penguin_module";
const SUBSYSTEM: &str = "skauswatch";

/// SkausWatch's Prometheus collectors.
pub struct SkausWatchMetrics {
    /// Whether this agent has completed at least one successful
    /// `register()` check-in with the Manager since this process started
    /// (1=yes, 0=no). The agent's identity itself is always provisioned
    /// (never earned via this call) — this only tracks check-in state.
    pub checked_in: Gauge,
    /// Total number of heartbeats successfully acknowledged by the Manager.
    pub heartbeats_total: Counter,
    /// Total number of [`skauswatch_client::EndpointEvent`]s successfully
    /// reported to the Manager.
    pub events_reported_total: Counter,
    /// Total number of transient errors in the heartbeat/report loop
    /// (registration, heartbeat, or event-report failures) — the loop never
    /// panics or exits on these, it only counts and retries next tick.
    pub errors_total: Counter,
}

impl SkausWatchMetrics {
    /// Builds all four collectors and registers each with `registerer`.
    pub fn register(registerer: &dyn Metrics) -> Result<SkausWatchMetrics, MetricsError> {
        let checked_in = new_gauge(
            "checked_in",
            "Whether this agent has completed at least one successful check-in (1=yes, 0=no)",
        )?;
        registerer.register(Box::new(checked_in.clone()))?;

        let heartbeats_total = new_counter(
            "heartbeats_total",
            "Total number of heartbeats successfully acknowledged by the Manager",
        )?;
        registerer.register(Box::new(heartbeats_total.clone()))?;

        let events_reported_total = new_counter(
            "events_reported_total",
            "Total number of endpoint events successfully reported to the Manager",
        )?;
        registerer.register(Box::new(events_reported_total.clone()))?;

        let errors_total = new_counter(
            "errors_total",
            "Total number of transient errors in the heartbeat/report loop",
        )?;
        registerer.register(Box::new(errors_total.clone()))?;

        Ok(SkausWatchMetrics {
            checked_in,
            heartbeats_total,
            events_reported_total,
            errors_total,
        })
    }
}

fn new_counter(name: &str, help: &str) -> Result<Counter, MetricsError> {
    let opts = Opts::new(name, help)
        .namespace(NAMESPACE)
        .subsystem(SUBSYSTEM);
    Counter::with_opts(opts).map_err(|err| MetricsError(err.to_string()))
}

fn new_gauge(name: &str, help: &str) -> Result<Gauge, MetricsError> {
    let opts = Opts::new(name, help)
        .namespace(NAMESPACE)
        .subsystem(SUBSYSTEM);
    Gauge::with_opts(opts).map_err(|err| MetricsError(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct RecordingRegisterer {
        registry: prometheus::Registry,
        names: Mutex<Vec<String>>,
    }

    impl Metrics for RecordingRegisterer {
        fn register(
            &self,
            collector: Box<dyn prometheus::core::Collector>,
        ) -> Result<(), MetricsError> {
            let desc = collector.desc()[0].fq_name.clone();
            self.registry
                .register(collector)
                .map_err(|err| MetricsError(err.to_string()))?;
            self.names.lock().unwrap().push(desc);
            Ok(())
        }
    }

    fn registerer() -> RecordingRegisterer {
        RecordingRegisterer {
            registry: prometheus::Registry::new(),
            names: Mutex::new(Vec::new()),
        }
    }

    #[test]
    fn register_wires_all_four_collectors() {
        let reg = registerer();
        let metrics = SkausWatchMetrics::register(&reg).expect("register succeeds");

        let names = reg.names.lock().unwrap().clone();
        let expected = [
            "penguin_module_skauswatch_checked_in",
            "penguin_module_skauswatch_heartbeats_total",
            "penguin_module_skauswatch_events_reported_total",
            "penguin_module_skauswatch_errors_total",
        ];
        for name in expected {
            assert!(names.contains(&name.to_string()), "missing {name}");
        }

        metrics.heartbeats_total.inc();
        assert_eq!(metrics.heartbeats_total.get(), 1.0);
    }

    #[test]
    fn registering_twice_reports_a_duplicate_error() {
        let reg = registerer();
        SkausWatchMetrics::register(&reg).expect("first registration succeeds");
        let Err(err) = SkausWatchMetrics::register(&reg) else {
            panic!("a second registration of the same names must fail");
        };
        assert!(!err.0.is_empty());
    }
}
