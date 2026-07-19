#!/usr/bin/env bash
#
# M4 CLI golden-diff gate: the FROZEN Go `penguin` CLI and the Rust `penguin`
# CLI both drive the SAME running Rust `penguind`. For each command this
# diffs stdout, stderr, and exit code between the two CLIs, then does it
# again after killing the daemon (the "is it down" message matters as much
# as the happy path).
#
# This is stricter than wire-compat/run.sh, which only proves the Go CLI can
# drive the Rust daemon at the protocol level. This gate proves the Rust CLI
# is a faithful *user-facing* replacement for the Go one — same words, same
# exit codes — everywhere the two are supposed to agree, and it names every
# place they are deliberately allowed to differ.
#
# Ground rule: fail on any UNEXPECTED difference; print every expected one.
# Expected differences are hard-coded in the `check_*` functions below, each
# with a comment citing the docs/PARITY.md section that documents it. Adding
# a newly-accepted difference is a one-line change to the relevant
# function's condition, plus the comment explaining why.
#
# Usage:
#   PENGUIND=... PENGUIN_RS=... PENGUIN_GO=... scripts/cli-parity/run.sh
#
# Bash 3.2 compatible (macOS ships 3.2): no associative arrays, no mapfile.

set -uo pipefail
# Deliberately NOT `set -e`: several of these commands are SUPPOSED to exit
# non-zero (load of an unknown module, anything run against a dead daemon).
# The gate inspects exit codes itself rather than letting one abort the
# script.

PENGUIND="${PENGUIND:-target/debug/penguind}"
PENGUIN_RS="${PENGUIN_RS:-target/debug/penguin}"
PENGUIN_GO="${PENGUIN_GO:-go-client/bin/penguin}"

fail_setup() {
    echo "cli-parity: FAIL: $*" >&2
    exit 1
}

[ -x "$PENGUIND" ] || fail_setup "rust daemon not executable at $PENGUIND"
[ -x "$PENGUIN_RS" ] || fail_setup "rust CLI not executable at $PENGUIN_RS"
[ -x "$PENGUIN_GO" ] || fail_setup "go CLI not executable at $PENGUIN_GO"

# Same reasoning as scripts/wire-compat/run.sh: the socket's parent dir must
# be one this process owns (the daemon chmods it to 0750), and the unix
# socket path limit is 103 bytes, so this uses its own short-named
# subdirectory rather than a bare /tmp or a deeply nested mktemp dir.
SOCKDIR="/tmp/pgcp-$$"
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

echo "cli-parity: starting rust penguind on $SOCKET"
"$PENGUIND" --config-dir "$CONFIG_DIR" --state-dir "$STATE_DIR" --socket "$SOCKET" \
    >"$WORKDIR/daemon.log" 2>&1 &
DAEMON_PID=$!

waited=0
while [ ! -S "$SOCKET" ]; do
    if ! kill -0 "$DAEMON_PID" 2>/dev/null; then
        echo "--- daemon log ---" >&2
        cat "$WORKDIR/daemon.log" >&2
        fail_setup "daemon exited before creating the socket"
    fi
    sleep 0.1
    waited=$((waited + 1))
    if [ "$waited" -gt 100 ]; then
        echo "--- daemon log ---" >&2
        cat "$WORKDIR/daemon.log" >&2
        fail_setup "socket did not appear within 10s"
    fi
done
echo "cli-parity: daemon up (pid $DAEMON_PID)"

# --- result tracking ---------------------------------------------------
# Bash 3.2 has no associative arrays, so results are just running counters
# plus each check printing its own verdict as it happens.
CHECKED=0
UNEXPECTED=0

report() {
    # report <label> <verdict: identical|expected|unexpected> <detail>
    CHECKED=$((CHECKED + 1))
    case "$2" in
        identical) echo "cli-parity: IDENTICAL     — $1" ;;
        expected) echo "cli-parity: EXPECTED DIFF — $1 ($3)" ;;
        unexpected)
            UNEXPECTED=$((UNEXPECTED + 1))
            echo "cli-parity: UNEXPECTED    — $1 ($3)" >&2
            ;;
    esac
}

