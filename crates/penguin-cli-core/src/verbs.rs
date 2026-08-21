//! Extracts a static verb's flags/positionals from its parsed `ArgMatches`.
//! One function per verb-specific need, kept separate from
//! [`crate::render`] (which turns the *results* of running a verb into text)
//! and [`crate::tree`] (which defines the args these read).

use clap::ArgMatches;

use crate::tree::{FOLLOW_ARG_ID, JSON_ARG_ID, LINES_ARG_ID, MODULE_ARG_ID, YES_ARG_ID};

/// The optional/required `module` positional shared by `load`, `unload`,
/// `status`, and `logs`.
pub fn module_name(matches: &ArgMatches) -> Option<&str> {
    matches.get_one::<String>(MODULE_ARG_ID).map(String::as_str)
}

/// The global `--json` switch, read by `modules` and `status`.
pub fn json_requested(matches: &ArgMatches) -> bool {
    matches.get_flag(JSON_ARG_ID)
}

/// `penguin logs`'s parsed options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogsOptions {
    /// The optional module to tail; `None` means the daemon's own log.
    pub module: Option<String>,
    /// `--follow`.
    pub follow: bool,
    /// `--lines`, still unvalidated — pass to [`validate_lines`] before use.
    pub lines: i32,
}

/// Reads `penguin logs`'s flags and positional module name off its matches.
pub fn logs_options(matches: &ArgMatches) -> LogsOptions {
    let lines = matches
        .get_one::<i64>(LINES_ARG_ID)
        .copied()
        .and_then(|value| i32::try_from(value).ok())
        // Out-of-i32-range inputs are invalid `--lines` values either way;
        // saturating to i32::MAX (rather than truncating with `as i32`,
        // which could wrap a huge value into a false-positive in-range
        // number) guarantees `validate_lines` still rejects them.
        .unwrap_or(i32::MAX);
    LogsOptions {
        module: module_name(matches).map(str::to_string),
        follow: matches.get_flag(FOLLOW_ARG_ID),
        lines,
    }
}

/// `penguin update`'s `--yes` switch.
pub fn update_yes(matches: &ArgMatches) -> bool {
    matches.get_flag(YES_ARG_ID)
}

/// Validates `--lines`, matching `if lines < 0 || lines > 10000 { return
/// fmt.Errorf("lines must be between 0 and 10000") }` in `cmdLogs`
/// (`go-client/cmd/penguin/main.go`) — the exact error text is a parity
/// assertion.
pub fn validate_lines(lines: i32) -> Result<(), String> {
    if !(0..=10_000).contains(&lines) {
        return Err("lines must be between 0 and 10000".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::build_static_root;

    fn matches_for(argv: &[&str]) -> ArgMatches {
        build_static_root()
            .try_get_matches_from(argv)
            .expect("parse")
    }

    #[test]
    fn module_name_reads_the_positional_when_present() {
        let matches = matches_for(&["pdcli", "load", "squawk"]);
        let (_, sub) = matches.subcommand().expect("load matched");
        assert_eq!(module_name(sub), Some("squawk"));
    }

    #[test]
    fn module_name_is_none_when_omitted() {
        let matches = matches_for(&["pdcli", "status"]);
        let (_, sub) = matches.subcommand().expect("status matched");
        assert_eq!(module_name(sub), None);
    }

    #[test]
    fn json_requested_reflects_the_global_flag() {
        let matches = matches_for(&["pdcli", "--json", "modules"]);
        assert!(json_requested(&matches));

        let matches = matches_for(&["pdcli", "modules"]);
        assert!(!json_requested(&matches));
    }

    #[test]
    fn json_flag_after_the_subcommand_also_works() {
        // The deliberate divergence from Go's stricter placement rule.
        let matches = matches_for(&["pdcli", "modules", "--json"]);
        let (_, sub) = matches.subcommand().expect("modules matched");
        assert!(json_requested(sub));
    }

    #[test]
    fn logs_options_reads_module_follow_and_lines() {
        let matches = matches_for(&["pdcli", "logs", "squawk", "--follow", "--lines", "50"]);
        let (_, sub) = matches.subcommand().expect("logs matched");
        let options = logs_options(sub);
        assert_eq!(
            options,
            LogsOptions {
                module: Some("squawk".to_string()),
                follow: true,
                lines: 50
            }
        );
    }

    #[test]
    fn logs_options_defaults_match_go() {
        let matches = matches_for(&["pdcli", "logs"]);
        let (_, sub) = matches.subcommand().expect("logs matched");
        let options = logs_options(sub);
        assert_eq!(
            options,
            LogsOptions {
                module: None,
                follow: false,
                lines: 10
            }
        );
    }

    #[test]
    fn update_yes_reflects_the_flag() {
        let matches = matches_for(&["pdcli", "update", "--yes"]);
        let (_, sub) = matches.subcommand().expect("update matched");
        assert!(update_yes(sub));

        let matches = matches_for(&["pdcli", "update"]);
        let (_, sub) = matches.subcommand().expect("update matched");
        assert!(!update_yes(sub));
    }

    #[test]
    fn validate_lines_accepts_the_documented_range() {
        assert!(validate_lines(0).is_ok());
        assert!(validate_lines(10_000).is_ok());
        assert!(validate_lines(10).is_ok());
    }

    #[test]
    fn validate_lines_rejects_outside_the_range_with_the_exact_go_text() {
        assert_eq!(
            validate_lines(-1).unwrap_err(),
            "lines must be between 0 and 10000"
        );
        assert_eq!(
            validate_lines(10_001).unwrap_err(),
            "lines must be between 0 and 10000"
        );
    }
}
