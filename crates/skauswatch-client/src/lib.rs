//! SkausWatch agent API client library.
//!
//! Provides a Rust client for the SkausWatch Manager API with HMAC-SHA256
//! request signing for agent authentication.

pub mod auth;
pub mod config;
pub mod error;

pub use auth::HmacSigner;
pub use config::ClientConfig;
pub use error::ClientError;
