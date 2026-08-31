//! `penguind watchdog` — the daemon's mutual-supervision peer process.
//!
//! Self-protection for the agent as a *pair*, not a single point of
//! failure: `daemon_main::run_daemon` best-effort spawns a `penguind
//! watchdog` child at startup, and this module's [`run_watchdog`] loop
//! supervises the daemon right back — checking its liveness on a fixed
//! interval and relaunching it if it's gone. Killing either process alone
//! therefore does not stop the agent; the survivor relaunches its peer
//! within one supervision tick. This is deliberately not a fight against an
//! *authorized* stop: `systemctl stop`/`uninstall` signals the whole
//! service cgroup (systemd's default `KillMode=control-group`), which
//! includes this watchdog child, so both processes exit together rather
//! than the watchdog trying to resurrect a deliberately-stopped daemon.
//!
//! [`WatchTarget`] is the seam that makes the supervision decision
//! (relaunch vs. no-op) unit-testable without ever spawning a real process
//! — see the `#[cfg(test)]` section's `FakeTarget`. [`ProcessTarget`] is
//! the real, Unix-only implementation `run_watchdog` drives in production.

use std::io;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;
use std::process::ExitCode;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use penguin_daemon::lock::{self, LockError};

/// What the watchdog supervises: something that can report whether it's
/// currently alive and be relaunched if it isn't. A trait so
/// [`supervise_once`] can be driven by a `FakeTarget` in tests instead of a
/// real process — see this module's `#[cfg(test)]` section.
pub trait WatchTarget {
    /// True if the target is currently running / healthy.
    fn is_alive(&self) -> bool;
    /// Starts (or restarts) the target. A failure is reported by the
    /// caller (logged), never panicked on here — a failed relaunch attempt
    /// must not crash the watchdog itself, or there would be nothing left
    /// to retry on the next tick.
    fn relaunch(&self) -> io::Result<()>;
}

/// What [`supervise_once`] did on one tick.
#[derive(Debug, PartialEq, Eq)]
pub enum WatchAction {
    /// The target was not alive and a relaunch was attempted.
    Relaunched,
    /// The target was already alive; nothing to do.
    Alive,
}

/// Checks `target` once: relaunches it if [`WatchTarget::is_alive`] is
/// false, otherwise no-ops. A relaunch failure is logged, not propagated —
/// the caller's loop simply tries again on its next tick (see
/// [`run_watchdog`]'s backoff), so one failed spawn attempt never brings
/// the watchdog itself down.
pub fn supervise_once(target: &dyn WatchTarget) -> WatchAction {
    if target.is_alive() {
        return WatchAction::Alive;
    }

    if let Err(err) = target.relaunch() {
        tracing::warn!(error = %err, "watchdog: failed to relaunch supervised peer");
    }
    WatchAction::Relaunched
}

/// Default `--state-dir` this module assumes for both the daemon's lock
/// file and the watchdog's own singleton lock — matches
/// `daemon_main::Args`'s own default. Duplicated rather than shared: the
/// `watchdog` subcommand short-circuits in `main` before any argument
/// parsing exists (see that dispatch's doc), so there is no parsed flag to
/// read a custom path from. A daemon started with a non-default
/// `--state-dir` needs a `penguind watchdog` invocation that knows about
/// it too — not implemented in this milestone; the production systemd unit
/// (`packaging/systemd/penguind.service`) always uses this exact default,
/// so this is correct for the shipped deployment path.
#[cfg(unix)]
const DEFAULT_STATE_DIR: &str = "/var/lib/penguind";

/// Subdirectory, under the state dir, holding the watchdog's own
/// single-instance lock (kept separate from the daemon's own
/// `<state_dir>/penguind.lock` so the two locks never collide — see
/// [`run_watchdog`]).
#[cfg(unix)]
const WATCHDOG_LOCK_SUBDIR: &str = "watchdog";

/// How often [`run_watchdog`] checks the daemon's liveness when it was
/// already found alive. After a relaunch this is replaced by the daemon's
/// own restart-backoff formula (see [`penguin_daemon::backoff`]) so a
/// persistently crashing daemon cannot drive the watchdog into a tight
/// respawn storm.
#[cfg(unix)]
const SUPERVISE_INTERVAL: Duration = Duration::from_secs(5);

