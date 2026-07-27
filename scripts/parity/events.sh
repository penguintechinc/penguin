#!/usr/bin/env bash
#
# Parity dim 5 — event semantics (WatchEvents).
#
# Subscribe to the event stream, then trigger a module load in the foreground
# and assert the expected `Event` arrives with the right `type` string and
# module.
#
# Oracle caveat (docs/PARITY.md §1.1): the Go *daemon*'s WatchEvents is a
# dead-end that emits nothing ever, so it CANNOT be used as the event oracle.
# The only meaningful check is the Rust daemon's stream against the
# proto/PARITY contract — a load must publish a `state-changed` event naming
# the module (§1.2: `running` is published only after Start succeeds; the
# earlier `initializing` state-change is published first). This is the M8
# behaviour the Go implementation never had.
#
# Unprivileged (loads the squawk builtin with its forwarder off); plain CI.

set -uo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/parity/lib.sh
. "$DIR/lib.sh"

echo "=== parity: events (dim 5) ==="

[ -x "$PENGUIND" ] || { echo "events: rust daemon missing at $PENGUIND" >&2; exit 1; }
[ -x "$PENGUIN_RS" ] || { echo "events: rust CLI missing at $PENGUIN_RS" >&2; exit 1; }
[ -x "$PROBE" ] || { echo "events: wire probe missing at $PROBE" >&2; exit 1; }

trap pg_daemon_stop EXIT

if ! pg_daemon_start "$PENGUIND"; then
    echo "events: rust daemon failed to start" >&2
    pg_daemon_log
    exit 1
fi

pg_note "Go daemon is not the oracle here — its WatchEvents emits nothing (§1.1)"

# Subscribe FIRST (filtered to squawk), then trigger the load, so the
# state-change events the load publishes cannot be missed. The probe collects
# one event then returns, bounded by its own timeout so it can never hang.
events_out="$PG_WORKDIR/events.out"
PROBE_EVENT_COUNT=1 PROBE_TIMEOUT_MS=6000 \
    "$PROBE" "$PG_SOCKET" watch-events squawk >"$events_out" 2>&1 &
probe_pid=$!

# Give the subscription a moment to reach the broker before publishing.
sleep 0.5

load_out="$("$PENGUIN_RS" --socket "$PG_SOCKET" load squawk 2>&1)"
pg_assert_contains "load squawk publishes to the broker" "$load_out" "squawk"

wait "$probe_pid" 2>/dev/null || true
captured="$(cat "$events_out")"

pg_assert_contains "WatchEvents delivers an event for the loaded module" "$captured" "PROBE event module=squawk"
pg_assert_contains "the load event carries a state-changed type (§1.2)" "$captured" "type=state-changed"

if ! printf '%s' "$captured" | grep -q "PROBE event module=squawk"; then
    echo "    --- captured stream ---" >&2
    printf '%s\n' "$captured" >&2
    pg_daemon_log
fi

pg_summary "events"
