//! End-to-end proof that a module counter recorded through
//! [`penguin_otel::OtelPipeline`] actually reaches an OTLP/HTTP collector,
//! tagged with both the module attribute `ScopedTelemetry` adds and the
//! resource attribute the pipeline was built with.

mod mock_collector;

// `flavor = "multi_thread"` matters here, not just style: `penguind` always
// runs on `tokio::runtime::Builder::new_multi_thread()` (see
// bins/penguind/src/daemon_main.rs), and the exporter's
// `reqwest-blocking-client` reliably hangs for its full ~10s/5s
// force_flush/shutdown timeouts when driven from a `current_thread` runtime
// (`#[tokio::test]`'s default) even though the request/response themselves
// take under 2ms — reproduced in isolation against opentelemetry_sdk
// directly, so it is not specific to this crate's glue code. Matching the
// daemon's real runtime flavor here is what avoids it, not a workaround.
#[tokio::test(flavor = "multi_thread")]
async fn a_module_counter_reaches_the_collector() {
    let collector = mock_collector::MockCollector::start().await;

    let cfg = penguin_otel::OtelConfig {
        endpoint: collector.endpoint(),
        sampling_ratio: 1.0,
        enabled: true,
    };
    let pipe = penguin_otel::OtelPipeline::build(&cfg, &[("node_id", "n-1")]).expect("build");

    let t = pipe.scoped("skauswatch");
    t.counter_add("events_total", 5, &[("kind", "scan")]);

    pipe.shutdown();

    let seen = collector.wait_for_metric("events_total").await;
    assert!(seen.attributes_contain("module", "skauswatch"));
    assert!(seen.resource_contains("node_id", "n-1"));
}
