//! Minimal M2 stand-ins for the secret store and license checker, replaced
//! by the real `penguin-secrets` / `penguin-licensing` crates in M4. No
//! built-in module reads secrets or checks license flags before M6, so
//! these only need to satisfy the [`HostServices`](penguin_sdk::HostServices)
//! contract, not do anything durable yet.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use penguin_sdk::{LicenseChecker, SecretError, SecretStore};

/// An in-memory, non-persistent secret store.
///
/// M4: replace with `penguin-secrets` (OS keyring / keystore backends with
/// an encrypted-file fallback for headless daemons). Secrets stored here
/// vanish on daemon restart — acceptable only because nothing reads or
/// writes through this path before M6.
#[derive(Default)]
pub struct InMemorySecretStore {
    data: Mutex<HashMap<String, Vec<u8>>>,
}

impl InMemorySecretStore {
    /// Builds an empty store.
    pub fn new() -> InMemorySecretStore {
        InMemorySecretStore::default()
    }
}

#[async_trait]
impl SecretStore for InMemorySecretStore {
    async fn get(&self, key: &str) -> Result<Vec<u8>, SecretError> {
        let data = self.data.lock().expect("secret store mutex poisoned");
        data.get(key).cloned().ok_or(SecretError::NotFound)
    }

    async fn set(&self, key: &str, value: &[u8]) -> Result<(), SecretError> {
        let mut data = self.data.lock().expect("secret store mutex poisoned");
        data.insert(key.to_string(), value.to_vec());
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), SecretError> {
        let mut data = self.data.lock().expect("secret store mutex poisoned");
        data.remove(key);
        Ok(())
    }
}

/// Reports every feature disabled at the free tier.
///
/// M4: replace with `penguin-licensing` (a `license.penguintech.io` client
/// with an offline entitlement cache). Until then, "everything off, tier
/// free" is exactly the correct graceful-degradation default for a license
/// server this binary never contacts — see `general.md` Feature Toggling.
pub struct FreeTierLicenseChecker;

impl LicenseChecker for FreeTierLicenseChecker {
    fn feature_enabled(&self, _key: &str) -> bool {
        false
    }

    fn tier(&self) -> String {
        "free".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_secret_store_round_trips_and_reports_not_found() {
        let store = InMemorySecretStore::new();
        assert_eq!(store.get("k").await, Err(SecretError::NotFound));

        store.set("k", b"v").await.unwrap();
        assert_eq!(store.get("k").await.unwrap(), b"v");

        store.delete("k").await.unwrap();
        assert_eq!(store.get("k").await, Err(SecretError::NotFound));
    }

    #[tokio::test]
    async fn deleting_a_missing_key_is_not_an_error() {
        let store = InMemorySecretStore::new();
        assert!(store.delete("ghost").await.is_ok());
    }

    #[test]
    fn free_tier_license_checker_disables_everything() {
        let checker = FreeTierLicenseChecker;
        assert!(!checker.feature_enabled("penguin.squawk"));
        assert_eq!(checker.tier(), "free");
    }
}
