//! Implements [`penguin_daemon::host::SecretStoreProvider`] over the real
//! [`penguin_secrets::Store`], giving [`penguin_daemon::host::DaemonHostFactory`]
//! genuine per-module secret isolation.
//!
//! # Why this lives here and not in `penguin-daemon`
//!
//! `penguin-daemon` deliberately depends on no concrete secrets backend —
//! see `DaemonHostFactory`'s doc in that crate's `host.rs`. `SecretStoreProvider`
//! is the seam: the trait is defined there purely in terms of
//! `penguin_sdk::SecretStore`, and this binary supplies the one production
//! implementation, backed by the real encrypted-file/keychain store built in
//! `daemon_main.rs`. [`SecretsStoreProvider::store_for`] calls
//! `penguin_secrets::Store::namespaced(module)` per module — the Rust
//! equivalent of the Go reference's downcast-and-`Namespaced(moduleName)`
//! call in `go-client/internal/daemon/host.go`.
//!
//! There used to be a second layer here — a `NamespacingHostFactory`
//! decorator that wrapped a `DaemonHostFactory` already holding an
//! unnamespaced store and re-namespaced its output after the fact. That
//! existed only because `DaemonHostFactory::host_for` used to hand every
//! module the exact same `Arc<dyn SecretStore>` unmodified — a real
//! isolation gap, fixed now that `host_for` itself calls
//! [`SecretStoreProvider::store_for`]. Keeping both layers would have
//! double-prefixed every key (`"<module>/<module>/<key>"`); this module
//! provides the isolation exactly once.

use std::sync::Arc;

use penguin_daemon::host::SecretStoreProvider;
use penguin_sdk::SecretStore;
use penguin_secrets::Store as SecretsStore;

/// Gives every module a [`penguin_secrets::Store::namespaced`] view rooted
/// at one shared store.
pub struct SecretsStoreProvider {
    root: Arc<SecretsStore>,
}

impl SecretsStoreProvider {
    /// Wraps `root`; every [`store_for`](SecretStoreProvider::store_for)
    /// call returns a fresh namespaced view over the same backend.
    pub fn new(root: Arc<SecretsStore>) -> SecretsStoreProvider {
        SecretsStoreProvider { root }
    }
}

