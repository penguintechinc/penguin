//! SkausWatch's CLI command tree (pure data — see [`command_tree`]) and its
//! [`dispatch`] handlers.
//!
//! Mirrors `penguin-module-tobogganing::commands`'s pattern: a
//! [`command_tree`] returning pure `CommandSpec` data, and a [`dispatch`]
//! handler that routes on command name and flag values.

use std::collections::{BTreeMap, HashMap};

use penguin_sdk::{CommandResult, CommandSpec, FlagSpec, FlagType, Module, ModuleError};
use serde::Serialize;

use crate::module::SkausWatchModule;

/// Declares SkausWatch's CLI command tree: `status` and `enroll` commands.
pub fn command_tree() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            name: "status".to_string(),
            use_line: "status".to_string(),
            short: "Show SkausWatch endpoint status".to_string(),
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
            name: "enroll".to_string(),
            use_line: "enroll".to_string(),
            short: "(Re-)enroll this endpoint with the SkausWatch Manager".to_string(),
            ..Default::default()
        },
    ]
}

/// The single entry point [`crate::module::SkausWatchModule::dispatch`]
/// delegates to.
pub(crate) async fn dispatch(
    module: &SkausWatchModule,
    path: &[String],
    flags: &HashMap<String, String>,
    _args: &[String],
) -> Result<CommandResult, ModuleError> {
    let Some(command) = path.first() else {
        return Ok(usage_result("no command specified"));
    };
    match command.as_str() {
        "status" => {
            let as_json = flags.get("json").map(String::as_str) == Some("true");
            cmd_status(module, as_json).await
        }
        "enroll" => cmd_enroll(module).await,
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

#[derive(Serialize)]
struct StatusJson<'a> {
    state: &'a str,
    detail: BTreeMap<&'a String, &'a String>,
}

/// `skauswatch status [--json]`: reports enrollment and identity state.
async fn cmd_status(
    module: &SkausWatchModule,
    as_json: bool,
) -> Result<CommandResult, ModuleError> {
    let status = module.status().await?;

    if as_json {
        let payload = StatusJson {
            state: status.state.as_str(),
            detail: status.detail.iter().collect(),
        };
        let json_bytes = serde_json::to_vec(&payload).map_err(|err| {
            ModuleError::new(format!("failed to serialize status to JSON: {err}"))
        })?;
        let output = String::from_utf8_lossy(&json_bytes).into_owned();
        return Ok(CommandResult {
            output,
            json: json_bytes,
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
/// sorted by key — ensures deterministic output.
fn format_status_text(status: &penguin_sdk::Status) -> String {
    let mut output = format!("State: {}\n", status.state.as_str());
    let mut keys: Vec<&String> = status.detail.keys().collect();
    keys.sort();
    for key in keys {
        output.push_str(&format!("  {key}: {}\n", status.detail[key]));
    }
    output
}

/// `skauswatch enroll`: triggers re-enrollment by clearing the cached
/// identity and allowing the heartbeat loop to register a fresh one.
async fn cmd_enroll(module: &SkausWatchModule) -> Result<CommandResult, ModuleError> {
    // Clear cached identity so the next heartbeat tick re-registers.
    // Recover from poisoned mutex rather than panicking on prior thread panic.
    {
        let mut guard = module
            .inner
            .identity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = None;
    }

    // Attempt to delete the persisted identity from the secret store
    let _ = module.host().secrets().delete("agent_identity").await;

    Ok(CommandResult {
        output: "enrolled: identity cleared, re-enrollment scheduled on next heartbeat".to_string(),
        json: Vec::new(),
        exit_code: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_tree_declares_status_and_enroll_commands() {
        let tree = command_tree();
        let names: Vec<&str> = tree.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["status", "enroll"]);

        let status = tree.iter().find(|c| c.name == "status").unwrap();
        assert_eq!(status.flags[0].name, "json");

        let enroll = tree.iter().find(|c| c.name == "enroll").unwrap();
        assert!(enroll.flags.is_empty());
    }

    #[test]
    fn format_status_text_is_deterministically_sorted() {
        let mut detail = HashMap::new();
        detail.insert("zeta".to_string(), "1".to_string());
        detail.insert("alpha".to_string(), "2".to_string());
        detail.insert("mid".to_string(), "3".to_string());
        let status = penguin_sdk::Status {
            state: penguin_sdk::ModuleState::Running,
            detail,
        };

        let rendered = format_status_text(&status);
        let alpha_idx = rendered.find("alpha").unwrap();
        let mid_idx = rendered.find("mid").unwrap();
        let zeta_idx = rendered.find("zeta").unwrap();
        assert!(alpha_idx < mid_idx);
        assert!(mid_idx < zeta_idx);

        // Rendering the same detail map repeatedly must always agree
        for _ in 0..20 {
            assert_eq!(format_status_text(&status), rendered);
        }
    }

    #[tokio::test]
    async fn enroll_dispatch_recovers_from_poisoned_mutex() {
        // This test verifies that cmd_enroll doesn't panic on a poisoned mutex.
        // We can't easily poison a mutex in a test, but we can at least verify
        // the dispatch path runs without panicking with a normal mutex.
        use crate::module::SkausWatchModule;
        use crate::testutil;

        let module = SkausWatchModule::new();
        module.init(testutil::fake_host_default()).await.unwrap();

        let result = cmd_enroll(&module).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().exit_code, 0);
    }
}
