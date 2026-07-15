# penguin — PenguinTech unified endpoint agent

One hardened endpoint agent — a privileged daemon (`penguind`), an unprivileged
CLI (`penguin`), and a system tray (`penguin-tray`) — that hosts every
PenguinTech product (Squawk DoH, Tobogganing SASE, future ones) as a module
behind a single plugin framework.

## Status: Go → Rust migration in progress

The original **Go** implementation is complete and lives, frozen, under
[`go-client/`](go-client/). It builds standalone and doubles as the
feature-parity conformance oracle for the rewrite.

This repository root is a **Rust** Cargo workspace that is reimplementing the
agent with 100% feature parity (plus completion of the Go build's remaining
stubs). The Rust daemon stays **wire-compatible with hashicorp go-plugin
v1.7.0**, so existing Go-built external plugins keep loading unchanged.

### Layout

| Path | What |
|------|------|
| `crates/` | Library crates (sdk, ipc, daemon, go-plugin host, squawk/tobogganing modules, …) |
| `bins/` | `penguind`, `penguin`, `penguin-tray` |
| `proto/` | Canonical `.proto` sources (single source of truth) |
| `examples/plugin-hello-rs/` | Example Rust plugin (reverse go-plugin conformance) |
| `go-client/` | **Frozen** Go reference implementation (own Go module) |

### Build

```bash
cargo build --workspace          # Rust workspace
make -C go-client build test      # frozen Go client
```

The migration plan (crate selection, milestones, risk register) lives outside
the repo in the session plan file.
