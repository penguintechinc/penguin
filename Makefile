# Build front-end for the Rust workspace.
#
# Target names deliberately mirror the frozen Go client's Makefile so muscle
# memory and CI scripts carry over. Every target runs cargo directly, which is
# correct inside CI (the job already runs in the pinned Rust container).
#
# On a workstation, builds must NOT run on the host — prefix any target with
# `docker-` to run it inside the pinned image instead:
#
#     make docker-test      # == `make test`, inside penguin-rust:1.97
#
# The container needs protoc (tonic-prost-build shells out to it), which the
# stock rust image lacks; `make docker-image` builds the derived image.

CARGO ?= cargo
DOCKER_IMAGE ?= penguin-rust:1.97
RUST_VERSION ?= 1.97

# Unit-coverage floor, matching the Go client's gate.
COVER_MIN ?= 90

# Excluded from BOTH coverage tiers: generated code, binaries, examples, and the
# integration-test harnesses themselves.
COVER_EXCLUDE_ALWAYS := (crates/penguin-proto/|bins/|examples/|/tests/)

# Excluded from the UNIT tier only: zero-logic OS boundary adapters, isolated
# into their own files precisely so they can be excluded honestly. The
# integration tier puts them back.
#
# The go-plugin host's process/socket orchestration is excluded for the same
# reason the Go build excluded internal/extplugin/client.go from its unit gate:
# spawning a child process and completing a TLS handshake over a unix socket
# cannot be unit-tested, and pretending otherwise with mocks would test the mock.
# These files are covered for real by the goplugin_compat integration tests,
# which drive an actual Go-built plugin binary.
# penguin-sdk/src/plugin/* is the go-plugin SERVER side — process stdout
# handshake emission, TLS listener, gRPC serving, broker dial. Same argument as
# the host side above: it is exercised for real by the hostservice_roundtrip and
# reverse-compat integration tests, which run an actual plugin binary against an
# actual host. Its pure parts (mtls, handshake) stay in the unit tier.
COVER_EXCLUDE_BOUNDARY := (penguin-ipc/src/(listen|dial)_(unix|windows)\.rs|penguin-ipc/src/groups_unix\.rs|penguin-goplugin-host/src/(client|broker|stdio|controller)\.rs|penguin-sdk/src/plugin/(serve|broker|hostservices|services|tls_incoming)\.rs)

COVER_IGNORE_UNIT := $(COVER_EXCLUDE_ALWAYS)|$(COVER_EXCLUDE_BOUNDARY)
COVER_IGNORE_INT := $(COVER_EXCLUDE_ALWAYS)

.PHONY: help build test test-unit test-integration test-integration-cover \
        lint format test-security smoke-test clean proto go-client-check \
        pre-commit docker-image docker-volumes tools

help: ## Show available targets
	@grep -hE '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-24s\033[0m %s\n", $$1, $$2}'

build: ## Build every crate and binary
	$(CARGO) build --workspace --locked

test: test-unit ## Run the enforced test gate (unit + coverage floor)

tools: ## Install the cargo subcommands the gates need (cached in CARGO_HOME)
	@command -v cargo-llvm-cov >/dev/null 2>&1 || $(CARGO) install cargo-llvm-cov --locked
	@command -v cargo-deny >/dev/null 2>&1 || $(CARGO) install cargo-deny --locked
	@command -v cargo-audit >/dev/null 2>&1 || $(CARGO) install cargo-audit --locked

test-unit: tools ## Unit tests with the $(COVER_MIN)% line-coverage gate
	$(CARGO) llvm-cov --workspace --locked \
		--ignore-filename-regex '$(COVER_IGNORE_UNIT)' \
		--fail-under-lines $(COVER_MIN)

test-integration: ## Integration tests (marked #[ignore], need PENGUIN_INTEGRATION=1)
	PENGUIN_INTEGRATION=1 $(CARGO) test --workspace --locked -- --ignored

test-integration-cover: tools ## Combined unit+integration coverage (informational only)
	PENGUIN_INTEGRATION=1 $(CARGO) llvm-cov --workspace --locked --include-ignored \
		--ignore-filename-regex '$(COVER_IGNORE_INT)' \
		--summary-only

lint: ## Formatting + clippy (warnings are errors)
	$(CARGO) fmt --all --check
	$(CARGO) clippy --workspace --all-targets --locked -- -D warnings

format: ## Apply rustfmt
	$(CARGO) fmt --all

test-security: tools ## Supply-chain and advisory scans
	$(CARGO) deny check
	$(CARGO) audit

smoke-test: build ## Build, then check each binary answers --version
	@for bin in penguind penguin penguin-tray; do \
		if [ -x target/debug/$$bin ]; then \
			echo "smoke: $$bin"; target/debug/$$bin --version >/dev/null || exit 1; \
		else \
			echo "smoke: $$bin not built yet, skipping"; \
		fi; \
	done

proto: ## Regenerate protobuf bindings (build.rs does this; forces a rebuild)
	$(CARGO) clean -p penguin-proto
	$(CARGO) build -p penguin-proto --locked

go-client-check: ## Build + test the frozen Go reference client
	$(MAKE) -C go-client build test

pre-commit: lint test test-security ## Everything that must pass before a commit

clean: ## Remove build artifacts
	$(CARGO) clean

# libprotobuf-dev is required alongside protobuf-compiler: it supplies the
# well-known types (google/protobuf/empty.proto) that the vendored go-plugin
# grpc_stdio.proto imports. Without it the build fails on that import.
docker-image: ## Build the pinned Rust image (adds protoc, which rust:$(RUST_VERSION) lacks)
	@printf 'FROM rust:$(RUST_VERSION)-bookworm\nRUN apt-get update \
&& apt-get install -y --no-install-recommends protobuf-compiler libprotobuf-dev \
&& rm -rf /var/lib/apt/lists/*\n' | docker build -t $(DOCKER_IMAGE) -

# `make docker-<target>` runs <target> inside the pinned image. Cargo's home and
# target dirs live in named volumes so they persist across runs and never land
# in the repo.
# Docker auto-creates a named volume's root as root:root, which then blocks the
# --user container from writing into it. Creating and chowning them up front is
# idempotent and cheap, and turns a confusing mid-build permission error into a
# non-event.
docker-volumes:
	@docker volume create penguin_cargo_home >/dev/null
	@docker volume create penguin_target_make >/dev/null
	@docker run --rm -v penguin_cargo_home:/cargo -v penguin_target_make:/target \
		busybox chown -R $(shell id -u):$(shell id -g) /cargo /target

# --user is not optional: without it every file the container writes (Cargo.lock,
# target/, built binaries) lands root-owned in the working tree, which then needs
# root to clean up.
docker-%: docker-image docker-volumes
	docker run --rm \
		--user $(shell id -u):$(shell id -g) \
		-v $(CURDIR):/work -w /work \
		-v penguin_cargo_home:/cargo -e CARGO_HOME=/cargo \
		-v penguin_target_make:/target -e CARGO_TARGET_DIR=/target \
		-e PATH=/cargo/bin:/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin \
		$(DOCKER_IMAGE) make $*
