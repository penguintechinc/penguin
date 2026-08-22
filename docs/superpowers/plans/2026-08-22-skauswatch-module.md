# SkausWatch Endpoint Module Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a full-functioning `skauswatch` built-in module that enrolls the endpoint with the SkausWatch Manager, heartbeats, reports endpoint events, and pulls agent config — registered and loadable exactly like `squawk`/`tobogganing`.

**Architecture:** Two crates mirroring the established split: `skauswatch-client` (REST client to the SkausWatch Manager agent API, HMAC-authenticated) and `penguin-module-skauswatch` (a `penguin_sdk::Module` wrapping that client with a background report loop, health/status, and a CLI command tree). Registered in `penguin-registry::builtin_modules()`.

**Tech Stack:** Rust 2024, Tokio, `reqwest` (rustls/aws-lc-rs, TLS wired manually as in `squawk-client`), `serde`/`serde_json`, `serde-norway` (YAML config), `prometheus` (metrics via `HostServices::metrics`), `hmac`+`sha2` (HMAC-SHA256 request signing), `async-trait`, `thiserror`.

**Spec:** `docs/superpowers/specs/2026-08-21-endpoint-self-protection-and-modules-design.md` (§4.2)

## Global Constraints

- Rust edition 2024, `rust-version = 1.97`, workspace `version = 0.2.0`; all inherit via `.workspace = true`.
- Dependency pinning: add only crates already resolved in the workspace where possible; new crates (`hmac`, `sha2`) get exact `=x.y.z` pins in root `[workspace.dependencies]`; keep the `Cargo.lock` diff minimal (see memory: avoid-cargo-lock-churn).
- Every `pub` item carries a `///` doc comment (what + why); house style: minimal turbofish/closures, explicit types, named helpers, `if let` over `match` where it reads better.
- Module must load with **no license gate** (`license_feature` empty) — product entitlement is enforced server-side (the Manager gates `skauswatch.endpoint`), matching the other four built-ins.
- Coverage ≥90% on both new crates. Builds/tests run via `make test` or host `PATH=~/.cargo/bin:$PATH cargo test -p <crate> --locked`.
- SkausWatch Manager agent API (source of truth: `~/code/skauswatch/services/manager/src/routes/endpoint.rs`):
  - `POST /api/v1/endpoint/register` → returns `{ agent_id, api_key }`
  - `POST /api/v1/endpoint/heartbeat`
  - `POST /api/v1/endpoint/events`
  - `GET  /api/v1/endpoint/config`
  - Auth on all but register: headers `x-agent-id: <agent_id>` and `x-api-key: <HMAC-SHA256 of the request body, keyed by api_key, hex>`.

## File Structure

```
crates/skauswatch-client/
  Cargo.toml
  src/lib.rs          # crate doc + re-exports
  src/config.rs       # ClientConfig { base_url, enrollment_token, tls } + defaults
  src/auth.rs         # HmacSigner: body -> (x-agent-id, x-api-key) headers
  src/client.rs       # SkausWatchClient: register/heartbeat/report_events/fetch_config
  src/model.rs        # request/response DTOs (Serialize/Deserialize)
  src/error.rs        # ClientError (thiserror)
  tests/client_tests.rs + tests/mock_server/mod.rs   # axum mock, mirrors penguin-licensing tests

crates/penguin-module-skauswatch/
  Cargo.toml
  src/lib.rs          # crate doc + `pub fn factory` + re-export SkausWatchModule
  src/config.rs       # ModuleConfig (YAML) + JSON schema
  src/module.rs       # SkausWatchModule: Module impl + Inner state + report loop
  src/commands.rs     # command_tree() + dispatch handlers
  src/metrics.rs      # SkausWatchMetrics (prometheus collectors)

crates/penguin-registry/src/lib.rs   # add "skauswatch" entry + identity test
Cargo.toml (root)                    # workspace.dependencies: skauswatch-client, penguin-module-skauswatch, hmac, sha2
```

---

### Task 1: `skauswatch-client` scaffold + HMAC signer

