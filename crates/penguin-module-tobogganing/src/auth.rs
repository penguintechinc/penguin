//! JWT token acquisition, renewal, and storage against the manager.
//!
//! A genuine port of `go-client/internal/modules/tobogganing/auth.go`:
//! same three endpoints, same request bodies, same 30s HTTP timeout, same
//! obtain/refresh/revoke/cache-load shape. The one behavioral fix is
//! documented on [`crate::module`]'s refresh loop, where it actually lives
//! (see that module's doc for why `AuthManager` itself needed no change).
//!
//! # Reading a token's `exp` without verifying its signature
//!
//! [`extract_expiry`] uses `jsonwebtoken::dangerous::insecure_decode`,
//! which performs zero signature validation. That is legitimate here, not
//! a shortcut: the token was just received over TLS directly from the
//! manager (or loaded from this host's own secret store, where the manager
//! put it), and the only thing being read is our own `exp` claim, to
//! schedule a refresh — not to authenticate anything. `insecure_decode`
//! also performs no expiry check of its own, which is required: a token
//! that has already expired must still decode successfully so
//! [`AuthManager::is_token_expired`] can correctly report it as expired,
//! rather than the decode step itself rejecting it first.
//!
//! # Wire format: `expires_at` is Unix seconds, not Go's RFC3339 string
//!
//! Go's `TokenResponse.ExpiresAt` is a `time.Time`, which `encoding/json`
//! marshals as an RFC3339 string. This workspace has no RFC3339 parser in
//! its dependency graph (`chrono`/`time` are not workspace dependencies,
//! and adding one is out of scope for this milestone — see this crate's
//! brief on writing only inside this crate's own `Cargo.toml`). Unix
//! seconds are unambiguous, need no timezone/format parsing at all, and
//! this client and the manager are both part of the same in-flight
//! migration, so the wire contract is free to be defined here rather than
//! inherited unexamined from the old Go client.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use penguin_sdk::SecretStore;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::http;

const TOKEN_PATH: &str = "/api/v1/auth/token";
const REFRESH_PATH: &str = "/api/v1/auth/refresh";
const REVOKE_PATH: &str = "/api/v1/auth/revoke";

/// Matches Go's `httpClient: &http.Client{Timeout: 30 * time.Second}`.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

const ACCESS_TOKEN_KEY: &str = "access_token";
const REFRESH_TOKEN_KEY: &str = "refresh_token";
const API_KEY_KEY: &str = "api_key";

/// Every way the auth lifecycle can fail.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// No refresh token is cached and [`SecretStore::get`] found no
    /// `api_key` either — matches Go's `"no API key found: %w"`.
    #[error("no API key found: {0}")]
    NoApiKey(String),
    /// The HTTP request itself failed (connection error, timeout, ...).
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    /// The manager answered, but not with 2xx.
    #[error("request failed with status {status}: {body}")]
    Status { status: u16, body: String },
    /// The 2xx response body was not the expected JSON shape.
    #[error("failed to parse response: {0}")]
    Decode(#[from] serde_json::Error),
    /// [`AuthManager::refresh_token`] was called with no refresh token
    /// cached — matches Go's `"no refresh token available"`. Only reached
    /// through that method, which nothing in this crate's production code
    /// currently calls directly (see its doc) — dead outside tests until
    /// a caller needs an explicit no-fallback refresh, kept for Go-parity
    /// rather than deleted.
    #[error("no refresh token available")]
    #[allow(dead_code)]
    NoRefreshToken,
}

/// The manager's response to an obtain or refresh request.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    /// See this module's doc for why this is Unix seconds rather than
    /// Go's RFC3339 string. `None` (the field absent, matching Go's
    /// `ExpiresAt.IsZero()`) means "extract from the access token's own
    /// `exp` claim instead" — see [`AuthManager::apply_response`].
    #[serde(default)]
    expires_at: Option<i64>,
}

/// The request body for `api_key`.
#[derive(Serialize)]
struct ObtainRequest<'a> {
    api_key: &'a str,
}

/// The request body for `refresh_token`.
#[derive(Serialize)]
struct RefreshRequest<'a> {
    refresh_token: &'a str,
}

