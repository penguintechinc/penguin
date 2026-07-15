//! The tracing-backed [`Logger`] implementation modules log through.
//!
//! Every field passes through [`sanitize_value`] before it reaches tracing, so
//! secrets are redacted at the single boundary all module logging funnels
//! through — the module author never has to remember to mask.

use penguin_sdk::{LogLevel, Logger};

use crate::sanitize::sanitize_value;

/// A logger scoped to one module. Cheap to clone the name into; holds nothing
/// else because tracing's global subscriber does the actual writing.
pub struct TracingLogger {
    module: String,
}

impl TracingLogger {
    /// Creates a logger tagged with the given module name.
    pub fn new(module: impl Into<String>) -> TracingLogger {
        TracingLogger {
            module: module.into(),
        }
    }
}

/// Renders borrowed key/value fields into one `k=v k2=v2` string with sensitive
/// values already masked. Returns an owned string because it is emitted as a
/// single tracing field (tracing field *names* must be static, so dynamic
/// module fields are folded into one rendered value).
pub(crate) fn render_fields(fields: &[(&str, &str)]) -> String {
    let mut rendered = String::new();
    for (index, pair) in fields.iter().enumerate() {
        let (key, value) = *pair;
        if index > 0 {
            rendered.push(' ');
        }
        rendered.push_str(key);
        rendered.push('=');
        rendered.push_str(&sanitize_value(key, value));
    }
    rendered
}

impl Logger for TracingLogger {
    fn log(&self, level: LogLevel, message: &str, fields: &[(&str, &str)]) {
        let rendered = render_fields(fields);
        // The level is compile-time in each tracing macro, so this must fan out
        // per level rather than pass a runtime level — a genuine multi-way
        // branch, not stylistic. `message` goes through `%message` (not the
        // format string) so a message containing `{}` is never interpreted.
        match level {
            LogLevel::Debug => {
                tracing::debug!(module = %self.module, fields = %rendered, message = %message);
            }
            LogLevel::Info => {
                tracing::info!(module = %self.module, fields = %rendered, message = %message);
            }
            LogLevel::Warn => {
                tracing::warn!(module = %self.module, fields = %rendered, message = %message);
            }
            LogLevel::Error => {
                tracing::error!(module = %self.module, fields = %rendered, message = %message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_fields_masks_secret_values_only() {
        let fields = [("endpoint", "us-east"), ("auth_token", "abcdef")];
        assert_eq!(
            render_fields(&fields),
            "endpoint=us-east auth_token=****cdef"
        );
    }

    #[test]
    fn render_fields_handles_the_empty_case() {
        assert_eq!(render_fields(&[]), "");
    }

    #[test]
    fn log_at_every_level_does_not_panic() {
        // No global subscriber is installed in this unit test; the macros still
        // execute (covering each arm) and drop the event.
        let logger = TracingLogger::new("squawk");
        logger.log(LogLevel::Debug, "d", &[("k", "v")]);
        logger.log(LogLevel::Info, "i", &[]);
        logger.log(LogLevel::Warn, "w", &[]);
        logger.log(
            LogLevel::Error,
            "with braces {}",
            &[("password", "hunter2")],
        );
    }
}