**Files:**
- Create: `crates/skauswatch-client/Cargo.toml`, `src/lib.rs`, `src/config.rs`, `src/auth.rs`, `src/error.rs`
- Modify: root `Cargo.toml` (`[workspace.dependencies]`: `skauswatch-client = { path = ... }`, `hmac = "=0.12.1"`, `sha2 = "=0.10.9"`)
- Test: `crates/skauswatch-client/src/auth.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `ClientConfig { base_url: String, enrollment_token: String }`; `HmacSigner::new(agent_id: String, api_key: Vec<u8>)`; `HmacSigner::headers(&self, body: &[u8]) -> Vec<(String, String)>` returning `x-agent-id` + `x-api-key` (lowercase hex HMAC-SHA256).

- [ ] **Step 1: Write the failing test** (`src/auth.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::HmacSigner;

    #[test]
    fn headers_are_stable_hex_hmac_over_body() {
        let signer = HmacSigner::new("agent-7".to_string(), b"secret-key".to_vec());
        let h = signer.headers(br#"{"ping":true}"#);
        let get = |k: &str| h.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("x-agent-id").as_deref(), Some("agent-7"));
        // HMAC-SHA256("secret-key", {"ping":true}) — 64 lowercase hex chars, deterministic.
        let sig = get("x-api-key").expect("x-api-key present");
        assert_eq!(sig.len(), 64);
        assert_eq!(sig, signer.headers(br#"{"ping":true}"#)[1].1, "same body -> same sig");
        assert_ne!(sig, signer.headers(br#"{"ping":false}"#)[1].1, "body change -> sig change");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p skauswatch-client auth:: --locked`
Expected: FAIL — `skauswatch-client` / `HmacSigner` not found.

- [ ] **Step 3: Write minimal implementation**

`Cargo.toml` deps: `reqwest` (features `["json"]`), `rustls`, `webpki-roots`, `tokio`, `serde`, `serde_json`, `thiserror`, `hmac`, `sha2`, `hex` (use `base64`/manual hex; prefer `hex = "0.4"` only if already in lock — else format bytes with a small helper to avoid a new crate). `src/auth.rs`:

```rust
//! Request signing for the SkausWatch agent API: HMAC-SHA256 of the exact
//! request body, keyed by the api_key the Manager returned at register, sent
//! as the `x-api-key` header alongside `x-agent-id`. Mirrors the Manager's
//! HMAC check in `services/manager/src/routes/endpoint.rs`.
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Signs agent requests with the per-agent HMAC key.
pub struct HmacSigner {
    agent_id: String,
    api_key: Vec<u8>,
}

impl HmacSigner {
    /// Builds a signer from the identity the Manager issued at register.
    pub fn new(agent_id: String, api_key: Vec<u8>) -> HmacSigner {
        HmacSigner { agent_id, api_key }
    }

    /// Returns the auth headers for a request carrying `body`.
    pub fn headers(&self, body: &[u8]) -> Vec<(String, String)> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.api_key).expect("HMAC accepts any key length");
        mac.update(body);
        let digest = mac.finalize().into_bytes();
        let mut hex = String::with_capacity(digest.len() * 2);
        for byte in digest {
            hex.push_str(&format!("{byte:02x}"));
        }
        vec![
            ("x-agent-id".to_string(), self.agent_id.clone()),
            ("x-api-key".to_string(), hex),
        ]
    }
}
```

Add `src/config.rs` with `ClientConfig`, `src/error.rs` with `ClientError`, and re-exports in `src/lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p skauswatch-client auth:: --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/skauswatch-client Cargo.toml Cargo.lock
git commit -m "feat(skauswatch-client): scaffold crate + HMAC request signer"
```

---

### Task 2: `register()` against a mock Manager

**Files:**
- Create: `crates/skauswatch-client/src/model.rs`, `src/client.rs`, `tests/mock_server/mod.rs`, `tests/client_tests.rs`
- Test: `crates/skauswatch-client/tests/client_tests.rs`

**Interfaces:**
- Consumes: `ClientConfig`, `HmacSigner` (Task 1).
- Produces: `SkausWatchClient::new(cfg: ClientConfig) -> Result<SkausWatchClient, ClientError>`; `async fn register(&self) -> Result<AgentIdentity, ClientError>` where `AgentIdentity { agent_id: String, api_key: String }`; POSTs `{ enrollment_token, hostname, os, arch, agent_version }` to `/api/v1/endpoint/register`.

- [ ] **Step 1: Write the failing test**

```rust
// tests/client_tests.rs
mod mock_server;
use skauswatch_client::{ClientConfig, SkausWatchClient};

#[tokio::test]
async fn register_posts_enrollment_token_and_returns_identity() {
    let server = mock_server::start_register_ok("agent-42", "key-abc").await;
    let cfg = ClientConfig { base_url: server.base_url(), enrollment_token: "enr-tok".to_string() };
    let client = SkausWatchClient::new(cfg).expect("client builds");
    let id = client.register().await.expect("register ok");
    assert_eq!(id.agent_id, "agent-42");
    assert_eq!(id.api_key, "key-abc");
    assert_eq!(server.last_path(), "/api/v1/endpoint/register");
    assert!(server.last_body_contains("enr-tok"));
}
```

`tests/mock_server/mod.rs`: a small `axum` server (mirror `crates/penguin-licensing/tests/mock_server/mod.rs`) exposing `base_url()`, `last_path()`, `last_body_contains(&str)`, and `start_register_ok(agent_id, api_key)` returning `{ "agent_id":..., "api_key":... }`.

- [ ] **Step 2: Run test to verify it fails**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p skauswatch-client --test client_tests register --locked`
Expected: FAIL — `SkausWatchClient`/`register` not defined.

- [ ] **Step 3: Write minimal implementation**

`src/model.rs`: `RegisterRequest { enrollment_token, hostname, os, arch, agent_version }` (Serialize), `AgentIdentity { agent_id, api_key }` (Deserialize + Clone). `src/client.rs`: build a `reqwest::Client` with rustls+webpki-roots exactly as `squawk-client`/`penguin-licensing` do; `register()` serializes `RegisterRequest`, POSTs to `{base_url}/api/v1/endpoint/register`, deserializes `AgentIdentity`, maps non-2xx to `ClientError::Http { status }`.

- [ ] **Step 4: Run test to verify it passes**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p skauswatch-client --test client_tests register --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/skauswatch-client
git commit -m "feat(skauswatch-client): register() enrolls agent against Manager"
```

---

### Task 3: `heartbeat()`, `report_events()`, `fetch_config()` (HMAC-authed)

**Files:**
- Modify: `crates/skauswatch-client/src/client.rs`, `src/model.rs`, `tests/mock_server/mod.rs`, `tests/client_tests.rs`

**Interfaces:**
- Produces on `SkausWatchClient` (each takes `&self, identity: &AgentIdentity`):
  - `async fn heartbeat(&self, id: &AgentIdentity, status: &HeartbeatBody) -> Result<(), ClientError>`
  - `async fn report_events(&self, id: &AgentIdentity, events: &[EndpointEvent]) -> Result<(), ClientError>`
  - `async fn fetch_config(&self, id: &AgentIdentity) -> Result<AgentConfig, ClientError>`
  - Each attaches `HmacSigner::headers(body)` before sending; `fetch_config` signs an empty body.
- `EndpointEvent { kind: String, severity: String, detail: serde_json::Value, ts_unix: i64 }`; `AgentConfig { heartbeat_secs: u64, extra: serde_json::Value }`.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn heartbeat_sends_hmac_headers() {
    let server = mock_server::start_auth_echo().await; // 200 iff x-agent-id + valid x-api-key present
    let client = mock_server::client_for(&server, "agent-9", "k9");
    let id = skauswatch_client::AgentIdentity { agent_id: "agent-9".into(), api_key: "k9".into() };
    let body = skauswatch_client::HeartbeatBody { healthy: true, module_version: "0.2.0".into() };
    client.heartbeat(&id, &body).await.expect("heartbeat ok");
    assert_eq!(server.last_header("x-agent-id").as_deref(), Some("agent-9"));
    assert_eq!(server.last_header("x-api-key").map(|s| s.len()), Some(64));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p skauswatch-client --test client_tests heartbeat --locked`
Expected: FAIL — `heartbeat`/`HeartbeatBody` not defined.

- [ ] **Step 3: Write minimal implementation**

Add the three methods + DTOs. Factor a private `async fn send_signed(&self, method, path, id, body_bytes) -> Result<reqwest::Response, ClientError>` that serializes, computes headers with `HmacSigner::new(id.agent_id, id.api_key.as_bytes().to_vec())`, applies them, and checks status.

- [ ] **Step 4: Run test to verify it passes**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p skauswatch-client --locked`
Expected: PASS (all client tests).

- [ ] **Step 5: Commit**

```bash
git add crates/skauswatch-client
git commit -m "feat(skauswatch-client): heartbeat/report_events/fetch_config with HMAC auth"
```

---

### Task 4: `penguin-module-skauswatch` scaffold — identity + factory + config schema

**Files:**
- Create: `crates/penguin-module-skauswatch/Cargo.toml`, `src/lib.rs`, `src/config.rs`, `src/metrics.rs`, `src/module.rs` (skeleton)
- Modify: root `Cargo.toml` (`penguin-module-skauswatch = { path = ... }`)
- Test: `crates/penguin-module-skauswatch/src/lib.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `pub fn factory() -> Box<dyn penguin_sdk::Module>`; `SkausWatchModule` whose `info()` returns `ModuleInfo { name: "skauswatch", version: "1.0.0", description: "...", license_feature: "" }` (callable before `init`); `config_schema()` returns the JSON schema bytes.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use penguin_sdk::Module;
    #[test]
    fn factory_reports_identity_before_init() {
        let m = crate::factory();
        let info = m.info();
        assert_eq!(info.name, "skauswatch");
        assert_eq!(info.version, "1.0.0");
        assert!(info.license_feature.is_empty(), "loads core; entitlement enforced server-side");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-module-skauswatch factory_reports_identity --locked`
Expected: FAIL — crate/`factory` not defined.

- [ ] **Step 3: Write minimal implementation**

Mirror `crates/penguin-module-tobogganing` structure. `Cargo.toml` deps: `async-trait`, `thiserror`, `tokio`, `tokio-util`, `serde`, `serde_json`, `serde-norway`, `prometheus`, `penguin-sdk`, `skauswatch-client` (all `.workspace = true`). `src/module.rs` defines `SkausWatchModule` with an `Inner` (`OnceLock<Arc<dyn HostServices>>`, `OnceLock<Arc<SkausWatchClient>>`, `running: AtomicBool`), implementing `Module` with `info()` filled and all other methods `todo!()`-free minimal stubs (`start/stop` return `Ok(())`, `status` returns a default `Status`, `health` a healthy `HealthReport`, `commands` empty, `dispatch` errors "unknown command", `config_schema` = `Some(schema_bytes)`). `pub fn factory() -> Box<dyn Module> { Box::new(SkausWatchModule::new()) }`.

- [ ] **Step 4: Run test to verify it passes**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-module-skauswatch factory_reports_identity --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/penguin-module-skauswatch Cargo.toml Cargo.lock
git commit -m "feat(skauswatch): scaffold module crate with identity + factory"
```

---

### Task 5: Module lifecycle — init/start/stop + background report loop

**Files:**
- Modify: `crates/penguin-module-skauswatch/src/module.rs`, `src/config.rs`, `src/metrics.rs`
- Test: `crates/penguin-module-skauswatch/src/module.rs` (`#[cfg(test)]`, use a fake `HostServices`)

**Interfaces:**
- Consumes: `HostServices` (config bytes, secrets, logger, metrics), `SkausWatchClient` (Task 2–3).
- Produces: `init` builds the client from parsed `ModuleConfig` + stores host; `start` registers-if-needed (persisting `AgentIdentity` via `host.secrets()`), then spawns a heartbeat+report loop (interval from config, default 60s) guarded by a `CancellationToken`; `stop` cancels it and is idempotent; `status`/`health` reflect last successful heartbeat age (degraded after 2× interval), matching tobogganing's `updateHealthProbe` pattern.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test(start_paused = true)]
async fn start_then_stop_is_clean_and_idempotent() {
    let module = SkausWatchModule::new();
    let host = testutil::fake_host(/* config: base_url of a mock, short interval */);
    module.init(host).await.expect("init");
    module.start().await.expect("start returns promptly");
    assert!(module.is_running());
    module.stop().await.expect("stop");
    module.stop().await.expect("stop is idempotent");
    assert!(!module.is_running());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-module-skauswatch start_then_stop --locked`
Expected: FAIL — lifecycle not implemented.

- [ ] **Step 3: Write minimal implementation**

Implement `init/start/stop` mirroring `penguin-module-tobogganing::module` (CancellationToken, `Arc<Inner>` cloned into the spawned task, `running: AtomicBool`). The loop: `tokio::select!` on cancel vs an interval tick; each tick calls `heartbeat` and drains a queued-events buffer via `report_events`; log+count failures, never panic, never exit the loop on a transient error.

- [ ] **Step 4: Run test to verify it passes**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-module-skauswatch --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/penguin-module-skauswatch
git commit -m "feat(skauswatch): init/start/stop lifecycle + heartbeat/report loop"
```

---

### Task 6: CLI command tree + dispatch

**Files:**
- Modify: `crates/penguin-module-skauswatch/src/commands.rs`, `src/module.rs` (wire `commands()`/`dispatch()`)
- Test: `crates/penguin-module-skauswatch/src/commands.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `command_tree() -> Vec<CommandSpec>` with `status` (flag `--json`) and `enroll` (re-run register); `dispatch(path, flags, args)` returns `CommandResult`. Mirror `penguin-module-tobogganing::commands`.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn dispatch_status_json_returns_structured_result() {
    let module = SkausWatchModule::new();
    module.init(testutil::fake_host_default()).await.unwrap();
    let flags = std::collections::HashMap::from([("json".to_string(), "true".to_string())]);
    let out = module.dispatch(&["status".to_string()], &flags, &[]).await.expect("dispatch ok");
    assert!(out.stdout.contains("enrolled") || out.stdout.contains("agent_id"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-module-skauswatch dispatch_status --locked`
Expected: FAIL.

- [ ] **Step 3: Write minimal implementation**

Add `command_tree()` (return from `Module::commands`) and dispatch handlers producing text or JSON `CommandResult`, mirroring tobogganing's `format_status_text` (sort keys for determinism).

- [ ] **Step 4: Run test to verify it passes**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-module-skauswatch --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/penguin-module-skauswatch
git commit -m "feat(skauswatch): CLI status/enroll commands + dispatch"
```

---

### Task 7: Register `skauswatch` as a built-in

**Files:**
- Modify: `crates/penguin-registry/src/lib.rs` (add insert + doc line + two tests), `crates/penguin-registry/Cargo.toml` (`penguin-module-skauswatch.workspace = true`)
- Test: `crates/penguin-registry/src/lib.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `penguin_module_skauswatch::factory` (Task 4).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn skauswatch_is_registered_as_a_builtin() {
    assert!(builtin_modules().contains_key("skauswatch"));
}
#[test]
fn skauswatch_factory_reports_its_own_identity_before_init() {
    let f = builtin_modules();
    let info = f.get("skauswatch").expect("registered")().info();
    assert_eq!(info.name, "skauswatch");
    assert_eq!(info.version, "1.0.0");
    assert!(info.license_feature.is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-registry skauswatch --locked`
Expected: FAIL — key absent.

- [ ] **Step 3: Write minimal implementation**

```rust
registry.insert("skauswatch".to_string(), penguin_module_skauswatch::factory);
```

Add the crate to `penguin-registry/Cargo.toml` deps and a doc line noting skauswatch is the fifth built-in.

- [ ] **Step 4: Run test to verify it passes**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-registry --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/penguin-registry
git commit -m "feat(registry): register skauswatch as a built-in module"
```

---

### Task 8: Workspace wiring + full gate

**Files:**
- Verify: root `Cargo.toml` members glob already includes the new crates (`crates/*`); confirm `[workspace.dependencies]` entries added in Tasks 1/4/7.

- [ ] **Step 1: Full workspace check + tests**

Run: `PATH=~/.cargo/bin:$PATH cargo check --workspace --locked && cargo test -p skauswatch-client -p penguin-module-skauswatch -p penguin-registry --locked`
Expected: PASS, 0 warnings.

- [ ] **Step 2: Lint + coverage**

Run: `PATH=~/.cargo/bin:$PATH cargo fmt --all --check && cargo clippy -p skauswatch-client -p penguin-module-skauswatch -- -D warnings`
Then coverage: `cargo llvm-cov -p skauswatch-client -p penguin-module-skauswatch --summary-only` — assert ≥90%.

- [ ] **Step 3: Commit any fmt/lint fixes**

```bash
git add -A && git commit -m "chore(skauswatch): fmt/clippy/coverage gate green"
```

## Self-Review Notes
- Spec §4.2 coverage: client (4 endpoints, HMAC) = Tasks 1–3; module (lifecycle, commands, health) = Tasks 4–6; registry = Task 7; wiring/coverage = Task 8. ✅
- `AgentIdentity`, `HeartbeatBody`, `EndpointEvent`, `AgentConfig` names are consistent across Tasks 2–5.
- HMAC hex helper avoids adding `hex` crate if not already in lock (checked in Task 1 Step 3).
