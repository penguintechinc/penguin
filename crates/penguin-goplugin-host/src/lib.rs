//! A Rust implementation of the hashicorp go-plugin **host** (client) side.
//!
//! This is the load-bearing crate of the migration: it must launch and speak to
//! plugin binaries **built in Go against go-plugin v1.7.0** without those
//! binaries changing at all. Everything here is therefore dictated by the
//! upstream wire format, not by taste.
//!
//! Two facts drive the whole design:
//!
//! * Plugins present a self-signed **ECDSA P-521** certificate. rustls' default
//!   `ring` provider cannot verify secp521r1, so this crate requires the
//!   `aws-lc-rs` provider. There is no graceful degradation — with `ring` the
//!   handshake fails against every existing plugin.
//! * The plugin never reads stdin and **catches and ignores SIGINT**, so the
//!   only correct shutdown is the controller `Shutdown` RPC followed by a
//!   bounded wait and then SIGKILL. Anything else leaks child processes.
//!
//! # Divergence from the frozen Go host
//!
//! In the Go implementation the HostService broker leg is **dead code**: the
//! plugin-side hook that dials broker id 1 and calls `Module::init` is never
//! invoked, and the host serves that leg in plaintext while a correct plugin
//! would dial it with TLS. So no external plugin has ever received
//! `HostServices`. This crate serves broker id 1 properly and TLS-wrapped, so a
//! correctly-written plugin can call back into the daemon. Existing Go plugins
//! simply never dial it — which is why the compat gate proves the two halves
//! with two different plugins: the frozen Go one for the protocol, and a Rust
//! one for the host callbacks.

pub mod adapter;
pub mod broker;
pub mod client;
pub mod controller;
pub mod error;
pub mod handshake;
pub mod mtls;
pub mod stdio;

pub use error::HostError;
pub use handshake::{Handshake, HandshakeError};
