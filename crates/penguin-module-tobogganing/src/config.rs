//! Tobogganing's on-disk module configuration and the JSON Schema the
//! daemon validates it against before [`crate::module::TobogganingModule::init`]
//! ever reads it.
//!
//! Ported from the Go module's `ModuleConfig`/`ConfigSchema()`
//! (`go-client/internal/modules/tobogganing/module.go`), with one
//! deliberate fix: `dns` and `allowed_ips` are genuinely used now.
//!
//! Go declared both fields, populated them from the config file, and then
//! never read either one anywhere — `vpn.go`'s `Connect` built its
//! `wgtypes.Config` entirely from the *manager's* fetched `TunnelConfig`
//! (`tunnelCfg.AllowedIPs`), never `v.config.AllowedIPs`/`v.config.DNS`, and
//! `TunnelConfig.DNS` itself was fetched and then never applied either. This
//! port treats the manager's push config as authoritative (it is a ZTNA
//! policy server; it should win) but falls back to this module's own local
//! `dns`/`allowed_ips` when the manager sends none — see
//! [`crate::vpn::VpnManager::build_tunnel_spec`] for exactly where that
//! fallback happens. That makes both fields do something real instead of
//! being silently ignored, without discarding the manager's authority when
//! it does supply a value.
//!
//! `embedded` is the other field Go declared and read nowhere; see
//! [`crate::wireguard::select_backend`] for where this port wires it up.

use serde::{Deserialize, Serialize};

/// Default WireGuard interface name — matches Go's `Init` default exactly.
pub const DEFAULT_INTERFACE_NAME: &str = "wg0";
/// Default WireGuard MTU — matches Go's `Init` default exactly.
pub const DEFAULT_MTU: u32 = 1420;
/// Default persistent keepalive, in seconds — matches Go's `Init` default
/// exactly.
pub const DEFAULT_KEEPALIVE_SECS: u64 = 25;

/// Tobogganing's full on-disk config shape, validated by the daemon against
/// [`CONFIG_SCHEMA`] before [`crate::module::TobogganingModule::init`] ever
/// reads it.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ModuleConfig {
    /// Manager API base URL. Required — [`crate::module::TobogganingModule::init`]
    /// rejects an empty value even though the schema also marks it
    /// required, since a caller that skips schema validation (as this
    /// crate's own tests do, matching Go's `FakeHostServices`) must still
    /// be rejected.
    pub manager_url: String,
    /// This node's unique identifier. Required — see `manager_url`'s doc.
    pub node_id: String,
    /// WireGuard interface name.
    pub interface_name: String,
    /// WireGuard interface MTU.
    pub mtu: u32,
    /// DNS servers to use when the manager's tunnel config supplies none.
    /// See this module's doc for the fallback rule.
    pub dns: Vec<String>,
    /// Persistent keepalive interval, in seconds. `0` disables it.
    pub keepalive: u64,
    /// Allowed IPs (routes) to use when the manager's tunnel config
    /// supplies none. See this module's doc for the fallback rule.
    pub allowed_ips: Vec<String>,
    /// Selects the WireGuard backend: `true` (Go's own default) is the
    /// portable userspace engine, `false` opts into kernel WireGuard. See
    /// [`crate::wireguard::select_backend`].
    pub embedded: bool,
}

impl Default for ModuleConfig {
    fn default() -> ModuleConfig {
        ModuleConfig {
            manager_url: String::new(),
            node_id: String::new(),
            interface_name: DEFAULT_INTERFACE_NAME.to_string(),
            mtu: DEFAULT_MTU,
            dns: Vec::new(),
            keepalive: DEFAULT_KEEPALIVE_SECS,
            allowed_ips: Vec::new(),
            // Deliberately `false`, unlike Go's `true`.
            //
            // Go declared `embedded` and never read it, so its default was
            // inert — nothing ever selected a backend from it. This port makes
            // the field live, which means the default now decides whether the
            // module can connect at all. `true` selects the userspace engine,
            // whose data plane is not wired up yet and returns an explicit
            // Unsupported error, so shipping Go's default would leave the
            // module unable to establish a tunnel out of the box.
            //
            // Defaulting to the kernel path is the only default that works.
            // Setting `embedded: true` explicitly still selects userspace and
            // still fails loudly, which is the honest behaviour until that
            // path is finished.
            embedded: false,
        }
    }
}

