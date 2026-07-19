//! Maps a wire `FlagSpec` to a clap [`Arg`] and back: building the argument
//! definition the tree parses with, and reading the parsed value back out as
//! the string the `Dispatch` RPC carries.
//!
//! Ported from `internal/cli.Builder.buildCommand`'s flag loop and
//! `internal/cli.Builder.dispatch`'s `cmd.Flags().Visit` loop
//! (`go-client/internal/cli/builder.go`) — kept together in one file because
//! they are two halves of the same encoding: whichever [`FlagKind`] a flag
//! builds as is exactly the kind [`collect_flags`] must read it back as.

use std::collections::HashMap;

use clap::parser::ValueSource;
use clap::{Arg, ArgAction, ArgMatches};

use crate::pb;

/// The value kind a `FlagSpec.type` selects, plus the fallback applied to
/// anything else.
///
/// # Divergence from Go
///
/// `Builder.buildCommand`'s flag loop (`go-client/internal/cli/builder.go`)
/// is a `switch` over `"string" | "bool" | "int"` with **no default arm** —
/// verified against `TestBuilderFlagTypes`
/// (`go-client/internal/cli/builder_extended_test.go`), which registers a
/// flag with `Type: "unknown-type"` and never asserts it exists as a
/// registered flag. A module that ships an unrecognised `FlagSpec.type`
/// therefore has that flag silently vanish from the Go CLI entirely — not
/// merely default to string.
///
/// Rust instead falls back to [`FlagKind::String`], matching the convention
/// `penguin_sdk::command::FlagType::parse` already documents on the
/// module-authoring side of this same wire type ("An empty or unrecognised
/// value maps to `FlagType::String`, matching the Go zero-value behaviour
/// where an unset flag type defaults to string"). A module author's flag
/// should never disappear without so much as a warning; see
/// `docs/PARITY.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagKind {
    /// A free-form string flag — also the fallback for an unrecognised type.
    String,
    /// A boolean switch.
    Bool,
    /// An integer flag.
    Int,
}

/// Classifies a wire `FlagSpec.type` string into the [`FlagKind`] that
/// decides both how the clap [`Arg`] is built and how its value is read back.
pub fn flag_kind(type_name: &str) -> FlagKind {
    match type_name {
        "bool" => FlagKind::Bool,
        "int" => FlagKind::Int,
        _ => FlagKind::String,
    }
}

/// Builds the clap [`Arg`] for one `FlagSpec`, honouring its shorthand,
/// usage text, default, and type.
///
/// Bool flags use `num_args(0..=1)` with `default_missing_value("true")`
/// rather than clap's usual `ArgAction::SetTrue`, so that both bare `--flag`
/// (sets true) and explicit `--flag=false` work — matching pflag's `BoolP`,
/// which supports overriding a `true` default back to `false` the same way.
pub fn build_flag_arg(flag: &pb::FlagSpec) -> Arg {
    let mut arg = Arg::new(flag.name.clone()).long(flag.name.clone());
    if let Some(short) = flag.shorthand.chars().next() {
        arg = arg.short(short);
    }
    if !flag.usage.is_empty() {
        arg = arg.help(flag.usage.clone());
    }

    match flag_kind(&flag.r#type) {
        FlagKind::Bool => {
            let default = if flag.default == "true" {
                "true"
            } else {
                "false"
            };
            arg.num_args(0..=1)
                .default_missing_value("true")
                .default_value(default)
                .action(ArgAction::Set)
                .value_parser(clap::value_parser!(bool))
        }
        FlagKind::Int => {
            // strconv.Atoi's error is discarded in Go, leaving the Go
            // zero-value (0); `unwrap_or(0)` reproduces that exactly.
            let default = flag.default.parse::<i64>().unwrap_or(0).to_string();
            arg.num_args(1)
                .default_value(default)
                .action(ArgAction::Set)
                .value_parser(clap::value_parser!(i64))
        }
        FlagKind::String => arg
            .num_args(1)
            .default_value(flag.default.clone())
            .action(ArgAction::Set)
            .value_parser(clap::value_parser!(String)),
    }
}

