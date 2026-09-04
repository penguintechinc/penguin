//! The daemon-facing event loop shared by every platform shell: after an
//! initial render it drives itself off `WatchEvents` pushes, falls back to
//! a 15s poll when the stream is unavailable, applies whatever [`Action`] a
//! shell forwards from a click, and stops once that action is
//! [`Action::Quit`]. Ports the Go tray's `watch`/`poll`/`render` methods
//! (`go-client/cmd/penguin-tray/main.go`) into one self-contained task.
//!
//! A shell never talks to the daemon directly — it only reads [`Menu`]
//! snapshots off the channel [`spawn`] (or [`run`]) returns and writes
//! clicked [`Action`]s onto the other one. That split is what lets
//! `tray_native` run this loop on a background OS thread while the OS main
//! thread stays free for `tao`/`tray-icon`.

use std::collections::HashMap;
use std::time::Duration;

use penguin_proto::daemon::v1 as pb;
use penguin_proto::daemon::v1::daemon_client::DaemonClient;
use penguin_tray_model::{Action, DaemonConnection, Menu, build_menu};
use tokio::sync::mpsc;
use tonic::Streaming;
use tonic::transport::Channel;

use crate::connection::API_VERSION;
use crate::snapshot::fetch_snapshot;

/// How often to re-render when the `WatchEvents` stream is unavailable.
/// Matches the Go tray's `poll` ticker.
const POLL_INTERVAL: Duration = Duration::from_secs(15);
/// Deadline for any single RPC the loop issues (snapshot fetch, watch
/// subscribe, or an action's own call).
const RPC_TIMEOUT: Duration = Duration::from_secs(5);
/// Backlog for the outgoing menu channel — a shell only ever cares about the
/// newest [`Menu`], so this stays small on purpose.
const MENU_CHANNEL_CAPACITY: usize = 4;

/// Starts the loop as a background task, returning the channel pair a shell
/// drives it through. A convenience wrapper over [`run`] for shells (Linux's
/// ksni) that have no reason to build the channels themselves.
pub fn spawn(
    client: DaemonClient<Channel>,
) -> (mpsc::Receiver<Menu>, mpsc::UnboundedSender<Action>) {
    let (menu_tx, menu_rx) = mpsc::channel(MENU_CHANNEL_CAPACITY);
    let (action_tx, action_rx) = mpsc::unbounded_channel();
    tokio::spawn(run(client, menu_tx, action_rx));
    (menu_rx, action_tx)
}

/// Drives the loop until `action_rx` yields [`Action::Quit`] or is closed
/// (the shell went away). Exposed directly — rather than only via [`spawn`]
/// — for the native (macOS/Windows) shell, which must create its action
/// channel on the OS main thread *before* the background thread (and thus
/// this loop) even exists; see `tray_native`'s module doc.
pub async fn run(
    mut client: DaemonClient<Channel>,
    menu_tx: mpsc::Sender<Menu>,
    mut action_rx: mpsc::UnboundedReceiver<Action>,
) {
    render(&mut client, &menu_tx).await;
    let mut events = subscribe(&mut client).await;

    // `interval_at`, not `interval`: `interval`'s first tick fires
    // immediately, which would double-render right at startup whenever the
    // initial `WatchEvents` subscribe itself fails. `interval_at` instead
    // matches Go's `time.NewTicker`, whose first fire is one full
    // `POLL_INTERVAL` away.
    let mut poll =
        tokio::time::interval_at(tokio::time::Instant::now() + POLL_INTERVAL, POLL_INTERVAL);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            outcome = next_event(&mut events) => match outcome {
                StreamOutcome::Received => render(&mut client, &menu_tx).await,
                StreamOutcome::Ended | StreamOutcome::Failed => events = None,
            },
            _ = poll.tick(), if events.is_none() => render(&mut client, &menu_tx).await,
            action = action_rx.recv() => match action {
                None | Some(Action::Quit) => return,
                Some(action) => {
                    apply_action(&mut client, action_to_request(&action)).await;
                    render(&mut client, &menu_tx).await;
                }
            },
        }
    }
}

/// Re-fetches a snapshot and publishes the [`Menu`] `build_menu` derives
/// from it. An RPC failure (or timeout) degrades to
/// [`DaemonConnection::Unreachable`] rather than leaving the previous menu
/// stale with no indication anything is wrong — matching
/// [`penguin_tray_model::build_menu`]'s own "always renderable" contract.
async fn render(client: &mut DaemonClient<Channel>, menu_tx: &mpsc::Sender<Menu>) {
    let connection = match tokio::time::timeout(RPC_TIMEOUT, fetch_snapshot(client)).await {
        Ok(Ok(modules)) => DaemonConnection::Connected { modules },
        Ok(Err(status)) => DaemonConnection::Unreachable {
            reason: status.message().to_string(),
        },
        Err(_elapsed) => DaemonConnection::Unreachable {
            reason: "request timed out".to_string(),
        },
    };
    let _ = menu_tx.send(build_menu(&connection)).await;
}

/// Subscribes to `WatchEvents`, returning `None` if the subscribe call
/// itself fails or times out — the caller falls back to polling either way.
async fn subscribe(client: &mut DaemonClient<Channel>) -> Option<Streaming<pb::Event>> {
    let request = pb::WatchEventsRequest {
        api_version: API_VERSION.to_string(),
        module: String::new(),
    };
    match tokio::time::timeout(RPC_TIMEOUT, client.watch_events(request)).await {
        Ok(Ok(response)) => Some(response.into_inner()),
        Ok(Err(_)) | Err(_) => None,
    }
}

