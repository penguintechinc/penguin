# Parity with the frozen Go client

The Rust agent targets **100% feature parity** with the Go implementation frozen
at [`go-client/`](../go-client/), *plus* completion of behaviours Go left
stubbed. This document records every place the Rust build deliberately does
**not** match Go, and why.

It is written as we go rather than reconstructed at the end, because the reason
for a divergence is obvious the day it is made and archaeology a month later.

**Rules for this file**

- Every entry is a *decision*, not drift. If Rust behaves differently and it is
  not listed here, that is a bug in Rust.
- "Bug-compatible" entries are as important as fixes: they record places we
  deliberately reproduced Go's wrong behaviour to avoid breaking a caller.
- Each entry names the milestone that introduced it.

---

## 1. Fixes — Go behaviour is broken and Rust does the right thing

### 1.1 The event broker was a dead end (M2)

Go constructs **two** `EventBroker` instances: the host factory publishes module
events into one, while `WatchEvents` subscribes to another that nothing ever
publishes to. `NewServer` takes no broker parameter at all, so the two are never
unified.

**Effect in Go:** every module state-change, error, and restart event is
unreachable from any `WatchEvents` subscriber. `penguin watch` shows nothing,
ever. This is not partial degradation — the feature does not work.

**Rust:** one broker is constructed once and handed to both the gRPC service and
every module's `HostServices::events()`. A regression test asserts a
module-published event reaches a `WatchEvents` subscriber.

Because Go's event stream never functioned, there is no observable behaviour to
break — which is what makes the next three fixes safe as well.

### 1.2 `running` was published before the module was running (M2)

Go publishes `StateChanged -> running` *before* calling `Start()`. A module whose
`Start` fails therefore emits a spurious `running` immediately followed by a
failure.

**Rust:** `running` is published only after `start()` returns successfully.

### 1.3 Load-failure events were asymmetric (M2)

Go emits `EventError` **and** `StateChanged -> failed` when `Start` fails, but
only `EventError` when `Init` fails — and in both cases the module ends up not
loaded, so a subsequent `list()` reports it as `disabled`. The `failed` event
therefore describes a state the module is not in.

**Rust:** both paths emit `EventError` followed by `StateChanged -> disabled`,
which is what actually happened: a failed load leaves the module unloaded and
retryable. A `failed` entry is stored only when a *loaded* module later dies.

### 1.4 The restart budget never reset (M2)

Go never clears `restartAttempt`, so `MaxRestarts` is a lifetime total for the
whole daemon process rather than a consecutive-failure count. A module that
restarted once successfully and then failed again months later counted that as
attempt 2 of 5.

**Rust:** the attempt counter resets once a module has run continuously for
`stability_window` (default 60s). A module that ran fine gets a fresh budget; a
flapping module still burns through `MAX_RESTARTS`.

### 1.5 Crash detection did not exist (M2)

`ReportFailure` is exported and fully implemented in Go, with backoff, jitter,
and a restart scheduler behind it — but **nothing ever calls it**. There is no
health-poll loop and no crash hook. The entire restart machine is unreachable in
production; only tests invoke it.

**Rust:** the supervisor runs a per-module health-poll task (default every 10s):
`Unhealthy` drives the failure/restart path, `Degraded` marks state without
restarting, and a return to `Healthy` restores `running` from `degraded`.

This is the largest single behavioural gap in the port — Go's supervisor could
not actually supervise.

### 1.6 Shutdown order contradicted its own comment (M2)

Go's comment says modules stop in "reverse load order (LIFO)"; the code sorts
names descending **alphabetically**.

**Rust:** true reverse load order.

### 1.7 The single-instance lock was never released (M2)

Go captures a release function and deliberately never calls it, relying on
process exit to drop the flock.

**Rust:** a `LockGuard` releases on `Drop`. The lock *file* is still left in
place, matching flock daemon convention.

### 1.8 "Is the daemon running?" almost never fired (M2)

Go's CLI uses `grpc.NewClient`, which connects lazily. The dial therefore
succeeds even when no daemon exists, and the friendly
`cannot reach penguind at %s — is the daemon running?` message is bypassed; the
real failure surfaces later as an opaque error on the first RPC.

**Rust:** the client connects eagerly, so a dead daemon is detected at connect
time and reports the intended message.

