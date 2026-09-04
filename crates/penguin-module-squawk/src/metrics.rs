//! The five Prometheus collectors squawk reports, registered once in
//! `init` and then genuinely updated from every code path that changes the
//! state each one describes.
//!
//! The Go module registered these same five collectors but only ever
//! updated `forwarderUp`, and even that only from the module lifecycle
//! (`Start`/`Stop`), never from the `forward start`/`forward stop` CLI
//! subcommands. `queriesTotal`, `cacheEntries`, and `healthStatus` were
//! registered and then never touched again. This module fixes all of that;
//! see [`crate::module`] and [`crate::commands`] for every call site.

use penguin_sdk::{HealthLevel, Metrics, MetricsError};
use prometheus::{Counter, Gauge, Opts};

/// Namespace/subsystem shared by every squawk collector. Prometheus combines
/// namespace + subsystem + name into fully-qualified metric names:
/// `penguin_module_squawk_queries_total`, etc. This single-form scheme maps
/// 1:1 to Go's bare-name scheme (`squawk_queries_total`) via the waived
/// prefix-vs-label divergence (PARITY §2.8): Go adds a `module=squawk`
/// const-label; Rust uses the namespace/subsystem split.
const NAMESPACE: &str = "penguin_module";
const SUBSYSTEM: &str = "squawk";

/// squawk's Prometheus collectors. Each field is the live handle used to
/// update the metric; the boxed clone registered with the host's shared
/// registry in [`SquawkMetrics::register`] reports the same underlying
/// series (`prometheus::Counter`/`Gauge` are cheap, `Arc`-backed clones).
pub struct SquawkMetrics {
    pub queries_total: Counter,
    pub forwarder_up: Gauge,
    pub cache_entries: Gauge,
    pub dns_applied: Gauge,
    pub health_status: Gauge,
}

impl SquawkMetrics {
    /// Builds all five collectors and registers each one with `registerer`.
    /// Bare metric names are combined with namespace and subsystem into
    /// fully-qualified names like `penguin_module_squawk_queries_total`.
    pub fn register(registerer: &dyn Metrics) -> Result<SquawkMetrics, MetricsError> {
        let queries_total = new_counter("queries_total", "Total number of DNS queries issued")?;
        registerer.register(Box::new(queries_total.clone()))?;

        let forwarder_up = new_gauge(
            "forwarder_up",
            "Whether the DNS forwarder is running (1 = running, 0 = stopped)",
        )?;
        registerer.register(Box::new(forwarder_up.clone()))?;

        let cache_entries = new_gauge("cache_entries", "Number of entries in the DNS cache")?;
        registerer.register(Box::new(cache_entries.clone()))?;

        let dns_applied = new_gauge(
            "dns_applied",
            "Whether system DNS resolver is managed (1 = managed, 0 = not managed)",
        )?;
        registerer.register(Box::new(dns_applied.clone()))?;

        let health_status = new_gauge(
            "health_status",
            "Module health status (0 = healthy, 1 = degraded, 2 = unhealthy)",
        )?;
        registerer.register(Box::new(health_status.clone()))?;

        Ok(SquawkMetrics {
            queries_total,
            forwarder_up,
            cache_entries,
            dns_applied,
            health_status,
        })
    }

    /// Records a health probe's outcome — the same probe
    /// [`crate::module::SquawkModule::status`]/`health` report from.
    pub fn set_health(&self, level: HealthLevel) {
        self.health_status.set(f64::from(level.as_i32()));
    }
}

/// Builds a `penguin_module_squawk_<name>` counter. The options are always
/// valid static strings, so the only realistic failure mode is a duplicate
/// registration — which `register`'s own `Result` already surfaces to the
/// caller, so this only needs to unwrap construction itself.
fn new_counter(name: &str, help: &str) -> Result<Counter, MetricsError> {
    let opts = Opts::new(name, help)
        .namespace(NAMESPACE)
        .subsystem(SUBSYSTEM);
    Counter::with_opts(opts).map_err(|err| MetricsError(err.to_string()))
}

/// Builds a `penguin_module_squawk_<name>` gauge; see [`new_counter`].
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

    /// A [`Metrics`] test double recording every collector handed to it, so
    /// a test can gather the whole shared-registry surface without pulling
    /// in a real `penguin_telemetry::Telemetry`.
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

    #[test]
    fn register_wires_all_five_collectors_with_go_parity_names() {
        let registerer = RecordingRegisterer {
            registry: prometheus::Registry::new(),
            names: Mutex::new(Vec::new()),
        };
        let metrics = SquawkMetrics::register(&registerer).expect("register succeeds");

        let names = registerer.names.lock().unwrap().clone();
        let expected = [
            "penguin_module_squawk_queries_total",
            "penguin_module_squawk_forwarder_up",
            "penguin_module_squawk_cache_entries",
            "penguin_module_squawk_dns_applied",
            "penguin_module_squawk_health_status",
        ];
        for name in expected {
            assert!(names.contains(&name.to_string()), "missing {name}");
        }

        metrics.queries_total.inc();
        assert_eq!(metrics.queries_total.get(), 1.0);
    }

    #[test]
    fn set_health_writes_the_numeric_health_level() {
        let registerer = RecordingRegisterer {
            registry: prometheus::Registry::new(),
            names: Mutex::new(Vec::new()),
        };
        let metrics = SquawkMetrics::register(&registerer).expect("register succeeds");

        metrics.set_health(HealthLevel::Degraded);
        assert_eq!(metrics.health_status.get(), 1.0);
    }

    #[test]
    fn registering_the_same_module_twice_reports_a_duplicate_error() {
        let registerer = RecordingRegisterer {
            registry: prometheus::Registry::new(),
            names: Mutex::new(Vec::new()),
        };
        SquawkMetrics::register(&registerer).expect("first registration succeeds");
        let Err(err) = SquawkMetrics::register(&registerer) else {
            panic!("a second registration of the same names must fail");
        };
        assert!(!err.0.is_empty());
    }
}
