# Penguin Desktop Tauri Build Environment
# Full GUI app build with webkit2gtk and all required dev dependencies

FROM debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818

WORKDIR /app

# Install build dependencies for Tauri GUI app
# - webkit2gtk-4.1 and related packages for GUI rendering
# - rust toolchain build deps
# - pkg-config, cmake for build configuration
# - xvfb for headless testing (optional)
# - ca-certificates: TLS roots for `curl`, needed by scripts/fetch-webui.sh
#   (invoked automatically via tauri.conf.json's beforeBuildCommand) to fetch
#   the published waddlebot webui dist over HTTPS
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    ca-certificates \
    cargo \
    cmake \
    curl \
    git \
    libssl-dev \
    libgtk-3-dev \
    libwebkit2gtk-4.1-dev \
    libappindicator3-dev \
    librsvg2-dev \
    patchelf \
    pkg-config \
    rustc \
    xvfb \
    && rm -rf /var/lib/apt/lists/*

# Note: Node.js for frontend build is optional; Tauri Rust layer can build without it.
# The frontend (index.html + package.json) will be built separately if needed.
# scripts/fetch-webui.sh runs automatically as part of `cargo tauri build` /
# `npm run build` (Tauri's beforeBuildCommand) and needs the `curl` + `tar`
# already present in this image; it prefers `gh` if present but falls back to
# a plain HTTPS `curl` against public GitHub Release assets, which needs no
# authentication for this public repo.

# Verify Rust/cargo installation
RUN rustc --version && cargo --version

# Default: show build environment status
CMD ["bash", "-c", "echo 'Penguin Desktop Tauri build environment ready'; rustc --version; cargo --version"]