### 1.9 The configured log level was never applied to the daemon's own logger (M2)

Go's `cmd/penguind/main.go` initialises telemetry with a hardcoded `"info"`
*before* loading `config.yaml`, and never re-initialises it afterwards. The
operator-set `logLevel` therefore applies to nothing — setting `logLevel: debug`
silently does nothing for the daemon's own logs.

**Rust:** config is loaded first, and telemetry is initialised from its
`logLevel`.

### 1.10 The CLI sent an invalid HTTP/2 `:authority` (M2)

`internal/ipc/dial_unix.go` dialled `passthrough:///<socket-path>` without
`grpc.WithAuthority`, so grpc-go derived the `:authority` pseudo-header from the
target — the **URL-escaped socket path**, e.g.
`%2Frun%2Fpenguin%2Fpenguind.sock`. That is not a valid RFC 3986 authority.

grpc-go's own server accepts it, so the bug was invisible for as long as both
ends were Go. A spec-strict HTTP/2 server rejects the stream with
`PROTOCOL_ERROR` **before the request reaches any handler** — no interceptor,
middleware, or application code ever sees it.

This is the one change made to the frozen Go tree's production code
(`grpc.WithAuthority("localhost")`, one line, in dialling code only). The
alternative was losing the cross-implementation gate entirely, and the fix does
not touch the daemon contract that gate exists to test. Note the shipped product
is unaffected either way: production is the Rust CLI talking to the Rust daemon.

### 1.11 The CLI silently swallows RPC errors (M2)

Several verbs (`version`, `logs`, and the dynamic command-discovery call) check
the error returned by an RPC, discard it, and carry on — exiting 0 having
printed nothing useful.

Consequence: while every RPC was being rejected at the HTTP/2 layer (§1.10),
those commands still *looked* like they worked. The wire-compat harness was
initially fooled by exactly this and reported false passes.

The harness now requires non-empty output free of transport-error markers for
every success check, and cross-checks the `Version` RPC's result against the
daemon binary's own reported version. **Never assert on a CLI's exit status
alone when that CLI is known to swallow errors.**

### 1.12 `penguin logs --follow` silently died after 30 seconds (M4)

The Go CLI creates one 30-second context and reuses it for the entire `TailLogs`
call — *including* the follow loop. A `--follow` session therefore stops after 30
seconds. Because the same code swallows the resulting error (§1.11), it stops
**silently**: no message, exit 0, as if the log had simply ended.

**Rust:** the timeout bounds only establishing the stream; the receive loop then
runs until the user stops it or the daemon goes away.

### 1.13 A flag with an unrecognised type vanished (M4)

The Go CLI's flag builder switches on `FlagSpec.type` with **no default arm**, so
a module declaring a flag whose type is not `string`/`bool`/`int` has that flag
silently dropped from the command tree — no warning, no error, the flag simply
does not exist. Its own test never asserts the unknown-type flag is present, so
the gap was never visible.

**Rust:** an unrecognised type falls back to `string`, matching the precedent
already set by `penguin_sdk::command::FlagType::parse`.

### 1.14 The friendly daemon-down message only fired for static verbs (M4)

Go's `friendly()` wrapper applies to the built-in verbs, but an error-wrapping
quirk means module dispatch instead surfaces a bare `daemon unreachable` with no
socket path — the least useful message in exactly the case where the user most
needs to know which socket was tried.

**Rust:** the same
`cannot reach penguind at %s — is the daemon running?` message everywhere.

### 1.15 Enabling the DNS forwarder deadlocked the daemon (M5)

`forwarder.Start(ctx)` binds its listeners and then **blocks** on a select that
only returns when the context is cancelled. The squawk module calls it
*synchronously from `Module.Start`, while holding its own mutex*, and the
supervisor passes a module-lifetime context that is cancelled only on unload.

So with `forwarder.enabled: true`:
`Supervisor::load` never returns, and `Stop`/`Unload` then block forever trying
to take the mutex `Start` still holds — which is also what would have cancelled
the context and unblocked it. A complete deadlock, no timeout, on the module's
primary feature.

The Go test bounds it with a 5-second context and asserts only that `Start`
"completes without panic" — it never asserts that `Start` returns *promptly*,
so the hang reads as a pass.

