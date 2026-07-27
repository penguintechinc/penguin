//! waddlebot's on-disk module configuration and the JSON Schema the daemon
//! validates it against before [`crate::WaddlebotModule::init`] ever reads
//! it.
//!
//! The Community Access Token is deliberately **not** a field here: it is a
//! secret, read via `host.secrets()` in `init`, never from this document —
//! see that method's doc for why a config-carried value is still overridden
//! by the secret store.
//!
//! `bridge` is reserved for the local integration bridge, built on a
//! separate, later track: its shape is declared now so that track only has
//! to read the field, not add it — see
//! [`crate::WaddlebotModule::start_bridge`]'s doc for the seam.

use serde::{Deserialize, Serialize};

/// waddlebot's full on-disk config shape, validated by the daemon against
/// [`CONFIG_SCHEMA`] before [`crate::WaddlebotModule::init`] ever reads it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ModuleConfig {
    pub hub: HubSection,
    /// The community this module acts on. `0` (the default) is not a valid
    /// community id on the hub — `init` still builds a client against it
    /// (never fails on account of this alone; see `init`'s doc), so a
    /// misconfigured/unset value simply shows up as hub errors on first use
    /// rather than a load-time failure.
    pub community_id: i64,
    pub bridge: BridgeSection,
}

/// Which hub this module talks to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct HubSection {
    pub base_url: String,
}

impl Default for HubSection {
    fn default() -> HubSection {
        HubSection {
            base_url: waddlebot_client::config::DEFAULT_BASE_URL.to_string(),
        }
    }
}

/// Reserved configuration for the local integration bridge — see this
/// module's doc. Every field here is read by this crate's config parser and
/// schema, but nothing in this track acts on them yet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct BridgeSection {
    pub enabled: bool,
    pub listen_tcp: String,
    pub listen_unix: String,
    pub allowed_integrations: Vec<String>,
    pub obs: ObsSection,
}

/// OBS WebSocket adapter configuration for the bridge.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct ObsSection {
    /// Whether to enable the OBS adapter.
    pub enabled: bool,
    /// The OBS WebSocket server URL (typically `ws://127.0.0.1:4455`).
    pub url: String,
    /// The secret key name in the secrets store for the OBS password.
    pub secret_key: String,
}

impl Default for ObsSection {
    fn default() -> ObsSection {
        ObsSection {
            enabled: false,
            url: String::new(),
            secret_key: "obs_password".to_string(),
        }
    }
}

