//! Tobogganing: a WireGuard-based SASE/ZTNA endpoint client, implementing
//! `penguin_sdk::Module` — the last Go-dependent module in the endpoint
//! agent (see `go-client/internal/modules/tobogganing`).
//!
//! # What is a genuine port vs. what is greenfield
//!
//! [`auth`] (the JWT lifecycle against the manager) and [`module`]/[`commands`]
//! (the lifecycle glue and CLI surface) are real ports of working Go code,
//! fixing the specific bugs documented at each fix site. The WireGuard data
//! plane ([`wireguard`], driven by [`vpn::VpnManager`]) is greenfield: Go's
//! `VPNManager.Connect` built a `wgtypes.Config` and handed it to a
//! `WGController.Configure` that was a hard-coded `return nil`
//! (`go-client/internal/modules/tobogganing/vpn_wgctrl.go`) — no interface
//! was ever created, no peer was ever configured, and `Disconnect` only
//! flipped a boolean. This milestone implements that management surface for
//! the first time rather than translating it.
//!
//! [`wireguard::kernel`] is a real, working implementation over
//! `defguard_wireguard_rs`'s Linux netlink backend. [`wireguard::userspace`]
//! is honest about what it does and does not do — see its module doc.

mod auth;
mod commands;
mod config;
mod http;
pub mod metrics;
mod module;
#[cfg(test)]
mod testutil;
mod vpn;
// Private in the shipped module — `WireGuardBackend`/`KernelBackend` are an
// internal implementation detail of `vpn::VpnManager`. Made `pub` only under
// `integration-test` (a cfg, not a runtime check) so `tests/wg_tunnel.rs`
// can reach `KernelBackend` directly to prove it against a real kernel
// tunnel; see that feature's doc in `Cargo.toml`.
#[cfg(not(feature = "integration-test"))]
mod wireguard;
#[cfg(feature = "integration-test")]
pub mod wireguard;

pub use module::{TobogganingModule, factory};
