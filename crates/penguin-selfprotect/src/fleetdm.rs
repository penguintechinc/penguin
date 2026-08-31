//! FleetDM detection: detect whether FleetDM/osqueryd are present on the system.
//!
//! This module provides a probe-based interface for detecting FleetDM and osqueryd binaries.
//! Detection is read-only; we never start, stop, or configure FleetDM.

use std::path::Path;
use std::process::Command;

/// Trait for probing binary presence.
///
/// Abstraction over system binary detection to enable testing without shelling out.
pub trait FleetProbe {
    /// Check if a binary is present and return its version string.
    ///
    /// Returns `Some(version)` if the binary is found and callable, `None` otherwise.
    fn binary_present(&self, name: &str) -> Option<String>;
}

/// Detection results for FleetDM and osqueryd presence.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FleetStatus {
    /// Version string of fleetd if present, None otherwise.
    pub fleetd: Option<String>,
    /// Version string of osqueryd if present, None otherwise.
    pub osqueryd: Option<String>,
}

/// Detect the presence of FleetDM and osqueryd using the given probe.
///
/// # Arguments
///
/// * `probe` - A probe implementation to check binary presence
///
/// # Returns
///
/// A `FleetStatus` struct containing the version strings of detected binaries.
pub fn detect(probe: &dyn FleetProbe) -> FleetStatus {
    FleetStatus {
        fleetd: probe.binary_present("fleetd"),
        osqueryd: probe.binary_present("osqueryd"),
    }
}

/// Real probe implementation that checks the filesystem.
///
/// Searches for binaries in PATH and well-known installation directories.
/// This is detect-only; we never start, stop, or configure FleetDM.
#[cfg(unix)]
#[allow(dead_code)]
pub struct RealFleetProbe;

#[cfg(unix)]
impl FleetProbe for RealFleetProbe {
    fn binary_present(&self, name: &str) -> Option<String> {
        // Well-known installation directories for FleetDM/osqueryd.
        let search_dirs = ["/opt/orbit/bin/", "/usr/local/bin/", "/usr/bin/"];

        // Try each search directory.
        for dir in &search_dirs {
            let path = format!("{}{}", dir, name);
            if Path::new(&path).exists()
                && let Ok(output) = Command::new(&path).arg("--version").output()
                && output.status.success()
                && let Ok(version_str) = String::from_utf8(output.stdout)
            {
                return Some(version_str.trim().to_string());
            }
        }

        // Try PATH via `which` (best-effort).
        if let Ok(output) = Command::new("which").arg(name).output()
            && output.status.success()
            && let Ok(path_output) = String::from_utf8(output.stdout)
        {
            let path = path_output.trim();
            if let Ok(version_output) = Command::new(path).arg("--version").output()
                && version_output.status.success()
                && let Ok(version_str) = String::from_utf8(version_output.stdout)
            {
                return Some(version_str.trim().to_string());
            }
        }

        None
    }
}

#[cfg(not(unix))]
pub struct RealFleetProbe;

#[cfg(not(unix))]
impl FleetProbe for RealFleetProbe {
    fn binary_present(&self, _name: &str) -> Option<String> {
        // Non-Unix platforms: not supported.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        has_fleetd: bool,
    }

    impl FleetProbe for Fake {
        fn binary_present(&self, name: &str) -> Option<String> {
            if name == "fleetd" && self.has_fleetd {
                Some("1.30.0".into())
            } else {
                None
            }
        }
    }

    #[test]
    fn detect_reports_present_and_absent() {
        let s = detect(&Fake { has_fleetd: true });
        assert_eq!(s.fleetd.as_deref(), Some("1.30.0"));
        assert!(s.osqueryd.is_none());
    }
}
