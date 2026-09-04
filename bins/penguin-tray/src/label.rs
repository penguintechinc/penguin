//! Shared row-label rendering. Both platform shells (`tray_linux`,
//! `tray_native`) format a [`MenuItem`]'s visible text identically, so this
//! one function is the single place that defines it rather than two copies
//! quietly drifting apart across a `cfg` boundary neither build ever
//! compiles together.

use penguin_tray_model::MenuItem;

/// Renders one row's visible text: its severity glyph, its label, and — if
/// present — its detail, em-dash separated.
pub fn render_label(item: &MenuItem) -> String {
    if item.detail.is_empty() {
        format!("{} {}", item.severity.icon(), item.label)
    } else {
        format!("{} {} — {}", item.severity.icon(), item.label, item.detail)
    }
}

#[cfg(test)]
mod tests {
    use penguin_tray_model::Severity;

    use super::*;

    fn item(label: &str, detail: &str, severity: Severity) -> MenuItem {
        MenuItem {
            label: label.to_string(),
            detail: detail.to_string(),
            severity,
            action: None,
            children: Vec::new(),
        }
    }

    #[test]
    fn detail_is_appended_with_an_em_dash_when_present() {
        let rendered = render_label(&item("squawk", "Running · Healthy", Severity::Ok));
        assert_eq!(
            rendered,
            format!("{} squawk — Running · Healthy", Severity::Ok.icon())
        );
    }

    #[test]
    fn empty_detail_is_omitted_entirely() {
        let rendered = render_label(&item("Refresh", "", Severity::Unknown));
        assert_eq!(rendered, format!("{} Refresh", Severity::Unknown.icon()));
    }

    #[test]
    fn severity_icon_is_always_the_leading_glyph() {
        for severity in [
            Severity::Unknown,
            Severity::Ok,
            Severity::Warn,
            Severity::Bad,
        ] {
            let rendered = render_label(&item("x", "", severity));
            assert!(rendered.starts_with(severity.icon()));
        }
    }
}
