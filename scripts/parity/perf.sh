#!/usr/bin/env bash
#
# Parity dim 7 — perf sanity (forwarder QPS, daemon RSS). INFORMATIONAL ONLY.
#
# THIS GATE DOES NOT GATE. It records numbers; it never fails the harness, and
# run.sh deliberately does not invoke it. It belongs in the privileged
# integration tier (integration.yml), not plain parity CI, and must NOT be run
# locally as part of a normal parity pass.
#
# Why it is informational: the Go DNS forwarder is not in the frozen tree (it
# lives in the external squawk-client-go package), and the Rust forwarder is a
# from-scratch reimplementation that adds a DNS answer cache Go never had, so
# QPS/RSS are expected to differ — there is no apples-to-apples Go oracle to
# gate against. See docs/PARITY.md §3 (completed-stub) and the M8 gap analysis
# dim 7.
#
# Intended methodology (integration tier, mock upstream — no real network):
#   1. Stand up a mock DoH upstream using squawk-client's own testutil server
#      (crates/squawk-client/.../testutil), so no query leaves the host.
#   2. Configure squawk with the forwarder enabled on a HIGH UDP port
#      (unprivileged — never :53) pointed at that mock upstream.
#   3. Fire a fixed number of DNS queries (a tight loop or dnsperf) and record
#      achieved QPS.
#   4. Sample the daemon's RSS via `ps -o rss=` after warm-up.
#   5. Print the numbers as an informational artifact; do not compare or gate.
#
# It is a stub until wired into the integration tier: it prints its plan and
# exits 0 unless PG_PERF_RUN=1 is set explicitly (which the gating harness
# never sets).

set -uo pipefail

echo "=== parity: perf (dim 7) — INFORMATIONAL, NON-GATING ==="

if [ "${PG_PERF_RUN:-0}" != "1" ]; then
    echo "perf: informational tier — not executed in a normal parity pass."
    echo "perf: set PG_PERF_RUN=1 in the integration tier to collect QPS/RSS numbers."
    echo "perf: PASS (nothing to gate)"
    exit 0
fi

echo "perf: PG_PERF_RUN=1 set, but the integration-tier load generator + mock"
echo "perf: DoH upstream wiring is intentionally left for integration.yml to"
echo "perf: provide. This stub still never gates — it exits 0 regardless."
exit 0
