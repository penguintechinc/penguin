//! The Prometheus collectors this module reports, registered once in
//! `init` and updated from every code path that changes the state each one
//! describes.
//!
//! `hook_evaluation_latency_seconds` is the headline metric this crate
//! exists to produce: hooks run synchronously inside the calling agent's
//! loop (Claude Code, Antigravity/AGY, VS Code all block on the shim's exit
//! before continuing), so this module's own added latency — the shim
//! process spinning up, the round trip to WaddleAI, decoding the response —
//! is directly on the critical path of every gated tool call, not a
//! footnote an operator checks only when something feels slow.

use penguin_sdk::{Metrics, MetricsError};
use prometheus::{Counter, CounterVec, Gauge, Histogram, HistogramOpts, Opts};

/// Namespace/subsystem shared by every collector this crate registers,
/// matching every other built-in module's convention (fully-qualified names
/// come out as `penguin_module_waddleai_*`).
const NAMESPACE: &str = "penguin_module";
const SUBSYSTEM: &str = "waddleai";

/// Bucket boundaries for [`WaddleAiMetrics::hook_evaluation_latency_seconds`],
/// biased toward sub-second resolution: a hook adding even 250ms to every
/// gated tool call is user-visible, so the buckets stay fine-grained well
/// below one second rather than using Prometheus's web-latency-oriented
/// defaults.
const LATENCY_BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
];

/// This crate's Prometheus collectors.
pub struct WaddleAiMetrics {
    /// Incremented on every WaddleAI API request, success or failure.
    pub api_requests_total: Counter,
    /// Incremented whenever a WaddleAI API request fails — auth, any other
    /// non-2xx status, transport, or decode error.
    pub api_errors_total: Counter,
    /// Whether the module is currently running (1) or stopped (0).
    pub up: Gauge,
    /// Total hook invocations, labeled by `ecosystem` (`claude`, `gemini`,
    /// `vscode`) and `event` (e.g. `pre-tool-use`) — every time `penguin
    /// waddleai hook <event>` runs, regardless of outcome.
    pub hook_invocations_total: CounterVec,
    /// Total hook decisions, labeled by `outcome`: `allow`, `deny` (either a
    /// live server decision or a cached denylist match — see
    /// `crate::commands::hook_command`), `unavailable` (no live decision and
    /// no cache match), or `error` (malformed input, decode failure).
    pub hook_decisions_total: CounterVec,
    /// Wall-clock time of one full `hook` command invocation — this
    /// module's own overhead on the agent's critical path. See this
    /// module's doc.
    pub hook_evaluation_latency_seconds: Histogram,
    /// Number of entries in the currently cached Tier-1 denylist.
    pub denylist_entries: Gauge,
    /// Unix timestamp of the last successful denylist sync; `0` means never
    /// synced.
    pub denylist_last_synced_timestamp_seconds: Gauge,
    /// Whether the cached denylist is currently stale per
    /// `crate::cache::DenylistCache::is_stale` (1 = stale, 0 = fresh).
    pub denylist_stale: Gauge,
}

impl WaddleAiMetrics {
    /// Builds every collector and registers each with `registerer`.
    pub fn register(registerer: &dyn Metrics) -> Result<WaddleAiMetrics, MetricsError> {
        let api_requests_total = new_counter(
            "api_requests_total",
            "Total number of requests sent to the WaddleAI API",
        )?;
        registerer.register(Box::new(api_requests_total.clone()))?;

        let api_errors_total = new_counter(
            "api_errors_total",
            "Total number of failed WaddleAI API requests (auth, HTTP status, transport, or decode errors)",
        )?;
        registerer.register(Box::new(api_errors_total.clone()))?;

        let up = new_gauge(
            "up",
            "Whether the waddleai module is running (1 = running, 0 = stopped)",
        )?;
        registerer.register(Box::new(up.clone()))?;

        let hook_invocations_total = new_counter_vec(
            "hook_invocations_total",
            "Total agent-hook invocations",
            &["ecosystem", "event"],
        )?;
        registerer.register(Box::new(hook_invocations_total.clone()))?;

        let hook_decisions_total = new_counter_vec(
            "hook_decisions_total",
            "Total agent-hook decisions by outcome",
            &["outcome"],
        )?;
        registerer.register(Box::new(hook_decisions_total.clone()))?;

        let hook_evaluation_latency_seconds = new_histogram(
            "hook_evaluation_latency_seconds",
            "Wall-clock time of one hook command invocation, including the WaddleAI round trip",
        )?;
        registerer.register(Box::new(hook_evaluation_latency_seconds.clone()))?;

        let denylist_entries = new_gauge(
            "denylist_entries",
            "Number of entries in the currently cached Tier-1 denylist",
        )?;
        registerer.register(Box::new(denylist_entries.clone()))?;

        let denylist_last_synced_timestamp_seconds = new_gauge(
            "denylist_last_synced_timestamp_seconds",
            "Unix timestamp of the last successful denylist sync (0 = never synced)",
        )?;
        registerer.register(Box::new(denylist_last_synced_timestamp_seconds.clone()))?;

        let denylist_stale = new_gauge(
            "denylist_stale",
            "Whether the cached denylist is currently stale (1 = stale, 0 = fresh)",
        )?;
        registerer.register(Box::new(denylist_stale.clone()))?;

        Ok(WaddleAiMetrics {
            api_requests_total,
            api_errors_total,
            up,
            hook_invocations_total,
            hook_decisions_total,
            hook_evaluation_latency_seconds,
            denylist_entries,
            denylist_last_synced_timestamp_seconds,
            denylist_stale,
        })
    }
}

