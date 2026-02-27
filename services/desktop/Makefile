# Penguin Desktop Client - Build System
# Supports building the host binaries and individual module plugin binaries.

.PHONY: help build build-host build-modules build-module clean test lint

.DEFAULT_GOAL := help

VERSION := $(shell cat ../../.version 2>/dev/null || echo "0.1.0")
BUILD_DIR := bin
PLUGINS_DIR := plugins
MODULES := vpn openziti dns ntp nest articdbm
LDFLAGS := -ldflags "-X main.version=$(VERSION)"

help: ## Show available targets
	@echo "Penguin Desktop Client Build System"
	@echo ""
	@echo "Targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  %-20s %s\n", $$1, $$2}'

build: build-host build-modules ## Build everything (host + all modules)

build-host: ## Build host binaries (penguin-desktop, penguin-cli)
	@echo "Building host binaries..."
	@mkdir -p $(BUILD_DIR)
	go build $(LDFLAGS) -o $(BUILD_DIR)/penguin-desktop ./cmd/penguin-desktop/
	go build $(LDFLAGS) -o $(BUILD_DIR)/penguin-cli ./cmd/penguin-cli/
	@echo "Host binaries built: $(BUILD_DIR)/"

build-modules: ## Build all module plugin binaries
	@echo "Building module plugins..."
	@mkdir -p $(PLUGINS_DIR)
	@for mod in $(MODULES); do \
		echo "  Building penguin-mod-$$mod..."; \
		go build -o $(PLUGINS_DIR)/penguin-mod-$$mod ./cmd/modules/penguin-mod-$$mod/; \
	done
	@echo "Module plugins built: $(PLUGINS_DIR)/"

build-module: ## Build a single module (usage: make build-module MOD=vpn)
	@if [ -z "$(MOD)" ]; then echo "Usage: make build-module MOD=<name>"; exit 1; fi
	@mkdir -p $(PLUGINS_DIR)
	go build -o $(PLUGINS_DIR)/penguin-mod-$(MOD) ./cmd/modules/penguin-mod-$(MOD)/
	@echo "Built: $(PLUGINS_DIR)/penguin-mod-$(MOD)"

clean: ## Clean build artifacts
	rm -rf $(BUILD_DIR) $(PLUGINS_DIR)/penguin-mod-*

test: ## Run all tests
	go test ./...

test-pkg: ## Run tests for public packages
	go test ./pkg/...

test-internal: ## Run tests for internal packages
	go test ./internal/...

lint: ## Run linting
	@if command -v golangci-lint > /dev/null; then \
		golangci-lint run ./...; \
	else \
		echo "golangci-lint not installed, running go vet"; \
		go vet ./...; \
	fi

deps: ## Download and tidy dependencies
	go mod download
	go mod tidy
