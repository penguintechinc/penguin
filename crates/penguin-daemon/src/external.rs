//! Resolves a module name the builtin registry does not recognise into a
//! live external plugin — the piece that lets `penguin load <name>` reach a
//! plugin that ships as its own signed binary instead of one compiled into
//! `penguind`.
//!
//! Two halves:
//!
//! * [`ExternalLoader`] / [`ExternalLoadError`] — the interface
//!   [`crate::supervisor::Supervisor`] drives, kept separate from any
//!   concrete implementation so the supervisor's tests can fake it (see
//!   `supervisor.rs`'s own test module) without spawning a process.
//! * [`PluginDirLoader`] / [`ExternalModule`] — the real implementation:
//!   `<plugins_dir>/<name>/plugin.json` → [`penguin_extplugin::Verifier`]'s
//!   fail-closed pipeline (ownership, world-writable, SHA256, minisign — no
//!   TOFU) → [`penguin_goplugin_host::client::PluginProcess::launch`] →
//!   `dispense()`. [`ExternalModule`] then owns that process for the
//!   module's whole lifetime and kills it in `stop()`, so the supervisor's
//!   existing stop/restart/shutdown paths — entirely unmodified — are what
//!   tear a plugin process down.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::Mutex as AsyncMutex;

use penguin_extplugin::{Verifier, load_manifest};
use penguin_goplugin_host::client::PluginProcess;
use penguin_sdk::{
    CommandResult, CommandSpec, HealthReport, HostServices, Module, ModuleError, ModuleInfo, Status,
};

/// Resolves a module name the builtin registry does not recognise into a
/// freshly-constructed, un-initialised [`Module`] — the external-plugin
/// analogue of a builtin [`penguin_sdk::Factory`]. An implementation owns
/// whatever it takes to produce that instance (plugin-directory resolution,
/// manifest parsing, signature verification, process launch); the
/// supervisor only ever sees this trait, so builtin and external modules
/// are instantiated through one uniform path.
#[async_trait]
pub trait ExternalLoader: Send + Sync {
    /// Resolves and constructs `name`. Called at most once per load or
    /// restart attempt, and only for a name the builtin registry did not
    /// already serve.
    async fn load(&self, name: &str) -> Result<Box<dyn Module>, ExternalLoadError>;
}

/// Every way [`ExternalLoader::load`] can fail, kept distinct so
/// [`crate::supervisor::Supervisor`] can map "this name doesn't exist
/// anywhere" identically for builtin and external names, while still
/// surfacing a real load failure (bad signature, launch failure, ...) as its
/// own error instead of a misleading "unknown module".
#[derive(Debug, Error)]
pub enum ExternalLoadError {
    /// No plugin exists under this name at all (e.g. its directory is
    /// absent). The supervisor folds this into the same
    /// `SupervisorError::UnknownModule` a builtin registry miss produces,
    /// so callers see one "no such module" shape regardless of where the
    /// name would have come from.
    #[error("no external plugin named {0:?}")]
    NotFound(String),
    /// The plugin exists but failed to load — a manifest, ownership,
    /// signature, or process-launch failure. A real, retryable error for a
    /// module that does exist, never conflated with [`ExternalLoadError::NotFound`].
    #[error("{0}")]
    Load(String),
}

/// A loaded external plugin: owns the child process for the module's whole
/// lifetime and tears it down in [`Module::stop`].
///
/// `process` is `None` once torn down — that is what makes a second `stop()`
/// call the no-op [`Module::stop`]'s contract requires ("must be
/// idempotent"), and it is also why ownership has to live behind a lock at
/// all: [`PluginProcess::shutdown`] consumes `self`, but `Module::stop`
/// only ever gets `&self`, so the process has to be `.take()`-able out of
/// shared storage rather than moved out directly.
struct ExternalModule {
    process: AsyncMutex<Option<PluginProcess>>,
    inner: Box<dyn Module>,
}

