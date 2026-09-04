# FleetDM Coexistence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Have the penguin agent recommend and coexist with FleetDM (MDM/osquery) without rebuilding any of it — detect `fleetd`/`osqueryd`, surface that in status + telemetry, and document the responsibility split. Self-protection must never touch FleetDM processes.

**Architecture:** A tiny detector in `penguin-selfprotect` (or a small shared util) probes for FleetDM binaries/processes behind a testable seam; the daemon surfaces the result in status and as an OTel resource attribute; docs describe the split. No policy engine, no FleetDM control.

**Tech Stack:** Rust 2024, std process/fs probing behind a trait seam; docs in Markdown.

**Spec:** `docs/superpowers/specs/2026-08-21-endpoint-self-protection-and-modules-design.md` (§4.4)

## Global Constraints

- Detect only — never start, stop, configure, or supervise FleetDM. Self-protection guards ONLY penguin's own PIDs (never `fleetd`/`osqueryd`).
- No new external crate unless already in lock. Every `pub` item documented. Coverage ≥90% on new code.

## File Structure

```
crates/penguin-selfprotect/src/fleetdm.rs   # FleetProbe trait + detect() -> FleetStatus
bins/penguind/src/daemon_main.rs            # add fleet_dm attribute to OTel resource + status
docs/fleetdm-coexistence.md                 # responsibility split + recommendation
```

---

### Task 1: FleetDM detector

**Files:**
- Create: `crates/penguin-selfprotect/src/fleetdm.rs`
- Test: `src/fleetdm.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `trait FleetProbe { fn binary_present(&self, name: &str) -> Option<String>; }` (returns version if found); `struct FleetStatus { fleetd: Option<String>, osqueryd: Option<String> }`; `fn detect(probe: &dyn FleetProbe) -> FleetStatus`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    struct Fake { has_fleetd: bool }
    impl FleetProbe for Fake {
        fn binary_present(&self, name: &str) -> Option<String> {
            if name == "fleetd" && self.has_fleetd { Some("1.30.0".into()) } else { None }
        }
    }
    #[test]
    fn detect_reports_present_and_absent() {
        let s = detect(&Fake { has_fleetd: true });
        assert_eq!(s.fleetd.as_deref(), Some("1.30.0"));
        assert!(s.osqueryd.is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-selfprotect fleetdm --locked`
Expected: FAIL — module/types not defined.

- [ ] **Step 3: Write minimal implementation**

`detect` calls `probe.binary_present("fleetd")` and `("osqueryd")`. The real probe checks `PATH` + well-known install dirs (`/opt/orbit/bin/`, `/usr/local/bin/`) and runs `--version`; kept behind the trait so tests never shell out.

- [ ] **Step 4: Run test to verify it passes** — PASS.
- [ ] **Step 5: Commit** `feat(fleetdm): detect fleetd/osqueryd presence behind a probe seam`.

---

### Task 2: Surface FleetDM status in telemetry + agent status

**Files:**
- Modify: `bins/penguind/src/daemon_main.rs` (add `fleet_dm.fleetd`/`fleet_dm.osqueryd` to the OTel `resource_attrs` and to the daemon status snapshot)
- Test: a unit test asserting the attribute set includes the detected values (using the fake probe)

**Interfaces:**
- Consumes: `detect` (Task 1), OtelPipeline resource attrs (OTel plan Task 4).

- [ ] **Step 1: Write the failing test** — build resource attrs with a fake probe reporting fleetd present; assert `("fleet_dm.fleetd","1.30.0")` is included.
- [ ] **Step 2: Run** — FAIL. **Step 3: Implement** the attribute wiring. **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(fleetdm): report coexistence in OTel resource + status`.

---

### Task 3: Coexistence documentation

**Files:**
- Create: `docs/fleetdm-coexistence.md`

- [ ] **Step 1:** Write the doc: (a) recommend deploying FleetDM alongside penguin for MDM/system monitoring/policies; (b) responsibility split table — FleetDM = MDM/inventory/policy/osquery; penguin = product modules + self-protection + OTel/threat reporting; (c) explicit note that penguin self-protection guards only its own PIDs and never interferes with `fleetd`/`osqueryd`; (d) pointer to the central chart (SP3) that installs both by default.
- [ ] **Step 2: Commit** `docs(fleetdm): coexistence + responsibility split`.

## Self-Review Notes
- Spec §4.4 coverage: detect = Task 1; surface in telemetry/status = Task 2; recommend + split + non-interference doc = Task 3. ✅
- Depends on the OTel plan (resource attrs) for Task 2; sequence FleetDM after OTel Task 4.
- No FleetDM control surface introduced (detect-only), per the constraint.
