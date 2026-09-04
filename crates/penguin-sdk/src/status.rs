//! A module's self-reported operational state and health.

use std::collections::HashMap;
use std::time::SystemTime;

/// The supervisor-visible lifecycle state of a module.
///
/// This is what `penguin status` and the tray display and what the supervisor
/// drives through its state machine (M2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModuleState {
    /// Configured off; not loaded.
    #[default]
    Disabled,
    /// Loaded and running `Init`.
    Initializing,
    /// Started and operating normally.
    Running,
    /// Running but reporting reduced function.
    Degraded,
    /// Running `Stop`.
    Stopping,
    /// Cleanly stopped.
    Stopped,
    /// Stopped because of an unrecoverable error.
    Failed,
}

impl ModuleState {
    /// Returns the lowercase wire string for this state.
    pub fn as_str(&self) -> &'static str {
        match self {
            ModuleState::Disabled => "disabled",
            ModuleState::Initializing => "initializing",
            ModuleState::Running => "running",
            ModuleState::Degraded => "degraded",
            ModuleState::Stopping => "stopping",
            ModuleState::Stopped => "stopped",
            ModuleState::Failed => "failed",
        }
    }

    /// Parses a wire string into a [`ModuleState`]; unknown values return `None`.
    pub fn parse(value: &str) -> Option<ModuleState> {
        match value {
            "disabled" => Some(ModuleState::Disabled),
            "initializing" => Some(ModuleState::Initializing),
            "running" => Some(ModuleState::Running),
            "degraded" => Some(ModuleState::Degraded),
            "stopping" => Some(ModuleState::Stopping),
            "stopped" => Some(ModuleState::Stopped),
            "failed" => Some(ModuleState::Failed),
            _ => None,
        }
    }
}

/// A module's self-reported state plus small display details.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    /// The current lifecycle state.
    pub state: ModuleState,
    /// Small, non-sensitive key/value pairs for display
    /// (e.g. `"endpoint": "us-east"`, `"tunnel": "up"`).
    pub detail: HashMap<String, String>,
}

/// The graded health of a module, coarser than [`ModuleState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HealthLevel {
    /// Fully operational.
    #[default]
    Healthy,
    /// Operational with reduced function.
    Degraded,
    /// Not operational.
    Unhealthy,
}

impl HealthLevel {
    /// Returns the lowercase wire string for this level.
    pub fn as_str(&self) -> &'static str {
        match self {
            HealthLevel::Healthy => "healthy",
            HealthLevel::Degraded => "degraded",
            HealthLevel::Unhealthy => "unhealthy",
        }
    }

    /// Returns the numeric wire value (`0`/`1`/`2`) used in the proto.
    pub fn as_i32(&self) -> i32 {
        match self {
            HealthLevel::Healthy => 0,
            HealthLevel::Degraded => 1,
            HealthLevel::Unhealthy => 2,
        }
    }

    /// Maps a numeric wire value back to a level.
    ///
    /// Out-of-range values clamp to [`HealthLevel::Unhealthy`] — an unknown
    /// health reading is treated as the least-safe assumption, not silently
    /// reported as healthy.
    pub fn from_i32(value: i32) -> HealthLevel {
        match value {
            0 => HealthLevel::Healthy,
            1 => HealthLevel::Degraded,
            _ => HealthLevel::Unhealthy,
        }
    }
}

/// The result of a cheap health probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthReport {
    /// The graded health level.
    pub level: HealthLevel,
    /// A short human-readable explanation.
    pub message: String,
    /// When the probe ran.
    pub checked_at: SystemTime,
}

impl Default for HealthReport {
    /// A healthy report checked at the Unix epoch — a stable default for tests
    /// and for modules that have not yet run a probe.
    fn default() -> HealthReport {
        HealthReport {
            level: HealthLevel::Healthy,
            message: String::new(),
            checked_at: SystemTime::UNIX_EPOCH,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_state_round_trips_through_its_wire_string() {
        let all = [
            ModuleState::Disabled,
            ModuleState::Initializing,
            ModuleState::Running,
            ModuleState::Degraded,
            ModuleState::Stopping,
            ModuleState::Stopped,
            ModuleState::Failed,
        ];
        for want in all {
            assert_eq!(ModuleState::parse(want.as_str()), Some(want));
        }
    }

    #[test]
    fn module_state_unknown_is_none() {
        assert_eq!(ModuleState::parse("exploded"), None);
        assert_eq!(ModuleState::parse(""), None);
    }

    #[test]
    fn health_level_round_trips_through_its_numeric_value() {
        let all = [
            HealthLevel::Healthy,
            HealthLevel::Degraded,
            HealthLevel::Unhealthy,
        ];
        for want in all {
            assert_eq!(HealthLevel::from_i32(want.as_i32()), want);
        }
    }

    #[test]
    fn health_level_out_of_range_is_unhealthy() {
        assert_eq!(HealthLevel::from_i32(7), HealthLevel::Unhealthy);
        assert_eq!(HealthLevel::from_i32(-1), HealthLevel::Unhealthy);
    }

    #[test]
    fn health_level_strings_are_lowercase() {
        assert_eq!(HealthLevel::Healthy.as_str(), "healthy");
        assert_eq!(HealthLevel::Degraded.as_str(), "degraded");
        assert_eq!(HealthLevel::Unhealthy.as_str(), "unhealthy");
    }

    #[test]
    fn defaults_are_disabled_and_healthy_at_epoch() {
        assert_eq!(Status::default().state, ModuleState::Disabled);
        let report = HealthReport::default();
        assert_eq!(report.level, HealthLevel::Healthy);
        assert_eq!(report.checked_at, SystemTime::UNIX_EPOCH);
    }
}
