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
//!
//! The `uninstall` verb has a second seam of its own:
//! [`handle_service_command_with_ctx`] takes an explicit, injected
//! `Option<&TeardownCtx>` instead of resolving one from the real secret
//! store, so the self-protection teardown gate (see `penguin_selfprotect`)
//! can be driven deterministically in tests too, without ever touching a
//! real secrets directory.

use std::io;
use std::path::{Path, PathBuf};

use penguin_sdk::SecretStore;
use penguin_secrets::{Backend as SecretsBackend, Config as SecretsConfig, Store as SecretsStore};
use penguin_selfprotect::{TeardownAuthz, TeardownCtx, TeardownInput, authorize};

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

/// Default state directory, matching `daemon_main::Args`'s own
/// `--state-dir` default. Duplicated rather than shared: `service`
/// dispatch (see [`super::main`]) runs before `daemon_main::Args` is ever
/// parsed, so the `uninstall` teardown gate has no parsed flag to read this
/// from and falls back to the same default every real install uses.
const DEFAULT_STATE_DIR: &str = "/var/lib/penguind";

/// Secret-store namespace [`resolve_teardown_ctx`] reads the tamper-
/// protection secret's hash from.
const SELFPROTECT_SECRET_NAMESPACE: &str = "selfprotect";

/// Secret-store key, within [`SELFPROTECT_SECRET_NAMESPACE`], the tamper-
/// protection secret's Argon2id PHC hash is stored under.
const TAMPER_SECRET_KEY: &str = "tamper_secret";

/// Minisign public key trusted to verify `--break-glass` teardown tokens.
///
/// Empty — not a real key — for the same reason `daemon_main`'s
/// `RELEASE_PUBLIC_KEY` is `None`: a placeholder key would look configured
/// while verifying nothing real. SP2 provisions the real PenguinTech
/// break-glass signing key here; until then a `--break-glass` token never
/// verifies (an empty key fails to parse), so only the local-secret and
/// console-deauthorization paths can authorize a teardown on an armed node.
const BREAK_GLASS_PUBKEY: &str = "";

/// Refusal message for an armed, unauthorized `uninstall` attempt. Names
/// every recovery path — including break-glass — so a caller locked out by
/// a lost local secret is never stuck.
const TEARDOWN_REFUSED_MESSAGE: &str = "uninstall refused: this endpoint is tamper-protected. \
Provide --auth <secret>, a --break-glass <token>, or deauthorize the node in the Penguin \
console. Break-glass recovery: docs/self-protection.md.";

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

    let ctx = resolve_teardown_ctx();
    handle_service_command_with_ctx(args, host, ctx.as_ref())
}

/// [`handle_service_command`]'s testable core. `ctx` is the resolved
/// self-protection teardown context, or `None` when this node is not armed
/// — see the module doc's "Testability" section. `ctx == None` means every
/// verb, including `uninstall`, behaves exactly as it did before this gate
/// existed (an unenrolled or dev agent must always stay removable); `ctx ==
/// Some(_)` additionally gates `uninstall` through
/// `penguin_selfprotect::authorize` — every other verb ignores `ctx`
/// entirely.
fn handle_service_command_with_ctx(
    args: &[String],
    host: &dyn ServiceHost,
    ctx: Option<&TeardownCtx>,
) -> Option<Result<String, String>> {
    if args.first().map(String::as_str) != Some("service") {
        return None;
    }

    let Some(action) = args.get(1) else {
        return Some(Err(
            "service: missing action (install|uninstall|start|stop|status)".to_string(),
        ));
    };

    Some(run_action(action, args, host, ctx))
}

