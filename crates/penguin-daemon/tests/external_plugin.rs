//! Integration test: launch a REAL Go plugin binary through the full
//! external-plugin path — manifest load, [`Verifier`] signature
//! verification, process launch via `penguin-goplugin-host`, and dispatch —
//! driven end to end through [`Supervisor`], not just
//! `penguin-goplugin-host` directly (see that crate's own
//! `goplugin_compat.rs` for the lower-level compat proof this builds on).
//!
//! # Running
//!
//! ```sh
//! go build -o go-client/bin/plugin-hello ./go-client/examples/plugin-hello
//! PENGUIN_INTEGRATION=1 cargo test -p penguin-daemon --test external_plugin -- --ignored --nocapture
//! ```
//!
//! Every test here is `#[ignore]` *and* separately checks
//! `PENGUIN_INTEGRATION=1` at runtime, so neither a plain `cargo test` nor a
//! bare `cargo test -- --ignored` ever spawns a process — same convention as
//! `penguin-goplugin-host/tests/goplugin_compat.rs`. A missing plugin binary
//! skips (with a message) rather than failing the run.
//!
//! # Signing the fixture
//!
//! `penguin-extplugin`'s own `minisign-verify` dependency is verify-only by
//! design (production code never needs a private key), so it cannot produce
//! a signature over this test's binary, which is a real, non-reproducible
//! Go build. The `minisign` crate (same author, same wire format, pinned as
//! a dev-dependency only) generates a throwaway keypair and signs the
//! binary; [`PluginDirLoader::with_keys`] then trusts exactly that keypair
//! for the test, rather than weakening the production `Verifier::new()`
//! path this test does not use.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tempfile::TempDir;

use penguin_daemon::broker::EventBroker;
use penguin_daemon::config::ConfigStore;
use penguin_daemon::external::{ExternalLoader, PluginDirLoader};
use penguin_daemon::host::{DaemonHostFactory, HostFactory, SecretStoreProvider};
use penguin_daemon::supervisor::{Supervisor, SupervisorConfig, SupervisorError};
use penguin_sdk::{EventSink, LicenseChecker, ModuleState, SecretError, SecretStore};

/// The RPC-visible name of the sole command `plugin-hello` declares.
const GREET_COMMAND: &str = "greet";

/// Skips the calling test (with a message) unless the integration tier is
/// explicitly opted into. Kept separate from `#[ignore]`: a bare
/// `--ignored` run must still not spawn a real process.
macro_rules! require_integration {
    () => {
        if std::env::var("PENGUIN_INTEGRATION").as_deref() != Ok("1") {
            eprintln!("SKIP: set PENGUIN_INTEGRATION=1 to run external_plugin tests");
            return;
        }
    };
}

/// Locates the `plugin-hello` binary, preferring `PENGUIN_PLUGIN_HELLO` over
/// the default build output path. Returns `None` (after printing why) when
/// neither exists, so callers skip rather than fail — mirrors
/// `penguin-goplugin-host/tests/goplugin_compat.rs`'s identical helper.
fn plugin_hello_path() -> Option<PathBuf> {
    if let Ok(overridden) = std::env::var("PENGUIN_PLUGIN_HELLO") {
        let path = PathBuf::from(overridden);
        if path.is_file() {
            return Some(path);
        }
        eprintln!(
            "SKIP: PENGUIN_PLUGIN_HELLO={} does not exist",
            path.display()
        );
        return None;
    }

    let default =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../go-client/bin/plugin-hello");
    if default.is_file() {
        return Some(default);
    }
    eprintln!(
        "SKIP: no plugin-hello binary at {} — build it with `go build -o go-client/bin/plugin-hello ./go-client/examples/plugin-hello`, or set PENGUIN_PLUGIN_HELLO",
        default.display()
    );
    None
}

/// A laid-out `<plugins_dir>/hello/` fixture: a real copy of the Go-built
/// `plugin-hello` binary, a fresh minisign keypair signing it, and a
/// manifest. The temp dir guards are kept alive by being fields (never
/// bound with a leading underscore alone) so the directory tree they own
/// outlives every test's use of `plugins_dir`.
struct PluginFixture {
    _plugins_dir_guard: TempDir,
    plugins_dir: PathBuf,
    binary_path: PathBuf,
    trusted_public_key: String,
}

