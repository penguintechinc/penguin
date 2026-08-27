//! Wire-format types for the SkausWatch agent registration API.

use serde::{Deserialize, Serialize};

/// Payload posted to `/api/v1/endpoint/register` to enroll a new agent.
#[derive(Debug, Clone, Serialize)]
pub struct RegisterRequest {
    /// Enrollment token issued out-of-band by the Manager operator.
    pub enrollment_token: String,
    /// This host's hostname, best-effort — informational only, never used
    /// as an identity key (the Manager assigns [`AgentIdentity::agent_id`]
    /// for that).
    pub hostname: String,
    /// `std::env::consts::OS` (e.g. `"linux"`, `"macos"`, `"windows"`).
    pub os: String,
    /// `std::env::consts::ARCH` (e.g. `"x86_64"`, `"aarch64"`).
    pub arch: String,
    /// This crate's own build version (`CARGO_PKG_VERSION`).
    pub agent_version: String,
}

/// The identity the Manager assigns an agent at registration. `api_key` is
/// the shared secret [`crate::HmacSigner`] signs every subsequent request
/// with; the Manager never sends it again after this response.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentIdentity {
    /// The agent ID the Manager assigned, sent back as the `x-agent-id`
    /// header on every signed request.
    pub agent_id: String,
    /// The HMAC signing key, hex/opaque string as issued — [`crate::HmacSigner::new`]
    /// takes it as raw bytes via `.into_bytes()`.
    pub api_key: String,
}