/// Dispatches one already-confirmed `service <action>`, matching the exact
/// success/error strings `go-client/cmd/penguind/service_commands.go` prints
/// (see that file's success `fmt.Println`s and `fmt.Errorf` wraps). `args`
/// and `ctx` are only consulted by the `uninstall` branch (see
/// [`run_uninstall`]) — every other verb is untouched by the self-
/// protection gate.
fn run_action(
    action: &str,
    args: &[String],
    host: &dyn ServiceHost,
    ctx: Option<&TeardownCtx>,
) -> Result<String, String> {
    match action {
        "install" => host
            .install(unit_content())
            .and_then(|()| host.enable())
            .map(|()| "penguind service installed successfully".to_string())
            .map_err(|err| format!("install failed: {err}")),
        "uninstall" => run_uninstall(args, host, ctx),
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

/// The `uninstall` verb's own dispatch: gated by [`TeardownCtx`] when this
/// node is armed (`ctx == Some`), free to proceed when it isn't (`ctx ==
/// None`) — see [`handle_service_command_with_ctx`]'s doc for why `None`
/// must mean "proceed." When armed, `host.uninstall()` is only ever called
/// after `penguin_selfprotect::authorize` clears the request — an
/// `Unauthorized` verdict returns [`TEARDOWN_REFUSED_MESSAGE`] and never
/// touches `host` at all.
fn run_uninstall(
    args: &[String],
    host: &dyn ServiceHost,
    ctx: Option<&TeardownCtx>,
) -> Result<String, String> {
    if let Some(ctx) = ctx {
        let input = parse_teardown_input(args);
        if authorize(&input, ctx) == TeardownAuthz::Unauthorized {
            return Err(TEARDOWN_REFUSED_MESSAGE.to_string());
        }
    }

    host.uninstall()
        .map(|()| "penguind service uninstalled successfully".to_string())
        .map_err(|err| format!("uninstall failed: {err}"))
}

/// Parses `--auth <secret>` and `--break-glass <token>` out of the full
/// `service uninstall ...` argument list into a [`TeardownInput`]. Any
/// other token (including `"service"` and `"uninstall"` themselves) is
/// ignored, matching every other `penguind service` verb — none of which
/// validate their argument lists either.
fn parse_teardown_input(args: &[String]) -> TeardownInput {
    let mut secret = None;
    let mut break_glass = None;

    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--auth" => secret = rest.next().cloned(),
            "--break-glass" => break_glass = rest.next().cloned(),
            _ => {}
        }
    }

    TeardownInput {
        secret,
        break_glass,
    }
}

/// Resolves this node's [`TeardownCtx`] for the production `uninstall`
/// gate, or `None` if the node should be treated as unarmed.
///
/// **Interim arming proxy** (documented; a later milestone replaces this
/// with the real `penguin_selfprotect::is_armed(enrolled, flag_on)`): a
/// node is armed here iff it already has a secrets store on disk *and*
/// that store has a [`TAMPER_SECRET_KEY`] provisioned under
/// [`SELFPROTECT_SECRET_NAMESPACE`] — a tamper secret is only ever written
/// at enroll time when protection is turned on, so its presence is
/// currently the best available signal. An unenrolled or dev agent has
/// neither, so it is always treated as unarmed and uninstalls freely.
///
/// Deliberately checks `<state_dir>/secrets` for existence *before* ever
/// calling [`penguin_secrets::Store::open`]: opening a `FileOnly` backend
/// creates the directory and a fresh master key as a side effect if either
/// is missing (see `penguin_secrets::file_backend`'s module doc), which
/// must never happen just from asking "is this node armed?" — including
/// from every pre-existing test in this module that drives the public
/// [`handle_service_command`] directly and must never provision a real
/// secrets directory on the test machine as a side effect of `cargo test`.
fn resolve_teardown_ctx() -> Option<TeardownCtx> {
    let secrets_dir = Path::new(DEFAULT_STATE_DIR).join("secrets");
    if !secrets_dir.is_dir() {
        return None;
    }

    let secret_phc = read_tamper_secret_phc(secrets_dir)?;
    let node_id = nix::unistd::gethostname()
        .ok()
        .and_then(|name| name.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string());

    Some(TeardownCtx {
        is_root: nix::unistd::Uid::effective().is_root(),
        secret_phc: Some(secret_phc),
        node_id,
        pubkey: BREAK_GLASS_PUBKEY.to_string(),
        console_deauthorized: false,
    })
}

