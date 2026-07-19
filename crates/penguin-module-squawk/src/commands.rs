//! squawk's CLI command tree (pure data — see [`command_tree`]) and its
//! [`dispatch`] handlers.
//!
//! Every handler here that was a canned stub in the Go module (`cache
//! stats`/`cache flush`, `time`) is wired to a real backing primitive; see
//! each handler's doc comment for exactly what changed and why.
//!
//! # `config`/`license`'s `Use` string
//!
//! The Go module declared `config` with `Use: "config show"` and `license`
//! with `Use: "license status"` — a two-word usage string implying a real
//! subcommand — but neither `Dispatch` handler ever routed on, or even
//! inspected, that second word; both are single actions that always run.
//! This port collapses both to their single, coherent command name
//! (`"config"`, `"license"`) rather than inventing subcommand trees neither
//! Go nor this port actually implements — the `Use` string now matches what
//! the command genuinely does.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use penguin_sdk::{CommandResult, CommandSpec, FlagSpec, FlagType, ModuleError};
use squawk_client::doh::DnsResponse;
use squawk_client::forwarder::Forwarder;

use crate::mask::mask_secret;
use crate::module::SquawkModule;

/// Upper bound on a `query` command's DoH round trip. Matches Go's
/// `context.WithTimeout(ctx, 5*time.Second)` in `handleQuery`.
const QUERY_TIMEOUT: Duration = Duration::from_secs(5);
/// Upper bound on a `license` command's validation round trip. Matches Go's
/// `context.WithTimeout(ctx, 5*time.Second)` in `handleLicense`.
const LICENSE_TIMEOUT: Duration = Duration::from_secs(5);

