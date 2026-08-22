# Endpoint Self-Protection, Core Modules, and the Central Control Plane

**Status:** Draft for review · **Date:** 2026-08-21 · **Branch base:** `release/v0.2.X` (`penguin`)
**Author:** Justin Bowen (with Claude)

## 1. Purpose & north star

Turn the penguin endpoint agent into a self-defending, centrally-manageable
endpoint platform, and offer **"MDM + monitoring + Penguin software" out of the
gate**. An operator deploys one central server chart and gets: a fleet console,
SigNoz (monitoring), and FleetDM (MDM) — with penguin agents and `fleetd`
running side by side on every endpoint.

This spec covers **SP1 in implementation-level detail** and **SP2–SP4 at the
contract level** (they get their own specs). SP1 is self-contained and is the
work being built now.

## 2. Current state (verified on `origin/release/v0.2.X`)

| Area | State |
|---|---|
| Product modules | `waddleai`, `waddlebot` (Waddles), `tobogganing`, `squawk` — all implemented (~6–10k LoC each) and registered in `penguin-registry::builtin_modules()` |
| SkausWatch module | **absent** (net-new) |
| OpenTelemetry module | **absent** (net-new) |
| Self-protection | **absent** — `penguind service uninstall` removes the agent trivially |
| Building blocks | supervisor w/ backoff restart; real `penguind service install/uninstall` (systemd/launchd); `penguin-update` signature-verifying updater (ed25519); `penguin-secrets` OS keychain; `penguin-telemetry`; `penguin-licensing` (validates against `license.penguintech.io` `/api/v2/validate`) |
| Server/console | **none in `penguin`** — agent only calls the license server. No enrollment, node registry, config pull, command channel, or tamper sink exists yet |
| Compile | workspace `cargo check --workspace --locked` — result recorded in §11 |

## 3. Target architecture

```
ENDPOINT (end-user machine)
  ├─ penguin agent (penguind): modules
  │     waddleai · waddles · tobogganing · squawk · skauswatch   (all hook into the embedded OTel client)
  │     + SELF-PROTECTION (watchdog + integrity + authorized teardown)
  └─ fleetd (FleetDM client)  ──► MDM / osquery policies + inventory   [we install alongside]

CENTRAL CONTROL PLANE  (dual: pick either, same agent↔console protocol)
  (a) penguin's OWN lightweight mgmt plane — server API + WebUI + Helm chart, IN the penguin repo
  (b) penguincloud module — the hosted/portal path
  Narrow scope: the core end-user / endpoint experience + config. Responsibilities:
      • signed-module store            • source of truth for valid signatures / binary hashes
      • enrollment authority           • module + per-module-config distributor
      • tamper-event sink              • node deauthorization authority

CENTRAL SERVER HELM CHART (everything default-on): mgmt server + SigNoz Community
      + FleetDM Community + fleetd rollout.  Lives in penguin repo (server chart) AND penguincloud (module).

flows:  agent →OTLP→ SigNoz   ·   agent ↔ console (enroll / checkin / config / tamper / deauth)
        agent →validate→ license-server   ·   fleetd ↔ FleetDM
```

### Repo placement (per user decision)
- **`penguin`**: client agent (this branch) + a **lightweight management-plane server (API + WebUI)** + a **server Helm chart** bundling SigNoz + FleetDM Community + fleetd. Standalone control.
- **`penguincloud`**: the same fleet-management capability as a **portal module**, and the bundle as an umbrella module. Hosted control.
- **`penguin-signoz`** (SigNoz chart) and **`license-server`** (entitlement/PostHog) are reused, not rebuilt.

> Note: this deliberately grows a *narrow* server tier inside `penguin`, overriding the
> default "penguin is client-only" reading of `client.md`. Recorded as an explicit product decision.

## 4. SP1 — client hardening & modules (BUILD NOW)

Split into independently-mergeable branches off `release/v0.2.X`:
`fix/workspace-compile` (only if §11 is red), `feature/agent-self-protection`,
`feature/skauswatch-module`, `feature/embedded-otel-client`, plus fleetd
coexistence docs/detect folded into self-protection or its own `docs/` branch.

### 4.1 Self-protection subsystem  →  new crate `penguin-selfprotect`

Legitimate, admin-authorized tamper resistance (the EDR/RMM pattern). **Explicitly NOT**
process/file hiding, AV/OS-defense tampering, or anti-forensics. OS root is never blocked at
the kernel level; an authorized admin is never permanently locked out.

**Packaging (chosen):** one binary, a `penguind watchdog` subcommand + shared logic in
`penguin-selfprotect`. No second signed artifact. (Rejected: separate `pwatchdog` binary —
extra artifact to version/sign/ship; in-process thread only — no protection against a full
process kill.)

Four layers:

1. **Service hardening** — extend `bins/penguind/src/service/{linux,macos}.rs` (+ new `windows.rs`):
   systemd `Restart=always` / `StartLimitIntervalSec=0`; launchd `KeepAlive=true`; Windows
   service auto-restart recovery. Admin can still stop via the service manager — never blocked.
