//! Port of `pkg/license/validator.go`: squawk's own product-license
//! validator, talking to `license.squawkdns.com` (or a configured
//! override) — **not** `license.penguintech.io` (that's
//! `penguin-licensing::LicenseClient`, PenguinTech's own entitlement
//! service, an unrelated system). The squawk module's `license` command
//! uses only this one; `LicenseFeature` is deliberately empty because
//! squawk is core and must load with no license server configured at all.
//!
//! **Fixes a real Go bug** (see `docs/PARITY.md` §1.16): Go's
//! `handleLicense` builds a `LicenseConfig` with `ValidateOnline: true` but
//! `ModuleConfig` has no field to ever populate `ServerURL`, so every
//! request goes to a schemeless relative URL and fails before any network
//! I/O — `squawk license status` could never succeed, and always exited
//! `0` regardless. [`crate::config::LicenseConfig::server_url`] defaults to
//! squawk's real license server, so this actually works.
//!
//! **Simplifies away a second Go bug**: Go caches a key-vs-token validation
//! under two different map keys (`"license_validation"` vs
//! `"token_validation"`), but `ValidateLicense`'s own "already validated
//! today" fast path only ever checks the first key — so a token-based
//! deployment never benefits from it (every call re-hits the network),
//! even though `IsValid`'s separate cache scan (which iterates *all* keys)
//! does benefit. A `Validator`'s credential mode (key vs. token) never
//! changes at runtime, so there is no reason for two cache keys at all;
//! this port uses one, which removes the inconsistency entirely.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::config::LicenseConfig;
use crate::tls_support::ensure_crypto_provider_installed;

/// The one cache slot a [`Validator`] keeps — see the module doc comment
/// for why Go's two differently-named keys are collapsed into one here.
const CACHE_KEY: &str = "validation";
/// How long a cached valid result is trusted for [`Validator::is_valid`]'s
/// graceful-degradation fallback when the server is unreachable, on top of
/// (not instead of) `LicenseConfig::cache_time` — matches Go's `IsValid`
/// exactly (`time.Since(entry.validated) < 24*time.Hour`).
const FALLBACK_FRESHNESS: Duration = Duration::from_secs(24 * 3600);
const USER_AGENT: &str = "Squawk-DNS-Client/2.0";

/// The license server's validation response — matches Go's
/// `ValidationResponse` JSON shape exactly.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ValidationResponse {
    #[serde(default)]
    pub valid: bool,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub tokens_used: Option<i64>,
    #[serde(default)]
    pub max_tokens: Option<i64>,
    #[serde(default)]
    pub user_email: Option<String>,
    #[serde(default)]
    pub license_expires: Option<String>,
}

/// Every way license validation can fail.
#[derive(Debug, thiserror::Error)]
pub enum LicenseError {
    #[error("license key or user token is required")]
    NoCredentials,
    #[error("license server unreachable: {0}")]
    Unreachable(String),
    #[error("failed to parse response: {0}")]
    Decode(String),
}

struct CacheEntry {
    valid: bool,
    message: String,
    validated_at: SystemTime,
    expires_at: SystemTime,
}

struct ValidatorState {
    cache: HashMap<String, CacheEntry>,
    validated_today: Option<String>,
}

/// Validates a squawk product license/user token against
/// [`LicenseConfig::server_url`], with a daily short-circuit and an
/// offline-degradation fallback to the last cached valid result.
pub struct Validator {
    config: LicenseConfig,
    http: reqwest::Client,
    state: Mutex<ValidatorState>,
}

impl Validator {
    /// Builds a validator. Never touches the network — the first real
    /// request happens on the first [`validate_license`](Self::validate_license) call.
    pub fn new(config: LicenseConfig) -> Validator {
        ensure_crypto_provider_installed();
        Validator {
            config,
            http: build_http_client(),
            state: Mutex::new(ValidatorState {
                cache: HashMap::new(),
                validated_today: None,
            }),
        }
    }

