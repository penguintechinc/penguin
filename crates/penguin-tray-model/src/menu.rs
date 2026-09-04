//! Builds the tray's menu tree from daemon state. This is the crate's single
//! entry point — everything else here is a helper `build_menu` calls.
//!
//! `build_menu` never fails and never panics: an unreachable daemon or an
//! empty module list both produce a small but complete [`Menu`], because a
//! system tray has nowhere to show an error page and no user to retry a
//! panic.

use std::cmp::Ordering;

use penguin_sdk::CommandSpec;

use crate::action::Action;
use crate::module::{self, ModuleInput};
use crate::severity::{Severity, worse};

/// One row in the tray menu tree.
///
/// A row with `action: None` and no `children` is a plain label (the
/// header); a row with `children` but no `action` is a submenu (a module); a
/// row with `action: Some(_)` is clickable, and may itself have children
/// (a `tray: true` command that also has `tray: true` subcommands).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    /// The row's visible text.
    pub label: String,
    /// Secondary text (state, health, a command's usage line); empty when
    /// there is nothing to add beyond the label.
    pub detail: String,
    /// The severity a shell paints this row's icon with.
    pub severity: Severity,
    /// What activating this row does, or `None` if it is not clickable.
    pub action: Option<Action>,
    /// Nested rows (a module's actions, or a command's tray subcommands).
    pub children: Vec<MenuItem>,
}

/// The full menu snapshot a shell renders: a status header, one row per
/// module (each a submenu of that module's actions), and global actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    /// A non-clickable summary row: module count and overall severity, or the
    /// unreachable-daemon message.
    pub header: MenuItem,
    /// One row per module, sorted by name, empty when there are none.
    pub modules: Vec<MenuItem>,
    /// Global actions available regardless of daemon reachability
    /// (currently: Refresh, Quit) — always non-empty, so the menu is always
    /// usable even when [`modules`](Menu::modules) is not.
    pub footer: Vec<MenuItem>,
}

/// Whether the tray can currently reach the daemon, and if so, what it
/// reported. The shell constructs this after calling `ListModules`,
/// `GetStatus`, and `ListCommands` (joining their responses per module by
/// name) or after failing to reach the daemon at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonConnection {
    /// The daemon answered; here is what it reported.
    Connected { modules: Vec<ModuleInput> },
    /// The daemon could not be reached. `reason` is a short, user-facing
    /// explanation (e.g. `"socket not found"`); it may be empty.
    Unreachable { reason: String },
}

/// Builds the tray menu for the current daemon connection state. Pure: no
/// I/O, no clock, no panics — every input, including an empty module list or
/// an unreachable daemon, produces a renderable [`Menu`].
pub fn build_menu(connection: &DaemonConnection) -> Menu {
    match connection {
        DaemonConnection::Connected { modules } => build_connected_menu(modules),
        DaemonConnection::Unreachable { reason } => build_unreachable_menu(reason),
    }
}

/// Builds the menu for a reachable daemon: a header summarising the worst
/// severity among loaded modules, one row per module, and the global footer.
fn build_connected_menu(modules: &[ModuleInput]) -> Menu {
    let mut sorted: Vec<&ModuleInput> = modules.iter().collect();
    sorted.sort_by(compare_module_names);

    let mut rows: Vec<MenuItem> = Vec::with_capacity(sorted.len());
    let mut overall = Severity::Ok;
    for module in sorted {
        let row = build_module_item(module);
        if module::is_loaded(module.state) {
            overall = worse(overall, row.severity);
        }
        rows.push(row);
    }

    let header = MenuItem {
        label: "Penguin".to_string(),
        detail: format!("{} module(s) — {}", rows.len(), overall.label()),
        severity: overall,
        action: None,
        children: Vec::new(),
    };

    Menu {
        header,
        modules: rows,
        footer: footer_items(),
    }
}

/// Builds the degraded menu shown when the daemon cannot be reached: no
/// module rows, but a clear header and the same always-available footer, so
/// the user can still quit (or retry via Refresh).
fn build_unreachable_menu(reason: &str) -> Menu {
    let detail = if reason.is_empty() {
        "Daemon unreachable".to_string()
    } else {
        format!("Daemon unreachable: {reason}")
    };
    let header = MenuItem {
        label: "Penguin".to_string(),
        detail,
        severity: Severity::Bad,
        action: None,
        children: Vec::new(),
    };
    Menu {
        header,
        modules: Vec::new(),
        footer: footer_items(),
    }
}