impl ExternalModule {
    fn new(process: PluginProcess, inner: Box<dyn Module>) -> ExternalModule {
        ExternalModule {
            process: AsyncMutex::new(Some(process)),
            inner,
        }
    }
}

#[async_trait]
impl Module for ExternalModule {
    fn info(&self) -> ModuleInfo {
        self.inner.info()
    }

    async fn init(&self, host: Arc<dyn HostServices>) -> Result<(), ModuleError> {
        self.inner.init(host).await
    }

    async fn start(&self) -> Result<(), ModuleError> {
        self.inner.start().await
    }

    /// Stops the module at the wire level, then unconditionally tears down
    /// its child process — even if the wire-level stop failed or the
    /// process was never fully started, the process must not survive a
    /// `stop()` call. This is the sole place an external plugin's process
    /// is ever killed; the supervisor's unload/shutdown/restart paths reach
    /// it by calling `stop()` on whatever `Arc<dyn Module>` they are
    /// holding, exactly as they already do for a builtin module.
    async fn stop(&self) -> Result<(), ModuleError> {
        if let Err(err) = self.inner.stop().await {
            tracing::warn!(
                error = %err,
                "external plugin module-level stop failed; killing its process anyway"
            );
        }
        let mut guard = self.process.lock().await;
        let Some(process) = guard.take() else {
            return Ok(());
        };
        process
            .shutdown()
            .await
            .map_err(|err| ModuleError::new(err.to_string()))
    }

    async fn status(&self) -> Result<Status, ModuleError> {
        self.inner.status().await
    }

    async fn health(&self) -> HealthReport {
        self.inner.health().await
    }

    fn commands(&self) -> Vec<CommandSpec> {
        self.inner.commands()
    }

    async fn dispatch(
        &self,
        path: &[String],
        flags: &HashMap<String, String>,
        args: &[String],
    ) -> Result<CommandResult, ModuleError> {
        self.inner.dispatch(path, flags, args).await
    }

    fn config_schema(&self) -> Option<Vec<u8>> {
        self.inner.config_schema()
    }
}

/// How [`PluginDirLoader`] builds the [`Verifier`] it verifies each plugin
/// with.
///
/// A `Verifier` cannot be stored as a loader field and reused across calls:
/// `penguin_extplugin::Verifier` holds a `Box<dyn StatSource>` with no
/// `Send`/`Sync` bound on that trait object, so — by construction, in a
/// crate this change does not modify — a `Verifier` value is neither `Send`
/// nor `Sync` and cannot be held live across an `.await` point (see
/// `ExternalLoadError`'s module for why every `ExternalLoader` must be
/// `Send + Sync`). Storing the *construction parameters* instead and
/// building a fresh `Verifier` synchronously inside `load`, entirely before
/// the next `.await`, sidesteps that while keeping identical behaviour to
/// holding one long-lived instance — actually a small correctness
/// improvement for the `Default`/`TrustedDir` cases, since a freshly built
/// `Verifier` re-scans the trusted-publishers directory on every load, so a
/// key pinned there after the daemon started is picked up without a
/// restart.
enum VerifierSource {
    /// [`Verifier::new`] — the embedded key plus [`DEFAULT_TRUSTED_PUBLISHERS_DIR`].
    Default,
    /// [`Verifier::with_trusted_dir`].
    TrustedDir(PathBuf),
    /// [`Verifier::with_keys`] — exactly these keys, nothing implicit.
    Keys(Vec<String>),
}

impl VerifierSource {
    fn build(&self) -> Verifier {
        match self {
            VerifierSource::Default => Verifier::new(),
            VerifierSource::TrustedDir(dir) => Verifier::with_trusted_dir(dir),
            VerifierSource::Keys(keys) => Verifier::with_keys(keys.clone()),
        }
    }
}

/// The production [`ExternalLoader`]: resolves `<plugins_dir>/<name>/`,
/// verifies it, launches it, and wraps the result so its process is torn
/// down on `stop()`.
pub struct PluginDirLoader {
    plugins_dir: PathBuf,
    socket_dir: PathBuf,
    verifier_source: VerifierSource,
    daemon_uid: u32,
}

