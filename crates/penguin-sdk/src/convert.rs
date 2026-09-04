//! Conversions between the ergonomic sdk types and the generated `sdk.v1` wire
//! types.
//!
//! These live here (not in `penguin-proto`) so the generated crate stays pure
//! codegen. They are plain, exhaustively-tested functions rather than `From`
//! impls: naming each direction (`_to_proto` / `_from_proto`) reads more
//! clearly than inferring direction from a target type, and it keeps the two
//! `CommandSpec`/`FlagSpec` structs (ours and the proto's) from colliding.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use penguin_proto::sdk::v1 as pb;

use crate::command::{CommandResult, CommandSpec, FlagSpec, FlagType};
use crate::host::{Event, EventType};
use crate::status::{HealthLevel, HealthReport, ModuleState, Status};

/// The `api_version` field every `sdk.v1` request carries. Unknown versions are
/// rejected by the receiver per the PenguinTech gRPC versioning standard.
pub const API_VERSION: &str = "v1";

/// Converts a wall-clock time to Unix nanoseconds.
///
/// Times before the epoch produce a negative value, matching Go's `UnixNano`.
fn system_time_to_unix_nano(time: SystemTime) -> i64 {
    // duration_since returns Err carrying the gap when `time` predates the
    // epoch — the two arms are the after/before-epoch cases, not stylistic.
    match time.duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_nanos() as i64,
        Err(before_epoch) => -(before_epoch.duration().as_nanos() as i64),
    }
}

/// Inverse of [`system_time_to_unix_nano`].
fn unix_nano_to_system_time(nanos: i64) -> SystemTime {
    if nanos >= 0 {
        UNIX_EPOCH + Duration::from_nanos(nanos as u64)
    } else {
        // unsigned_abs avoids overflow at i64::MIN that plain negation would hit.
        UNIX_EPOCH - Duration::from_nanos(nanos.unsigned_abs())
    }
}

/// Converts a flag spec to its wire form.
pub fn flag_spec_to_proto(flag: &FlagSpec) -> pb::FlagSpec {
    pb::FlagSpec {
        name: flag.name.clone(),
        shorthand: flag.shorthand.clone(),
        usage: flag.usage.clone(),
        default: flag.default.clone(),
        r#type: flag.flag_type.as_str().to_string(),
    }
}

