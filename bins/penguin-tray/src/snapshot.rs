//! Joins the daemon's `ListModules` + `GetStatus` + `ListCommands`
//! responses into the `Vec<ModuleInput>` `penguin_tray_model::build_menu`
//! consumes — this crate's equivalent of the Go tray's `tray.Snapshot`
//! (`go-client/internal/tray/model.go`), which has no Rust port yet.
//!
//! The join itself ([`join_snapshot`]) is a pure function over the three
//! response messages — no socket, no daemon, no async — so every join
//! decision (a module missing from `GetStatus`, an extra `GetStatus`/
//! `ListCommands` entry for a name `ListModules` never reported, an
//! unrecognised state/health string) is unit-tested below with synthetic
//! proto values. [`fetch_snapshot`] is the thin, deliberately untested async
//! wrapper that calls the three RPCs and hands their responses to it.

use std::collections::HashMap;

use penguin_proto::daemon::v1 as pb;
use penguin_proto::daemon::v1::daemon_client::DaemonClient;
use penguin_sdk::{CommandSpec, FlagSpec, FlagType, HealthLevel, ModuleState};
use penguin_tray_model::ModuleInput;
use tonic::Status;
use tonic::transport::Channel;

use crate::connection::API_VERSION;

/// Calls `ListModules`, `GetStatus`, and `ListCommands` against `client` and
/// joins their responses via [`join_snapshot`].
///
/// The three calls are sequential rather than concurrent: `DaemonClient`'s
/// generated methods take `&mut self`, and a daemon reachable for one of
/// these three is reachable for all of them in practice, so there is no
/// latency case here worth the extra complexity of cloning the client
/// (`tonic::transport::Channel` is cheaply `Clone`) to run them
/// concurrently.
pub async fn fetch_snapshot(
    client: &mut DaemonClient<Channel>,
) -> Result<Vec<ModuleInput>, Status> {
    let modules = client
        .list_modules(pb::ListModulesRequest {
            api_version: API_VERSION.to_string(),
        })
        .await?
        .into_inner();
    let status = client
        .get_status(pb::GetStatusRequest {
            api_version: API_VERSION.to_string(),
            name: String::new(),
        })
        .await?
        .into_inner();
    let commands = client
        .list_commands(pb::ListCommandsRequest {
            api_version: API_VERSION.to_string(),
        })
        .await?
        .into_inner();
    Ok(join_snapshot(&modules, &status, &commands))
}

/// Joins three independent RPC responses into one [`ModuleInput`] per
/// module, driven entirely by `modules` — a `GetStatus`/`ListCommands`
/// entry for a name `modules` never reported is silently ignored, matching
/// the Go tray's `Snapshot`, which only ever ranges over `mods.Modules`.
pub fn join_snapshot(
    modules: &pb::ListModulesResponse,
    status: &pb::GetStatusResponse,
    commands: &pb::ListCommandsResponse,
) -> Vec<ModuleInput> {
    let mut status_by_name: HashMap<&str, &pb::ModuleStatus> =
        HashMap::with_capacity(status.modules.len());
    for entry in &status.modules {
        status_by_name.insert(entry.name.as_str(), entry);
    }

    let mut commands_by_module: HashMap<&str, &Vec<pb::CommandSpec>> =
        HashMap::with_capacity(commands.modules.len());
    for entry in &commands.modules {
        commands_by_module.insert(entry.module.as_str(), &entry.commands);
    }

    let mut inputs = Vec::with_capacity(modules.modules.len());
    for summary in &modules.modules {
        let status_entry = status_by_name.get(summary.name.as_str()).copied();
        let health = status_entry.and_then(|entry| parse_health(&entry.health));
        let health_message = status_entry
            .map(|entry| entry.health_message.clone())
            .unwrap_or_default();
        let module_commands = commands_by_module
            .get(summary.name.as_str())
            .map(|specs| specs.iter().map(command_spec_from_daemon_proto).collect())
            .unwrap_or_default();

        inputs.push(ModuleInput {
            name: summary.name.clone(),
            state: ModuleState::parse(&summary.state).unwrap_or_default(),
            health,
            health_message,
            commands: module_commands,
        });
    }
    inputs
}

