//! [`ConsoleSink`]: where the integrity loop reports tamper events and
//! polls for a console-recorded deauthorization.
//!
//! **SP2 note**: [`NoopConsoleSink`] is the only implementation today —
//! `report_tamper` does nothing and `poll_deauthorized` always answers
//! `false`. A real HTTP-backed sink (reporting to the Penguin console and
//! polling `TeardownCtx::console_deauthorized`'s source of truth — see
//! `crate::authz`) is an SP2 follow-up, not implemented here. Every caller
//! goes through `&dyn ConsoleSink`, so swapping in the real implementation
//! later requires no change on the caller side.

use crate::event::TamperEvent;

/// Where tamper events get reported, and where a console-issued
/// deauthorization decision is read from.
///
/// `Send + Sync` so a single sink can be shared across the daemon's
/// integrity-loop task and any other caller without extra synchronization.
pub trait ConsoleSink: Send + Sync {
    /// Reports one tamper event to the console. Implementations must never
    /// panic — a reporting failure (network error, console unreachable)
    /// must not take down the caller's integrity loop; log and return.
    fn report_tamper(&self, event: &TamperEvent);

    /// Polls whether the console has recorded `node_id` as deauthorized
    /// (i.e. removal/teardown has been centrally approved — see
    /// `crate::authz::TeardownAuthz::NodeDeauthorized`). Returns `false` on
    /// any failure to reach the console, never panics: an unreachable
    /// console must never be misread as "deauthorized."
    fn poll_deauthorized(&self, node_id: &str) -> bool;
}

/// The no-op [`ConsoleSink`]: `report_tamper` does nothing,
/// `poll_deauthorized` always answers `false`. See this module's doc for why
/// — SP2 provisions the real HTTP-backed sink.
pub struct NoopConsoleSink;

impl ConsoleSink for NoopConsoleSink {
    fn report_tamper(&self, _event: &TamperEvent) {}

    fn poll_deauthorized(&self, _node_id: &str) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TamperEventKind;

    #[test]
    fn noop_sink_never_reports_deauthorized_and_never_panics() {
        let sink = NoopConsoleSink;
        let event = TamperEvent {
            node_id: "n-1".to_string(),
            kind: TamperEventKind::BinaryModified,
            path: "bin/penguind".to_string(),
            expected_hash: "a".repeat(64),
            actual_hash: Some("b".repeat(64)),
            ts_unix: 0,
            remediation: "restored from protected copy".to_string(),
        };
        sink.report_tamper(&event); // must not panic
        assert!(!sink.poll_deauthorized("n-1"));
    }
}
