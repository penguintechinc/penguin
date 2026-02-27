# Pre-Commit Checklist

Run these steps before every commit. Total time: ~1 minute.

## Required Steps

### 1. Build Verification

```bash
make clean && make build
```

All 8 binaries must compile without errors:
- `bin/penguin-desktop`
- `bin/penguin-cli`
- `plugins/penguin-mod-{vpn,openziti,dns,ntp,nest,articdbm}`

### 2. Lint

```bash
make lint
```

Must pass with zero warnings:
- `go vet ./...`
- `staticcheck ./...` (if installed)

### 3. Tests

```bash
make test
```

All 41+ tests must pass. Run with `-race` for concurrency-sensitive changes:
```bash
go test ./... -race
```

### 4. Security Scan

```bash
# Check for known vulnerabilities in dependencies
go install golang.org/x/vuln/cmd/govulncheck@latest
govulncheck ./...

# Check for hardcoded secrets
grep -r "password\|secret\|api_key\|token" --include="*.go" . | grep -v "_test.go" | grep -v "// "
```

### 5. No Compiled Binaries

Ensure no compiled plugin binaries are staged:
```bash
git diff --cached --name-only | grep "plugins/penguin-mod-"
# Should return nothing
```

### 6. Review Changes

```bash
git diff --cached --stat
git diff --cached  # Full diff review
```

Check for:
- No hardcoded credentials or secrets
- No debug `fmt.Println` statements left behind
- No commented-out code blocks
- Test coverage for new functionality

## Quick One-Liner

```bash
make clean && make build && make lint && make test
```
