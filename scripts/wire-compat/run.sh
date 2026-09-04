#!/usr/bin/env bash
#
# Wire-compat gate #1: the FROZEN Go `penguin` CLI must drive the Rust `penguind`.
#
# This is the whole point of keeping go-client/ around. The Go CLI speaks the
# daemon.v1 contract; if the Rust daemon has drifted on the wire — field
# numbers, api_version handling, status codes, streaming shape — the Go CLI is
# the oracle that catches it.
#
# Usage:
#   PENGUIND=target/debug/penguind PENGUIN_GO=go-client/bin/penguin scripts/wire-compat/run.sh
#
# Bash 3.2 compatible (macOS ships 3.2): no associative arrays, no mapfile.

set -euo pipefail

PENGUIND="${PENGUIND:-target/debug/penguind}"
PENGUIN_GO="${PENGUIN_GO:-go-client/bin/penguin}"

fail() {
    echo "wire-compat: FAIL: $*" >&2
    exit 1
}

[ -x "$PENGUIND" ] || fail "rust daemon not executable at $PENGUIND"
[ -x "$PENGUIN_GO" ] || fail "go CLI not executable at $PENGUIN_GO"

# The unix socket path limit is 103 bytes, so keep this short and out of any
# deep temp path — a nested mktemp dir can blow the limit on its own.
#
# The socket's parent directory must be one this process actually owns: the
# daemon unconditionally chmods it to 0750 (see penguin-ipc's listen_unix.rs
# module doc — that's deliberate hardening, not a bug, for a directory the
# daemon manages itself). Handing the daemon a bare /tmp as that parent means
# asking it to chmod /tmp, which it does not own and cannot chmod without
# root — an EPERM before the socket is ever created. So this creates its own
# short-named subdirectory under /tmp, which it does own, rather than
# pointing the daemon at /tmp directly.
SOCKDIR="/tmp/pgwc-$$"
mkdir -p "$SOCKDIR"
SOCKET="$SOCKDIR/d.sock"
WORKDIR="$(mktemp -d)"
CONFIG_DIR="$WORKDIR/etc"
STATE_DIR="$WORKDIR/state"
mkdir -p "$CONFIG_DIR" "$STATE_DIR"

DAEMON_PID=""
cleanup() {
    if [ -n "$DAEMON_PID" ] && kill -0 "$DAEMON_PID" 2>/dev/null; then
        kill "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
    fi
    rm -rf "$WORKDIR"
    rm -rf "$SOCKDIR"
}
trap cleanup EXIT

echo "wire-compat: starting rust penguind on $SOCKET"
"$PENGUIND" --config-dir "$CONFIG_DIR" --state-dir "$STATE_DIR" --socket "$SOCKET" \
    >"$WORKDIR/daemon.log" 2>&1 &
DAEMON_PID=$!

# Wait for the socket to appear rather than sleeping a fixed amount.
waited=0
while [ ! -S "$SOCKET" ]; do
    if ! kill -0 "$DAEMON_PID" 2>/dev/null; then
        echo "--- daemon log ---" >&2
        cat "$WORKDIR/daemon.log" >&2
        fail "daemon exited before creating the socket"
    fi
    sleep 0.1
    waited=$((waited + 1))
    if [ "$waited" -gt 100 ]; then
        echo "--- daemon log ---" >&2
        cat "$WORKDIR/daemon.log" >&2
        fail "socket did not appear within 10s"
    fi
done
echo "wire-compat: daemon up (pid $DAEMON_PID)"

# --- assertions -----------------------------------------------------------
# Each check runs the FROZEN Go CLI against the Rust daemon.

# The Go CLI swallows RPC errors on several verbs: it checks the error, discards
# it, and still exits 0 having printed nothing useful. Asserting on the exit
# status alone therefore produces FALSE PASSES — an earlier version of this
# harness "passed" while every RPC was actually being rejected at the HTTP/2
# layer. So every success check also requires non-empty output that carries no
# transport-error marker.
transport_error_markers='rpc error|PROTOCOL_ERROR|RST_STREAM|Unimplemented|Unavailable|connection refused|transport:'

