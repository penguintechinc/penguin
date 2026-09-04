#!/usr/bin/env bash
#
# Parity dim 6 — file permissions on the running daemon's real artifacts.
#
# Per-crate Rust unit tests already assert each mode in isolation
# (penguin-ipc listen_unix.rs, penguin-secrets master_key.rs,
# penguin-licensing cache.rs). This gate is the confirmation layer: it starts
# the actual daemon and `stat`s the files it really creates, proving the modes
# hold end-to-end, not just in a unit fixture. The Go daemon pins the identical
# modes (go-client internal/ipc, internal/secrets, internal/licensing), so
# "both pin the same modes" is true by inspection; this proves the Rust side on
# real files.
#
#   * control socket        0660   (parent dir 0750)
#   * secrets master key     0600
#   * offline license cache  0600   (only present if the license fetch ran;
#                                     network-dependent, so NOTE when absent)
#
# All three are the daemon's own owner-writable files — unprivileged, plain CI.

set -uo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/parity/lib.sh
. "$DIR/lib.sh"

echo "=== parity: perms (dim 6) ==="

[ -x "$PENGUIND" ] || { echo "perms: rust daemon missing at $PENGUIND" >&2; exit 1; }

trap pg_daemon_stop EXIT

if ! pg_daemon_start "$PENGUIND"; then
    echo "perms: rust daemon failed to start" >&2
    pg_daemon_log
    exit 1
fi

# The secrets store (and its master key) is opened at startup; create a secret
# is not needed. Give the async startup a beat to finish writing the key.
key_path="$PG_STATE_DIR/secrets/master.key"
waited=0
while [ ! -e "$key_path" ] && [ "$waited" -lt 50 ]; do
    sleep 0.1
    waited=$((waited + 1))
done

pg_assert_eq "control socket is 0660" "$(pg_mode "$PG_SOCKET")" "660"
pg_assert_eq "socket parent dir is 0750" "$(pg_mode "$PG_SOCKDIR")" "750"

if [ -e "$key_path" ]; then
    pg_assert_eq "secrets master key is 0600" "$(pg_mode "$key_path")" "600"
else
    pg_fail "secrets master key was never created at $key_path"
    pg_daemon_log
fi

cache_path="$PG_STATE_DIR/license/license-cache.json"
if [ -e "$cache_path" ]; then
    pg_assert_eq "offline license cache is 0600" "$(pg_mode "$cache_path")" "600"
else
    pg_note "license cache not populated (no network / no valid license) — mode covered by penguin-licensing cache.rs unit test"
fi

pg_summary "perms"
