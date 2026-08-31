//! [`TamperKind`] and [`TamperFinding`]: classifications of files that
//! failed integrity checks — modified, missing, or corrupted — and the
//! verification result from [`crate::check`].
//!
//! Task 10: [`TamperEvent`]/[`TamperEventKind`] — the reportable record built
//! from a [`TamperFinding`] once it has actually been dealt with (an attempt
//! made to heal it), for `crate::monitor::scan_heal_report` to hand to a
//! `crate::ConsoleSink` and the daemon to hand to telemetry.

/// Classification of how a file failed an integrity check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TamperKind {
    /// Binary executable (daemon or main binary) has been modified.
    BinaryModified,
    /// Systemd unit file (`.service`) has been modified.
    UnitModified,
    /// Configuration file has been modified.
    ConfigModified,
    /// Expected file is missing from disk.
    FileMissing,
}

/// A file that failed an integrity check: its path, the kind of failure,
/// the expected SHA-256 hash, and the actual hash (if the file was readable).
#[derive(Debug, Clone)]
pub struct TamperFinding {
    /// Path relative to the agent install root, e.g. `"bin/penguind"`.
    pub path: String,
    /// The kind of tampering detected.
    pub kind: TamperKind,
    /// Expected SHA-256 hash of the file's contents, as lower-case hex.
    pub expected_sha256: String,
    /// Actual SHA-256 hash if the file was readable; `None` if missing or
    /// unreadable.
    pub actual_sha256: Option<String>,
}

/// Classification of a reportable tamper event. A superset of
/// [`TamperKind`]: the first four variants map 1:1 from an on-disk
/// [`TamperFinding`] via [`TamperEvent::from_finding`]; [`ProcessKilled`] and
/// [`UnauthorizedUninstall`] have no [`TamperKind`] equivalent — they exist
/// for the watchdog (`penguind watchdog` relaunching a killed peer) and
/// teardown (`penguind service uninstall` refused) paths to report through
/// the same event shape, not for anything [`crate::check`] itself produces.
///
/// [`ProcessKilled`]: TamperEventKind::ProcessKilled
/// [`UnauthorizedUninstall`]: TamperEventKind::UnauthorizedUninstall
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TamperEventKind {
    /// Binary executable (daemon or main binary) was modified.
    BinaryModified,
    /// Systemd unit file (`.service`) was modified.
    UnitModified,
    /// Configuration file was modified.
    ConfigModified,
    /// Expected file was missing from disk.
    FileMissing,
    /// A protected process (daemon or watchdog peer) was killed.
    ProcessKilled,
    /// An uninstall/teardown attempt was refused as unauthorized.
    UnauthorizedUninstall,
}

impl From<TamperKind> for TamperEventKind {
    /// The only four [`TamperKind`] variants map onto their identically
    /// named [`TamperEventKind`] counterpart — see that enum's doc for why
    /// it carries two more variants this conversion never produces.
    fn from(kind: TamperKind) -> Self {
        match kind {
            TamperKind::BinaryModified => TamperEventKind::BinaryModified,
            TamperKind::UnitModified => TamperEventKind::UnitModified,
            TamperKind::ConfigModified => TamperEventKind::ConfigModified,
            TamperKind::FileMissing => TamperEventKind::FileMissing,
        }
    }
}

/// A reportable tamper event: what was found, on which node, when, and what
/// (if anything) was done about it. Built by [`TamperEvent::from_finding`]
/// from a [`TamperFinding`] the integrity loop already attempted to heal —
/// see `crate::monitor::scan_heal_report`, the sole production constructor
/// path — and handed to a `crate::ConsoleSink` plus the daemon's telemetry
/// handle.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TamperEvent {
    /// This node's identifier, so a fleet-wide console can attribute the
    /// event to the right endpoint.
    pub node_id: String,
    /// The kind of tampering (or, for the watchdog/teardown paths, the kind
    /// of unauthorized action) this event reports.
    pub kind: TamperEventKind,
    /// Path relative to the agent install root, e.g. `"bin/penguind"`.
    pub path: String,
    /// Expected SHA-256 hash of the file's contents, as lower-case hex.
    pub expected_hash: String,
    /// Actual SHA-256 hash if the file was readable at detection time;
    /// `None` if missing or unreadable.
    pub actual_hash: Option<String>,
    /// Unix timestamp (seconds) this event was generated, as supplied by
    /// the caller — never read from the system clock here, so this type
    /// stays pure and deterministic for tests. See
    /// `crate::monitor::scan_heal_report`'s doc for why.
    pub ts_unix: i64,
    /// Human-readable description of what remediation was attempted (e.g.
    /// `"restored from protected copy"`, or a heal failure's reason).
    pub remediation: String,
}

impl TamperEvent {
    /// Builds a [`TamperEvent`] from an on-disk [`TamperFinding`], the
    /// reporting node's ID, a caller-supplied timestamp, and a description
    /// of what remediation was attempted for it.
    pub fn from_finding(
        finding: &TamperFinding,
        node_id: &str,
        ts_unix: i64,
        remediation: &str,
    ) -> TamperEvent {
        TamperEvent {
            node_id: node_id.to_string(),
            kind: TamperEventKind::from(finding.kind),
            path: finding.path.clone(),
            expected_hash: finding.expected_sha256.clone(),
            actual_hash: finding.actual_sha256.clone(),
            ts_unix,
            remediation: remediation.to_string(),
        }
    }
}