fn opts(name: &str, help: &str) -> Opts {
    Opts::new(name, help)
        .namespace(NAMESPACE)
        .subsystem(SUBSYSTEM)
}

fn new_counter(name: &str, help: &str) -> Result<Counter, MetricsError> {
    Counter::with_opts(opts(name, help)).map_err(|err| MetricsError(err.to_string()))
}

fn new_gauge(name: &str, help: &str) -> Result<Gauge, MetricsError> {
    Gauge::with_opts(opts(name, help)).map_err(|err| MetricsError(err.to_string()))
}

fn new_counter_vec(name: &str, help: &str, labels: &[&str]) -> Result<CounterVec, MetricsError> {
    CounterVec::new(opts(name, help), labels).map_err(|err| MetricsError(err.to_string()))
}

fn new_histogram(name: &str, help: &str) -> Result<Histogram, MetricsError> {
    let hist_opts = HistogramOpts::new(name, help)
        .namespace(NAMESPACE)
        .subsystem(SUBSYSTEM)
        .buckets(LATENCY_BUCKETS.to_vec());
    Histogram::with_opts(hist_opts).map_err(|err| MetricsError(err.to_string()))
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
    fn register_wires_every_collector_with_its_documented_name() {
        let reg = registerer();
        let metrics = WaddleAiMetrics::register(&reg).expect("register succeeds");

        let names = reg.names.lock().unwrap().clone();
        let expected = [
            "penguin_module_waddleai_api_requests_total",
            "penguin_module_waddleai_api_errors_total",
            "penguin_module_waddleai_up",
            "penguin_module_waddleai_hook_invocations_total",
            "penguin_module_waddleai_hook_decisions_total",
            "penguin_module_waddleai_hook_evaluation_latency_seconds",
            "penguin_module_waddleai_denylist_entries",
            "penguin_module_waddleai_denylist_last_synced_timestamp_seconds",
            "penguin_module_waddleai_denylist_stale",
        ];
        for name in expected {
            assert!(names.contains(&name.to_string()), "missing {name}");
        }

        metrics.api_requests_total.inc();
        assert_eq!(metrics.api_requests_total.get(), 1.0);
    }

    #[test]
    fn hook_invocations_total_is_labeled_by_ecosystem_and_event() {
        let reg = registerer();
        let metrics = WaddleAiMetrics::register(&reg).unwrap();

        metrics
            .hook_invocations_total
            .with_label_values(&["claude", "pre-tool-use"])
            .inc();
        metrics
            .hook_invocations_total
            .with_label_values(&["claude", "pre-tool-use"])
            .inc();
        metrics
            .hook_invocations_total
            .with_label_values(&["vscode", "pre-tool-use"])
            .inc();

        assert_eq!(
            metrics
                .hook_invocations_total
                .with_label_values(&["claude", "pre-tool-use"])
                .get(),
            2.0
        );
        assert_eq!(
            metrics
                .hook_invocations_total
                .with_label_values(&["vscode", "pre-tool-use"])
                .get(),
            1.0
        );
    }

    #[test]
    fn hook_decisions_total_is_labeled_by_outcome() {
        let reg = registerer();
        let metrics = WaddleAiMetrics::register(&reg).unwrap();

        metrics
            .hook_decisions_total
            .with_label_values(&["allow"])
            .inc();
        metrics
            .hook_decisions_total
            .with_label_values(&["deny"])
            .inc();
        metrics
            .hook_decisions_total
            .with_label_values(&["deny"])
            .inc();

        assert_eq!(
            metrics
                .hook_decisions_total
                .with_label_values(&["deny"])
                .get(),
            2.0
        );
        assert_eq!(
            metrics
                .hook_decisions_total
                .with_label_values(&["allow"])
                .get(),
            1.0
        );
    }

    #[test]
    fn hook_evaluation_latency_records_observations() {
        let reg = registerer();
        let metrics = WaddleAiMetrics::register(&reg).unwrap();

        metrics.hook_evaluation_latency_seconds.observe(0.02);
        assert_eq!(
            metrics.hook_evaluation_latency_seconds.get_sample_count(),
            1
        );
    }

    #[test]
    fn denylist_gauges_reflect_the_current_snapshot() {
        let reg = registerer();
        let metrics = WaddleAiMetrics::register(&reg).unwrap();

        metrics.denylist_entries.set(42.0);
        metrics
            .denylist_last_synced_timestamp_seconds
            .set(1_700_000_000.0);
        metrics.denylist_stale.set(0.0);

        assert_eq!(metrics.denylist_entries.get(), 42.0);
        assert_eq!(
            metrics.denylist_last_synced_timestamp_seconds.get(),
            1_700_000_000.0
        );
        assert_eq!(metrics.denylist_stale.get(), 0.0);
    }

    #[test]
    fn registering_twice_reports_a_duplicate_error() {
        let reg = registerer();
        WaddleAiMetrics::register(&reg).expect("first registration succeeds");
        let Err(err) = WaddleAiMetrics::register(&reg) else {
            panic!("a second registration of the same names must fail");
        };
        assert!(!err.0.is_empty());
    }
}