dump_both() {
    echo "  --- $1: go    exit=$GO_EXIT stdout ---" >&2
    printf '%s\n' "$GO_OUT" >&2
    echo "  --- $1: go    stderr ---" >&2
    printf '%s\n' "$GO_ERR" >&2
    echo "  --- $1: rust  exit=$RS_EXIT stdout ---" >&2
    printf '%s\n' "$RS_OUT" >&2
    echo "  --- $1: rust  stderr ---" >&2
    printf '%s\n' "$RS_ERR" >&2
}

# Runs a CLI binary with the shared --socket plus the given args, capturing
# stdout/stderr/exit code into the GO_*/RS_* globals via capture_go/capture_rs
# below. Never lets a nonzero exit escape — most of what this gate runs is
# supposed to fail.
run_cli() {
    bin="$1"
    shift
    "$bin" --socket "$SOCKET" "$@" >"$WORKDIR/out" 2>"$WORKDIR/err"
    LAST_EXIT=$?
    LAST_OUT="$(cat "$WORKDIR/out")"
    LAST_ERR="$(cat "$WORKDIR/err")"
}

capture_go() {
    run_cli "$PENGUIN_GO" "$@"
    GO_OUT="$LAST_OUT"
    GO_ERR="$LAST_ERR"
    GO_EXIT="$LAST_EXIT"
}

capture_rs() {
    run_cli "$PENGUIN_RS" "$@"
    RS_OUT="$LAST_OUT"
    RS_ERR="$LAST_ERR"
    RS_EXIT="$LAST_EXIT"
}

# Byte-for-byte comparison of exit code, stdout, and stderr. The default for
# any command not given its own bespoke check_* function below — most of
# them (modules, status, load, unload, and logs against a dead daemon) are
# genuinely identical between the two CLIs, so a plain diff is the strongest
# assertion and the right default.
compare_exact() {
    if [ "$GO_EXIT" = "$RS_EXIT" ] && [ "$GO_OUT" = "$RS_OUT" ] && [ "$GO_ERR" = "$RS_ERR" ]; then
        report "$1" identical ""
    else
        report "$1" unexpected "byte-for-byte diff — see dump below"
        dump_both "$1"
    fi
}

check_exact() {
    label="$1"
    shift
    capture_go "$@"
    capture_rs "$@"
    compare_exact "$label"
}

# `penguin version` against a live daemon. Both CLIs print their OWN local
# version on line 1 ("penguin version <local>") before the shared
# "penguind version <X>" line — Go's frozen build reports its hardcoded
# internal/version.Version ("dev" unless built with -ldflags), Rust reports
# its own Cargo package version. That is an inherent identity difference
# between two different binaries, not a docs/PARITY.md divergence, so it is
# normalized away here rather than treated as a finding: only the shared
# daemon-version line (and everything else) needs to match exactly.
check_version_up() {
    capture_go version
    capture_rs version

    go_tail="$(printf '%s\n' "$GO_OUT" | tail -n +2)"
    rs_tail="$(printf '%s\n' "$RS_OUT" | tail -n +2)"
    go_first_line_ok="$(printf '%s\n' "$GO_OUT" | head -n1 | grep -c '^penguin version ')"
    rs_first_line_ok="$(printf '%s\n' "$RS_OUT" | head -n1 | grep -c '^penguin version ')"

    if [ "$GO_EXIT" != "0" ] || [ "$RS_EXIT" != "0" ] || [ "$GO_ERR" != "" ] || [ "$RS_ERR" != "" ] \
        || [ "$go_first_line_ok" != "1" ] || [ "$rs_first_line_ok" != "1" ] || [ "$go_tail" != "$rs_tail" ]; then
        report "version (daemon up)" unexpected "expected only the local-version line to differ"
        dump_both "version (daemon up)"
    elif [ "$GO_OUT" = "$RS_OUT" ]; then
        report "version (daemon up)" identical ""
    else
        report "version (daemon up)" expected "different local client version strings (Go's frozen build vs. Rust's Cargo version) — daemon version line is identical"
    fi
}