/// Opens the real secret store rooted at `secrets_dir` (already known to
/// exist — see [`resolve_teardown_ctx`]) and reads the tamper-protection
/// secret's PHC hash, or `None` if the store can't be opened or the key
/// isn't set. Builds a throwaway single-shot Tokio runtime for the one
/// async `get` call — this path runs in `main()` before `daemon_main::run`
/// ever builds the daemon's own runtime, so there is no outer runtime to
/// reuse.
fn read_tamper_secret_phc(secrets_dir: PathBuf) -> Option<String> {
    let store = SecretsStore::open(SecretsConfig {
        service_name: String::new(),
        backend: SecretsBackend::FileOnly {
            file_dir: secrets_dir,
        },
    })
    .ok()?;
    let namespaced = store.namespaced(SELFPROTECT_SECRET_NAMESPACE);

    let runtime = tokio::runtime::Runtime::new().ok()?;
    let bytes = runtime.block_on(namespaced.get(TAMPER_SECRET_KEY)).ok()?;
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use penguin_selfprotect::hash_secret;

    use super::*;

    /// A [`TeardownCtx`] for an armed node whose local secret is
    /// `"s3cret"`. `pubkey` is left empty because none of this module's
    /// tests exercise the break-glass path — `authorize` never calls
    /// `verify_break_glass` when `TeardownInput::break_glass` is `None`.
    fn test_armed_ctx() -> TeardownCtx {
        TeardownCtx {
            is_root: true,
            secret_phc: Some(hash_secret("s3cret").expect("hash test secret")),
            node_id: "test".to_string(),
            pubkey: String::new(),
            console_deauthorized: false,
        }
    }

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
    fn armed_uninstall_without_auth_is_refused_and_does_not_uninstall() {
        let host = FakeServiceHost::new();
        let ctx = test_armed_ctx();
        let args = vec!["service".to_string(), "uninstall".to_string()];
        let res = handle_service_command_with_ctx(&args, &host, Some(&ctx));
        assert!(res.unwrap().unwrap_err().contains("break-glass"));
        assert_eq!(host.uninstall_calls.get(), 0);
    }

    #[test]
    fn armed_uninstall_with_correct_secret_proceeds() {
        let host = FakeServiceHost::new();
        let ctx = test_armed_ctx();
        let args = vec![
            "service".to_string(),
            "uninstall".to_string(),
            "--auth".to_string(),
            "s3cret".to_string(),
        ];
        let res = handle_service_command_with_ctx(&args, &host, Some(&ctx));
        assert!(res.unwrap().is_ok());
        assert_eq!(host.uninstall_calls.get(), 1);
    }

    #[test]
    fn armed_uninstall_with_wrong_secret_is_refused() {
        let host = FakeServiceHost::new();
        let ctx = test_armed_ctx();
        let args = vec![
            "service".to_string(),
            "uninstall".to_string(),
            "--auth".to_string(),
            "wrong".to_string(),
        ];
        let res = handle_service_command_with_ctx(&args, &host, Some(&ctx));
        assert!(res.unwrap().unwrap_err().contains("break-glass"));
        assert_eq!(host.uninstall_calls.get(), 0);
    }

    #[test]
    fn unarmed_uninstall_proceeds_freely_without_any_auth() {
        let host = FakeServiceHost::new();
        let args = vec!["service".to_string(), "uninstall".to_string()];
        let res = handle_service_command_with_ctx(&args, &host, None);
        assert_eq!(
            res,
            Some(Ok("penguind service uninstalled successfully".to_string()))
        );
        assert_eq!(host.uninstall_calls.get(), 1);
    }

    #[test]
    fn resolve_teardown_ctx_is_none_when_no_secrets_dir_exists() {
        // Every test in this module drives `handle_service_command`
        // (production wiring) or `handle_service_command_with_ctx`
        // (injected ctx). This asserts the production resolver's guard
        // directly: on a machine with no `/var/lib/penguind/secrets` — true
        // for every CI/dev sandbox — it must resolve to `None` rather than
        // creating that directory as a side effect. See
        // `resolve_teardown_ctx`'s doc for why the existence check must
        // come before any `Store::open` call.
        assert!(
            !std::path::Path::new(DEFAULT_STATE_DIR)
                .join("secrets")
                .is_dir()
        );
        assert!(resolve_teardown_ctx().is_none(), "no secrets dir → unarmed");
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
