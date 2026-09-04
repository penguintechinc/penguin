//! Pure decision logic for `penguin update`, split out from the actual
//! stdin prompt so the branching (ported from `cmdUpdate`'s `RunE` in
//! `go-client/cmd/penguin/main.go`) is testable without a terminal.

/// What `penguin update` should do next, once `CheckUpdate` has answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateAction {
    /// `CheckUpdate` reported nothing newer — print
    /// [`crate::render::NO_UPDATES_AVAILABLE`] and stop.
    NoUpdateAvailable,
    /// An update exists and `--yes` was passed — call `ApplyUpdate`
    /// immediately, no prompt.
    Apply,
    /// An update exists and `--yes` was not passed — print
    /// [`crate::render::UPDATE_CONFIRM_PROMPT`] and read an answer.
    Confirm,
}

/// Decides the next step after `CheckUpdate`, mirroring
/// `if !checkResp.Available { ...; return nil }` followed by `if !yes { ...
/// prompt ... }` in `cmdUpdate`.
pub fn decide_update(available: bool, yes: bool) -> UpdateAction {
    if !available {
        return UpdateAction::NoUpdateAvailable;
    }
    if yes {
        return UpdateAction::Apply;
    }
    UpdateAction::Confirm
}

/// Interprets a line read in response to [`UpdateAction::Confirm`]'s prompt.
/// Matches `answer != "y" && answer != "yes"` — Go's `fmt.Scanln` splits on
/// whitespace and drops the trailing newline itself, so trimming here
/// reproduces the same effective comparison for a line read with a newline
/// still attached.
pub fn confirm_answer(answer: &str) -> bool {
    let trimmed = answer.trim();
    trimmed == "y" || trimmed == "yes"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_update_available_wins_regardless_of_yes() {
        assert_eq!(decide_update(false, false), UpdateAction::NoUpdateAvailable);
        assert_eq!(decide_update(false, true), UpdateAction::NoUpdateAvailable);
    }

    #[test]
    fn available_with_yes_applies_without_confirmation() {
        assert_eq!(decide_update(true, true), UpdateAction::Apply);
    }

    #[test]
    fn available_without_yes_asks_for_confirmation() {
        assert_eq!(decide_update(true, false), UpdateAction::Confirm);
    }

    #[test]
    fn confirm_answer_accepts_y_and_yes() {
        assert!(confirm_answer("y"));
        assert!(confirm_answer("yes"));
        assert!(confirm_answer("y\n"));
        assert!(confirm_answer("yes\n"));
    }

    #[test]
    fn confirm_answer_rejects_anything_else() {
        assert!(!confirm_answer("n"));
        assert!(!confirm_answer("no"));
        assert!(!confirm_answer(""));
        assert!(!confirm_answer("Y"));
        assert!(!confirm_answer("YES"));
    }
}
