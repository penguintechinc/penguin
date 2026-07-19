//! squawk's on-disk module configuration and the JSON Schema the daemon
//! validates it against before `init` ever sees it.
//!
//! Every field carries its own default (via `#[serde(default)]` at every
//! nesting level), which reproduces Go's "unmarshal on top of an
//! already-defaulted struct" merge behaviour in one step: a document that
//! omits a section, or omits one field within a present section, ends up
//! with exactly the same value a full document listing every default
//! explicitly would have produced.

use serde::{Deserialize, Serialize};

/// The DoH server URL squawk actually applies when the operator configures
/// none.
///
/// **Deliberately not** `https://dns.penguintech.io/dns-query`, which is
/// what the Go module's [`CONFIG_SCHEMA`] advertised as the default while
/// the Go code itself applied a different one — a real, load-bearing
/// contradiction (see this milestone's brief, Part 3).
/// [`squawk_client::doh::DohClient::new`] rejects any server host that
/// is not an IP literal, `localhost`, or one of a small allow-list of
/// well-known public resolvers (`squawk_client::doh`'s internal
/// `ALLOWED_DOH_HOSTS`) — a bare hostname like `dns.penguintech.io` fails
/// construction immediately, so the schema's old default could never
/// actually have worked. `127.0.0.1` is both a valid IP literal and, per
/// the Go source's own comment, chosen specifically to prevent the DoH
/// client's upstream lookup from looping back through the host's own
/// resolver.
pub const DEFAULT_DOH_SERVER_URL: &str = "https://127.0.0.1:443/dns-query";

/// Cache entry cap the forwarder's answer cache is built with. Go had no
/// cache at all; this matches
/// [`squawk_client::forwarder::CacheConfig::default`]'s own value.
pub const DEFAULT_CACHE_MAX_ENTRIES: usize = 10_000;

/// squawk's full on-disk config shape, validated by the daemon against
/// [`CONFIG_SCHEMA`] before [`crate::SquawkModule::init`] ever reads it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ModuleConfig {
    pub doh: DohSection,
    pub forwarder: ForwarderSection,
    pub system_dns: SystemDnsSection,
    pub cache: CacheSection,
    pub ntp: NtpSection,
    /// squawk's own product-license validation settings — reused directly
    /// from `squawk-client` rather than re-declared here, so this section's
    /// default (crucially, `server_url`) can never drift from what
    /// [`squawk_client::license::Validator`] actually uses. See that type's
    /// module doc for the Go `ServerURL` bug this fixes.
    pub license: squawk_client::config::LicenseConfig,
}

/// DoH client settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct DohSection {
    pub server_url: String,
    pub verify_tls: bool,
    pub auth_token: String,
    pub client_cert: String,
    pub client_key: String,
    pub ca_cert: String,
}

impl Default for DohSection {
    fn default() -> DohSection {
        DohSection {
            server_url: DEFAULT_DOH_SERVER_URL.to_string(),
            verify_tls: true,
            auth_token: String::new(),
            client_cert: String::new(),
            client_key: String::new(),
            ca_cert: String::new(),
        }
    }
}

/// Local `:53` forwarder settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct ForwarderSection {
    pub enabled: bool,
    pub udp_addr: String,
    pub tcp_addr: String,
}

impl Default for ForwarderSection {
    fn default() -> ForwarderSection {
        ForwarderSection {
            enabled: false,
            udp_addr: "127.0.0.1:53".to_string(),
            tcp_addr: "127.0.0.1:53".to_string(),
        }
    }
}

/// System DNS resolver management toggle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct SystemDnsSection {
    pub manage: bool,
}

/// Forwarder answer-cache toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct CacheSection {
    pub enabled: bool,
}

impl Default for CacheSection {
    fn default() -> CacheSection {
        CacheSection { enabled: true }
    }
}

/// NTP server pool used by the `time` command. Empty means
/// [`squawk_client::ntp::NtpClient`]'s own built-in public pool.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct NtpSection {
    pub server_urls: Vec<String>,
}

