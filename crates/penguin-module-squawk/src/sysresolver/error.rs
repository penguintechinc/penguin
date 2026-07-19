//! The error surface every sysresolver operation returns.

/// Failures the DNS resolver state machine and its platform backends can
/// return. Kept as one flat enum (rather than a per-backend error type)
/// because every caller — the state machine, the future `Module` glue,
/// `penguin status` — handles them the same way: log and surface to the
/// operator, never branch on backend identity.
#[derive(Debug, thiserror::Error)]
pub enum SysResolverError {
    /// `apply()` was called with an empty server list.
    #[error("no DNS servers provided")]
    NoServers,

    /// Every configured backend refused (or was unavailable) at once.
    /// Should not happen on a real host — the resolv.conf/networksetup/
    /// netsh fallback always accepts — but a fully-faked backend list in a
    /// test can trigger it deliberately.
    #[error("no DNS backend is available on this platform")]
    NoBackendAvailable,

    /// `restore()` / `recover_from_crash()` was asked to restore but there
    /// is neither an in-memory record nor a marker file. Nothing safe to
    /// revert to: the host's current DNS, whatever `apply()` last set it
    /// to, is left untouched rather than guessing a fallback.
    #[error("no DNS backup available to restore")]
    NoBackupAvailable,

    /// The crash marker existed but could not be parsed as JSON. It has
    /// already been renamed from `original` to `quarantined` (evidence
    /// preserved, but out of the way of every future recovery attempt)
    /// rather than silently treated as "nothing to recover" forever.
    #[error(
        "crash marker at {original} is corrupt and has been quarantined to {quarantined}: {source}"
    )]
    CorruptMarker {
        /// Display-formatted path the marker used to live at.
        original: String,
        /// Display-formatted path it was renamed to.
        quarantined: String,
        #[source]
        source: serde_json::Error,
    },

    /// A filesystem operation failed. `context` is a short human
    /// description of what was being attempted.
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    /// A platform mechanism (D-Bus call, external command, output parse)
    /// failed with a message that doesn't fit the categories above.
    #[error("{0}")]
    Backend(String),
}
