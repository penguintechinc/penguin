# Embedded OpenTelemetry Client Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Embed an OpenTelemetry client in the daemon that exports metrics/traces/logs over OTLP to SigNoz, and expose it to every product module through a one-call `HostServices::telemetry("<module>")` hook — without breaking any existing `HostServices` implementation.

**Architecture:** A new `penguin-otel` crate owns the OTLP pipeline (tracer + meter + logger providers) and a `ModuleTelemetry` wrapper trait that hides the OpenTelemetry API behind stable, testable methods. `HostServices` gains a `telemetry()` method with a **default no-op implementation** (so external-plugin proxies and test fakes keep compiling); the daemon's real `HostServices` overrides it with a per-module-scoped handle. The existing Prometheus `metrics()` path is retained and bridged.

**Tech Stack:** Rust 2024, `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp` (exact pins chosen at build to minimize `Cargo.lock` churn), Tokio, `async-trait`, `thiserror`. Mock OTLP collector for tests via a minimal Tonic/axum receiver.

**Spec:** `docs/superpowers/specs/2026-08-21-endpoint-self-protection-and-modules-design.md` (§4.3)

## Global Constraints

- Edition 2024, rust 1.97, workspace version 0.2.0; inherit via `.workspace = true`.
- **Non-breaking SDK change:** `HostServices::telemetry()` MUST have a default method body, so adding it does not force edits to `penguin-goplugin-host`, `penguin-extplugin`, test fakes, or any other impl.
- Flag `penguin.otel` (PostHog), default OFF. When off, `telemetry()` returns a no-op handle; modules can always call it safely.
- Exporter failures never block a module hot path and never panic: bounded buffer, drop-oldest, log-and-continue.
- New OTLP deps get exact `=x.y.z` pins in root `[workspace.dependencies]`; measure and record the `Cargo.lock` diff (memory: avoid-cargo-lock-churn) — prefer minimal feature sets (`grpc-tonic` only).
- Coverage ≥90% on `penguin-otel`. Every `pub` item documented.

## File Structure

```
crates/penguin-otel/
  Cargo.toml
  src/lib.rs        # crate doc + re-exports
  src/config.rs     # OtelConfig { endpoint, sampling_ratio, enabled } + precedence merge
  src/telemetry.rs  # ModuleTelemetry trait + ScopedTelemetry (real) + NoopTelemetry
  src/pipeline.rs   # OtelPipeline: build providers from OtelConfig; shutdown()
  src/error.rs      # OtelError
  tests/otlp_roundtrip.rs + tests/mock_collector/mod.rs

crates/penguin-sdk/src/host.rs   # add HostServices::telemetry() default method + ModuleTelemetry re-export path
crates/penguin-daemon/src/host.rs # override telemetry() to return a ScopedTelemetry per module
bins/penguind/src/daemon_main.rs  # build OtelPipeline at startup from config + flag; hold + shutdown
bins/pdcli/...                    # `otel status` command
Cargo.toml (root)                 # workspace.dependencies: penguin-otel + opentelemetry crates
```

---

### Task 1: `penguin-otel` config with precedence merge

**Files:**
- Create: `crates/penguin-otel/Cargo.toml`, `src/lib.rs`, `src/config.rs`, `src/error.rs`
- Modify: root `Cargo.toml`
- Test: `src/config.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `OtelConfig { endpoint: String, sampling_ratio: f64, enabled: bool }`; `OtelConfig::merge(local: OtelConfig, console: Option<OtelConfig>) -> OtelConfig` where any `Some(console)` field wins over local (console > local).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::OtelConfig;
    #[test]
    fn console_overrides_local() {
        let local = OtelConfig { endpoint: "http://local:4317".into(), sampling_ratio: 0.1, enabled: false };
        let console = OtelConfig { endpoint: "http://signoz:4317".into(), sampling_ratio: 1.0, enabled: true };
        let merged = OtelConfig::merge(local, Some(console));
        assert_eq!(merged.endpoint, "http://signoz:4317");
        assert_eq!(merged.enabled, true);
        assert!((merged.sampling_ratio - 1.0).abs() < f64::EPSILON);
    }
    #[test]
    fn no_console_keeps_local() {
        let local = OtelConfig { endpoint: "http://local:4317".into(), sampling_ratio: 0.5, enabled: true };
        let merged = OtelConfig::merge(local.clone(), None);
        assert_eq!(merged.endpoint, local.endpoint);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-otel config:: --locked`
