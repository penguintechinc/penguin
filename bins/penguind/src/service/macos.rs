//! The real `penguind service <action>` backend for macOS: `launchd` via
//! the [`service_manager`] crate's [`LaunchdServiceManager`], plus a direct
//! `launchctl` invocation for `enable` (`load -w`) since the crate's own
//! `install()` only calls plain `load` (no `-w`) and only when `autostart`
//! is set — this milestone needs the `-w` (`overrideDisabled`) flag so the
//! daemon stays loaded across reboots, matching the task's explicit
//! `launchctl load -w` requirement. `status` isn't part of the crate's
//! [`ServiceManager`] trait at all, so it's a direct `launchctl list
//! <label>` presence check.
//!
//! # Compile-gated only
//!
//! There is no macOS runner in this environment — this file is `cfg`-gated
//! to `target_os = "macos"` and is neither compiled nor exercised by the
//! Linux CI/test suite that validates the rest of this crate. It is written
//! against `service-manager 0.7.1`'s source and documented `launchctl`
//! behaviour, not verified by a real build.

use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};

use service_manager::{
    LaunchdServiceManager, ServiceInstallCtx, ServiceLabel, ServiceManager, ServiceStartCtx,
    ServiceStopCtx, ServiceUninstallCtx,
};

use super::{SERVICE_NAME, ServiceHost};

/// Where [`super::LAUNCHD_PLIST`] lands for a system-level `LaunchDaemon`,
/// matching `packaging/launchd/io.penguintech.penguind.plist`'s own
/// intended install location (and the Go reference's identical path).
pub(crate) const LAUNCHD_PLIST_PATH: &str = "/Library/LaunchDaemons/io.penguintech.penguind.plist";

/// Where the installed binary is expected to live. Not actually consulted
/// by `install()` below — a raw-content install ignores `program`/`args`
/// entirely, same as the Linux backend — but still supplied for
/// [`ServiceInstallCtx`]'s sake and to document the deployment convention
/// [`super::LAUNCHD_PLIST`]'s `ProgramArguments` assumes.
const INSTALLED_BINARY_PATH: &str = "/usr/local/bin/penguind";

/// Drives the real `launchd` service manager. Compile-gated only — see the
/// module doc.
pub struct RealServiceHost;

impl RealServiceHost {
    /// The `io.penguintech.penguind` label every operation below addresses.
    fn label() -> ServiceLabel {
        ServiceLabel {
            qualifier: Some("io".to_string()),
            organization: Some("penguintech".to_string()),
            application: SERVICE_NAME.to_string(),
        }
    }
}

impl ServiceHost for RealServiceHost {
    fn install(&self, unit_content: &str) -> io::Result<()> {
        LaunchdServiceManager::system().install(ServiceInstallCtx {
            label: Self::label(),
            program: PathBuf::from(INSTALLED_BINARY_PATH),
            args: Vec::new(),
            contents: Some(unit_content.to_string()),
            username: None,
            working_directory: None,
            environment: None,
            // `enable()` performs the `-w` load explicitly — see its doc.
            autostart: false,
        })
    }

    fn enable(&self) -> io::Result<()> {
        run_launchctl(&["load", "-w", LAUNCHD_PLIST_PATH])
    }

    fn start(&self) -> io::Result<()> {
        LaunchdServiceManager::system().start(ServiceStartCtx {
            label: Self::label(),
        })
    }

    fn stop(&self) -> io::Result<()> {
        LaunchdServiceManager::system().stop(ServiceStopCtx {
            label: Self::label(),
        })
    }

    fn status(&self) -> io::Result<String> {
        let output = Command::new("launchctl")
            .args(["list", &Self::label().to_qualified_name()])
            .output()?;
        if output.status.success() {
            Ok("loaded".to_string())
        } else {
            Ok("not loaded".to_string())
        }
    }

    fn uninstall(&self) -> io::Result<()> {
        LaunchdServiceManager::system().uninstall(ServiceUninstallCtx {
            label: Self::label(),
        })
    }
}

/// Runs `launchctl <args>`, mapping a non-zero exit to an `io::Error`.
fn run_launchctl(args: &[&str]) -> io::Result<()> {
    Command::new("launchctl")
        .args(args)
        .status()
        .and_then(exit_status_to_result)
}

fn exit_status_to_result(status: ExitStatus) -> io::Result<()> {
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("launchctl exited with {status}")))
    }
}
