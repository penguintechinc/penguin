//! Keychain-backed session token store using penguin-secrets.
//!
//! Stores and retrieves the user's access token, refresh token, and hub base URL
//! in the OS keychain (via penguin-secrets), isolated in the `waddlebot-desktop`
//! namespace so tokens never get mixed with other product's secrets.
//!
//! Tests use Backend::FileOnly in a temporary directory.

use penguin_sdk::SecretStore;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::debug;

use crate::error::{DesktopError, Result};

const NAMESPACE: &str = "waddlebot-desktop";
const KEY_ACCESS_TOKEN: &str = "access_token";
const KEY_REFRESH_TOKEN: &str = "refresh_token";
const KEY_HUB_BASE_URL: &str = "hub_base_url";

/// A stored session with access token, optional refresh token, and hub URL.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredSession {
    /// The user's current access token (Bearer).
    pub access_token: String,

    /// Optional refresh token for automatic token rotation.
    pub refresh_token: Option<String>,

    /// The hub's base URL for all API calls.
    pub hub_base_url: String,
}

impl StoredSession {
    /// Creates a new session with the given credentials.
    pub fn new(
        access_token: impl Into<String>,
        refresh_token: Option<String>,
        hub_base_url: impl Into<String>,
    ) -> Self {
        StoredSession {
            access_token: access_token.into(),
            refresh_token,
            hub_base_url: hub_base_url.into(),
        }
    }
}

/// Persists and retrieves user sessions in the OS keychain (via penguin-secrets).
///
/// All operations are namespaced under `waddlebot-desktop` to prevent token
/// collision or leakage to other products.
pub struct TokenStore {
    store: penguin_secrets::Store,
}

impl TokenStore {
    /// Creates a new token store with the default OS keychain backend (Keychain on macOS,
    /// Credential Manager on Windows, Secret Service on Linux, with file fallback).
    pub fn new(data_dir: PathBuf) -> Result<Self> {
        let config = penguin_secrets::Config {
            service_name: "penguind".to_string(),
            backend: penguin_secrets::Backend::Auto {
                file_dir: data_dir.join("secrets"),
            },
        };

        let store = penguin_secrets::Store::open(config)
            .map_err(|e| DesktopError::KeychainError(e.to_string()))?
            .namespaced(NAMESPACE);

        Ok(TokenStore { store })
    }

    /// Creates a token store with the file-only backend (for tests).
    /// Tokens are stored in an encrypted file, never touching the OS keyring.
    pub fn new_file_only(file_dir: PathBuf) -> Result<Self> {
        let config = penguin_secrets::Config {
            service_name: "penguind".to_string(),
            backend: penguin_secrets::Backend::FileOnly { file_dir },
        };

        let store = penguin_secrets::Store::open(config)
            .map_err(|e| DesktopError::KeychainError(e.to_string()))?
            .namespaced(NAMESPACE);

        Ok(TokenStore { store })
    }

    /// Stores a session in the keychain.
    /// Tokens are never logged; only the operation is traced.
    pub async fn store(&self, session: &StoredSession) -> Result<()> {
        // Validate the hub URL format before storing.
        let _parsed = url::Url::parse(&session.hub_base_url)
            .map_err(|e| DesktopError::UrlError(e.to_string()))?;

        // Store each component separately so they can be updated independently
        // (e.g., refresh without re-storing the hub URL).
        self.store
            .set(KEY_ACCESS_TOKEN, session.access_token.as_bytes())
            .await
            .map_err(|e| {
                DesktopError::KeychainError(format!("failed to store access token: {}", e))
            })?;

        if let Some(refresh) = &session.refresh_token {
            self.store
                .set(KEY_REFRESH_TOKEN, refresh.as_bytes())
                .await
                .map_err(|e| {
                    DesktopError::KeychainError(format!("failed to store refresh token: {}", e))
                })?;
        } else {
            // Clear any previous refresh token if the new session has none.
            let _ = self.store.delete(KEY_REFRESH_TOKEN).await;
        }

        self.store
            .set(KEY_HUB_BASE_URL, session.hub_base_url.as_bytes())
            .await
            .map_err(|e| DesktopError::KeychainError(format!("failed to store hub URL: {}", e)))?;

        debug!("session stored in keychain");
        Ok(())
    }

