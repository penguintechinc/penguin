#!/usr/bin/env bash
#
# Parity dim 1 — daemon gRPC RPC coverage.
#
# scripts/wire-compat/run.sh already drives 6 of the 11 daemon RPCs with the
# frozen Go CLI (Version, ListModules, GetStatus, LoadModule-reject, TailLogs,
# UnloadModule). This gate closes the remaining wire coverage the frozen CLIs
# structurally cannot reach:
#
#   * ListCommands  — load squawk, assert a non-empty command tree comes back.
#   * Dispatch      — run a real squawk builtin subcommand, assert EXACTLY one
#                     final:true chunk (docs/PARITY.md §2.3).
#   * WatchEvents   — open the stream cleanly (the full subscribe+trigger test
#                     lives in events.sh, dim 5).
#   * CheckUpdate   — verify it is bounded / fail-closed offline and never
#                     hangs (D6); its availability result is environment
#                     dependent, so it is not asserted.
#   * ApplyUpdate   — assert OK-status-with-applied:false, NEVER a gRPC error
#                     (docs/PARITY.md §2.2). Network-free: apply fails closed
#                     before any network call when no publisher key is embedded.
#   * api_version   — an unknown version is rejected UNIMPLEMENTED over the
#                     wire, and that rejection precedes the update path.
#
# The ListCommands/Dispatch/WatchEvents/api_version checks use the raw wire
# probe (bins/penguin/examples/parity_probe.rs) because the frozen CLIs only
# speak api_version="v1" and only reach ApplyUpdate through the gated update
# flow. Everything here is unprivileged (squawk with the forwarder off) and
# runs in plain CI.

set -uo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/parity/lib.sh
. "$DIR/lib.sh"

echo "=== parity: rpc (dim 1) ==="

[ -x "$PENGUIND" ] || { echo "rpc: rust daemon missing at $PENGUIND" >&2; exit 1; }
[ -x "$PENGUIN_RS" ] || { echo "rpc: rust CLI missing at $PENGUIN_RS" >&2; exit 1; }
[ -x "$PROBE" ] || { echo "rpc: wire probe missing at $PROBE" >&2; exit 1; }

trap pg_daemon_stop EXIT

if ! pg_daemon_start "$PENGUIND"; then
    echo "rpc: rust daemon failed to start" >&2
    pg_daemon_log
    exit 1
fi

# ListCommands returns only *loaded* modules, and Dispatch needs a loaded
# target, so load the squawk builtin first. It is core product (no license
# gate) and, with the forwarder disabled by default, loads unprivileged.
load_out="$("$PENGUIN_RS" --socket "$PG_SOCKET" load squawk 2>&1)"
pg_assert_contains "load squawk (unprivileged)" "$load_out" "squawk"

# --- ListCommands -----------------------------------------------------------
lc_out="$(pg_probe "$PG_SOCKET" list-commands)"
pg_assert_contains "ListCommands returns squawk's tree" "$lc_out" "PROBE cmd squawk|"
pg_assert_contains "ListCommands reports at least one module" "$lc_out" "modules="

# --- Dispatch (§2.3 single final chunk) ------------------------------------
# `squawk config` is read-only, network-free, and always exits 0.
disp_out="$(pg_probe "$PG_SOCKET" dispatch squawk config)"
pg_assert_contains "Dispatch streams exactly one final chunk (§2.3)" "$disp_out" "finals=1"
pg_assert_contains "Dispatch reports the command's exit code" "$disp_out" "exit=0"
pg_assert_contains "Dispatch completes with an OK status" "$disp_out" "PROBE status=ok"

# --- WatchEvents (stream opens cleanly; full test in events.sh) -------------
we_out="$(PROBE_EVENT_COUNT=1 PROBE_TIMEOUT_MS=800 pg_probe "$PG_SOCKET" watch-events)"
pg_assert_contains "WatchEvents opens a stream without error" "$we_out" "PROBE done count="

# --- api_version rejection over the wire ------------------------------------
bad_ver="$(PROBE_API_VERSION="v-not-real" pg_probe "$PG_SOCKET" version)"
pg_assert_contains "unknown api_version -> UNIMPLEMENTED (Version)" "$bad_ver" "PROBE status=Unimplemented"
bad_apply="$(PROBE_API_VERSION="v-not-real" pg_probe "$PG_SOCKET" apply-update)"
pg_assert_contains "api_version checked before the update path (ApplyUpdate)" "$bad_apply" "PROBE status=Unimplemented"

# --- ApplyUpdate (§2.2 never a gRPC error) ----------------------------------
apply_out="$(pg_probe "$PG_SOCKET" apply-update)"
pg_assert_contains "ApplyUpdate returns an OK status, not a gRPC error (§2.2)" "$apply_out" "PROBE status=ok"
pg_assert_contains "ApplyUpdate reports applied=false when it cannot act (§2.2)" "$apply_out" "applied=0"

# --- CheckUpdate (D6: bounded / fail-closed, never hangs) -------------------
# CheckUpdate may reach api.github.com; wrap it in a hard cap so a black-hole
# network can never hang the gate. Any terminal status is acceptable — the
# assertion is "it returned and the daemon survived", not a specific verdict.
cu_out="$(pg_run_bounded 40 "$PROBE" "$PG_SOCKET" check-update 2>&1)"
cu_rc=$?
if [ "$cu_rc" -eq 124 ]; then
    pg_fail "CheckUpdate did not return within 40s (D6: must fail-closed, not hang)"
else
    pg_assert_contains "CheckUpdate returns a terminal status without hanging (D6)" "$cu_out" "PROBE status="
fi
if kill -0 "$PG_DAEMON_PID" 2>/dev/null; then
    pg_ok "daemon survived the CheckUpdate round trip"
else
    pg_fail "daemon died during CheckUpdate"
    pg_daemon_log
fi

pg_summary "rpc"
