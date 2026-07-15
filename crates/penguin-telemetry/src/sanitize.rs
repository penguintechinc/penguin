//! The secret-redaction core.
//!
//! The Go build left PII sanitisation as a hand-applied convention (`maskSecret`
//! called at each risky log site). This is the completed version: a small,
//! pure, exhaustively-tested module the logger applies to *every* field so a
//! module author cannot forget to mask a secret.

use std::borrow::Cow;

/// Case-insensitive substrings that mark a field key as carrying a secret.
///
/// This is the canonical PenguinTech list (`password|token|api_key|secret`);
/// keys are matched by lowercased substring, so `dbPassword`, `access_token`,
/// and `client_secret` all redact.
const SENSITIVE_MARKERS: [&str; 4] = ["password", "token", "api_key", "secret"];

/// Reports whether a field key names a secret whose value must be masked.
pub fn is_sensitive_key(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    for marker in SENSITIVE_MARKERS {
        if lowered.contains(marker) {
            return true;
        }
    }
    false
}

/// Renders a secret as a non-reversible hint, or `""` when unset.
///
/// Port of the Go `maskSecret`: `""` stays empty, values of four characters or
/// fewer become `"****"`, and longer values become `"****"` plus their last
/// four characters. It works on character boundaries (not raw bytes like the Go
/// version) so multibyte input can never panic; for the ASCII tokens we mask
/// the result is identical.
pub fn mask_secret(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    if value.chars().count() <= 4 {
        return "****".to_string();
    }

    // Find the byte index of the 4th-from-last character, char-boundary safe.
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

/// Returns a field value with secrets masked: masked when `key` is sensitive,
/// borrowed unchanged otherwise (so the common non-secret path never allocates).
pub fn sanitize_value<'a>(key: &str, value: &'a str) -> Cow<'a, str> {
    if is_sensitive_key(key) {
        Cow::Owned(mask_secret(value))
    } else {
        Cow::Borrowed(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_secret_matches_the_go_rules() {
        assert_eq!(mask_secret(""), "");
        assert_eq!(mask_secret("a"), "****");
        assert_eq!(mask_secret("abcd"), "****");
        assert_eq!(mask_secret("abcde"), "****bcde");
        assert_eq!(mask_secret("supersecret"), "****cret");
    }

    #[test]
    fn mask_secret_is_char_safe_on_multibyte_input() {
        // Six multibyte chars; must not panic and must keep the last four.
        assert_eq!(mask_secret("αβγδεζ"), "****γδεζ");
    }

    #[test]
    fn sensitive_keys_are_detected_case_insensitively_and_as_substrings() {
        let sensitive = [
            "password",
            "dbPassword",
            "Password",
            "token",
            "access_token",
            "api_key",
            "secret",
            "client_secret",
        ];
        for key in sensitive {
            assert!(is_sensitive_key(key), "expected {key} to be sensitive");
        }
    }

    #[test]
    fn ordinary_keys_are_not_sensitive() {
        let ordinary = ["username", "endpoint", "count", "tunnel", "module"];
        for key in ordinary {
            assert!(!is_sensitive_key(key), "expected {key} to be ordinary");
        }
    }

    #[test]
    fn sanitize_value_masks_only_sensitive_keys() {
        assert_eq!(sanitize_value("endpoint", "us-east"), "us-east");
        assert_eq!(sanitize_value("auth_token", "abcdef"), "****cdef");
    }

    #[test]
    fn sanitize_value_borrows_the_non_secret_path() {
        let borrowed = sanitize_value("endpoint", "us-east");
        assert!(matches!(borrowed, Cow::Borrowed(_)));
    }
}
