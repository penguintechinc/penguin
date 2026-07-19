//! Renders CLI output text, ported field-for-field and column-for-column
//! from `go-client/cmd/penguin/main.go`'s `Printf`/`Println` calls. Every
//! function here returns an owned `String` — the bin decides where it goes
//! (stdout, a specific writer in a test) — so none of this needs any I/O to
//! exercise.

use crate::pb;
use crate::timestamp::format_log_timestamp;

/// `penguin version` — the CLI's own version line. Matches
/// `fmt.Printf("penguin version %s\n", version.Version)`.
pub fn render_local_version(version: &str) -> String {
    format!("penguin version {version}\n")
}

/// `penguin version` — the daemon's version line, printed only once the
/// `Version` RPC has succeeded. Matches
/// `fmt.Printf("penguind version %s\n", resp.DaemonVersion)`.
pub fn render_daemon_version(daemon_version: &str) -> String {
    format!("penguind version {daemon_version}\n")
}

/// `penguin modules` with no modules loaded. Matches
/// `fmt.Println("No modules loaded")`.
pub const NO_MODULES_LOADED: &str = "No modules loaded\n";

/// `penguin modules`'s table, including the "no modules" fallback — the
/// column widths and separator length are Go's own
/// (`"%-14s %-10s %-10s %-42s %s\n"`, header row, then a 100-dash rule).
pub fn render_modules_table(modules: &[pb::ModuleSummary]) -> String {
    if modules.is_empty() {
        return NO_MODULES_LOADED.to_string();
    }
    let mut out = format!(
        "{:<14} {:<10} {:<10} {:<42} {}\n",
        "NAME", "STATE", "VERSION", "DESCRIPTION", "LICENSE FLAG"
    );
    out.push_str(&"-".repeat(100));
    out.push('\n');
    for module in modules {
        out.push_str(&format!(
            "{:<14} {:<10} {:<10} {:<42} {}\n",
            module.name, module.state, module.version, module.description, module.license_feature
        ));
    }
    out
}

/// `penguin status`'s header line. Matches
/// `fmt.Printf("Daemon version: %s\n\n", resp.DaemonVersion)`.
pub fn render_status_header(daemon_version: &str) -> String {
    format!("Daemon version: {daemon_version}\n\n")
}

/// `penguin status` with no modules. Matches `fmt.Println("No modules")`.
pub const NO_MODULES: &str = "No modules\n";

/// `penguin status`'s table, including the "no modules" fallback — column
/// widths and separator length are Go's own
/// (`"%-20s %-15s %-15s %s\n"`, header row, then a 70-dash rule). Callers
/// prepend [`render_status_header`] themselves so the header renders even
/// when `--json` is not requested and the module list happens to be empty.
pub fn render_status_table(modules: &[pb::ModuleStatus]) -> String {
    if modules.is_empty() {
        return NO_MODULES.to_string();
    }
    let mut out = format!(
        "{:<20} {:<15} {:<15} {}\n",
        "NAME", "STATE", "HEALTH", "MESSAGE"
    );
    out.push_str(&"-".repeat(70));
    out.push('\n');
    for module in modules {
        out.push_str(&format!(
            "{:<20} {:<15} {:<15} {}\n",
            module.name, module.state, module.health, module.health_message
        ));
    }
    out
}

/// `penguin load <module>` on success. Matches
/// `fmt.Printf("Module %q loaded (state: %s)\n", args[0], resp.State)`.
///
/// Uses Rust's `Debug`-quoted string (`{:?}`) for the `%q`-equivalent
/// escaping. This matches Go's `%q` for every module name that is realistic
/// in practice (alphanumeric plus `-`/`_`); the two only disagree on the
/// escape spelling of exotic control/Unicode characters, which no module
/// name uses — see `docs/PARITY.md`.
pub fn render_load_success(module: &str, state: &str) -> String {
    format!("Module {module:?} loaded (state: {state})\n")
}

/// `penguin unload <module>` on success. Matches
/// `fmt.Printf("Module %q unloaded\n", args[0])`.
pub fn render_unload_success(module: &str) -> String {
    format!("Module {module:?} unloaded\n")
}

/// `penguin logs`'s per-line format. Matches
/// `fmt.Printf("[%s] %s: %s\n", ts, line.Level, line.Message)`, with the
/// timestamp rendered by [`format_log_timestamp`] — see its module doc for
/// the local-vs-UTC divergence from Go.
pub fn render_log_line(line: &pb::LogLine) -> String {
    let ts = format_log_timestamp(line.at_unix_nano);
    format!("[{ts}] {}: {}\n", line.level, line.message)
}

/// `penguin logs` when the daemon reports `TailLogs` as unimplemented.
/// Matches `fmt.Println("TailLogs not implemented yet")`.
pub const TAIL_LOGS_NOT_IMPLEMENTED: &str = "TailLogs not implemented yet\n";