impl SecretStoreProvider for SecretsStoreProvider {
    fn store_for(&self, module: &str) -> Arc<dyn SecretStore> {
        Arc::new(self.root.namespaced(module))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use penguin_daemon::broker::EventBroker;
    use penguin_daemon::config::ConfigStore;
    use penguin_daemon::host::{DaemonHostFactory, HostFactory};
    use penguin_sdk::{EventSink, LicenseChecker, SecretError};
    use penguin_secrets::{Backend, Config};
    use penguin_telemetry::Telemetry;
    use tempfile::TempDir;

    /// A [`LicenseChecker`] double; these tests only exercise secrets
    /// isolation, so its answers are never asserted on.
    struct FakeLicenseChecker;

    impl LicenseChecker for FakeLicenseChecker {
        fn feature_enabled(&self, _key: &str) -> bool {
            false
        }
        fn tier(&self) -> String {
            "free".to_string()
        }
    }

    /// Builds the exact chain `daemon_main.rs` wires in production: a real
    /// file-backed [`SecretsStore`] and a real `DaemonHostFactory` driven by
    /// a [`SecretsStoreProvider`] over it. Not a hand-rolled test double
    /// standing in for the real wiring — this is the real wiring. Returns
    /// the root store too, so `module_keys_are_namespaced_exactly_once_in_the_underlying_store`
    /// can read the backend directly, bypassing any namespacing.
    fn real_factory() -> (
        TempDir,
        TempDir,
        TempDir,
        DaemonHostFactory,
        Arc<SecretsStore>,
    ) {
        let secrets_dir = TempDir::new().expect("secrets tempdir");
        let config_dir = TempDir::new().expect("config tempdir");
        let state_dir = TempDir::new().expect("state tempdir");

        let secrets_root = Arc::new(
            SecretsStore::open(Config {
                service_name: String::new(),
                backend: Backend::FileOnly {
                    file_dir: secrets_dir.path().to_path_buf(),
                },
            })
            .expect("open file-backed secret store"),
        );

        let telemetry = Arc::new(Telemetry::new("error").expect("telemetry"));
        let config_store = Arc::new(ConfigStore::new(config_dir.path()));
        let broker: Arc<dyn EventSink> = Arc::new(EventBroker::new(4));
        let provider: Arc<dyn SecretStoreProvider> =
            Arc::new(SecretsStoreProvider::new(secrets_root.clone()));

        let factory = DaemonHostFactory::new(
            telemetry,
            config_store,
            provider,
            Arc::new(FakeLicenseChecker),
            broker,
            state_dir.path().to_path_buf(),
            None,
        );

        (secrets_dir, config_dir, state_dir, factory, secrets_root)
    }

    #[tokio::test]
    async fn a_module_receives_a_working_secret_store_through_host_services() {
        let (_secrets_dir, _config_dir, _state_dir, factory, _root) = real_factory();
        let host = factory.host_for("squawk", None);

        // The exact call shape a module's `init(host: Arc<dyn HostServices>)`
        // makes: fetch the secrets handle off the host, then use it.
        let secrets = host.secrets();
        secrets.set("api_key", b"squawk-secret").await.expect("set");
        let got = secrets.get("api_key").await.expect("get");
        assert_eq!(got, b"squawk-secret");
    }

    #[tokio::test]
    async fn two_modules_have_isolated_secret_namespaces() {
        let (_secrets_dir, _config_dir, _state_dir, factory, _root) = real_factory();
        let host_a = factory.host_for("module-a", None);
        let host_b = factory.host_for("module-b", None);

        host_a
            .secrets()
            .set("api_key", b"secret-a")
            .await
            .expect("a set");
        host_b
            .secrets()
            .set("api_key", b"secret-b")
            .await
            .expect("b set");

        assert_eq!(
            host_a.secrets().get("api_key").await.expect("a get"),
            b"secret-a"
        );
        assert_eq!(
            host_b.secrets().get("api_key").await.expect("b get"),
            b"secret-b"
        );

        // Module A cannot make module B's copy of the same key disappear —
        // the two hosts must not merely read different values, they must be
        // fully independent stores.
        host_a.secrets().delete("api_key").await.expect("a delete");
        let after_delete = host_a.secrets().get("api_key").await.unwrap_err();
        assert!(matches!(after_delete, SecretError::NotFound));
        assert_eq!(
            host_b
                .secrets()
                .get("api_key")
                .await
                .expect("b still present"),
            b"secret-b"
        );
    }

    #[tokio::test]
    async fn module_keys_are_namespaced_exactly_once_in_the_underlying_store() {
        // Proves DaemonHostFactory + SecretsStoreProvider add the module
        // prefix exactly once. A leftover second namespacing layer (the old
        // NamespacingHostFactory decorator, stacked on a DaemonHostFactory
        // that also namespaced) would store this under
        // "squawk/squawk/api_key" instead — this test reads the root store
        // directly, bypassing any namespacing, so it only finds the value at
        // the single-prefixed key if there is exactly one layer.
        let (_secrets_dir, _config_dir, _state_dir, factory, root) = real_factory();
        let host = factory.host_for("squawk", None);

        host.secrets()
            .set("api_key", b"squawk-secret")
            .await
            .expect("set via host");

        let direct = root
            .get("squawk/api_key")
            .await
            .expect("direct read at the single-prefixed key");
        assert_eq!(direct, b"squawk-secret");

        let double_prefixed = root.get("squawk/squawk/api_key").await.unwrap_err();
        assert!(matches!(double_prefixed, SecretError::NotFound));
    }
}
