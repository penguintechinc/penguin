//! Tobogganing's CLI command tree (pure data — see [`command_tree`]) and
//! its [`dispatch`] handlers.
//!
//! A direct port of `go-client/internal/modules/tobogganing/module.go`'s
//! `Commands`/`Dispatch`/`cmd*` functions, with one fix: [`format_status_text`]
//! sorts its detail keys before rendering, where Go iterated
//! `map[string]string` directly and so produced output whose key order
//! varied from call to call (Go's own JSON path did not have this bug —
//! `encoding/json` sorts map keys when marshaling — so only the
//! plain-text `status` command was affected; this port sorts both, for the
//! same determinism at no real cost).

use std::collections::{BTreeMap, HashMap};

use penguin_sdk::{CommandResult, CommandSpec, FlagSpec, FlagType, Module, ModuleError, Status};
use serde::Serialize;

use crate::module::TobogganingModule;

/// Declares Tobogganing's CLI command tree. Preserves the Go module's tray
/// flags (`connect`/`disconnect`) and command shape exactly.
pub fn command_tree() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            name: "connect".to_string(),
            use_line: "connect".to_string(),
            short: "Establish VPN connection".to_string(),
            tray: true,
            ..Default::default()
        },
        CommandSpec {
            name: "disconnect".to_string(),
            use_line: "disconnect".to_string(),
            short: "Terminate VPN connection".to_string(),
            tray: true,
            ..Default::default()
        },
        CommandSpec {
            name: "status".to_string(),
            use_line: "status".to_string(),
            short: "Show VPN connection status".to_string(),
            flags: vec![FlagSpec {
                name: "json".to_string(),
                shorthand: String::new(),
                usage: "Output as JSON".to_string(),
                default: "false".to_string(),
                flag_type: FlagType::Bool,
            }],
            ..Default::default()
        },
        CommandSpec {
            name: "rotate".to_string(),
            use_line: "rotate".to_string(),
            short: "Force config/cert rotation".to_string(),
            flags: vec![FlagSpec {
                name: "force".to_string(),
                shorthand: String::new(),
                usage: "Force refresh without checking expiry".to_string(),
                default: "false".to_string(),
                flag_type: FlagType::Bool,
            }],
            ..Default::default()
        },
    ]
}

/// The single entry point [`crate::module::TobogganingModule::dispatch`]
/// delegates to.
pub(crate) async fn dispatch(
    module: &TobogganingModule,
    path: &[String],
    flags: &HashMap<String, String>,
    _args: &[String],
) -> Result<CommandResult, ModuleError> {
    let Some(command) = path.first() else {
        return Ok(usage_result("no command specified"));
    };
    match command.as_str() {
        "connect" => Ok(cmd_connect(module).await),
        "disconnect" => Ok(cmd_disconnect(module).await),
        "status" => {
            let as_json = flags.get("json").map(String::as_str) == Some("true");
            cmd_status(module, as_json).await
        }
        "rotate" => {
            let force = flags.get("force").map(String::as_str) == Some("true");
            Ok(cmd_rotate(module, force).await)
        }
        other => Ok(usage_result(format!("unknown command: {other}"))),
    }
}

fn usage_result(message: impl Into<String>) -> CommandResult {
    CommandResult {
        output: message.into(),
        json: Vec::new(),
        exit_code: 1,
    }
}

/// `tobogganing connect`: brings the tunnel up via
/// [`crate::module::establish_tunnel`] (the same path the module's initial
/// connect and reconnect-on-failure retries use).
async fn cmd_connect(module: &TobogganingModule) -> CommandResult {
    match crate::module::establish_tunnel(module).await {
        Ok(()) => CommandResult {
            output: "connected".to_string(),
            json: Vec::new(),
            exit_code: 0,
        },
        Err(err) => usage_result(format!("connection failed: {err}")),
    }
}

/// `tobogganing disconnect`.
async fn cmd_disconnect(module: &TobogganingModule) -> CommandResult {
    match module.vpn().disconnect().await {
        Ok(()) => {
            module.metrics().tunnel_up.set(0.0);
            CommandResult {
                output: "disconnected".to_string(),
                json: Vec::new(),
                exit_code: 0,
            }
        }
        Err(err) => usage_result(format!("disconnection failed: {err}")),
    }
}

#[derive(Serialize)]
struct StatusJson<'a> {
    state: &'a str,
    detail: BTreeMap<&'a String, &'a String>,
}

/// `tobogganing status [--json]`.
async fn cmd_status(
    module: &TobogganingModule,
    as_json: bool,
) -> Result<CommandResult, ModuleError> {
    let status: Status = module.status().await?;

    if as_json {
        let payload = StatusJson {
            state: status.state.as_str(),
            detail: status.detail.iter().collect(),
        };
        let json = serde_json::to_vec(&payload).unwrap_or_default();
        return Ok(CommandResult {
            output: String::from_utf8_lossy(&json).into_owned(),
            json,
            exit_code: 0,
        });
    }

    Ok(CommandResult {
        output: format_status_text(&status),
        json: Vec::new(),
        exit_code: 0,
    })
}

/// Renders `status` as `State: <state>` followed by each detail entry,
/// sorted by key — see this module's doc for the Go non-determinism this
/// fixes.
fn format_status_text(status: &Status) -> String {
    let mut output = format!("State: {}\n", status.state.as_str());
    let mut keys: Vec<&String> = status.detail.keys().collect();
    keys.sort();
    for key in keys {
        output.push_str(&format!("  {key}: {}\n", status.detail[key]));
    }
    output
}

/// `tobogganing rotate [--force]`.
async fn cmd_rotate(module: &TobogganingModule, force: bool) -> CommandResult {
    match module.vpn().rotate(module.auth(), force).await {
        Ok(()) => CommandResult {
            output: "config rotated".to_string(),
            json: Vec::new(),
            exit_code: 0,
        },
        Err(err) => usage_result(format!("rotation failed: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_tree_declares_every_command_with_go_parity_tray_flags() {
        let tree = command_tree();
        let names: Vec<&str> = tree.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["connect", "disconnect", "status", "rotate"]);

        let connect = tree.iter().find(|c| c.name == "connect").unwrap();
        let disconnect = tree.iter().find(|c| c.name == "disconnect").unwrap();
        assert!(connect.tray);
        assert!(disconnect.tray);

        let status = tree.iter().find(|c| c.name == "status").unwrap();
        assert!(!status.tray);
        assert_eq!(status.flags[0].name, "json");

        let rotate = tree.iter().find(|c| c.name == "rotate").unwrap();
        assert_eq!(rotate.flags[0].name, "force");
    }

    #[test]
    fn format_status_text_is_deterministically_sorted() {
        let mut detail = HashMap::new();
        detail.insert("zeta".to_string(), "1".to_string());
        detail.insert("alpha".to_string(), "2".to_string());
        detail.insert("mid".to_string(), "3".to_string());
        let status = Status {
            state: penguin_sdk::ModuleState::Running,
            detail,
        };

        let rendered = format_status_text(&status);
        let alpha_idx = rendered.find("alpha").unwrap();
        let mid_idx = rendered.find("mid").unwrap();
        let zeta_idx = rendered.find("zeta").unwrap();
        assert!(alpha_idx < mid_idx);
        assert!(mid_idx < zeta_idx);

        // Rendering the same detail map repeatedly must always agree —
        // the actual regression this guards against (HashMap iteration
        // order is randomized per-process in both Go and Rust).
        for _ in 0..20 {
            assert_eq!(format_status_text(&status), rendered);
        }
    }
}
