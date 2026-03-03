# Penguin Desktop Client

```
    .__                          .__
    |__| ____    ____  __ ___  __|__| ____
    |  |/    \  / ___\|  |  \/  |  |/    \
    |  |   |  \/ /_/  >  |  /\  |  |   |  \
    |__|___|  /\___  / \____/  \_|__|___|  /
     desktop\//_____/   client            \/
```

A cross-platform desktop client for the Penguin ecosystem, built with a plugin architecture that enables independent module updates without rebuilding the entire application.

**Company**: [Penguin Tech Inc](https://www.penguintech.io)

## Architecture

The client uses the [HashiCorp go-plugin](https://github.com/hashicorp/go-plugin) pattern where each module runs as a separate binary communicating with the host over net/rpc via stdin/stdout:

```
Host Binary (penguin-desktop / pcli)
    │
    ├── discovers plugins/ directory for penguin-mod-* binaries
    ├── launches each as child process via go-plugin
    └── communicates via net/rpc (stdin/stdout, magic cookie handshake)
         │
         ├── penguin-mod-vpn         (WireGuard VPN management)
         ├── penguin-mod-openziti    (OpenZiti zero-trust overlay)
         ├── penguin-mod-dns         (DNS-over-HTTPS resolver)
         ├── penguin-mod-ntp         (NTP time synchronization)
         ├── penguin-mod-nest        (Penguin Nest device management)
         └── penguin-mod-articdbm    (ArticDB database management)
```

**Key Design Decisions:**
- **Separate binaries** — each module can be built, tested, and released independently
- **Declarative UI** — modules describe GUI panels as widget trees; host renders via Fyne
- **CLI schema** — modules declare CLI commands as proto trees; host converts to Cobra
- **Crash recovery** — supervisor monitors plugins with progressive backoff restart

## Quick Start

```bash
# Install dependencies
make deps

# Build everything (host binaries + 6 module plugins)
make build

# Run the desktop client (GUI)
./bin/penguin-desktop

# Run the CLI
./bin/pcli --help
```

## Project Structure

```
services/desktop/
├── api/proto/          # Proto definitions (reference documentation)
├── cmd/
│   ├── pcli/           # CLI host binary
│   ├── penguin-desktop/# Desktop GUI host binary
│   └── modules/        # 6 plugin module binaries
├── internal/
│   ├── app/            # Application orchestrator
│   ├── config/         # Configuration management
│   ├── gui/            # Fyne GUI rendering
│   ├── module/         # Module interfaces and plugin host
│   └── modules/        # 6 module implementations
├── pkg/
│   ├── clischema/      # CLI command tree builder + Cobra conversion
│   ├── modulepb/       # Shared types (proto-equivalent)
│   ├── plugin/         # go-plugin integration (handshake, RPC, adapter)
│   └── uischema/       # Declarative UI builder + Fyne renderer
├── plugins/            # Runtime plugin binaries directory
└── docs/               # Documentation
```

See [docs/](docs/) for detailed documentation:
- [Development Guide](docs/DEVELOPMENT.md) — local setup and workflow
- [Testing Guide](docs/TESTING.md) — test categories and execution
- [Pre-Commit Checklist](docs/PRE_COMMIT.md) — required steps before committing
- [App Standards](docs/APP_STANDARDS.md) — architecture decisions and conventions

## Build

```bash
make build           # Build all (host + modules)
make build-host      # Build only host binaries
make build-modules   # Build only plugin modules
make build-module MOD=vpn  # Build a single module
make clean           # Remove build artifacts
```

Individual module builds:
```bash
go build -o plugins/penguin-mod-vpn ./cmd/modules/penguin-mod-vpn/
```

## Testing

```bash
make test            # Run all tests
make lint            # Run linters (go vet + staticcheck)
```

41 tests covering:
- `pkg/clischema` — CLI builder and Cobra conversion
- `pkg/modulepb` — shared type behavior
- `pkg/uischema` — widget builder helpers
- `internal/module/pluginhost` — discovery, manager, supervisor

## Platforms

| Platform | GUI | CLI | Status |
|----------|-----|-----|--------|
| Linux    | Fyne | Yes | Primary |
| macOS    | Fyne | Yes | Supported |
| Windows  | Fyne | Yes | Supported |

## License

Limited AGPL-3.0 — See [LICENSE.md](../../LICENSE.md)
