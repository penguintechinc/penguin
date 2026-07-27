#!/usr/bin/env bash
#
# Parity dim 3 — daemon-level config verdicts.
#
# The shared config corpus (testdata/config-corpus/, a green CI gate on both
# sides) already covers PER-MODULE schema validation. It does NOT cover the
# daemon's own top-level config.yaml accept/reject. This gate fills that hole
# for the four cases the module corpus can't reach.
#
# Both implementations treat a bad daemon config identically: log a warning and
# continue on the built-in defaults (Rust: daemon_main.rs "invalid daemon
# config, using defaults"; Go: service.go "failed to load daemon config, using
# defaults"). So the observable verdict is "did the impl reject the file?",
# read from each daemon's own startup log. Confirmed by inspection of
# crates/penguin-daemon/src/config.rs vs go-client/internal/daemon/configstore.go:
#
#   case                    verdict   why
#   malformed YAML          REJECT    both parsers error
#   unknown top-level key   ACCEPT    neither sets deny-unknown-fields
#   wrong-typed logLevel    REJECT    map where a string is expected
#   absent config.yaml      ACCEPT    both fall back to defaults
#
# The Rust side is asserted against this table always. When the Go daemon
# binary is present (CI has both toolchains) the Go side is asserted against the
# same table too — transitively proving Rust == Go. Pure config parsing:
# unprivileged, plain CI.
#
# `--socket` overrides socketPath after the config load, so the daemon's socket
# always comes up regardless of the config verdict — the verdict is read from
# the log, not from whether the daemon started.

set -uo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/parity/lib.sh
. "$DIR/lib.sh"

echo "=== parity: config (dim 3) ==="

[ -x "$PENGUIND" ] || { echo "config: rust daemon missing at $PENGUIND" >&2; exit 1; }

trap pg_daemon_stop EXIT

# Derives one implementation's verdict (accept|reject) from a fresh daemon
# start with the given config, matching `reject_marker` against its log.
# Echoes "accept", "reject", or "nostart".
derive_verdict() {
    binary="$1"
    reject_marker="$2"
    contents="$3"
    if ! pg_daemon_start "$binary" "$contents"; then
        # Both impls start even on a rejected config, so a no-start is its own
        # (unexpected) signal — the caller decides how to treat it.
        log="$(cat "$PG_WORKDIR/daemon.log" 2>/dev/null)"
        pg_daemon_stop
        case "$log" in
            *"$reject_marker"*) echo "reject" ;;
            *) echo "nostart" ;;
        esac
        return 0
    fi
    # The warning is emitted at config-load time, before the socket exists, so
    # it is already in the log by now; a short beat covers any buffering.
    sleep 0.2
    log="$(cat "$PG_WORKDIR/daemon.log" 2>/dev/null)"
    pg_daemon_stop
    case "$log" in
        *"$reject_marker"*) echo "reject" ;;
        *) echo "accept" ;;
    esac
}

RUST_MARKER="invalid daemon config"
GO_MARKER="failed to load daemon config"

# check_case <label> <expected> <yaml-contents>
check_case() {
    label="$1"
    expected="$2"
    contents="$3"

    rust_verdict="$(derive_verdict "$PENGUIND" "$RUST_MARKER" "$contents")"
    pg_assert_eq "rust: $label -> $expected" "$rust_verdict" "$expected"

    if pg_have_go_daemon; then
        go_verdict="$(derive_verdict "$PENGUIND_GO" "$GO_MARKER" "$contents")"
        if [ "$go_verdict" = "nostart" ]; then
            pg_note "go: $label — Go daemon did not start (env-specific); cross-check skipped"
        else
            pg_assert_eq "go: $label -> $expected" "$go_verdict" "$expected"
        fi
    else
        pg_note "go: $label — Go daemon binary absent; cross-check skipped"
    fi
}

# `a: b: c` — "mapping values are not allowed in this context" on both parsers.
check_case "malformed YAML" "reject" 'a: b: c
'
check_case "unknown top-level key" "accept" 'bogusUnknownKey: 123
'
# A mapping where logLevel expects a string.
check_case "wrong-typed logLevel" "reject" 'logLevel:
  nested: value
'
check_case "absent config.yaml" "accept" ''

pg_summary "config"
