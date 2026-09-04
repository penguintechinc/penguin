//! Port of the subset of `pkg/config` the ported packages actually need.
//!
//! The Go `pkg/config` package also defines `AppConfig`, `DHCPConfig`,
//! `NTPConfig` (the intercept/NTS-KE variant), `FeaturesConfig`,
//! `TransportConfig`, plus `LoadConfig`/env-var/YAML-file plumbing — none of
//! that is ported, since the penguin squawk module never used it and the
//! brief scopes this crate to "config structs the [ported] above need".
//! [`crate::doh::Config`], [`crate::forwarder::Config`], and
//! [`crate::ntp::ClientConfig`]/[`crate::ntp::ForwarderConfig`] all live in
//! their own modules, matching the Go source's layout (`pkg/client`,
//! `pkg/forwarder`, `pkg/time` each define their own `Config`). Only
//! `LicenseConfig` actually lived in `pkg/config` itself.

use serde::{Deserialize, Serialize};

/// squawk's own product-license server default — see [`LicenseConfig::server_url`].
pub const DEFAULT_LICENSE_SERVER_URL: &str = "https://license.squawkdns.com";

/// Default cache time for an offline-mode validation result, in minutes
/// (24 hours) — matches the Go `DefaultConfig()` value exactly.
const DEFAULT_CACHE_TIME_MINUTES: i64 = 1440;

/// License validation configuration, ported from Go's `pkg/config.LicenseConfig`.
///
/// **Fixes a real Go bug** (see `docs/PARITY.md` §1.16): the Go
/// `ModuleConfig` never has a way to populate `LicenseConfig.ServerURL`, so
/// `handleLicense` always builds a client pointed at `""` and every
/// validation request fails before any network I/O — `squawk license
/// status` could never work. Here, [`LicenseConfig::server_url`] defaults to
/// squawk's real license server via `#[serde(default)]`, so a config that
/// never mentions `license.server_url` still validates successfully.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct LicenseConfig {
    /// The license server's base URL, e.g. `https://license.squawkdns.com`.
    /// Defaults to [`DEFAULT_LICENSE_SERVER_URL`] when omitted from
    /// deserialized config — see the type-level doc comment for why that
    /// default (rather than an empty string, as Go effectively shipped) is
    /// the whole point of this field's existence.
    #[serde(default = "default_server_url")]
    pub server_url: String,
    /// A product license key. Mutually exclusive in practice with
    /// `user_token` — [`crate::license::Validator`] prefers `user_token`
    /// when both are set, matching Go.
    #[serde(default)]
    pub license_key: String,
    /// A user auth token, validated via the bearer-token endpoint instead
    /// of the license-key endpoint.
    #[serde(default)]
    pub user_token: String,
    /// Whether to contact the license server at all versus relying solely
    /// on the cached result until it exceeds `cache_time`.
    #[serde(default = "default_true")]
    pub validate_online: bool,
    /// How long a cached validation result stays usable when
    /// `validate_online` is `false`, in minutes.
    #[serde(default = "default_cache_time")]
    pub cache_time: i64,
}

impl Default for LicenseConfig {
    fn default() -> Self {
        LicenseConfig {
            server_url: default_server_url(),
            license_key: String::new(),
            user_token: String::new(),
            validate_online: true,
            cache_time: DEFAULT_CACHE_TIME_MINUTES,
        }
    }
}

fn default_server_url() -> String {
    DEFAULT_LICENSE_SERVER_URL.to_string()
}

fn default_true() -> bool {
    true
}

fn default_cache_time() -> i64 {
    DEFAULT_CACHE_TIME_MINUTES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_points_at_squawks_real_license_server() {
        let config = LicenseConfig::default();
        assert_eq!(config.server_url, "https://license.squawkdns.com");
        assert!(config.validate_online);
        assert_eq!(config.cache_time, 1440);
        assert!(config.license_key.is_empty());
        assert!(config.user_token.is_empty());
    }

    #[test]
    fn deserializing_an_empty_object_fills_every_default() {
        let config: LicenseConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config, LicenseConfig::default());
    }

    #[test]
    fn deserializing_overrides_only_the_fields_present() {
        let config: LicenseConfig = serde_json::from_str(r#"{"license_key":"abc123"}"#).unwrap();
        assert_eq!(config.license_key, "abc123");
        assert_eq!(config.server_url, "https://license.squawkdns.com");
    }
}