    /// Validates against the server (or the cache, per
    /// `validate_online`/`cache_time`), preferring `user_token` over
    /// `license_key` when both are configured. Returns
    /// [`LicenseError::NoCredentials`] immediately when neither is set —
    /// this never happens over the network.
    pub async fn validate_license(&self) -> Result<ValidationResponse, LicenseError> {
        let today = today_string();

        if let Some(cached) = self.today_cached_result(&today) {
            return Ok(cached);
        }

        if !self.config.validate_online
            && let Some(cached) = self.fresh_offline_result()
        {
            return Ok(cached);
        }

        if !self.config.user_token.is_empty() {
            return self.validate_user_token().await;
        }
        if !self.config.license_key.is_empty() {
            return self.validate_license_key().await;
        }

        Err(LicenseError::NoCredentials)
    }

    /// Whether the current license/token is valid — validates fresh, but
    /// falls back to a cached valid result (within [`FALLBACK_FRESHNESS`]
    /// or the configured `cache_time`, whichever is more permissive) when
    /// the server is unreachable. Matches Go's `IsValid`.
    pub async fn is_valid(&self) -> Result<bool, LicenseError> {
        let today = today_string();
        if let Some(cached) = self.today_cached_result(&today)
            && cached.valid
        {
            return Ok(true);
        }

        match self.validate_license().await {
            Ok(response) => Ok(response.valid),
            Err(err) => {
                if self.has_recent_valid_cache() {
                    return Ok(true);
                }
                Err(err)
            }
        }
    }

    /// Alias for [`validate_license`](Self::validate_license) — matches
    /// Go's `GetStatus`, a separate method name for the same behavior.
    pub async fn get_status(&self) -> Result<ValidationResponse, LicenseError> {
        self.validate_license().await
    }

    pub fn clear_cache(&self) {
        let mut state = self.lock_state();
        state.cache.clear();
        state.validated_today = None;
    }

    /// A human-readable status block for the module's `license status`
    /// command.
    pub async fn get_license_info(&self) -> Result<String, LicenseError> {
        let status = self.validate_license().await?;
        let mark = if status.valid {
            "\u{2713} Valid"
        } else {
            "\u{2717} Invalid"
        };

        let mut info = format!("License Status: {mark}\n");
        if !status.message.is_empty() {
            info.push_str(&format!("Message: {}\n", status.message));
        }
        if let Some(expires) = &status.expires_at {
            info.push_str(&format!("License Expires: {expires}\n"));
        }
        if let Some(email) = &status.user_email {
            info.push_str(&format!("User: {email}\n"));
        }
        if let (Some(used), Some(max)) = (status.tokens_used, status.max_tokens) {
            info.push_str(&format!("Tokens: {used}/{max} used\n"));
        }
        Ok(info)
    }

    async fn validate_license_key(&self) -> Result<ValidationResponse, LicenseError> {
        let url = format!("{}/api/validate", self.config.server_url);

        #[derive(Serialize)]
        struct Payload<'a> {
            license_key: &'a str,
        }

        let response = self
            .http
            .post(&url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .json(&Payload {
                license_key: &self.config.license_key,
            })
            .send()
            .await
            .map_err(|err| LicenseError::Unreachable(err.to_string()))?;

        let parsed = self.decode_response(response).await?;
        self.cache_validation(parsed.valid, parsed.message.clone());
        Ok(parsed)
    }

