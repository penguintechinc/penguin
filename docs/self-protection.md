# Agent Self-Protection

Operator runbook for `penguind`'s self-protection subsystem (SP1). Covers what
it does, when it's active, and — most importantly — how an authorized admin
recovers a protected node.

## What this is

Self-protection is **legitimate, admin-authorized tamper resistance** for the
endpoint agent. It exists so that malware, a confused script, or an
unauthorized local user cannot silently disable or remove the agent on a
managed endpoint. It is composed of four pieces, all shipped in this repo:

1. **Auto-restart via hardened service units.** The systemd unit
   (`packaging/systemd/penguind.service`) and launchd plist
   (`packaging/launchd/io.penguintech.penguind.plist`) that `penguind service
   install` writes set `Restart=always` / `RestartSec=2` /
   `StartLimitIntervalSec=0`, so the daemon always comes back after a crash
   or a kill, never blocked by systemd's default start-limit backoff. The
   unit is also otherwise hardened (`ProtectSystem=strict`, capability
   bounding set limited to `CAP_NET_ADMIN`/`CAP_NET_BIND_SERVICE`,
   `NoNewPrivileges=true`, etc.) — see that file for the full,
   directive-by-directive rationale.
2. **Mutual watchdog.** `daemon_main::run_daemon` spawns a `penguind
   watchdog` child at startup; the watchdog checks the daemon's liveness on
   a fixed interval and relaunches it if it's gone, and the daemon does the
   same for the watchdog. Killing either process alone does not stop the
   agent — the survivor relaunches its peer within one supervision tick.
   Source: `bins/penguind/src/watchdog.rs`.
3. **Signed-manifest integrity monitoring with self-heal.** The armed
   daemon periodically loads a controller-signed [`IntegrityManifest`],
   verifies its signature, hashes the binary/unit/config files it lists,
   and restores (`heal`) any file that's missing or doesn't match from a
   protected reference copy. Source: `crates/penguin-selfprotect/` (see
   `manifest.rs`, `integrity.rs`, `monitor.rs`).
