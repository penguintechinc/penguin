//! SkausWatch agent API client library.
//!
//! Provides a Rust client for the SkausWatch Manager's ENDPOINT agent API
//! (`/api/v1/endpoint/*`). Authentication is a static, out-of-band
//! provisioned `agent_id`/`api_key` pair sent verbatim as the
//! `x-agent-id`/`x-api-key` headers — see [`ClientConfig`] and
//! `crate::auth` for why there is no client-side HMAC/signing step.

pub mod auth;
pub mod client;
pub mod config;
pub mod error;
pub mod model;
mod tls_support;

pub use auth::agent_headers;
pub use client::SkausWatchClient;
pub use config::ClientConfig;
pub use error::ClientError;
pub use model::{
    AgentConfig, AgentConfigBody, EndpointEvent, HeartbeatRequest, HeartbeatResponse,
    RegisterRequest, RegisterResponse, ReportEventsResponse, Severity,
};