assert_clean_output() {
    description="$1"
    output="$2"
    if [ -z "$output" ]; then
        fail "$description: command exited 0 but produced no output (the CLI swallows RPC errors — this is not a pass)"
    fi
    if printf '%s' "$output" | grep -qE "$transport_error_markers"; then
        echo "--- output ---" >&2
        echo "$output" >&2
        fail "$description: output carries a transport error despite exit 0"
    fi
}

check_ok() {
    description="$1"
    shift
    if ! output="$("$PENGUIN_GO" --socket "$SOCKET" "$@" 2>&1)"; then
        echo "--- output ---" >&2
        echo "$output" >&2
        fail "$description (command: $*)"
    fi
    assert_clean_output "$description" "$output"
    echo "wire-compat: ok — $description"
}

# Strongest form: exit 0 AND the output actually contains the expected content.
# Use this wherever the expected text is known, so a swallowed error cannot pass.
check_contains() {
    description="$1"
    expected="$2"
    shift 2
    if ! output="$("$PENGUIN_GO" --socket "$SOCKET" "$@" 2>&1)"; then
        echo "--- output ---" >&2
        echo "$output" >&2
        fail "$description (command: $*)"
    fi
    assert_clean_output "$description" "$output"
    case "$output" in
        *"$expected"*) echo "wire-compat: ok — $description" ;;
        *)
            echo "--- output ---" >&2
            echo "$output" >&2
            fail "$description: expected output containing '$expected'"
            ;;
    esac
}

check_fails_with() {
    description="$1"
    expected="$2"
    shift 2
    if output="$("$PENGUIN_GO" --socket "$SOCKET" "$@" 2>&1)"; then
        echo "--- output ---" >&2
        echo "$output" >&2
        fail "$description: expected failure but command succeeded"
    fi
    case "$output" in
        *"$expected"*) echo "wire-compat: ok — $description" ;;
        *)
            echo "--- output ---" >&2
            echo "$output" >&2
            fail "$description: expected message containing '$expected'"
            ;;
    esac
}

# Cross-check the version the CLI reports over the wire against what the daemon
# binary reports locally. This is the strongest available assertion that the
# Version RPC genuinely round-tripped rather than being swallowed.
DAEMON_VERSION="$("$PENGUIND" version 2>/dev/null | tr -d '\r\n' || true)"
if [ -z "$DAEMON_VERSION" ]; then
    fail "could not determine the daemon version via '$PENGUIND version'"
fi
echo "wire-compat: daemon reports version $DAEMON_VERSION"

check_contains "Version RPC returns the daemon's real version" "$DAEMON_VERSION" version
check_ok "ListModules RPC" modules
check_ok "GetStatus RPC (all modules)" status
check_fails_with "LoadModule rejects an unknown module" "not found" load definitely-not-a-real-module

# TailLogs is a real implementation on the Rust daemon (Go returns
# UNIMPLEMENTED here — see penguin-daemon/src/service.rs's module doc). Pass
# --lines explicitly and rely on the CLI's default --follow=false so the
# call is a single bounded backlog replay, never a following stream — the
# harness must never be able to hang on this.
check_ok "TailLogs RPC (bounded, non-following)" logs --lines 5

# UnloadModule is idempotent: a name that was never loaded — known or not —
# is a no-op success, not an error (see Supervisor::unload /
# DaemonService::unload_module's doc comment).
check_ok "UnloadModule of a never-loaded module is idempotent success" unload never-loaded-module

check_fails_with "GetStatus rejects an unknown module" "not found" status definitely-not-a-real-module-either

echo "wire-compat: PASS — the frozen Go CLI drives the Rust daemon"
