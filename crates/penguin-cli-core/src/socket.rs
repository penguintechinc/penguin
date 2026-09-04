//! The daemon's control-socket path: its default and how a raw argument list
//! overrides it before any daemon connection exists to ask for anything else.

/// The default control-socket path, ported verbatim from Go's
/// `defaultSocket()` (`go-client/cmd/penguin/main.go`). Go returns this same
/// literal unconditionally, including on Windows — its comment notes the
/// value is "ignored and replaced with `\\.\pipe\penguind` internally" by
/// the platform-specific dialer rather than varying the string itself. Kept
/// here as the constant Go actually returns, not as platform-conditional
/// logic, since Go never varied it either.
pub const DEFAULT_SOCKET_PATH: &str = "/run/penguin/penguind.sock";

/// Scans a raw argument list for a `--socket`/`-socket` override, accepting
/// both `--socket VALUE` and `--socket=VALUE` spellings (and their
/// single-dash equivalents, which Go's `flag` package treats identically).
///
/// This exists because the socket path must be known *before* the daemon can
/// be dialed to fetch `ListCommands` and build the rest of the command tree
/// — long before a full clap parse of that (not yet built) tree is possible.
///
/// Deliberate divergence from Go: `run()`'s manual pre-scan loop
/// (`go-client/cmd/penguin/main.go`) stops at the first argument that does
/// not start with `-`, so `--socket` is only recognised when it appears
/// *before* the first subcommand — `penguin modules --socket /tmp/x.sock`
/// does not work in Go. This scans the whole argument list instead, so the
/// flag works in either position; see `docs/PARITY.md`. The last occurrence
/// wins, matching the behaviour of Go's `flag` package when a flag is passed
/// more than once.
pub fn extract_socket_override(args: &[String]) -> Option<String> {
    let mut found = None;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if let Some(value) = arg
            .strip_prefix("--socket=")
            .or_else(|| arg.strip_prefix("-socket="))
        {
            found = Some(value.to_string());
        } else if (arg == "--socket" || arg == "-socket")
            && let Some(value) = args.get(i + 1)
        {
            found = Some(value.clone());
            i += 1;
        }
        i += 1;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_socket_path_matches_go() {
        assert_eq!(DEFAULT_SOCKET_PATH, "/run/penguin/penguind.sock");
    }

    #[test]
    fn no_override_present_returns_none() {
        let args: Vec<String> = ["modules"].iter().map(|s| s.to_string()).collect();
        assert_eq!(extract_socket_override(&args), None);
    }

    #[test]
    fn space_separated_override_is_found() {
        let args: Vec<String> = ["--socket", "/tmp/x.sock", "modules"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            extract_socket_override(&args),
            Some("/tmp/x.sock".to_string())
        );
    }

    #[test]
    fn equals_separated_override_is_found() {
        let args: Vec<String> = ["--socket=/tmp/x.sock", "modules"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            extract_socket_override(&args),
            Some("/tmp/x.sock".to_string())
        );
    }

    #[test]
    fn single_dash_spelling_is_accepted() {
        let args: Vec<String> = ["-socket", "/tmp/x.sock"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            extract_socket_override(&args),
            Some("/tmp/x.sock".to_string())
        );
    }

    #[test]
    fn override_after_the_subcommand_is_still_found() {
        // The deliberate divergence from Go documented above.
        let args: Vec<String> = ["modules", "--socket", "/tmp/x.sock"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            extract_socket_override(&args),
            Some("/tmp/x.sock".to_string())
        );
    }

    #[test]
    fn last_occurrence_wins() {
        let args: Vec<String> = ["--socket", "/tmp/a.sock", "--socket", "/tmp/b.sock"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            extract_socket_override(&args),
            Some("/tmp/b.sock".to_string())
        );
    }

    #[test]
    fn dangling_socket_flag_with_no_value_is_ignored() {
        let args: Vec<String> = ["--socket"].iter().map(|s| s.to_string()).collect();
        assert_eq!(extract_socket_override(&args), None);
    }
}