2. **Mutual watchdog** — `penguind` and `penguind watchdog` are two processes that monitor each
   other's liveness (pidfile + heartbeat over the existing control socket) and relaunch the peer
   on unexpected death. Belt-and-suspenders where the service manager can't be trusted.
3. **Integrity monitor + self-heal** — periodic hash-verify of the daemon binary, service unit,
   and config against a **signed manifest** (reuse `penguin-update`'s ed25519 verify). On
   tamper/deletion: restore from a root-owned protected copy, then emit a **tamper event**
   (§4.1.a) to telemetry + OTel + the console sink. The manifest's source of truth is the central
   server (SP2); SP1 consumes a **locally-provisioned signed manifest** via a `ManifestSource`
   trait so the server implementation drops in later without touching the monitor.
4. **Authorized teardown gate** — wraps uninstall + daemon shutdown. Two authorization paths:
   - **Local:** root/admin **AND** a tamper-protection secret set at enroll (Argon2-hashed in a
     root-only store via `penguin-secrets`), **or** an offline **break-glass** signed uninstall
     token verified against Penguin's pubkey offline.
   - **Console:** the central server deauthorizes the node (SP2); on deauth, protection disarms.
   - CLI: `penguind service uninstall --auth <secret|token>`; a clear error names the break-glass
     procedure when refused.

**Armed only when enrolled.** A fresh/dev agent is unarmed (no dev friction). Whole subsystem
behind PostHog flag `penguin.self-protection` (default OFF, cached offline; graceful degradation
= last-known value, never crash).

**4.1.a Contracts SP1 defines (consumed by SP2):**
- `TamperEvent { node_id, kind (BinaryModified|UnitModified|ConfigModified|ProcessKilled|UnauthorizedUninstall), path, expected_hash, actual_hash, ts, remediation }`
- `IntegrityManifest { version, entries: [{path, sha256, mode}], signature }` + `ManifestSource` trait.
- `TeardownAuthz { NodeDeauthorized | LocalSecret(hash) | BreakGlassToken(sig) }`.
- `ConsoleSink` trait (tamper report + deauth poll) — SP1 ships a no-op/local impl; SP2 the HTTP impl.

**Errors:** unreachable manifest source → keep last-known-good manifest, do not disarm, warn.
Integrity check I/O error → warn + retry, never crash the daemon. Watchdog respawn storms →
bounded by the existing backoff formula.

**Tests:** integrity detects modified/deleted binary+unit+config; self-heal restores; teardown
refuses without authz and accepts each valid path; break-glass token verify accepts good / rejects
tampered; watchdog relaunches a killed peer (faked process handle); armed-only-when-enrolled.

### 4.2 SkausWatch module (full functional)

SkausWatch = S3 malware + threat-intel + vuln-scanning platform with a Go endpoint agent
(K8s DaemonSet). We port the endpoint-agent role to run under `penguind`.

- `crates/skauswatch-client` — REST client to the SkausWatch **Manager** API (`/api/v1`, agent JWT
  auth), modeled on `~/code/skauswatch` `services/manager` + `k8s/helm/endpoint-agent` (exact
  endpoints read from that repo during build). Reuses workspace `reqwest`/TLS deps (avoid lock churn).
- `crates/penguin-module-skauswatch` — `Module` impl mirroring `penguin-module-tobogganing`
  (config/auth/http/module/commands/metrics): endpoint-posture + scan-finding reporting loop,
  health/status, CLI commands (`skauswatch status`, `skauswatch scan …`), clean stop.
- Register `"skauswatch"` in `penguin-registry`; `license_feature` empty (loads core; product
  entitlement enforced server-side, matching the other four). Registry identity test added.

### 4.3 Embedded OpenTelemetry client (SDK hook for all modules)

The OTel agent is **embedded telemetry infrastructure**, not a peer supervised module — so product
modules can *depend on it and hook in*, which a sibling module could not provide.

- `crates/penguin-otel` — the OTLP pipeline: tracer + meter + logger providers exporting over
  **OTLP** (`opentelemetry` + `opentelemetry-otlp` + `opentelemetry_sdk`, exact pins chosen at build
  to minimize lock churn) to a configured **SigNoz** endpoint. Built once at daemon startup and held
  in the daemon's telemetry layer. Resource attributes: `node_id`, agent version, tenant, detected
  FleetDM presence (§4.4).
- **The module hook** — extend `penguin-sdk::HostServices` (which every module already receives at
  `init`) so a module gets a per-module, pre-scoped telemetry handle in **one call**:
  `host.telemetry("skauswatch")` → `{ meter, tracer, logger }` already tagged with the module name.
  This subsumes today's `Metrics`/`module_registerer` path (kept as a thin compatibility shim over
  the meter). A module emits a metric / span / log without ever touching OTLP or the SigNoz endpoint.
- **Central sink** — daemon-internal telemetry (supervisor restarts, module health) and **tamper
  events** (§4.1.a) flow through the same client, so everything lands in SigNoz.
