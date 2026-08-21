//! Raw daemon.v1 wire probe for the M8 parity harness.
//!
//! The frozen Go CLI and the Rust CLI only ever speak `api_version = "v1"` and
//! only reach `ApplyUpdate` through the gated `update` flow, so neither can
//! exercise two things the parity harness must check over the wire: an
//! *unknown* `api_version` (must be rejected `UNIMPLEMENTED`) and a *direct*
//! `ApplyUpdate` call (must report `applied:false` with an OK status, never a
//! gRPC error — see docs/PARITY.md §2.2). This example is a minimal gRPC
//! client that can, speaking the same `penguin-proto` contract both daemons
//! serve, so `scripts/parity/*.sh` can drive it against either implementation.
//!
//! It prints one machine-parseable `PROBE ...` line per fact and a final
//! `PROBE status=<code>` line (`ok` or a tonic status code), then exits 0 for
//! any completed RPC — the caller asserts on the printed lines, not the exit
//! code. A dial failure exits 3; a usage error exits 2.
//!
//! Usage:
//!   parity_probe <socket> <op> [args...]
//! Ops:
//!   version                     — Version RPC (used for the bad-api_version check)
//!   list-commands               — dump every loaded module's command tree (structural)
//!   dispatch <module> [path...]  — run a command, report chunk count / final / exit
//!   watch-events [module]        — subscribe and print events (PROBE_EVENT_COUNT / PROBE_TIMEOUT_MS)
//!   check-update                — CheckUpdate RPC (may reach the network)
//!   apply-update                — ApplyUpdate RPC (network-free: fail-closed on missing key)
//! Env:
//!   PROBE_API_VERSION  api_version to send (default "v1")
//!   PROBE_EVENT_COUNT  events to collect before watch-events returns (default 1)
//!   PROBE_TIMEOUT_MS   watch-events overall deadline in ms (default 4000)

use std::collections::HashMap;
use std::process::ExitCode;
use std::time::Duration;

use penguin_cli_core::pb;
use penguin_cli_core::pb::daemon_client::DaemonClient;
use tonic::transport::Channel;

/// A parsed `api_version` override, defaulting to `"v1"`.
fn api_version() -> String {
    std::env::var("PROBE_API_VERSION").unwrap_or_else(|_| "v1".to_string())
}

/// Prints the terminal status line for a completed RPC. `Ok` renders as
/// `ok`; any gRPC failure renders its tonic [`tonic::Code`] (e.g.
/// `Unimplemented`, `Internal`) plus the message, so a caller can assert on
/// either.
fn print_status(result: Result<(), tonic::Status>) {
    if let Err(status) = result {
        println!(
            "PROBE status={:?} message={:?}",
            status.code(),
            status.message()
        );
    } else {
        println!("PROBE status=ok");
    }
}

/// Emits one `PROBE cmd ...` line per command, depth-first, joining the path
/// with `/` so nested subcommands are unambiguous and the whole dump sorts
/// into a stable order for diffing.
fn dump_command(module: &str, prefix: &str, spec: &pb::CommandSpec) {
    let path = if prefix.is_empty() {
        spec.name.clone()
    } else {
        format!("{prefix}/{}", spec.name)
    };
    let mut flags = String::new();
    for flag in &spec.flags {
        if !flags.is_empty() {
            flags.push(',');
        }
        flags.push_str(&format!(
            "{}:{}:{}:{}",
            flag.name, flag.shorthand, flag.r#type, flag.default
        ));
    }
    println!(
        "PROBE cmd {module}|{path}|use={}|short={}|min={}|max={}|tray={}|flags={}",
        spec.r#use,
        spec.short,
        spec.min_args,
        spec.max_args,
        u8::from(spec.tray),
        flags
    );
    for sub in &spec.subcommands {
        dump_command(module, &path, sub);
    }
}

/// `list-commands`: dump every loaded module's tree, then report status.
async fn op_list_commands(client: &mut DaemonClient<Channel>) {
    let request = pb::ListCommandsRequest {
        api_version: api_version(),
    };
    let response = client.list_commands(request).await;
    match response {
        Ok(response) => {
            let modules = response.into_inner().modules;
            for module in &modules {
                for command in &module.commands {
                    dump_command(&module.module, "", command);
                }
            }
            println!("PROBE list-commands modules={}", modules.len());
            print_status(Ok(()));
        }
        Err(status) => print_status(Err(status)),
    }
}

/// `dispatch <module> [path...]`: run a command and report how many chunks
/// came back, whether exactly one was `final`, and its exit code — the §2.3
/// single-final-chunk contract.
async fn op_dispatch(client: &mut DaemonClient<Channel>, module: String, path: Vec<String>) {
    let request = pb::DispatchRequest {
        api_version: api_version(),
        module,
        path,
        flags: HashMap::new(),
        args: Vec::new(),
    };
    let stream = client.dispatch(request).await;
    let mut stream = match stream {
        Ok(response) => response.into_inner(),
        Err(status) => {
            print_status(Err(status));
            return;
        }
    };

    let mut chunks = 0u32;
    let mut finals = 0u32;
    let mut exit = 0i32;
    loop {
        let message = stream.message().await;
        match message {
            Ok(Some(chunk)) => {
                chunks += 1;
                if chunk.r#final {
                    finals += 1;
                    exit = chunk.exit_code;
                }
            }
            Ok(None) => break,
            Err(status) => {
                print_status(Err(status));
                return;
            }
        }
    }
    println!("PROBE dispatch chunks={chunks} finals={finals} exit={exit}");
    print_status(Ok(()));
}