impl PluginFixture {
    /// The individual plugin's own directory (`<plugins_dir>/hello`) — the
    /// path [`penguin_extplugin::Verifier`] actually stats for ownership,
    /// as opposed to [`PluginFixture::plugins_dir`], which is the root
    /// [`PluginDirLoader`] scans.
    fn plugin_dir(&self) -> &Path {
        self.binary_path
            .parent()
            .expect("binary_path always has a parent directory")
    }
}

/// Lays out the fixture. `sha256_override`, when given, replaces the
/// manifest's correctly-computed hash — used by the negative test to
/// produce a plugin directory that fails verification without needing a
/// second, deliberately-corrupted binary.
fn write_plugin_fixture(source_binary: &Path, sha256_override: Option<&str>) -> PluginFixture {
    let plugins_dir_guard = TempDir::new().expect("tempdir for plugins_dir");
    let plugins_dir = plugins_dir_guard.path().to_path_buf();
    let plugin_dir = plugins_dir.join("hello");
    std::fs::create_dir(&plugin_dir).expect("mkdir plugin dir");

    let binary_bytes = std::fs::read(source_binary).expect("read built plugin-hello");
    let binary_path = plugin_dir.join("hello-bin");
    std::fs::write(&binary_path, &binary_bytes).expect("write plugin binary");
    // `fs::write` creates the file mode 0644 (no execute bit) — go-plugin
    // has to actually exec this. 0755 (rwxr-xr-x) is also explicitly
    // not-world-writable, satisfying `Verifier`'s ownership check.
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755))
        .expect("make plugin binary executable");

    let keypair =
        minisign::KeyPair::generate_unencrypted_keypair().expect("generate minisign keypair");
    let signature_box = minisign::sign(
        Some(&keypair.pk),
        &keypair.sk,
        Cursor::new(binary_bytes.as_slice()),
        Some("external_plugin.rs integration fixture"),
        None,
    )
    .expect("sign plugin binary");
    std::fs::write(
        plugin_dir.join("hello-bin.minisig"),
        signature_box.to_string(),
    )
    .expect("write signature");

    let sha256 = sha256_override
        .map(str::to_string)
        .unwrap_or_else(|| hex_sha256(&binary_bytes));
    let manifest = format!(
        r#"{{"name":"hello","version":"1.0.0","sdk_version":"v1","binary":"hello-bin","sha256":"{sha256}","publisher":"integration-test"}}"#
    );
    std::fs::write(plugin_dir.join("plugin.json"), manifest).expect("write manifest");

    let trusted_public_key = keypair.pk.to_box().expect("public key box").into_string();

    PluginFixture {
        _plugins_dir_guard: plugins_dir_guard,
        plugins_dir,
        binary_path,
        trusted_public_key,
    }
}

/// Lowercase hex SHA256, matching `penguin_extplugin::verify`'s own
/// encoding — no `hex` crate dependency needed for one digest.
fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The uid `plugin_dir` (and everything under it) is owned by — the daemon
/// uid `Verifier`'s ownership check expects, since this fixture is created
/// by (and so owned by) the test process itself.
#[cfg(unix)]
fn owner_uid_of(path: &Path) -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).expect("stat for uid").uid()
}

/// True if any process on this host has `binary_path` as `argv[0]` — proves
/// a plugin's child process is gone after unload/shutdown without needing
/// the daemon to expose OS pids through the type-erased `Module` trait
/// (`ExternalModule` deliberately keeps its `PluginProcess` private). Valid
/// on the Linux containers every build/test in this repo runs in, same
/// approach as `goplugin_compat.rs`'s own `pid_exists`.
fn any_process_running(binary_path: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    for entry in entries.flatten() {
        let is_pid = entry
            .file_name()
            .to_string_lossy()
            .chars()
            .all(|c| c.is_ascii_digit());
        if !is_pid {
            continue;
        }
        let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let argv0 = cmdline.split(|&b| b == 0).next().unwrap_or(&[]);
        let Ok(argv0) = std::str::from_utf8(argv0) else {
            continue;
        };
        if Path::new(argv0) == binary_path {
            return true;
        }
    }
    false
}

