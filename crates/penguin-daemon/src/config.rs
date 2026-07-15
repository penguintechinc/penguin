//! Loading and validating daemon and module configuration from disk.
//!
//! Port of the Go `internal/daemon/configstore.go`. The daemon config falls
//! back to sensible defaults when absent; module configs are validated against
//! the JSON Schema the module declares, using the same YAML → JSON bridge the Go
//! validator uses so the two engines return identical verdicts (the config
//! corpus enforces that).

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The daemon's own configuration, loaded from `<dir>/config.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonConfig {
    /// Path to the daemon's control socket.
    #[serde(default)]
    pub socket_path: String,
    /// Directory scanned for external plugin binaries.
    #[serde(default)]
    pub plugins_dir: String,
    /// Log level (`debug`/`info`/`warn`/`error`).
    #[serde(default)]
    pub log_level: String,
    /// Unix group allowed to talk to the control socket.
    #[serde(default)]
    pub group: String,
}

impl DaemonConfig {
    /// The built-in defaults, applied when `config.yaml` is missing or leaves a
    /// field empty. Kept separate from a `Default` impl because the parse step
    /// deliberately starts every field empty (so it can tell "unset" apart) and
    /// then fills the gaps from here — matching the Go two-step exactly.
    pub fn defaults() -> DaemonConfig {
        DaemonConfig {
            socket_path: "/run/penguin/penguind.sock".to_string(),
            plugins_dir: "/opt/penguin/plugins".to_string(),
            log_level: "info".to_string(),
            group: "penguin".to_string(),
        }
    }
}

/// An error loading or validating configuration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// A module name contained a path separator or `..` (traversal guard).
    #[error("invalid module name: contains path separators or '..': {0:?}")]
    InvalidName(String),
    /// A filesystem read failed.
    #[error("{0}")]
    Io(String),
    /// A YAML document was malformed.
    #[error("{0}")]
    Parse(String),
    /// A JSON Schema was itself malformed or would not compile.
    #[error("{0}")]
    Schema(String),
    /// The config parsed but failed schema validation. `details` lists the
    /// failing instance paths joined with `; `, mirroring the Go output.
    #[error("schema validation failed for module {module:?}: {details}")]
    Validation {
        /// The module whose config failed.
        module: String,
        /// The joined failing instance paths.
        details: String,
    },
}

/// Loads daemon and module configuration from a base directory (`/etc/penguin`
/// in production, a temp dir in tests).
pub struct ConfigStore {
    dir: PathBuf,
}

