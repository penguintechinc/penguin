# Development Guide

## Prerequisites

- **Go 1.24+** (check with `go version`)
- **Fyne dependencies** (Linux): `sudo apt install libgl1-mesa-dev xorg-dev`
- **Fyne dependencies** (macOS): Xcode command line tools
- **staticcheck** (optional, for linting): `go install honnef.co/go/tools/cmd/staticcheck@latest`

## Setup

```bash
cd services/desktop
make deps    # Download Go modules
make build   # Build all binaries
```

## Project Layout

The codebase follows Go standard layout conventions:

- `cmd/` — Entry points (main packages)
- `internal/` — Private application code
- `pkg/` — Public library code usable by external packages
- `api/` — Protocol definitions

### Host Binaries

| Binary | Path | Purpose |
|--------|------|---------|
| `penguin-desktop` | `cmd/penguin-desktop/` | GUI client (Fyne) |
| `penguin-cli` | `cmd/penguin-cli/` | CLI client (Cobra) |

### Module Plugins

Each module lives in `internal/modules/<name>/` with its entry point in `cmd/modules/penguin-mod-<name>/main.go`. The entry point is minimal — it wraps the module with the shared `ModuleAdapter` and calls `plugin.Serve()`.

| Module | Package | Description |
|--------|---------|-------------|
| VPN | `internal/modules/vpn` | WireGuard tunnel management |
| OpenZiti | `internal/modules/openziti` | Zero-trust network overlay |
| DNS | `internal/modules/dns` | DNS-over-HTTPS resolver |
| NTP | `internal/modules/ntp` | Time synchronization |
| Nest | `internal/modules/nest` | Device management |
| ArticDBM | `internal/modules/articdbm` | Database management |

## Development Workflow

### Building

```bash
# Full build
make build

# Build only the host (faster iteration)
make build-host

# Build a single module
make build-module MOD=vpn

# Clean and rebuild
make clean && make build
```

### Running

```bash
# Desktop GUI
./bin/penguin-desktop

# CLI
./bin/penguin-cli status
./bin/penguin-cli vpn connect --server us-west-1
```

The host automatically discovers `penguin-mod-*` binaries in the `plugins/` directory at startup.

### Adding a New Module

1. Create `internal/modules/<name>/module.go` implementing `module.PluginModule`
2. Create `cmd/modules/penguin-mod-<name>/main.go` with the 3-line adapter pattern
3. Add to the `MODULES` list in `Makefile`
4. Build: `make build-module MOD=<name>`

### Configuration

Default config at `~/.config/penguin/penguin.yaml` (Linux) or platform equivalent:

```yaml
log_level: info
modules:
  vpn:
    enabled: true
  dns:
    enabled: true
plugins:
  dir: plugins
```

## Key Patterns

### Declarative UI (pkg/uischema)

Modules describe their GUI panels using builder helpers instead of Fyne widgets directly:

```go
func (m *Module) GetGUIPanel(ctx context.Context) (*modulepb.GUIPanel, error) {
    panel := uischema.Panel("My Module",
        uischema.Label("Status: Running"),
        uischema.Button("action-btn", "Do Something"),
    )
    return panel, nil
}
```

The host renders these widget trees as Fyne objects and routes events back via `HandleGUIEvent`.

### CLI Schema (pkg/clischema)

Modules declare CLI commands as trees:

```go
func (m *Module) GetCLICommands(ctx context.Context) (*modulepb.CLICommandList, error) {
    return clischema.CommandList(
        *clischema.Command("status", "Show status"),
        *clischema.Command("connect", "Connect to service"),
    ), nil
}
```

The host converts these to Cobra commands at runtime.

### Module Adapter (pkg/plugin)

The shared adapter eliminates per-module RPC boilerplate:

```go
// cmd/modules/penguin-mod-vpn/main.go
func main() {
    pluginpkg.Serve(&pluginpkg.ModuleAdapter{Mod: vpn.New()})
}
```

## Troubleshooting

**"Fyne requires a display"** — Set `FYNE_RENDER=software` or build with `-tags nogui` for headless/CLI-only mode.

**Module not discovered** — Ensure the binary is named `penguin-mod-<name>`, is executable (`chmod +x`), and is in the `plugins/` directory or a configured search path.

**Plugin crashes on startup** — Check that the magic cookie matches (`PENGUIN_MODULE_PLUGIN` / `penguin-module-v1`). Run the plugin binary directly to see its stderr output.
