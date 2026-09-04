//! Entry point for the unprivileged system-tray binary.
//!
//! Connects to the daemon the same way the CLI does (see [`connection`]),
//! then hands off to whichever platform shell this binary was built for:
//! [`tray_linux`] (ksni, runs entirely on Tokio) on Linux, [`tray_native`]
//! (`tray-icon` + `tao`, needs the OS main thread) on macOS/Windows.

mod connection;
mod daemon_loop;
mod label;
mod snapshot;

#[cfg(target_os = "linux")]
mod tray_linux;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod tray_native;

use std::process::ExitCode;

/// The default control-socket path, matching the daemon's own default and
/// the Go tray's `defaultSocket()` (`go-client/cmd/penguin-tray/main.go`).
const DEFAULT_SOCKET_PATH: &str = "/run/penguin/penguind.sock";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 2 && (args[1] == "version" || args[1] == "--version") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let socket_path = socket_path_from_args(&args[1..]);
    run(socket_path)
}

/// Builds a multi-threaded Tokio runtime, connects to the daemon, and runs
/// the ksni shell on it. Unlike the native shell, ksni needs no dedicated OS
/// main thread (see `tray_linux`'s module doc), so the whole binary is one
/// async program on Linux.
#[cfg(target_os = "linux")]
fn run(socket_path: String) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("penguin-tray: cannot start async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async move {
        let client = match connection::connect(&socket_path).await {
            Ok(client) => client,
            Err(err) => {
                eprintln!(
                    "penguin-tray: cannot reach penguind at {socket_path} — is the daemon running? ({err})"
                );
                return ExitCode::FAILURE;
            }
        };
        tray_linux::run(client).await
    })
}

/// Hands off directly to the native shell, which owns its own thread
/// structure (and its own daemon connection, made on a background thread)
/// — see `tray_native`'s module doc for why this cannot also build a Tokio
/// runtime and connect here first, the way [`run`]'s Linux twin does.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn run(socket_path: String) -> ExitCode {
    tray_native::run(socket_path)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn run(socket_path: String) -> ExitCode {
    let _ = socket_path;
    eprintln!("penguin-tray: unsupported platform");
    ExitCode::FAILURE
}

/// Scans `args` for a `--socket`/`-socket` override, accepting both
/// `VALUE`-as-next-token and `=VALUE` spellings; the last occurrence wins.
///
/// A hand-rolled twin of `penguin_cli_core::socket::extract_socket_override`
/// — this binary deliberately does not depend on `penguin-cli-core`, which
/// exists to build the `penguin` CLI's dynamic clap command tree, something
/// a single-flag binary like this one has no use for.
fn socket_path_from_args(args: &[String]) -> String {
    let mut result = None;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if let Some(value) = arg
            .strip_prefix("--socket=")
            .or_else(|| arg.strip_prefix("-socket="))
        {
            result = Some(value.to_string());
        } else if (arg == "--socket" || arg == "-socket")
            && let Some(value) = args.get(i + 1)
        {
            result = Some(value.clone());
            i += 1;
        }
        i += 1;
    }
    result.unwrap_or_else(|| DEFAULT_SOCKET_PATH.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_override_returns_the_default_socket_path() {
        let args: Vec<String> = Vec::new();
        assert_eq!(socket_path_from_args(&args), DEFAULT_SOCKET_PATH);
    }

    #[test]
    fn space_separated_override_is_found() {
        let args = vec!["--socket".to_string(), "/tmp/x.sock".to_string()];
        assert_eq!(socket_path_from_args(&args), "/tmp/x.sock");
    }

    #[test]
    fn equals_separated_override_is_found() {
        let args = vec!["--socket=/tmp/x.sock".to_string()];
        assert_eq!(socket_path_from_args(&args), "/tmp/x.sock");
    }

    #[test]
    fn last_occurrence_wins() {
        let args = vec![
            "--socket".to_string(),
            "/tmp/a.sock".to_string(),
            "--socket".to_string(),
            "/tmp/b.sock".to_string(),
        ];
        assert_eq!(socket_path_from_args(&args), "/tmp/b.sock");
    }

    #[test]
    fn dangling_socket_flag_with_no_value_is_ignored() {
        let args = vec!["--socket".to_string()];
        assert_eq!(socket_path_from_args(&args), DEFAULT_SOCKET_PATH);
    }
}
