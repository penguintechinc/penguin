#!/usr/bin/env bash
#
# Shared helpers for the M8 parity harness (scripts/parity/*.sh).
#
# Sourced, never executed — it sets no `-e` of its own so a sub-gate can
# inspect the exit codes of commands that are *supposed* to fail (a load of an
# unknown module, a bad-api_version RPC). Each sub-gate sets its own shell
# options.
#
# What it factors out of the two green precursors (scripts/wire-compat/run.sh,
# scripts/cli-parity/run.sh):
#   * building both implementations (Rust always; the Go oracle best-effort),
#   * starting/stopping a daemon of either implementation on a temp unix
#     socket, with the same short-socket-dir care those scripts document,
#   * a tiny pass/fail assertion + counter vocabulary.
#
# Bash 3.2 compatible (macOS ships 3.2): no associative arrays, no mapfile.

# Repo root = two levels up from this file (scripts/parity/lib.sh).
PG_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PG_ROOT="$(cd "$PG_LIB_DIR/../.." && pwd)"

# Cargo's target dir — honour CARGO_TARGET_DIR (the docker-% wrapper points it
# at a named volume, /target, so binaries do NOT land under the working tree).
PG_TARGET_DIR="${CARGO_TARGET_DIR:-$PG_ROOT/target}"

# Binaries under test — overridable so CI can point at release builds.
PENGUIND="${PENGUIND:-$PG_TARGET_DIR/debug/penguind}"
PENGUIN_RS="${PENGUIN_RS:-$PG_TARGET_DIR/debug/pdcli}"
PROBE="${PROBE:-$PG_TARGET_DIR/debug/examples/parity_probe}"
PENGUIN_GO="${PENGUIN_GO:-$PG_ROOT/go-client/bin/penguin}"
PENGUIND_GO="${PENGUIND_GO:-$PG_ROOT/go-client/bin/penguind}"

# Result counters — every sub-gate shares this vocabulary so run.sh can
# aggregate a single verdict.
PG_CHECKS=0
PG_FAILS=0

pg_ok() {
    PG_CHECKS=$((PG_CHECKS + 1))
    echo "  PASS  $*"
}

pg_fail() {
    PG_CHECKS=$((PG_CHECKS + 1))
    PG_FAILS=$((PG_FAILS + 1))
    echo "  FAIL  $*" >&2
}

# Informational only — a skipped/pending/expected-diff note that never fails
# the gate (privileged-tier work, an absent Go oracle, etc.).
pg_note() {
    echo "  NOTE  $*"
}

# pg_assert_contains <desc> <haystack> <needle>
pg_assert_contains() {
    case "$2" in
        *"$3"*) pg_ok "$1" ;;
        *)
            pg_fail "$1: expected to contain '$3'"
            echo "    --- actual ---" >&2
            printf '%s\n' "$2" >&2
            ;;
    esac
}

# pg_assert_eq <desc> <actual> <expected>
pg_assert_eq() {
    if [ "$2" = "$3" ]; then
        pg_ok "$1"
    else
        pg_fail "$1: expected '$3', got '$2'"
    fi
}

# pg_assert_not_empty <desc> <value>
pg_assert_not_empty() {
    if [ -n "$2" ]; then
        pg_ok "$1"
    else
        pg_fail "$1: expected non-empty output"
    fi
}

# pg_summary <gate-name> — prints the tally and returns nonzero on any failure.
pg_summary() {
    echo "$1: $PG_CHECKS checks, $PG_FAILS failed"
    [ "$PG_FAILS" -eq 0 ]
}

pg_have_go_cli() { [ -x "$PENGUIN_GO" ]; }
pg_have_go_daemon() { [ -x "$PENGUIND_GO" ]; }

# Builds the Rust binaries + the raw wire probe example. Honours PG_SKIP_BUILD
# for iterating on the scripts against an already-built tree.
pg_build_rust() {
    [ "${PG_SKIP_BUILD:-0}" = "1" ] && return 0
    ( cd "$PG_ROOT" && cargo build -p penguind -p penguin --locked \
        && cargo build -p penguin --example parity_probe --locked )
}

# Best-effort Go oracle build. The frozen go-client/ tree has been removed
# from this repository, so this always self-skips now (a NOTE, not a
# failure) unless PENGUIN_GO/PENGUIND_GO are pointed at binaries built from a
# go-client checkout kept elsewhere — the Go-dependent checks then self-skip.
pg_build_go() {
    [ "${PG_SKIP_BUILD:-0}" = "1" ] && return 0
    if ! command -v go >/dev/null 2>&1; then
        pg_note "go toolchain not found — Go-oracle checks will self-skip"
        return 0
    fi
    ( cd "$PG_ROOT/go-client" && make build ) \
        || pg_note "go-client build failed — Go-oracle checks will self-skip"
}