/// Compares two modules by name, for the ascending sort each menu build
/// uses. A named comparator instead of an inline closure at the call site.
fn compare_module_names(a: &&ModuleInput, b: &&ModuleInput) -> Ordering {
    a.name.cmp(&b.name)
}

/// Builds one module's submenu row: its state/health as the label and
/// detail, a combined severity, a Load-or-Unload action, and its tray
/// commands nested underneath.
fn build_module_item(module: &ModuleInput) -> MenuItem {
    let severity = module::module_severity(module.state, module.health);

    let mut detail = format!(
        "{} · {}",
        module::state_text(module.state),
        module::module_health_text(module.health)
    );
    if !module.health_message.is_empty() {
        detail.push_str(" — ");
        detail.push_str(&module.health_message);
    }

    let mut children = Vec::with_capacity(module.commands.len() + 1);
    children.push(load_unload_item(module));
    for command in &module.commands {
        if let Some(item) = tray_item(&module.name, &[], command) {
            children.push(item);
        }
    }

    MenuItem {
        label: module.name.clone(),
        detail,
        severity,
        action: None,
        children,
    }
}

/// Builds the Load/Unload row every module gets: Unload while it is running
/// in any non-disabled state, Load while it is disabled.
fn load_unload_item(module: &ModuleInput) -> MenuItem {
    if module::is_loaded(module.state) {
        return MenuItem {
            label: "Unload".to_string(),
            detail: String::new(),
            severity: Severity::Unknown,
            action: Some(Action::UnloadModule {
                module: module.name.clone(),
            }),
            children: Vec::new(),
        };
    }
    MenuItem {
        label: "Load".to_string(),
        detail: String::new(),
        severity: Severity::Unknown,
        action: Some(Action::LoadModule {
            module: module.name.clone(),
        }),
        children: Vec::new(),
    }
}

/// Recursively turns one command (and its subcommands) into a menu row,
/// dropping branches that carry no `tray: true` command anywhere in their
/// subtree. `prefix` is the command path accumulated from ancestors; the
/// returned row's [`Action::Dispatch`] path (if any) is `prefix` plus this
/// command's own name, matching the daemon's `Dispatch` RPC contract.
fn tray_item(module_name: &str, prefix: &[String], command: &CommandSpec) -> Option<MenuItem> {
    let mut path: Vec<String> = prefix.to_vec();
    path.push(command.name.clone());

    let mut children = Vec::new();
    for sub in &command.subcommands {
        if let Some(child) = tray_item(module_name, &path, sub) {
            children.push(child);
        }
    }

    if !command.tray && children.is_empty() {
        return None;
    }

    let label = if command.short.is_empty() {
        command.name.clone()
    } else {
        command.short.clone()
    };
    let is_tray_leaf = command.tray;
    let action = if is_tray_leaf {
        Some(Action::Dispatch {
            module: module_name.to_string(),
            path,
        })
    } else {
        None
    };

    Some(MenuItem {
        label,
        detail: command.use_line.clone(),
        severity: Severity::Unknown,
        action,
        children,
    })
}