/// Parses `GetStatus`'s `health` wire string. Unlike [`ModuleState::parse`],
/// an unrecognised value maps to `None` rather than a default variant:
/// [`ModuleInput::health`] uses `None` specifically to mean "not probed
/// yet", and `HealthLevel` — unlike Go's four-value `Health` enum — has no
/// variant of its own for "unknown", so an unparseable string is treated the
/// same as a module simply missing from the response.
fn parse_health(value: &str) -> Option<HealthLevel> {
    match value {
        "healthy" => Some(HealthLevel::Healthy),
        "degraded" => Some(HealthLevel::Degraded),
        "unhealthy" => Some(HealthLevel::Unhealthy),
        _ => None,
    }
}

/// Converts one wire flag spec (`daemon.v1.FlagSpec`) to its ergonomic
/// `penguin_sdk` form. A hand-written twin of
/// `penguin_sdk::convert::flag_spec_from_proto`, which converts the
/// shape-identical but distinct `sdk.v1.FlagSpec` type instead — see this
/// module's doc for why the daemon's own wire type needs its own converter
/// here rather than reusing that one.
fn flag_spec_from_daemon_proto(flag: &pb::FlagSpec) -> FlagSpec {
    FlagSpec {
        name: flag.name.clone(),
        shorthand: flag.shorthand.clone(),
        usage: flag.usage.clone(),
        default: flag.default.clone(),
        flag_type: FlagType::parse(&flag.r#type),
    }
}

/// Converts one wire command spec (and its whole subtree) from
/// `daemon.v1.CommandSpec` to `penguin_sdk::CommandSpec` — the type
/// [`penguin_tray_model::ModuleInput::commands`] expects.
fn command_spec_from_daemon_proto(command: &pb::CommandSpec) -> CommandSpec {
    let mut flags = Vec::with_capacity(command.flags.len());
    for flag in &command.flags {
        flags.push(flag_spec_from_daemon_proto(flag));
    }
    let mut subcommands = Vec::with_capacity(command.subcommands.len());
    for sub in &command.subcommands {
        subcommands.push(command_spec_from_daemon_proto(sub));
    }
    CommandSpec {
        name: command.name.clone(),
        use_line: command.r#use.clone(),
        short: command.short.clone(),
        flags,
        subcommands,
        tray: command.tray,
        min_args: command.min_args,
        max_args: command.max_args,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn module_summary(name: &str, state: &str) -> pb::ModuleSummary {
        pb::ModuleSummary {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
            state: state.to_string(),
            external: false,
            license_feature: String::new(),
        }
    }

    fn module_status(name: &str, health: &str, message: &str) -> pb::ModuleStatus {
        pb::ModuleStatus {
            name: name.to_string(),
            state: String::new(),
            detail: HashMap::new(),
            health: health.to_string(),
            health_message: message.to_string(),
            checked_at_unix_nano: 0,
        }
    }

    fn tray_command(name: &str) -> pb::CommandSpec {
        pb::CommandSpec {
            name: name.to_string(),
            r#use: name.to_string(),
            short: format!("{name} short"),
            flags: Vec::new(),
            subcommands: Vec::new(),
            tray: true,
            min_args: 0,
            max_args: 0,
        }
    }

    #[test]
    fn module_present_in_all_three_responses_joins_completely() {
        let modules = pb::ListModulesResponse {
            modules: vec![module_summary("squawk", "running")],
        };
        let status = pb::GetStatusResponse {
            daemon_version: "1.2.3".to_string(),
            modules: vec![module_status("squawk", "degraded", "server down")],
            otel: None,
            fleet_dm: None,
        };
        let commands = pb::ListCommandsResponse {
            modules: vec![pb::ModuleCommands {
                module: "squawk".to_string(),
                commands: vec![tray_command("forward")],
            }],
        };

        let joined = join_snapshot(&modules, &status, &commands);

        assert_eq!(joined.len(), 1);
        let squawk = &joined[0];
        assert_eq!(squawk.name, "squawk");
        assert_eq!(squawk.state, ModuleState::Running);
        assert_eq!(squawk.health, Some(HealthLevel::Degraded));
        assert_eq!(squawk.health_message, "server down");
        assert_eq!(squawk.commands.len(), 1);
        assert_eq!(squawk.commands[0].name, "forward");
        assert!(squawk.commands[0].tray);
    }

    #[test]
    fn module_present_in_modules_only_is_not_probed_and_has_no_commands() {
        let modules = pb::ListModulesResponse {
            modules: vec![module_summary("tobogganing", "disabled")],
        };

        let joined = join_snapshot(
            &modules,
            &pb::GetStatusResponse::default(),
            &pb::ListCommandsResponse::default(),
        );

        assert_eq!(joined.len(), 1);
        let module = &joined[0];
        assert_eq!(module.state, ModuleState::Disabled);
        assert_eq!(module.health, None, "never probed, not merely unhealthy");
        assert_eq!(module.health_message, "");
        assert!(module.commands.is_empty());
    }

    #[test]
    fn status_and_commands_for_a_module_not_in_list_modules_are_ignored() {
        let modules = pb::ListModulesResponse {
            modules: vec![module_summary("squawk", "running")],
        };
        let status = pb::GetStatusResponse {
            daemon_version: String::new(),
            modules: vec![
                module_status("squawk", "healthy", ""),
                module_status("ghost", "unhealthy", "should never surface"),
            ],
            otel: None,
            fleet_dm: None,
        };
        let commands = pb::ListCommandsResponse {
            modules: vec![
                pb::ModuleCommands {
                    module: "squawk".to_string(),
                    commands: vec![tray_command("forward")],
                },
                pb::ModuleCommands {
                    module: "ghost".to_string(),
                    commands: vec![tray_command("haunt")],
                },
            ],
        };

        let joined = join_snapshot(&modules, &status, &commands);

        assert_eq!(joined.len(), 1, "only the module ListModules reported");
        assert_eq!(joined[0].name, "squawk");
    }

    #[test]
    fn unrecognised_state_string_falls_back_to_disabled() {
        let modules = pb::ListModulesResponse {
            modules: vec![module_summary("mystery", "levitating")],
        };

        let joined = join_snapshot(
            &modules,
            &pb::GetStatusResponse::default(),
            &pb::ListCommandsResponse::default(),
        );

        assert_eq!(joined[0].state, ModuleState::Disabled);
    }

    #[test]
    fn unrecognised_health_string_is_treated_as_not_probed() {
        let modules = pb::ListModulesResponse {
            modules: vec![module_summary("squawk", "running")],
        };
        let status = pb::GetStatusResponse {
            daemon_version: String::new(),
            modules: vec![module_status("squawk", "levitating", "")],
            otel: None,
            fleet_dm: None,
        };

        let joined = join_snapshot(&modules, &status, &pb::ListCommandsResponse::default());

        assert_eq!(joined[0].health, None);
    }

    #[test]
    fn empty_responses_join_to_an_empty_list() {
        let joined = join_snapshot(
            &pb::ListModulesResponse::default(),
            &pb::GetStatusResponse::default(),
            &pb::ListCommandsResponse::default(),
        );
        assert!(joined.is_empty());
    }

    #[test]
    fn nested_subcommands_convert_recursively() {
        let modules = pb::ListModulesResponse {
            modules: vec![module_summary("squawk", "running")],
        };
        let mut parent = tray_command("forward");
        parent.subcommands = vec![tray_command("start")];
        let commands = pb::ListCommandsResponse {
            modules: vec![pb::ModuleCommands {
                module: "squawk".to_string(),
                commands: vec![parent],
            }],
        };

        let joined = join_snapshot(&modules, &pb::GetStatusResponse::default(), &commands);

        assert_eq!(joined[0].commands[0].subcommands.len(), 1);
        assert_eq!(joined[0].commands[0].subcommands[0].name, "start");
    }
}
