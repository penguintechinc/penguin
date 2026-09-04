//! The real `penguind service <action>` backend for Windows: SCM
//! registration via the [`windows_service`] crate's safe `service_manager`
//! API (`CreateServiceW`/`OpenServiceW`/`StartServiceW`/`DeleteService`/
//! `QueryServiceStatusEx`, all wrapped by that crate without requiring
//! `unsafe` in this file — the crate's own internals are `unsafe`, its
//! public surface is not, so no `#[allow(unsafe_code)]` is needed here).
//!
//! # Registration only — not the SCM dispatch loop
//!
//! This wires the `penguind service install/uninstall/start/stop/status`
//! subcommands (the "ZIP + manual service registration" path documented in
//! `go-client/packaging/windows/WINDOWS_INSTALL.md`), which is
//! registration against the SCM database. It is not the service entry point
//! a binary needs to actually *run under* the SCM
//! (`service_dispatcher::start!` + `define_windows_service!`, both still
//! unused here). `bins/penguind/src/main.rs` only supports Unix targets as
//! a daemon runtime in this milestone (see its `cfg(not(unix))` stub) — the
//! dispatch loop is out of scope for M7.1 and lands with Windows
//! daemon-runtime support.
//!
//! # Compile-gated only
//!
//! No Windows runner exists in this environment (no `windows-*` Rust target
//! is installed, no MSVC/mingw linker) — this file is `cfg(windows)`-gated
//! and is neither compiled nor exercised by anything in this repository's
//! CI today. Written against `windows-service 0.8.1`'s source, not
//! verified by a real build.

use std::ffi::OsString;
use std::io;

use windows_service::Error as WindowsServiceError;
use windows_service::service::{
    ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState, ServiceType,
};
use windows_service::service_manager::{
    ServiceManager as WindowsServiceManager, ServiceManagerAccess,
};

use super::{SERVICE_NAME, ServiceHost};

/// Drives the real Windows Service Control Manager. Compile-gated only —
/// see the module doc.
pub struct RealServiceHost;

impl ServiceHost for RealServiceHost {
    fn install(&self, _unit_content: &str) -> io::Result<()> {
        // The SCM has no unit-file concept — registration is a single API
        // call, not a file write, so `_unit_content` (the systemd/launchd
        // backends' embedded text) does not apply here.
        let manager = open_manager(ServiceManagerAccess::CREATE_SERVICE)?;
        let executable_path = std::env::current_exe()?;
        let info = ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from("Penguin Daemon"),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path,
            launch_arguments: Vec::new(),
            dependencies: Vec::new(),
            account_name: None, // LocalSystem
            account_password: None,
        };
        let service = manager
            .create_service(&info, ServiceAccess::CHANGE_CONFIG)
            .map_err(win_err)?;
        service
            .set_description("Privileged endpoint-agent daemon for Penguin")
            .map_err(win_err)
    }

    fn enable(&self) -> io::Result<()> {
        // Auto-start is already set at `create_service` time
        // (`ServiceStartType::AutoStart`) — the SCM has no separate
        // "enable" step the way systemd/launchd do.
        Ok(())
    }

    fn start(&self) -> io::Result<()> {
        let manager = open_manager(ServiceManagerAccess::CONNECT)?;
        let service = manager
            .open_service(SERVICE_NAME, ServiceAccess::START)
            .map_err(win_err)?;
        service.start(&[] as &[&str]).map_err(win_err)
    }

    fn stop(&self) -> io::Result<()> {
        let manager = open_manager(ServiceManagerAccess::CONNECT)?;
        let service = manager
            .open_service(SERVICE_NAME, ServiceAccess::STOP)
            .map_err(win_err)?;
        service.stop().map(|_status| ()).map_err(win_err)
    }

    fn status(&self) -> io::Result<String> {
        let manager = open_manager(ServiceManagerAccess::CONNECT)?;
        let service = manager
            .open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)
            .map_err(win_err)?;
        let status = service.query_status().map_err(win_err)?;
        Ok(state_name(status.current_state).to_string())
    }

    fn uninstall(&self) -> io::Result<()> {
        let manager = open_manager(ServiceManagerAccess::CONNECT)?;
        let service = manager
            .open_service(SERVICE_NAME, ServiceAccess::DELETE)
            .map_err(win_err)?;
        service.delete().map_err(win_err)
    }
}

/// Connects to the local machine's SCM database with `access`.
fn open_manager(access: ServiceManagerAccess) -> io::Result<WindowsServiceManager> {
    WindowsServiceManager::local_computer(None::<&str>, access).map_err(win_err)
}

/// Flattens [`windows_service::Error`] to a plain `io::Error`, matching
/// every other backend's `io::Result` return type.
fn win_err(err: WindowsServiceError) -> io::Error {
    io::Error::other(err.to_string())
}

/// Maps the SCM's [`ServiceState`] to the short word substituted into
/// `penguind service status: <state>`.
fn state_name(state: ServiceState) -> &'static str {
    match state {
        ServiceState::Stopped => "stopped",
        ServiceState::StartPending => "start_pending",
        ServiceState::StopPending => "stop_pending",
        ServiceState::Running => "running",
        ServiceState::ContinuePending => "continue_pending",
        ServiceState::PausePending => "pause_pending",
        ServiceState::Paused => "paused",
    }
}
