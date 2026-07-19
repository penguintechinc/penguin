//! Squawk module: a DNS-over-HTTPS endpoint client with local `:53`
//! forwarding and system DNS resolver management, implementing
//! `penguin_sdk::Module` — the first *real* built-in module the daemon's
//! plugin framework hosts (previously only fakes and the trivial `hello`
//! example plugin ever exercised it).
//!
//! [`SquawkModule`] is the lifecycle/CLI glue; [`sysresolver`] — the
//! per-OS, crash-safe system-resolver state machine it drives when
//! `system_dns.manage` is enabled — is its own reviewed-and-tested-in-
//! isolation submodule, implemented ahead of this one.

mod commands;
mod config;
mod mask;
mod metrics;
mod module;
pub mod sysresolver;

pub use module::{SquawkModule, factory};