Expected: FAIL — crate/`OtelConfig` not defined.

- [ ] **Step 3: Write minimal implementation**

Create the crate; `OtelConfig` (derive `Clone, Debug`) + `merge`. Deps in `Cargo.toml`: `opentelemetry = "=0.<pin>"`, `opentelemetry_sdk = { version = "=0.<pin>", features = ["rt-tokio"] }`, `opentelemetry-otlp = { version = "=0.<pin>", default-features = false, features = ["grpc-tonic", "trace", "metrics", "logs"] }`, `tokio`, `async-trait`, `thiserror`. (Confirm exact compatible versions with `cargo add --dry-run` first; record the lock diff.)

- [ ] **Step 4: Run test to verify it passes**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-otel config:: --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/penguin-otel Cargo.toml Cargo.lock
git commit -m "feat(otel): penguin-otel crate + OtelConfig precedence merge"
```

---

### Task 2: `ModuleTelemetry` trait + `NoopTelemetry`

**Files:**
- Create: `crates/penguin-otel/src/telemetry.rs`
- Test: `src/telemetry.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `pub trait ModuleTelemetry: Send + Sync { fn counter_add(&self, name: &str, value: u64, attrs: &[(&str,&str)]); fn record_span(&self, name: &str, attrs: &[(&str,&str)]); fn emit_log(&self, level: penguin_sdk::LogLevel, message: &str, attrs: &[(&str,&str)]); }`; `pub struct NoopTelemetry;` implementing it as no-ops. This is the stable boundary the SDK re-exports.

- [ ] **Step 1: Write the failing test**

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-otel telemetry::tests::noop --locked`
Expected: FAIL — trait/type not defined.

- [ ] **Step 3: Write minimal implementation**

Define the trait + `NoopTelemetry` (all methods empty bodies). No OpenTelemetry dependency in this file yet — keep the boundary pure so the SDK can re-export it cheaply.

- [ ] **Step 4: Run test to verify it passes**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-otel telemetry::tests::noop --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/penguin-otel/src/telemetry.rs
git commit -m "feat(otel): ModuleTelemetry trait + NoopTelemetry handle"
```

---

### Task 3: `HostServices::telemetry()` default method (non-breaking SDK change)

**Files:**
- Modify: `crates/penguin-sdk/Cargo.toml` (dep `penguin-otel.workspace = true` for the trait re-export — OR define `ModuleTelemetry` in the SDK and have `penguin-otel` depend on the SDK to avoid a cycle; see Step 3), `crates/penguin-sdk/src/host.rs`, `src/lib.rs`
- Test: `crates/penguin-sdk/src/host.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `HostServices::telemetry(&self) -> Arc<dyn ModuleTelemetry>` with default body `Arc::new(NoopTelemetry)`. Re-export `ModuleTelemetry` + `NoopTelemetry` from `penguin_sdk`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod telemetry_default_tests {
    use super::*;
    struct MinimalHost; // implements every OTHER HostServices method, NOT telemetry()
    // ... impl HostServices for MinimalHost with the existing 7 methods ...
    #[test]
    fn telemetry_defaults_to_noop_for_impls_that_do_not_override() {
        let host = MinimalHost;
        // Compiles only if telemetry() has a default body; returns a usable handle.
        host.telemetry().counter_add("x", 1, &[]);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-sdk telemetry_default --locked`
Expected: FAIL — `telemetry` not a member of `HostServices`.