/// Supervises the real `penguind` daemon process.
///
/// Liveness is read from the same single-instance `flock` the daemon
/// itself holds for its entire lifetime (see [`penguin_daemon::lock`]): a
/// *successful* non-blocking acquire of that lock means nobody currently
/// holds it (dead), while [`LockError::AlreadyRunning`] means the daemon is
/// up. A relaunch starts a fresh `penguind` with no arguments — i.e. the
/// daemon's own compiled-in defaults, which match the packaged systemd
/// unit's `ExecStart` (`--config-dir /etc/penguin --state-dir
/// /var/lib/penguind`).
#[cfg(unix)]
pub struct ProcessTarget {
    state_dir: PathBuf,
    exe: PathBuf,
}

#[cfg(unix)]
impl ProcessTarget {
    /// A target for the daemon at [`DEFAULT_STATE_DIR`], relaunching via
    /// the currently-running executable's own path (this binary itself —
    /// `penguind watchdog` and `penguind` are the same executable).
    pub fn for_daemon() -> io::Result<Self> {
        Ok(Self {
            state_dir: PathBuf::from(DEFAULT_STATE_DIR),
            exe: std::env::current_exe()?,
        })
    }
}

#[cfg(unix)]
impl WatchTarget for ProcessTarget {
    /// Alive iff another process currently holds the daemon's
    /// single-instance lock. The probe guard (on a successful acquire) is
    /// dropped immediately, releasing the lock right back — this call's
    /// only purpose is the check, never to hold the lock itself. Any
    /// failure to even open the lock file (e.g. the state dir doesn't
    /// exist yet) is conservatively also read as "not alive," matching
    /// pre-first-start state.
    fn is_alive(&self) -> bool {
        match lock::acquire(&self.state_dir) {
            Ok(_probe_guard) => false,
            Err(LockError::AlreadyRunning { .. }) => true,
            Err(LockError::Acquire { .. }) => false,
        }
    }

    /// Spawns a fresh `penguind` (no arguments) and does not wait for it —
    /// the daemon runs indefinitely, so waiting here would block the
    /// watchdog loop forever on the success path.
    fn relaunch(&self) -> io::Result<()> {
        Command::new(&self.exe).spawn().map(|_child| ())
    }
}

/// Runs `penguind watchdog`: the long-lived half of mutual supervision —
/// see this module's doc for the full picture.
///
/// First takes its own singleton lock at
/// `<state_dir>/watchdog/penguind.lock` (reusing [`penguin_daemon::lock`]
/// pointed at a dedicated subdirectory, so it never collides with the
/// daemon's own `<state_dir>/penguind.lock`). This guards against
/// accumulating idle watchdog processes: `daemon_main::spawn_watchdog_peer`
/// best-effort spawns a new `penguind watchdog` child every time the
/// daemon starts, including every `Restart=always` crash-restart cycle —
/// systemd's `KillMode=control-group` only reaps a service's children on a
/// full `systemctl stop`, not on a plain crash-restart, so without this
/// guard each restart would leave the previous watchdog still running. A
/// second `penguind watchdog` that loses this race exits immediately
/// rather than duplicating supervision.
///
/// Then loops [`supervise_once`] against a real [`ProcessTarget`] on
/// [`SUPERVISE_INTERVAL`], applying [`penguin_daemon::backoff`]'s formula
/// after a relaunch. Never returns under normal operation — it runs until
/// the process receives a termination signal (systemd, or the daemon's own
/// process-group teardown on an authorized stop).
#[cfg(unix)]
pub fn run_watchdog() -> ExitCode {
    let state_dir = Path::new(DEFAULT_STATE_DIR);
    let watchdog_dir = state_dir.join(WATCHDOG_LOCK_SUBDIR);
    if let Err(err) = create_state_dir(&watchdog_dir) {
        eprintln!("penguind watchdog: create watchdog state dir: {err}");
        return ExitCode::FAILURE;
    }

    let _singleton_guard = match lock::acquire(&watchdog_dir) {
        Ok(guard) => guard,
        Err(LockError::AlreadyRunning { .. }) => {
            eprintln!("penguind watchdog: another watchdog is already running; exiting");
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            eprintln!("penguind watchdog: acquire singleton lock: {err}");
            return ExitCode::FAILURE;
        }
    };

    let target = match ProcessTarget::for_daemon() {
        Ok(target) => target,
        Err(err) => {
            eprintln!("penguind watchdog: resolve own executable path: {err}");
            return ExitCode::FAILURE;
        }
    };

    let mut restart_attempt: u32 = 0;
    loop {
        match supervise_once(&target) {
            WatchAction::Alive => {
                restart_attempt = 0;
                std::thread::sleep(SUPERVISE_INTERVAL);
            }
            WatchAction::Relaunched => {
                let backoff = penguin_daemon::backoff::delay_for_random(restart_attempt);
                restart_attempt = (restart_attempt + 1).min(penguin_daemon::backoff::MAX_RESTARTS);
                std::thread::sleep(backoff.max(SUPERVISE_INTERVAL));
            }
        }
    }
}

