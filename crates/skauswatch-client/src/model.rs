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

/// Health-check payload posted to `/api/v1/endpoint/heartbeat` on every
/// heartbeat interval.
#[derive(Debug, Clone, Serialize)]
pub struct HeartbeatBody {
    /// Whether the agent's own self-check currently passes.
    pub healthy: bool,
    /// This crate's own build version (`CARGO_PKG_VERSION`), so the
    /// Manager can flag endpoints running a stale module build.
    pub module_version: String,
}

/// One observed event (module fault, policy violation, etc.), batched and
/// posted to `/api/v1/endpoint/events`.
#[derive(Debug, Clone, Serialize)]
pub struct EndpointEvent {
    /// Event category, defined by the emitting module (e.g.
    /// `"module_fault"`).
    pub kind: String,
    /// Severity label (e.g. `"info"`, `"warning"`, `"critical"`).
    pub severity: String,
    /// Arbitrary structured detail specific to `kind` — free-form so
    /// modules don't need a client-side schema change to add a field.
    pub detail: serde_json::Value,
    /// Unix timestamp (seconds) the event occurred, not when it was sent.
    pub ts_unix: i64,
}

/// Runtime configuration the Manager hands back from
/// `GET /api/v1/endpoint/config`.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    /// Seconds between heartbeats the agent should use going forward. No
    /// default — an agent must not silently fall back to a guessed cadence
    /// if the Manager omits this field.
    pub heartbeat_secs: u64,
    /// Additional, module-specific config the Manager may send — defaults
    /// to `Value::Null` so this client stays forward-compatible with new
    /// fields it doesn't know about yet.
    #[serde(default)]
    pub extra: serde_json::Value,
}
