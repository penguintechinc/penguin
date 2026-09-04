#!/usr/bin/env bash
#
# M8 parity harness — umbrella driver.
#
# Builds both implementations, then runs every gating parity gate and
# aggregates a single verdict. It REUSES the two green precursors rather than
# reimplementing them:
#
#   scripts/wire-compat/run.sh   frozen Go CLI drives the Rust daemon (6 RPCs)
#   scripts/cli-parity/run.sh    Go CLI vs Rust CLI, byte-diff of built-in verbs
#
# and adds the M8 sub-gates:
#
#   rpc.sh        dim 1  — the RPCs wire-compat doesn't reach
#   cli-tree.sh   dim 2  — structural ListCommands diff + --help exit codes
#   config.sh     dim 3  — daemon-level config verdicts
#   events.sh     dim 5  — WatchEvents subscribe + trigger
#   perms.sh      dim 6  — socket/key/cache modes on real files
#   metrics       dim 4  — a cargo test (metrics_parity) owned by another
#                          workstream; PENDING (not a failure) if absent
#
# perf.sh (dim 7) is informational/non-gating and is deliberately NOT run here.
#
# Go-dependent gates (wire-compat, cli-parity, and the Go halves of config /
# cli-tree) self-skip with a NOTE when the Go toolchain/binaries are absent —
# e.g. inside the Rust-only penguin-rust:1.97 container. In CI both toolchains
# are present (the wire-compat job pattern) so everything runs.
#
# Bash 3.2 compatible.

set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/parity/lib.sh
. "$DIR/lib.sh"

cd "$PG_ROOT"

echo "############################################################"
echo "# M8 parity harness"
echo "############################################################"

pg_build_rust
pg_build_go

# Export resolved binary paths so the precursor scripts (which default to
# CWD-relative target/debug/...) and the sub-gates all agree, regardless of CWD.
export PENGUIND PENGUIN_RS PROBE PENGUIN_GO PENGUIND_GO

# --- gate ledger (bash 3.2: parallel positional lists, no assoc arrays) -----
GATE_NAMES=""
GATE_VERDICTS=""

record() {
    GATE_NAMES="$GATE_NAMES $1"
    GATE_VERDICTS="$GATE_VERDICTS $2"
}

# run_gate <name> <script-path> — run a gating sub-gate, capture its verdict.
run_gate() {
    name="$1"
    script="$2"
    echo ""
    echo "------------------------------------------------------------"
    if bash "$script"; then
        record "$name" "PASS"
    else
        record "$name" "FAIL"
    fi
}

# run_go_gate <name> <script-path> — a gate that needs the Go CLI; SKIP with a
# note when it is absent instead of failing.
run_go_gate() {
    name="$1"
    script="$2"
    echo ""
    echo "------------------------------------------------------------"
    if ! pg_have_go_cli; then
        echo "SKIP $name — Go CLI absent ($PENGUIN_GO)"
        record "$name" "SKIP"
        return 0
    fi
    if bash "$script"; then
        record "$name" "PASS"
    else
        record "$name" "FAIL"
    fi
}

# --- reused precursors (Go CLI required) ------------------------------------
run_go_gate "wire-compat" "$PG_ROOT/scripts/wire-compat/run.sh"
run_go_gate "cli-parity" "$PG_ROOT/scripts/cli-parity/run.sh"

# --- new M8 sub-gates -------------------------------------------------------
run_gate "rpc" "$DIR/rpc.sh"
run_gate "cli-tree" "$DIR/cli-tree.sh"
run_gate "config" "$DIR/config.sh"
run_gate "events" "$DIR/events.sh"
run_gate "perms" "$DIR/perms.sh"

# --- metrics gate (dim 4) — owned by another workstream ---------------------
# Wire it in by test name; adjust the package automatically from wherever that
# agent placed crates/**/tests/metrics_parity.rs. PENDING (not a failure) until
# it lands, so this harness never blocks on work it doesn't own.
echo ""
echo "------------------------------------------------------------"
METRICS_TEST="$(find "$PG_ROOT/crates" -path '*/tests/metrics_parity.rs' 2>/dev/null | head -1)"
if [ -n "$METRICS_TEST" ]; then
    metrics_crate_dir="$(dirname "$(dirname "$METRICS_TEST")")"
    metrics_pkg="$(grep -E '^name[[:space:]]*=' "$metrics_crate_dir/Cargo.toml" | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
    echo "metrics: running cargo test -p $metrics_pkg --test metrics_parity"
    if ( cd "$PG_ROOT" && cargo test -p "$metrics_pkg" --test metrics_parity --locked ); then
        record "metrics" "PASS"
    else
        record "metrics" "FAIL"
    fi
else
    echo "metrics: crates/**/tests/metrics_parity.rs not found — PENDING (owned by the metrics workstream)"
    record "metrics" "PENDING"
fi

# --- aggregate --------------------------------------------------------------
echo ""
echo "############################################################"
echo "# parity summary"
echo "############################################################"
# Iterate the two parallel lists in lockstep.
# Deliberate word-split of the space-separated verdict list into argv.
# shellcheck disable=SC2086
set -- $GATE_VERDICTS
failed=0
for name in $GATE_NAMES; do
    verdict="$1"
    shift
    printf '  %-14s %s\n' "$name" "$verdict"
    [ "$verdict" = "FAIL" ] && failed=$((failed + 1))
done

echo ""
if [ "$failed" -gt 0 ]; then
    echo "parity: FAIL — $failed gate(s) failed"
    exit 1
fi
echo "parity: PASS — every gating check passed (SKIP/PENDING are non-blocking)"