    async fn validate_user_token(&self) -> Result<ValidationResponse, LicenseError> {
        let url = format!("{}/api/validate_token", self.config.server_url);

        let response = self
            .http
            .post(&url)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.config.user_token),
            )
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await
            .map_err(|err| LicenseError::Unreachable(err.to_string()))?;

        let parsed = self.decode_response(response).await?;
        self.cache_validation(parsed.valid, parsed.message.clone());
        Ok(parsed)
    }

    /// Reads and decodes the response body. Matches Go exactly in not
    /// checking the HTTP status code first — a non-200 response with a
    /// decodable JSON body (even `{"valid":false,...}`) is still treated as
    /// a normal answer, not a transport failure.
    async fn decode_response(
        &self,
        response: reqwest::Response,
    ) -> Result<ValidationResponse, LicenseError> {
        let body = response
            .bytes()
            .await
            .map_err(|err| LicenseError::Unreachable(err.to_string()))?;
        serde_json::from_slice(&body).map_err(|err| LicenseError::Decode(err.to_string()))
    }

    fn cache_validation(&self, valid: bool, message: String) {
        let now = SystemTime::now();
        let mut state = self.lock_state();
        state.cache.insert(
            CACHE_KEY.to_string(),
            CacheEntry {
                valid,
                message,
                validated_at: now,
                expires_at: now + cache_time_duration(self.config.cache_time),
            },
        );
        state.validated_today = Some(today_string());
    }

    /// If already validated today, the cached result — regardless of
    /// whether that validation came from the key or token path.
    fn today_cached_result(&self, today: &str) -> Option<ValidationResponse> {
        let state = self.lock_state();
        if state.validated_today.as_deref() != Some(today) {
            return None;
        }
        let entry = state.cache.get(CACHE_KEY)?;
        Some(ValidationResponse {
            valid: entry.valid,
            message: entry.message.clone(),
            ..Default::default()
        })
    }

    /// The cached result, if present and younger than `cache_time` — used
    /// only in offline mode (`validate_online == false`).
    fn fresh_offline_result(&self) -> Option<ValidationResponse> {
        let state = self.lock_state();
        let entry = state.cache.get(CACHE_KEY)?;
        let age = SystemTime::now()
            .duration_since(entry.validated_at)
            .unwrap_or(Duration::MAX);
        if age >= cache_time_duration(self.config.cache_time) {
            return None;
        }
        Some(ValidationResponse {
            valid: entry.valid,
            message: entry.message.clone(),
            ..Default::default()
        })
    }

    /// Whether a valid cached result exists that's either within
    /// [`FALLBACK_FRESHNESS`] or still within its own `cache_time` expiry —
    /// [`is_valid`](Self::is_valid)'s graceful-degradation check.
    fn has_recent_valid_cache(&self) -> bool {
        let state = self.lock_state();
        let Some(entry) = state.cache.get(CACHE_KEY) else {
            return false;
        };
        if !entry.valid {
            return false;
        }
        let now = SystemTime::now();
        let recent = now
            .duration_since(entry.validated_at)
            .map(|age| age < FALLBACK_FRESHNESS)
            .unwrap_or(true);
        let still_fresh = now < entry.expires_at;
        recent || still_fresh
    }

    fn lock_state(&self) -> MutexGuard<'_, ValidatorState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn cache_time_duration(cache_time_minutes: i64) -> Duration {
    Duration::from_secs(cache_time_minutes.max(0) as u64 * 60)
}

/// Today's date as `YYYY-MM-DD`, in UTC — matches Go's
/// `time.Now().Format("2006-01-02")` closely enough for the daily
/// short-circuit's purpose (a process-lifetime cache boundary, not a
/// timezone-sensitive calculation).
fn today_string() -> String {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days_since_epoch = secs / 86_400;
    let (year, month, day) = civil_from_days(days_since_epoch as i64);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Converts a day count since the Unix epoch to a proleptic-Gregorian
/// (year, month, day) triple. Howard Hinnant's well-known `civil_from_days`
/// algorithm — used here instead of pulling in a full date/time crate for
/// one calendar conversion.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
}

/// Builds the reqwest client used for every license-server request: rustls
/// with the aws-lc-rs crypto provider and the default webpki root store.
/// Go pins `MinVersion: tls.VersionTLS12`; rustls (with this workspace's
/// `tls12` feature) never negotiates below TLS 1.2 at all, so no explicit
/// minimum-version configuration is needed to match that.
fn build_http_client() -> reqwest::Client {
    let root_store = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    reqwest::Client::builder()
        .use_preconfigured_tls(tls_config)
        .timeout(Duration::from_secs(10))
        .build()
        .expect("license HTTP client config is static and always valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn today_string_matches_a_known_date() {
        // 2024-01-15T00:00:00Z
        let secs = 1_705_276_800u64;
        let (year, month, day) = civil_from_days((secs / 86_400) as i64);
        assert_eq!((year, month, day), (2024, 1, 15));
    }

    #[test]
    fn civil_from_days_handles_the_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }
}
