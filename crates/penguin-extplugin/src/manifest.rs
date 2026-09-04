//! Plugin manifest (`plugin.json`) parsing.
//!
//! Mirrors the Go `extplugin.Manifest` / `LoadManifest` (go-client/internal
//! /extplugin/manifest.go) byte-for-byte: same JSON field names, same
//! `name`/`binary`/`sha256` required-field checks, same derived paths.
//! Ownership, hash, and signature verification live in [`crate::verify`].

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// Plugin manifest: describes a plugin binary, its expected hash, and
/// publisher metadata. Parsed from `plugin.json` in the plugin directory.
///
/// Every field defaults to empty when the JSON key is absent (rather than
/// failing to parse) so [`load_manifest`] can report a precise
/// [`ManifestError::MissingField`] for the fields that are actually
/// required, matching the Go reference's parse-then-validate order.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    /// Plugin name, e.g. "hello". Required.
    #[serde(default)]
    pub name: String,
    /// Semantic version, e.g. "1.0.0". Not validated for shape.
    #[serde(default)]
    pub version: String,
    /// SDK version the plugin targets, e.g. "v1".
    #[serde(default, rename = "sdk_version")]
    pub sdk_version: String,
    /// Relative filename of the binary, e.g. "hello". Required.
    #[serde(default)]
    pub binary: String,
    /// Hex-encoded SHA256 of the binary. Required.
    #[serde(default)]
    pub sha256: String,
    /// Publisher name, for audit logging only — never a trust decision
    /// input (the trust decision is the pinned minisign key, not this
    /// string).
    #[serde(default)]
    pub publisher: String,
}

/// Every distinct way loading a manifest can fail.
#[derive(Debug, Error)]
pub enum ManifestError {
    /// `plugin.json` could not be read (missing file, permission denied).
    #[error("read {path}: {source}")]
    Read {
        /// The manifest path that could not be read.
        path: PathBuf,
        /// The underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// `plugin.json` was read but is not valid JSON.
    #[error("parse {path}: {source}")]
    Parse {
        /// The manifest path that failed to parse.
        path: PathBuf,
        /// The underlying JSON error.
        #[source]
        source: serde_json::Error,
    },
    /// `plugin.json` parsed, but a required field is empty or absent.
    #[error("plugin.json: missing required field '{0}'")]
    MissingField(&'static str),
}

impl Manifest {
    /// Full path to the plugin binary inside `plugin_dir`.
    pub fn binary_path(&self, plugin_dir: &Path) -> PathBuf {
        plugin_dir.join(&self.binary)
    }

    /// Full path to the binary's `.minisig` signature file.
    pub fn signature_path(&self, plugin_dir: &Path) -> PathBuf {
        let mut file_name = self.binary.clone();
        file_name.push_str(".minisig");
        plugin_dir.join(file_name)
    }
}

/// Loads and parses `plugin.json` from `plugin_dir`, then checks that
/// `name`, `binary`, and `sha256` are all present — the same three fields
/// the Go reference requires.
pub fn load_manifest(plugin_dir: &Path) -> Result<Manifest, ManifestError> {
    let manifest_path = plugin_dir.join("plugin.json");
    let data = std::fs::read(&manifest_path).map_err(|source| ManifestError::Read {
        path: manifest_path.clone(),
        source,
    })?;

    let manifest: Manifest =
        serde_json::from_slice(&data).map_err(|source| ManifestError::Parse {
            path: manifest_path.clone(),
            source,
        })?;

    if manifest.name.is_empty() {
        return Err(ManifestError::MissingField("name"));
    }
    if manifest.binary.is_empty() {
        return Err(ManifestError::MissingField("binary"));
    }
    if manifest.sha256.is_empty() {
        return Err(ManifestError::MissingField("sha256"));
    }

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(dir: &Path, body: &str) {
        std::fs::write(dir.join("plugin.json"), body).expect("write plugin.json");
    }

    #[test]
    fn load_manifest_happy_path_parses_all_fields() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_manifest(
            tmp.path(),
            r#"{
              "name": "test-plugin",
              "version": "1.0.0",
              "sdk_version": "v1",
              "binary": "test-binary",
              "sha256": "abc123def456",
              "publisher": "test-publisher"
            }"#,
        );

        let manifest = load_manifest(tmp.path()).expect("load manifest");

        assert_eq!(manifest.name, "test-plugin");
        assert_eq!(manifest.version, "1.0.0");
        assert_eq!(manifest.sdk_version, "v1");
        assert_eq!(manifest.binary, "test-binary");
        assert_eq!(manifest.sha256, "abc123def456");
        assert_eq!(manifest.publisher, "test-publisher");
    }

    #[test]
    fn load_manifest_missing_file_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");

        let err = load_manifest(tmp.path()).expect_err("missing plugin.json must fail");

        assert!(matches!(err, ManifestError::Read { .. }));
    }

    #[test]
    fn load_manifest_garbage_json_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_manifest(tmp.path(), "not valid json {{{");

        let err = load_manifest(tmp.path()).expect_err("garbage json must fail");

        assert!(matches!(err, ManifestError::Parse { .. }));
    }

    #[test]
    fn load_manifest_missing_name_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_manifest(
            tmp.path(),
            r#"{"version": "1.0.0", "binary": "bin", "sha256": "abc"}"#,
        );

        let err = load_manifest(tmp.path()).expect_err("missing name must fail");

        assert!(matches!(err, ManifestError::MissingField("name")));
    }

    #[test]
    fn load_manifest_missing_binary_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_manifest(
            tmp.path(),
            r#"{"name": "test", "version": "1.0.0", "sha256": "abc"}"#,
        );

        let err = load_manifest(tmp.path()).expect_err("missing binary must fail");

        assert!(matches!(err, ManifestError::MissingField("binary")));
    }

    #[test]
    fn load_manifest_missing_sha256_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_manifest(
            tmp.path(),
            r#"{"name": "test", "version": "1.0.0", "binary": "bin"}"#,
        );

        let err = load_manifest(tmp.path()).expect_err("missing sha256 must fail");

        assert!(matches!(err, ManifestError::MissingField("sha256")));
    }

    #[test]
    fn binary_path_joins_plugin_dir_and_binary_name() {
        let manifest = Manifest {
            name: String::new(),
            version: String::new(),
            sdk_version: String::new(),
            binary: String::from("mybin"),
            sha256: String::new(),
            publisher: String::new(),
        };

        let plugin_dir = Path::new("/path/to/plugin");

        assert_eq!(
            manifest.binary_path(plugin_dir),
            Path::new("/path/to/plugin/mybin")
        );
    }

    #[test]
    fn signature_path_appends_minisig_suffix() {
        let manifest = Manifest {
            name: String::new(),
            version: String::new(),
            sdk_version: String::new(),
            binary: String::from("mybin"),
            sha256: String::new(),
            publisher: String::new(),
        };

        let plugin_dir = Path::new("/path/to/plugin");

        assert_eq!(
            manifest.signature_path(plugin_dir),
            Path::new("/path/to/plugin/mybin.minisig")
        );
    }
}