- [ ] **Step 3: Write minimal implementation**

To avoid a crate cycle, define `ModuleTelemetry`/`NoopTelemetry` **in `penguin-sdk`** (Task 2 moves there) and have `penguin-otel` depend on `penguin-sdk` to implement `ModuleTelemetry` for its real `ScopedTelemetry`. Add to the `HostServices` trait:

```rust
/// A telemetry handle scoped to this module (metrics/traces/logs → OTLP/SigNoz).
/// Defaults to a no-op so existing impls need no change and a disabled exporter
/// is always safe to call.
fn telemetry(&self) -> std::sync::Arc<dyn ModuleTelemetry> {
    std::sync::Arc::new(NoopTelemetry)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-sdk --locked && cargo check --workspace --locked`
Expected: PASS — and the whole workspace still compiles (proves non-breaking).

- [ ] **Step 5: Commit**

```bash
git add crates/penguin-sdk crates/penguin-otel Cargo.toml Cargo.lock
git commit -m "feat(sdk): HostServices::telemetry() default no-op hook (non-breaking)"
```

---

### Task 4: Real OTLP pipeline + `ScopedTelemetry`, verified against a mock collector

**Files:**
- Create: `crates/penguin-otel/src/pipeline.rs`, `tests/mock_collector/mod.rs`, `tests/otlp_roundtrip.rs`
- Modify: `crates/penguin-otel/src/telemetry.rs` (add `ScopedTelemetry`)
- Test: `crates/penguin-otel/tests/otlp_roundtrip.rs`