/// A [`SecretStore`] double; these tests never exercise it. Also implements
/// [`SecretStoreProvider`], handing every module the same no-op instance —
/// real per-module isolation is covered by `penguin-daemon`'s own `host.rs`
/// tests and `bins/penguind`'s `host_wiring.rs`, not this integration test.
struct FakeSecretStore;
#[async_trait]
impl SecretStore for FakeSecretStore {
    async fn get(&self, _key: &str) -> Result<Vec<u8>, SecretError> {
        Err(SecretError::NotFound)
    }
    async fn set(&self, _key: &str, _value: &[u8]) -> Result<(), SecretError> {
        Ok(())
    }
    async fn delete(&self, _key: &str) -> Result<(), SecretError> {
        Ok(())
    }
}
impl SecretStoreProvider for FakeSecretStore {
    fn store_for(&self, _module: &str) -> Arc<dyn SecretStore> {
        Arc::new(FakeSecretStore)
    }
}

/// A [`LicenseChecker`] double; everything is enabled.
struct FakeLicenseChecker;
impl LicenseChecker for FakeLicenseChecker {
    fn feature_enabled(&self, _key: &str) -> bool {
        true
    }
    fn tier(&self) -> String {
        "free".to_string()
    }
}

/// Builds a [`Supervisor`] with no builtin registry and `loader` as its
/// only way to resolve a name, mirroring `penguin-daemon`'s own internal
/// `build_supervisor` test helpers (duplicated here rather than exported,
/// since a `#[cfg(test)]` module is not visible across the crate boundary
/// to an integration test binary).
fn build_supervisor(loader: Arc<dyn ExternalLoader>) -> (Supervisor, TempDir, TempDir) {
    let state_dir = TempDir::new().expect("tempdir for state_dir");
    let config_dir = TempDir::new().expect("tempdir for config_dir");
    let telemetry = Arc::new(penguin_telemetry::Telemetry::new("error").expect("telemetry"));
    let config_store = Arc::new(ConfigStore::new(config_dir.path()));
    let broker = Arc::new(EventBroker::new(16));
    let events: Arc<dyn EventSink> = broker.clone();
    let host_factory: Arc<dyn HostFactory> = Arc::new(DaemonHostFactory::new(
        telemetry,
        config_store,
        Arc::new(FakeSecretStore),
        Arc::new(FakeLicenseChecker),
        events,
        state_dir.path().to_path_buf(),
    ));
    let supervisor = Supervisor::new(SupervisorConfig {
        registry: std::collections::BTreeMap::new(),
        host_factory,
        broker,
        state_dir: state_dir.path().to_path_buf(),
        max_restarts: 5,
        health_interval: Duration::from_secs(3600),
        stability_window: Duration::from_secs(3600),
        external: Some(loader),
    });
    (supervisor, state_dir, config_dir)
}

/// Headline assertion: a real, minisign-verified Go plugin loads through
/// the supervisor and `Dispatch(["greet"], {}, ["world"])` round-trips
/// through the whole stack — manifest, verification, AutoMTLS process
/// launch, and the wire `ModuleService.Dispatch` RPC — to `"hello, world"`.
#[tokio::test]
#[ignore]
async fn verified_external_plugin_loads_and_dispatches_through_the_supervisor() {
    require_integration!();
    let Some(binary) = plugin_hello_path() else {
        return;
    };

    let fixture = write_plugin_fixture(&binary, None);
    let daemon_uid = owner_uid_of(fixture.plugin_dir());
    let socket_dir = TempDir::new().expect("tempdir for socket_dir");
    let loader: Arc<dyn ExternalLoader> = Arc::new(PluginDirLoader::with_keys(
        fixture.plugins_dir.clone(),
        socket_dir.path().to_path_buf(),
        daemon_uid,
        vec![fixture.trusted_public_key.clone()],
    ));
    let (supervisor, _state_dir, _config_dir) = build_supervisor(loader);

    let state = supervisor
        .load("hello")
        .await
        .expect("verified external plugin must load");
    assert_eq!(state, ModuleState::Running);
    assert!(
        any_process_running(&fixture.binary_path),
        "a real child process must be running once the plugin is loaded"
    );

    let result = supervisor
        .dispatch(
            "hello",
            &[GREET_COMMAND.to_string()],
            &HashMap::new(),
            &["world".to_string()],
        )
        .await
        .expect("dispatch must succeed at both the transport and module level");
    assert_eq!(result.output, "hello, world");
    assert_eq!(result.exit_code, 0);

    supervisor.unload("hello").await.expect("unload");
}