/// Creates `path` (and any missing parents) with mode `0700` if it does not
/// already exist — same convention as `daemon_main::ensure_state_dir`,
/// duplicated here rather than shared since that helper is private to
/// `daemon_main` and this is the only other caller.
#[cfg(unix)]
fn create_state_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    builder.mode(0o700);
    builder.create(path)
}

/// Non-Unix stub: the daemon itself only runs on Unix in this milestone
/// (see `main.rs`'s `run` stub), so there is nothing for a watchdog to
/// supervise yet.
#[cfg(not(unix))]
pub fn run_watchdog() -> ExitCode {
    eprintln!("penguind watchdog: only Unix targets are supported in this milestone");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// A [`WatchTarget`] test double: an in-memory alive flag plus a
    /// relaunch call counter, so tests can assert exactly how many times
    /// [`supervise_once`] attempted a relaunch without ever spawning a real
    /// process.
    struct FakeTarget {
        alive: Cell<bool>,
        relaunch_calls: Cell<u32>,
        fail_relaunch: bool,
    }

    impl FakeTarget {
        /// A target that reports itself as not running.
        fn dead() -> Self {
            Self {
                alive: Cell::new(false),
                relaunch_calls: Cell::new(0),
                fail_relaunch: false,
            }
        }

        /// A target that reports itself as running.
        fn alive() -> Self {
            Self {
                alive: Cell::new(true),
                relaunch_calls: Cell::new(0),
                fail_relaunch: false,
            }
        }

        /// A dead target whose `relaunch` always fails — used to prove a
        /// failed relaunch attempt is reported, not panicked on.
        fn dead_with_failing_relaunch() -> Self {
            Self {
                alive: Cell::new(false),
                relaunch_calls: Cell::new(0),
                fail_relaunch: true,
            }
        }

        fn relaunch_calls(&self) -> u32 {
            self.relaunch_calls.get()
        }
    }

    impl WatchTarget for FakeTarget {
        fn is_alive(&self) -> bool {
            self.alive.get()
        }

        fn relaunch(&self) -> io::Result<()> {
            self.relaunch_calls.set(self.relaunch_calls.get() + 1);
            if self.fail_relaunch {
                Err(io::Error::other("fake relaunch failure"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn supervise_relaunches_a_dead_peer() {
        let dead = FakeTarget::dead();
        assert_eq!(supervise_once(&dead), WatchAction::Relaunched);
        assert_eq!(dead.relaunch_calls(), 1);

        let alive = FakeTarget::alive();
        assert_eq!(supervise_once(&alive), WatchAction::Alive);
        assert_eq!(alive.relaunch_calls(), 0);
    }

    #[test]
    fn a_failing_relaunch_is_reported_but_does_not_panic() {
        let dead = FakeTarget::dead_with_failing_relaunch();
        assert_eq!(supervise_once(&dead), WatchAction::Relaunched);
        assert_eq!(dead.relaunch_calls(), 1);
    }
}
