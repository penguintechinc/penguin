//! Linux system-tray shell: renders the model's [`Menu`] as a
//! KDE/freedesktop StatusNotifierItem via `ksni`, and turns a clicked row's
//! [`Action`] back into whatever [`daemon_loop`] needs to run it.
//!
//! `ksni`'s `Tray` runs entirely inside a Tokio task via `TrayMethods::spawn`
//! — it needs no OS main thread. That is *why* this is the platform shell
//! Linux CI actually builds and tests (see `tray_native`'s module doc for
//! the main-thread constraint the other two platforms carry instead).

use std::process::ExitCode;

use ksni::TrayMethods;
use penguin_proto::daemon::v1::daemon_client::DaemonClient;
use penguin_tray_model::{Action, DaemonConnection, Menu, MenuItem, Severity, build_menu};
use tokio::sync::mpsc::UnboundedSender;
use tonic::transport::Channel;

use crate::daemon_loop;
use crate::label::render_label;

/// The tray's own persistent state: the most recently rendered [`Menu`] plus
/// where to report a click. `ksni` calls `Tray::menu` fresh every time the
/// desktop environment asks for the current menu, so no separate dirty flag
/// or caching is needed here.
struct AppTray {
    menu: Menu,
    action_tx: UnboundedSender<Action>,
}

impl ksni::Tray for AppTray {
    fn id(&self) -> String {
        "penguin-tray".to_string()
    }

    fn title(&self) -> String {
        format!("{} — {}", self.menu.header.label, self.menu.header.detail)
    }

    fn icon_name(&self) -> String {
        severity_icon_name(self.menu.header.severity).to_string()
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        build_ksni_menu(&self.menu, &self.action_tx)
    }
}

/// Maps a [`Severity`] to a freedesktop icon-naming-spec name, so the tray
/// icon itself — not just each row's label — reflects overall health.
fn severity_icon_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Ok => "emblem-default",
        Severity::Warn => "dialog-warning",
        Severity::Bad => "dialog-error",
        Severity::Unknown => "dialog-question",
    }
}

/// Renders the whole [`Menu`] tree (header, modules, footer) into `ksni`'s
/// flat `Vec<MenuItem<AppTray>>`, with a separator ahead of each section
/// that actually has rows.
fn build_ksni_menu(
    menu: &Menu,
    action_tx: &UnboundedSender<Action>,
) -> Vec<ksni::MenuItem<AppTray>> {
    let mut items = vec![label_item(&menu.header)];
    if !menu.modules.is_empty() {
        items.push(ksni::MenuItem::Separator);
        for module in &menu.modules {
            items.push(convert_item(module, action_tx));
        }
    }
    if !menu.footer.is_empty() {
        items.push(ksni::MenuItem::Separator);
        for footer in &menu.footer {
            items.push(convert_item(footer, action_tx));
        }
    }
    items
}

/// Converts one model [`MenuItem`] into a `ksni` item, recursing into
/// `children`. A node with children becomes a `SubMenu`; a node with an
/// `action` and no children becomes a clickable leaf. A node with *both* (a
/// `tray: true` command that itself has `tray: true` subcommands — see
/// `penguin_tray_model::menu`'s own doc) becomes a `SubMenu` whose first
/// entry repeats the parent's own action: `ksni::menu::SubMenu` has no
/// `activate` of its own, so this is the only way to keep that action
/// reachable at all.
fn convert_item(item: &MenuItem, action_tx: &UnboundedSender<Action>) -> ksni::MenuItem<AppTray> {
    if item.children.is_empty() {
        return leaf_item(item, action_tx);
    }

    let mut submenu = Vec::with_capacity(item.children.len() + 1);
    if item.action.is_some() {
        submenu.push(leaf_item(item, action_tx));
        submenu.push(ksni::MenuItem::Separator);
    }
    for child in &item.children {
        submenu.push(convert_item(child, action_tx));
    }

    ksni::menu::SubMenu {
        label: render_label(item),
        submenu,
        ..Default::default()
    }
    .into()
}

/// Builds a childless, clickable `StandardItem` for `item`, wiring its
/// `activate` callback to forward `item.action` onto `action_tx`, or a
/// disabled one if `item` has no action at all.
///
/// `activate` must be synchronous and infallible, so a full channel is
/// treated as "nothing to do" rather than blocking the desktop
/// environment's menu — [`daemon_loop`]'s action channel is unbounded, so
/// this is not expected to ever actually happen.
fn leaf_item(item: &MenuItem, action_tx: &UnboundedSender<Action>) -> ksni::MenuItem<AppTray> {
    let label = render_label(item);
    let Some(action) = item.action.clone() else {
        return ksni::menu::StandardItem {
            label,
            enabled: false,
            ..Default::default()
        }
        .into();
    };

    let action_tx = action_tx.clone();
    ksni::menu::StandardItem {
        label,
        enabled: true,
        activate: Box::new(move |_tray: &mut AppTray| {
            let _ = action_tx.send(action.clone());
        }),
        ..Default::default()
    }
    .into()
}

/// Builds a non-clickable, disabled label row for text-only items — today,
/// only the menu's own header.
fn label_item(item: &MenuItem) -> ksni::MenuItem<AppTray> {
    ksni::menu::StandardItem {
        label: render_label(item),
        enabled: false,
        ..Default::default()
    }
    .into()
}

/// Runs the Linux tray shell to completion: spawns [`daemon_loop`], starts
/// the `ksni` service with a placeholder "connecting" menu, then republishes
/// every [`Menu`] the loop produces via `Handle::update` until the loop
/// exits — which happens exactly when its action channel yields
/// [`Action::Quit`] (or is dropped), closing the menu channel this loop
/// reads from.
pub async fn run(client: DaemonClient<Channel>) -> ExitCode {
    let (mut menu_rx, action_tx) = daemon_loop::spawn(client);

    let placeholder = build_menu(&DaemonConnection::Unreachable {
        reason: "connecting…".to_string(),
    });
    let tray = AppTray {
        menu: placeholder,
        action_tx,
    };
    let handle = match tray.spawn().await {
        Ok(handle) => handle,
        Err(err) => {
            eprintln!("penguin-tray: cannot start the StatusNotifierItem service: {err}");
            return ExitCode::FAILURE;
        }
    };

    while let Some(menu) = menu_rx.recv().await {
        handle.update(|tray: &mut AppTray| tray.menu = menu).await;
    }
    handle.shutdown().await;
    ExitCode::SUCCESS
}
