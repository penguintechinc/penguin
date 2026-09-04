# penguind — Full Restart to a Modular All-in-One Rust Agent

**Status:** Draft for review · **Date:** 2026-09-04 · **Author:** Justin Bowen (with Claude)
**Repo:** `penguin` → **`penguind`** · **Deliverable of this doc:** the migration spec (implementation plan follows separately)

## 1. Purpose

Restart the endpoint agent from zero — same repo, preserved history, fresh branch/tag/release/version topology — as **`penguind`** ("Penguin daemon"): a single, super-modular, all-in-one Rust agent/client that deploys **native or in a Docker container**, runs **one module per product** as external signed plugins, exposes an **optional tray + GUI**, and runs in one of two first-class **operating modes: `user` or `server`**. All prior lessons, features, and code/modules are carried forward into the new structure.

This is a **greenfield restart**, not an incremental change: every prior feature already built (product modules, self-protection, OpenTelemetry export, FleetDM coexistence, licensing, secrets, IPC, update, tray) is re-homed into the new architecture rather than rewritten.

## 2. Restart mechanics (do this before any deletion)

The current `release/v0.2.X` (tip at restart time; ~`7691154` as of writing) carries all recently-merged work. **Nothing is deleted until a verified backup exists and the user explicitly approves the destructive steps.**

### 2.1 Backup (mandatory precondition)
- **Full-repo `git bundle`** of every ref: `git bundle create penguind-pre-restart-<epoch>.bundle --all` — a single recoverable artifact (verify with `git bundle verify`). Store it as a release asset / durable artifact, not only on disk.
- **Retained archive ref** that is NEVER deleted: tag `archive/pre-restart` (annotated) pinning the exact pre-restart `release/v0.2.X` tip, plus `archive/<branch>` tags for each other branch carrying unmerged work. These stay in the repo forever as the recovery anchor.
- **Inventory** every branch's unmerged content before deletion (the in-flight `feature/*`, `fix/*`, `chore/*` worktree branches) so nothing valuable is lost; fold anything worth keeping into the carry-forward set (§7) or an `archive/*` tag.

### 2.2 History & topology
- **Preserve full git history** under the renamed `penguind` repo (provenance, blame, lessons stay visible).
- **Reset only the topology + version line:** delete git tags, GitHub releases, and the release branches (`release/v0.1.X`, `release/v0.2.X`) — **after** §2.1, and each destructive action user-gated at execution. Deleting release branches deliberately **overrides** `devops.md`'s "release branches are permanent" rule for this one-time restart; the override is recorded here and the backup makes it reversible.

### 2.3 Version reset (per the Version Increment Rule + `versioning` skill)
- The restart deletes all tags → **no published tag exists** → per the rule, **only the build epoch may change; no Major/Minor/Patch increment** until something is tagged.
- Reset the baseline to **`0.1.0.<epoch>`** (`vMajor.Minor.Patch.build`), **unreleased**, via `./scripts/version/update-version.sh` (build-epoch only) throughout the migration.
- **No `release/v0.1.X` branch and no `v0.1.0` tag until the migration is complete.** Cutting `release/v0.1.X` + tagging `v0.1.0` is the migration's completion gate (§6, Phase 5).

## 3. Target architecture

```
                     ┌─────────────────────────── penguind (ONE privileged daemon binary) ───────────────────────────┐
   native OR         │  Supervisor · Module SDK host · Plugin loader + SIGNATURE/HASH verifier · IPC control server    │
   Docker (same      │  SHARED SERVICES (single home, never duplicated): secrets · telemetry/OTel · licensing ·        │
   binary)           │  self-protection · config · enrollment · update · fleetdm-detect                                │
                     └───▲───────────────▲───────────────────────────────────────────────────▲────────────────────────┘
                         │ IPC (control socket, peer-cred authed)                              │ loads/verifies
        ┌────────────────┴───┐   ┌───────┴────────────┐                     ┌──────────────────┴───────────────────────┐
        │ tray (optional)    │   │ GUI (optional)      │   user mode only    │ external signed PLUGIN binaries          │
        │ thin client, no    │   │ thin client, no     │   never on server   │ 1 per product: waddleai, waddles,        │
        │ shared logic       │   │ shared logic        │                     │ tobogganing, squawk, skauswatch, …       │
        └────────────────────┘   └─────────────────────┘                     └──────────────────────────────────────────┘
```

### 3.1 Core daemon (`penguind`) — the single main binary
Holds **all shared functionality** (nothing shared is duplicated anywhere else): the supervisor, the `Module` SDK host, the **external-plugin loader + verifier**, the IPC control server, and every shared service (secrets, telemetry/OTel export, licensing, self-protection, config, enrollment, update, FleetDM detection). Ships as a **native binary and a Docker image built from the same binary** — the container is the daemon, not a re-packaged variant.

