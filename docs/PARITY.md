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

### 1.15 The HostService broker leg was dead code, and mis-TLS'd behind that (M3)

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
tail. Cosmetic and user-visible; revisit at M8 if it grates.

### 2.7 The embedded publisher key is malformed (M3)

Go's `embeddedPublicKey` decodes to 41 bytes; a valid minisign key is 42. The
constant is an unfilled placeholder and can never verify anything — the real
mechanism is the pinned keys in `/etc/penguin/trusted-publishers.d`.

Ported verbatim and inert, with a comment. **To be resolved in M7**: either bake
in the real publisher key or delete the constant. An inert malformed constant is
a trap for whoever next assumes it works.

---

## 3. Completed stubs — Go declared it, Rust implements it

| Behaviour | Go | Rust | Milestone |
|---|---|---|---|
| `TailLogs` | returns `UNIMPLEMENTED` though the proto declares the stream | real bounded log ring buffer per source, backlog replay + `follow` | M2 |
| Telemetry PII sanitisation | hand-applied `maskSecret` convention at call sites | a sanitiser applied to *every* field at the single logging boundary, so a module author cannot forget | M1 |
| Crash detection | see §1.5 | health-poll loop | M2 |
| Encrypted-file secret backend | delegated to 99designs/keyring (JWE) | implemented directly: XChaCha20-Poly1305, per-record nonce, AAD bound to the namespaced key | M4 |
| Tray menu | `internal/tray/model.go` builds a model, but the shell's `onReady` only wires static Refresh/Quit — the model is largely unused | full menu model as a pure, GUI-free crate: nested `tray:true` subtrees (Go flattens), per-module load/unload, severity combining state *and* health so a `Failed` module reads urgent before a health probe lands | M4 |

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

These are unimplemented *yet*, tracked to their milestone, and listed so they are
not mistaken for parity gaps.

| Item | Deferred to | Why |
|---|---|---|
| `serve()` — the Rust-plugin entry point | M3 | Untestable until the go-plugin host exists to load it |
| go-plugin / squawk protos | consuming milestone | Avoids accumulating generated-but-unused code |
| Per-module const-label metric namespacing | M5 | tikv's prometheus has no `WrapRegistererWith`; no metrics consumer exists until squawk |
| Windows IPC verification | M7 | Written and `cfg`-gated, but not compiled or exercised by Linux CI |
