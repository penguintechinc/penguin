//! SkausWatch agent API client library.
//!
//! Provides a Rust client for the SkausWatch Manager API with HMAC-SHA256
//! request signing for agent authentication.

pub mod auth;
pub mod client;
pub mod config;
pub mod error;
pub mod model;
mod tls_support;

pub use auth::HmacSigner;
pub use client::SkausWatchClient;
pub use config::ClientConfig;
pub use error::ClientError;
pub use model::{AgentConfig, AgentIdentity, EndpointEvent, HeartbeatBody, RegisterRequest};
