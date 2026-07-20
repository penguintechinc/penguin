//! The six Prometheus collectors Tobogganing reports, registered once in
//! `init` and then genuinely updated from the code paths that change the
//! state each one describes.
//!
//! The Go module registered the same six collectors but only ever updated
//! `tunnelUp`/`connErrors`/`tokenRefreshes`/`handshakeAge`
//! (`go-client/internal/modules/tobogganing/module.go`); `rxBytes` and
//! `txBytes` were registered and never touched again anywhere in the
//! package. [`TobogganingMetrics::record_bytes`] fixes that — see its doc.

use std::sync::atomic::{AtomicU64, Ordering};

use penguin_sdk::{Metrics, MetricsError};
use prometheus::{Counter, Gauge, Opts};

/// Namespace/subsystem shared by every collector — matches the Go module's
/// `prometheus.GaugeOpts`/`CounterOpts{Name: "tobogganing_..."}` naming
/// exactly via the namespace/subsystem/name split this workspace's metrics
/// convention uses (see `penguin-module-squawk::metrics` for the same
/// pattern), so the fully-qualified names stay
/// `penguin_module_tobogganing_tobogganing_*`.
const NAMESPACE: &str = "penguin_module";
const SUBSYSTEM: &str = "tobogganing";

/// Tobogganing's Prometheus collectors.
pub struct TobogganingMetrics {
    pub tunnel_up: Gauge,
    pub handshake_age: Gauge,
    pub rx_bytes: Counter,
    pub tx_bytes: Counter,
    pub token_refreshes: Counter,
    pub conn_errors: Counter,
    /// The device's own cumulative rx counter as of the last
    /// [`record_bytes`](Self::record_bytes) call, so repeated reads of the
    /// same absolute value (a `Counter` only ever moves forward) turn into
    /// the correct `inc_by` delta rather than double-counting.
    last_rx_bytes: AtomicU64,
    /// See `last_rx_bytes`.
    last_tx_bytes: AtomicU64,
}

impl TobogganingMetrics {
    /// Builds all six collectors and registers each with `registerer`,
    /// preserving the exact metric names the Go module used.
    pub fn register(registerer: &dyn Metrics) -> Result<TobogganingMetrics, MetricsError> {
        let tunnel_up = new_gauge(
            "tobogganing_tunnel_up",
            "Whether the WireGuard tunnel is up (1=up, 0=down)",
        )?;
        registerer.register(Box::new(tunnel_up.clone()))?;

        let handshake_age = new_gauge(
            "tobogganing_handshake_age_seconds",
            "Age of the last WireGuard handshake in seconds",
        )?;
        registerer.register(Box::new(handshake_age.clone()))?;

        let rx_bytes = new_counter(
            "tobogganing_rx_bytes_total",
            "Total bytes received on the tunnel",
        )?;
        registerer.register(Box::new(rx_bytes.clone()))?;

        let tx_bytes = new_counter(
            "tobogganing_tx_bytes_total",
            "Total bytes transmitted on the tunnel",
        )?;
        registerer.register(Box::new(tx_bytes.clone()))?;

        let token_refreshes = new_counter(
            "tobogganing_token_refreshes_total",
            "Total number of token refresh operations",
        )?;
        registerer.register(Box::new(token_refreshes.clone()))?;

        let conn_errors = new_counter(
            "tobogganing_connection_errors_total",
            "Total number of connection errors",
        )?;
        registerer.register(Box::new(conn_errors.clone()))?;

        Ok(TobogganingMetrics {
            tunnel_up,
            handshake_age,
            rx_bytes,
            tx_bytes,
            token_refreshes,
            conn_errors,
            last_rx_bytes: AtomicU64::new(0),
            last_tx_bytes: AtomicU64::new(0),
        })
    }

    /// Records the tunnel device's current cumulative rx/tx byte counts.
    ///
    /// `rx_bytes_total`/`tx_bytes_total` are Prometheus `Counter`s, which
    /// only support monotonic `inc_by` — but a [`crate::wireguard::PeerStats`]
    /// read hands back the device's own *absolute* cumulative counters, not
    /// a delta. This converts one to the other: the difference from the
    /// last recorded value is added, and a value that went backwards (the
    /// tunnel was recreated and its counters reset) is treated as a
    /// zero-delta rebase rather than an underflow, so a reconnect can never
    /// produce a negative `inc_by`.
    pub fn record_bytes(&self, rx_bytes: u64, tx_bytes: u64) {
        record_delta(&self.rx_bytes, &self.last_rx_bytes, rx_bytes);
        record_delta(&self.tx_bytes, &self.last_tx_bytes, tx_bytes);
    }
}

/// Adds `new_value.saturating_sub(*last)` to `counter` and stores
/// `new_value` as the new baseline. See
/// [`TobogganingMetrics::record_bytes`]'s doc for why this is a
/// `saturating_sub`, not a plain subtraction.
fn record_delta(counter: &Counter, last: &AtomicU64, new_value: u64) {
    let previous = last.swap(new_value, Ordering::SeqCst);
    let delta = new_value.saturating_sub(previous);
    if delta > 0 {
        counter.inc_by(delta as f64);
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
    fn register_wires_all_six_collectors_with_go_parity_names() {
        let reg = registerer();
        let metrics = TobogganingMetrics::register(&reg).expect("register succeeds");

        let names = reg.names.lock().unwrap().clone();
        let expected = [
            "penguin_module_tobogganing_tobogganing_tunnel_up",
            "penguin_module_tobogganing_tobogganing_handshake_age_seconds",
            "penguin_module_tobogganing_tobogganing_rx_bytes_total",
            "penguin_module_tobogganing_tobogganing_tx_bytes_total",
            "penguin_module_tobogganing_tobogganing_token_refreshes_total",
            "penguin_module_tobogganing_tobogganing_connection_errors_total",
        ];
        for name in expected {
            assert!(names.contains(&name.to_string()), "missing {name}");
        }

        metrics.token_refreshes.inc();
        assert_eq!(metrics.token_refreshes.get(), 1.0);
    }

    #[test]
    fn record_bytes_converts_absolute_counters_into_deltas() {
        let reg = registerer();
        let metrics = TobogganingMetrics::register(&reg).unwrap();

        metrics.record_bytes(100, 50);
        assert_eq!(metrics.rx_bytes.get(), 100.0);
        assert_eq!(metrics.tx_bytes.get(), 50.0);

        metrics.record_bytes(150, 80);
        assert_eq!(metrics.rx_bytes.get(), 150.0);
        assert_eq!(metrics.tx_bytes.get(), 80.0);
    }

    #[test]
    fn record_bytes_treats_a_counter_reset_as_a_zero_delta_rebase() {
        let reg = registerer();
        let metrics = TobogganingMetrics::register(&reg).unwrap();

        metrics.record_bytes(500, 500);
        assert_eq!(metrics.rx_bytes.get(), 500.0);

        // Interface recreated: device counters reset to a smaller value.
        metrics.record_bytes(10, 10);
        assert_eq!(metrics.rx_bytes.get(), 500.0, "must not underflow");

        // Subsequent growth from the new baseline still accumulates.
        metrics.record_bytes(30, 30);
        assert_eq!(metrics.rx_bytes.get(), 520.0);
    }

    #[test]
    fn registering_twice_reports_a_duplicate_error() {
        let reg = registerer();
        TobogganingMetrics::register(&reg).expect("first registration succeeds");
        let Err(err) = TobogganingMetrics::register(&reg) else {
            panic!("a second registration of the same names must fail");
        };
        assert!(!err.0.is_empty());
    }
}
