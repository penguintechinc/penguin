//! Compat integration tests: launch the real, unmodified Go `plugin-hello`
//! binary (`go-client/examples/plugin-hello`, built against go-plugin
//! v1.7.0) and drive it through the whole host lifecycle end to end.
//!
//! Every other test in this crate is a pure unit test — no process, no
//! socket. This file is the only place that proves the real, load-bearing
//! claim of the migration: that the Rust host can launch and speak to a
//! genuine Go-built plugin binary, AutoMTLS handshake against a live ECDSA
//! P-521 certificate included.
//!
//! # Running
//!
//! Every test here is `#[ignore]` *and* separately checks
//! `PENGUIN_INTEGRATION=1` at runtime, so neither a plain `cargo test` nor a
//! bare `cargo test -- --ignored` ever spawns a process:
//!
//! ```sh
//! PENGUIN_INTEGRATION=1 cargo test -p penguin-goplugin-host \
//!     --test goplugin_compat -- --ignored --nocapture
//! ```
//!
//! The plugin binary is located via `PENGUIN_PLUGIN_HELLO`, else
//! `go-client/bin/plugin-hello` relative to the repo root; build it with:
//!
//! ```sh
//! go build -o go-client/bin/plugin-hello ./go-client/examples/plugin-hello
//! ```
//!
//! A missing binary skips (with a message) rather than failing the run.
//!
//! # What is not covered here
//!
//! `plugin-hello` never dials the host's broker id=1/`HostService` leg — the
//! plugin-side hook that would is dead code in the frozen Go SDK (see
//! `docs/PARITY.md` §1.9). Every launch below passes `host_routes: None`,
//! which both matches that reality and confirms an unserved broker leg does
//! not break the session.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use penguin_goplugin_host::client::PluginProcess;
// `Box<dyn Module>` dispatches through its vtable, so calling `.info()`,
// `.commands()`, and `.dispatch()` on it below needs no `Module` trait
// import — unlike a generic `T: Module` bound, a trait object already knows
// how to resolve its own methods.

/// The RPC-visible name of the sole command `plugin-hello` declares.
const GREET_COMMAND: &str = "greet";

/// Skips the calling test (with a message) unless the integration tier is
/// explicitly opted into. Kept separate from `#[ignore]`: a bare `--ignored`
/// run must still not spawn a real process — see the module doc comment.
macro_rules! require_integration {
    () => {
        if std::env::var("PENGUIN_INTEGRATION").as_deref() != Ok("1") {
            eprintln!("SKIP: set PENGUIN_INTEGRATION=1 to run goplugin_compat tests");
            return;
        }
    };
}

/// Locates the `plugin-hello` binary, preferring `PENGUIN_PLUGIN_HELLO` over
/// the default build output path. Returns `None` (after printing why) when
/// neither exists, so callers skip rather than fail.
fn plugin_hello_path() -> Option<PathBuf> {
    if let Ok(overridden) = std::env::var("PENGUIN_PLUGIN_HELLO") {
        let path = PathBuf::from(overridden);
        if path.is_file() {
            return Some(path);
        }
        eprintln!(
            "SKIP: PENGUIN_PLUGIN_HELLO={} does not exist",
            path.display()
        );
        return None;
    }

    let default =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../go-client/bin/plugin-hello");
    if default.is_file() {
        return Some(default);
    }
    eprintln!(
        "SKIP: no plugin-hello binary at {} — build it with `go build -o go-client/bin/plugin-hello ./go-client/examples/plugin-hello`, or set PENGUIN_PLUGIN_HELLO",
        default.display()
    );
    None
}

/// Launches `plugin-hello` with no host-service routes served, returning
/// `None` (already having printed why) when the binary is missing. Panics
/// with the real `HostError` on any launch failure — spawn, handshake,
/// AutoMTLS, or health check — so the failure is diagnosable directly from
/// the panic message.
async fn launch_hello_or_skip() -> Option<PluginProcess> {
    let path = plugin_hello_path()?;
    let process = PluginProcess::launch(&path, Path::new("/tmp"), None)
        .await
        .expect(
            "plugin-hello launch failed: process spawn, AutoMTLS P-521 handshake, \
             or grpc.health.v1 check did not complete — see the HostError above",
        );
    Some(process)
}