/// The token state guarded by [`AuthManager`]'s single lock. `expires_at`
/// defaults to [`SystemTime::UNIX_EPOCH`] — the same "so far in the past
/// it always reads as expired" sentinel Go's zero-value `time.Time{}`
/// serves, so [`is_expired_at`] needs no separate "unknown" case.
struct TokenState {
    access_token: String,
    refresh_token: String,
    expires_at: SystemTime,
}

impl Default for TokenState {
    fn default() -> TokenState {
        TokenState {
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: SystemTime::UNIX_EPOCH,
        }
    }
}

/// Handles JWT token acquisition, renewal, and storage against the
/// manager. See this module's doc for what is and is not a direct Go port.
pub struct AuthManager {
    manager_url: String,
    secrets: Arc<dyn SecretStore>,
    http: reqwest::Client,
    state: Mutex<TokenState>,
}

impl AuthManager {
    /// Builds a new auth manager and best-effort loads any cached token
    /// from `secrets` — matches Go's `NewAuthManager` calling
    /// `loadCachedToken` and discarding its error.
    pub async fn new(manager_url: impl Into<String>, secrets: Arc<dyn SecretStore>) -> AuthManager {
        let mut state = TokenState::default();
        load_cached_token(secrets.as_ref(), &mut state).await;
        AuthManager {
            manager_url: manager_url.into(),
            secrets,
            http: http::build_client(HTTP_TIMEOUT),
            state: Mutex::new(state),
        }
    }

    /// Ensures a currently-valid token is cached, obtaining one if needed:
    /// returns immediately if the current token has not yet expired,
    /// otherwise tries a refresh, and — if there is no refresh token or
    /// the refresh itself fails — falls back to obtaining a fresh token
    /// from the cached API key. Matches Go's `EnsureValidToken` exactly.
    pub async fn ensure_valid_token(&self) -> Result<(), AuthError> {
        let mut state = self.state.lock().await;

        if !state.access_token.is_empty()
            && !is_expired_at(state.expires_at, SystemTime::now(), Duration::ZERO)
        {
            return Ok(());
        }

        if !state.refresh_token.is_empty() && self.refresh_locked(&mut state).await.is_ok() {
            return Ok(());
        }

        let api_key = self.get_api_key().await?;
        self.obtain_locked(&mut state, &api_key).await
    }

    /// Refreshes the access token using the cached refresh token only —
    /// no API-key fallback. This is Go's `RefreshToken`, kept as a direct,
    /// explicit primitive; the periodic refresh loop's Go-parity bug fix
    /// lives in `module.rs`, which calls [`Self::ensure_valid_token`]
    /// instead of this method — see that file's doc for why. Not called
    /// from this crate's production code (only its own tests), kept for
    /// Go-parity and as a primitive a future caller needing a
    /// no-fallback refresh can reach for.
    #[allow(dead_code)]
    pub async fn refresh_token(&self) -> Result<(), AuthError> {
        let mut state = self.state.lock().await;
        if state.refresh_token.is_empty() {
            return Err(AuthError::NoRefreshToken);
        }
        self.refresh_locked(&mut state).await
    }

    /// Reports whether the cached token will expire within `threshold`
    /// (or is already expired/absent). Matches Go's `IsTokenExpired`.
    pub async fn is_token_expired(&self, threshold: Duration) -> bool {
        let state = self.state.lock().await;
        if state.access_token.is_empty() {
            return true;
        }
        is_expired_at(state.expires_at, SystemTime::now(), threshold)
    }

    /// Revokes the current token on the manager and clears the local and
    /// cached copies. A no-op if there is no token to revoke. Matches
    /// Go's `RevokeToken`.
    pub async fn revoke_token(&self) -> Result<(), AuthError> {
        let token = {
            let state = self.state.lock().await;
            state.access_token.clone()
        };
        if token.is_empty() {
            return Ok(());
        }

        let url = format!("{}{REVOKE_PATH}", self.manager_url);
        let response = self
            .http
            .post(&url)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
            .send()
            .await
            .map_err(AuthError::Request)?;

        let status = response.status();
        if !status.is_success() {
            return Err(AuthError::Status {
                status: status.as_u16(),
                body: String::new(),
            });
        }

        {
            let mut state = self.state.lock().await;
            *state = TokenState::default();
        }
        let _ = self.secrets.delete(ACCESS_TOKEN_KEY).await;
        let _ = self.secrets.delete(REFRESH_TOKEN_KEY).await;
        Ok(())
    }

