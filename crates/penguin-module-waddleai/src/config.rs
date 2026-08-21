//! waddleai's on-disk module configuration and the JSON Schema the daemon
//! validates it against before [`crate::module::WaddleAiModule::init`] ever
//! reads it.
//!
//! The virtual key is deliberately **not** a field here: it is a secret,
//! read via `host.secrets()` in `init`, never from this document — see that
//! method's doc, matching every other built-in module's identical rule for
//! its own credential.

use serde::{Deserialize, Serialize};

use crate::cache::DEFAULT_MAX_AGE;

/// How often, while running, the background task refreshes the cached Tier-1
/// denylist. Independent of [`DenylistSection::max_age_secs`]: the sync
/// interval is "how often we try", staleness is "how long a failed-to-sync
/// cache stays trustworthy" — a long outage can make the cache stale well
/// before this interval's next attempt would have refreshed it anyway.
const DEFAULT_SYNC_INTERVAL_SECS: u64 = 60 * 60;

/// waddleai's full on-disk config shape, validated by the daemon against
/// [`CONFIG_SCHEMA`] before [`crate::module::WaddleAiModule::init`] ever
/// reads it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ModuleConfig {
    pub server: ServerSection,
    pub denylist: DenylistSection,
    pub hooks: HooksSection,
}

/// Which WaddleAI server this module talks to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct ServerSection {
    pub base_url: String,
}

impl Default for ServerSection {
    fn default() -> ServerSection {
        ServerSection {
            base_url: crate::client::DEFAULT_BASE_URL.to_string(),
        }
    }
}

/// The Tier-1 denylist cache's sync cadence and staleness bound. See
/// `crate::cache::DenylistCache`'s doc for what "stale" changes about the
/// offline fail-closed path.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct DenylistSection {
    pub sync_interval_secs: u64,
    pub max_age_secs: u64,
}

impl Default for DenylistSection {
    fn default() -> DenylistSection {
        DenylistSection {
            sync_interval_secs: DEFAULT_SYNC_INTERVAL_SECS,
            max_age_secs: DEFAULT_MAX_AGE.as_secs(),
        }
    }
}

/// Which ecosystem shims [`crate::module::WaddleAiModule::start`] installs
/// automatically, in addition to the explicit `penguin waddleai hooks
/// install <ecosystem>` CLI path. Every field defaults to `false`: shim
/// installation touches a file the operator did not create, so this module
/// never does it unasked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct HooksSection {
    pub claude: bool,
    pub gemini: bool,
    pub vscode: bool,
}

/// The JSON Schema the daemon validates `waddleai.yaml` against.
pub const CONFIG_SCHEMA: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "type": "object",
  "properties": {
    "server": {
      "type": "object",
      "properties": {
        "base_url": {
          "type": "string",
          "description": "WaddleAI API base URL",
          "default": "https://waddleai.app/api/v1"
        }
      }
    },
    "denylist": {
      "type": "object",
      "properties": {
        "sync_interval_secs": {
          "type": "integer",
          "description": "How often, while running, to refresh the cached Tier-1 denylist",
          "default": 3600
        },
        "max_age_secs": {
          "type": "integer",
          "description": "How long a synced denylist stays trustworthy for the offline fail-closed path before it is treated as stale",
          "default": 86400
        }
      }
    },
    "hooks": {
      "type": "object",
      "description": "Which ecosystem shims to install automatically on module start, in addition to the explicit CLI install path",
      "properties": {
        "claude": {
          "type": "boolean",
          "description": "Install the Claude Code / Cortex hook shim on start",
          "default": false
        },
        "gemini": {
          "type": "boolean",
          "description": "Install the Google Antigravity / AGY CLI hook shim on start",
          "default": false
        },
        "vscode": {
          "type": "boolean",
          "description": "Install the VS Code hook shim on start",
          "default": false
        }
      }
    }
  }
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_points_at_the_real_api_with_no_auto_installed_hooks() {
        let cfg = ModuleConfig::default();
        assert_eq!(cfg.server.base_url, crate::client::DEFAULT_BASE_URL);
        assert_eq!(cfg.denylist.sync_interval_secs, DEFAULT_SYNC_INTERVAL_SECS);
        assert_eq!(cfg.denylist.max_age_secs, DEFAULT_MAX_AGE.as_secs());
        assert!(!cfg.hooks.claude);
        assert!(!cfg.hooks.gemini);
        assert!(!cfg.hooks.vscode);
    }

    #[test]
    fn empty_document_deserializes_to_the_full_default() {
        let cfg: ModuleConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg, ModuleConfig::default());
    }

    #[test]
    fn partial_document_only_overrides_what_it_names() {
        let yaml = "hooks:\n  claude: true\n";
        let cfg: ModuleConfig = serde_norway::from_str(yaml).unwrap();
        assert!(cfg.hooks.claude);
        assert!(!cfg.hooks.vscode);
        assert_eq!(cfg.server.base_url, crate::client::DEFAULT_BASE_URL);
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
    /// applies — squawk shipped a schema promising one default while its
    /// code applied another; not repeating that here.
    #[test]
    fn schema_defaults_match_the_code_defaults() {
        let schema: serde_json::Value = serde_json::from_str(CONFIG_SCHEMA).unwrap();
        assert_eq!(
            schema["properties"]["server"]["properties"]["base_url"]["default"],
            crate::client::DEFAULT_BASE_URL
        );
        assert_eq!(
            schema["properties"]["denylist"]["properties"]["sync_interval_secs"]["default"],
            DEFAULT_SYNC_INTERVAL_SECS
        );
        assert_eq!(
            schema["properties"]["denylist"]["properties"]["max_age_secs"]["default"],
            DEFAULT_MAX_AGE.as_secs()
        );
        assert_eq!(
            schema["properties"]["hooks"]["properties"]["claude"]["default"],
            false
        );
    }
}
