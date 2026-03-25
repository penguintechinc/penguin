# App Standards — Penguin Desktop Client

## Architecture

### Plugin Architecture (HashiCorp go-plugin)

The desktop client uses a host-plugin architecture where each module runs as an independent binary:

- **Transport**: net/rpc over stdin/stdout (not gRPC, to avoid protoc dependency)
- **Discovery**: Host scans `plugins/` directory for `penguin-mod-*` executables
- **Handshake**: Magic cookie `PENGUIN_MODULE_PLUGIN` / `penguin-module-v1`
- **Crash recovery**: Supervisor with progressive backoff (1s → 5s → 15s, max 3 restarts)

### Interface Hierarchy

```
ModuleBase (lifecycle: Init/Start/Stop/HealthCheck)
    ├── PluginModule (declarative: GetGUIPanel/HandleGUIEvent/GetCLICommands/ExecuteCLICommand)
    └── LegacyModule (direct: GUIPanel/Icon/CLICommands — deprecated, for migration)
```

All modules currently implement `PluginModule`. `LegacyModule` exists for backward compatibility during migration and will be removed.

### Declarative UI Pattern

Modules cannot create Fyne widgets directly (they run in separate processes). Instead:

1. Module builds a `Widget` tree using `pkg/uischema` builders
2. Host fetches the tree via `GetGUIPanel` RPC
3. Host renders the tree as Fyne `CanvasObject` via `uischema.Render()`
4. User interactions generate `GUIEvent` messages routed back via `HandleGUIEvent` RPC
5. Module returns an updated `Widget` tree for re-rendering

### CLI Pattern

Same declarative approach for CLI commands:

1. Module declares `CLICommand` trees using `pkg/clischema` builders
2. Host converts to Cobra commands via `clischema.ToCobra()`
3. Command execution calls `ExecuteCLICommand` RPC with args/flags
4. Module returns stdout/stderr/exit_code

## Conventions

### Package Organization

- `pkg/` — Public packages safe for external use or plugin binaries
- `internal/` — Private to the desktop service
- `cmd/` — Binary entry points only (minimal code)

### Module Entry Points

All module binaries follow the same 3-line pattern using `ModuleAdapter`:

```go
func main() {
    pluginpkg.Serve(&pluginpkg.ModuleAdapter{Mod: mymodule.New()})
}
```

### Build Tags

- `gui` (default) — includes Fyne GUI code
- `nogui` — excludes Fyne, CLI-only mode

### Error Handling

- Modules should return errors from RPC methods, not panic
- The supervisor handles process crashes; modules don't need their own recovery
- CLI commands use `ExitCode` field in response, not Go errors

### Logging

- Host uses logrus with structured fields
- Plugin processes use a separate stderr JSON logger (via `pkg/plugin/plugin_logger.go`)
- The `hclogAdapter` bridges logrus → hclog for go-plugin compatibility

## Dependencies

| Dependency | Version | Purpose |
|-----------|---------|---------|
| hashicorp/go-plugin | v1.6.2 | Plugin process management |
| hashicorp/go-hclog | v1.6.3 | Logging interface for go-plugin |
| spf13/cobra | v1.8.1 | CLI framework |
| sirupsen/logrus | v1.9.3 | Structured logging |
| fyne.io/fyne/v2 | v2.5.4 | GUI framework (host only) |

## Platform Support

Platform-specific code lives in `internal/platform/`:
- `linux.go` — XDG paths, systemd integration
- `darwin.go` — macOS paths, launchd integration
- `windows.go` — AppData paths, service integration

Build constraints ensure only the relevant file compiles per platform.