### 3.2 Modules = external signed plugins only (one per product)
Per the decision, the core ships with **zero product modules compiled in**. Each product module is a **standalone signed Rust plugin binary** implementing the existing `Module` contract, launched and verified by the daemon through the extplugin / go-plugin host pipeline (process-isolated, gRPC `Module` surface — reused for Rust plugins). Verification is signature + hash against the integrity manifest, so a plugin the daemon didn't ship (or a tampered one) will not load. Which plugins **run** is selected per deployment from (operating mode × config × license entitlement). Process isolation is a security property, not incidental — a crashing or hostile plugin cannot take down the daemon or read another plugin's secrets.

### 3.3 Operating mode — `user` | `server` (first-class)

| Dimension | **user** (desktop/laptop; local user may be an insider threat) | **server** (infra host) |
|---|---|---|
| Overriding priority | **protection + completeness** | **performance + resource efficiency** |
| Self-protection | Full: watchdog, integrity monitor + self-heal, authorized-uninstall gate, hardened service | Lighter/configurable; watchdog optional |
| Licensing metering | per-**seat** | per-**node** |
| Tray / GUI | Available (optional, opt-in) | **Never** built or run |
| Module posture | All modules **enforce** | Per-module **enforce vs detect-only** toggle |
| Secret backend | OS keychain / keystore (per-user) | file / Vault / external-KMS |
| Service shape | per-user launch agent or system service (see §3.6) | system service (systemd/root or container PID 1) |
| Trust of local user | **Not trusted** — daemon runs above the user, resists tampering | Standard operator trust |
| Resource profile | Completeness over footprint | Minimal footprint; lazy-load plugins; tuned intervals |

Mode is **set explicitly at install** (`penguind --mode user|server`, persisted to daemon config) with a sane **auto-default** (headless / running as root / inside a container → `server`; interactive desktop session detected → `user`), always overridable. Mode is resolved once at daemon start and threaded into every subsystem (it is an input to self-protection arming, licensing axis selection, secret-backend choice, plugin-posture resolution, and tray/GUI gating).

### 3.4 Tray + GUI — optional thin clients (user mode only)
Separate binaries that render state and issue commands **over the daemon's IPC** and contain **no shared logic** — all shared functions live in the daemon (§3.1). Built and run only in `user` mode when opted in; `server` and Docker builds neither ship nor launch them. The daemon launches/monitors them (user mode) the way it supervises plugins.

### 3.5 Single install point
One installer/entrypoint lays down everything for the chosen mode: the daemon, the mode-appropriate service registration, and — user mode, opt-in — the tray/GUI + the selected product plugins (verified on install). There is **not** a separate install per component. The Docker image bundles the daemon + selected plugins for `server` mode; `docker run` is the "install."

