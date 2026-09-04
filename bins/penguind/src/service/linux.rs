//! The real `penguind service <action>` backend for Linux: `systemd` via
//! the [`service_manager`] crate for the verbs it genuinely provides
//! (install/uninstall/start/stop), plus two direct `systemctl` invocations
//! for what it doesn't: `daemon-reload` (there is no crate API for it at
//! all) and `is-active` (the crate's [`ServiceManager`] trait has no
//! `status()` method whatsoever).
//!
//! # `service-manager`'s raw-content override is real
//!
//! [`ServiceInstallCtx::contents`] is not hypothetical: reading
//! `service-manager 0.7.1`'s `src/systemd.rs` confirms that when it is
//! `Some`, the crate's own unit-file *template* (`make_service`) is never
//! invoked — the given string is written byte-for-byte via its
//! `utils::write_file` at mode `0644`. So [`RealServiceHost::install`] hands
//! it [`super::SYSTEMD_UNIT`] directly rather than hand-rolling the file
//! write; this is exactly as byte-exact as writing the file ourselves would
//! be, and is the one Go install-path divergence this milestone must NOT
//! repeat (see the parent module's doc).

use std::io;
use std::path::PathBuf;
use std::process::Command;

use service_manager::{
    ServiceInstallCtx, ServiceLabel, ServiceManager, ServiceStartCtx, ServiceStopCtx,
    ServiceUninstallCtx, SystemdServiceManager,
};

use super::{SERVICE_NAME, ServiceHost};

/// Where [`super::SYSTEMD_UNIT`] lands — `/etc/systemd/system/penguind.service`,
/// the same path a system-level (non-`--user`) [`SystemdServiceManager`]
/// derives from the unqualified `penguind` label (`to_script_name()`), and
/// the path Go's `.deb`/`.rpm` packaging already used.
pub(crate) const SYSTEMD_UNIT_PATH: &str = "/etc/systemd/system/penguind.service";

/// Where the installed binary is expected to live. Not actually consulted
/// by `install()` below — a raw-content install ignores `program`/`args`
/// entirely, see the module doc — but still supplied for
/// [`ServiceInstallCtx`]'s sake and to document the deployment convention
/// [`super::SYSTEMD_UNIT`]'s `ExecStart` assumes.
const INSTALLED_BINARY_PATH: &str = "/usr/bin/penguind";

/// Drives the real `systemd` service manager. Never touched by unit tests —
/// see the parent module's "Testability" doc.
pub struct RealServiceHost;

impl RealServiceHost {
    /// The unqualified `penguind` label every operation below addresses.
    fn label() -> ServiceLabel {
        ServiceLabel {
            qualifier: None,
            organization: None,
            application: SERVICE_NAME.to_string(),
        }
    }
}

impl ServiceHost for RealServiceHost {
    fn install(&self, unit_content: &str) -> io::Result<()> {
        // `SystemdServiceManager` derives its own write path from `label()`
        // rather than accepting one — this keeps that derivation pinned to
        // `SYSTEMD_UNIT_PATH` (asserted, not just documented) so the two
        // can never silently drift apart.
        debug_assert_eq!(
            service_manager::systemd_global_dir_path().join(format!("{SERVICE_NAME}.service")),
            std::path::Path::new(SYSTEMD_UNIT_PATH)
        );

        SystemdServiceManager::system().install(ServiceInstallCtx {
            label: Self::label(),
            program: PathBuf::from(INSTALLED_BINARY_PATH),
            args: Vec::new(),
            contents: Some(unit_content.to_string()),
            username: None,
            working_directory: None,
            environment: None,
            // `enable()` performs the systemd-specific enable step
            // explicitly, after `daemon-reload` — see its doc below.
            autostart: false,
        })
    }

    fn enable(&self) -> io::Result<()> {
        run_systemctl(&["daemon-reload"])?;
        run_systemctl(&["enable", SERVICE_NAME])
    }

    fn start(&self) -> io::Result<()> {
        SystemdServiceManager::system().start(ServiceStartCtx {
            label: Self::label(),
        })
    }

    fn stop(&self) -> io::Result<()> {
        SystemdServiceManager::system().stop(ServiceStopCtx {
            label: Self::label(),
        })
    }

    fn status(&self) -> io::Result<String> {
        // `systemctl is-active` exits non-zero for "inactive"/"failed", but
        // still prints the real state word to stdout — unlike the other
        // verbs here, a non-zero exit is not itself a failure to report.
        let output = Command::new("systemctl")
            .args(["is-active", SERVICE_NAME])
            .output()?;
        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if state.is_empty() {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(io::Error::other(message))
        } else {
            Ok(state)
        }
    }

    fn uninstall(&self) -> io::Result<()> {
        SystemdServiceManager::system().uninstall(ServiceUninstallCtx {
            label: Self::label(),
        })
    }
}

/// Runs `systemctl <args>`, mapping a non-zero exit to an `io::Error`
/// carrying stderr — the same convention [`service_manager`]'s own
/// `systemctl` helper uses internally, for the two operations
/// (`daemon-reload`, `is-active`) that crate doesn't expose at all.
fn run_systemctl(args: &[&str]) -> io::Result<()> {
    let output = Command::new("systemctl").args(args).output()?;
    if output.status.success() {
        Ok(())
    } else {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(io::Error::other(message))
    }
}