    /// Retrieves a session from the keychain.
    /// Returns `DesktopError::NoSession` if the session is not stored or incomplete.
    pub async fn load(&self) -> Result<StoredSession> {
        let access_token_bytes = self.store.get(KEY_ACCESS_TOKEN).await.map_err(|e| {
            DesktopError::KeychainError(format!("failed to load access token: {}", e))
        })?;

        let access_token = String::from_utf8(access_token_bytes).map_err(|_| {
            DesktopError::KeychainError("invalid access token encoding".to_string())
        })?;

        if access_token.is_empty() {
            return Err(DesktopError::NoSession);
        }

        // Refresh token is optional - if it doesn't exist, treat it as None
        let refresh_token = match self.store.get(KEY_REFRESH_TOKEN).await {
            Ok(bytes) if !bytes.is_empty() => Some(String::from_utf8(bytes).map_err(|_| {
                DesktopError::KeychainError("invalid refresh token encoding".to_string())
            })?),
            Ok(_) => None,  // Empty bytes = no refresh token
            Err(_) => None, // Key not found = no refresh token
        };

        let hub_base_url_bytes =
            self.store.get(KEY_HUB_BASE_URL).await.map_err(|e| {
                DesktopError::KeychainError(format!("failed to load hub URL: {}", e))
            })?;

        let hub_base_url = String::from_utf8(hub_base_url_bytes)
            .map_err(|_| DesktopError::KeychainError("invalid hub URL encoding".to_string()))?;

        if hub_base_url.is_empty() {
            return Err(DesktopError::NoSession);
        }

        debug!("session loaded from keychain");
        Ok(StoredSession {
            access_token,
            refresh_token,
            hub_base_url,
        })
    }

    /// Clears the entire stored session from the keychain (logout).
    pub async fn clear(&self) -> Result<()> {
        let _ = self.store.delete(KEY_ACCESS_TOKEN).await;
        let _ = self.store.delete(KEY_REFRESH_TOKEN).await;
        let _ = self.store.delete(KEY_HUB_BASE_URL).await;

        debug!("session cleared from keychain");
        Ok(())
    }

    /// Checks whether a session exists in the keychain (without loading it).
    pub async fn has_session(&self) -> bool {
        match self.store.get(KEY_ACCESS_TOKEN).await {
            Ok(token_bytes) => !token_bytes.is_empty(),
            Err(_) => false,
        }
    }