/// Reads every *explicitly set* flag back off `matches` as the string the
/// `Dispatch` RPC's `flags` map carries, keyed by flag name.
///
/// Only flags the user actually typed are included — a flag left at its
/// default is omitted entirely, matching pflag's `Flags().Visit`, which
/// (per its own doc) "visits the command-line flags in lexicographical order
/// ... but only those that have been set". The daemon-side module applies
/// its own defaults for anything absent from the map.
pub fn collect_flags(flag_specs: &[pb::FlagSpec], matches: &ArgMatches) -> HashMap<String, String> {
    let mut flags = HashMap::with_capacity(flag_specs.len());
    for flag in flag_specs {
        if matches.value_source(&flag.name) != Some(ValueSource::CommandLine) {
            continue;
        }
        let value = match flag_kind(&flag.r#type) {
            FlagKind::Bool => matches.get_one::<bool>(&flag.name).map(bool::to_string),
            FlagKind::Int => matches.get_one::<i64>(&flag.name).map(i64::to_string),
            FlagKind::String => matches.get_one::<String>(&flag.name).cloned(),
        };
        if let Some(value) = value {
            flags.insert(flag.name.clone(), value);
        }
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Command;

    fn flag(name: &str, kind: &str, default: &str) -> pb::FlagSpec {
        pb::FlagSpec {
            name: name.to_string(),
            shorthand: String::new(),
            usage: format!("usage for {name}"),
            default: default.to_string(),
            r#type: kind.to_string(),
        }
    }

    #[test]
    fn flag_kind_maps_known_types() {
        assert_eq!(flag_kind("string"), FlagKind::String);
        assert_eq!(flag_kind("bool"), FlagKind::Bool);
        assert_eq!(flag_kind("int"), FlagKind::Int);
    }

    #[test]
    fn flag_kind_falls_back_to_string_for_unknown_types() {
        assert_eq!(flag_kind(""), FlagKind::String);
        assert_eq!(flag_kind("float"), FlagKind::String);
        assert_eq!(flag_kind("unknown-type"), FlagKind::String);
    }

    #[test]
    fn unset_flags_are_excluded_from_the_collected_map() {
        let specs = vec![flag("name", "string", "default-name")];
        let cmd = Command::new("test").arg(build_flag_arg(&specs[0]));
        let matches = cmd.try_get_matches_from(["test"]).expect("parse");

        let flags = collect_flags(&specs, &matches);
        assert!(
            flags.is_empty(),
            "default-valued flag must not be forwarded"
        );
    }

    #[test]
    fn explicitly_set_string_flag_is_collected() {
        let specs = vec![flag("name", "string", "default-name")];
        let cmd = Command::new("test").arg(build_flag_arg(&specs[0]));
        let matches = cmd
            .try_get_matches_from(["test", "--name", "custom"])
            .expect("parse");

        let flags = collect_flags(&specs, &matches);
        assert_eq!(flags.get("name"), Some(&"custom".to_string()));
    }

    #[test]
    fn bare_bool_flag_sets_true() {
        let specs = vec![flag("verbose", "bool", "false")];
        let cmd = Command::new("test").arg(build_flag_arg(&specs[0]));
        let matches = cmd
            .try_get_matches_from(["test", "--verbose"])
            .expect("parse");

        let flags = collect_flags(&specs, &matches);
        assert_eq!(flags.get("verbose"), Some(&"true".to_string()));
    }

    #[test]
    fn bool_flag_can_override_a_true_default_back_to_false() {
        let specs = vec![flag("enabled", "bool", "true")];
        let cmd = Command::new("test").arg(build_flag_arg(&specs[0]));
        let matches = cmd
            .try_get_matches_from(["test", "--enabled=false"])
            .expect("parse");

        let flags = collect_flags(&specs, &matches);
        assert_eq!(flags.get("enabled"), Some(&"false".to_string()));
    }

    #[test]
    fn int_flag_is_collected_as_decimal_string() {
        let specs = vec![flag("count", "int", "0")];
        let cmd = Command::new("test").arg(build_flag_arg(&specs[0]));
        let matches = cmd
            .try_get_matches_from(["test", "--count", "42"])
            .expect("parse");

        let flags = collect_flags(&specs, &matches);
        assert_eq!(flags.get("count"), Some(&"42".to_string()));
    }

    #[test]
    fn unparsable_int_default_falls_back_to_zero() {
        let specs = [flag("count", "int", "not-a-number")];
        let cmd = Command::new("test").arg(build_flag_arg(&specs[0]));
        let matches = cmd.try_get_matches_from(["test"]).expect("parse");

        assert_eq!(matches.get_one::<i64>("count"), Some(&0));
    }

    #[test]
    fn shorthand_is_usable() {
        let mut spec = flag("verbose", "bool", "false");
        spec.shorthand = "v".to_string();
        let cmd = Command::new("test").arg(build_flag_arg(&spec));
        let matches = cmd.try_get_matches_from(["test", "-v"]).expect("parse");

        assert_eq!(matches.get_one::<bool>("verbose"), Some(&true));
    }

    #[test]
    fn unknown_flag_type_is_still_registered_as_a_string_flag() {
        let specs = vec![flag("odd", "unknown-type", "fallback")];
        let cmd = Command::new("test").arg(build_flag_arg(&specs[0]));
        let matches = cmd
            .try_get_matches_from(["test", "--odd", "value"])
            .expect("parse");

        let flags = collect_flags(&specs, &matches);
        assert_eq!(flags.get("odd"), Some(&"value".to_string()));
    }
}
