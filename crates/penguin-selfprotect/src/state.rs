//! Protection arm state: determines when the agent is armed and what secrets
//! are available for tamper detection.

use crate::manifest::IntegrityManifest;

/// Determines whether the agent should be armed based on enrollment and
/// feature-flag state.
///
/// The agent is ARMED only when both conditions are true:
/// - `enrolled`: the node is enrolled in the protection system
/// - `flag_on`: the protection feature flag is enabled
///
/// Rationale: a fresh/unenrolled agent or flag-disabled agent should never
/// be armed — no operational friction for development or disabled deployments.
/// Both must be true to prevent a single point of failure from silently
/// disarming protection.
pub fn is_armed(enrolled: bool, flag_on: bool) -> bool {
    enrolled && flag_on
}

/// Resolved protection state when armed: the node identity, optional tamper
/// secret, and the controller-signed integrity manifest.
///
/// Only populated when both enrollment and feature flag are true; all other
/// state is considered inactive.
#[derive(Debug, Clone)]
pub struct ProtectionState {
    /// The unique node identifier for this agent install.
    node_id: String,
    /// Optional PHC-encoded (Argon2id) hash of the tamper secret,
    /// or None if the secret was not provisioned.
    secret_phc: Option<String>,
    /// The controller-signed manifest of expected files, hashes, and modes.
    manifest: IntegrityManifest,
}

impl ProtectionState {
    /// Creates a new protection state.
    ///
    /// # Arguments
    /// * `node_id` - The unique node identifier
    /// * `secret_phc` - Optional PHC-encoded secret hash
    /// * `manifest` - The integrity manifest
    pub fn new(node_id: String, secret_phc: Option<String>, manifest: IntegrityManifest) -> Self {
        Self {
            node_id,
            secret_phc,
            manifest,
        }
    }

    /// Returns a reference to the node identifier.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Returns a reference to the optional secret PHC hash.
    pub fn secret_phc(&self) -> Option<&str> {
        self.secret_phc.as_deref()
    }

    /// Returns a reference to the integrity manifest.
    pub fn manifest(&self) -> &IntegrityManifest {
        &self.manifest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armed_only_when_enrolled_and_flag_on() {
        assert!(is_armed(true, true));
        assert!(!is_armed(false, true)); // unenrolled/dev agent stays unarmed
        assert!(!is_armed(true, false)); // flag off
        assert!(!is_armed(false, false)); // both false
    }
}
