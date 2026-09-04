//! SkausWatch module configuration: YAML-parsed via serde-norway.
//!
//! `api_key` is deliberately **not** a field here: it is a credential, read
//! via `host.secrets()` in `crate::module::SkausWatchModule::init`, never
//! from this document — matching every other built-in module's rule for its
//! own credential (see `penguin_module_waddleai::config`'s identical
//! reasoning for its virtual key).

use serde::{Deserialize, Serialize};

/// SkausWatch module configuration, parsed from the daemon's YAML config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleConfig {
    /// Base URL of the monitoring backend (e.g., "https://monitoring.example.com").
    #[serde(default)]
    pub base_url: String,

    /// This agent's identity, provisioned out-of-band by whatever process
    /// seeds the Manager's `endpoint_agents` row for this host — sent as
    /// the `x-agent-id` header on every request. Not obtained over the
    /// wire; see `crate::module`'s `init` doc.
    #[serde(default)]
    pub agent_id: String,

    /// Per-tenant enrollment token, only needed the very first time a
    /// brand-new `agent_id` calls `register()` — the Manager uses it to
    /// resolve which tenant the row is created under. Leave unset once the
    /// operator has already provisioned the `endpoint_agents` row
    /// out-of-band; re-registration of an already-known `agent_id` ignores
    /// it even if present.
    #[serde(default)]
    pub enrollment_token: Option<String>,

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
            agent_id: String::new(),
            enrollment_token: None,
            heartbeat_interval: default_heartbeat_interval(),
        }
    }
}

/// JSON Schema for SkausWatch module configuration, validated at daemon startup.
///
/// `api_key` is intentionally absent — it's supplied via the host secret
/// store, never this config document.
pub const CONFIG_SCHEMA: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "type": "object",
  "properties": {
    "base_url": {
      "type": "string",
      "description": "Base URL of the monitoring backend"
    },
    "agent_id": {
      "type": "string",
      "description": "This agent's identity, provisioned out-of-band with the Manager"
    },
    "enrollment_token": {
      "type": "string",
      "description": "Per-tenant enrollment token, needed only for a brand-new agent_id's first registration"
    },
    "heartbeat_interval": {
      "type": "integer",
      "minimum": 1,
      "default": 60,
      "description": "Heartbeat interval in seconds"
    }
  },
  "required": ["base_url", "agent_id"]
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_no_enrollment_token_and_a_60s_heartbeat() {
        let cfg = ModuleConfig::default();
        assert!(cfg.base_url.is_empty());
        assert!(cfg.agent_id.is_empty());
        assert_eq!(cfg.enrollment_token, None);
        assert_eq!(cfg.heartbeat_interval, 60);
    }

    #[test]
    fn parses_a_full_document_including_the_optional_enrollment_token() {
        let cfg: ModuleConfig = serde_json::from_str(
            r#"{"base_url":"https://manager.example.com","agent_id":"agent-1","enrollment_token":"tok","heartbeat_interval":30}"#,
        )
        .unwrap();
        assert_eq!(cfg.base_url, "https://manager.example.com");
        assert_eq!(cfg.agent_id, "agent-1");
        assert_eq!(cfg.enrollment_token.as_deref(), Some("tok"));
        assert_eq!(cfg.heartbeat_interval, 30);
    }

    #[test]
    fn schema_is_valid_json_and_requires_base_url_and_agent_id() {
        let schema: serde_json::Value = serde_json::from_str(CONFIG_SCHEMA).unwrap();
        let required = schema["required"].as_array().expect("required is an array");
        let required: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(required, vec!["base_url", "agent_id"]);
        assert!(schema["properties"].get("api_key").is_none());
    }
}
