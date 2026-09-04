# penguin — PenguinTech unified endpoint agent

One hardened endpoint agent — a privileged daemon (`penguind`), an unprivileged
CLI (`penguin`), and a system tray (`penguin-tray`) — that hosts every
PenguinTech product (Squawk DoH, Tobogganing SASE, future ones) as a module
behind a single plugin framework.

## Status: Go → Rust migration complete

This repository root is a **Rust** Cargo workspace implementing the agent with
100% feature parity with the original Go implementation (plus completion of
the Go build's remaining stubs). The original Go implementation (`go-client/`)
was kept frozen as a feature-parity conformance oracle through the rewrite and
has since been removed — its only remaining purpose was generating stale
Dependabot bump PRs against code that would never accept them. See
[`docs/PARITY.md`](docs/PARITY.md) for the record of every deliberate
divergence from Go behaviour.

The Rust daemon remains **wire-compatible with hashicorp go-plugin v1.7.0**,
so existing Go-built external plugins keep loading unchanged — this wire
protocol support (`crates/penguin-goplugin-host`) is pure Rust and has no
dependency on the removed Go source tree.

### Layout

| Path | What |
|------|------|
| `crates/` | Library crates (sdk, ipc, daemon, go-plugin host, squawk/tobogganing modules, …) |
| `bins/` | `penguind`, `penguin`, `penguin-tray` |
| `proto/` | Canonical `.proto` sources (single source of truth) |
| `examples/plugin-hello-rs/` | Example Rust plugin (reverse go-plugin conformance) |

### Build

```bash
cargo build --workspace          # Rust workspace
```
