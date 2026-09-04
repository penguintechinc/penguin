# Agent Self-Protection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the penguin agent resist casual removal — auto-restart if killed, detect + self-heal binary/unit/config tampering, and require authorization to uninstall (local admin+secret / offline break-glass token / central-console deauthorization) — while guaranteeing an authorized admin is never permanently locked out.

**Architecture:** A new `penguin-selfprotect` crate holds all platform-agnostic logic: a signed `IntegrityManifest` (+ `ManifestSource` trait), the integrity checker + self-heal, `TamperEvent` types, and the `TeardownAuthz` authorization decision. Platform-specific hardening lives in the embedded service units (`packaging/`) and `bins/penguind/src/service/*`. The watchdog is a `penguind watchdog` subcommand (a second process of the same binary) that mutually supervises the daemon. Signature verification reuses `penguin_update::verify`; the tamper secret is stored via `penguin_secrets::Store`. The whole subsystem arms only when the node is enrolled and the `penguin.self-protection` flag is on.

**Security boundary (non-negotiable):** legitimate EDR-style tamper resistance only. NO process/file hiding, NO disabling of AV/OS security tooling, NO anti-forensics, NEVER blocking OS root at the kernel. Break-glass token + console deauth are always-available authorized overrides.

**Tech Stack:** Rust 2024, Tokio, `sha2` (hashing), `penguin-update` (ed25519 verify), `penguin-secrets` (secret store), `serde`/`serde_json`, `thiserror`, `async-trait`.

**Spec:** `docs/superpowers/specs/2026-08-21-endpoint-self-protection-and-modules-design.md` (§4.1, §9)

## Global Constraints

- Edition 2024, rust 1.97, workspace version 0.2.0; inherit via `.workspace = true`.
- Reuse, do not reimplement: signature verification = `penguin_update::verify(data, sig_text, pubkey_text)`; secret storage = `penguin_secrets::Store` (`.namespaced("selfprotect")`).
- Tamper secret stored **hashed** (Argon2id via `argon2 = "=0.5.3"`, exact pin) — never plaintext; never logged (mask in all log lines).
- Behind PostHog flag `penguin.self-protection` (default OFF, cached offline). Unenrolled agent = unarmed.
- Uninstall path must ALWAYS surface the break-glass procedure in its refusal message. OS root is never blocked.
- Single service-unit source of truth: Linux edits `packaging/systemd/penguind.service`; macOS edits `packaging/launchd/io.penguintech.penguind.plist`. `penguind service install` already writes these verbatim.
- Coverage ≥90% on `penguin-selfprotect`. Every `pub` item documented.

## File Structure

```
crates/penguin-selfprotect/
  Cargo.toml
  src/lib.rs         # crate doc + re-exports
  src/manifest.rs    # IntegrityManifest, ManifestEntry, ManifestSource trait, LocalFileSource
  src/integrity.rs   # check() -> Vec<TamperFinding>; heal(finding, protected_dir)
  src/event.rs       # TamperEvent, TamperKind
  src/authz.rs       # TeardownAuthz, authorize(...), break-glass token verify, secret hash/verify
  src/state.rs       # ProtectionState (armed/disarmed), arming rules
  src/console.rs     # ConsoleSink trait + NoopConsoleSink (SP2 provides the HTTP impl)

bins/penguind/src/watchdog.rs         # `penguind watchdog` peer loop
bins/penguind/src/main.rs             # dispatch `watchdog` before run(); authorize uninstall
bins/penguind/src/service/mod.rs      # teardown-gate hook in handle_service_command (uninstall)
bins/penguind/src/service/linux.rs    # (unchanged verbs; hardening is in the unit file)
bins/penguind/src/daemon_main.rs      # arm subsystem when enrolled + flag on; spawn integrity loop; emit tamper events via telemetry
packaging/systemd/penguind.service    # hardening directives (Restart=always, StartLimitIntervalSec=0, ...)
packaging/launchd/io.penguintech.penguind.plist  # KeepAlive
Cargo.toml (root)                     # workspace.dependencies: penguin-selfprotect, argon2, sha2
```

---

### Task 1: `IntegrityManifest` + signed verification

