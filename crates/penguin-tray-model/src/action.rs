//! What a tray click *means*, as inert data. The platform shell (M7) matches
//! on [`Action`] and issues the corresponding daemon RPC or process exit; it
//! never decides on its own what a click should do.

/// The effect of activating a tray menu item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Invoke a `tray: true` command on a module. `path` is the full command
    /// path from the module's command tree root (e.g. `["forward",
    /// "start"]`), exactly as the daemon's `Dispatch` RPC expects.
    Dispatch { module: String, path: Vec<String> },
    /// Load a currently-disabled module.
    LoadModule { module: String },
    /// Unload a currently-loaded module.
    UnloadModule { module: String },
    /// Rebuild the menu from a fresh daemon snapshot.
    Refresh,
    /// Exit the tray application.
    Quit,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_actions_compare_by_module_and_path() {
        let a = Action::Dispatch {
            module: "squawk".to_string(),
            path: vec!["forward".to_string(), "start".to_string()],
        };
        let b = a.clone();
        assert_eq!(a, b);

        let different_path = Action::Dispatch {
            module: "squawk".to_string(),
            path: vec!["forward".to_string(), "stop".to_string()],
        };
        assert_ne!(a, different_path);
    }

    #[test]
    fn global_actions_are_unit_like_and_distinct() {
        assert_eq!(Action::Refresh, Action::Refresh);
        assert_eq!(Action::Quit, Action::Quit);
        assert_ne!(Action::Refresh, Action::Quit);
    }
}