### 3.6 Security posture for hostile environments (insider-threat)
The `user`-mode threat model treats the machine's own user as a potential adversary:
- The daemon runs at **higher privilege than the user** (system service); the user cannot stop, uninstall, or tamper with it without authorization (self-protection's authorized-uninstall gate + hardened unit + watchdog).
- **Plugins are signature+hash-verified** before load — a user cannot substitute a malicious module.
- **Secrets live in a backend the user cannot read** (keychain scoped away from the user, or a root-only store); tokens and the tamper secret never sit in user-readable plaintext.
- The daemon **does not trust IPC callers by identity alone** — peer-credential checks + scoped commands; the tray/GUI get only what an unprivileged local client should have.
- **Resource discipline** is itself a hardening goal: minimal attack surface (external plugins loaded only when entitled/enabled), lean server footprint.

## 4. Super-modularity contract
- One `Module` SDK trait, one plugin ABI (the gRPC/go-plugin surface), one verification pipeline — every product is a plugin, no special-casing.
- Adding a product = ship a new signed plugin binary + a license entitlement; **zero core changes**.
- The core never imports a product crate. Shared building blocks (client crates, telemetry, secrets) are libraries the plugins link, or services the daemon exposes over the host interface — not compiled-in product logic.

## 5. Full rename `penguin` → `penguind`
- **GitHub repo** `penguin` → `penguind` (GitHub redirects old URLs); update git remotes.
- **Artifact/image paths** → `ghcr.io/penguintechinc/penguind/{service}:{tag}`; local alpha registry paths.
- **CI/CD**: every workflow ref, image name, and the five-tier tag scheme rebased onto `penguind`.
- **Crates**: rename every internal crate `penguin-* → penguind-*` (e.g. `penguin-sdk → penguind-sdk`, `penguin-daemon → penguind-daemon`, `penguin-module-* → penguind-module-*`, client crates, `penguin-selfprotect → penguind-selfprotect`, etc.) — touches every `Cargo.toml` + every `use`/path. Binaries `penguind` and `pdcli` already correctly named; `penguin-tray → penguind-tray`.
- **Docs**: `docs/`, READMEs, runbooks (`docs/self-protection.md`, `docs/fleetdm-coexistence.md`), and the SPIFFE/service identity strings referencing the product name.

## 6. Migration phasing (each phase independently shippable; NO release/tag until Phase 5)

- **Phase 0 — Backup + restart topology.** §2: bundle + `archive/*` refs, verified. Then (user-gated) delete tags/releases/release-branches; reset version to `0.1.0.<epoch>` unreleased. Repo rename + remote/CI/image-path rename.
- **Phase 1 — Crate rename.** `penguin-* → penguind-*` across the workspace; workspace compiles + tests green under the new names. Pure mechanical rename, no behavior change.
- **Phase 2 — Externalize modules.** Move each product module from the built-in registry to a standalone signed Rust **plugin binary**; the core daemon ships with none built in; the loader/verifier selects + launches them. Per-module parity tests carry over.
- **Phase 3 — Operating mode.** Introduce `user`/`server` as a first-class resolved mode; thread it into self-protection arming, licensing axis (seat/node), secret backend, plugin posture (enforce/detect-only), and tray/GUI gating. Explicit `--mode` + auto-default.
- **Phase 4 — Single install + tray/GUI thin clients + Docker.** One install point per mode; tray/GUI reduced to IPC thin clients (user-mode only); the Docker image (server, daemon + selected plugins) built in CI. Hostile-environment hardening (§3.6) finalized.
- **Phase 5 — Completion gate.** Full green across all phases → cut `release/v0.1.X`, tag `v0.1.0` (the first release of penguind). Only here does a release exist.

## 7. Carry-forward inventory (lessons, features, code)
Re-homed into the new structure, not rewritten:
- **Product modules** → external signed plugins: waddleai, waddles (waddlebot), tobogganing, squawk, skauswatch (already conformed to the real SkausWatch Manager contract).
- **Shared services** → daemon: `penguind-selfprotect` (signed integrity + self-heal, watchdog, argon2 tamper-secret, break-glass authz, authorized-uninstall, hardened units, tamper→OTel), `penguind-otel` (OTLP→SigNoz + the `HostServices::telemetry()` hook), licensing (seat/node, `--dev`), secrets, IPC, update, config, FleetDM detect.
- **Docs**: self-protection runbook, FleetDM coexistence, the earlier SP1–SP4 design (agent↔console channel, central chart bundling SigNoz+FleetDM, fleet-management server API+WebUI) — SP2–SP4 remain forward roadmap, now naturally the `server`-mode + central-console track.
- **Hard-won lessons** (encode as guardrails): build API clients from the server's handler code, not route names (the SkausWatch fabricated-contract failure); keep `Cargo.lock` churn minimal; every gate must be able to fail; read hook/CI logs, never trust a bare exit code.

## 8. Standards & constraints
- **RustLang** throughout (security-sensitive → Rust/Python only; this is Rust). Edition 2024, pinned toolchain.
- **≥90% coverage** on every crate; builds fail below.
- **Feature flags** (PostHog, default OFF) gate each product module + new capability; graceful offline degradation.
- **Licensing**: seat (user mode) and node (server mode) axes enforced independently per `critical-rules.md`.
- **Dependency pinning** exact; `Cargo.lock` committed; `cargo deny` clean (advisories/bans/licenses/sources).
- **Rootless containers** at both layers; the Docker image runs `USER` non-root except where a module's declared capability requires otherwise (documented exception).
- **OpenAPI / gRPC** contracts where a REST/gRPC surface exists (the future central console, SP2–SP4).

## 9. Risks & open questions
- **Externalizing modules is the biggest lift** — the current build compiles modules in; moving all to signed plugin binaries reworks packaging, the loader path, and per-module distribution/signing. Phase 2 is the critical path.
- **Rust plugins over the go-plugin gRPC handshake** need confirming end-to-end for Rust-launched plugins (the pipeline exists but has been exercised primarily for the Go oracle) — a Phase-2 spike de-risks it.
- **Deleting release branches is irreversible-in-spirit** — mitigated by §2.1 backup; execution stays user-gated per destructive step.
- **`user`-mode privilege model** (daemon above the user) must not break legitimate desktop UX or single-user dev (`--dev`); the existing "armed only when enrolled" default keeps dev friction-free.
- **Secret backend on server** (file vs Vault vs external-KMS) — pick the baseline in Phase 3; external-KMS is an Enterprise upsell per licensing tiers.

## 10. Completion definition
Migration is complete when: repo + crates renamed and green; all product modules run as external signed plugins with the core carrying none; `user`/`server` mode governs the §3.3 matrix; single-install + native + Docker + optional tray/GUI all work; hostile-environment hardening verified; every gate green at ≥90% coverage — at which point `release/v0.1.X` is cut and `v0.1.0` tagged.