**Files:**
- Create: `crates/penguin-selfprotect/Cargo.toml`, `src/lib.rs`, `src/manifest.rs`
- Modify: root `Cargo.toml` (`penguin-selfprotect`, `argon2 = "=0.5.3"`, `sha2` if not present)
- Test: `src/manifest.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `ManifestEntry { path: String, sha256: String, mode: u32 }`; `IntegrityManifest { version: u32, entries: Vec<ManifestEntry>, signature: String }`; `IntegrityManifest::verify_signature(&self, pubkey_text: &str) -> Result<(), SelfProtectError>` (canonicalizes entries to JSON bytes, calls `penguin_update::verify`); `ManifestSource` trait with `fn load(&self) -> Result<IntegrityManifest, SelfProtectError>` and `LocalFileSource { path }`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tampered_manifest_body_fails_signature() {
        // Fixture signed at test-setup time with a throwaway ed25519 key (see testdata helper).
        let (pubkey, mut manifest) = testfix::signed_manifest();
        assert!(manifest.verify_signature(&pubkey).is_ok());
        manifest.entries[0].sha256 = "0".repeat(64); // tamper the body, keep old signature
        assert!(matches!(manifest.verify_signature(&pubkey), Err(SelfProtectError::Signature)));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-selfprotect manifest --locked`
Expected: FAIL — crate/types not defined.

- [ ] **Step 3: Write minimal implementation**

