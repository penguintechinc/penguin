//! Turns an RPC failure into the text the CLI prints, mirroring — and
//! deliberately broadening — Go's `friendly()` helper
//! (`go-client/cmd/penguin/main.go`).
//!
//! # Divergence from Go
//!
//! Go's `friendly()` only ever runs on the error `root.Execute()` returns,
//! which only carries the raw `*status.Status` for the *static* verbs
//! (`version`, `modules`, `load`, …) — their `RunE` functions return the gRPC
//! error unwrapped. The dynamic dispatch path
//! (`internal/cli.Builder.dispatch`) has its own inline `Unavailable` check
//! that produces a *different*, less specific message (`"daemon
//! unreachable"`, no socket path) and returns a plain `fmt.Errorf`, which
//! `status.FromError` cannot recover a code from — so `friendly()` never
//! actually reprocesses it. The result is that Go shows two different
//! messages for the same failure depending on whether the command that hit
//! it was static or module-provided.
//!
//! Rust applies [`friendly_status_message`] uniformly to every RPC — static
//! verb or dynamic dispatch alike — so a dead daemon always reports the same,
//! more informative message. Documented in `docs/PARITY.md`.

use tonic::{Code, Status};

/// The friendly message the CLI prints when it cannot reach `penguind` at
/// all — ported byte-for-byte from Go's `friendly()`. This exact string is a
/// parity assertion: the M4 cross-implementation gate diffs it against the
/// Go CLI's own output.
pub fn daemon_unreachable_message(socket_path: &str) -> String {
    format!("cannot reach penguind at {socket_path} — is the daemon running?")
}

/// True when `status` is the transport-level "can't reach the server"
/// failure ([`daemon_unreachable_message`]'s trigger condition), rather than
/// an application-level error the daemon actively returned.
pub fn is_unavailable(status: &Status) -> bool {
    status.code() == Code::Unavailable
}

/// Renders any RPC failure as the text the CLI should print: the friendly
/// daemon-down message for [`is_unavailable`] statuses, and the status's own
/// message otherwise (rather than a raw Go-style `rpc error: code = ...
/// desc = ...` dump, which Rust's `tonic::Status` does not produce anyway).
pub fn friendly_status_message(status: &Status, socket_path: &str) -> String {
    if is_unavailable(status) {
        daemon_unreachable_message(socket_path)
    } else {
        status.message().to_string()
    }
}

/// Renders a `LoadModule` failure, ported from `cmdLoad`'s `RunE`
/// (`go-client/cmd/penguin/main.go`): `NotFound` and `PermissionDenied` get
/// their own dedicated wording, `Unavailable` gets the friendly daemon-down
/// message (uniformly with every other RPC — see the module doc), and
/// anything else falls back to the status's own message.
pub fn load_error_message(status: &Status, module: &str, socket_path: &str) -> String {
    match status.code() {
        Code::NotFound => format!("module {module:?} not found"),
        Code::PermissionDenied => format!("license feature required: {}", status.message()),
        _ => friendly_status_message(status, socket_path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_unreachable_message_matches_go_exactly() {
        assert_eq!(
            daemon_unreachable_message("/run/penguin/penguind.sock"),
            "cannot reach penguind at /run/penguin/penguind.sock — is the daemon running?"
        );
    }

    #[test]
    fn unavailable_status_is_detected() {
        let status = Status::unavailable("connection refused");
        assert!(is_unavailable(&status));
    }

    #[test]
    fn non_unavailable_status_is_not_flagged() {
        let status = Status::not_found("module \"foo\" not found");
        assert!(!is_unavailable(&status));
    }

    #[test]
    fn friendly_message_prefers_the_daemon_down_text_for_unavailable() {
        let status = Status::unavailable("transport error");
        assert_eq!(
            friendly_status_message(&status, "/tmp/d.sock"),
            daemon_unreachable_message("/tmp/d.sock")
        );
    }

    #[test]
    fn friendly_message_passes_other_statuses_through_as_their_own_text() {
        let status = Status::internal("module crashed");
        assert_eq!(
            friendly_status_message(&status, "/tmp/d.sock"),
            "module crashed"
        );
    }

    #[test]
    fn load_error_message_reports_not_found_by_module_name() {
        let status = Status::not_found("ignored");
        assert_eq!(
            load_error_message(&status, "squawk", "/tmp/d.sock"),
            "module \"squawk\" not found"
        );
    }

    #[test]
    fn load_error_message_reports_the_license_feature_on_permission_denied() {
        let status = Status::permission_denied("waddleai");
        assert_eq!(
            load_error_message(&status, "squawk", "/tmp/d.sock"),
            "license feature required: waddleai"
        );
    }

    #[test]
    fn load_error_message_uses_the_friendly_text_when_unavailable() {
        let status = Status::unavailable("down");
        assert_eq!(
            load_error_message(&status, "squawk", "/tmp/d.sock"),
            daemon_unreachable_message("/tmp/d.sock")
        );
    }

    #[test]
    fn load_error_message_falls_back_to_the_status_message_otherwise() {
        let status = Status::internal("boom");
        assert_eq!(load_error_message(&status, "squawk", "/tmp/d.sock"), "boom");
    }
}