    /// Updates only the access and refresh tokens (called after token rotation).
    /// The hub URL remains unchanged.
    pub async fn update_tokens(
        &self,
        access_token: String,
        refresh_token: Option<String>,
    ) -> Result<()> {
        self.store
            .set(KEY_ACCESS_TOKEN, access_token.as_bytes())
            .await
            .map_err(|e| {
                DesktopError::KeychainError(format!("failed to update access token: {}", e))
            })?;

        if let Some(refresh) = refresh_token {
            self.store
                .set(KEY_REFRESH_TOKEN, refresh.as_bytes())
                .await
                .map_err(|e| {
                    DesktopError::KeychainError(format!("failed to update refresh token: {}", e))
                })?;
        } else {
            let _ = self.store.delete(KEY_REFRESH_TOKEN).await;
        }

        debug!("tokens updated in keychain");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stored_session_new() {
        let session = StoredSession::new(
            "access123",
            Some("refresh456".to_string()),
            "https://hub.example.com",
        );
        assert_eq!(session.access_token, "access123");
        assert_eq!(session.refresh_token, Some("refresh456".to_string()));
        assert_eq!(session.hub_base_url, "https://hub.example.com");
    }

    #[tokio::test]
    async fn test_token_store_file_only() -> Result<()> {
        let test_dir = tempfile::tempdir()
            .map_err(|e| DesktopError::Internal(format!("failed to create test dir: {}", e)))?;

        let store = TokenStore::new_file_only(test_dir.path().to_path_buf())?;

        // Initially no session
        assert!(!store.has_session().await);
        let load_result = store.load().await;
        assert!(load_result.is_err());

        // Store a session
        let session = StoredSession::new(
            "access123",
            Some("refresh456".to_string()),
            "https://hub.example.com",
        );
        store.store(&session).await?;

        // Should now have a session
        assert!(store.has_session().await);

        // Load it back
        let loaded = store.load().await?;
        assert_eq!(loaded.access_token, "access123");
        assert_eq!(loaded.refresh_token, Some("refresh456".to_string()));
        assert_eq!(loaded.hub_base_url, "https://hub.example.com");

        // Clear it
        store.clear().await?;
        assert!(!store.has_session().await);

        Ok(())
    }

    #[tokio::test]
    async fn test_token_store_update_tokens() -> Result<()> {
        let test_dir = tempfile::tempdir()
            .map_err(|e| DesktopError::Internal(format!("failed to create test dir: {}", e)))?;

        let store = TokenStore::new_file_only(test_dir.path().to_path_buf())?;

        // Store an initial session
        let session = StoredSession::new(
            "access123",
            Some("refresh456".to_string()),
            "https://hub.example.com",
        );
        store.store(&session).await?;

        // Update just the tokens, keeping the URL
        store
            .update_tokens("new_access".to_string(), Some("new_refresh".to_string()))
            .await?;

        // Load and verify
        let loaded = store.load().await?;
        assert_eq!(loaded.access_token, "new_access");
        assert_eq!(loaded.refresh_token, Some("new_refresh".to_string()));
        assert_eq!(loaded.hub_base_url, "https://hub.example.com");

        Ok(())
    }

    #[tokio::test]
    async fn test_token_store_invalid_url() -> Result<()> {
        let test_dir = tempfile::tempdir()
            .map_err(|e| DesktopError::Internal(format!("failed to create test dir: {}", e)))?;

        let store = TokenStore::new_file_only(test_dir.path().to_path_buf())?;

        let session = StoredSession::new("access123", None, "not a url");
        let result = store.store(&session).await;
        assert!(result.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_token_store_no_refresh_token() -> Result<()> {
        let test_dir = tempfile::tempdir()
            .map_err(|e| DesktopError::Internal(format!("failed to create test dir: {}", e)))?;

        let store = TokenStore::new_file_only(test_dir.path().to_path_buf())?;

        // Store session without refresh token
        let session = StoredSession::new("access123", None, "https://hub.example.com");
        store.store(&session).await?;

        // Load it back
        let loaded = store.load().await?;
        assert_eq!(loaded.access_token, "access123");
        assert_eq!(loaded.refresh_token, None);
        assert_eq!(loaded.hub_base_url, "https://hub.example.com");

        Ok(())
    }

    #[tokio::test]
    async fn test_token_store_update_tokens_no_refresh() -> Result<()> {
        let test_dir = tempfile::tempdir()
            .map_err(|e| DesktopError::Internal(format!("failed to create test dir: {}", e)))?;

        let store = TokenStore::new_file_only(test_dir.path().to_path_buf())?;

        // Store an initial session with refresh token
        let session = StoredSession::new(
            "access123",
            Some("refresh456".to_string()),
            "https://hub.example.com",
        );
        store.store(&session).await?;

        // Update tokens without refresh token
        store.update_tokens("new_access".to_string(), None).await?;

        // Load and verify refresh token is gone
        let loaded = store.load().await?;
        assert_eq!(loaded.access_token, "new_access");
        assert_eq!(loaded.refresh_token, None);
        assert_eq!(loaded.hub_base_url, "https://hub.example.com");

        Ok(())
    }

    #[tokio::test]
    async fn test_stored_session_without_refresh() {
        let session = StoredSession::new("access123", None, "https://hub.example.com");
        assert_eq!(session.access_token, "access123");
        assert_eq!(session.refresh_token, None);
        assert_eq!(session.hub_base_url, "https://hub.example.com");
    }
}