/// The JSON Schema the daemon validates `waddlebot.yaml` against.
pub const CONFIG_SCHEMA: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "type": "object",
  "properties": {
    "hub": {
      "type": "object",
      "properties": {
        "base_url": {
          "type": "string",
          "description": "waddlebot hub API base URL",
          "default": "https://waddles.app/api/v1"
        }
      }
    },
    "community_id": {
      "type": "integer",
      "description": "The community this module acts on",
      "default": 0
    },
    "bridge": {
      "type": "object",
      "description": "Reserved for the local integration bridge, built on a separate, later track. Unused by this module today.",
      "properties": {
        "enabled": {
          "type": "boolean",
          "description": "Enable the local integration bridge",
          "default": false
        },
        "listen_tcp": {
          "type": "string",
          "description": "TCP address the bridge listens on, when enabled",
          "default": ""
        },
        "listen_unix": {
          "type": "string",
          "description": "Unix socket path the bridge listens on, when enabled",
          "default": ""
        },
        "allowed_integrations": {
          "type": "array",
          "items": { "type": "string" },
          "description": "Local integration names permitted to connect to the bridge",
          "default": []
        },
        "obs": {
          "type": "object",
          "description": "OBS WebSocket adapter configuration",
          "properties": {
            "enabled": {
              "type": "boolean",
              "description": "Enable the OBS WebSocket adapter",
              "default": false
            },
            "url": {
              "type": "string",
              "description": "OBS WebSocket server URL (e.g., ws://127.0.0.1:4455)",
              "default": ""
            },
            "secret_key": {
              "type": "string",
              "description": "Secret store key for the OBS WebSocket password",
              "default": "obs_password"
            }
          }
        }
      }
    }
  }
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_points_at_the_real_hub_with_no_active_community() {
        let cfg = ModuleConfig::default();
        assert_eq!(cfg.hub.base_url, waddlebot_client::config::DEFAULT_BASE_URL);
        assert_eq!(cfg.community_id, 0);
        assert!(!cfg.bridge.enabled);
        assert!(cfg.bridge.listen_tcp.is_empty());
        assert!(cfg.bridge.listen_unix.is_empty());
        assert!(cfg.bridge.allowed_integrations.is_empty());
    }

    #[test]
    fn empty_document_deserializes_to_the_full_default() {
        let cfg: ModuleConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg, ModuleConfig::default());
    }

    #[test]
    fn partial_document_only_overrides_what_it_names() {
        let yaml = "community_id: 42\n";
        let cfg: ModuleConfig = serde_norway::from_str(yaml).unwrap();
        assert_eq!(cfg.community_id, 42);
        assert_eq!(cfg.hub.base_url, waddlebot_client::config::DEFAULT_BASE_URL);
        assert!(!cfg.bridge.enabled);
    }

    #[test]
    fn bridge_section_round_trips_through_yaml() {
        let yaml = "bridge:\n  enabled: true\n  listen_tcp: \"127.0.0.1:9700\"\n  allowed_integrations: [\"twitch\", \"discord\"]\n";
        let cfg: ModuleConfig = serde_norway::from_str(yaml).unwrap();
        assert!(cfg.bridge.enabled);
        assert_eq!(cfg.bridge.listen_tcp, "127.0.0.1:9700");
        assert_eq!(
            cfg.bridge.allowed_integrations,
            vec!["twitch".to_string(), "discord".to_string()]
        );
    }

    #[test]
    fn schema_is_valid_json_and_compiles() {
        let schema: serde_json::Value = serde_json::from_str(CONFIG_SCHEMA).unwrap();
        let validator = jsonschema::validator_for(&schema);
        assert!(validator.is_ok(), "schema must compile");
    }

    #[test]
    fn schema_accepts_the_default_config_document() {
        let schema: serde_json::Value = serde_json::from_str(CONFIG_SCHEMA).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let instance = serde_json::to_value(ModuleConfig::default()).unwrap();
        let errors: Vec<_> = validator.iter_errors(&instance).collect();
        assert!(errors.is_empty(), "unexpected schema errors: {errors:?}");
    }

    /// The advertised schema default must match what the code actually
    /// applies — squawk shipped a schema promising one DoH default while
    /// its code applied another; not repeating that here.
    #[test]
    fn schema_default_for_hub_base_url_matches_the_code_default() {
        let schema: serde_json::Value = serde_json::from_str(CONFIG_SCHEMA).unwrap();
        let advertised = schema["properties"]["hub"]["properties"]["base_url"]["default"]
            .as_str()
            .unwrap();
        assert_eq!(advertised, ModuleConfig::default().hub.base_url);
        assert_eq!(advertised, waddlebot_client::config::DEFAULT_BASE_URL);
    }

    #[test]
    fn schema_default_for_bridge_enabled_matches_the_code_default() {
        let schema: serde_json::Value = serde_json::from_str(CONFIG_SCHEMA).unwrap();
        let advertised = schema["properties"]["bridge"]["properties"]["enabled"]["default"]
            .as_bool()
            .unwrap();
        assert_eq!(advertised, ModuleConfig::default().bridge.enabled);
    }

    #[test]
    fn schema_default_for_obs_config_matches_the_code_default() {
        let schema: serde_json::Value = serde_json::from_str(CONFIG_SCHEMA).unwrap();
        let obs_schema = &schema["properties"]["bridge"]["properties"]["obs"]["properties"];

        let enabled_advertised = obs_schema["enabled"]["default"].as_bool().unwrap();
        assert_eq!(
            enabled_advertised,
            ModuleConfig::default().bridge.obs.enabled
        );

        let url_advertised = obs_schema["url"]["default"].as_str().unwrap();
        assert_eq!(url_advertised, ModuleConfig::default().bridge.obs.url);

        let secret_key_advertised = obs_schema["secret_key"]["default"].as_str().unwrap();
        assert_eq!(
            secret_key_advertised,
            ModuleConfig::default().bridge.obs.secret_key
        );
    }
}