/// The global actions every menu ends with, regardless of daemon
/// reachability — the menu is never usable without at least a way to quit.
fn footer_items() -> Vec<MenuItem> {
    vec![
        MenuItem {
            label: "Refresh".to_string(),
            detail: String::new(),
            severity: Severity::Unknown,
            action: Some(Action::Refresh),
            children: Vec::new(),
        },
        MenuItem {
            label: "Quit".to_string(),
            detail: String::new(),
            severity: Severity::Unknown,
            action: Some(Action::Quit),
            children: Vec::new(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use penguin_sdk::{HealthLevel, ModuleState};

    use super::*;

    /// A representative two-module state: one healthy leaf command, one
    /// disabled module with no commands — enough to exercise sorting,
    /// header aggregation, and both load/unload branches in one build.
    fn sample_modules() -> Vec<ModuleInput> {
        vec![
            ModuleInput {
                name: "tobogganing".to_string(),
                state: ModuleState::Disabled,
                health: None,
                health_message: String::new(),
                commands: Vec::new(),
            },
            ModuleInput {
                name: "squawk".to_string(),
                state: ModuleState::Running,
                health: Some(HealthLevel::Degraded),
                health_message: "server down".to_string(),
                commands: vec![CommandSpec {
                    name: "forward".to_string(),
                    use_line: "forward".to_string(),
                    short: "Forwarding".to_string(),
                    flags: Vec::new(),
                    subcommands: vec![
                        CommandSpec {
                            name: "start".to_string(),
                            use_line: "start".to_string(),
                            short: "Start forwarding".to_string(),
                            flags: Vec::new(),
                            subcommands: Vec::new(),
                            tray: true,
                            min_args: 0,
                            max_args: 0,
                        },
                        CommandSpec {
                            name: "status".to_string(),
                            use_line: "status".to_string(),
                            short: "Show status".to_string(),
                            flags: Vec::new(),
                            subcommands: Vec::new(),
                            tray: false,
                            min_args: 0,
                            max_args: 0,
                        },
                    ],
                    tray: false,
                    min_args: 0,
                    max_args: 0,
                }],
            },
        ]
    }

    #[test]
    fn connected_menu_orders_header_then_modules_by_name_then_footer() {
        let connection = DaemonConnection::Connected {
            modules: sample_modules(),
        };
        let menu = build_menu(&connection);

        assert_eq!(menu.header.label, "Penguin");
        assert_eq!(menu.header.detail, "2 module(s) — Warning");
        assert_eq!(menu.header.severity, Severity::Warn);
        assert!(menu.header.action.is_none());

        let names: Vec<&str> = menu.modules.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(names, ["squawk", "tobogganing"]);

        let footer_labels: Vec<&str> = menu.footer.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(footer_labels, ["Refresh", "Quit"]);
        assert_eq!(menu.footer[0].action, Some(Action::Refresh));
        assert_eq!(menu.footer[1].action, Some(Action::Quit));
    }

    #[test]
    fn running_module_gets_an_unload_action_first() {
        let connection = DaemonConnection::Connected {
            modules: sample_modules(),
        };
        let menu = build_menu(&connection);
        let squawk = &menu.modules[0];

        assert_eq!(squawk.detail, "Running · Degraded — server down");
        assert_eq!(squawk.severity, Severity::Warn);
        assert_eq!(squawk.children[0].label, "Unload");
        assert_eq!(
            squawk.children[0].action,
            Some(Action::UnloadModule {
                module: "squawk".to_string()
            })
        );
    }

    #[test]
    fn disabled_module_gets_a_load_action_and_no_commands() {
        let connection = DaemonConnection::Connected {
            modules: sample_modules(),
        };
        let menu = build_menu(&connection);
        let tobogganing = &menu.modules[1];

        assert_eq!(tobogganing.detail, "Disabled · Unknown");
        assert_eq!(tobogganing.children.len(), 1, "only the Load action");
        assert_eq!(tobogganing.children[0].label, "Load");
        assert_eq!(
            tobogganing.children[0].action,
            Some(Action::LoadModule {
                module: "tobogganing".to_string()
            })
        );
    }

    #[test]
    fn only_tray_flagged_commands_become_actions() {
        let connection = DaemonConnection::Connected {
            modules: sample_modules(),
        };
        let menu = build_menu(&connection);
        let squawk = &menu.modules[0];

        // children[0] is the Load/Unload row; children[1] is the "forward"
        // container, which is not itself tray:true but has a tray:true child.
        let forward = &squawk.children[1];
        assert_eq!(forward.label, "Forwarding");
        assert!(
            forward.action.is_none(),
            "forward is a container, not tray:true"
        );
        assert_eq!(forward.children.len(), 1, "status is not tray:true");

        let start = &forward.children[0];
        assert_eq!(start.label, "Start forwarding");
        assert_eq!(
            start.action,
            Some(Action::Dispatch {
                module: "squawk".to_string(),
                path: vec!["forward".to_string(), "start".to_string()],
            })
        );
    }

    #[test]
    fn nested_non_tray_branch_with_no_tray_descendants_is_pruned() {
        let leaf = CommandSpec {
            name: "status".to_string(),
            short: "Show status".to_string(),
            tray: false,
            ..Default::default()
        };
        let branch = CommandSpec {
            name: "diag".to_string(),
            short: "Diagnostics".to_string(),
            subcommands: vec![leaf],
            tray: false,
            ..Default::default()
        };
        let module = ModuleInput {
            name: "squawk".to_string(),
            state: ModuleState::Running,
            health: Some(HealthLevel::Healthy),
            health_message: String::new(),
            commands: vec![branch],
        };
        let menu = build_menu(&DaemonConnection::Connected {
            modules: vec![module],
        });

        // Only the Load/Unload row remains — the whole diag/status branch had
        // no tray:true command anywhere in it.
        assert_eq!(menu.modules[0].children.len(), 1);
        assert_eq!(menu.modules[0].children[0].label, "Unload");
    }

    #[test]
    fn a_tray_command_can_itself_have_tray_subcommands() {
        let child = CommandSpec {
            name: "child".to_string(),
            short: "Child action".to_string(),
            tray: true,
            ..Default::default()
        };
        let parent = CommandSpec {
            name: "parent".to_string(),
            short: "Parent action".to_string(),
            subcommands: vec![child],
            tray: true,
            ..Default::default()
        };
        let module = ModuleInput {
            name: "squawk".to_string(),
            state: ModuleState::Running,
            health: Some(HealthLevel::Healthy),
            health_message: String::new(),
            commands: vec![parent],
        };
        let menu = build_menu(&DaemonConnection::Connected {
            modules: vec![module],
        });

        let parent_item = &menu.modules[0].children[1];
        assert_eq!(parent_item.label, "Parent action");
        assert_eq!(
            parent_item.action,
            Some(Action::Dispatch {
                module: "squawk".to_string(),
                path: vec!["parent".to_string()],
            })
        );
        assert_eq!(parent_item.children.len(), 1);
        let child_item = &parent_item.children[0];
        assert_eq!(child_item.label, "Child action");
        assert_eq!(
            child_item.action,
            Some(Action::Dispatch {
                module: "squawk".to_string(),
                path: vec!["parent".to_string(), "child".to_string()],
            })
        );
    }

    #[test]
    fn tray_action_falls_back_to_the_command_name_when_short_is_empty() {
        let command = CommandSpec {
            name: "start".to_string(),
            tray: true,
            ..Default::default()
        };
        let module = ModuleInput {
            name: "squawk".to_string(),
            state: ModuleState::Running,
            health: Some(HealthLevel::Healthy),
            health_message: String::new(),
            commands: vec![command],
        };
        let menu = build_menu(&DaemonConnection::Connected {
            modules: vec![module],
        });
        assert_eq!(menu.modules[0].children[1].label, "start");
    }

    #[test]
    fn empty_module_list_is_a_valid_healthy_menu() {
        let menu = build_menu(&DaemonConnection::Connected {
            modules: Vec::new(),
        });

        assert!(menu.modules.is_empty());
        assert_eq!(menu.header.detail, "0 module(s) — OK");
        assert_eq!(menu.header.severity, Severity::Ok);
        assert_eq!(menu.footer.len(), 2, "quit must still be reachable");
    }

    #[test]
    fn unreachable_daemon_produces_a_usable_degraded_menu() {
        let menu = build_menu(&DaemonConnection::Unreachable {
            reason: "socket not found".to_string(),
        });

        assert!(menu.modules.is_empty());
        assert_eq!(menu.header.severity, Severity::Bad);
        assert_eq!(menu.header.detail, "Daemon unreachable: socket not found");
        let footer_labels: Vec<&str> = menu.footer.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(footer_labels, ["Refresh", "Quit"]);
        assert_eq!(menu.footer[1].action, Some(Action::Quit));
    }

    #[test]
    fn unreachable_daemon_with_no_reason_still_has_a_message() {
        let menu = build_menu(&DaemonConnection::Unreachable {
            reason: String::new(),
        });
        assert_eq!(menu.header.detail, "Daemon unreachable");
    }

    #[test]
    fn disabled_modules_are_excluded_from_the_overall_severity() {
        let module = ModuleInput {
            name: "tobogganing".to_string(),
            state: ModuleState::Disabled,
            health: Some(HealthLevel::Unhealthy),
            health_message: String::new(),
            commands: Vec::new(),
        };
        let menu = build_menu(&DaemonConnection::Connected {
            modules: vec![module],
        });
        // A disabled module's own row still shows its (irrelevant) health,
        // but it must not drag the header down.
        assert_eq!(menu.header.severity, Severity::Ok);
    }
}
