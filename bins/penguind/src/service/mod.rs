//! `penguind service <action>` — install/uninstall/start/stop/status
//! against the host OS's real service manager (systemd on Linux, launchd on
//! macOS, the Windows SCM). Ported from
//! `go-client/cmd/penguind/service_commands.go`, but deliberately NOT
//! porting that build's install-path divergence: Go's `.deb`/`.rpm`
//! packaging shipped the hardened
//! `go-client/packaging/systemd/penguind.service` unit, while `penguind
//! service install` (kardianos/service) wrote a generic, unhardened unit of
//! its own instead. Here there is exactly one unit, embedded via
//! `include_str!` from `packaging/systemd/penguind.service` (repo root),
//! and `install` always writes exactly that — never a
//! service-manager-generated approximation of it.
//!
//! # Testability
//!
//! [`ServiceHost`] is the seam: [`handle_service_command`] never calls
//! systemd/launchd/the SCM directly, only through a `&dyn ServiceHost`. The
//! real per-platform implementations (`linux::RealServiceHost`,
//! `macos::RealServiceHost`, `windows::RealServiceHost`, each `cfg`-gated to
//! its own target) are wired in production via [`real_host`]; the unit
//! suite below drives a `FakeServiceHost` instead, so `cargo test` never
//! shells out to `systemctl`/`launchctl`/`sc.exe` and never triggers
//! polkit.

use std::io;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// The service name/label registered with the OS service manager, matching
/// the Go reference's `service.Config{Name: "penguind", ...}`.
pub const SERVICE_NAME: &str = "penguind";

/// The hardened systemd unit this daemon needs, embedded verbatim from
/// `packaging/systemd/penguind.service` (repo root) — see that file for the
/// directive-by-directive rationale. `install` on Linux writes exactly this
/// string, never a generated approximation (see the module doc).
#[cfg(target_os = "linux")]
pub const SYSTEMD_UNIT: &str = include_str!("../../../../packaging/systemd/penguind.service");

/// The launchd property list this daemon needs, embedded verbatim from
/// `packaging/launchd/io.penguintech.penguind.plist` (repo root).
#[cfg(target_os = "macos")]
pub const LAUNCHD_PLIST: &str =
    include_str!("../../../../packaging/launchd/io.penguintech.penguind.plist");

/// Operations `penguind service <action>` needs from the host OS's service
/// manager. Exists purely so [`handle_service_command`] can be driven by a
/// `FakeServiceHost` in tests instead of a real systemd/launchd/SCM — see
/// the module doc.
pub trait ServiceHost {
    /// Writes `unit_content` verbatim to this platform's canonical service
    /// definition path (mode 0644 on Unix).
    fn install(&self, unit_content: &str) -> io::Result<()>;
    /// Makes an already-written definition take effect: `systemctl
    /// daemon-reload` + `systemctl enable` on Linux, `launchctl load -w` on
    /// macOS, a no-op on Windows (auto-start is already set at
    /// `create_service` time — the SCM has no separate enable step).
    fn enable(&self) -> io::Result<()>;
    /// Starts the service.
    fn start(&self) -> io::Result<()>;
    /// Stops the service.
    fn stop(&self) -> io::Result<()>;
    /// A short, platform-native state word (e.g. `"active"`, `"running"`),
    /// substituted verbatim into `penguind service status: <state>`.
    fn status(&self) -> io::Result<String>;
    /// Removes the service registration.
    fn uninstall(&self) -> io::Result<()>;
}

/// The real [`ServiceHost`] for the platform this binary is built for.
#[cfg(target_os = "linux")]
pub fn real_host() -> impl ServiceHost {
    linux::RealServiceHost
}

/// The real [`ServiceHost`] for the platform this binary is built for.
#[cfg(target_os = "macos")]
pub fn real_host() -> impl ServiceHost {
    macos::RealServiceHost
}

/// The real [`ServiceHost`] for the platform this binary is built for.
#[cfg(windows)]
pub fn real_host() -> impl ServiceHost {
    windows::RealServiceHost
}

/// This platform's embedded service-definition content, handed to
/// [`ServiceHost::install`] by [`run_action`].
#[cfg(target_os = "linux")]
fn unit_content() -> &'static str {
    SYSTEMD_UNIT
}

/// This platform's embedded service-definition content, handed to
/// [`ServiceHost::install`] by [`run_action`].
#[cfg(target_os = "macos")]
fn unit_content() -> &'static str {
    LAUNCHD_PLIST
}