/// The JSON Schema the daemon validates `tobogganing.yaml` against. Ported
/// verbatim from the Go module's `ConfigSchema()`.
pub const CONFIG_SCHEMA: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "type": "object",
  "properties": {
    "manager_url": {
      "type": "string",
      "description": "Manager API base URL"
    },
    "node_id": {
      "type": "string",
      "description": "Unique node identifier"
    },
    "interface_name": {
      "type": "string",
      "description": "WireGuard interface name",
      "default": "wg0"
    },
    "mtu": {
      "type": "integer",
      "description": "WireGuard MTU",
      "default": 1420
    },
    "dns": {
      "type": "array",
      "items": { "type": "string" },
      "description": "DNS servers to push"
    },
    "keepalive": {
      "type": "integer",
      "description": "WireGuard keepalive interval (seconds)",
      "default": 25
    },
    "allowed_ips": {
      "type": "array",
      "items": { "type": "string" },
      "description": "Allowed IPs on tunnel"
    },
    "embedded": {
      "type": "boolean",
      "description": "Use the embedded userspace WireGuard engine instead of kernel WireGuard. The userspace data plane is not implemented yet and returns an explicit error, so this defaults to false.",
      "default": false
    }
  },
  "required": ["manager_url", "node_id"]
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_go_defaults() {
        let cfg = ModuleConfig::default();
        assert_eq!(cfg.interface_name, "wg0");
        assert_eq!(cfg.mtu, 1420);
        assert_eq!(cfg.keepalive, 25);
        assert!(cfg.manager_url.is_empty());
        assert!(cfg.node_id.is_empty());
        assert!(cfg.dns.is_empty());
        assert!(cfg.allowed_ips.is_empty());
    }

    /// The one default that deliberately differs from Go.
    ///
    /// Go defaulted `embedded` to true but never read the field, so the value
    /// was inert. Making it live means the default now decides whether the
    /// module can connect at all, and `true` selects the userspace engine whose
    /// data plane is not implemented. A default that cannot establish a tunnel
    /// is not a faithful port of anything — it is just broken.
    #[test]
    fn embedded_defaults_to_the_backend_that_actually_works() {
        assert!(
            !ModuleConfig::default().embedded,
            "default must select the kernel backend; the userspace data plane is not implemented"
        );
    }

    /// The advertised schema default must match what the code actually applies.
    /// Squawk shipped a schema promising one DoH default while its code applied
    /// another; that contradiction is worth not repeating here.
    #[test]
    fn schema_default_for_embedded_matches_the_code_default() {
        let schema: serde_json::Value = serde_json::from_str(CONFIG_SCHEMA).unwrap();
        let advertised = &schema["properties"]["embedded"]["default"];
        assert_eq!(
            advertised.as_bool(),
            Some(ModuleConfig::default().embedded),
            "schema default and code default disagree"
        );
    }

    #[test]
    fn empty_document_deserializes_to_the_full_default() {
        let cfg: ModuleConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg, ModuleConfig::default());
    }

    #[test]
    fn partial_document_only_overrides_what_it_names() {
        let yaml = "manager_url: https://manager.example.com\nnode_id: node-1\n";
        let cfg: ModuleConfig = serde_norway::from_str(yaml).unwrap();
        assert_eq!(cfg.manager_url, "https://manager.example.com");
        assert_eq!(cfg.node_id, "node-1");
        assert_eq!(cfg.interface_name, "wg0");
        assert_eq!(cfg.mtu, 1420);
    }

    #[test]
    fn schema_is_valid_json_and_compiles() {
        let schema: serde_json::Value = serde_json::from_str(CONFIG_SCHEMA).unwrap();
        let validator = jsonschema::validator_for(&schema);
        assert!(validator.is_ok(), "schema must compile");
    }

    #[test]
    fn schema_requires_manager_url_and_node_id() {
        let schema: serde_json::Value = serde_json::from_str(CONFIG_SCHEMA).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let instance = serde_json::json!({});
        let errors: Vec<_> = validator.iter_errors(&instance).collect();
        assert!(!errors.is_empty(), "empty document must fail validation");
    }

    #[test]
    fn schema_accepts_a_minimal_valid_document() {
        let schema: serde_json::Value = serde_json::from_str(CONFIG_SCHEMA).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let instance = serde_json::json!({
            "manager_url": "https://manager.example.com",
            "node_id": "node-1",
        });
        let errors: Vec<_> = validator.iter_errors(&instance).collect();
        assert!(errors.is_empty(), "unexpected schema errors: {errors:?}");
    }
}
