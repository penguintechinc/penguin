//! VS Code hook shim: merges two flat `waddleai.*` keys into VS Code's user
//! `settings.json`.
//!
//! VS Code has no generic hook-array mechanism like Claude Code or
//! Gemini/AGY — extensions read arbitrary keys out of the same flat
//! `settings.json` an operator's own preferences live in (`"editor.
//! fontSize"`, etc.), so this shim follows that convention rather than
//! inventing an array shape VS Code itself has no concept of. A companion
//! WaddleAI VS Code extension (out of scope for this crate) is expected to
//! read `waddleai.enabled`/`waddleai.hookCommand` and drive the actual
//! tool-call gating; this shim only ever writes the two settings.
//!
//! `dirs::config_dir()` resolves to the exact directory VS Code itself uses
//! on every platform this workspace targets: `~/.config` on Linux,
//! `~/Library/Application Support` on macOS, `%APPDATA%` on Windows — VS
//! Code's own `User` settings always live at `<config_dir>/Code/User/
//! settings.json`.

use std::path::PathBuf;

use serde_json::Value;

use super::{Ecosystem, HOOK_COMMAND, Shim, ShimError};

/// Installs into VS Code's user `settings.json` (or a caller-supplied path
/// in tests).
pub struct VsCodeShim {
    override_path: Option<PathBuf>,
}

impl VsCodeShim {
    /// The real, production shim: resolves the platform config directory at
    /// [`Shim::target_path`] time.
    pub fn new() -> VsCodeShim {
        VsCodeShim {
            override_path: None,
        }
    }

    /// A shim pointed at a fixed path, for tests that must never touch a
    /// real config directory.
    pub fn with_target(path: PathBuf) -> VsCodeShim {
        VsCodeShim {
            override_path: Some(path),
        }
    }
}

impl Default for VsCodeShim {
    fn default() -> VsCodeShim {
        VsCodeShim::new()
    }
}

impl Shim for VsCodeShim {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::VsCode
    }

    fn target_path(&self) -> Result<PathBuf, ShimError> {
        if let Some(path) = &self.override_path {
            return Ok(path.clone());
        }
        let config_dir = dirs::config_dir().ok_or(ShimError::NoConfigDir)?;
        Ok(config_dir.join("Code").join("User").join("settings.json"))
    }

    fn merge(&self, document: &mut Value) {
        // `install`'s contract (see `super::read_document`) guarantees
        // `document` is always a JSON object before `merge` ever runs —
        // provably infallible here, not a real runtime condition.
        let root = document
            .as_object_mut()
            .expect("Shim::merge contract: document is always a JSON object");

        // Flat, VS-Code-style dotted keys, overwritten unconditionally —
        // trivially idempotent, and never touches any other key already in
        // the document.
        root.insert("waddleai.enabled".to_string(), Value::Bool(true));
        root.insert(
            "waddleai.hookCommand".to_string(),
            Value::String(HOOK_COMMAND.to_string()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn target_path_defaults_to_the_real_config_directory() {
        let shim = VsCodeShim::new();
        let path = shim
            .target_path()
            .expect("config dir resolves in this test env");
        assert!(path.ends_with("Code/User/settings.json"));
    }

    #[test]
    fn merge_sets_both_keys_on_an_empty_document() {
        let shim = VsCodeShim::with_target(PathBuf::new());
        let mut document = json!({});
        shim.merge(&mut document);

        assert_eq!(document["waddleai.enabled"], true);
        assert_eq!(document["waddleai.hookCommand"], HOOK_COMMAND);
    }

    #[test]
    fn merge_preserves_unrelated_existing_settings() {
        let shim = VsCodeShim::with_target(PathBuf::new());
        let mut document = json!({"editor.fontSize": 14, "workbench.colorTheme": "Default Dark+"});
        shim.merge(&mut document);

        assert_eq!(document["editor.fontSize"], 14);
        assert_eq!(document["workbench.colorTheme"], "Default Dark+");
        assert_eq!(document["waddleai.enabled"], true);
    }

    #[test]
    fn merge_is_idempotent() {
        let shim = VsCodeShim::with_target(PathBuf::new());
        let mut document = json!({});
        shim.merge(&mut document);
        shim.merge(&mut document);

        assert_eq!(document.as_object().unwrap().len(), 2);
    }
}