**Rust:** `start()` binds the sockets synchronously — so a bind failure still
surfaces immediately and honestly — then spawns the serve loop as a background
task owned by the module. `stop()` signals it with a cancellation token.
`start()` never blocks.

### 1.16 `squawk license status` could never work (M5)

`handleLicense` builds a license config with `ValidateOnline: true` but never
sets `ServerURL`, and `ModuleConfig` has no field to supply one. Every call
therefore requests a schemeless relative URL and fails before any network I/O.
The command always reports an error — and always exits **0**, so a caller cannot
detect it from the exit status.

**Rust:** a real `license.server_url` config field (defaulting to squawk's own
`https://license.squawkdns.com`), and a non-zero exit code when validation
fails.

Note these are **two unrelated licensing systems**, easily conflated:
`HostServices::license()` is PenguinTech's entitlement service, while squawk's
own validator talks to squawkdns.com about Squawk product keys. The module's
`license` command uses only the latter; `LicenseFeature` is deliberately empty
because Squawk is core and must load with no license server at all.

### 1.17 A corrupt DNS backup marker wedged recovery permanently (M5)

`RecoverFromCrash` treats an unparseable marker exactly like a missing one:
returns `nil`, logs nothing above debug, and **never deletes or quarantines the
file**. Every subsequent daemon start hits the same silent no-op, so the host's
DNS is never restored and the operator is never told. A test asserts this
"no error" behaviour as correct.

**Rust:** a corrupt marker is a loud warning and gets quarantined (renamed), so
recovery is attempted once and the evidence is preserved rather than the failure
repeating in silence forever.

### 1.18 "0600" backup files were often not 0600 (M5)

Go writes its DNS backups with `os.WriteFile(path, data, 0o600)`. That mode
argument is only applied when the file is **created** — for a file that already
exists, the permissions are left untouched. `/etc/resolv.conf`'s backup and the
crash marker are both rewritten on every apply, so after the first run they keep
whatever mode they already had, not 0600. The same POSIX gap exists in Rust's
`OpenOptions::mode()`.

**Rust:** an explicit `chmod` after writing, so the mode is what the code claims
regardless of whether the file pre-existed.

### 1.19 The DNS crash marker was written after the damage, not before (M5)

Go applies the resolver change first and writes the crash marker afterwards, so
a crash in between leaves the host's DNS modified with **no marker at all** —
nothing ever restores it, and the operator is never told.

**Rust** inverts the ordering: the read-only snapshot and any durable per-backend
backup happen first, then the marker is written, and only then is the host
actually mutated. "A marker exists" becomes a *precondition* for "DNS may have
changed" rather than a record that it did, so a crash before the marker provably
means nothing was touched and a crash after is always recoverable. The remaining
sub-millisecond window can orphan one inert backup file, which the next apply
cleans up.

### 1.20 CNAME chains were silently mangled (M5)

Go's answer conversion builds every returned resource record using the
**question's** type and owner name, for every entry in the answer array. A
CNAME→A chain therefore loses the CNAME entirely, and the A record is returned
under the wrong owner name. Any client relying on the chain sees a subtly wrong
answer rather than an error.

**Rust:** each answer is converted using its own `name` and `type` fields.

### 1.21 The WireGuard tunnel was never actually established (M6)

The tobogganing module's VPN data plane does nothing to host networking. Three
independent gaps, each sufficient on its own:

1. **`realWGController.Configure` is `return nil`.** Its comment claims
   "wgctrl.Client doesn't have a direct Configure method" — this is false.
   `Client.ConfigureDevice(name, cfg)` exists in the pinned version and its own
   doc comment describes exactly this operation.
2. **The interface is never created.** wgctrl only *configures an existing*
   WireGuard device; every lookup path returns `ErrNotExist` when it is absent,
   and nothing in the module ever creates one.
3. **The client's public key is never sent to the manager.** A fresh keypair is
   generated on every connect, but the config fetch carries only a bearer token.
   The manager's peer table cannot learn this client's key, so no handshake is
   possible even with (1) and (2) fixed.

Around that: `Disconnect` only flips a boolean, `RotateConfig` re-fetches config
and never re-applies it (its `force` flag is read nowhere), and `LastHandshake`
is set once at connect time and never refreshed from the device — so the
"handshake staleness" health check actually measures time since connect, and the
`handshake_age_seconds` metric reports that same fiction.