# pg_daemon_start <binary> [config_yaml_contents]
#
# Starts a daemon (Rust or Go — identical --config-dir/--state-dir/--socket
# flags) on a fresh temp socket and waits for the socket to appear. On success
# sets PG_SOCKET, PG_DAEMON_PID, PG_WORKDIR, PG_SOCKDIR, PG_CONFIG_DIR,
# PG_STATE_DIR. Returns nonzero if the daemon never created the socket (the
# caller decides whether that is expected). One daemon at a time.
#
# The socket's parent dir must be one this process owns (the daemon chmods it
# to 0750) and the unix path limit is 103 bytes, so this uses its own
# short-named /tmp subdirectory — the same reasoning wire-compat/run.sh
# documents at length.
pg_daemon_start() {
    binary="$1"
    config_contents="${2:-}"

    PG_SOCKDIR="/tmp/pgp-$$-$RANDOM"
    mkdir -p "$PG_SOCKDIR"
    PG_SOCKET="$PG_SOCKDIR/d.sock"
    PG_WORKDIR="$(mktemp -d)"
    PG_CONFIG_DIR="$PG_WORKDIR/etc"
    PG_STATE_DIR="$PG_WORKDIR/state"
    mkdir -p "$PG_CONFIG_DIR" "$PG_STATE_DIR"
    if [ -n "$config_contents" ]; then
        printf '%s' "$config_contents" >"$PG_CONFIG_DIR/config.yaml"
    fi

    "$binary" --config-dir "$PG_CONFIG_DIR" --state-dir "$PG_STATE_DIR" \
        --socket "$PG_SOCKET" >"$PG_WORKDIR/daemon.log" 2>&1 &
    PG_DAEMON_PID=$!

    waited=0
    while [ ! -S "$PG_SOCKET" ]; do
        if ! kill -0 "$PG_DAEMON_PID" 2>/dev/null; then
            return 1
        fi
        sleep 0.1
        waited=$((waited + 1))
        if [ "$waited" -gt 100 ]; then
            return 1
        fi
    done
    return 0
}

# Stops the daemon started by pg_daemon_start and removes its scratch dirs.
# Safe to call when no daemon is running.
pg_daemon_stop() {
    if [ -n "${PG_DAEMON_PID:-}" ] && kill -0 "$PG_DAEMON_PID" 2>/dev/null; then
        kill "$PG_DAEMON_PID" 2>/dev/null || true
        wait "$PG_DAEMON_PID" 2>/dev/null || true
    fi
    PG_DAEMON_PID=""
    [ -n "${PG_WORKDIR:-}" ] && rm -rf "$PG_WORKDIR"
    [ -n "${PG_SOCKDIR:-}" ] && rm -rf "$PG_SOCKDIR"
}

# Dumps the current daemon's log to stderr — call from a sub-gate when an
# assertion fails and the daemon's own output would explain why.
pg_daemon_log() {
    if [ -n "${PG_WORKDIR:-}" ] && [ -f "$PG_WORKDIR/daemon.log" ]; then
        echo "    --- daemon log ---" >&2
        cat "$PG_WORKDIR/daemon.log" >&2
    fi
}

# pg_probe <socket> <op> [args...] — run the raw wire probe, echo its stdout.
pg_probe() {
    "$PROBE" "$@"
}

# pg_mode <path> — the file's permission bits as an octal string (e.g. 660),
# portably across GNU coreutils `stat` (Linux) and BSD `stat` (macOS). Prints
# nothing if the path does not exist.
pg_mode() {
    [ -e "$1" ] || return 0
    if stat -c '%a' "$1" >/dev/null 2>&1; then
        stat -c '%a' "$1"
    else
        stat -f '%Lp' "$1"
    fi
}

# pg_run_bounded <seconds> <cmd...> — run a command under a hard wall-clock
# cap so a network-reaching RPC (CheckUpdate) can never hang the gate. Uses
# GNU `timeout` (or macOS `gtimeout`); if neither exists it runs the command
# directly and relies on the RPC's own client-side timeout. Propagates the
# command's exit status (124 = timed out, from `timeout`).
pg_run_bounded() {
    seconds="$1"
    shift
    if command -v timeout >/dev/null 2>&1; then
        timeout "$seconds" "$@"
    elif command -v gtimeout >/dev/null 2>&1; then
        gtimeout "$seconds" "$@"
    else
        "$@"
    fi
}
