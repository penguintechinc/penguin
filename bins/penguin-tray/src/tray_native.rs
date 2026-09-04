//! macOS/Windows tray shell: `tray-icon` (using its own re-export of `muda`
//! for menu construction) driven by a `tao` event loop.
//!
//! # Thread ownership — read this before touching this file
//!
//! `tray-icon`/`muda` require their objects to be created, and their events
//! pumped, on the OS main thread: mandatory on macOS, and effectively so on
//! Windows too per `tray-icon`'s own docs ("an event loop must be running on
//! the thread"). `tao::EventLoop` is what pumps that thread, and
//! `EventLoop::run` never returns — it blocks its calling thread for the
//! rest of the process, dispatching OS messages until `ControlFlow::Exit`
//! tears the process down.
//!
//! That means the thread [`run`] is called on — which [`crate::main`]
//! arranges to be the real OS main thread — is reserved for
//! `tao`/`tray-icon`/`muda` alone, for good. Everything that talks to the
//! daemon (connecting, [`daemon_loop::run`], the `WatchEvents` stream)
//! instead runs on a second, dedicated OS thread with its own Tokio
//! runtime, started by [`run`] and driven by [`run_background`]. Unlike
//! `tray_linux`, there is no way to run this loop "inside a Tokio task" on
//! the main thread — the main thread never enters Tokio at all.
//!
//! The two threads talk over two channels, both plain data with no shared
//! locking:
//! - an [`tao::event_loop::EventLoopProxy`] carries [`Menu`] updates (and
//!   native click events, forwarded from `tray-icon`/`muda`'s own global
//!   handlers) from the background thread *and* from those handlers into
//!   tao's loop as [`UserEvent`]s;
//! - a `tokio::sync::mpsc` unbounded channel carries clicked [`Action`]s the
//!   other way. Its sender is created on the main thread *before* the
//!   background thread starts, specifically so menu-click callbacks can be
//!   wired to it immediately — without waiting on a daemon connection that,
//!   at that point, has not even begun.

use std::collections::HashMap;
use std::process::ExitCode;

use penguin_tray_model::{Action, DaemonConnection, Menu, MenuItem as ModelMenuItem, build_menu};
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tokio::sync::mpsc;
use tray_icon::menu::{IsMenuItem, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::connection;
use crate::daemon_loop;
use crate::label::render_label;

/// Events tao's loop reacts to: either a fresh [`Menu`] (or an unreachable
/// reason) produced on the background thread, a menu-row click forwarded
/// from `muda`'s global handler, or a signal that the background thread
/// itself has ended.
enum UserEvent {
    Menu(Menu),
    Unreachable(String),
    MenuClicked(MenuId),
    BackgroundGone,
}

/// Runs the native tray shell to completion. See this module's doc for the
/// thread-ownership rules this function sets up and never violates itself.
pub fn run(socket_path: String) -> ExitCode {
    let event_loop: EventLoop<UserEvent> = EventLoopBuilder::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // Created here, before the background thread exists, so the click
    // handlers registered by `install_native_event_forwarding` below (which
    // must themselves be registered before `event_loop.run` takes over)
    // have somewhere to forward a click regardless of whether the daemon
    // connection the receiving end depends on has been established yet.
    let (action_tx, action_rx) = mpsc::unbounded_channel::<Action>();

    install_native_event_forwarding(&proxy);

    let background_proxy = proxy.clone();
    let spawned = std::thread::Builder::new()
        .name("penguin-tray-daemon".to_string())
        .spawn(move || run_background(socket_path, background_proxy, action_rx));
    if let Err(err) = spawned {
        eprintln!("penguin-tray: cannot start the daemon-connection thread: {err}");
        return ExitCode::FAILURE;
    }

    let mut state = NativeTrayState::new(action_tx);
    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;
        let Event::UserEvent(user_event) = event else {
            return;
        };
        match user_event {
            UserEvent::Menu(menu) => state.apply(menu),
            UserEvent::Unreachable(reason) => {
                state.apply(build_menu(&DaemonConnection::Unreachable { reason }));
            }
            UserEvent::MenuClicked(id) => {
                if state.dispatch(&id) {
                    *control_flow = ControlFlow::Exit;
                }
            }
            UserEvent::BackgroundGone => *control_flow = ControlFlow::Exit,
        }
    })
    // `tao::EventLoop::run` never returns on any platform it supports —
    // this call diverges, which is why nothing follows it. The function
    // still declares `-> ExitCode` (rather than `-> !`) because a divergent
    // tail expression coerces to any type, and an ordinary return type
    // reads more clearly at this function's call site in `main`.
}