/// Declares squawk's CLI command tree. Preserves the Go module's tray flags
/// (`forward start`/`forward stop`/`cache flush`) and command shape exactly,
/// except for the `config`/`license` `Use`-string fix documented in this
/// file's module doc.
pub fn command_tree() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            name: "query".to_string(),
            use_line: "query <domain> [--type TYPE]".to_string(),
            short: "Query a DNS record".to_string(),
            flags: vec![FlagSpec {
                name: "type".to_string(),
                shorthand: "t".to_string(),
                usage: "DNS record type (A, AAAA, MX, TXT, etc.)".to_string(),
                default: "A".to_string(),
                flag_type: FlagType::String,
            }],
            min_args: 1,
            max_args: 1,
            ..Default::default()
        },
        CommandSpec {
            name: "forward".to_string(),
            use_line: "forward".to_string(),
            short: "Manage DNS forwarding".to_string(),
            subcommands: vec![
                CommandSpec {
                    name: "status".to_string(),
                    use_line: "status".to_string(),
                    short: "Show forwarder status".to_string(),
                    ..Default::default()
                },
                CommandSpec {
                    name: "start".to_string(),
                    use_line: "start".to_string(),
                    short: "Start DNS forwarding".to_string(),
                    tray: true,
                    ..Default::default()
                },
                CommandSpec {
                    name: "stop".to_string(),
                    use_line: "stop".to_string(),
                    short: "Stop DNS forwarding".to_string(),
                    tray: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        CommandSpec {
            name: "config".to_string(),
            use_line: "config".to_string(),
            short: "Show current configuration".to_string(),
            ..Default::default()
        },
        CommandSpec {
            name: "cache".to_string(),
            use_line: "cache".to_string(),
            short: "Manage DNS cache".to_string(),
            subcommands: vec![
                CommandSpec {
                    name: "stats".to_string(),
                    use_line: "stats".to_string(),
                    short: "Show cache statistics".to_string(),
                    ..Default::default()
                },
                CommandSpec {
                    name: "flush".to_string(),
                    use_line: "flush".to_string(),
                    short: "Flush the DNS cache".to_string(),
                    tray: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        CommandSpec {
            name: "license".to_string(),
            use_line: "license".to_string(),
            short: "Check license status".to_string(),
            ..Default::default()
        },
        CommandSpec {
            name: "time".to_string(),
            use_line: "time".to_string(),
            short: "Check NTP/NTS status".to_string(),
            ..Default::default()
        },
    ]
}

/// The single entry point [`crate::module::SquawkModule::dispatch`] delegates
/// to. Routes on `path[0]`; each handler owns its own subcommand routing.
pub(crate) async fn dispatch(
    module: &SquawkModule,
    path: &[String],
    flags: &HashMap<String, String>,
    args: &[String],
) -> Result<CommandResult, ModuleError> {
    let Some(command) = path.first() else {
        return Ok(usage_result("squawk: no command specified"));
    };
    match command.as_str() {
        "query" => handle_query(module, flags, args).await,
        "forward" => handle_forward(module, path).await,
        "config" => handle_config(module),
        "cache" => Ok(handle_cache(module, path)),
        "license" => handle_license(module).await,
        "time" => handle_time(module).await,
        other => Ok(unknown_command(other)),
    }
}

fn usage_result(message: impl Into<String>) -> CommandResult {
    CommandResult {
        output: message.into(),
        json: Vec::new(),
        exit_code: 1,
    }
}

fn unknown_command(name: &str) -> CommandResult {
    usage_result(format!("squawk: unknown command '{name}'"))
}

fn unknown_subcommand(name: &str) -> CommandResult {
    usage_result(format!("Unknown subcommand: {name}"))
}

/// Unix nanoseconds since the epoch, matching the `_unix_nano` convention
/// `penguin_sdk::convert` uses elsewhere in this workspace.
fn unix_nano(time: SystemTime) -> i64 {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_nanos() as i64,
        Err(before_epoch) => -(before_epoch.duration().as_nanos() as i64),
    }
}

#[derive(Serialize)]
struct QueryOutput<'a> {
    domain: &'a str,
    record_type: &'a str,
    status: i32,
    answers: &'a [squawk_client::doh::DnsRecord],
    queried_at_unix_nano: i64,
}

/// `squawk query <domain> [--type TYPE]`: a live DoH lookup. Also increments
/// `queries_total` — the Go module registered this counter but never
/// incremented it anywhere.
async fn handle_query(
    module: &SquawkModule,
    flags: &HashMap<String, String>,
    args: &[String],
) -> Result<CommandResult, ModuleError> {
    let Some(domain) = args.first() else {
        return Ok(usage_result("Usage: squawk query <domain> [--type TYPE]"));
    };
    let record_type = flags.get("type").map(String::as_str).unwrap_or("A");

    let cancel = CancellationToken::new();
    module.metrics().queries_total.inc();
    let lookup = module.doh().query(&cancel, domain, record_type);

    let response: DnsResponse = match tokio::time::timeout(QUERY_TIMEOUT, lookup).await {
        Ok(Ok(response)) => response,
        Ok(Err(err)) => return Ok(usage_result(format!("Query failed: {err}"))),
        Err(_elapsed) => return Ok(usage_result("Query failed: timed out")),
    };

    let output = format!(
        "{domain} {record_type}: {} answer(s)",
        response.answer.len()
    );
    let json = serde_json::to_vec(&QueryOutput {
        domain,
        record_type,
        status: response.status,
        answers: &response.answer,
        queried_at_unix_nano: unix_nano(SystemTime::now()),
    })
    .unwrap_or_default();

    Ok(CommandResult {
        output,
        json,
        exit_code: 0,
    })
}

#[derive(Serialize)]
struct ForwarderStatusOutput<'a> {
    status: &'a str,
}

/// `squawk forward {status|start|stop}`.
async fn handle_forward(
    module: &SquawkModule,
    path: &[String],
) -> Result<CommandResult, ModuleError> {
    let Some(sub) = path.get(1) else {
        return Ok(usage_result("Usage: squawk forward {status|start|stop}"));
    };
    match sub.as_str() {
        "status" => Ok(forward_status(module)),
        "start" => Ok(forward_start(module).await),
        "stop" => Ok(forward_stop(module).await),
        other => Ok(unknown_subcommand(other)),
    }
}

fn forward_status(module: &SquawkModule) -> CommandResult {
    let Some(forwarder) = module.forwarder() else {
        return usage_result("Forwarder not configured");
    };
    let status = if forwarder.is_running() {
        "running"
    } else {
        "stopped"
    };
    let json = serde_json::to_vec(&ForwarderStatusOutput { status }).unwrap_or_default();
    CommandResult {
        output: format!("Forwarder: {status}"),
        json,
        exit_code: 0,
    }
}

/// `forward start`: also flips the `forwarder_up` metric — the Go module
/// only ever set this from the module lifecycle (`Module.Start`), never
/// from this CLI subcommand, so an operator-triggered start left the
/// metric reporting stale/wrong state.
async fn forward_start(module: &SquawkModule) -> CommandResult {
    let Some(forwarder) = module.forwarder() else {
        return usage_result("Forwarder not configured");
    };
    match forwarder.start().await {
        Ok(()) => {
            module.metrics().forwarder_up.set(1.0);
            CommandResult {
                output: "Forwarder started".to_string(),
                json: Vec::new(),
                exit_code: 0,
            }
        }
        Err(err) => usage_result(format!("Failed to start forwarder: {err}")),
    }
}

/// `forward stop`: see [`forward_start`]'s doc for why this also updates
/// the metric.
async fn forward_stop(module: &SquawkModule) -> CommandResult {
    let Some(forwarder) = module.forwarder() else {
        return usage_result("Forwarder not configured");
    };
    match forwarder.stop().await {
        Ok(()) => {
            module.metrics().forwarder_up.set(0.0);
            CommandResult {
                output: "Forwarder stopped".to_string(),
                json: Vec::new(),
                exit_code: 0,
            }
        }
        Err(err) => usage_result(format!("Failed to stop forwarder: {err}")),
    }
}

/// `squawk config`: the module's redacted running configuration. Every
/// credential-shaped field (`doh.auth_token`, `license.license_key`,
/// `license.user_token`) is masked before it can ever reach the terminal,
/// a log, or a support ticket screenshot.
fn handle_config(module: &SquawkModule) -> Result<CommandResult, ModuleError> {
    let mut redacted = module.config().clone();
    redacted.doh.auth_token = mask_secret(&redacted.doh.auth_token);
    redacted.license.license_key = mask_secret(&redacted.license.license_key);
    redacted.license.user_token = mask_secret(&redacted.license.user_token);

    let json = serde_json::to_vec_pretty(&redacted)
        .map_err(|err| ModuleError::new(format!("render config: {err}")))?;
    let output = String::from_utf8_lossy(&json).into_owned();
    Ok(CommandResult {
        output,
        json,
        exit_code: 0,
    })
}

#[derive(Serialize)]
struct CacheStatsOutput {
    entries: usize,
    hits: u64,
    misses: u64,
}

/// `squawk cache {stats|flush}`: wired directly to the forwarder's real
/// answer cache. The Go module returned hard-coded text here
/// ("Cache statistics from DoH client not currently exposed" / "cache
/// flushed (client-level cache not directly accessible)") because no real
/// cache existed at the time it was written — `squawk-client`'s Rust port
/// adds one (see that crate's module doc), so that excuse no longer
/// applies.
fn handle_cache(module: &SquawkModule, path: &[String]) -> CommandResult {
    let Some(sub) = path.get(1) else {
        return usage_result("Usage: squawk cache {stats|flush}");
    };
    let Some(forwarder) = module.forwarder() else {
        return usage_result("Forwarder not configured; cache unavailable");
    };
    match sub.as_str() {
        "stats" => cache_stats(module, forwarder),
        "flush" => cache_flush(module, forwarder),
        other => unknown_subcommand(other),
    }
}

fn cache_stats(module: &SquawkModule, forwarder: &Forwarder) -> CommandResult {
    let stats = forwarder.cache().stats();
    module.metrics().cache_entries.set(stats.entries as f64);
    let json = serde_json::to_vec(&CacheStatsOutput {
        entries: stats.entries,
        hits: stats.hits,
        misses: stats.misses,
    })
    .unwrap_or_default();
    CommandResult {
        output: format!(
            "Cache: {} entries, {} hits, {} misses",
            stats.entries, stats.hits, stats.misses
        ),
        json,
        exit_code: 0,
    }
}

fn cache_flush(module: &SquawkModule, forwarder: &Forwarder) -> CommandResult {
    forwarder.cache().flush();
    module.metrics().cache_entries.set(0.0);
    CommandResult {
        output: "Cache flushed".to_string(),
        json: br#"{"status":"flushed"}"#.to_vec(),
        exit_code: 0,
    }
}

#[derive(Serialize)]
struct LicenseOutput<'a> {
    status: &'a str,
    valid: bool,
    checked_at_unix_nano: i64,
    feature_key: &'a str,
}