# `penguin version` against a dead daemon.
#
# docs/PARITY.md §1.11: Go's cmdVersion checks the Version RPC's error and
# discards it, so a dead daemon still exits 0 having printed only the local
# version line — no failure is visible at all.
# docs/PARITY.md §1.14: the friendly daemon-down message is Go's
# static-verb-only concept; Rust applies it uniformly to every RPC instead,
# so Rust additionally reports the failure on stderr and exits non-zero.
check_version_down() {
    capture_go version
    capture_rs version

    go_ok="no"
    if [ "$GO_EXIT" = "0" ] && [ "$GO_ERR" = "" ] && printf '%s' "$GO_OUT" | grep -q '^penguin version '; then
        go_ok="yes"
    fi
    rs_ok="no"
    if [ "$RS_EXIT" != "0" ] && printf '%s' "$RS_ERR" | grep -q "cannot reach penguind"; then
        rs_ok="yes"
    fi

    if [ "$go_ok" = "yes" ] && [ "$rs_ok" = "yes" ]; then
        report "version (daemon down)" expected "Go swallows the RPC error (PARITY §1.11) and exits 0; Rust surfaces the friendly daemon-down message (PARITY §1.14) and exits nonzero"
    else
        report "version (daemon down)" unexpected "expected Go to swallow the error (exit 0, no stderr) and Rust to report daemon-down (nonzero, 'cannot reach penguind' on stderr)"
        dump_both "version (daemon down)"
    fi
}

# `penguin logs --lines 5` against a live daemon. Non-following (the CLI
# default) and bounded, so this is a single backlog replay, never a stream
# the harness could hang on.
#
# docs/PARITY.md §2.6: log line timestamps render in the host's local
# timezone on Go, UTC on Rust — this strips the leading "[...]" bracket from
# every line before comparing. Level and message (everything after the
# bracket) must still match exactly, including line count.
check_logs_up() {
    capture_go logs --lines 5
    capture_rs logs --lines 5

    go_stripped="$(printf '%s\n' "$GO_OUT" | sed -E 's/^\[[^]]*\] //')"
    rs_stripped="$(printf '%s\n' "$RS_OUT" | sed -E 's/^\[[^]]*\] //')"

    if [ "$GO_EXIT" != "$RS_EXIT" ] || [ "$GO_ERR" != "$RS_ERR" ] || [ "$go_stripped" != "$rs_stripped" ]; then
        report "logs --lines 5 (daemon up)" unexpected "expected level/message content (ignoring the timestamp bracket) to match exactly"
        dump_both "logs --lines 5 (daemon up)"
    elif [ "$GO_OUT" = "$RS_OUT" ]; then
        report "logs --lines 5 (daemon up)" identical ""
    else
        report "logs --lines 5 (daemon up)" expected "timestamps differ (PARITY §2.6: Rust renders UTC, Go renders local time) — level/message content is identical"
    fi
}

echo ""
echo "=== daemon-up comparisons ==="
check_version_up
check_exact "modules (daemon up)" modules
check_exact "status (daemon up)" status
check_logs_up
check_exact "load nope (daemon up)" load nope
check_exact "unload nope (daemon up)" unload nope

echo ""
echo "cli-parity: stopping daemon for daemon-down comparisons"
kill "$DAEMON_PID" 2>/dev/null || true
wait "$DAEMON_PID" 2>/dev/null || true
DAEMON_PID=""
# The daemon removes its own socket on graceful shutdown (see
# daemon_main.rs); this is a safety net, not something the checks below
# depend on — the whole point of the down-phase is that dialing fails.
[ -S "$SOCKET" ] && rm -f "$SOCKET"

echo ""
echo "=== daemon-down comparisons ==="
check_version_down
check_exact "modules (daemon down)" modules
check_exact "status (daemon down)" status
check_exact "logs --lines 5 (daemon down)" logs --lines 5
check_exact "load nope (daemon down)" load nope
check_exact "unload nope (daemon down)" unload nope

echo ""
echo "cli-parity: $CHECKED checks, $UNEXPECTED unexpected"
if [ "$UNEXPECTED" -gt 0 ]; then
    echo "cli-parity: FAIL — unexpected difference(s) found" >&2
    exit 1
fi
echo "cli-parity: PASS — every difference is either identical or documented in docs/PARITY.md"