/// Converts a wire flag spec back into an sdk flag spec.
pub fn flag_spec_from_proto(flag: &pb::FlagSpec) -> FlagSpec {
    FlagSpec {
        name: flag.name.clone(),
        shorthand: flag.shorthand.clone(),
        usage: flag.usage.clone(),
        default: flag.default.clone(),
        flag_type: FlagType::parse(&flag.r#type),
    }
}

/// Converts a command spec (and its whole subtree) to its wire form.
pub fn command_spec_to_proto(command: &CommandSpec) -> pb::CommandSpec {
    let mut flags: Vec<pb::FlagSpec> = Vec::with_capacity(command.flags.len());
    for flag in &command.flags {
        flags.push(flag_spec_to_proto(flag));
    }
    let mut subcommands: Vec<pb::CommandSpec> = Vec::with_capacity(command.subcommands.len());
    for sub in &command.subcommands {
        subcommands.push(command_spec_to_proto(sub));
    }
    pb::CommandSpec {
        name: command.name.clone(),
        r#use: command.use_line.clone(),
        short: command.short.clone(),
        flags,
        subcommands,
        tray: command.tray,
        min_args: command.min_args,
        max_args: command.max_args,
    }
}

/// Converts a wire command spec (and its whole subtree) back into sdk form.
pub fn command_spec_from_proto(command: &pb::CommandSpec) -> CommandSpec {
    let mut flags: Vec<FlagSpec> = Vec::with_capacity(command.flags.len());
    for flag in &command.flags {
        flags.push(flag_spec_from_proto(flag));
    }
    let mut subcommands: Vec<CommandSpec> = Vec::with_capacity(command.subcommands.len());
    for sub in &command.subcommands {
        subcommands.push(command_spec_from_proto(sub));
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

/// Converts a status to a wire `StatusResponse`. The `error` field is left
/// empty; a failing `Status` call carries its error separately.
pub fn status_to_proto(status: &Status) -> pb::StatusResponse {
    pb::StatusResponse {
        state: status.state.as_str().to_string(),
        detail: status.detail.clone(),
        error: String::new(),
    }
}

/// Extracts the status value from a wire `StatusResponse`, ignoring its `error`
/// field (the caller inspects that separately). An unknown state string maps to
/// the default, [`ModuleState::Disabled`].
pub fn status_from_proto(response: &pb::StatusResponse) -> Status {
    Status {
        state: ModuleState::parse(&response.state).unwrap_or_default(),
        detail: response.detail.clone(),
    }
}

/// Converts a health report to its wire form.
pub fn health_report_to_proto(report: &HealthReport) -> pb::HealthResponse {
    pb::HealthResponse {
        level: report.level.as_i32(),
        message: report.message.clone(),
        checked_at_unix_nano: system_time_to_unix_nano(report.checked_at),
    }
}

/// Converts a wire health response back into a health report.
pub fn health_report_from_proto(response: &pb::HealthResponse) -> HealthReport {
    HealthReport {
        level: HealthLevel::from_i32(response.level),
        message: response.message.clone(),
        checked_at: unix_nano_to_system_time(response.checked_at_unix_nano),
    }
}

/// Converts a command result to a wire `DispatchResponse`. The `error` field is
/// left empty; a failing dispatch carries its error separately.
pub fn command_result_to_proto(result: &CommandResult) -> pb::DispatchResponse {
    pb::DispatchResponse {
        output: result.output.clone(),
        json: result.json.clone(),
        exit_code: result.exit_code,
        error: String::new(),
    }
}

/// Extracts the result value from a wire `DispatchResponse`, ignoring its
/// `error` field (the caller inspects that separately).
pub fn command_result_from_proto(response: &pb::DispatchResponse) -> CommandResult {
    CommandResult {
        output: response.output.clone(),
        json: response.json.clone(),
        exit_code: response.exit_code,
    }
}

/// Converts an event to a wire `PublishEventRequest`, stamping the api_version.
pub fn event_to_proto(event: &Event) -> pb::PublishEventRequest {
    pb::PublishEventRequest {
        api_version: API_VERSION.to_string(),
        module: event.module.clone(),
        r#type: event.event_type.as_str().to_string(),
        message: event.message.clone(),
        at_unix_nano: system_time_to_unix_nano(event.at),
        fields: event.fields.clone(),
    }
}

/// Converts a wire `PublishEventRequest` back into an event. An unknown type
/// string maps to the default, [`EventType::StateChanged`].
pub fn event_from_proto(request: &pb::PublishEventRequest) -> Event {
    Event {
        module: request.module.clone(),
        event_type: EventType::parse(&request.r#type).unwrap_or_default(),
        message: request.message.clone(),
        at: unix_nano_to_system_time(request.at_unix_nano),
        fields: request.fields.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn unix_nano_round_trips_across_the_epoch() {
        let cases = [
            0_i64,
            1_600_000_000_123_456_789,
            -1_000_000_000,
            i64::MIN + 1,
        ];
        for nanos in cases {
            let time = unix_nano_to_system_time(nanos);
            assert_eq!(system_time_to_unix_nano(time), nanos);
        }
    }

    #[test]
    fn flag_spec_round_trips() {
        let original = FlagSpec {
            name: "endpoint".to_string(),
            shorthand: "e".to_string(),
            usage: "the endpoint".to_string(),
            default: "us-east".to_string(),
            flag_type: FlagType::Int,
        };
        let restored = flag_spec_from_proto(&flag_spec_to_proto(&original));
        assert_eq!(restored, original);
    }

    #[test]
    fn command_spec_round_trips_including_nested_subcommands() {
        let leaf = CommandSpec {
            name: "query".to_string(),
            use_line: "query <domain>".to_string(),
            short: "run a query".to_string(),
            flags: vec![FlagSpec {
                name: "json".to_string(),
                shorthand: String::new(),
                usage: "json output".to_string(),
                default: "false".to_string(),
                flag_type: FlagType::Bool,
            }],
            subcommands: Vec::new(),
            tray: true,
            min_args: 1,
            max_args: -1,
        };
        let root = CommandSpec {
            name: "squawk".to_string(),
            use_line: "squawk".to_string(),
            short: "DoH client".to_string(),
            flags: Vec::new(),
            subcommands: vec![leaf],
            tray: false,
            min_args: 0,
            max_args: 0,
        };
        let restored = command_spec_from_proto(&command_spec_to_proto(&root));
        assert_eq!(restored, root);
    }

    #[test]
    fn status_round_trips_its_value_fields() {
        let mut detail = HashMap::new();
        detail.insert("tunnel".to_string(), "up".to_string());
        let original = Status {
            state: ModuleState::Running,
            detail,
        };
        let restored = status_from_proto(&status_to_proto(&original));
        assert_eq!(restored, original);
    }

    #[test]
    fn status_from_proto_maps_unknown_state_to_disabled() {
        let wire = pb::StatusResponse {
            state: "levitating".to_string(),
            detail: HashMap::new(),
            error: String::new(),
        };
        assert_eq!(status_from_proto(&wire).state, ModuleState::Disabled);
    }

    #[test]
    fn health_report_round_trips() {
        let original = HealthReport {
            level: HealthLevel::Degraded,
            message: "slow upstream".to_string(),
            checked_at: unix_nano_to_system_time(1_700_000_000_000_000_000),
        };
        let restored = health_report_from_proto(&health_report_to_proto(&original));
        assert_eq!(restored, original);
    }

    #[test]
    fn command_result_round_trips() {
        let original = CommandResult {
            output: "ok".to_string(),
            json: b"{\"ok\":true}".to_vec(),
            exit_code: 3,
        };
        let restored = command_result_from_proto(&command_result_to_proto(&original));
        assert_eq!(restored, original);
    }

    #[test]
    fn event_round_trips_and_stamps_the_api_version() {
        let mut fields = HashMap::new();
        fields.insert("endpoint".to_string(), "us-east".to_string());
        let original = Event {
            module: "tobogganing".to_string(),
            event_type: EventType::Health,
            message: "handshake stale".to_string(),
            at: unix_nano_to_system_time(1_650_000_000_000_000_000),
            fields,
        };
        let wire = event_to_proto(&original);
        assert_eq!(wire.api_version, API_VERSION);
        assert_eq!(event_from_proto(&wire), original);
    }

    #[test]
    fn event_from_proto_maps_unknown_type_to_state_changed() {
        let wire = pb::PublishEventRequest {
            api_version: API_VERSION.to_string(),
            module: "squawk".to_string(),
            r#type: "meltdown".to_string(),
            message: String::new(),
            at_unix_nano: 0,
            fields: HashMap::new(),
        };
        assert_eq!(event_from_proto(&wire).event_type, EventType::StateChanged);
    }
}
