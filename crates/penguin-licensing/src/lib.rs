//! license.penguintech.io client with an offline cache and graceful
//! degradation — the Rust port of the Go `internal/licensing` package.
//!
//! [`LicenseClient`] implements [`penguin_sdk::LicenseChecker`]: its
//! `feature_enabled`/`tier` methods are synchronous, never touch the
//! network, and never panic, by reading a cache that [`LicenseClient::refresh`]
//! (or the periodic loop from [`LicenseClient::spawn_background_refresh`])
//! keeps up to date out of band. See `client.rs` for the endpoint, request
//! shape, and defaults ported from the Go client, and `cache.rs` for the
//! on-disk persistence that survives a daemon restart.
//!
//! ## Why the TLS stack is wired by hand
//!
//! This workspace pins `rustls` to the `aws-lc-rs` crypto provider
//! workspace-wide, because the go-plugin integration elsewhere needs to
//! verify P-521 certificates that rustls' default `ring` provider cannot
//! handle at all. `reqwest` is therefore built here with its default TLS
//! feature set disabled and `rustls-no-provider` enabled instead, which
//! means this crate is responsible for installing the aws-lc-rs provider
//! and supplying root certificates (from `webpki-roots`) itself — see
//! `client.rs::build_http_client`. `cargo tree -p penguin-licensing -i ring`
//! must always report nothing; that's the gate this wiring exists to keep
//! green.

mod cache;
mod client;

pub use client::{
    DEFAULT_BASE_URL, DEFAULT_PRODUCT, LicenseClient, LicenseClientOptions, RefreshHandle,
};
