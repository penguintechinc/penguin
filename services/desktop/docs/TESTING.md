# Testing Guide

## Running Tests

```bash
# All tests
make test

# Verbose output
go test ./... -v

# Specific package
go test ./pkg/clischema/... -v
go test ./internal/module/pluginhost/... -v

# With race detection
go test ./... -race

# With coverage
go test ./... -coverprofile=coverage.out
go tool cover -html=coverage.out
```

## Test Categories

### Unit Tests (41 tests)

| Package | Tests | What's Covered |
|---------|-------|----------------|
| `pkg/clischema` | 14 | CLI builder helpers, Cobra conversion, flags, subcommands, exit codes |
| `pkg/modulepb` | 1 | HealthState string conversion |
| `pkg/uischema` | 15 | All widget builders, nested tree construction |
| `internal/module/pluginhost` | 12 | Discovery (empty/nonexistent/found/dedup/non-exec), manager lifecycle |

### What Each Test Category Covers

**clischema tests** — Verify that `Command()`, `Flag()`, `RequiredFlag()`, `CommandList()` produce correct proto structures, and that `ToCobra()` correctly converts them to Cobra commands with working argument parsing, flag handling, and exit code propagation.

**uischema tests** — Verify all widget builder helpers (`Label`, `Button`, `Entry`, `Select`, `Card`, `VBox`, `HBox`, `Separator`, `Scroll`, `Checkbox`, `RichText`, `Panel`) produce correct Widget proto trees, including nested composition.

**pluginhost tests** — Verify plugin discovery logic (scanning directories, matching `penguin-mod-*` prefix, skipping non-executables, deduplicating across search paths) and manager operations (create, get, stop, stop-all on empty state).

### Integration Tests (Future)

These require actual plugin binaries and will test:
- Plugin launch and RPC communication
- Supervisor crash recovery and backoff
- GUI event round-trips
- CLI command execution through RPC

### Smoke Tests (Future)

Quick verification that:
- All 8 binaries compile (`make build`)
- Host starts and discovers plugins
- CLI `--help` works for all registered commands
- GUI launches without crashes (headless mode)

## Writing Tests

### Test File Naming

Tests live alongside the code they test:
```
pkg/clischema/builder.go       → pkg/clischema/builder_test.go
pkg/clischema/to_cobra.go      → pkg/clischema/to_cobra_test.go
internal/module/pluginhost/discovery.go → internal/module/pluginhost/discovery_test.go
```

### Test Patterns Used

**Table-driven tests** — Used for type conversions and builders:
```go
tests := []struct {
    input string
    want  string
}{
    {"penguin-mod-vpn", "vpn"},
    {"penguin-mod-dns", "dns"},
}
```

**Temp directories** — Used for discovery tests to avoid filesystem side effects:
```go
dir := t.TempDir()
os.WriteFile(filepath.Join(dir, "penguin-mod-vpn"), []byte("#!/bin/sh\n"), 0755)
```

**Discarded loggers** — Suppress log output in tests:
```go
logger := logrus.New()
logger.SetOutput(io.Discard)
```

## Linting

```bash
make lint    # Runs go vet + staticcheck

# Individual linters
go vet ./...
staticcheck ./...
```

## Pre-Test Checklist

Before running tests:
1. `make deps` — ensure dependencies are downloaded
2. `go vet ./...` — catch obvious issues
3. `make test` — run full suite
