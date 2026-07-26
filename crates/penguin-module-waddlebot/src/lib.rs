//! The waddlebot built-in module: a full read-and-write CLI-over-API surface
//! over the waddlebot hub, implementing `penguin_sdk::Module`.
//!
//! [`module::WaddlebotModule`] is the lifecycle/CLI glue; it wraps
//! `waddlebot_client::WaddlebotClient` (the hub REST client — its own
//! crate, already built and tested) rather than talking HTTP itself.
//!
//! The local dial-in bridge ([`bridge`]) is built here: a loopback
//! TCP/WebSocket + unix-socket server that brokers scoped, per-script access
//! to the hub on a connecting integration script's behalf, so no script ever
//! sees the module's upstream Community Access Token. See [`bridge`]'s doc
//! for the security model and [`bridge::BridgeAdapter`] for the seam a
//! later, separate track (a dial-**out** OBS adapter) plugs into — that
//! adapter itself is out of scope here.

mod bridge;
mod commands;
mod config;
mod mask;
mod metrics;
mod module;
#[cfg(test)]
mod testutil;

pub use module::{WaddlebotModule, factory};
