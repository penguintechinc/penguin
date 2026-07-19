//! Secret redaction for anywhere a credential could otherwise leak into
//! command output or logs (`squawk config`'s `doh.auth_token`,
//! `license.license_key`, `license.user_token`).
//!
//! `penguin_telemetry::sanitize::mask_secret` already implements this exact
//! behaviour, but this milestone's brief calls for the module to own a
//! small local helper rather than pull in the telemetry crate for one pure
//! function — see this file's git history / the M5 report for that
//! decision. The two are intentionally byte-for-byte identical in
//! behaviour.

/// Renders `value` as a non-reversible hint, never the value itself:
/// empty stays empty, four characters or fewer become `"****"`, and
/// anything longer becomes `"****"` plus its last four characters. Matches
/// the Go squawk module's `maskSecret` exactly.
pub fn mask_secret(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    if value.chars().count() <= 4 {
        return "****".to_string();
    }

    // Byte index of the 4th-from-last character, found via `char_indices`
    // so a multibyte boundary can never split a character mid-way.
    let mut boundary = value.len();
    let mut seen = 0;
    for (index, _) in value.char_indices().rev() {
        boundary = index;
        seen += 1;
        if seen == 4 {
            break;
        }
    }

    let mut masked = String::with_capacity(4 + (value.len() - boundary));
    masked.push_str("****");
    masked.push_str(&value[boundary..]);
    masked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_go_mask_secret_rules() {
        assert_eq!(mask_secret(""), "");
        assert_eq!(mask_secret("a"), "****");
        assert_eq!(mask_secret("abcd"), "****");
        assert_eq!(mask_secret("abcde"), "****bcde");
        assert_eq!(mask_secret("supersecret"), "****cret");
    }

    #[test]
    fn is_char_boundary_safe_on_multibyte_input() {
        assert_eq!(mask_secret("αβγδεζ"), "****γδεζ");
    }
}
