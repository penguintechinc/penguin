//! The platform keyring backend: OS-provided credential storage (Windows
//! Credential Manager, macOS Keychain, or Linux Secret Service) via the
//! `keyring` crate.
//!
//! Every operation is async because the underlying `keyring` calls are
//! blocking — they can do real IPC (D-Bus on Linux, Keychain Services on
//! macOS) — so each one runs on tokio's blocking pool rather than stalling
//! the caller's executor, matching the contract documented on
//! [`penguin_sdk::SecretStore`].
//!
//! **Never exercised by this crate's tests.** [`crate::Backend::FileOnly`]
//! is the only selection any test in this crate uses; this module only runs
//! in production, or when a caller explicitly opts into
//! [`crate::Backend::Auto`] on a machine that actually has a platform
//! credential store.

use keyring::Entry;

use penguin_sdk::SecretError;

/// A fixed, near-certainly-absent key used purely to probe whether the
/// platform keyring backend is reachable at all (see [`PlatformBackend::probe`]).
const PROBE_KEY: &str = "__penguin_secrets_backend_probe__";

/// A handle to the OS-provided keyring, scoped to one service name.
pub struct PlatformBackend {
    service_name: String,
}

impl PlatformBackend {
    /// Wraps `service_name` for later per-key [`Entry`] construction.
    /// Constructing this does not itself touch the OS keyring — that only
    /// happens on the first get/set/delete call.
    pub fn new(service_name: &str) -> PlatformBackend {
        PlatformBackend {
            service_name: service_name.to_string(),
        }
    }

    /// Reports whether the platform keyring backend appears reachable, by
    /// looking up a key that near-certainly does not exist.
    /// [`keyring::Error::NoEntry`] means the backend answered — reachable.
    /// Any other error (no D-Bus session, a keychain the user never
    /// unlocked, an unsupported target) means it is not, and the caller
    /// should fall back to the file backend instead.
    pub fn probe(service_name: &str) -> bool {
        let entry = match Entry::new(service_name, PROBE_KEY) {
            Ok(entry) => entry,
            Err(_) => return false,
        };
        matches!(entry.get_secret(), Err(keyring::Error::NoEntry))
    }

    /// Builds the per-key [`Entry`] for this service.
    fn entry(&self, key: &str) -> Result<Entry, SecretError> {
        Entry::new(&self.service_name, key).map_err(|err| {
            SecretError::Other(format!("failed to open platform keyring entry: {err}"))
        })
    }

    /// Fetches a secret from the platform keyring.
    pub async fn get(&self, key: &str) -> Result<Vec<u8>, SecretError> {
        let entry = self.entry(key)?;
        let owned_key = key.to_string();
        let outcome = tokio::task::spawn_blocking(move || match entry.get_secret() {
            Ok(secret) => Ok(secret),
            Err(keyring::Error::NoEntry) => Err(SecretError::NotFound),
            Err(err) => Err(SecretError::Other(format!(
                "failed to get secret {owned_key:?}: {err}"
            ))),
        })
        .await;
        join_blocking(outcome)
    }

    /// Stores (or replaces) a secret in the platform keyring.
    pub async fn set(&self, key: &str, value: &[u8]) -> Result<(), SecretError> {
        let entry = self.entry(key)?;
        let owned_value = value.to_vec();
        let owned_key = key.to_string();
        let outcome = tokio::task::spawn_blocking(move || {
            entry.set_secret(&owned_value).map_err(|err| {
                SecretError::Other(format!("failed to set secret {owned_key:?}: {err}"))
            })
        })
        .await;
        join_blocking(outcome)
    }

    /// Deletes a secret from the platform keyring.
    pub async fn delete(&self, key: &str) -> Result<(), SecretError> {
        let entry = self.entry(key)?;
        let owned_key = key.to_string();
        let outcome = tokio::task::spawn_blocking(move || match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Err(SecretError::NotFound),
            Err(err) => Err(SecretError::Other(format!(
                "failed to delete secret {owned_key:?}: {err}"
            ))),
        })
        .await;
        join_blocking(outcome)
    }
}

/// Collapses a `spawn_blocking` join result into the same [`SecretError`]
/// surface as every other backend, treating a panicked blocking task as an
/// opaque backend failure rather than propagating a panic.
fn join_blocking<T>(
    outcome: Result<Result<T, SecretError>, tokio::task::JoinError>,
) -> Result<T, SecretError> {
    let Ok(result) = outcome else {
        return Err(SecretError::Other(
            "platform keyring task panicked".to_string(),
        ));
    };
    result
}
