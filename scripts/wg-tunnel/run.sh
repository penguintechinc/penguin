#!/usr/bin/env bash
#
# M6 gate: prove the real KernelBackend brings up an actual kernel WireGuard
# tunnel against a controlled peer in a network namespace, then tears it
# down cleanly. No unit test can make this assertion — it needs a real
# privileged network namespace, which is exactly what this script provides
# and a plain `cargo test` on a workstation cannot.
#
# Runs entirely in a privileged Docker container, on THIS machine. Never on
# the bare host, never with sudo/polkit — see crates/penguin-module-
# tobogganing/tests/wg_tunnel.rs's module doc for what the test itself does
# and why it self-skips instead of failing when it cannot run for real.
#
# Usage:
#   scripts/wg-tunnel/run.sh
#
# Bash 3.2 compatible (macOS ships 3.2): no associative arrays, no mapfile.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BASE_IMAGE="${WG_TEST_BASE_IMAGE:-penguin-rust:1.97}"
IMAGE="${WG_TEST_IMAGE:-penguin-wgtest:1.97}"
TARGET_VOLUME="penguin_target_wg"

fail() {
    echo "wg-tunnel: FAIL: $*" >&2
    exit 1
}

command -v docker >/dev/null 2>&1 || fail "docker not found on PATH"

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
    echo "wg-tunnel: building $IMAGE from $BASE_IMAGE"
    # iproute2 (netns/veth/link plumbing), wireguard-tools (the `wg` CLI the
    # test uses to stand up the CONTROLLED PEER — never the code under
    # test, which goes through KernelBackend directly), iputils-ping (the
    # traffic that triggers a real handshake), openresolv (provides the
    # `resolvconf` binary: defguard_wireguard_rs's remove_interface() shells
    # out to it unconditionally on teardown, even when DNS was never
    # configured — without it, KernelBackend::teardown fails on ANY
    # interface with ENOENT, discovered by this exact gate).
    printf 'FROM %s\nRUN apt-get update \\\n    && apt-get install -y --no-install-recommends iproute2 wireguard-tools iputils-ping openresolv \\\n    && rm -rf /var/lib/apt/lists/*\n' "$BASE_IMAGE" \
        | docker build -t "$IMAGE" -
fi

# Dedicated, isolated target volume — deliberately NOT the shared
# penguin_target_make/penguin_target_final volumes other make targets use.
# This container runs privileged/root (required for netns + real kernel
# WireGuard interfaces), so anything it writes here is root-owned; keeping
# it in its own volume means that never leaks into a volume a non-root
# `docker-%` build later needs to write into.
docker volume create "$TARGET_VOLUME" >/dev/null

echo "wg-tunnel: running the real kernel WireGuard tunnel gate (privileged)"
set +e
# /usr/sbin:/sbin are required, not cosmetic: `resolvconf` (from openresolv,
# used internally by defguard_wireguard_rs's teardown path — see the image
# build step above) installs ONLY to /usr/sbin/resolvconf, with no /usr/bin
# copy or symlink. `ip`/`wg` happen to resolve under /usr/bin either way, so
# their absence here would fail silently only for teardown, not setup —
# found by running this gate without them and watching teardown ENOENT.
docker run --rm --privileged \
    -v "$REPO_ROOT:/work" -w /work \
    -v penguin_cargo_home:/cargo -e CARGO_HOME=/cargo \
    -v "$TARGET_VOLUME:/target" -e CARGO_TARGET_DIR=/target \
    -e PATH=/cargo/bin:/usr/local/cargo/bin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    -e PENGUIN_INTEGRATION=1 \
    "$IMAGE" \
    cargo test -p penguin-module-tobogganing --test wg_tunnel \
        --features integration-test --locked -- --ignored --nocapture
status=$?
set -e

if [ "$status" -ne 0 ]; then
    fail "wg_tunnel gate failed (exit $status) — see output above"
fi

echo "wg-tunnel: gate script finished — check the output above for SKIP vs. real assertions A-D"
echo "wg-tunnel: a SKIP means the container lacked privilege/module/tools, not that the tunnel worked"