    /// The current access token, or empty if none is cached.
    pub async fn token(&self) -> String {
        self.state.lock().await.access_token.clone()
    }

    async fn get_api_key(&self) -> Result<String, AuthError> {
        let value = self
            .secrets
            .get(API_KEY_KEY)
            .await
            .map_err(|err| AuthError::NoApiKey(err.to_string()))?;
        Ok(String::from_utf8_lossy(&value).into_owned())
    }

    async fn refresh_locked(&self, state: &mut TokenState) -> Result<(), AuthError> {
        let body = RefreshRequest {
            refresh_token: &state.refresh_token,
        };
        let response = self.post_token(REFRESH_PATH, &body).await?;
        // Only overwrite the refresh token if the manager sent a new one —
        // matches Go's `refreshTokenLocked`, which preserves the existing
        // refresh token when the response omits it.
        self.apply_response(state, response, false).await;
        Ok(())
    }

    async fn obtain_locked(&self, state: &mut TokenState, api_key: &str) -> Result<(), AuthError> {
        let body = ObtainRequest { api_key };
        let response = self.post_token(TOKEN_PATH, &body).await?;
        // Unconditional overwrite, even to empty — matches Go's
        // `obtainTokenLocked`, which always sets `a.refreshToken =
        // tokenResp.RefreshToken` with no non-empty guard.
        self.apply_response(state, response, true).await;
        Ok(())
    }

    async fn post_token(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<TokenResponse, AuthError> {
        let url = format!("{}{path}", self.manager_url);
        let response = self
            .http
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(AuthError::Request)?;

        let status = response.status();
        let bytes = response.bytes().await.map_err(AuthError::Request)?;
        if !status.is_success() {
            return Err(AuthError::Status {
                status: status.as_u16(),
                body: String::from_utf8_lossy(&bytes).into_owned(),
            });
        }
        serde_json::from_slice(&bytes).map_err(AuthError::Decode)
    }

    /// Stores `response` into `state` and best-effort persists it to
    /// `secrets`. `overwrite_refresh_unconditionally` selects between
    /// obtain's and refresh's slightly different Go semantics — see the
    /// two call sites' doc comments.
    async fn apply_response(
        &self,
        state: &mut TokenState,
        response: TokenResponse,
        overwrite_refresh_unconditionally: bool,
    ) {
        state.access_token = response.access_token.clone();
        if overwrite_refresh_unconditionally || !response.refresh_token.is_empty() {
            state.refresh_token = response.refresh_token.clone();
        }
        state.expires_at = match response.expires_at {
            Some(seconds) => unix_seconds_to_system_time(seconds),
            None => extract_expiry(&state.access_token).unwrap_or(SystemTime::UNIX_EPOCH),
        };

        let _ = self
            .secrets
            .set(ACCESS_TOKEN_KEY, response.access_token.as_bytes())
            .await;
        if !response.refresh_token.is_empty() {
            let _ = self
                .secrets
                .set(REFRESH_TOKEN_KEY, response.refresh_token.as_bytes())
                .await;
        }
    }
}

/// Loads a cached access token (and, if present, refresh token) from
/// `secrets` into `state`. A missing access token leaves `state` at its
/// default — matches Go's `loadCachedToken`, whose error is always
/// discarded by its one caller.
async fn load_cached_token(secrets: &dyn SecretStore, state: &mut TokenState) {
    let Ok(access) = secrets.get(ACCESS_TOKEN_KEY).await else {
        return;
    };
    state.access_token = String::from_utf8_lossy(&access).into_owned();
    if let Some(expiry) = extract_expiry(&state.access_token) {
        state.expires_at = expiry;
    }
    if let Ok(refresh) = secrets.get(REFRESH_TOKEN_KEY).await {
        state.refresh_token = String::from_utf8_lossy(&refresh).into_owned();
    }
}

/// The claims [`extract_expiry`] reads — only `exp` is needed.
#[derive(Deserialize)]
struct ExpiryClaims {
    exp: i64,
}

/// Decodes `token`'s `exp` claim without verifying its signature. Returns
/// `None` if the token is malformed or carries no `exp` claim at all —
/// never if the claim is simply in the past (an already-expired token
/// must still decode). See this module's doc for why an unverified decode
/// is the correct tool here.
fn extract_expiry(token: &str) -> Option<SystemTime> {
    let data = jsonwebtoken::dangerous::insecure_decode::<ExpiryClaims>(token).ok()?;
    Some(unix_seconds_to_system_time(data.claims.exp))
}

/// Converts Unix seconds (which may be negative, for dates before 1970 —
/// not realistic for a token expiry, but handled rather than panicking) to
/// a [`SystemTime`].
fn unix_seconds_to_system_time(seconds: i64) -> SystemTime {
    if seconds >= 0 {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds as u64)
    } else {
        SystemTime::UNIX_EPOCH - Duration::from_secs(seconds.unsigned_abs())
    }
}