/// `squawk license`: a live check against squawk's own product license
/// server. Fixes two real Go bugs:
///
/// 1. Go's `ModuleConfig` had no field to ever populate a license
///    `ServerURL`, so `handleLicense` always built a validator pointed at
///    `""` and every request failed before any network I/O —
///    `squawk license status` could never succeed. Here,
///    `squawk_client::config::LicenseConfig::server_url` defaults to
///    squawk's real license server, so this genuinely works.
/// 2. Go's `handleLicense` always returned `ExitCode: 0`, even when
///    validation failed or errored. Here, `exit_code` is `0` only when the
///    license is confirmed valid.
async fn handle_license(module: &SquawkModule) -> Result<CommandResult, ModuleError> {
    let mut license_cfg = module.config().license.clone();
    if license_cfg.user_token.is_empty()
        && let Ok(token) = module.host().secrets().get("user_token").await
    {
        license_cfg.user_token = String::from_utf8_lossy(&token).into_owned();
    }

    let validator = squawk_client::license::Validator::new(license_cfg);
    let outcome = tokio::time::timeout(LICENSE_TIMEOUT, validator.is_valid()).await;

    let (status_text, valid, exit_code) = match outcome {
        Ok(Ok(true)) => ("valid".to_string(), true, 0),
        Ok(Ok(false)) => ("invalid or expired".to_string(), false, 1),
        Ok(Err(err)) => (format!("error: {err}"), false, 1),
        Err(_elapsed) => ("error: validation timed out".to_string(), false, 1),
    };

    let json = serde_json::to_vec(&LicenseOutput {
        status: &status_text,
        valid,
        checked_at_unix_nano: unix_nano(SystemTime::now()),
        feature_key: "penguin.squawk",
    })
    .unwrap_or_default();

    Ok(CommandResult {
        output: format!("License status: {status_text}"),
        json,
        exit_code,
    })
}