Create the crate. `manifest.rs`: the structs (serde), a `canonical_bytes()` (serde_json over `{version, entries}` with sorted keys), and `verify_signature` calling `penguin_update::verify(self.canonical_bytes(), &self.signature, pubkey_text)`. `ManifestSource`/`LocalFileSource::load` reads+deserializes JSON. Add `testfix` under `#[cfg(test)]` producing a signed fixture (reuse whatever ed25519 signing helper `penguin-update`'s own tests use).

- [ ] **Step 4: Run test to verify it passes**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-selfprotect manifest --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/penguin-selfprotect Cargo.toml Cargo.lock
git commit -m "feat(selfprotect): signed IntegrityManifest + ManifestSource"
```

---

### Task 2: Integrity check detects tampering

**Files:**
- Create: `crates/penguin-selfprotect/src/integrity.rs`, `src/event.rs`
- Test: `src/integrity.rs` (`#[cfg(test)]`, real temp files via `tempfile`)

**Interfaces:**
- Consumes: `IntegrityManifest` (Task 1).
- Produces: `TamperKind { BinaryModified, UnitModified, ConfigModified, FileMissing }`; `TamperFinding { path: String, kind: TamperKind, expected_sha256: String, actual_sha256: Option<String> }`; `fn check(manifest: &IntegrityManifest, root: &Path) -> Vec<TamperFinding>` (hashes each entry's file under `root`, compares; missing file → `FileMissing`).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn check_flags_modified_and_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("penguind");
    std::fs::write(&good, b"real-binary").unwrap();
    let manifest = testfix::manifest_for(&[("penguind", &sha256_hex(b"real-binary")), ("missing.conf", &sha256_hex(b"x"))]);
    // penguind present but we now corrupt it; missing.conf never created.
    std::fs::write(&good, b"corrupted").unwrap();
    let findings = check(&manifest, dir.path());
    assert!(findings.iter().any(|f| f.path == "penguind" && f.kind == TamperKind::BinaryModified));
    assert!(findings.iter().any(|f| f.path == "missing.conf" && f.kind == TamperKind::FileMissing));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `PATH=~/.cargo/bin:$PATH cargo test -p penguin-selfprotect integrity --locked`
Expected: FAIL — `check`/types not defined.

- [ ] **Step 3: Write minimal implementation**

`check` iterates entries, hashes the file with sha2, classifies (`.service`→UnitModified, ends-with binary name→BinaryModified, else ConfigModified; absent→FileMissing). No I/O panic — a read error becomes a `FileMissing`/warn.

- [ ] **Step 4: Run test to verify it passes** — PASS.
- [ ] **Step 5: Commit** `feat(selfprotect): integrity check detects modified/missing files`.

---

### Task 3: Self-heal from a protected copy

**Files:**
- Modify: `crates/penguin-selfprotect/src/integrity.rs`
- Test: `src/integrity.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `fn heal(finding: &TamperFinding, protected_dir: &Path, target_root: &Path) -> Result<(), SelfProtectError>` — copies the protected pristine copy back over the tampered/missing path, restoring `mode`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn heal_restores_a_corrupted_file_from_protected_copy() {
    let target = tempfile::tempdir().unwrap();
    let protected = tempfile::tempdir().unwrap();
    std::fs::write(protected.path().join("penguind"), b"real-binary").unwrap();
    std::fs::write(target.path().join("penguind"), b"corrupted").unwrap();
    let finding = TamperFinding { path: "penguind".into(), kind: TamperKind::BinaryModified,
        expected_sha256: sha256_hex(b"real-binary"), actual_sha256: Some(sha256_hex(b"corrupted")) };
    heal(&finding, protected.path(), target.path()).unwrap();
    assert_eq!(std::fs::read(target.path().join("penguind")).unwrap(), b"real-binary");
}
```

- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** `heal` (copy protected→target, set mode; error if protected copy missing).
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(selfprotect): self-heal restores files from protected copy`.

---

### Task 4: Tamper-protection secret — hash + verify

**Files:**
- Create: `crates/penguin-selfprotect/src/authz.rs`
- Test: `src/authz.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `fn hash_secret(plain: &str) -> Result<String, SelfProtectError>` (Argon2id PHC string); `fn verify_secret(plain: &str, phc: &str) -> bool` (constant-time; never logs the plaintext).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn secret_verifies_against_its_hash_and_rejects_wrong() {
    let phc = hash_secret("correct horse").unwrap();
    assert!(verify_secret("correct horse", &phc));
    assert!(!verify_secret("Tr0ub4dor", &phc));
    assert_ne!(phc, "correct horse", "stored form is a hash, never the plaintext");
}
```

- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** with `argon2::Argon2` default params; PHC encode/verify.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(selfprotect): argon2 tamper-secret hash/verify`.

---

### Task 5: Break-glass token + `TeardownAuthz` decision

**Files:**
- Modify: `crates/penguin-selfprotect/src/authz.rs`, `src/state.rs`, `src/console.rs`
- Test: `src/authz.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `enum TeardownAuthz { NodeDeauthorized, LocalSecret, BreakGlassToken, Unauthorized }`; `fn verify_break_glass(token: &str, node_id: &str, pubkey_text: &str) -> bool` (a `penguin_update::verify`-checked, node-bound, signed token); `fn authorize(input: &TeardownInput, ctx: &TeardownCtx) -> TeardownAuthz` where `TeardownInput { secret: Option<String>, break_glass: Option<String> }` and `TeardownCtx { is_root: bool, secret_phc: Option<String>, node_id: String, pubkey: String, console_deauthorized: bool }`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn authorize_accepts_each_valid_path_and_refuses_otherwise() {
    let ctx = TeardownCtx { is_root: true, secret_phc: Some(hash_secret("s3cret").unwrap()),
        node_id: "n-1".into(), pubkey: testfix::pubkey(), console_deauthorized: false };
    // wrong/no creds while armed → Unauthorized
    assert_eq!(authorize(&TeardownInput { secret: None, break_glass: None }, &ctx), TeardownAuthz::Unauthorized);
    // correct local secret → LocalSecret
    assert_eq!(authorize(&TeardownInput { secret: Some("s3cret".into()), break_glass: None }, &ctx), TeardownAuthz::LocalSecret);
    // valid break-glass token → BreakGlassToken
    let tok = testfix::sign_break_glass("n-1");
    assert_eq!(authorize(&TeardownInput { secret: None, break_glass: Some(tok) }, &ctx), TeardownAuthz::BreakGlassToken);
    // console said remove → NodeDeauthorized even with no local creds
    let mut ctx2 = ctx.clone(); ctx2.console_deauthorized = true;
    assert_eq!(authorize(&TeardownInput { secret: None, break_glass: None }, &ctx2), TeardownAuthz::NodeDeauthorized);
}
```

- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** `authorize` precedence: console_deauthorized → NodeDeauthorized; valid break_glass → BreakGlassToken; matching secret → LocalSecret; else Unauthorized. `verify_break_glass` verifies `"<node_id>"` payload signature via `penguin_update::verify`. `is_root` gates the local paths (non-root local uninstall is always Unauthorized regardless of secret — OS still lets root remove files, but our service uninstall verb refuses).
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(selfprotect): teardown authorization + break-glass token`.

---

### Task 6: Arming rules (enrolled + flag) — `ProtectionState`

**Files:**
- Modify: `crates/penguin-selfprotect/src/state.rs`
- Test: `src/state.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `fn is_armed(enrolled: bool, flag_on: bool) -> bool` (armed iff both true); `ProtectionState` holding the resolved manifest + secret_phc + node_id when armed.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn armed_only_when_enrolled_and_flag_on() {
    assert!(is_armed(true, true));
    assert!(!is_armed(false, true));  // unenrolled/dev agent stays unarmed
    assert!(!is_armed(true, false));  // flag off
}
```

- [ ] **Step 2: Run** — FAIL. **Step 3: Implement** `is_armed`. **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(selfprotect): arm only when enrolled and flag on`.

---

### Task 7: Service hardening (Linux + macOS unit files)

**Files:**
- Modify: `packaging/systemd/penguind.service`, `packaging/launchd/io.penguintech.penguind.plist`
- Test: `bins/penguind/src/service/mod.rs` (`#[cfg(test)]`, assert on the embedded `SYSTEMD_UNIT`/`LAUNCHD_PLIST` constants)

**Interfaces:** none (declarative units; consumed verbatim by `install`).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn systemd_unit_is_hardened_for_auto_restart() {
    let unit = super::SYSTEMD_UNIT;
    assert!(unit.contains("Restart=always"));
    assert!(unit.contains("StartLimitIntervalSec=0")); // never give up restarting
    assert!(unit.contains("RestartSec="));
}
#[cfg(target_os = "macos")]
#[test]
fn launchd_plist_keepalive_true() {
    assert!(super::LAUNCHD_PLIST.contains("KeepAlive"));
}
```

- [ ] **Step 2: Run** — FAIL (directives absent).
- [ ] **Step 3: Implement** — add `Restart=always`, `RestartSec=2`, `StartLimitIntervalSec=0` to `[Service]`; keep admin `systemctl stop` working (do NOT add anything that blocks manual stop). Add `<key>KeepAlive</key><true/>` + `RunAtLoad` to the plist. Keep existing hardening directives.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(selfprotect): harden systemd/launchd units for auto-restart`.

---

### Task 8: Teardown gate in `handle_service_command` (uninstall)

**Files:**
- Modify: `bins/penguind/src/service/mod.rs` (intercept the `uninstall` verb), `bins/penguind/src/main.rs` (parse `--auth`/`--break-glass` flags), `bins/penguind/Cargo.toml` (`penguin-selfprotect`, `penguin-secrets`)
- Test: `bins/penguind/src/service/mod.rs` (`#[cfg(test)]`, `FakeServiceHost`)

**Interfaces:**
- Consumes: `penguin_selfprotect::authorize`, `is_armed`; the `FakeServiceHost` seam already present.
- Produces: `uninstall` calls `host.uninstall()` only when armed→authorized OR unarmed; on `Unauthorized`, returns an error string that names the break-glass procedure and does NOT call `host.uninstall()`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn armed_uninstall_without_auth_is_refused_and_does_not_uninstall() {
    let host = FakeServiceHost::new();
    let ctx = test_armed_ctx(); // armed, secret set, not console-deauthorized
    let res = handle_service_command_with_ctx(&["service".into(), "uninstall".into()], &host, &ctx);
    assert!(res.unwrap().unwrap_err().contains("break-glass"));
    assert_eq!(host.uninstall_calls(), 0);
}
#[test]
fn armed_uninstall_with_correct_secret_proceeds() {
    let host = FakeServiceHost::new();
    let ctx = test_armed_ctx();
    let res = handle_service_command_with_ctx(&["service".into(), "uninstall".into(), "--auth".into(), "s3cret".into()], &host, &ctx);
    assert!(res.unwrap().is_ok());
    assert_eq!(host.uninstall_calls(), 1);
}
```

- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** — thread a `TeardownCtx` (built from `penguin_secrets` secret + enrollment/flag state) into the uninstall branch; call `authorize`; refuse or proceed. Keep the existing non-uninstall verbs untouched. Refusal message: `"uninstall refused: this endpoint is tamper-protected. Provide --auth <secret>, a --break-glass <token>, or deauthorize the node in the Penguin console. Break-glass: <docs path>."`
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(selfprotect): authorize service uninstall (secret/break-glass/console)`.

---

### Task 9: `penguind watchdog` mutual supervision

**Files:**
- Create: `bins/penguind/src/watchdog.rs`
- Modify: `bins/penguind/src/main.rs` (dispatch `watchdog` before `run()`), `bins/penguind/src/daemon_main.rs` (spawn/adopt the watchdog peer + heartbeat)
- Test: `bins/penguind/src/watchdog.rs` (`#[cfg(test)]`, fake process-control seam)

**Interfaces:**
- Produces: `WatchTarget` trait (`is_alive(&self) -> bool`, `relaunch(&self) -> io::Result<()>`); `fn supervise_once(target: &dyn WatchTarget) -> WatchAction` returning `Relaunched`/`Alive`; the real target launches `penguind`/`penguind watchdog` respectively.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn supervise_relaunches_a_dead_peer() {
    let dead = FakeTarget::dead();
    assert_eq!(supervise_once(&dead), WatchAction::Relaunched);
    assert_eq!(dead.relaunch_calls(), 1);
    let alive = FakeTarget::alive();
    assert_eq!(supervise_once(&alive), WatchAction::Alive);
    assert_eq!(alive.relaunch_calls(), 0);
}
```

- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** `supervise_once` (check `is_alive`; if not, `relaunch` and return `Relaunched`). Wire `main.rs`: `if args.first() == Some("watchdog") { return watchdog::run_watchdog(); }` before `run()`. `run_watchdog` loops `supervise_once` on a real target (pidfile of the daemon) on an interval; the daemon spawns/adopts the watchdog process and supervises it symmetrically, bounded by the existing backoff formula to prevent respawn storms.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(selfprotect): penguind watchdog mutual supervision`.

---

### Task 10: Daemon arming + integrity loop + tamper reporting

**Files:**
- Modify: `bins/penguind/src/daemon_main.rs`, `crates/penguin-selfprotect/src/console.rs`
- Test: an integration test under `crates/penguin-selfprotect/tests/` driving a full check→heal→report cycle with fakes.

**Interfaces:**
- Consumes: `is_armed`, `ManifestSource`, `check`, `heal`, `TamperEvent`, `ConsoleSink`, daemon telemetry handle (OTel plan).
- Produces: at daemon startup, if `is_armed(enrolled, feature_enabled("penguin.self-protection"))`, spawn an interval task that loads the manifest (via `LocalFileSource` now; `ConsoleSink`/server source in SP2), runs `check`, `heal`s findings, and emits a `TamperEvent` through the daemon telemetry handle + `ConsoleSink` (NoopConsoleSink today). Never crash the daemon on any error.

- [ ] **Step 1: Write the failing test** — a `tests/arm_cycle.rs` that, with a fake manifest source + fake sink + corrupted temp file, asserts one loop iteration heals the file and pushes exactly one `TamperEvent` of kind `BinaryModified` to the sink.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** the loop + `ConsoleSink`/`NoopConsoleSink`; wire into `daemon_main`.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(selfprotect): daemon arms integrity loop + emits tamper events`.

---

### Task 11: (Optional / may split) Windows service recovery + watchdog

**Files:** `bins/penguind/src/service/windows.rs`, packaging for the Windows service.

> DECISION NEEDED (spec §10): Windows service-manager integration is currently out-of-scope in `daemon_main`. Adding SCM auto-restart recovery actions + the watchdog on Windows is the largest new surface. Recommend landing Linux/macOS self-protection first (Tasks 1–10) and doing Windows as a follow-up branch. If included: set `FailureActions` (restart) via `sc.exe failure` in `windows::RealServiceHost`, and gate the watchdog target's process control behind `#[cfg(windows)]`.

- [ ] Deferred pending the scope decision. If approved now, mirror Tasks 7–9 for Windows with an SCM-recovery test on a `FakeServiceHost`.

---

### Task 12: Full gate

- [ ] **Step 1:** `PATH=~/.cargo/bin:$PATH cargo check --workspace --locked && cargo test -p penguin-selfprotect -p penguind --locked` → PASS.
- [ ] **Step 2:** `cargo fmt --all --check && cargo clippy -p penguin-selfprotect -- -D warnings`; `cargo llvm-cov -p penguin-selfprotect --summary-only` ≥90%.
- [ ] **Step 3:** Verify no secret/token is ever logged (grep the crate for log calls near secret handling).
- [ ] **Step 4: Commit** `chore(selfprotect): gate green`.

## Self-Review Notes
- Spec §4.1 coverage: service hardening = Task 7; watchdog = Task 9; integrity+self-heal = Tasks 1–3,10; authorized teardown (local secret / break-glass / console) = Tasks 4,5,8; armed-only-when-enrolled = Task 6; §9 security boundary honored (no hiding/AV-tamper; root never blocked; break-glass always surfaced in Task 8). ✅
- Types consistent: `TamperKind`/`TamperFinding`/`TamperEvent`, `TeardownAuthz`/`TeardownInput`/`TeardownCtx`, `IntegrityManifest`/`ManifestEntry`/`ManifestSource`, `ConsoleSink`/`NoopConsoleSink` used identically across tasks.
- Reuse verified: `penguin_update::verify` (Tasks 1,5), `penguin_secrets::Store` (Task 8). Console path + server manifest source are SP2; SP1 uses `LocalFileSource` + `NoopConsoleSink` (documented).
- OPEN: Windows scope (Task 11) — flagged for the user, not silently dropped.