/// Returns whether a process with `pid` is still alive, via `/proc` — valid
/// on the Linux containers every build/test in this repo runs in.
fn pid_exists(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Assertion 1: the process starts and emits a parseable handshake line
/// carrying core version 1 (implied by `launch` succeeding at all — an
/// unsupported core version is a hard parse error before AutoMTLS is even
/// attempted), protocol version 1, `unix`, and a real AutoMTLS certificate.
#[tokio::test]
#[ignore]
async fn raw_handshake_line_is_well_formed_and_carries_a_real_cert() {
    require_integration!();
    let Some(process) = launch_hello_or_skip().await else {
        return;
    };

    let handshake = process.handshake();
    assert_eq!(handshake.protocol_version, 1, "negotiated protocol version");
    assert_eq!(handshake.network, "unix", "handshake network field");
    let cert = handshake
        .server_cert_der
        .as_ref()
        .expect("plugin must present an AutoMTLS server certificate");
    assert!(
        cert.len() > 50,
        "AutoMTLS cert DER should be well over 50 bytes for a P-521 cert, got {}",
        cert.len()
    );

    process.shutdown().await.expect("plugin shutdown");
}

/// Assertions 2 + 3, the headline claim of the whole crate: a real TLS
/// session is established against the plugin's self-signed P-521
/// certificate, and the `grpc.health.v1` check reports SERVING once it is
/// up. `PluginProcess::launch` performs both steps internally and returns
/// `Err` if either fails, so a successful launch here *is* the proof — see
/// the loud `.expect` message in `launch_hello_or_skip` for how a failure
/// here is diagnosed.
#[tokio::test]
#[ignore]
async fn automtls_p521_handshake_and_health_check_succeed() {
    require_integration!();
    let Some(process) = launch_hello_or_skip().await else {
        return;
    };

    process.shutdown().await.expect("plugin shutdown");
}

/// Assertion 4: `Info` round-trips the module's declared identity.
#[tokio::test]
#[ignore]
async fn module_info_round_trips() {
    require_integration!();
    let Some(process) = launch_hello_or_skip().await else {
        return;
    };

    let module = process.dispense().await.expect("dispense ModuleService");
    let info = module.info();
    assert_eq!(info.name, "hello");
    assert_eq!(info.version, "1.0.0");

    process.shutdown().await.expect("plugin shutdown");
}

/// Assertion 5: the plugin declares exactly one command, `greet`, with
/// `min_args == max_args == 1`.
#[tokio::test]
#[ignore]
async fn commands_tree_matches_the_go_source() {
    require_integration!();
    let Some(process) = launch_hello_or_skip().await else {
        return;
    };

    let module = process.dispense().await.expect("dispense ModuleService");
    let commands = module.commands();
    assert_eq!(
        commands.len(),
        1,
        "plugin-hello declares exactly one command"
    );
    let greet = &commands[0];
    assert_eq!(greet.name, GREET_COMMAND);
    assert_eq!(greet.min_args, 1);
    assert_eq!(greet.max_args, 1);

    process.shutdown().await.expect("plugin shutdown");
}

/// Assertion 6: `greet world` dispatches successfully.
#[tokio::test]
#[ignore]
async fn dispatch_greet_succeeds() {
    require_integration!();
    let Some(process) = launch_hello_or_skip().await else {
        return;
    };

    let module = process.dispense().await.expect("dispense ModuleService");
    let path = vec![GREET_COMMAND.to_string()];
    let flags = HashMap::new();
    let args = vec!["world".to_string()];
    let result = module
        .dispatch(&path, &flags, &args)
        .await
        .expect("dispatch must succeed at both the transport and module level");
    assert_eq!(result.output, "hello, world");
    assert_eq!(result.exit_code, 0);

    process.shutdown().await.expect("plugin shutdown");
}

/// Assertion 7: a missing positional arg is a usage failure signalled via
/// exit code, not a transport or module error — `Dispatch` still returns
/// `Ok` on the wire, since the Go handler passes back `nil` for `err`.
#[tokio::test]
#[ignore]
async fn dispatch_greet_missing_arg_is_a_usage_failure_not_an_error() {
    require_integration!();
    let Some(process) = launch_hello_or_skip().await else {
        return;
    };

    let module = process.dispense().await.expect("dispense ModuleService");
    let path = vec![GREET_COMMAND.to_string()];
    let flags = HashMap::new();
    let args = Vec::new();
    let result = module
        .dispatch(&path, &flags, &args)
        .await
        .expect("a usage failure is still a successful RPC — see the doc comment");
    assert_eq!(result.exit_code, 1);
    assert!(
        !result.output.is_empty(),
        "usage failure must explain itself"
    );

    process.shutdown().await.expect("plugin shutdown");
}

/// Assertion 8: an unrecognised command path is a module error carrying the
/// Go source's exact message.
#[tokio::test]
#[ignore]
async fn dispatch_unknown_command_is_a_module_error() {
    require_integration!();
    let Some(process) = launch_hello_or_skip().await else {
        return;
    };

    let module = process.dispense().await.expect("dispense ModuleService");
    let path = vec!["bogus".to_string()];
    let flags = HashMap::new();
    let args = Vec::new();
    let error = module
        .dispatch(&path, &flags, &args)
        .await
        .expect_err("an unknown command must fail");
    assert!(
        error.message.contains("unknown command: bogus"),
        "unexpected error message: {}",
        error.message
    );

    process.shutdown().await.expect("plugin shutdown");
}

/// Assertions 9 + 10: `shutdown` accepts the controller RPC and the child
/// exits gracefully inside the 2s window — proven by timing the call, since
/// a SIGKILL-escalation shutdown would take at least that long — and no
/// child process remains afterward.
///
/// `plugin-hello` is the real, unmodified go-plugin v1.7.0 binary and
/// honours `GRPCController.Shutdown` correctly (it stops its own gRPC
/// server, which unblocks `main` and exits the process), so this test
/// cannot exercise the SIGKILL-escalation branch itself without a plugin
/// binary that ignores shutdown, which is out of scope for a black-box
/// compat test against the frozen Go binary. What it does assert, per the
/// task brief's "at minimum": no child process survives `shutdown()`'s
/// return, which holds on both the graceful and the escalated path.
#[tokio::test]
#[ignore]
async fn shutdown_is_clean_and_no_children_survive() {
    require_integration!();
    let Some(process) = launch_hello_or_skip().await else {
        return;
    };

    let pid = process
        .pid()
        .expect("child must still be running pre-shutdown");
    let started = Instant::now();
    process.shutdown().await.expect("plugin shutdown");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "shutdown took {elapsed:?}, at or beyond the 2s SIGKILL-escalation \
         timeout — the graceful path did not complete in time (assertion 9)"
    );
    assert!(
        !pid_exists(pid),
        "plugin pid {pid} is still present in /proc after shutdown() returned (assertion 10)"
    );
}
