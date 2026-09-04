//! The CLI command tree a module declares, plus the result of running one.
//!
//! These are pure data: the `penguin` CLI renders a [`CommandSpec`] tree into
//! clap commands and routes execution back to [`crate::Module::dispatch`]. The
//! CLI never links module code, so everything here must be serialisable across
//! the daemon boundary.

/// One command in a module's command tree.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandSpec {
    /// The command token as typed, e.g. `"connect"` or `"query"`.
    pub name: String,
    /// The one-line usage string, e.g. `"query <domain>"`. Named `use_line`
    /// because `use` is a Rust keyword; it maps to the proto `use` field.
    pub use_line: String,
    /// The short help summary.
    pub short: String,
    /// The command's own flags.
    pub flags: Vec<FlagSpec>,
    /// Nested subcommands; leaf commands are the dispatchable ones.
    pub subcommands: Vec<CommandSpec>,
    /// Whether this command is surfaced as a tray menu action.
    pub tray: bool,
    /// Minimum positional argument count.
    pub min_args: i32,
    /// Maximum positional argument count (`-1` means unlimited).
    pub max_args: i32,
}

/// A single flag on a command.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlagSpec {
    /// The long flag name, e.g. `"endpoint"`.
    pub name: String,
    /// The optional single-character shorthand, e.g. `"e"`.
    pub shorthand: String,
    /// The help text for the flag.
    pub usage: String,
    /// The default value rendered as a string.
    pub default: String,
    /// The value type the CLI parses this flag as.
    pub flag_type: FlagType,
}

/// The value type of a flag, used to pick the CLI value parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlagType {
    /// A free-form string flag (the default).
    #[default]
    String,
    /// A boolean switch.
    Bool,
    /// An integer flag.
    Int,
}

impl FlagType {
    /// Returns the wire string for this type (`"string"`, `"bool"`, `"int"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            FlagType::String => "string",
            FlagType::Bool => "bool",
            FlagType::Int => "int",
        }
    }

    /// Parses a wire string back into a [`FlagType`].
    ///
    /// An empty or unrecognised value maps to [`FlagType::String`], matching the
    /// Go zero-value behaviour where an unset flag type defaults to string.
    pub fn parse(value: &str) -> FlagType {
        match value {
            "bool" => FlagType::Bool,
            "int" => FlagType::Int,
            _ => FlagType::String,
        }
    }
}

/// The outcome of a [`crate::Module::dispatch`] invocation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandResult {
    /// Human-readable text for the terminal.
    pub output: String,
    /// Optional machine-readable payload (used by `--json`); empty means none.
    pub json: Vec<u8>,
    /// The process exit code the CLI should use (`0` = success).
    pub exit_code: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_type_round_trips_through_its_wire_string() {
        let all = [FlagType::String, FlagType::Bool, FlagType::Int];
        for want in all {
            let got = FlagType::parse(want.as_str());
            assert_eq!(got, want);
        }
    }

    #[test]
    fn flag_type_unknown_and_empty_default_to_string() {
        assert_eq!(FlagType::parse(""), FlagType::String);
        assert_eq!(FlagType::parse("float"), FlagType::String);
    }

    #[test]
    fn flag_type_default_is_string() {
        assert_eq!(FlagType::default(), FlagType::String);
    }

    #[test]
    fn command_result_default_is_empty_success() {
        let result = CommandResult::default();
        assert_eq!(result.output, "");
        assert!(result.json.is_empty());
        assert_eq!(result.exit_code, 0);
    }
}