/// `watch-events [module]`: subscribe, then print each event up to
/// `PROBE_EVENT_COUNT`, bounded by `PROBE_TIMEOUT_MS` so it can never hang.
async fn op_watch_events(client: &mut DaemonClient<Channel>, module: String) {
    let want: u32 = std::env::var("PROBE_EVENT_COUNT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    let deadline_ms: u64 = std::env::var("PROBE_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(4000);

    let request = pb::WatchEventsRequest {
        api_version: api_version(),
        module,
    };
    let stream = client.watch_events(request).await;
    let mut stream = match stream {
        Ok(response) => response.into_inner(),
        Err(status) => {
            print_status(Err(status));
            return;
        }
    };

    let mut seen = 0u32;
    let deadline = Duration::from_millis(deadline_ms);
    while seen < want {
        let next = tokio::time::timeout(deadline, stream.message()).await;
        let message = match next {
            Ok(inner) => inner,
            Err(_elapsed) => break,
        };
        match message {
            Ok(Some(event)) => {
                seen += 1;
                println!(
                    "PROBE event module={} type={} message={}",
                    event.module, event.r#type, event.message
                );
            }
            Ok(None) => break,
            Err(status) => {
                print_status(Err(status));
                return;
            }
        }
    }
    println!("PROBE done count={seen}");
    print_status(Ok(()));
}

/// `check-update`: may reach the network; the harness only asserts it returns
/// (never hangs) and the daemon survives, so both a well-formed response and a
/// clean error status are acceptable outcomes.
async fn op_check_update(client: &mut DaemonClient<Channel>) {
    let request = pb::CheckUpdateRequest {
        api_version: api_version(),
    };
    let response = client.check_update(request).await;
    match response {
        Ok(response) => {
            let response = response.into_inner();
            println!(
                "PROBE check-update available={} current={} latest={}",
                u8::from(response.available),
                response.current_version,
                response.latest_version
            );
            print_status(Ok(()));
        }
        Err(status) => print_status(Err(status)),
    }
}

/// `apply-update`: network-free when no publisher key is embedded (apply fails
/// closed before any network call), so this deterministically reports
/// `applied:false` with an OK status — docs/PARITY.md §2.2.
async fn op_apply_update(client: &mut DaemonClient<Channel>) {
    let request = pb::ApplyUpdateRequest {
        api_version: api_version(),
    };
    let response = client.apply_update(request).await;
    match response {
        Ok(response) => {
            let response = response.into_inner();
            println!(
                "PROBE apply-update applied={} message={:?}",
                u8::from(response.applied),
                response.message
            );
            print_status(Ok(()));
        }
        Err(status) => print_status(Err(status)),
    }
}

/// `version`: the smallest RPC, used to check that an unknown `api_version` is
/// rejected `UNIMPLEMENTED` over the wire without loading any module.
async fn op_version(client: &mut DaemonClient<Channel>) {
    let request = pb::VersionRequest {
        api_version: api_version(),
    };
    let response = client.version(request).await;
    match response {
        Ok(response) => {
            println!(
                "PROBE version daemon={}",
                response.into_inner().daemon_version
            );
            print_status(Ok(()));
        }
        Err(status) => print_status(Err(status)),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: parity_probe <socket> <op> [args...]");
        return ExitCode::from(2);
    }
    let socket = args[1].clone();
    let op = args[2].clone();

    let channel = penguin_ipc::dial_unix::dial(&socket).await;
    let channel = match channel {
        Ok(channel) => channel,
        Err(err) => {
            println!("PROBE status=dial-failed message={err:?}");
            return ExitCode::from(3);
        }
    };
    let mut client = DaemonClient::new(channel);

    match op.as_str() {
        "version" => op_version(&mut client).await,
        "list-commands" => op_list_commands(&mut client).await,
        "dispatch" => {
            if args.len() < 4 {
                eprintln!("usage: parity_probe <socket> dispatch <module> [path...]");
                return ExitCode::from(2);
            }
            let module = args[3].clone();
            let path = args[4..].to_vec();
            op_dispatch(&mut client, module, path).await;
        }
        "watch-events" => {
            let module = args.get(3).cloned().unwrap_or_default();
            op_watch_events(&mut client, module).await;
        }
        "check-update" => op_check_update(&mut client).await,
        "apply-update" => op_apply_update(&mut client).await,
        other => {
            eprintln!("parity_probe: unknown op {other:?}");
            return ExitCode::from(2);
        }
    }
    ExitCode::SUCCESS
}
