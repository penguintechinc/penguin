//! waddleai: the desktop-side companion to WaddleAI's agent-hooks feature,
//! implementing `penguin_sdk::Module`.
//!
//! # Architectural boundary
//!
//! **This module does not host MCP servers.** WaddleAI's MCP servers (the
//! user assistant at `/mcp`, the admin assistant at `/mcp/admin`, and the
//! external MCP gateway) all run in the WaddleAI server cluster. This crate
//! is a *client* of those endpoints and an installer of local glue — never
//! a host. If a change here starts standing up a tool registry or anything
//! that answers `list_tools`, that is a sign the change belongs
//! server-side instead.
//!
//! What this crate *does* own — localized, on-machine concerns only:
//!
//! - **Connection + credential management** ([`client`], [`module`]): the
//!   WaddleAI base URL and a `wa-`-prefixed virtual key, stored through
//!   `host.secrets()` (platform secure storage), never a plaintext file,
//!   env var, log line, or unmasked command output.
//! - **Hook shim installation** ([`hooks`]): per-ecosystem hook
//!   registrations for Claude Code/Cortex, Google Antigravity/AGY CLI, and
//!   VS Code. Every edit merges into the operator's existing config file
//!   and every uninstall restores it byte-for-byte — see [`hooks`]'s doc.
//! - **Health + status** ([`module`]): WaddleAI reachability, virtual-key
//!   validity, which shims are installed, and the denylist cache's
//!   staleness.
//! - **Localized caching** ([`cache`]): the last-synced Tier-1 denylist,
//!   the one piece of local state this crate is allowed to evaluate
//!   against — always a cached replay of the server's own answer, never a
//!   rule this crate invented. See [`cache`]'s doc for the offline
//!   fail-closed design and the staleness policy.
//! - **Telemetry** ([`metrics`]): hook invocation counts, decision
//!   outcomes, and evaluation latency — hooks run synchronously inside the
//!   calling agent's loop, so this module's own overhead is a headline
//!   metric, not a footnote.
//!
//! **No policy logic.** All rule evaluation happens in WaddleAI's engine;
//! this crate ships shims and forwards normalized events. If a change here
//! starts writing an allow/deny rule beyond an exact-match cache lookup
//! against the server's own last-synced denylist, that decision belongs
//! server-side instead — see [`module::WaddleAiModule::evaluate_hook_event`].

mod cache;
mod client;
mod commands;
mod config;
mod error;
mod fsutil;
pub mod hooks;
mod mask;
pub mod metrics;
mod module;
#[cfg(test)]
mod testutil;
mod tls;

pub use module::{DecisionSource, HookOutcome, WaddleAiModule, factory};