4. **Authorized-uninstall gate.** `penguind service uninstall` on an armed
   node refuses to proceed unless the request is authorized by one of three
   paths — see [Authorized teardown paths](#authorized-teardown-paths)
   below. Source: `bins/penguind/src/service/mod.rs`.

### What this explicitly is NOT

- **Not process or file hiding.** The agent's process, binary, and files are
  fully visible to the OS, to `ps`/`Task Manager`/EDR tools, and to the
  admin. Self-protection makes the agent hard to *tamper with*, not hard to
  *see*.
- **Not AV/OS-defense tampering.** Self-protection never disables, evades,
  or interferes with antivirus, EDR, or other OS security controls. It
  protects only the agent's own binary, config, and service registration.
- **Never a permanent lockout.** An authorized admin always has a way back
  in — see [Break-glass recovery](#break-glass-recovery-procedure). This is
  a hard design invariant, not a best-effort goal.

## Arming: when self-protection is actually active

Self-protection is armed only when **both** of the following are true:

- The node is **enrolled** (has a tamper-protection secret provisioned in
  the local secret store — see `penguin_selfprotect::is_armed` and
  `bins/penguind/src/service/mod.rs`'s `resolve_teardown_ctx`, which
  documents the interim enrollment proxy used until SP2 ships real
  enrollment state)
- The `penguin.self-protection` PostHog feature flag is on

A **fresh or dev agent is unarmed**: no tamper-protection secret has been
provisioned, so `service install`/`uninstall`/`stop` behave exactly as they
did before self-protection existed — no operational friction for local
development or a not-yet-enrolled install. Both conditions are checked
independently so a single point of failure (e.g. a flag flip) can never be
the sole thing standing between "protected" and "unprotected" — see
`crates/penguin-selfprotect/src/state.rs`'s `is_armed`.

## Authorized teardown paths

On an **armed** node, `penguind service uninstall` refuses by default. It
proceeds only when one of three authorized paths clears it (evaluated by
`penguin_selfprotect::authorize`, in precedence order — see
`crates/penguin-selfprotect/src/authz.rs`):

| # | Path | Command / mechanism | Status |
|---|---|---|---|
| 1 | **Local secret** | `penguind service uninstall --auth <secret>` | Shipping in SP1 |
| 2 | **Break-glass token** | `penguind service uninstall --break-glass <token>` | Shipping in SP1; **inert until SP2 provisions the signing key** (see below) |
| 3 | **Console deauthorization** | Node deauthorized from the Penguin console | **Coming with SP2** (the central console) |

1. **Local secret** (`--auth <secret>`) — the tamper-protection secret set
   at enroll time. Stored only as an Argon2id PHC hash
   (`penguin_selfprotect::hash_secret`/`verify_secret`), never in plaintext,
   never logged. Requires the caller to be root; a non-root caller cannot
   use this path regardless of whether the secret is correct.
2. **Break-glass token** (`--break-glass <token>`) — an offline,
   node-bound, Penguin-signed recovery token (a minisign signature over
   this node's hostname). Verified via
   `penguin_selfprotect::verify_break_glass`, independent of whether the
   local secret is known or the console is reachable — the guaranteed
   emergency path. **The break-glass public key is provisioned by the
   central console (SP2).** Until then, `BREAK_GLASS_PUBKEY` in
   `bins/penguind/src/service/mod.rs` is an empty string, which never
   parses as a valid key — so a `--break-glass` token is accepted syntactically
   but **never actually verifies today**, meaning this path is currently
   inert. See [Break-glass recovery](#break-glass-recovery-procedure) for
   what changes once SP2 ships the key.
3. **Console deauthorization** — an admin marks the node deauthorized
   (removal approved) from the Penguin console. This wins over every other
   path, even with no local credentials presented at all. **Not implemented
   in SP1** — `resolve_teardown_ctx` hardcodes `console_deauthorized:
   false`, and `ConsoleSink`'s only implementation today,
   `NoopConsoleSink`, always answers `poll_deauthorized() == false` (see
   `crates/penguin-selfprotect/src/console.rs`). This path activates when
   SP2's console ships a real `ConsoleSink`.

An **authorized `systemctl stop` still stops the service** — self-protection
gates `penguind service uninstall` (removal), not `stop`. Stopping the
service is not the operation this subsystem defends against, and the mutual
watchdog does not fight a deliberately-issued `systemctl stop`/`uninstall`:
systemd's default `KillMode=control-group` signals the whole service
cgroup, including the watchdog child, so both processes exit together
rather than the watchdog trying to resurrect a stop it didn't initiate (see
`bins/penguind/src/watchdog.rs`'s module doc).

If none of the three paths authorize the request, `uninstall` refuses with:

```
uninstall refused: this endpoint is tamper-protected. Provide --auth <secret>,
a --break-glass <token>, or deauthorize the node in the Penguin console.
Break-glass recovery: docs/self-protection.md.
```

## Break-glass recovery procedure

This is the guaranteed override that keeps a lost local secret, or an admin
who can no longer authenticate locally, from becoming a permanent lockout.

**Today (SP1, no console yet):**

- The break-glass *mechanism* (node-bound minisign token verification) is
  fully implemented and tested in `penguin_selfprotect::verify_break_glass`.
- The break-glass *public key* that would let a real token verify is not yet
  provisioned — `BREAK_GLASS_PUBKEY` is empty. This means `--break-glass`
  cannot be used to recover an SP1-armed node today.
- **If you lose the local secret on an SP1-armed node before SP2 ships**,
  recovery is via direct OS-level access (see "OS root is never blocked"
  below) — the self-protection subsystem's own gate cannot be satisfied by
  the break-glass path yet.

**Once SP2 (central console) ships:**

1. From the Penguin console, request a break-glass token for the specific
   node (identified by hostname / node ID). The console signs a token over
   that exact node ID with the trusted break-glass private key — this
   binds the token to one node; it will not verify against any other node.
2. Deliver the token to whoever has physical/console access to the
   endpoint (it is designed to be usable fully offline — no network call
   from the endpoint itself is required to verify it).
3. Run:
   ```
   penguind service uninstall --break-glass <token>
   ```
4. `authorize()` verifies the token against the node's hostname and the
   now-provisioned public key and, on success, proceeds with the uninstall
   unconditionally — no local secret needed.

**OS root is never blocked at the kernel level, in either case.** Self-
protection gates this service's own `uninstall` verb and the mutual
watchdog's relaunch behavior — it does not, and cannot, prevent a root user
from stopping the process, removing files, or disabling the systemd unit
directly via the OS. `TeardownCtx::is_root` gates only the *local-secret*
path's convenience; it is not a restriction on what root itself can do
outside this tool. Self-protection raises the bar against casual/automated
tampering; it is not a kernel-level anti-tamper mechanism and does not
claim to be.

## Deliberate fail-open / fail-closed behaviors

Two behaviors below look asymmetric on purpose — they are documented
design decisions, not oversights:

- **Secret store unreadable → resolves to unarmed (fails toward
  availability).** If the local secret store can't be read (missing,
  corrupted, permission error), the node is treated as *not enrolled* and
  therefore unarmed — `service` verbs proceed normally. Rationale: a
  storage failure must never turn into an agent that can't be managed at
  all; availability wins over protection in this specific failure mode.
- **Integrity manifest missing or its signature doesn't verify → the
  monitor takes NO action (fails closed on tampering).** If
  `scan_heal_report` can't load the manifest, or `verify_signature` fails
  against it, the cycle logs a warning and returns immediately — it never
  hashes files, never heals, never reports a tamper event based on
  unverified data. Rationale: the integrity monitor must never act on data
  it cannot trust; an attacker who can swap in a bogus manifest must not be
  able to make the monitor "heal" a file *to* attacker-controlled content.
  See `crates/penguin-selfprotect/src/monitor.rs`'s `scan_heal_report` doc
  ("Trust boundary") for the implementation.

These are opposite defaults for a reason: losing read access to your own
secret store is an operational failure that should never brick management;
an unverifiable integrity manifest is exactly the kind of input tampering
this subsystem exists to be suspicious of.

## What SP1 ships vs. what's coming with SP2 (central console)

| Piece | SP1 (this repo, today) | SP2 (central console) |
|---|---|---|
| Auto-restart hardening (systemd/launchd) | Shipped | — |
| Mutual watchdog | Shipped | — |
| Signed-manifest integrity check + self-heal | Shipped (`LocalFileSource` manifest loading) | Server-fetched manifest source |
| Local-secret teardown (`--auth`) | Shipped | — |
| Break-glass token verification | Shipped (mechanism) | **Signing key provisioning** — makes `--break-glass` usable |
| Console-recorded deauthorization | Stubbed (`NoopConsoleSink`, always `false`) | Real `ConsoleSink` + console UI |
| Tamper event reporting to console | Stubbed (`NoopConsoleSink::report_tamper` no-ops) | Real reporting |
| Windows support | Out of scope (flagged, not silently dropped — see plan notes) | TBD |

## Reference: source locations

- `crates/penguin-selfprotect/` — manifest verification, integrity check/heal,
  teardown authorization, arming logic (platform-independent library crate)
- `bins/penguind/src/service/mod.rs` — `penguind service` CLI dispatch,
  including the `uninstall` gate and refusal message
- `bins/penguind/src/watchdog.rs` — mutual watchdog
- `bins/penguind/src/daemon_main.rs` — daemon-side wiring: arming decision,
  spawning the integrity loop, shutdown ordering
- `packaging/systemd/penguind.service` / `packaging/launchd/io.penguintech.penguind.plist`
  — the hardened service unit definitions
