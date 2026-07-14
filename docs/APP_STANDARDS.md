# Penguin Endpoint Agent — App Standards

App-specific architecture and constraints. Company-wide standards:
`docs/standards/`. Full implementation plan history: see repo issues/PRs.

## What this is

One hardened desktop/endpoint agent for all PenguinTech products. Product
clients are modules on a shared core (auth, licensing/flags, secrets, config,
IPC, tray, self-update, packaging). Go 1.25, three binaries:

- `penguind` — privileged daemon (system service). Hosts all modules; every
  privileged op (WireGuard-compatible tunnels, port-53 bind, resolver rewrite)
  happens here under least-privilege capabilities.
- `penguin` — unprivileged CLI over authenticated local IPC.
- `penguin-tray` — unprivileged user-session tray (separate binary; cgo only
  here, so daemon/CLI stay pure-Go and cross-compilable).

## Module contract

`pkg/sdk.Module`: `Info, Init(host), Start, Stop, Status, Health, Commands()
(declarative CLI tree), Dispatch(path, flags, args), ConfigSchema()`.
`HostServices` (daemon-injected): sanitized logger, namespaced secrets store,
license/feature-flag checker, metrics registerer, data dir, event sink.

- **Compiled-in modules**: one factory line in `internal/registry`.
- **External plugins**: HashiCorp go-plugin (gRPC) binaries under a root-owned
  plugins dir; verified before launch — manifest → sha256 → minisign signature
  against **pinned publisher keys (no TOFU)** → SecureConfig + AutoMTLS
  handshake. Extra publisher keys only via root-owned
  `/etc/penguin/trusted-publishers.d/*.pub`.

The CLI never links module code: it builds its cobra tree from the daemon's
`ListCommands` RPC and forwards invocations via `Dispatch`.

## IPC & local authn

gRPC over unix socket `/run/penguin/penguind.sock` (0660 root:penguin) with
SO_PEERCRED checks; Windows named pipe `\\.\pipe\penguind` with SDDL
restricting to Administrators/SYSTEM + configured group.

## Security invariants

- Zero secrets in the repo or distributed builds. Tokens/keys live only in the
  OS secure store (`internal/secrets`, 99designs/keyring; encrypted-file
  backend for the headless daemon keyed by root-only
  `/var/lib/penguind/keyring.key`).
- License/feature flags via `https://license.penguintech.io`
  (`internal/licensing`): offline cache with grace TTL; unreachable server ⇒
  cached result; unknown flags default OFF; never crash. Flag keys:
  `penguin.{module-or-feature}`.
- Self-update only applies artifacts whose minisign signature verifies against
  the embedded PenguinTech key.
- Sanitized logging only (penguin-libs go-common SanitizedLogger) — no tokens,
  PII, or full emails.
- Resolver/tunnel changes must restore on module Stop AND daemon shutdown; a
  crash-recovery marker restores on next start.

## Dependency risk register

| # | Risk | Mitigation in place |
|---|---|---|
| R1 | penguin-libs Go modules lack `packages/go-X/vX.Y.Z` tags | Pinned by commit pseudo-version (`v0.0.0-20260521191846-f8a443a6f88c`, origin/main). Upstream: add subdir tags. |
| R2 | go-aaa's `replace ../go-common` not inherited by consumers | Consumer-side `replace` in our go.mod pins go-common to the same commit. Remove once upstream tags exist. |
| R3 | squawk-client-go is an untagged subdirectory module | Pin pseudo-version at reviewed commit; upstream: tag `squawk-client-go/vX.Y.Z`. |
| R4 | No Go system-DNS set/restore exists | New `internal/modules/squawk/sysresolver*.go` (per-OS). |
| R5 | tobogganing native client code is `internal/` | Ported into `internal/modules/tobogganing` (GUI dropped). |
| R6 | Tray needs cgo on macOS | Tray isolated in `penguin-tray` binary. |
| R7 | Secret Service unavailable to headless root daemon | keyring encrypted-file backend fallback (root-only key file). |
| R8 | quic-go version skew (squawk vs go-h3) | MVS picks max; verified at module-import time. |
| R9 | "WireGuard" trademark | Docs say "WireGuard-compatible"; embedded wireguard-go is MIT. |
| R10 | Windows pipe auth | go-winio SDDL `D:P(A;;GA;;;BA)(A;;GA;;;SY)`. |

## Upstream work queued (do not push without approval)

- penguin-libs: tag `packages/go-common`, `go-h3`, `go-aaa`; fix go-aaa require
  on go-common to a real version.
- squawk: tag `squawk-client-go/vX.Y.Z`.

## Coverage policy (two gates)

The house 90% bar applies to hand-written logic. Two gates enforce it:

**Enforced coverage gate = the unit gate at 90%.** `make test` runs it everywhere
(dev + CI, unprivileged) and fails below 90. It measures hand-written logic,
excluding from the denominator:

- generated `*.pb.go`
- `cmd/` main wiring + `examples/` (exercised by `make smoke-test` + E2E)
- zero-logic OS/framework adapters isolated into their own files: `plugin_glue.go`
  (go-plugin boilerplate), `vpn_wgctrl.go` (kernel WireGuard adapter),
  `sysresolver_resolvectl_linux.go` (resolvectl exec)
- the subprocess/socket **orchestration** only reachable with a real peer/child:
  `internal/ipc` transport, `internal/extplugin/client.go`

**Integration tier = functional validation of that boundary, not a second hard
gate.** `//go:build integration` files compile *in addition to* the unit tests, so
`make test-integration` (privileged CI, `integration.yml`) runs the real
subprocess plugin lifecycle, the `SO_PEERCRED` socket handshake, the host-service
callbacks, the system resolver, and a WireGuard device against genuine kernels and
processes. `make test-integration-cover` prints the combined boundary-inclusive
coverage (≈90%) as an **informational** report — it is not a blocking gate,
because part of the boundary (the go-plugin process entrypoint, kernel wgctrl
adapters) is structurally uncoverable and root-gated tests self-skip
(`os.Geteuid() != 0`) off the privileged runner.

When adding code: put logic under the unit gate. If something is a genuine
OS/subprocess boundary, isolate it into a dedicated adapter file (excluded, like
generated code) and add a `//go:build integration` test that exercises it for
real — do not hide untested logic behind the exclusion.

## Adding a new product module (checklist)

1. `internal/modules/<name>/module.go` implementing `sdk.Module`.
2. Register: one factory line in `internal/registry/registry.go`.
3. Config schema + `/etc/penguin/modules.d/<name>.yaml` example.
4. Run the SDK conformance suite: `sdktest.TestModule(t, <name>.New)`.
5. Feature flag `penguin.<name>` (default OFF) + license gating if enterprise.
6. Tray-worthy commands flagged `Tray: true` in `Commands()`.