**Rust** implements the data plane for real on the **kernel path** (netlink):
create the interface, configure it, read genuine handshake times and byte
counters from the device on every read, and tear the interface down on
disconnect.

The **userspace path is not a working tunnel.** It builds a real `boringtun`
session from the configured keys — a test proves it emits a correctly-formed
148-byte handshake initiation entirely offline — and then returns an explicit
`Unsupported` error, because the TUN device, UDP socket, and packet-forwarding
loop are not wired up. That is deliberate: an honest error is the opposite of
the Go defect this section describes, which reported success while doing
nothing. Completing it is tracked separately.

**Gap 3 is not fixable here.** Sending the client public key is a manager-API
contract change. The Rust client sends its public key when fetching tunnel
config, which is the correct client behaviour, but a real handshake against a
production manager additionally requires the manager to accept and register it.
Until then the data plane is verified against a peer we control in a network
namespace, which proves our implementation independently of that gap.

### 1.22 A restart silently disabled token refresh and health monitoring (M6)

The module's stop channel is created once at construction and closed by `Stop`.
It is never recreated, so after `Start → Stop → Start` every loop spawned by the
second `Start` sees an already-closed channel and returns immediately. Token
refresh and health monitoring silently never run again, with no error anywhere.

Related: nothing ever reconnects. A comment on the initial connect claims
"failures are logged and left to the monitor loop to retry", but the monitor
loop only updates a health probe and the refresh loop only refreshes tokens —
neither attempts a reconnect. After a failed initial connect the only recovery
is a manual `tobogganing connect`.

### 1.24 Self-update was dead code that would have broken on macOS and Windows (M7)

Go's `internal/update` package is never imported by any command — it's a tested
library wired into nothing. Beyond being unreachable, its logic was broken in
ways that would only bite in the field:

- `getGOOS()`/`getGOARCH()` return the literal strings `"linux"`/`"amd64"` with a
  comment "would be replaced with actual runtime in production" — it never was.
- The asset match hardcodes `.tar.gz`, silently excluding every Windows `.zip`,
  and `extract()` only implements `tar+gzip` — so a Windows update could never
  even unpack.

**Rust** detects OS/arch at runtime and maps to the release vocabulary — notably
`"macos"` → `"darwin"`, the trap that would 404 every Mac update — matches the
exact asset filename, extracts both `tar.gz` and `zip`, and verifies with
minisign. With no release key configured it fails **closed** (no key baked in as
a placeholder — that mistake was already removed once from extplugin).

### 1.25 `penguind service install` wrote an unhardened unit (M7)

Go ships a properly hardened systemd unit in its `.deb`/`.rpm` (NoNewPrivileges,
ProtectSystem=strict, a tight CapabilityBoundingSet, a dedicated user, …), but
`penguind service install` goes through kardianos/service, which **generates its
own minimal unit** from a few config fields — so the manual-install path writes
an *unhardened* service while the package path writes the hardened one. The two
disagree on the security posture of the same daemon.

**Rust** embeds the hardened unit and writes it verbatim on `install` (a test
asserts every hardening directive survives), so there is one unit, hardened,
regardless of install path.

### 1.23 The HostService broker leg was dead code, and mis-TLS'd behind that (M3)

Two stacked bugs:

1. The plugin-side hook that dials broker id 1 and calls `Module.Init` lives in
   `sdk.ModulePlugin.GRPCClient`, but go-plugin only ever calls `GRPCServer()`
   on the plugin side, and the host registers its own plugin type. That code
   never runs. **`Module.Init(ctx, HostServices)` has never fired for an external
   plugin**, and `ModuleServiceImpl.Init` is a bare no-op that ignores the module
   entirely.
2. Even if (1) were fixed, the host calls `broker.Accept(1)` and serves a plain
   `grpc.NewServer()` with no credentials, bypassing go-plugin's own
   `AcceptAndServe` helper which wraps the listener in TLS. A correct plugin
   dialing that leg under AutoMTLS would hit a plaintext server.

**Effect in Go:** external plugins cannot log, read secrets, check their
license, read config, or publish events. The entire `HostServices` surface is
unreachable across the plugin boundary. It is invisible in testing only because
the example plugin stores `host` and never uses it.