/// `penguin update`'s check-result lines. Matches
/// `fmt.Printf("Current version: %s\n", ...)` followed by
/// `fmt.Printf("Latest version: %s\n", ...)`.
pub fn render_update_check(current_version: &str, latest_version: &str) -> String {
    format!("Current version: {current_version}\nLatest version: {latest_version}\n")
}

/// `penguin update` when no update is available. Matches
/// `fmt.Println("No updates available")`.
pub const NO_UPDATES_AVAILABLE: &str = "No updates available\n";

/// `penguin update`'s interactive confirmation prompt (no trailing newline —
/// Go's `fmt.Print`, not `Println`). Matches
/// `fmt.Print("Apply update? (y/n): ")`.
pub const UPDATE_CONFIRM_PROMPT: &str = "Apply update? (y/n): ";

/// `penguin update` once the update has been applied successfully. Matches
/// `fmt.Println("Update applied successfully")`.
pub const UPDATE_APPLIED_SUCCESS: &str = "Update applied successfully\n";

/// `penguin update` when the daemon reports the apply itself failed. Matches
/// `fmt.Printf("Update failed: %s\n", applyResp.Message)`.
pub fn render_update_failed(message: &str) -> String {
    format!("Update failed: {message}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module_summary(name: &str) -> pb::ModuleSummary {
        pb::ModuleSummary {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: "a module".to_string(),
            state: "running".to_string(),
            external: false,
            license_feature: "free".to_string(),
        }
    }

    #[test]
    fn local_and_daemon_version_lines_match_go_format() {
        assert_eq!(render_local_version("0.2.0"), "penguin version 0.2.0\n");
        assert_eq!(render_daemon_version("0.2.0"), "penguind version 0.2.0\n");
    }

    #[test]
    fn empty_modules_prints_the_no_modules_loaded_line() {
        assert_eq!(render_modules_table(&[]), NO_MODULES_LOADED);
    }

    #[test]
    fn modules_table_has_the_expected_header_and_separator() {
        let table = render_modules_table(&[module_summary("squawk")]);
        let mut lines = table.lines();
        assert_eq!(
            lines.next().unwrap(),
            "NAME           STATE      VERSION    DESCRIPTION                                LICENSE FLAG"
        );
        assert_eq!(lines.next().unwrap(), "-".repeat(100));
        assert_eq!(
            lines.next().unwrap(),
            "squawk         running    1.0.0      a module                                   free"
        );
    }

    #[test]
    fn empty_status_modules_prints_no_modules() {
        assert_eq!(render_status_table(&[]), NO_MODULES);
    }

    #[test]
    fn status_header_matches_go_format_with_blank_line() {
        assert_eq!(render_status_header("0.2.0"), "Daemon version: 0.2.0\n\n");
    }

    #[test]
    fn status_table_has_the_expected_header_and_separator() {
        let module = pb::ModuleStatus {
            name: "squawk".to_string(),
            state: "running".to_string(),
            health: "healthy".to_string(),
            health_message: "ok".to_string(),
            ..Default::default()
        };
        let table = render_status_table(&[module]);
        let mut lines = table.lines();
        assert_eq!(
            lines.next().unwrap(),
            "NAME                 STATE           HEALTH          MESSAGE"
        );
        assert_eq!(lines.next().unwrap(), "-".repeat(70));
        assert_eq!(
            lines.next().unwrap(),
            "squawk               running         healthy         ok"
        );
    }

    #[test]
    fn load_and_unload_success_messages_quote_the_module_name() {
        assert_eq!(
            render_load_success("squawk", "running"),
            "Module \"squawk\" loaded (state: running)\n"
        );
        assert_eq!(
            render_unload_success("squawk"),
            "Module \"squawk\" unloaded\n"
        );
    }

    #[test]
    fn log_line_renders_timestamp_level_and_message() {
        let line = pb::LogLine {
            at_unix_nano: 0,
            level: "info".to_string(),
            message: "started".to_string(),
        };
        assert_eq!(
            render_log_line(&line),
            "[1970-01-01 00:00:00] info: started\n"
        );
    }

    #[test]
    fn update_check_and_terminal_messages_match_go_format() {
        assert_eq!(
            render_update_check("1.0.0", "1.1.0"),
            "Current version: 1.0.0\nLatest version: 1.1.0\n"
        );
        assert_eq!(NO_UPDATES_AVAILABLE, "No updates available\n");
        assert_eq!(UPDATE_CONFIRM_PROMPT, "Apply update? (y/n): ");
        assert_eq!(UPDATE_APPLIED_SUCCESS, "Update applied successfully\n");
        assert_eq!(
            render_update_failed("disk full"),
            "Update failed: disk full\n"
        );
    }
}
