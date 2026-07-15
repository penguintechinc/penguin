# penguin — PenguinTech unified endpoint agent
# `make help` lists targets. CI builds run in golang:1.25-bookworm for parity.

MODULE      := github.com/penguintechinc/penguin
VERSION     := $(shell cat .version)
LDFLAGS     := -ldflags "-s -w -X $(MODULE)/internal/version.Version=$(VERSION)"
GO_IMAGE    := golang:1.25-bookworm
BINARIES    := penguin penguind penguin-tray
COVER_MIN   := 90

.DEFAULT_GOAL := help

.PHONY: help
help: ## Show this help
	@grep -hE '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-22s\033[0m %s\n", $$1, $$2}'

## ---- Build ----

.PHONY: build
build: ## Build all binaries into ./bin
	@mkdir -p bin
	@for b in $(BINARIES); do go build $(LDFLAGS) -o bin/$$b ./cmd/$$b || exit 1; done

.PHONY: build-docker
build-docker: ## Build all binaries inside golang:1.25-bookworm (CI parity)
	docker run --rm -v $(PWD):/src -w /src -e GOFLAGS=-buildvcs=false $(GO_IMAGE) make build

.PHONY: proto
proto: ## Regenerate gRPC code from api/proto and pkg/sdk/proto
	@command -v protoc >/dev/null || { echo "protoc not found"; exit 1; }
	protoc --go_out=. --go_opt=module=$(MODULE) \
	       --go-grpc_out=. --go-grpc_opt=module=$(MODULE) \
	       $(shell find api/proto pkg/sdk/proto -name '*.proto' 2>/dev/null)

## ---- Quality ----

.PHONY: lint
lint: ## golangci-lint (staticcheck, gosec, errcheck, unused)
	golangci-lint run ./...

.PHONY: format
format: ## gofmt + goimports
	gofmt -w .
	@command -v goimports >/dev/null && goimports -w . || true

.PHONY: test
test: test-unit ## All tests

# ---- Coverage exclusion sets (see docs/APP_STANDARDS.md "Coverage policy") ----
# ALWAYS excluded from every gate — generated code or zero-logic OS/framework
# adapters isolated into dedicated files (see docs/APP_STANDARDS.md):
#   *.pb.go                        generated gRPC/protobuf
#   cmd/                           thin main() wiring (exercised by smoke-test + E2E)
#   examples/                      reference plugin, not shipped product code
#   plugin_glue.go                 go-plugin framework boilerplate (needs plugin runtime)
#   vpn_wgctrl.go                  kernel WireGuard adapter (needs the wireguard kmod)
#   sysresolver_resolvectl_linux.go systemd-resolved/resolvectl exec adapter
COVER_EXCLUDE_ALWAYS := \.pb\.go:|/cmd/|/examples/|/plugin_glue\.go:|/vpn_wgctrl\.go:|/sysresolver_resolvectl_linux\.go:
# Excluded from the UNIT gate only — the subprocess/socket orchestration a
# unit-test process cannot reach (real SO_PEERCRED handshake, real plugin
# subprocess launch). The integration gate (-tags=integration, privileged CI)
# exercises these against real peers/subprocesses and counts them.
COVER_EXCLUDE_BOUNDARY := internal/ipc/(dial|listen)_unix|internal/extplugin/client\.go:

.PHONY: test-unit
test-unit: ## Unit tests: race + coverage gate on logic (boundary excluded — see integration gate)
	go test -race -coverprofile=coverage.out -covermode=atomic ./...
	@grep -vE '$(COVER_EXCLUDE_ALWAYS)|$(COVER_EXCLUDE_BOUNDARY)' coverage.out > coverage.unit.out
	@go tool cover -func=coverage.unit.out | awk '/^total:/ {gsub(/%/,"",$$3); if ($$3+0 < $(COVER_MIN)) {printf "unit coverage %.1f%% below $(COVER_MIN)%% threshold\n", $$3; exit 1} else printf "unit coverage %.1f%% (>= $(COVER_MIN)%%)\n", $$3}'

.PHONY: test-integration
test-integration: ## Integration tests (Linux netns / real subprocess; root-gated tests self-skip if unprivileged)
	go test -race -tags=integration ./...

.PHONY: test-integration-cover
test-integration-cover: ## Report combined unit+integration coverage (boundary INCLUDED; informational — the enforced gate is `make test`)
	go test -race -tags=integration -coverprofile=coverage.int.out -covermode=atomic ./...
	@grep -vE '$(COVER_EXCLUDE_ALWAYS)' coverage.int.out > coverage.intfiltered.out
	@go tool cover -func=coverage.intfiltered.out | awk '/^total:/ {printf "combined unit+integration coverage (boundary included): %s\n", $$3}'

.PHONY: smoke-test
smoke-test: build ## Build + version smoke check on all binaries
	@for b in $(BINARIES); do ./bin/$$b version | grep -q "$(VERSION)" || { echo "$$b version mismatch"; exit 1; }; done
	@echo "smoke-test OK"

.PHONY: test-security
test-security: ## Security scans (gosec, govulncheck, gitleaks)
	@command -v gosec >/dev/null && gosec -exclude-generated ./... || echo "gosec not installed — skipping (CI enforces)"
	@command -v govulncheck >/dev/null && govulncheck ./... || echo "govulncheck not installed — skipping (CI enforces)"
	@command -v gitleaks >/dev/null && gitleaks protect --no-banner || echo "gitleaks not installed — skipping (CI enforces)"

.PHONY: pre-commit
pre-commit: lint test-security build smoke-test test ## Full pre-commit gate

## ---- Housekeeping ----

.PHONY: clean
clean: ## Remove build artifacts
	rm -rf bin coverage.out coverage.unit.out coverage.int.out coverage.intfiltered.out

.PHONY: version-show
version-show: ## Print current version
	@echo $(VERSION)