/// The Windows SCM has no unit-file concept — registration is a single API
/// call (see `windows::RealServiceHost::install`), so there is no content to
/// hand it.
#[cfg(windows)]
fn unit_content() -> &'static str {
    ""
}

/// Handles `penguind service <action>`, mirroring
/// `go-client/cmd/penguind/service_commands.go`'s `handleServiceCommand`.
///
/// Returns `None` when `args` doesn't start with `service` — the caller
/// should fall through to normal daemon startup. Returns `Some(Ok(line))`
/// when the action succeeded, `line` being the exact success message to
/// print to stdout. Returns `Some(Err(message))` when the action failed or
/// the action name was invalid; the caller should print `penguind:
/// {message}` to stderr and exit non-zero.
pub fn handle_service_command(
    args: &[String],
    host: &dyn ServiceHost,
) -> Option<Result<String, String>> {
    if args.first().map(String::as_str) != Some("service") {
        return None;
    }

    let Some(action) = args.get(1) else {
        return Some(Err(
            "service: missing action (install|uninstall|start|stop|status)".to_string(),
        ));
    };

    Some(run_action(action, host))
}

/// Dispatches one already-confirmed `service <action>`, matching the exact
/// success/error strings `go-client/cmd/penguind/service_commands.go` prints
/// (see that file's success `fmt.Println`s and `fmt.Errorf` wraps).
fn run_action(action: &str, host: &dyn ServiceHost) -> Result<String, String> {
    match action {
        "install" => host
            .install(unit_content())
            .and_then(|()| host.enable())
            .map(|()| "penguind service installed successfully".to_string())
            .map_err(|err| format!("install failed: {err}")),
        "uninstall" => host
            .uninstall()
            .map(|()| "penguind service uninstalled successfully".to_string())
            .map_err(|err| format!("uninstall failed: {err}")),
        "start" => host
            .start()
            .map(|()| "penguind service started".to_string())
            .map_err(|err| format!("start failed: {err}")),
        "stop" => host
            .stop()
            .map(|()| "penguind service stopped".to_string())
            .map_err(|err| format!("stop failed: {err}")),
        "status" => host
            .status()
            .map(|state| format!("penguind service status: {state}"))
            .map_err(|err| format!("status check failed: {err}")),
        other => Err(format!(
            "service: unknown action {other:?} (install|uninstall|start|stop|status)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;

    /// A [`ServiceHost`] test double that records every call instead of
    /// touching a real service manager. Every test in this module drives
    /// this, never a `RealServiceHost` — see the parent module's
    /// "Testability" doc. Failure injection is per-operation via the
    /// `*_err` fields (`Some(message)` makes that call fail).
    struct FakeServiceHost {
        installed_content: RefCell<Vec<String>>,
        enable_calls: Cell<u32>,
        start_calls: Cell<u32>,
        stop_calls: Cell<u32>,
        uninstall_calls: Cell<u32>,
        install_err: RefCell<Option<String>>,
        enable_err: RefCell<Option<String>>,
        start_err: RefCell<Option<String>>,
        stop_err: RefCell<Option<String>>,
        uninstall_err: RefCell<Option<String>>,
        status_result: RefCell<Result<String, String>>,
    }

    impl FakeServiceHost {
        fn new() -> Self {
            Self {
                installed_content: RefCell::new(Vec::new()),
                enable_calls: Cell::new(0),
                start_calls: Cell::new(0),
                stop_calls: Cell::new(0),
                uninstall_calls: Cell::new(0),
                install_err: RefCell::new(None),
                enable_err: RefCell::new(None),
                start_err: RefCell::new(None),
                stop_err: RefCell::new(None),
                uninstall_err: RefCell::new(None),
                status_result: RefCell::new(Ok("active".to_string())),
            }
        }
    }

    impl ServiceHost for FakeServiceHost {
        fn install(&self, unit_content: &str) -> io::Result<()> {
            self.installed_content
                .borrow_mut()
                .push(unit_content.to_string());
            fail_if_set(&self.install_err)
        }

        fn enable(&self) -> io::Result<()> {
            self.enable_calls.set(self.enable_calls.get() + 1);
            fail_if_set(&self.enable_err)
        }

        fn start(&self) -> io::Result<()> {
            self.start_calls.set(self.start_calls.get() + 1);
            fail_if_set(&self.start_err)
        }

        fn stop(&self) -> io::Result<()> {
            self.stop_calls.set(self.stop_calls.get() + 1);
            fail_if_set(&self.stop_err)
        }

        fn status(&self) -> io::Result<String> {
            self.status_result
                .borrow()
                .clone()
                .map_err(io::Error::other)
        }

        fn uninstall(&self) -> io::Result<()> {
            self.uninstall_calls.set(self.uninstall_calls.get() + 1);
            fail_if_set(&self.uninstall_err)
        }
    }

    /// Shared by every `FakeServiceHost` method above: `Ok(())` unless the
    /// matching `*_err` slot has been armed with a message.
    fn fail_if_set(slot: &RefCell<Option<String>>) -> io::Result<()> {
        match slot.borrow().clone() {
            Some(message) => Err(io::Error::other(message)),
            None => Ok(()),
        }
    }

    /// The exact directive lines required by M7.1 — see the task's own
    /// hardened-unit table. Checked byte-for-byte, not just by key, so a
    /// future edit that weakens a value (e.g. `ProtectSystem=full` instead
    /// of `strict`) fails the same as dropping the line entirely.
    #[cfg(target_os = "linux")]
    const REQUIRED_HARDENING_DIRECTIVES: &[&str] = &[
        "CapabilityBoundingSet=CAP_NET_ADMIN CAP_NET_BIND_SERVICE",
        "AmbientCapabilities=CAP_NET_ADMIN CAP_NET_BIND_SERVICE",
        "NoNewPrivileges=true",
        "ProtectSystem=strict",
        "ProtectHome=true",
        "PrivateTmp=true",
        "ProtectKernelTunables=true",
        "ProtectKernelModules=true",
        "ProtectControlGroups=true",
        "RestrictSUIDSGID=true",
        "RestrictNamespaces=true",
        "LockPersonality=true",
        "ReadWritePaths=/etc/resolv.conf",
        "User=penguind",
        "Group=penguin",
        "RuntimeDirectory=penguin",
        "RuntimeDirectoryMode=0750",
        "StateDirectory=penguind",
        "StateDirectoryMode=0700",
    ];

    #[test]
    fn non_service_args_return_none() {
        let host = FakeServiceHost::new();
        assert!(handle_service_command(&[], &host).is_none());
        assert!(handle_service_command(&["version".to_string()], &host).is_none());
        assert!(handle_service_command(&["foo".to_string()], &host).is_none());
    }

    #[test]
    fn missing_action_is_the_exact_go_error() {
        let host = FakeServiceHost::new();
        let result = handle_service_command(&["service".to_string()], &host);
        assert_eq!(
            result,
            Some(Err(
                "service: missing action (install|uninstall|start|stop|status)".to_string()
            ))
        );
    }

    #[test]
    fn unknown_action_is_the_exact_go_error() {
        let host = FakeServiceHost::new();
        let args = vec!["service".to_string(), "bogus".to_string()];
        let result = handle_service_command(&args, &host);
        assert_eq!(
            result,
            Some(Err(
                r#"service: unknown action "bogus" (install|uninstall|start|stop|status)"#
                    .to_string()
            ))
        );
    }

    #[test]
    fn install_writes_the_unit_then_calls_enable() {
        let host = FakeServiceHost::new();
        let args = vec!["service".to_string(), "install".to_string()];
        let result = handle_service_command(&args, &host);
        assert_eq!(
            result,
            Some(Ok("penguind service installed successfully".to_string()))
        );
        assert_eq!(host.installed_content.borrow().len(), 1);
        assert_eq!(
            host.enable_calls.get(),
            1,
            "install must reload/enable after writing the unit"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn installed_content_is_exactly_the_embedded_hardened_systemd_unit() {
        let host = FakeServiceHost::new();
        let args = vec!["service".to_string(), "install".to_string()];
        let result = handle_service_command(&args, &host);
        assert!(matches!(result, Some(Ok(_))));

        let written = host.installed_content.borrow();
        assert_eq!(written.len(), 1);
        assert_eq!(
            written[0], SYSTEMD_UNIT,
            "install must write the unit byte-for-byte"
        );
        for directive in REQUIRED_HARDENING_DIRECTIVES {
            assert!(
                written[0].contains(directive),
                "installed content missing hardening directive: {directive}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn embedded_systemd_unit_contains_every_required_hardening_directive() {
        for directive in REQUIRED_HARDENING_DIRECTIVES {
            assert!(
                SYSTEMD_UNIT.contains(directive),
                "embedded unit missing hardening directive: {directive}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn systemd_unit_path_is_the_canonical_system_unit_path() {
        assert_eq!(
            linux::SYSTEMD_UNIT_PATH,
            "/etc/systemd/system/penguind.service"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn systemd_unit_is_hardened_for_auto_restart() {
        let unit = super::SYSTEMD_UNIT;
        assert!(unit.contains("Restart=always"));
        assert!(unit.contains("StartLimitIntervalSec=0"));
        assert!(unit.contains("RestartSec="));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launchd_plist_keepalive_true() {
        assert!(super::LAUNCHD_PLIST.contains("KeepAlive"));
    }

    #[test]
    fn install_failure_is_wrapped_go_style() {
        let host = FakeServiceHost::new();
        *host.install_err.borrow_mut() = Some("permission denied".to_string());
        let args = vec!["service".to_string(), "install".to_string()];
        let result = handle_service_command(&args, &host);
        assert_eq!(
            result,
            Some(Err("install failed: permission denied".to_string()))
        );
        assert_eq!(
            host.enable_calls.get(),
            0,
            "enable must not run if install failed"
        );
    }

    #[test]
    fn install_enable_failure_is_wrapped_go_style() {
        let host = FakeServiceHost::new();
        *host.enable_err.borrow_mut() = Some("daemon-reload failed".to_string());
        let args = vec!["service".to_string(), "install".to_string()];
        let result = handle_service_command(&args, &host);
        assert_eq!(
            result,
            Some(Err("install failed: daemon-reload failed".to_string()))
        );
        assert_eq!(
            host.installed_content.borrow().len(),
            1,
            "the unit is still written even if the subsequent enable fails"
        );
    }

    #[test]
    fn uninstall_prints_the_exact_go_string() {
        let host = FakeServiceHost::new();
        let args = vec!["service".to_string(), "uninstall".to_string()];
        let result = handle_service_command(&args, &host);
        assert_eq!(
            result,
            Some(Ok("penguind service uninstalled successfully".to_string()))
        );
        assert_eq!(host.uninstall_calls.get(), 1);
    }

    #[test]
    fn uninstall_failure_is_wrapped_go_style() {
        let host = FakeServiceHost::new();
        *host.uninstall_err.borrow_mut() = Some("not installed".to_string());
        let args = vec!["service".to_string(), "uninstall".to_string()];
        let result = handle_service_command(&args, &host);
        assert_eq!(
            result,
            Some(Err("uninstall failed: not installed".to_string()))
        );
    }

    #[test]
    fn start_prints_the_exact_go_string() {
        let host = FakeServiceHost::new();
        let args = vec!["service".to_string(), "start".to_string()];
        let result = handle_service_command(&args, &host);
        assert_eq!(result, Some(Ok("penguind service started".to_string())));
        assert_eq!(host.start_calls.get(), 1);
    }

    #[test]
    fn start_failure_is_wrapped_go_style() {
        let host = FakeServiceHost::new();
        *host.start_err.borrow_mut() = Some("unit not found".to_string());
        let args = vec!["service".to_string(), "start".to_string()];
        let result = handle_service_command(&args, &host);
        assert_eq!(
            result,
            Some(Err("start failed: unit not found".to_string()))
        );
    }

    #[test]
    fn stop_prints_the_exact_go_string() {
        let host = FakeServiceHost::new();
        let args = vec!["service".to_string(), "stop".to_string()];
        let result = handle_service_command(&args, &host);
        assert_eq!(result, Some(Ok("penguind service stopped".to_string())));
        assert_eq!(host.stop_calls.get(), 1);
    }

    #[test]
    fn stop_failure_is_wrapped_go_style() {
        let host = FakeServiceHost::new();
        *host.stop_err.borrow_mut() = Some("unit not active".to_string());
        let args = vec!["service".to_string(), "stop".to_string()];
        let result = handle_service_command(&args, &host);
        assert_eq!(
            result,
            Some(Err("stop failed: unit not active".to_string()))
        );
    }

    #[test]
    fn status_prints_the_exact_go_format() {
        let host = FakeServiceHost::new();
        *host.status_result.borrow_mut() = Ok("active".to_string());
        let args = vec!["service".to_string(), "status".to_string()];
        let result = handle_service_command(&args, &host);
        assert_eq!(
            result,
            Some(Ok("penguind service status: active".to_string()))
        );
    }

    #[test]
    fn status_failure_is_wrapped_go_style() {
        let host = FakeServiceHost::new();
        *host.status_result.borrow_mut() = Err("no such unit".to_string());
        let args = vec!["service".to_string(), "status".to_string()];
        let result = handle_service_command(&args, &host);
        assert_eq!(
            result,
            Some(Err("status check failed: no such unit".to_string()))
        );
    }
}