impl PluginDirLoader {
    /// Builds a loader scanning `plugins_dir` for `<name>/plugin.json`
    /// directories, verifying each against [`Verifier::new`]'s embedded key
    /// plus the system trusted-publishers directory — the production
    /// configuration. `daemon_uid` is the uid plugin files must be owned by
    /// (besides root); production wiring passes the daemon's own uid (see
    /// `bins/penguind`), keeping this loader itself privilege-free to
    /// construct.
    ///
    /// `socket_dir` is where the go-plugin broker would create secondary
    /// unix sockets — unused today since every `load` passes `host_routes:
    /// None` (no built-in module currently needs to receive callbacks from
    /// an external plugin), but required by
    /// [`PluginProcess::launch`]'s signature regardless.
    pub fn new(plugins_dir: PathBuf, socket_dir: PathBuf, daemon_uid: u32) -> PluginDirLoader {
        PluginDirLoader {
            plugins_dir,
            socket_dir,
            verifier_source: VerifierSource::Default,
            daemon_uid,
        }
    }

    /// Same as [`PluginDirLoader::new`], but verifies against
    /// [`Verifier::with_trusted_dir`] instead of the system path — lets a
    /// deployment point at its own trusted-publishers directory.
    pub fn with_trusted_dir(
        plugins_dir: PathBuf,
        socket_dir: PathBuf,
        daemon_uid: u32,
        trusted_dir: PathBuf,
    ) -> PluginDirLoader {
        PluginDirLoader {
            plugins_dir,
            socket_dir,
            verifier_source: VerifierSource::TrustedDir(trusted_dir),
            daemon_uid,
        }
    }

    /// Same as [`PluginDirLoader::new`], but verifies against exactly
    /// `trusted_public_keys` via [`Verifier::with_keys`] — no embedded key,
    /// no directory scan. For tests that inject a fixture's own keypair
    /// rather than trusting production's embedded key.
    pub fn with_keys(
        plugins_dir: PathBuf,
        socket_dir: PathBuf,
        daemon_uid: u32,
        trusted_public_keys: Vec<String>,
    ) -> PluginDirLoader {
        PluginDirLoader {
            plugins_dir,
            socket_dir,
            verifier_source: VerifierSource::Keys(trusted_public_keys),
            daemon_uid,
        }
    }
}

#[async_trait]
impl ExternalLoader for PluginDirLoader {
    async fn load(&self, name: &str) -> Result<Box<dyn Module>, ExternalLoadError> {
        let plugin_dir = self.plugins_dir.join(name);
        if !plugin_dir.is_dir() {
            return Err(ExternalLoadError::NotFound(name.to_string()));
        }

        let manifest =
            load_manifest(&plugin_dir).map_err(|err| ExternalLoadError::Load(err.to_string()))?;
        // Scoped to a block, and the fallible call is a plain `?` inside it,
        // so the non-`Send` `Verifier` this builds is fully dropped before
        // the function's next `.await` — see `VerifierSource`'s doc comment.
        {
            let verifier = self.verifier_source.build();
            verifier
                .verify(&plugin_dir, &manifest, self.daemon_uid)
                .map_err(|err| ExternalLoadError::Load(err.to_string()))?;
        }

        let binary_path = manifest.binary_path(&plugin_dir);
        let process = PluginProcess::launch(&binary_path, &self.socket_dir, None)
            .await
            .map_err(|err| ExternalLoadError::Load(err.to_string()))?;

        let inner = match process.dispense().await {
            Ok(inner) => inner,
            Err(err) => {
                // The process launched and passed its health check, but its
                // ModuleService could not be dispensed — it must not be
                // left running with nothing left to supervise it.
                let _ = process.shutdown().await;
                return Err(ExternalLoadError::Load(err.to_string()));
            }
        };

        Ok(Box::new(ExternalModule::new(process, inner)))
    }
}