/// What the next `WatchEvents` message meant for [`run`]'s loop.
enum StreamOutcome {
    /// An event arrived; the caller should re-render.
    Received,
    /// The stream ended cleanly (the daemon closed it).
    Ended,
    /// The stream failed (a transport error).
    Failed,
}

/// Awaits the next stream message, or never resolves if there is no stream
/// to read from — letting `tokio::select!`'s `if events.is_none()` guard on
/// the poll-ticker branch in [`run`] do the actual fallback selection.
async fn next_event(stream: &mut Option<Streaming<pb::Event>>) -> StreamOutcome {
    let Some(inner) = stream else {
        std::future::pending::<()>().await;
        unreachable!("a pending future never resolves");
    };
    match inner.message().await {
        Ok(Some(_event)) => StreamOutcome::Received,
        Ok(None) => StreamOutcome::Ended,
        Err(_status) => StreamOutcome::Failed,
    }
}

/// What issuing an [`Action`] against the daemon means, as a request the
/// loop can send. Split out from [`apply_action`] so the mapping itself —
/// "this click means this RPC" — is a pure function a test can check
/// without a live client.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionRequest {
    /// Run a module's `tray: true` command.
    Dispatch(pb::DispatchRequest),
    /// Load a disabled module.
    LoadModule(pb::LoadModuleRequest),
    /// Unload a loaded module.
    UnloadModule(pb::UnloadModuleRequest),
    /// Re-render from a fresh snapshot; no RPC of its own.
    Refresh,
    /// Stop the loop; no RPC of its own.
    Quit,
}

/// Maps a clicked [`Action`] to the request that expresses it, stamping the
/// shared [`API_VERSION`] on every RPC-bearing variant.
pub fn action_to_request(action: &Action) -> ActionRequest {
    match action {
        Action::Dispatch { module, path } => ActionRequest::Dispatch(pb::DispatchRequest {
            api_version: API_VERSION.to_string(),
            module: module.clone(),
            path: path.clone(),
            flags: HashMap::new(),
            args: Vec::new(),
        }),
        Action::LoadModule { module } => ActionRequest::LoadModule(pb::LoadModuleRequest {
            api_version: API_VERSION.to_string(),
            name: module.clone(),
        }),
        Action::UnloadModule { module } => ActionRequest::UnloadModule(pb::UnloadModuleRequest {
            api_version: API_VERSION.to_string(),
            name: module.clone(),
        }),
        Action::Refresh => ActionRequest::Refresh,
        Action::Quit => ActionRequest::Quit,
    }
}

/// Issues whichever RPC `request` names. `Dispatch`'s response stream is
/// drained and discarded — a tray click has nowhere to show command output,
/// unlike the CLI's own `dispatch` handler — and every RPC's error, if any,
/// is left for the next [`render`] call to surface as an updated (or
/// unreachable) menu rather than handled here. [`ActionRequest::Quit`] is
/// unreachable in practice (`run` returns before calling this for
/// `Action::Quit`) but handled anyway so the match stays exhaustive with no
/// catch-all arm.
async fn apply_action(client: &mut DaemonClient<Channel>, request: ActionRequest) {
    match request {
        ActionRequest::Dispatch(request) => drain_dispatch(client, request).await,
        ActionRequest::LoadModule(request) => {
            let _ = tokio::time::timeout(RPC_TIMEOUT, client.load_module(request)).await;
        }
        ActionRequest::UnloadModule(request) => {
            let _ = tokio::time::timeout(RPC_TIMEOUT, client.unload_module(request)).await;
        }
        ActionRequest::Refresh | ActionRequest::Quit => {}
    }
}

/// Drives a `Dispatch` stream to completion, discarding every chunk — see
/// [`apply_action`]'s doc for why a tray click has nothing to show it.
async fn drain_dispatch(client: &mut DaemonClient<Channel>, request: pb::DispatchRequest) {
    let Ok(Ok(response)) = tokio::time::timeout(RPC_TIMEOUT, client.dispatch(request)).await else {
        return;
    };
    let mut stream = response.into_inner();
    while let Ok(Some(_chunk)) = stream.message().await {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_action_becomes_a_dispatch_request_with_the_shared_api_version() {
        let action = Action::Dispatch {
            module: "squawk".to_string(),
            path: vec!["forward".to_string(), "start".to_string()],
        };

        let ActionRequest::Dispatch(request) = action_to_request(&action) else {
            panic!("expected ActionRequest::Dispatch");
        };
        assert_eq!(request.api_version, API_VERSION);
        assert_eq!(request.module, "squawk");
        assert_eq!(
            request.path,
            vec!["forward".to_string(), "start".to_string()]
        );
        assert!(request.flags.is_empty());
        assert!(request.args.is_empty());
    }

    #[test]
    fn load_module_action_becomes_a_load_module_request() {
        let action = Action::LoadModule {
            module: "tobogganing".to_string(),
        };

        let ActionRequest::LoadModule(request) = action_to_request(&action) else {
            panic!("expected ActionRequest::LoadModule");
        };
        assert_eq!(request.api_version, API_VERSION);
        assert_eq!(request.name, "tobogganing");
    }

    #[test]
    fn unload_module_action_becomes_an_unload_module_request() {
        let action = Action::UnloadModule {
            module: "tobogganing".to_string(),
        };

        let ActionRequest::UnloadModule(request) = action_to_request(&action) else {
            panic!("expected ActionRequest::UnloadModule");
        };
        assert_eq!(request.api_version, API_VERSION);
        assert_eq!(request.name, "tobogganing");
    }

    #[test]
    fn refresh_and_quit_carry_no_rpc_request() {
        assert_eq!(action_to_request(&Action::Refresh), ActionRequest::Refresh);
        assert_eq!(action_to_request(&Action::Quit), ActionRequest::Quit);
    }
}
