//! Registry of built-in modules the daemon can load without an external
//! plugin binary.
//!
//! `squawk` was the first entry, proving the M1-M4 plugin framework end to
//! end. `tobogganing` is the second — and, per `docs/PARITY.md`, the last
//! Go-dependent module: once it is registered here, the shipped agent has
//! no Go left in it.

use std::collections::BTreeMap;

use penguin_sdk::Factory;

/// Builds the name -> [`Factory`] map for every compiled-in module, for
/// [`penguin_daemon::supervisor::SupervisorConfig::registry`] (see
/// `bins/penguind/src/daemon_main.rs`).
pub fn builtin_modules() -> BTreeMap<String, Factory> {
    let mut registry: BTreeMap<String, Factory> = BTreeMap::new();
    registry.insert("squawk".to_string(), penguin_module_squawk::factory);
    registry.insert(
        "tobogganing".to_string(),
        penguin_module_tobogganing::factory,
    );
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squawk_is_registered_as_a_builtin() {
        let registry = builtin_modules();
        assert!(registry.contains_key("squawk"));
    }

    #[test]
    fn squawk_factory_reports_its_own_identity_before_init() {
        let registry = builtin_modules();
        let factory = registry.get("squawk").expect("squawk registered");
        let info = factory().info();
        assert_eq!(info.name, "squawk");
        assert_eq!(info.version, "1.0.0");
        assert!(
            info.license_feature.is_empty(),
            "squawk is core product and must load with no license gate"
        );
    }

    #[test]
    fn tobogganing_is_registered_as_a_builtin() {
        let registry = builtin_modules();
        assert!(registry.contains_key("tobogganing"));
    }

    #[test]
    fn tobogganing_factory_reports_its_own_identity_before_init() {
        let registry = builtin_modules();
        let factory = registry.get("tobogganing").expect("tobogganing registered");
        let info = factory().info();
        assert_eq!(info.name, "tobogganing");
        assert_eq!(info.version, "1.0.0");
        assert!(
            info.license_feature.is_empty(),
            "tobogganing is core product and must load with no license gate"
        );
    }
}
