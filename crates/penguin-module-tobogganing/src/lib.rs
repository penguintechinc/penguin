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
mod metrics;
mod module;
#[cfg(test)]
mod testutil;
mod vpn;
mod wireguard;

pub use module::{TobogganingModule, factory};
