# Penguin Desktop Tauri Build Environment
# Full GUI app build with webkit2gtk and all required dev dependencies

FROM rust:1.97-slim-bookworm@sha256:96c0af8cf054fd006435089f0076729716784ec9be485bd655de59c55df105ce

WORKDIR /app

# Install build dependencies for Tauri GUI app
# - webkit2gtk-4.1 and related packages for GUI rendering
# - pkg-config, cmake for build configuration
# - protobuf-compiler: for proto code generation
# - xvfb for headless testing (optional)
# - ca-certificates: TLS roots for `curl`, needed by scripts/fetch-webui.sh
#   (invoked automatically via tauri.conf.json's beforeBuildCommand) to fetch
#   the published waddlebot webui dist over HTTPS
# Unpinned apt package versions: this is a local/CI build-tool image (compiles
# the Tauri app; never deployed as a running service — see README.md
# "Docker (full build environment)"), not a customer-distributed artifact.
# The base image is SHA256-pinned above; supply-chain integrity comes from
# Debian's signed apt repos, not per-package version pins. Pinning webkit2gtk/
# gtk/dbus dev-package versions here would make the image break every time
# Debian bookworm rotates a point release, for no security benefit on a
# throwaway build container.
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake \
    curl \
    git \
    libdbus-1-dev \
    libprotobuf-dev \
    libssl-dev \
    libgtk-3-dev \
    libwebkit2gtk-4.1-dev \
    libappindicator3-dev \
    librsvg2-dev \
    patchelf \
    pkg-config \
    protobuf-compiler \
    xvfb \
    && rm -rf /var/lib/apt/lists/*

# Note: Node.js for frontend build is optional; Tauri Rust layer can build without it.
# The frontend (index.html + package.json) will be built separately if needed.
# scripts/fetch-webui.sh runs automatically as part of `cargo tauri build` /
# `npm run build` (Tauri's beforeBuildCommand) and needs the `curl` + `tar`
# already present in this image; it prefers `gh` if present but falls back to
# a plain HTTPS `curl` against public GitHub Release assets, which needs no
# authentication for this public repo.

# Non-root user for the build environment (devops-containers.md: rootless
# containers are mandatory by default). rust:*-slim-bookworm installs
# CARGO_HOME/RUSTUP_HOME world-readable+executable
# under /usr/local, so any UID can run rustc/cargo without extra chown there.
# /app is chowned so a plain `docker run` (no bind mount) is writable too; a
# host bind mount (see README.md) is owned by the host UID instead, which is
# why the README's docker run example passes --user $(id -u):$(id -g).
RUN groupadd -r builder && useradd -r -m -g builder -d /home/builder -s /bin/bash builder \
    && chown -R builder:builder /app
USER builder

# Verify Rust/cargo installation
RUN rustc --version && cargo --version

# Default: show build environment status
CMD ["bash", "-c", "echo 'Penguin Desktop Tauri build environment ready'; rustc --version; cargo --version"]