/// The JSON Schema the daemon validates `squawk.yaml` against.
///
/// Ported from the Go module's `ConfigSchema()`, with two fixes: the `doh`
/// section's advertised default now matches what this crate actually
/// applies ([`DEFAULT_DOH_SERVER_URL`], not the unreachable
/// `dns.penguintech.io` hostname Go advertised — see that constant's doc),
/// and a new `license` section documents [`squawk_client::config::LicenseConfig`]'s
/// fields, including the `server_url` this milestone adds.
pub const CONFIG_SCHEMA: &str = r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "type": "object",
  "properties": {
    "doh": {
      "type": "object",
      "properties": {
        "server_url": {
          "type": "string",
          "description": "DoH server URL",
          "default": "https://127.0.0.1:443/dns-query"
        },
        "verify_tls": {
          "type": "boolean",
          "description": "Verify TLS certificate",
          "default": true
        },
        "auth_token": {
          "type": "string",
          "description": "Authentication token (prefer secrets)"
        },
        "client_cert": {
          "type": "string",
          "description": "mTLS client certificate path"
        },
        "client_key": {
          "type": "string",
          "description": "mTLS client key path"
        },
        "ca_cert": {
          "type": "string",
          "description": "CA certificate path for server verification"
        }
      }
    },
    "forwarder": {
      "type": "object",
      "properties": {
        "enabled": {
          "type": "boolean",
          "description": "Enable local DNS forwarding on :53",
          "default": false
        },
        "udp_addr": {
          "type": "string",
          "description": "UDP listen address",
          "default": "127.0.0.1:53"
        },
        "tcp_addr": {
          "type": "string",
          "description": "TCP listen address",
          "default": "127.0.0.1:53"
        }
      }
    },
    "system_dns": {
      "type": "object",
      "properties": {
        "manage": {
          "type": "boolean",
          "description": "Manage system DNS resolver",
          "default": false
        }
      }
    },
    "cache": {
      "type": "object",
      "properties": {
        "enabled": {
          "type": "boolean",
          "description": "Enable DNS result caching",
          "default": true
        }
      }
    },
    "ntp": {
      "type": "object",
      "properties": {
        "server_urls": {
          "type": "array",
          "items": { "type": "string" },
          "description": "NTP server pool for the `time` command; empty uses the built-in public pool",
          "default": []
        }
      }
    },
    "license": {
      "type": "object",
      "properties": {
        "server_url": {
          "type": "string",
          "description": "squawk product license server URL",
          "default": "https://license.squawkdns.com"
        },
        "license_key": {
          "type": "string",
          "description": "Product license key (prefer secrets)"
        },
        "user_token": {
          "type": "string",
          "description": "User auth token, validated instead of license_key when set (prefer secrets)"
        },
        "validate_online": {
          "type": "boolean",
          "description": "Contact the license server; false relies solely on the cached result",
          "default": true
        },
        "cache_time": {
          "type": "integer",
          "description": "Minutes a cached validation result stays usable when validate_online is false",
          "default": 1440
        }
      }
    }
  }
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_applies_the_ip_literal_doh_url() {
        let cfg = ModuleConfig::default();
        assert_eq!(cfg.doh.server_url, DEFAULT_DOH_SERVER_URL);
        assert!(cfg.doh.verify_tls);
        assert!(!cfg.forwarder.enabled);
        assert!(!cfg.system_dns.manage);
        assert!(cfg.cache.enabled);
        assert!(cfg.ntp.server_urls.is_empty());
        assert_eq!(cfg.license.server_url, "https://license.squawkdns.com");
    }

    #[test]
    fn empty_document_deserializes_to_the_full_default() {
        let cfg: ModuleConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg, ModuleConfig::default());
    }

    #[test]
    fn partial_document_only_overrides_what_it_names() {
        let yaml = "forwarder:\n  enabled: true\n";
        let cfg: ModuleConfig = serde_norway::from_str(yaml).unwrap();
        assert!(cfg.forwarder.enabled);
        // Untouched fields, including the rest of the same section, keep
        // their defaults.
        assert_eq!(cfg.forwarder.udp_addr, "127.0.0.1:53");
        assert_eq!(cfg.doh.server_url, DEFAULT_DOH_SERVER_URL);
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
}