/// Registers `tray-icon`'s and `muda`'s global event handlers so their
/// clicks are forwarded into tao's loop instead of needing to be polled.
/// Must run on the main thread, before [`run`] hands that thread over to
/// `event_loop.run`.
fn install_native_event_forwarding(proxy: &EventLoopProxy<UserEvent>) {
    let menu_proxy = proxy.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = menu_proxy.send_event(UserEvent::MenuClicked(event.id));
    }));
    // Tray-icon-body clicks (as opposed to menu-item clicks) have no
    // `Action` of their own in this model — every action comes from a menu
    // row — so this handler exists only to keep tray-icon's own global
    // event channel drained; nothing is forwarded from it.
    TrayIconEvent::set_event_handler(Some(|_event: TrayIconEvent| {}));
}

/// Connects to the daemon and runs [`daemon_loop::run`] on a fresh Tokio
/// runtime, forwarding every [`Menu`] it produces to the main thread via
/// `proxy`. Runs entirely off the OS main thread — see this module's doc.
fn run_background(
    socket_path: String,
    proxy: EventLoopProxy<UserEvent>,
    action_rx: mpsc::UnboundedReceiver<Action>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            let _ = proxy.send_event(UserEvent::Unreachable(format!(
                "cannot start async runtime: {err}"
            )));
            let _ = proxy.send_event(UserEvent::BackgroundGone);
            return;
        }
    };

    runtime.block_on(async move {
        let client = match connection::connect(&socket_path).await {
            Ok(client) => client,
            Err(err) => {
                let _ = proxy.send_event(UserEvent::Unreachable(err.to_string()));
                let _ = proxy.send_event(UserEvent::BackgroundGone);
                return;
            }
        };

        let (menu_tx, mut menu_rx) = mpsc::channel(4);
        let loop_task = tokio::spawn(daemon_loop::run(client, menu_tx, action_rx));

        while let Some(menu) = menu_rx.recv().await {
            if proxy.send_event(UserEvent::Menu(menu)).is_err() {
                break; // main thread is gone; nothing left to forward to.
            }
        }
        let _ = loop_task.await;
        let _ = proxy.send_event(UserEvent::BackgroundGone);
    });
}

/// The main thread's own tray state: the live `TrayIcon` (created lazily on
/// the first [`Menu`], since none exists at startup), the [`Action`] for
/// every currently clickable `MenuId`, and where to forward a click.
struct NativeTrayState {
    tray_icon: Option<TrayIcon>,
    action_by_id: HashMap<MenuId, Action>,
    action_tx: mpsc::UnboundedSender<Action>,
}

impl NativeTrayState {
    fn new(action_tx: mpsc::UnboundedSender<Action>) -> NativeTrayState {
        NativeTrayState {
            tray_icon: None,
            action_by_id: HashMap::new(),
            action_tx,
        }
    }

    /// Rebuilds the native menu tree from a fresh [`Menu`] and either
    /// updates the existing `TrayIcon` or creates it on first use.
    fn apply(&mut self, menu: Menu) {
        self.action_by_id.clear();
        let native_menu = build_native_menu(&menu, &mut self.action_by_id);
        let tooltip = format!("{} — {}", menu.header.label, menu.header.detail);

        if let Some(tray_icon) = &self.tray_icon {
            if let Err(err) = tray_icon.set_menu(Some(Box::new(native_menu))) {
                eprintln!("penguin-tray: cannot update the tray menu: {err}");
            }
            if let Err(err) = tray_icon.set_tooltip(Some(&tooltip)) {
                eprintln!("penguin-tray: cannot update the tray tooltip: {err}");
            }
            return;
        }

        match TrayIconBuilder::new()
            .with_menu(Box::new(native_menu))
            .with_tooltip(tooltip)
            .with_icon(placeholder_icon())
            .build()
        {
            Ok(tray_icon) => self.tray_icon = Some(tray_icon),
            Err(err) => eprintln!("penguin-tray: cannot create the tray icon: {err}"),
        }
    }