/// Assertion: `unload` terminates the child and no orphan process survives.
#[tokio::test]
#[ignore]
async fn unload_terminates_the_child_process_with_no_orphan() {
    require_integration!();
    let Some(binary) = plugin_hello_path() else {
        return;
    };

    let fixture = write_plugin_fixture(&binary, None);
    let daemon_uid = owner_uid_of(fixture.plugin_dir());
    let socket_dir = TempDir::new().expect("tempdir for socket_dir");
    let loader: Arc<dyn ExternalLoader> = Arc::new(PluginDirLoader::with_keys(
        fixture.plugins_dir.clone(),
        socket_dir.path().to_path_buf(),
        daemon_uid,
        vec![fixture.trusted_public_key.clone()],
    ));
    let (supervisor, _state_dir, _config_dir) = build_supervisor(loader);

    supervisor.load("hello").await.expect("load");
    assert!(any_process_running(&fixture.binary_path));

    supervisor.unload("hello").await.expect("unload");
    assert!(
        !any_process_running(&fixture.binary_path),
        "plugin-hello's process must not survive unload()"
    );
}

/// Assertion: a daemon-wide `shutdown` also terminates the child — the
/// other teardown path besides `unload`, exercised separately since
/// `Supervisor::shutdown` never touches the persisted enabled-set and takes
/// a different internal path (`Supervisor::shutdown` -> `stop_locked` for
/// every loaded module) than `unload` does.
#[tokio::test]
#[ignore]
async fn daemon_shutdown_terminates_the_child_process_with_no_orphan() {
    require_integration!();
    let Some(binary) = plugin_hello_path() else {
        return;
    };

    let fixture = write_plugin_fixture(&binary, None);
    let daemon_uid = owner_uid_of(fixture.plugin_dir());
    let socket_dir = TempDir::new().expect("tempdir for socket_dir");
    let loader: Arc<dyn ExternalLoader> = Arc::new(PluginDirLoader::with_keys(
        fixture.plugins_dir.clone(),
        socket_dir.path().to_path_buf(),
        daemon_uid,
        vec![fixture.trusted_public_key.clone()],
    ));
    let (supervisor, _state_dir, _config_dir) = build_supervisor(loader);

    supervisor.load("hello").await.expect("load");
    assert!(any_process_running(&fixture.binary_path));

    supervisor.shutdown().await;
    assert!(
        !any_process_running(&fixture.binary_path),
        "plugin-hello's process must not survive daemon shutdown()"
    );
}

/// Negative assertion: a plugin whose manifest sha256 does not match its
/// actual binary is refused — verification fails before the process is
/// ever launched, so no child process exists to leak in the first place.
#[tokio::test]
#[ignore]
async fn plugin_with_mismatched_sha256_is_refused() {
    require_integration!();
    let Some(binary) = plugin_hello_path() else {
        return;
    };

    let bogus_sha256 = "0".repeat(64);
    let fixture = write_plugin_fixture(&binary, Some(&bogus_sha256));
    let daemon_uid = owner_uid_of(fixture.plugin_dir());
    let socket_dir = TempDir::new().expect("tempdir for socket_dir");
    let loader: Arc<dyn ExternalLoader> = Arc::new(PluginDirLoader::with_keys(
        fixture.plugins_dir.clone(),
        socket_dir.path().to_path_buf(),
        daemon_uid,
        vec![fixture.trusted_public_key.clone()],
    ));
    let (supervisor, _state_dir, _config_dir) = build_supervisor(loader);

    let err = supervisor
        .load("hello")
        .await
        .expect_err("a sha256 mismatch must be refused, never loaded unverified");
    match err {
        SupervisorError::ExternalLoad(message) => {
            assert!(
                message.contains("sha256 mismatch"),
                "unexpected error message: {message}"
            );
        }
        other => panic!("expected SupervisorError::ExternalLoad, got {other:?}"),
    }
    assert!(
        !any_process_running(&fixture.binary_path),
        "a refused plugin must never have been launched at all"
    );
}