- **Config** — SigNoz endpoint, sampling, and enable/disable from daemon config **and** console
  override (SP2, console > local). Exporter unreachable → buffer within OTLP limits, drop-oldest,
  never block a module's hot path, never crash. `pdcli otel status` reports exporter health.
- Wrapped in PostHog flag `penguin.otel` (default OFF; when off, the hook returns a **no-op handle**
  so modules still call it safely).
- Tests: hook returns a working scoped handle; a module metric/span/log reaches a mock OTLP
  collector; tamper + daemon telemetry flow through; config precedence (console > local); no-op
  handle when the flag is off; exporter-down neither blocks nor panics.

### 4.4 FleetDM coexistence (recommend + detect, do NOT rebuild)

- Docs: recommend deploying FleetDM (`fleetd`/osquery) for MDM/system monitoring/policies; document
  the responsibility split (FleetDM = MDM/inventory/policy; penguin = product modules +
  self-protection + OTel/threat reporting). Self-protection guards **only** our own PIDs — never
  `fleetd`/`osqueryd`.
- Light detect: report `fleetd` presence/version in agent status + OTel attributes so the console
  sees coexistence. No policy engine of our own.

## 5. SP2 — agent ↔ console protocol (spec-only here)

Realizes the central-server responsibilities from §3 as a versioned API + agent client
(`/api/v1`, tenant-scoped JWT, `api_version` field per backend.md):
`enroll` → node identity + tamper secret bootstrap; `checkin` → desired module set + per-module
config + current signed integrity manifest; `report-tamper` → sink for §4.1.a events;
`deauthorize` → disarm + uninstall authorization; `modules` → signed-artifact store + hash source
of truth. Agent side implements the `ManifestSource` + `ConsoleSink` traits SP1 defined. Lights up
self-protection's console path and remote config/module management.

## 6. SP3 — central server Helm chart (spec-only here)

Umbrella chart, everything default-on: penguin mgmt server + **SigNoz Community** + **FleetDM
Community** + a **fleetd rollout** alongside penguin agents. Lives as a **server chart in `penguin`**
and a **module in penguincloud**. External images pinned by SHA256 digest; rootless at both layers;
default-deny CiliumNetworkPolicy; per-service DB accounts. Reuse `penguin-signoz`'s SigNoz chart as
the SigNoz subchart.

## 7. SP4 — fleet-management server API + WebUI (spec-only here)

The narrow console surface: enroll/list endpoints, choose+configure modules, view status/telemetry,
view tamper events, deauthorize. **In `penguin`** as the lightweight standalone plane; **in
penguincloud** as the hosted product module. Both consume the SP2 protocol. OpenAPI 3.x, authed docs
(split public-login + full behind JWT), OIDC scopes, tenant isolation.

## 8. Standards & rollout

- **Feature flags:** `penguin.self-protection`, `penguin.module.skauswatch`, `penguin.otel`
  — PostHog, default OFF, cached offline.
- **Coverage:** ≥90% on new crates. `make test` / `make docker-test`.
- **Deps:** exact pins, `Cargo.lock` committed, `cargo deny`; keep lock diff minimal (add only
  OTLP + skauswatch client deps).
- **Versioning:** `.version` per the tag-gated rule (build epoch only unless current is tagged).
- **Docs:** module docs, FleetDM coexistence guide, self-protection + break-glass runbook.
- **Tray:** add SkausWatch/OTel to `penguin-tray-model` if the tray lists modules.
- **PRs:** SP1 feature branches → `release/v0.2.X` (auto-merge when green). `release/v0.2.X` → `main`
  stays user-gated.

## 9. Security posture (self-protection, explicit)

Allowed: service hardening, mutual watchdog restart, integrity verify + self-heal from a protected
copy, authorized-uninstall requiring admin+secret/token or console deauth, tamper alerting.
Excluded: hiding processes/files, disabling AV/OS security tooling, anti-forensics, blocking OS root
at the kernel, any permanent lockout of an authorized admin. Break-glass + console deauth are the
guaranteed overrides.

## 10. Risks & open questions

- **Windows service backend** is noted out-of-scope in the current daemon; adding recovery actions +
  the `windows.rs` service backend is the largest new surface — may split to its own branch.
- **SkausWatch API shape** confirmed against `~/code/skauswatch` at build time; endpoints may differ
  from the modeled `/api/v1`.
- **OTLP dep footprint** vs. the avoid-lock-churn rule — measure the lock diff; prefer minimal
  feature sets.
- Self-protection is **armed only when enrolled**, so before SP2 exists it arms against a
  locally-provisioned manifest/secret only (documented).

## 11. Compile baseline (explicit "make sure it compiles")

`cargo check --workspace --locked` on synced `release/v0.2.X` (`9724816`): **PASS** — 392 crates checked, 0 errors, 0 warnings, finished in 22.31s (host cargo 1.97, protoc 27.1). Desktop (`desktop/`, excluded from workspace) checked separately.
