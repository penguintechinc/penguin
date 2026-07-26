//! Client configuration: which hub, which community, and which credential a
//! [`crate::WaddlebotClient`] uses.

use std::time::Duration;

/// waddlebot's production hub — see `~/.claude/rules/penguintech.md`'s
/// product domain table (`waddlebot` → `waddles.app`). [`Config::default`]
/// points here so a config built with only `community_id`/`cat` overridden
/// talks to the real hub; override `base_url` for local/mock testing.
pub const DEFAULT_BASE_URL: &str = "https://waddles.app/api/v1";

/// Default per-request timeout — matches squawk-client's `doh::DohClient`
/// default.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Configuration for [`crate::WaddlebotClient`].
#[derive(Debug, Clone)]
pub struct Config {
    /// The hub API's base URL, e.g. `https://waddles.app/api/v1`. A
    /// trailing slash is tolerated — [`crate::WaddlebotClient::new`]
    /// normalizes it.
    pub base_url: String,
    /// The community this client acts on. Every admin-scoped endpoint is
    /// namespaced under `/admin/{community_id}/...`.
    pub community_id: i64,
    /// The Community Access Token secret (`wdl_c_<hex>`), sent as
    /// `Authorization: Bearer <cat>` on every request. See
    /// [`crate::error::WaddlebotError::Auth`] for the current server-side
    /// caveat on this credential.
    pub cat: String,
    /// Per-request timeout, applied to the whole `WaddlebotClient`'s
    /// underlying HTTP client.
    pub timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            base_url: DEFAULT_BASE_URL.to_string(),
            community_id: 0,
            cat: String::new(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_points_at_the_real_hub_with_a_thirty_second_timeout() {
        let config = Config::default();
        assert_eq!(config.base_url, "https://waddles.app/api/v1");
        assert_eq!(config.timeout, Duration::from_secs(30));
        assert_eq!(config.community_id, 0);
        assert!(config.cat.is_empty());
    }
}
