#!/usr/bin/env bash
#
# Parity dim 2 — CLI command-tree structure.
#
# clap and cobra render `--help` in fundamentally different layouts, so a
# byte-diff of help text will NEVER match and is not attempted. Instead this
# gate compares the STRUCTURE the trees are built from:
#
#   1. Structural ListCommands diff — the raw wire probe dumps each daemon's
#      module command tree (name / use / short / flag name:shorthand:type:
#      default / min/max args / tray bit). Because the probe speaks the shared
#      penguin-proto contract both daemons serve, the SAME probe dumps the Rust
#      daemon and (when built) the Go daemon; the two dumps must be identical.
#      This is the real Rust-vs-Go module-tree parity check.
#
#   2. `--help` exit codes — every static verb and every squawk subcommand must
#      exit 0 under BOTH the Go CLI and the Rust CLI (driving the same Rust
#      daemon, the source of the grafted module subtree), proving each CLI
#      actually exposes each command.
#
# squawk loads unprivileged (forwarder off); tobogganing is loaded best-effort
# for --help coverage (its background connect needs privilege it degrades from
# gracefully). Plain CI.

set -uo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/parity/lib.sh
. "$DIR/lib.sh"

echo "=== parity: cli-tree (dim 2) ==="

[ -x "$PENGUIND" ] || { echo "cli-tree: rust daemon missing at $PENGUIND" >&2; exit 1; }
[ -x "$PENGUIN_RS" ] || { echo "cli-tree: rust CLI missing at $PENGUIN_RS" >&2; exit 1; }
[ -x "$PROBE" ] || { echo "cli-tree: wire probe missing at $PROBE" >&2; exit 1; }

# Tree dumps must survive pg_daemon_stop (which deletes the daemon's WORKDIR),
# since the Rust dump is captured, the Rust daemon stopped, then the Go daemon
# started before the two are diffed — so they live in their own temp dir.
PARITY_TMP="$(mktemp -d)"
trap 'pg_daemon_stop; rm -rf "$PARITY_TMP"' EXIT

STATIC_VERBS="version modules load unload status logs update"
# Representative squawk paths spanning leaves, groups, nested subcommands, and
# a flag-bearing command.
SQUAWK_PATHS="config query forward forward/status cache cache/stats"

# assert_help_ok <cli-bin> <label> <path-with-slashes>
# Runs `<bin> --socket <sock> <space-split path> --help` and asserts exit 0.
assert_help_ok() {
    bin="$1"
    label="$2"
    path="$3"
    # Deliberate word-split: turn a slash-path ("forward/status") into argv.
    # shellcheck disable=SC2046,SC2086
    set -- $(printf '%s' "$path" | tr '/' ' ')
    "$bin" --socket "$PG_SOCKET" "$@" --help >/dev/null 2>&1
    rc=$?
    if [ "$rc" -eq 0 ]; then
        pg_ok "$label"
    else
        pg_fail "$label: --help exited $rc (expected 0)"
    fi
}

# waive_squawk_tree — canonicalise the ONE deliberate, source-documented
# Rust<->Go tree divergence so it does not read as a failure, while leaving
# every other structural difference intact.
#
# The Rust squawk port collapses Go's phantom "config show" / "license status"
# two-word usage — a subcommand NEITHER implementation ever routed on — down to
# the bare command name, and drops the never-inspected optional positional
# (max 1 -> 0). Authority: crates/penguin-module-squawk/src/commands.rs module
# doc ("# `config`/`license`'s `Use` string"); a docs/PARITY.md §1-class entry
# ("Rust is deliberately better") is pending. This rewrites Go's two lines to
# the Rust form (a no-op on the already-collapsed Rust tree), addressed to those
# exact lines so a real divergence anywhere else survives and still fails the
# diff. Plain BRE (no -E): `|` is a literal char in both GNU and BSD sed.
waive_squawk_tree() {
    sed \
        -e '/^PROBE cmd squawk|config|/ s/|use=config show|/|use=config|/' \
        -e '/^PROBE cmd squawk|config|/ s/|max=1|/|max=0|/' \
        -e '/^PROBE cmd squawk|license|/ s/|use=license status|/|use=license|/' \
        -e '/^PROBE cmd squawk|license|/ s/|max=1|/|max=0|/'
}

# --- Rust daemon: load squawk, capture its structural tree -------------------
if ! pg_daemon_start "$PENGUIND"; then
    echo "cli-tree: rust daemon failed to start" >&2
    pg_daemon_log
    exit 1
fi

load_out="$("$PENGUIN_RS" --socket "$PG_SOCKET" load squawk 2>&1)"
pg_assert_contains "load squawk on the rust daemon" "$load_out" "squawk"

