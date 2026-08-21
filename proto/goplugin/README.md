# Vendored hashicorp/go-plugin protos (MPL-2.0)

`grpc_broker.proto`, `grpc_controller.proto`, and `grpc_stdio.proto` are
vendored **verbatim, with their MPL-2.0 headers intact**, from
[hashicorp/go-plugin](https://github.com/hashicorp/go-plugin) **v1.7.0**
(`internal/plugin/`). That is the exact version the (now-removed) frozen Go
client depended on, which is what made wire compatibility checkable during the
Go→Rust migration.

## Why they are here

The Rust daemon reimplements the go-plugin **client (host)** side so that
existing **Go-built** plugin binaries load unchanged. These three services are
the parts of that protocol the host must speak:

| Proto | Role in the handshake |
|---|---|
| `grpc_broker.proto` | Bidirectional stream used to open extra connections between host and plugin. The host serves `penguin.sdk.v1.HostService` on a brokered connection announced with `service_id = 1`. |
| `grpc_controller.proto` | How the host asks the plugin to shut down cleanly before escalating to a kill. |
| `grpc_stdio.proto` | Streams the plugin's stdout/stderr back to the host so plugin output lands in the daemon's log. |

`grpc.health.v1` is **not** vendored — `tonic-health` already ships both the
generated client and server for it, and the host only needs the client to poll
the plugin's health service to `SERVING`.

## Rules

- **Do not edit these files.** They define someone else's wire format; editing
  them breaks compatibility with every existing Go plugin.
- They are pinned to v1.7.0. Re-vendoring means re-verifying the whole M3
  compat gate against the new version.
- `go_package` is left in place even though it is meaningless to Rust codegen —
  keeping the files byte-identical to upstream is the point.
- The generated Rust lands in `penguin_proto::goplugin` (proto package `plugin`).