**Interfaces:**
- Produces: `OtelPipeline::build(cfg: &OtelConfig, resource_attrs: &[(&str,&str)]) -> Result<OtelPipeline, OtelError>`; `OtelPipeline::scoped(&self, module: &str) -> Arc<dyn ModuleTelemetry>` (a `ScopedTelemetry` tagged with `module`); `OtelPipeline::shutdown(self)`.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn a_module_counter_reaches_the_collector() {
    let collector = mock_collector::start().await; // minimal OTLP/gRPC metrics receiver
    let cfg = penguin_otel::OtelConfig { endpoint: collector.endpoint(), sampling_ratio: 1.0, enabled: true };
    let pipe = penguin_otel::OtelPipeline::build(&cfg, &[("node_id", "n-1")]).expect("build");
    let t = pipe.scoped("skauswatch");
    t.counter_add("events_total", 5, &[("kind", "scan")]);
    pipe.shutdown();
    let seen = collector.wait_for_metric("events_total").await;
    assert!(seen.attributes_contain("module", "skauswatch"));
    assert!(seen.resource_contains("node_id", "n-1"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-otel --test otlp_roundtrip --locked`
Expected: FAIL — `OtelPipeline` not defined.

- [ ] **Step 3: Write minimal implementation**

`pipeline.rs`: build meter/tracer/logger providers via `opentelemetry_sdk` with an OTLP `grpc-tonic` exporter pointed at `cfg.endpoint`, a `Resource` from `resource_attrs`, sampler from `sampling_ratio`. `ScopedTelemetry` holds a `Meter`/`Tracer`/`Logger` obtained with scope name = module, adds a `module` attribute to every record; `counter_add` uses an up-down counter cache keyed by name. Bounded export queue; on exporter error, log-and-continue. `mock_collector`: a Tonic server implementing the OTLP metrics service, capturing requests.

- [ ] **Step 4: Run test to verify it passes**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-otel --test otlp_roundtrip --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/penguin-otel
git commit -m "feat(otel): OTLP pipeline + ScopedTelemetry, verified via mock collector"
```

---

### Task 5: Daemon wiring — build pipeline at startup, override `telemetry()`, gate on flag

**Files:**
- Modify: `crates/penguin-daemon/src/host.rs` (override `telemetry()` → `pipeline.scoped(module)`), `crates/penguin-daemon/Cargo.toml`, `bins/penguind/src/daemon_main.rs` (construct pipeline from merged config + `license.feature_enabled("penguin.otel")`; hold it; `shutdown()` on daemon stop)
- Test: `crates/penguin-daemon/src/host.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `OtelPipeline` (Task 4), `LicenseChecker::feature_enabled` (SDK).
- Produces: daemon `HostServices::telemetry(module)` returns a real scoped handle when `penguin.otel` is on and a pipeline exists, else `NoopTelemetry`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn host_returns_scoped_telemetry_when_pipeline_present() {
    let host = /* build daemon HostServices with a test OtelPipeline + flag on */;
    let t = host.telemetry(); // scoped to the host's module name
    t.counter_add("probe", 1, &[]); // does not panic; real handle
}
#[test]
fn host_returns_noop_when_flag_off() {
    let host = /* daemon HostServices, penguin.otel = false */;
    // returns NoopTelemetry; safe to call.
    host.telemetry().record_span("x", &[]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-daemon telemetry --locked`
Expected: FAIL — override not present.

- [ ] **Step 3: Write minimal implementation**

Give the daemon's host struct an `Option<Arc<OtelPipeline>>` + module name; override `telemetry()` to return `pipeline.scoped(name)` when `Some` and the flag is enabled, else `Arc::new(NoopTelemetry)`. In `daemon_main.rs`, read `OtelConfig` from daemon config, merge with any console override (SP2 hook: `None` for now), build the pipeline only if `feature_enabled("penguin.otel")`, store it, and call `shutdown()` in the shutdown path.

- [ ] **Step 4: Run test to verify it passes**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-daemon --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/penguin-daemon bins/penguind Cargo.toml
git commit -m "feat(otel): daemon builds pipeline + serves scoped telemetry gated by penguin.otel"
```

---

### Task 6: `pdcli otel status` command

**Files:**
- Modify: `bins/pdcli/src/...` (add `otel status` subcommand that queries the daemon over the control socket for exporter health)
- Test: the pdcli command layer's existing test harness

**Interfaces:**
- Produces: `pdcli otel status` prints endpoint, enabled, and last-export result.

- [ ] **Step 1: Write the failing test** — assert the command is registered and renders a status line for a faked daemon response.
- [ ] **Step 2: Run** — FAIL (command absent).
- [ ] **Step 3: Implement** the subcommand mirroring an existing pdcli status command; add a daemon control RPC `otel_status` if none exists, else reuse the module-status channel.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(otel): pdcli otel status`.

---

### Task 7: Full gate + lock-diff record

- [ ] **Step 1:** `PATH=~/.cargo/bin:$PATH cargo check --workspace --locked && cargo test -p penguin-otel -p penguin-sdk -p penguin-daemon --locked` → PASS.
- [ ] **Step 2:** `cargo fmt --all --check && cargo clippy -p penguin-otel -- -D warnings`; `cargo llvm-cov -p penguin-otel --summary-only` ≥90%.
- [ ] **Step 3:** Record the `Cargo.lock` line-count delta from the OTLP deps in the commit message; if it re-resolved unrelated crates, do a minimal-add pass.
- [ ] **Step 4: Commit** `chore(otel): gate green + record lock diff`.

## Self-Review Notes
- Spec §4.3 coverage: `penguin-otel` pipeline = Tasks 1,4; SDK hook (default no-op) = Tasks 2,3; daemon wiring + flag gating + config precedence = Tasks 1,5; `pdcli otel status` = Task 6; lock-churn discipline = Task 7. ✅
- `ModuleTelemetry`/`NoopTelemetry` live in `penguin-sdk` (Task 3 resolves the cycle) and are re-exported; `ScopedTelemetry` in `penguin-otel`. Names consistent across tasks.
- Non-breaking guarantee is *tested* (Task 3 Step 4 runs `cargo check --workspace`).
- Tamper-event export is provided by this client but emitted from the self-protection plan (that plan calls `host.telemetry()` / a daemon telemetry handle).