rust_tree="$PARITY_TMP/rust-tree.txt"
pg_probe "$PG_SOCKET" list-commands | grep '^PROBE cmd ' | LC_ALL=C sort >"$rust_tree"
pg_assert_not_empty "rust ListCommands tree is non-empty" "$(cat "$rust_tree")"
pg_assert_contains "rust tree includes squawk config" "$(cat "$rust_tree")" "squawk|config|"

# --help exit codes for static verbs + squawk subcommands, Rust CLI.
for verb in $STATIC_VERBS; do
    assert_help_ok "$PENGUIN_RS" "rust CLI: '$verb --help' exits 0" "$verb"
done
assert_help_ok "$PENGUIN_RS" "rust CLI: 'squawk --help' exits 0" "squawk"
for path in $SQUAWK_PATHS; do
    assert_help_ok "$PENGUIN_RS" "rust CLI: 'squawk $path --help' exits 0" "squawk/$path"
done

# Same --help checks under the Go CLI (drives the same Rust daemon).
if pg_have_go_cli; then
    for verb in $STATIC_VERBS; do
        assert_help_ok "$PENGUIN_GO" "go CLI: '$verb --help' exits 0" "$verb"
    done
    assert_help_ok "$PENGUIN_GO" "go CLI: 'squawk --help' exits 0" "squawk"
    for path in $SQUAWK_PATHS; do
        assert_help_ok "$PENGUIN_GO" "go CLI: 'squawk $path --help' exits 0" "squawk/$path"
    done
else
    pg_note "Go CLI absent — its --help exit-code checks skipped"
fi

# tobogganing is best-effort: its tree + --help when it loads, a NOTE if not.
tob_out="$("$PENGUIN_RS" --socket "$PG_SOCKET" load tobogganing 2>&1)"
if printf '%s' "$tob_out" | grep -qi "tobogganing"; then
    if pg_probe "$PG_SOCKET" list-commands | grep -q '^PROBE cmd tobogganing|'; then
        pg_ok "tobogganing tree present after load"
        assert_help_ok "$PENGUIN_RS" "rust CLI: 'tobogganing --help' exits 0" "tobogganing"
    else
        pg_note "tobogganing loaded but exposed no commands"
    fi
else
    pg_note "tobogganing did not load unprivileged (background connect needs privilege) — subcommand tree deferred to the integration tier"
fi

pg_daemon_stop

# --- Go daemon: same squawk tree, structural diff ---------------------------
if pg_have_go_daemon; then
    if pg_daemon_start "$PENGUIND_GO"; then
        go_load="$("$PENGUIN_RS" --socket "$PG_SOCKET" load squawk 2>&1)"
        pg_assert_contains "load squawk on the go daemon" "$go_load" "squawk"
        go_tree="$PARITY_TMP/go-tree.txt"
        pg_probe "$PG_SOCKET" list-commands | grep '^PROBE cmd ' | LC_ALL=C sort >"$go_tree"
        if diff -u "$rust_tree" "$go_tree" >"$PARITY_TMP/tree.diff" 2>&1; then
            pg_ok "squawk ListCommands tree is byte-identical between Rust and Go daemons"
        else
            # Raw trees differ only by the documented config/license use-string
            # divergence. Waive exactly that (see waive_squawk_tree) and re-diff;
            # any OTHER difference survives normalisation and still fails here.
            waive_squawk_tree <"$rust_tree" >"$PARITY_TMP/rust-tree.norm"
            waive_squawk_tree <"$go_tree" >"$PARITY_TMP/go-tree.norm"
            if diff -u "$PARITY_TMP/rust-tree.norm" "$PARITY_TMP/go-tree.norm" \
                >"$PARITY_TMP/tree.norm.diff" 2>&1; then
                pg_ok "squawk ListCommands tree matches Rust<->Go modulo the documented config/license use-string divergence (waived)"
                pg_note "waived: Rust collapses Go's phantom 'config show'/'license status' usage to 'config'/'license' (commands.rs module doc; docs/PARITY.md entry pending)"
            else
                pg_fail "squawk ListCommands tree differs between Rust and Go daemons beyond the documented config/license waiver"
                echo "    --- residual tree diff after waiver (rust vs go) ---" >&2
                cat "$PARITY_TMP/tree.norm.diff" >&2
            fi
        fi
        pg_daemon_stop
    else
        pg_note "Go daemon did not start — structural tree diff skipped"
        pg_daemon_stop
    fi
else
    pg_note "Go daemon binary absent — structural Rust-vs-Go tree diff skipped (runs in CI)"
fi

pg_summary "cli-tree"