    /// Looks up and forwards the [`Action`] for a clicked `MenuId`, if any
    /// is registered. Returns whether the event loop should exit — true iff
    /// the action was [`Action::Quit`].
    fn dispatch(&self, id: &MenuId) -> bool {
        let Some(action) = self.action_by_id.get(id) else {
            return false;
        };
        let is_quit = matches!(action, Action::Quit);
        let _ = self.action_tx.send(action.clone());
        is_quit
    }
}

/// A minimal solid-color placeholder icon, good enough to satisfy
/// `TrayIconBuilder::build`'s icon requirement on platforms that need one.
/// Real per-severity icon assets are a separate, later concern (icon/asset
/// work, not this milestone's platform-shell structure) — this exists
/// purely so the tray icon has *something* to show.
fn placeholder_icon() -> Icon {
    const SIZE: u32 = 8;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _ in 0..(SIZE * SIZE) {
        rgba.extend_from_slice(&[0xf5, 0x9e, 0x0b, 0xff]); // opaque amber
    }
    Icon::from_rgba(rgba, SIZE, SIZE).expect("placeholder icon dimensions are always valid")
}

/// Converts the whole model [`Menu`] into a native `tray_icon::menu::Menu`,
/// recording every clickable row's freshly minted `MenuId` in `action_by_id`
/// so [`NativeTrayState::dispatch`] can map a click back to its [`Action`].
fn build_native_menu(
    menu: &Menu,
    action_by_id: &mut HashMap<MenuId, Action>,
) -> tray_icon::menu::Menu {
    let root = tray_icon::menu::Menu::new();
    append(&root, &disabled_item(&menu.header));
    if !menu.modules.is_empty() {
        append(&root, &PredefinedMenuItem::separator());
        for module in &menu.modules {
            append(&root, convert_item(module, action_by_id).as_ref());
        }
    }
    if !menu.footer.is_empty() {
        append(&root, &PredefinedMenuItem::separator());
        for footer in &menu.footer {
            append(&root, convert_item(footer, action_by_id).as_ref());
        }
    }
    root
}

/// Appends one item to the root menu, logging (never panicking) on the rare
/// platform-level failure `muda` itself can return.
fn append(root: &tray_icon::menu::Menu, item: &dyn IsMenuItem) {
    if let Err(err) = root.append(item) {
        eprintln!("penguin-tray: cannot append a menu item: {err}");
    }
}

/// Converts one model [`ModelMenuItem`] into a native menu item, recursing
/// into `children`. A node with children becomes a `Submenu`; a node with
/// an `action` and no children becomes a clickable leaf. A node with *both*
/// (a `tray: true` command that itself has `tray: true` subcommands) gets
/// its own action duplicated as the submenu's first entry, followed by a
/// separator — `Submenu` itself carries no click of its own, mirroring
/// `tray_linux::convert_item`'s identical problem and identical fix.
fn convert_item(
    item: &ModelMenuItem,
    action_by_id: &mut HashMap<MenuId, Action>,
) -> Box<dyn IsMenuItem> {
    if item.children.is_empty() {
        return Box::new(clickable_or_disabled_item(item, action_by_id));
    }

    let submenu = Submenu::new(render_label(item), true);
    if item.action.is_some() {
        if let Err(err) = submenu.append(&clickable_or_disabled_item(item, action_by_id)) {
            eprintln!("penguin-tray: cannot append a submenu's own action: {err}");
        }
        if let Err(err) = submenu.append(&PredefinedMenuItem::separator()) {
            eprintln!("penguin-tray: cannot append a submenu separator: {err}");
        }
    }
    for child in &item.children {
        if let Err(err) = submenu.append(convert_item(child, action_by_id).as_ref()) {
            eprintln!("penguin-tray: cannot append a submenu child: {err}");
        }
    }
    Box::new(submenu)
}

/// Builds a clickable leaf for `item`, recording its `Action` in
/// `action_by_id`, or a disabled leaf if `item` has no action.
fn clickable_or_disabled_item(
    item: &ModelMenuItem,
    action_by_id: &mut HashMap<MenuId, Action>,
) -> MenuItem {
    let label = render_label(item);
    let Some(action) = &item.action else {
        return MenuItem::new(label, false, None);
    };
    let menu_item = MenuItem::new(label, true, None);
    action_by_id.insert(menu_item.id().clone(), action.clone());
    menu_item
}

/// Builds a non-clickable, disabled label row — today, only the menu's own
/// header.
fn disabled_item(item: &ModelMenuItem) -> MenuItem {
    MenuItem::new(render_label(item), false, None)
}
