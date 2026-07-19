//! Per-module display mapping: turning the daemon's raw [`ModuleState`] and
//! [`HealthLevel`] into text and a [`Severity`] a tray row can render.
//!
//! Both mapping functions are exhaustive `match` expressions with no
//! catch-all arm, on purpose: adding a new [`ModuleState`] or [`HealthLevel`]
//! variant anywhere in the workspace fails this crate's build instead of
//! silently rendering a blank or misleading row.

use penguin_sdk::{CommandSpec, HealthLevel, ModuleState};

use crate::severity::Severity;

/// One module as the daemon reports it, already joined from `ListModules`
/// (name, state), `GetStatus` (health), and `ListCommands` (its command
/// tree) — the shell performs that join before calling [`crate::build_menu`];
/// this crate never talks to the daemon itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleInput {
    /// The module's CLI-visible name (`"squawk"`, `"tobogganing"`).
    pub name: String,
    /// The module's current lifecycle state.
    pub state: ModuleState,
    /// The module's last reported health, or `None` if the daemon has not
    /// probed it yet (distinct from an unhealthy report).
    pub health: Option<HealthLevel>,
    /// A short human-readable explanation accompanying `health`; empty when
    /// there is nothing to add beyond the health level itself.
    pub health_message: String,
    /// The module's declared command tree; only `tray: true` leaves become
    /// menu actions (see [`crate::action`]).
    pub commands: Vec<CommandSpec>,
}

/// A module is considered loaded (running in some form) whenever it is not
/// sitting in [`ModuleState::Disabled`] — mirrors the Go tray's `Loaded`
/// field, used to exclude disabled modules from the overall severity roll-up.
pub fn is_loaded(state: ModuleState) -> bool {
    !matches!(state, ModuleState::Disabled)
}

/// Renders a lifecycle state as the text a tray row shows.
pub fn state_text(state: ModuleState) -> &'static str {
    match state {
        ModuleState::Disabled => "Disabled",
        ModuleState::Initializing => "Starting…",
        ModuleState::Running => "Running",
        ModuleState::Degraded => "Degraded",
        ModuleState::Stopping => "Stopping…",
        ModuleState::Stopped => "Stopped",
        ModuleState::Failed => "Failed",
    }
}

/// Maps a lifecycle state to its own severity signal. This is independent of
/// (and combined with, see [`module_severity`]) the module's reported health:
/// a module can be [`ModuleState::Failed`] before any health probe has run.
pub fn state_severity(state: ModuleState) -> Severity {
    match state {
        ModuleState::Running => Severity::Ok,
        ModuleState::Degraded => Severity::Warn,
        ModuleState::Failed => Severity::Bad,
        ModuleState::Disabled
        | ModuleState::Initializing
        | ModuleState::Stopping
        | ModuleState::Stopped => Severity::Unknown,
    }
}

/// Renders a health level as the text a tray row shows.
pub fn health_text(level: HealthLevel) -> &'static str {
    match level {
        HealthLevel::Healthy => "Healthy",
        HealthLevel::Degraded => "Degraded",
        HealthLevel::Unhealthy => "Unhealthy",
    }
}

/// Maps a health level to its severity signal.
pub fn health_severity(level: HealthLevel) -> Severity {
    match level {
        HealthLevel::Healthy => Severity::Ok,
        HealthLevel::Degraded => Severity::Warn,
        HealthLevel::Unhealthy => Severity::Bad,
    }
}

/// Renders an optional health reading (`None` means "not probed yet") as tray
/// row text.
pub fn module_health_text(health: Option<HealthLevel>) -> &'static str {
    let Some(level) = health else {
        return "Unknown";
    };
    health_text(level)
}

/// Maps an optional health reading to its severity signal.
pub fn module_health_severity(health: Option<HealthLevel>) -> Severity {
    let Some(level) = health else {
        return Severity::Unknown;
    };
    health_severity(level)
}

/// The single severity a module's tray row is painted with: the worse of its
/// lifecycle-state severity and its health severity, so a module that has
/// already failed reads as urgent even before a health probe confirms it.
pub fn module_severity(state: ModuleState, health: Option<HealthLevel>) -> Severity {
    crate::severity::worse(state_severity(state), module_health_severity(health))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_STATES: [ModuleState; 7] = [
        ModuleState::Disabled,
        ModuleState::Initializing,
        ModuleState::Running,
        ModuleState::Degraded,
        ModuleState::Stopping,
        ModuleState::Stopped,
        ModuleState::Failed,
    ];

    const ALL_HEALTH: [HealthLevel; 3] = [
        HealthLevel::Healthy,
        HealthLevel::Degraded,
        HealthLevel::Unhealthy,
    ];

    #[test]
    fn every_module_state_has_display_text_and_severity() {
        let want = [
            (ModuleState::Disabled, "Disabled", Severity::Unknown),
            (ModuleState::Initializing, "Starting…", Severity::Unknown),
            (ModuleState::Running, "Running", Severity::Ok),
            (ModuleState::Degraded, "Degraded", Severity::Warn),
            (ModuleState::Stopping, "Stopping…", Severity::Unknown),
            (ModuleState::Stopped, "Stopped", Severity::Unknown),
            (ModuleState::Failed, "Failed", Severity::Bad),
        ];
        for (state, text, severity) in want {
            assert_eq!(state_text(state), text, "state_text({state:?})");
            assert_eq!(state_severity(state), severity, "state_severity({state:?})");
        }
        // Guard against the table above silently drifting from the real
        // variant set if a new ModuleState is ever added.
        assert_eq!(want.len(), ALL_STATES.len());
    }

    #[test]
    fn every_health_level_has_display_text_and_severity() {
        let want = [
            (HealthLevel::Healthy, "Healthy", Severity::Ok),
            (HealthLevel::Degraded, "Degraded", Severity::Warn),
            (HealthLevel::Unhealthy, "Unhealthy", Severity::Bad),
        ];
        for (level, text, severity) in want {
            assert_eq!(health_text(level), text, "health_text({level:?})");
            assert_eq!(
                health_severity(level),
                severity,
                "health_severity({level:?})"
            );
        }
        assert_eq!(want.len(), ALL_HEALTH.len());
    }

    #[test]
    fn unprobed_health_renders_as_unknown() {
        assert_eq!(module_health_text(None), "Unknown");
        assert_eq!(module_health_severity(None), Severity::Unknown);
    }

    #[test]
    fn probed_health_passes_through() {
        for level in ALL_HEALTH {
            assert_eq!(module_health_text(Some(level)), health_text(level));
            assert_eq!(module_health_severity(Some(level)), health_severity(level));
        }
    }

    #[test]
    fn module_severity_takes_the_worse_of_state_and_health() {
        // Failed state outranks a merely-degraded health reading.
        assert_eq!(
            module_severity(ModuleState::Failed, Some(HealthLevel::Degraded)),
            Severity::Bad
        );
        // Running state with no health probe yet stays Unknown, not falsely Ok.
        assert_eq!(
            module_severity(ModuleState::Running, None),
            Severity::Unknown
        );
        // Running and healthy is the only way to reach Ok.
        assert_eq!(
            module_severity(ModuleState::Running, Some(HealthLevel::Healthy)),
            Severity::Ok
        );
    }

    #[test]
    fn only_disabled_state_is_unloaded() {
        for state in ALL_STATES {
            assert_eq!(
                is_loaded(state),
                state != ModuleState::Disabled,
                "{state:?}"
            );
        }
    }
}
