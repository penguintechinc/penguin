//! [`TamperKind`] and [`TamperFinding`]: classifications of files that
//! failed integrity checks — modified, missing, or corrupted — and the
//! verification result from [`crate::check`].

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
