//! SkausWatch module configuration: YAML-parsed via serde-norway.

use serde::{Deserialize, Serialize};

/// SkausWatch module configuration, parsed from the daemon's YAML config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleConfig {
    /// Base URL of the monitoring backend (e.g., "https://monitoring.example.com").
    #[serde(default)]
    pub base_url: String,

    /// Enrollment token issued out-of-band by the Manager operator, used to
    /// register this agent the first time the heartbeat loop runs (see
    /// `module.rs`'s `ensure_identity`) — never used again once an
    /// [`skauswatch_client::AgentIdentity`] has been persisted.
    #[serde(default)]
    pub enrollment_token: String,

    /// Heartbeat interval in seconds for health checks.
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval: u64,
}

fn default_heartbeat_interval() -> u64 {
    60
}

impl Default for ModuleConfig {
    fn default() -> Self {
        ModuleConfig {
            base_url: String::new(),
            enrollment_token: String::new(),
            heartbeat_interval: default_heartbeat_interval(),
        }
    }
}

/// JSON Schema for SkausWatch module configuration, validated at daemon startup.
pub const CONFIG_SCHEMA: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "type": "object",
  "properties": {
    "base_url": {
      "type": "string",
      "description": "Base URL of the monitoring backend"
    },
    "enrollment_token": {
      "type": "string",
      "description": "Enrollment token issued by the Manager operator, used to register this agent"
    },
    "heartbeat_interval": {
      "type": "integer",
      "minimum": 1,
      "default": 60,
      "description": "Heartbeat interval in seconds"
    }
  },
  "required": ["base_url", "enrollment_token"]
}"#;
