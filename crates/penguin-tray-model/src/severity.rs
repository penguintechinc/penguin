//! The tray's single severity scale, used for both the icon a shell paints
//! next to a row and for reducing many rows down to one overall indicator.

/// How urgently a tray row (or the whole menu) should draw the user's eye.
///
/// Ordered worst-to-best is `Bad > Warn > Unknown > Ok`: an unprobed module is
/// treated as worse than a confirmed-healthy one, matching the Go tray's
/// `rank` table, so a daemon that has not reported yet never reads as "all
/// good" by omission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Severity {
    /// No signal yet — not probed, not applicable, or not yet reported.
    #[default]
    Unknown,
    /// Fully operational.
    Ok,
    /// Operational with reduced function.
    Warn,
    /// Not operational, or unreachable.
    Bad,
}

impl Severity {
    /// Returns this severity's rank on the worst-to-best scale described on
    /// the type, used by [`worse`] to compare two severities.
    fn rank(self) -> u8 {
        match self {
            Severity::Ok => 0,
            Severity::Unknown => 1,
            Severity::Warn => 2,
            Severity::Bad => 3,
        }
    }

    /// A short glyph a shell can prefix onto a label without implementing its
    /// own severity-to-icon mapping.
    pub fn icon(self) -> &'static str {
        match self {
            Severity::Unknown => "\u{2022}", // •
            Severity::Ok => "\u{25cf}",      // ●
            Severity::Warn => "\u{25b2}",    // ▲
            Severity::Bad => "\u{2716}",     // ✖
        }
    }

    /// A short word for the severity, used in status-summary text.
    pub fn label(self) -> &'static str {
        match self {
            Severity::Unknown => "Unknown",
            Severity::Ok => "OK",
            Severity::Warn => "Warning",
            Severity::Bad => "Critical",
        }
    }
}

/// Returns the worse (more urgent) of two severities, per [`Severity`]'s
/// ranking. Used to reduce a list of rows to one overall indicator.
pub fn worse(a: Severity, b: Severity) -> Severity {
    if b.rank() > a.rank() { b } else { a }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worse_picks_the_higher_ranked_severity() {
        assert_eq!(worse(Severity::Ok, Severity::Warn), Severity::Warn);
        assert_eq!(worse(Severity::Bad, Severity::Ok), Severity::Bad);
        assert_eq!(worse(Severity::Unknown, Severity::Ok), Severity::Unknown);
        assert_eq!(worse(Severity::Warn, Severity::Unknown), Severity::Warn);
    }

    #[test]
    fn worse_is_reflexive_for_equal_inputs() {
        for level in [
            Severity::Unknown,
            Severity::Ok,
            Severity::Warn,
            Severity::Bad,
        ] {
            assert_eq!(worse(level, level), level);
        }
    }

    #[test]
    fn default_severity_is_unknown() {
        assert_eq!(Severity::default(), Severity::Unknown);
    }

    #[test]
    fn every_severity_has_a_distinct_icon_and_label() {
        let all = [
            Severity::Unknown,
            Severity::Ok,
            Severity::Warn,
            Severity::Bad,
        ];
        for a in all {
            for b in all {
                if a != b {
                    assert_ne!(a.icon(), b.icon());
                    assert_ne!(a.label(), b.label());
                }
            }
        }
    }
}