/// `true` if `expires_at` is less than `threshold` away from `now` (or
/// already past it). Matches Go's `time.Until(a.expiresAt) < threshold`:
/// an `expires_at` at or before `now` computes a negative/zero
/// "time until", which is always less than any non-negative threshold.
fn is_expired_at(expires_at: SystemTime, now: SystemTime, threshold: Duration) -> bool {
    match expires_at.duration_since(now) {
        Ok(remaining) => remaining < threshold,
        Err(_already_past) => true,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde::Serialize;

    use super::*;
    use crate::testutil::{InMemorySecretStore, MockManager, MockResponse};

    /// Installs jsonwebtoken's aws-lc-rs `CryptoProvider`, exactly once,
    /// so [`jwt_with_exp`] can mint a real signed JWT fixture. Production
    /// code (`extract_expiry`) never signs or verifies anything and needs
    /// no provider at all — this is test-fixture plumbing only.
    fn ensure_signer_installed() {
        use std::sync::OnceLock;
        static INSTALLED: OnceLock<()> = OnceLock::new();
        INSTALLED.get_or_init(|| {
            let _ = jsonwebtoken::crypto::aws_lc::DEFAULT_PROVIDER.install_default();
        });
    }

    /// Builds an HS256 JWT with only an `exp` claim — signed with a
    /// throwaway secret, since [`extract_expiry`] never checks the
    /// signature at all.
    fn jwt_with_exp(exp_unix_seconds: i64) -> String {
        ensure_signer_installed();
        #[derive(Serialize)]
        struct Claims {
            exp: i64,
        }
        encode(
            &Header::default(),
            &Claims {
                exp: exp_unix_seconds,
            },
            &EncodingKey::from_secret(b"throwaway-test-secret"),
        )
        .expect("encode test JWT")
    }

    fn unix_seconds_from_now(delta: i64) -> i64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        now + delta
    }

    #[test]
    fn extract_expiry_decodes_a_future_claim_without_verifying_signature() {
        let token = jwt_with_exp(unix_seconds_from_now(3600));
        let expiry = extract_expiry(&token).expect("exp claim present");
        let remaining = expiry.duration_since(SystemTime::now()).unwrap();
        assert!(remaining.as_secs() > 3000, "expiry should be ~1h out");
    }

    /// Regression: an already-expired token must still decode — the
    /// decode step performs no validation at all, only the caller's
    /// separate `is_token_expired` check does.
    #[test]
    fn extract_expiry_decodes_an_already_expired_claim() {
        let token = jwt_with_exp(unix_seconds_from_now(-3600));
        let expiry = extract_expiry(&token).expect("exp claim present, even if past");
        assert!(expiry < SystemTime::now());
    }

    #[test]
    fn extract_expiry_returns_none_for_a_malformed_token() {
        assert!(extract_expiry("not-a-jwt").is_none());
    }

    #[tokio::test]
    async fn obtain_token_via_api_key_caches_token_and_refresh_token() {
        let manager = MockManager::start().await;
        manager
            .respond(
                "POST",
                TOKEN_PATH,
                MockResponse::json(
                    200,
                    r#"{"access_token":"tok-1","refresh_token":"ref-1","expires_at":9999999999}"#,
                ),
            )
            .await;

        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.set("api_key", b"test-api-key").await.unwrap();
        let auth = AuthManager::new(manager.base_url.clone(), secrets.clone()).await;

        auth.ensure_valid_token().await.expect("obtain succeeds");
        assert_eq!(auth.token().await, "tok-1");
        assert!(!auth.is_token_expired(Duration::from_secs(60)).await);

        // Cached to secrets, matching Go's `secrets.Set` calls.
        assert_eq!(secrets.get("access_token").await.unwrap(), b"tok-1");
        assert_eq!(secrets.get("refresh_token").await.unwrap(), b"ref-1");

        let requests = manager.requests().await;
        assert_eq!(requests.len(), 1);
        assert!(requests[0].body.contains(r#""api_key":"test-api-key""#));

        manager.stop().await;
    }

    #[tokio::test]
    async fn obtain_token_without_explicit_expiry_falls_back_to_jwt_exp() {
        let manager = MockManager::start().await;
        let jwt = jwt_with_exp(unix_seconds_from_now(1800));
        manager
            .respond(
                "POST",
                TOKEN_PATH,
                MockResponse::json(200, format!(r#"{{"access_token":"{jwt}"}}"#)),
            )
            .await;

        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.set("api_key", b"k").await.unwrap();
        let auth = AuthManager::new(manager.base_url.clone(), secrets).await;

        auth.ensure_valid_token().await.expect("obtain succeeds");
        assert!(!auth.is_token_expired(Duration::from_secs(60)).await);
        assert!(auth.is_token_expired(Duration::from_secs(3600)).await);

        manager.stop().await;
    }

    #[tokio::test]
    async fn refresh_token_updates_cached_token() {
        let manager = MockManager::start().await;
        manager
            .respond(
                "POST",
                TOKEN_PATH,
                MockResponse::json(
                    200,
                    r#"{"access_token":"tok-1","refresh_token":"ref-1","expires_at":9999999999}"#,
                ),
            )
            .await;
        manager
            .respond(
                "POST",
                REFRESH_PATH,
                MockResponse::json(
                    200,
                    r#"{"access_token":"tok-2","refresh_token":"ref-2","expires_at":9999999999}"#,
                ),
            )
            .await;

        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.set("api_key", b"k").await.unwrap();
        let auth = AuthManager::new(manager.base_url.clone(), secrets).await;
        auth.ensure_valid_token().await.unwrap();

        auth.refresh_token().await.expect("refresh succeeds");
        assert_eq!(auth.token().await, "tok-2");

        let requests = manager.requests().await;
        let refresh_req = requests
            .iter()
            .find(|r| r.path == REFRESH_PATH)
            .expect("refresh request recorded");
        assert!(refresh_req.body.contains(r#""refresh_token":"ref-1""#));

        manager.stop().await;
    }

    #[tokio::test]
    async fn revoke_token_sends_bearer_header_and_clears_cache() {
        let manager = MockManager::start().await;
        manager
            .respond(
                "POST",
                TOKEN_PATH,
                MockResponse::json(
                    200,
                    r#"{"access_token":"tok-1","refresh_token":"ref-1","expires_at":9999999999}"#,
                ),
            )
            .await;
        manager
            .respond("POST", REVOKE_PATH, MockResponse::empty(200))
            .await;

        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.set("api_key", b"k").await.unwrap();
        let auth = AuthManager::new(manager.base_url.clone(), secrets.clone()).await;
        auth.ensure_valid_token().await.unwrap();

        auth.revoke_token().await.expect("revoke succeeds");
        assert_eq!(auth.token().await, "");
        assert!(secrets.get("access_token").await.is_err());
        assert!(secrets.get("refresh_token").await.is_err());

        let requests = manager.requests().await;
        let revoke_req = requests
            .iter()
            .find(|r| r.path == REVOKE_PATH)
            .expect("revoke request recorded");
        assert_eq!(revoke_req.header("authorization"), Some("Bearer tok-1"));

        manager.stop().await;
    }

    #[tokio::test]
    async fn revoke_token_with_nothing_cached_is_a_noop() {
        let secrets = Arc::new(InMemorySecretStore::default());
        let auth = AuthManager::new("http://127.0.0.1:1", secrets).await;
        auth.revoke_token().await.expect("no-op revoke succeeds");
    }

    #[tokio::test]
    async fn ensure_valid_token_without_api_key_or_refresh_token_fails() {
        let secrets = Arc::new(InMemorySecretStore::default());
        let auth = AuthManager::new("http://127.0.0.1:1", secrets).await;
        let err = auth.ensure_valid_token().await.unwrap_err();
        assert!(matches!(err, AuthError::NoApiKey(_)));
    }

    /// Regression for the module-level Go bug this milestone's brief calls
    /// out: `ensure_valid_token` (what `module.rs`'s refresh loop now
    /// calls instead of a bare `refresh_token`) must fall back to the API
    /// key when the cached refresh token is rejected, rather than failing
    /// forever. `AuthManager` itself already had this fallback in Go too —
    /// this test proves the Rust port preserves it, since the loop-level
    /// fix (in `module.rs`) depends on this method actually doing it.
    #[tokio::test]
    async fn ensure_valid_token_falls_back_to_api_key_when_refresh_fails() {
        let manager = MockManager::start().await;
        manager
            .respond("POST", REFRESH_PATH, MockResponse::json(401, "{}"))
            .await;
        manager
            .respond(
                "POST",
                TOKEN_PATH,
                MockResponse::json(
                    200,
                    r#"{"access_token":"fresh-tok","expires_at":9999999999}"#,
                ),
            )
            .await;

        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.set("api_key", b"k").await.unwrap();
        // `load_cached_token` only loads a cached refresh token when a
        // cached access token is present too (matches Go's
        // `loadCachedToken`, which returns early once `Get("access_token")`
        // fails) — so an access token, even an unparsable placeholder that
        // reads as already-expired, must be cached alongside the refresh
        // token for the refresh token to actually be loaded at all.
        secrets
            .set("access_token", b"stale-access-token")
            .await
            .unwrap();
        secrets
            .set("refresh_token", b"stale-refresh-token")
            .await
            .unwrap();
        let auth = AuthManager::new(manager.base_url.clone(), secrets).await;

        auth.ensure_valid_token()
            .await
            .expect("falls back to API key after refresh fails");
        assert_eq!(auth.token().await, "fresh-tok");

        let requests = manager.requests().await;
        assert!(requests.iter().any(|r| r.path == REFRESH_PATH));
        assert!(requests.iter().any(|r| r.path == TOKEN_PATH));

        manager.stop().await;
    }

    #[tokio::test]
    async fn is_token_expired_matches_go_threshold_semantics() {
        let secrets = Arc::new(InMemorySecretStore::default());
        let auth = AuthManager::new("http://127.0.0.1:1", secrets).await;

        // No token at all: always expired.
        assert!(auth.is_token_expired(Duration::ZERO).await);

        {
            let mut state = auth.state.lock().await;
            state.access_token = "test-token".to_string();
            state.expires_at = SystemTime::now() + Duration::from_secs(5 * 60);
        }

        assert!(!auth.is_token_expired(Duration::from_secs(4 * 60)).await);
        assert!(auth.is_token_expired(Duration::from_secs(6 * 60)).await);
    }

    #[tokio::test]
    async fn load_cached_token_on_construction_restores_state() {
        let secrets = Arc::new(InMemorySecretStore::default());
        let jwt = jwt_with_exp(unix_seconds_from_now(1800));
        secrets.set("access_token", jwt.as_bytes()).await.unwrap();
        secrets
            .set("refresh_token", b"cached-refresh")
            .await
            .unwrap();

        let auth = AuthManager::new("http://127.0.0.1:1", secrets).await;
        assert_eq!(auth.token().await, jwt);
        assert!(!auth.is_token_expired(Duration::from_secs(60)).await);
    }
}