**Rust:** the host serves `penguin.sdk.v1.HostService` on broker id 1, correctly
TLS-wrapped (host is the TLS server, pinning the plugin's leaf certificate).

**Consequence for the compat gate:** an existing *Go-built* plugin still will not
dial the broker, because the dead code is on the plugin side. So the M3 gate
proves the two halves with two different plugins — the frozen Go `plugin-hello`
for the protocol (handshake, AutoMTLS, every ModuleService RPC, shutdown), and a
correctly-written `plugin-hello-rs` for the HostService callbacks.

**Verified.** `plugin-hello-rs` receives a working, RPC-backed `HostServices`
from our host: the log line and event published during `init` both arrive, and a
secrets round-trip succeeds. Loaded by the *frozen Go host* instead, the same
binary finds only a plaintext listener on broker id 1, fails the TLS handshake
fast, and falls back to a no-op `HostServices` without hanging — which is what
makes it a valid reverse-compatibility fixture as well.

### 1.26 Squawk's `config`/`license` shed a phantom two-word subcommand (M5)

Go's squawk module declared its `config` and `license` commands with a two-word
usage — `config show`, `license status` — plus an optional positional (max args
1). But **neither implementation ever routed on the second word**: bare `config`
and `config show` did the same thing, and the trailing argument was never read.
Rust collapses each to the bare command name (`use=config`/`use=license`, max
args 0), dropping a subcommand level that only ever added confusion. Authority:
the `crates/penguin-module-squawk/src/commands.rs` module doc.

This is the one deliberate Rust↔Go difference in the M8 CLI-tree structural diff.
The parity gate waives exactly it (`scripts/parity/cli-tree.sh` `waive_squawk_tree`,
addressed to those two lines) and still fails on any *other* tree divergence, so
the waiver cannot mask a real regression elsewhere.

---

## 2. Bug-compatible — Go is odd, Rust reproduces it anyway

### 2.1 `api_version` accepts the empty string (M2)

`checkAPIVersion` treats `""` and `"v1"` as equally valid. Rejecting `""` would
be defensible, but callers may rely on omitting it, so Rust accepts both and
returns `UNIMPLEMENTED` with `api_version %q not supported` otherwise.

### 2.2 `ApplyUpdate` never returns a gRPC error (M2)

Failure is reported in the response payload (`applied: false` plus a message)
with an OK status, unlike `CheckUpdate` which returns `Internal`. The CLI
branches on `applied`, so changing this would break it.

### 2.3 `Dispatch` streams exactly one chunk (M2)

The proto models a stream, but the implementation always sends a single chunk
with `final: true`. True streaming would require a streaming `Module` contract;
out of scope.

### 2.4 The bind→chmod window on the control socket (M2)

The unix socket is created by `bind(2)` with umask-derived permissions and only
then `chmod`'d to `0660`, leaving a brief window where it is more permissive.

Rust preserves this order deliberately. The window is not exploitable: the parent
directory is `0750`, so world cannot traverse to the socket at all, and only the
owner and the allowed group — exactly who is authorised — can reach it. Every RPC
is independently re-checked against `SO_PEERCRED` regardless. The obvious
"fix" (manipulating umask around the bind) is process-global and thread-unsafe,
so it would trade a non-issue for a real one.

### 2.5 Windows has no per-RPC peer check (M2)

On Unix every RPC re-validates the peer's credentials; on Windows the named-pipe
DACL is the entire authorization boundary and the interceptor is a no-op. This
asymmetry is intentional upstream and is preserved rather than "helpfully"
hardened, so both platforms behave as documented.

### 2.6 Log timestamps render in UTC, not local time (M4)

The Go CLI formats `penguin logs` timestamps in the host's local timezone. Rust
renders UTC, because matching local time means either a timezone-database
dependency or libc `localtime_r` plumbing, and neither is worth it for a log
tail. Cosmetic and user-visible. **M8: kept as-is (waived)** — UTC is the more
portable choice for a daemon log tail and is what the CLI-parity harness
normalises; a timezone-database dependency is not worth it.

### 2.7 The embedded publisher key is malformed (M3)

Go's `embeddedPublicKey` decodes to 41 bytes; a valid minisign key is 42. The
constant is an unfilled placeholder and can never verify anything — the real
mechanism is the pinned keys in `/etc/penguin/trusted-publishers.d`.

Ported verbatim and inert, with a comment. **Resolved in M7** (`c4a5dab`): the
malformed constant was removed rather than filled, so nothing now presents a
fake verification path — the pinned keys in `/etc/penguin/trusted-publishers.d`
remain the only mechanism.

### 2.8 Metrics carry a name prefix, not a `module=` label (M8)

Go namespaces each module's metrics with a `module="<name>"` **const label** on
an otherwise-bare metric name (`squawk_queries_total{module="squawk"}`), applied
centrally via `prometheus.WrapRegistererWith`. Rust instead prefixes the name
(`penguin_module_squawk_queries_total`, with no `module` label), because tikv's
`prometheus` crate has no `WrapRegistererWith` equivalent — reproducing Go's
scheme would mean hand-adding a const label to every collector's `Opts` for no
behavioural gain. The module identity is present either way, so the **scheme is
waived**, not fixed.

The metric *set* is otherwise matched 1:1: the base names now agree — the one
gratuitous mismatch, tobogganing `conn_errors` → `connection_errors_total`, was
fixed in M8 — and a `metrics_parity` test pins the Rust family/label set against
a checked-in golden. (The waddlebot module is Rust-only, has no Go oracle, and is
out of parity scope; a doubled `waddlebot_waddlebot_` prefix in it was a plain
bug, also fixed in M8.)

### 2.9 Raw daemon-stdout JSON log field names differ (M8)

Go logs through zap (`timestamp`/`level`/`msg`/`logger` + ad-hoc fields); Rust
through `tracing` (`module`/`fields`/`message`). The field names in the raw
stdout JSON therefore differ — an inherent consequence of two different logging
libraries, not a regression. The only log surface that crosses a boundary, the
`LogLine{level,message,at}` returned by `TailLogs`, matches on both sides and is
diffed by the parity harness. The internal stdout format is **waived**.

### 2.10 One daemon-unreachable message where Go printed two (M8)

Go emits two different "is penguind running?" strings depending on which code
path noticed — a dial-time `"penguin: is penguind running? daemon unreachable"`
and a friendly gRPC-`Unavailable` `"cannot reach penguind at %s — is the daemon
running?"`. §1.14 already records that Rust makes the friendly message uniform
across every verb; this completes it: Rust prints that **one** message everywhere
and has no separate dial-path string. **Waived** — one clear message beats two
that differ by accident of call path.

---

## 3. Completed stubs — Go declared it, Rust implements it

| Behaviour | Go | Rust | Milestone |
|---|---|---|---|
| `TailLogs` | returns `UNIMPLEMENTED` though the proto declares the stream | real bounded log ring buffer per source, backlog replay + `follow` | M2 |
| Telemetry PII sanitisation | hand-applied `maskSecret` convention at call sites | a sanitiser applied to *every* field at the single logging boundary, so a module author cannot forget | M1 |
| Crash detection | see §1.5 | health-poll loop | M2 |
| Encrypted-file secret backend | delegated to 99designs/keyring (JWE) | implemented directly: XChaCha20-Poly1305, per-record nonce, AAD bound to the namespaced key | M4 |
| squawk DNS cache | **none at all** — every forwarded query round-trips upstream, while `ConfigSchema` still advertises a `cache.enabled` toggle with nothing behind it, and `cache stats`/`cache flush` return canned text | a real TTL-respecting answer cache keyed on (name, qtype), which also makes `cache.enabled`, `cache stats`, and `cache flush` mean something | M5 |
| squawk `time` | hardcoded JSON saying NTP/NTS is "not currently exposed" | a real SNTP offset query (the self-contained plain-UDP client, not the NTS/interceptor stack, which stays out of scope) | M5 |
| systemd-resolved | the apply/restore path is a stub that **always errors**, so it silently falls through to clobbering `/etc/resolv.conf` — fighting the resolver that owns it on most modern distros | implemented for real over D-Bus (`org.freedesktop.resolve1`), with the resolv.conf path kept as the fallback it was meant to be | M5 |
| squawk metrics | four of five are registered and **never written** (`queries_total`, `cache_entries`, `health_status`), and `forwarder_up` is not updated by the `forward start`/`stop` commands so it drifts from reality | all five actually wired to the values they claim to report | M5 |
| Tray menu | `internal/tray/model.go` builds a model, but the shell's `onReady` only wires static Refresh/Quit — the model is largely unused | full menu model as a pure, GUI-free crate: nested `tray:true` subtrees (Go flattens), per-module load/unload, severity combining state *and* health so a `Failed` module reads urgent before a health probe lands | M4 |
| Self-update | a library imported by nothing, hardcoding `linux`/`amd64` and unable to unpack a Windows `.zip` (§1.24) | runtime OS/arch detection, exact asset match, `tar.gz`+`zip` extraction, minisign-verified, fail-closed with no key | M7 |
| Tray shells | `render()` only updates the tooltip; its own comment admits "a minimal static menu" (§ Tray menu) | `ksni` (Linux) and `tray-icon`+`tao` (mac/win) shells that snapshot the daemon and rebuild the full menu on every event | M7 |

### A recurring shape

Three of the entries above and three in §1 are the same failure: **the component was written, tested, and never connected.** The event broker, the supervisor's restart machine, the plugin HostService leg, and the tray model were all implemented competently and left unreachable by their callers.

Component-level tests cannot see this — each unit passes in isolation precisely because the wiring is what is missing. It is worth remembering when reading the Go build's 90%+ coverage: coverage measures whether code ran under test, not whether anything in production calls it.

---

## 3a. Deliberate non-features

Things that look absent but are absent **on purpose**. Recorded so a future
reader does not "restore" them.

### No domain-based license bypass (M4)

PenguinTech's platform standards describe a domain-based bypass for license
checks. The Go endpoint agent implements **none** — it never inspects a
deployment domain, and there is no env var or config flag that disables
licensing either. That rule is for web services evaluating their own serving
domain; a desktop endpoint agent has no equivalent notion.

The Rust client ports this faithfully: **no bypass mechanism of any kind**, with
a test (`no_domain_based_bypass_exists`) pinning it. Adding one would introduce a
security-relevant escape hatch that has never existed in this product. If a
bypass is ever genuinely wanted here, it needs a deliberate design, not an
inference from a rule written for a different tier.

Items that were deferred to a later milestone, with their disposition as of the
M8 audit. Listed so a done item is not mistaken for a parity gap, and a still-open
one is not forgotten.

| Item | Disposition | Detail |
|---|---|---|
| `serve()` — the Rust-plugin entry point | **delivered M3** | `crates/penguin-sdk/src/plugin/serve.rs`; `plugin-hello-rs` exercises it as the reverse-direction compat proof under the frozen Go daemon |
| go-plugin / squawk protos | **delivered** (goplugin M3, squawk M5) | generated in `penguin-proto` and consumed by the go-plugin host and the squawk client — no generated-but-unused code accumulated |
| Per-module const-label metric namespacing | **resolved M8 → waived (§2.8)** | tikv's prometheus has no `WrapRegistererWith`; Rust prefixes the metric name instead of adding a `module=` label — decided and recorded as a waiver in M8 |
| Windows IPC verification | **still deferred** | Written and `#[cfg(windows)]`-gated, but not compiled or exercised on Linux CI — needs a Windows runner to verify |

## 5. Dependency notes

### The "no ring" rule is about TLS, not the whole tree

The workspace pins rustls to the **aws-lc-rs** provider because go-plugin's
AutoMTLS presents ECDSA **P-521** certificates that `ring` cannot verify. The
guardrail during development was "`cargo tree -i ring` must be empty" — which was
too strong. Since M6, WireGuard's userspace engine (`boringtun`, and
`defguard_boringtun` pulled by `defguard_wireguard_rs`) brings `ring` into the
tree for its **Noise-protocol** crypto — a completely separate concern from TLS.

The invariant that actually matters still holds: every TLS-using crate
(`penguin-goplugin-host`, `penguin-licensing`, `waddlebot-client`, the DoH
client) passes `cargo tree -i ring` **empty**, so rustls is aws-lc-rs everywhere
and P-521 works. `ring`'s presence for WireGuard crypto is accepted and isolated;
the correct check is per-TLS-crate, not workspace-wide.