#[derive(Serialize)]
struct TimeOutput {
    synchronized: bool,
    offset_nanos: i64,
    round_trip_nanos: i64,
    stratum: u8,
    checked_at_unix_nano: i64,
}

#[derive(Serialize)]
struct TimeErrorOutput {
    synchronized: bool,
    error: String,
}

/// `squawk time`: a live SNTP query. The Go module returned a hard-coded
/// `"NTP/NTS not currently exposed by squawk-client-go at module level"`
/// blob; `squawk_client::ntp::NtpClient` (a real Rust port of Go's own SNTP
/// client) replaces that with an actual clock-offset measurement.
async fn handle_time(module: &SquawkModule) -> Result<CommandResult, ModuleError> {
    let ntp_config = squawk_client::ntp::ClientConfig {
        server_urls: module.config().ntp.server_urls.clone(),
        timeout: 0,
        max_retries: 0,
        retry_delay: 0,
    };
    let client = squawk_client::ntp::NtpClient::new(ntp_config);
    let cancel = CancellationToken::new();

    match client.query(&cancel).await {
        Ok(response) => {
            let json = serde_json::to_vec(&TimeOutput {
                synchronized: true,
                offset_nanos: response.offset_nanos,
                round_trip_nanos: response.round_trip_nanos,
                stratum: response.stratum,
                checked_at_unix_nano: unix_nano(SystemTime::now()),
            })
            .unwrap_or_default();
            let output = format!(
                "NTP: synchronized (offset {:.3}ms, round-trip {:.3}ms, stratum {})",
                response.offset_nanos as f64 / 1_000_000.0,
                response.round_trip_nanos as f64 / 1_000_000.0,
                response.stratum,
            );
            Ok(CommandResult {
                output,
                json,
                exit_code: 0,
            })
        }
        Err(err) => {
            let json = serde_json::to_vec(&TimeErrorOutput {
                synchronized: false,
                error: err.to_string(),
            })
            .unwrap_or_default();
            Ok(CommandResult {
                output: format!("NTP query failed: {err}"),
                json,
                exit_code: 1,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_tree_declares_every_top_level_command() {
        let tree = command_tree();
        let names: Vec<&str> = tree.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["query", "forward", "config", "cache", "license", "time"]
        );
    }

    #[test]
    fn config_and_license_use_a_single_coherent_word() {
        let tree = command_tree();
        let config = tree.iter().find(|c| c.name == "config").unwrap();
        let license = tree.iter().find(|c| c.name == "license").unwrap();
        assert_eq!(config.use_line, "config");
        assert_eq!(license.use_line, "license");
        assert!(config.subcommands.is_empty());
        assert!(license.subcommands.is_empty());
    }

    #[test]
    fn forward_and_cache_declare_their_tray_subcommands() {
        let tree = command_tree();
        let forward = tree.iter().find(|c| c.name == "forward").unwrap();
        let start = forward
            .subcommands
            .iter()
            .find(|c| c.name == "start")
            .unwrap();
        let stop = forward
            .subcommands
            .iter()
            .find(|c| c.name == "stop")
            .unwrap();
        assert!(start.tray);
        assert!(stop.tray);

        let cache = tree.iter().find(|c| c.name == "cache").unwrap();
        let flush = cache
            .subcommands
            .iter()
            .find(|c| c.name == "flush")
            .unwrap();
        assert!(flush.tray);
        let stats = cache
            .subcommands
            .iter()
            .find(|c| c.name == "stats")
            .unwrap();
        assert!(!stats.tray);
    }

    #[test]
    fn unix_nano_round_trips_a_known_instant() {
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(unix_nano(time), 1_700_000_000_000_000_000);
    }
}
