//! Command penguind is the privileged endpoint-agent daemon. All privileged
//! operations (tunnels, port 53, resolver changes) live in modules hosted
//! here; the `penguin` CLI and `penguin-tray` stay unprivileged. Ported from
//! `go-client/cmd/penguind/`.

#[cfg(unix)]
mod daemon_main;
#[cfg(unix)]
mod host_wiring;
#[cfg(unix)]
mod logging;
mod service;
mod watchdog;

use std::process::ExitCode;

/// The daemon's own version string.
///
/// M7: replace with a dedicated build-info/version crate if one lands (the
/// Go reference reads `internal/version.Version`, injected at link time via
/// `-ldflags`). For now this is simply the crate's own workspace version —
/// the same "0.2.0"-shaped string every other `penguind` artifact
/// (Cargo.toml, container tag) already carries.
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // `penguind version` / `--version` must work with no config, lock, or
    // privileged setup at all — checked against the raw args before clap,
    // or anything else, touches the filesystem.
    if is_version_request(&args) {
        println!("{VERSION}");
        return ExitCode::SUCCESS;
    }

    // `penguind watchdog` is a distinct top-level subcommand (the mutual-
    // supervision peer process — see `watchdog`'s module doc), not a
    // `service` verb: dispatched here, before `service` and before any
    // config/lock loading, the same "must work standalone" positioning as
    // the version check above.
    if args.first().map(String::as_str) == Some("watchdog") {
        return watchdog::run_watchdog();
    }

    // Handle `service` subcommands (install, uninstall, start, stop,
    // status). These must run BEFORE loading config or acquiring locks —
    // mirrors `go-client/cmd/penguind/main.go`'s `run()`, which checks
    // `handleServiceCommand`'s `handled` bool the same way.
    let host = service::real_host();
    if let Some(result) = service::handle_service_command(&args, &host) {
        return match result {
            Ok(line) => {
                println!("{line}");
                ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("penguind: {message}");
                ExitCode::FAILURE
            }
        };
    }

    run()
}

/// True for exactly `penguind version` or `penguind --version` — the only
/// two spellings that must work before any config, lock, or privileged
/// setup.
fn is_version_request(args: &[String]) -> bool {
    args.len() == 1 && (args[0] == "version" || args[0] == "--version")
}

/// Runs the real daemon. Unix-only for now — see `daemon_main`'s module doc.
#[cfg(unix)]
fn run() -> ExitCode {
    daemon_main::run()
}

/// Non-Unix stub: the IPC listener, peer-credential auth, and single-
/// instance lock this binary depends on are all Unix-specific in this
/// milestone (Windows named-pipe wiring lands with M7 service support).
#[cfg(not(unix))]
fn run() -> ExitCode {
    eprintln!("penguind: only Unix targets are supported in this milestone");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_request_matches_both_spellings_and_nothing_else() {
        assert!(is_version_request(&["version".to_string()]));
        assert!(is_version_request(&["--version".to_string()]));
        assert!(!is_version_request(&[]));
        assert!(!is_version_request(&[
            "version".to_string(),
            "extra".to_string()
        ]));
        assert!(!is_version_request(&[
            "--config-dir".to_string(),
            "/etc/penguin".to_string()
        ]));
    }
}