impl ConfigStore {
    /// Creates a store reading from `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> ConfigStore {
        ConfigStore { dir: dir.into() }
    }

    /// Reads the daemon config from `<dir>/config.yaml`.
    ///
    /// A missing file yields the defaults; a present-but-malformed file is an
    /// error. Any field left empty (missing or explicitly blank) is filled from
    /// [`DaemonConfig::defaults`], matching the Go behaviour where explicit `""`
    /// is treated the same as unset.
    pub fn daemon(&self) -> Result<DaemonConfig, ConfigError> {
        let defaults = DaemonConfig::defaults();
        let path = self.dir.join("config.yaml");

        let data = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(defaults),
            Err(err) => return Err(ConfigError::Io(format!("read config.yaml: {err}"))),
        };

        let mut config: DaemonConfig = match serde_norway::from_slice(&data) {
            Ok(parsed) => parsed,
            Err(err) => return Err(ConfigError::Parse(format!("parse config.yaml: {err}"))),
        };

        if config.socket_path.is_empty() {
            config.socket_path = defaults.socket_path;
        }
        if config.plugins_dir.is_empty() {
            config.plugins_dir = defaults.plugins_dir;
        }
        if config.log_level.is_empty() {
            config.log_level = defaults.log_level;
        }
        if config.group.is_empty() {
            config.group = defaults.group;
        }
        Ok(config)
    }

    /// Reads and parses `<dir>/modules.d/<name>.yaml`, validating it against
    /// `schema` when one is supplied.
    ///
    /// A missing file yields an empty object (the module applies its own
    /// defaults). The name is traversal-checked before it touches the path.
    pub fn module(
        &self,
        name: &str,
        schema: Option<&[u8]>,
    ) -> Result<serde_json::Value, ConfigError> {
        check_name(name)?;
        let path = self.module_path(name);

        let data = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                return Ok(serde_json::Value::Object(serde_json::Map::new()));
            }
            Err(err) => {
                return Err(ConfigError::Io(format!(
                    "read module config {name:?}: {err}"
                )));
            }
        };

        // Parse YAML into a JSON value — the same bridge the Go validator uses.
        // An empty document is Null; treat it as an empty object like Go's nil
        // map. Anything that is not a mapping is rejected (the module config
        // contract is a key/value document).
        let parsed: serde_json::Value = match serde_norway::from_slice(&data) {
            Ok(value) => value,
            Err(err) => {
                return Err(ConfigError::Parse(format!(
                    "parse module config {name:?}: {err}"
                )));
            }
        };
        let value = normalize_document(parsed, name)?;

        // A let-chain keeps the two guards flat; edition 2024 stabilised the
        // `if let ... && ...` form, so no nested `if` (clippy::collapsible_if).
        if let Some(schema_bytes) = schema
            && !schema_bytes.is_empty()
        {
            validate_schema(&value, schema_bytes, name)?;
        }
        Ok(value)
    }

    /// Returns the module's config file verbatim (YAML bytes) after validating
    /// it, or `None` when the file is absent. This is what the host hands a
    /// module, so the module never parses an unvalidated file itself.
    pub fn module_raw(
        &self,
        name: &str,
        schema: Option<&[u8]>,
    ) -> Result<Option<Vec<u8>>, ConfigError> {
        check_name(name)?;
        // Validate first (also re-checks the name and applies the schema).
        self.module(name, schema)?;

        let path = self.module_path(name);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
            Err(err) => Err(ConfigError::Io(format!(
                "read module config {name:?}: {err}"
            ))),
        }
    }

    /// Builds the on-disk path for a module's config file.
    fn module_path(&self, name: &str) -> PathBuf {
        self.dir.join("modules.d").join(format!("{name}.yaml"))
    }
}

/// Rejects module names that could escape the config directory.
fn check_name(name: &str) -> Result<(), ConfigError> {
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(ConfigError::InvalidName(name.to_string()));
    }
    Ok(())
}

/// Coerces a parsed YAML document into the object form a module config must
/// take: an empty document becomes an empty object; a mapping passes through;
/// anything else is a parse error.
fn normalize_document(
    value: serde_json::Value,
    name: &str,
) -> Result<serde_json::Value, ConfigError> {
    if value.is_null() {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }
    if value.is_object() {
        return Ok(value);
    }
    Err(ConfigError::Parse(format!(
        "parse module config {name:?}: top-level document must be a mapping"
    )))
}

/// Validates a JSON instance against JSON-Schema bytes.
///
/// Collects every failing instance path so the error mirrors the Go validator's
/// joined output. The schema is compiled fresh each call; caching lands when the
/// daemon supervisor holds long-lived module handles (M2/M3).
fn validate_schema(
    instance: &serde_json::Value,
    schema_bytes: &[u8],
    module: &str,
) -> Result<(), ConfigError> {
    let schema: serde_json::Value = match serde_json::from_slice(schema_bytes) {
        Ok(value) => value,
        Err(err) => {
            return Err(ConfigError::Schema(format!(
                "parse schema for module {module:?}: {err}"
            )));
        }
    };

    let validator = match jsonschema::validator_for(&schema) {
        Ok(compiled) => compiled,
        Err(err) => {
            return Err(ConfigError::Schema(format!(
                "compile schema for module {module:?}: {err}"
            )));
        }
    };

    let mut paths: Vec<String> = Vec::new();
    for error in validator.iter_errors(instance) {
        paths.push(error.instance_path().to_string());
    }

    if paths.is_empty() {
        Ok(())
    } else {
        Err(ConfigError::Validation {
            module: module.to_string(),
            details: paths.join("; "),
        })
    }
}

