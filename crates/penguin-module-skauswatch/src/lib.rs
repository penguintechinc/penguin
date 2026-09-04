//! SkausWatch: a monitoring and alerting endpoint client, implementing
//! `penguin_sdk::Module`.

mod commands;
mod config;
mod metrics;
mod module;
#[cfg(test)]
mod testutil;

pub use module::{SkausWatchModule, factory};

#[cfg(test)]
mod tests {
    #[test]
    fn factory_reports_identity_before_init() {
        let m = crate::factory();
        let info = m.info();
        assert_eq!(info.name, "skauswatch");
        assert_eq!(info.version, "1.0.0");
        assert!(
            info.license_feature.is_empty(),
            "loads core; entitlement enforced server-side"
        );
    }
}
