//! Rust port of the parts of `squawk-client-go` the penguin squawk module
//! actually uses: the DNS-over-HTTPS client, the local `:53` forwarder, the
//! squawk license validator, and the plain-UDP SNTP offset client.
//!
//! Ported packages and their new module names:
//!
//! | Go package             | This crate     |
//! |-------------------------|----------------|
//! | `pkg/client` (DoH)      | [`doh`]        |
//! | `pkg/forwarder`         | [`forwarder`]  |
//! | `pkg/license`           | [`license`]    |
//! | `pkg/time` (`ntp_client.go` only) | [`ntp`] |
//! | `pkg/config` (subset)   | [`config`]     |
//!
//! `pkg/dhcp`, `pkg/k8swatcher`, `pkg/performance`, `pkg/resolver`, the
//! NTS/interceptor stack under `pkg/ntp/*`, `pkg/transport`, `pkg/grpc`, and
//! everything under `cmd/` were never used by the penguin agent and are not
//! ported here.
//!
//! Two behavioral departures from the Go source are load-bearing, not
//! incidental — see each module's doc comment for the full reasoning:
//!
//! * [`forwarder`]: Go's `Start` binds its listeners and then blocks until
//!   its context is cancelled, which deadlocks a caller that (like the
//!   squawk module) invokes it synchronously under a mutex. Here, `start()`
//!   binds synchronously (so a bind error is still immediate) and then
//!   returns — the serve loops run as background tasks.
//! * [`forwarder::Cache`]: Go's forwarder has no answer cache at all despite
//!   the module advertising `cache.enabled`/`cache stats`/`cache flush` —
//!   every query round-trips upstream. This port adds a real,
//!   TTL-respecting, bounded cache so those commands do something real.

pub mod config;
pub mod doh;
pub mod forwarder;
pub mod license;
pub mod ntp;
mod pem;
mod tls_support;