/// Convenience so callers can log where a store reads from.
impl AsRef<Path> for ConfigStore {
    fn as_ref(&self) -> &Path {
        &self.dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Writes `modules.d/<name>.yaml` under a fresh temp dir and returns both.
    fn store_with_module(name: &str, yaml: &str) -> (TempDir, ConfigStore) {
        let dir = TempDir::new().unwrap();
        let modules = dir.path().join("modules.d");
        std::fs::create_dir_all(&modules).unwrap();
        std::fs::write(modules.join(format!("{name}.yaml")), yaml).unwrap();
        let store = ConfigStore::new(dir.path());
        (dir, store)
    }

    #[test]
    fn daemon_returns_defaults_when_file_absent() {
        let dir = TempDir::new().unwrap();
        let store = ConfigStore::new(dir.path());
        assert_eq!(store.daemon().unwrap(), DaemonConfig::defaults());
    }

    #[test]
    fn daemon_fills_empty_fields_from_defaults() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("config.yaml"),
            "logLevel: debug\nsocketPath: \"\"\n",
        )
        .unwrap();
        let store = ConfigStore::new(dir.path());

        let config = store.daemon().unwrap();
        assert_eq!(config.log_level, "debug");
        // Explicit empty socketPath is treated as unset and filled.
        assert_eq!(config.socket_path, DaemonConfig::defaults().socket_path);
        assert_eq!(config.group, "penguin");
    }

    #[test]
    fn daemon_reports_malformed_yaml() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("config.yaml"),
            "socketPath: [unterminated\n",
        )
        .unwrap();
        let store = ConfigStore::new(dir.path());
        assert!(matches!(store.daemon(), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn module_returns_empty_object_when_file_absent() {
        let dir = TempDir::new().unwrap();
        let store = ConfigStore::new(dir.path());
        let value = store.module("squawk", None).unwrap();
        assert!(value.is_object());
        assert_eq!(value.as_object().unwrap().len(), 0);
    }

    #[test]
    fn module_rejects_traversal_names() {
        let dir = TempDir::new().unwrap();
        let store = ConfigStore::new(dir.path());
        for bad in ["../etc", "a/b", "a\\b", ".."] {
            assert!(matches!(
                store.module(bad, None),
                Err(ConfigError::InvalidName(_))
            ));
        }
    }

    #[test]
    fn module_accepts_config_matching_schema() {
        let schema = br#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"endpoint":{"type":"string"}},"required":["endpoint"]}"#;
        let (_dir, store) = store_with_module("squawk", "endpoint: us-east\n");
        assert!(store.module("squawk", Some(schema)).is_ok());
    }

    #[test]
    fn module_rejects_config_violating_schema() {
        let schema = br#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"endpoint":{"type":"string"}},"required":["endpoint"]}"#;
        let (_dir, store) = store_with_module("squawk", "port: 53\n");
        assert!(matches!(
            store.module("squawk", Some(schema)),
            Err(ConfigError::Validation { .. })
        ));
    }

    #[test]
    fn module_reports_a_malformed_schema() {
        let schema = b"{not valid json";
        let (_dir, store) = store_with_module("squawk", "endpoint: us-east\n");
        assert!(matches!(
            store.module("squawk", Some(schema)),
            Err(ConfigError::Schema(_))
        ));
    }

    #[test]
    fn module_rejects_non_mapping_top_level() {
        let (_dir, store) = store_with_module("squawk", "- just\n- a\n- list\n");
        assert!(matches!(
            store.module("squawk", None),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn module_raw_round_trips_bytes_and_reports_absent() {
        let (_dir, store) = store_with_module("squawk", "endpoint: us-east\n");
        let bytes = store.module_raw("squawk", None).unwrap().unwrap();
        assert_eq!(bytes, b"endpoint: us-east\n");
        assert!(store.module_raw("absent", None).unwrap().is_none());
    }
}
