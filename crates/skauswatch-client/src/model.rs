//! Wire-format types for the SkausWatch Manager's ENDPOINT agent API
//! (`/api/v1/endpoint/*`). Every shape here is verified directly against
//! `services/manager/src/routes/endpoint.rs` in the `skauswatch` repo —
//! each type's doc comment cites the handler/struct it mirrors.

use serde::{Deserialize, Serialize};

/// Payload posted to `/api/v1/endpoint/register` (`RegisterBody`,
/// endpoint.rs ~line 312-329). Despite the name this is a check-in/upsert
/// against an agent identity usually provisioned out-of-band, not
/// exclusively an identity-issuing call: an existing `agent_id` is
/// re-registered (200, full field overwrite, tenant left untouched —
/// `register_agent` ~line 428-459) and `enrollment_token` is ignored even
/// if present; a brand-new `agent_id` additionally requires a valid,
/// unexpired `enrollment_token` (~line 322-328, ~line 461-471) to resolve
/// the tenant it's created under.
///
/// `enrollment_token` is `Some` only when [`crate::ClientConfig`] carries
/// one — see that struct's doc for when to set it. Omitted from the wire
/// entirely when `None` (`skip_serializing_if`), matching the field being
/// `Option<String>` server-side too.
#[derive(Debug, Clone, Serialize)]
pub struct RegisterRequest {
    /// This agent's provisioned identity — required server-side, 1-128
    /// chars.
    pub agent_id: String,
    /// Required server-side (may be an empty string — 0-255 chars).
    pub hostname: String,
    /// Defaults to `""` server-side if omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_address: Option<String>,
    /// Defaults to `"unknown"` server-side if omitted; this client always
    /// sends `std::env::consts::OS`.
    pub os_type: String,
    /// Defaults to `""` server-side if omitted.
    pub os_version: String,
    /// Required server-side (may be an empty string — 0-32 chars).
    pub agent_version: String,
    /// Defaults to `{}` server-side if omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// Present only for a brand-new agent's first registration — see this
    /// struct's doc and [`crate::ClientConfig::enrollment_token`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enrollment_token: Option<String>,
}

/// Response body from `register_agent` (`RegisterResponse`, endpoint.rs
/// ~line 356-363) — identical shape whether the agent was re-registered
/// (200) or newly created (201, ~line 489-496). Deliberately carries no
/// `api_key`: the credential is provisioned out-of-band, never issued over
/// the wire.
#[derive(Debug, Clone, Deserialize)]
pub struct RegisterResponse {
    pub message: String,
    pub agent_id: String,
    pub status: String,
}

/// Payload posted to `/api/v1/endpoint/heartbeat` (`HeartbeatBody`,
/// endpoint.rs ~line 500-505). The real body also accepts an optional
/// `metadata` object, merged over the stored metadata server-side — this
/// client doesn't currently expose a way to set it; omitting it is a valid
/// subset of the real contract (defaults to `{}` server-side, ~line 524).
#[derive(Debug, Clone, Serialize)]
pub struct HeartbeatRequest {
    pub agent_id: String,
    /// One of `"active"`, `"inactive"`, `"disconnected"` (`AGENT_STATUSES`,
    /// endpoint.rs ~line 31); absent defaults to `"active"` server-side.
    pub status: String,
}

/// Response body from `heartbeat` (`HeartbeatResponse`, endpoint.rs
/// ~line 528-535, wire body at ~line 581-585). **Not** just
/// `{"status":"ok"}` — it also echoes `agent_id` and a server-generated
/// `timestamp`.
#[derive(Debug, Clone, Deserialize)]
pub struct HeartbeatResponse {
    pub status: String,
    pub agent_id: String,
    pub timestamp: String,
}

/// `THREAT_LEVELS` (endpoint.rs ~line 33) — the only severities
/// `report_events` accepts; anything else is rejected with a per-event
/// validation error (`SEVERITY_MSG`, ~line 35, ~line 660-664).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
    Info,
}

/// One event, POSTed to `/api/v1/endpoint/events`.
///
/// **Not** wrapped in an `{agent_id, events: [...]}` envelope. The real
/// handler (`report_events`, endpoint.rs ~line 772-830) reads the raw
/// request body as JSON and treats it as either a single event object or a
/// bare array of event objects (~line 779-782); `parse_event`
/// (~line 646-684) requires **each individual event** to carry its own
/// `agent_id` field (~line 650-654) — there is no top-level `agent_id` at
/// all. [`crate::client::SkausWatchClient::report_events`] sends
/// `&[EndpointEvent]` directly as the JSON body for this reason.
#[derive(Debug, Clone, Serialize)]
pub struct EndpointEvent {
    /// Required, 1-128 chars (endpoint.rs ~line 650-654).
    pub agent_id: String,
    /// Required, 1-64 chars (~line 655-659).
    pub event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    /// Max 255 chars if present (~line 674).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_path: Option<String>,
    /// Max 128 chars if present (~line 676).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_hash: Option<String>,
    /// Max 255 chars if present (~line 677).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_process: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_line: Option<String>,
    /// List of objects (~line 628-642).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_connections: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_operations: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_operations: Option<serde_json::Value>,
    /// Object, defaults to `{}` server-side if omitted (~line 665-669).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// Response body from `report_events` (`ReportEventsResponse`, endpoint.rs
/// ~line 746-754, wire body ~line 821-829) — always HTTP 202; per-item
/// failures are reported in `errors` (capped at 10 entries), the batch
/// itself never fails.
#[derive(Debug, Clone, Deserialize)]
pub struct ReportEventsResponse {
    pub status: String,
    pub events_received: usize,
    pub events_stored: i64,
    pub errors: Vec<serde_json::Value>,
}

/// Per-agent config nested under [`AgentConfig::config`] (`AgentConfigInner`,
/// endpoint.rs ~line 844-851). Every field is metadata-overridable
/// server-side (~line 888), so each is typed as a generic
/// `serde_json::Value` rather than a fixed primitive, matching the real
/// struct exactly.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfigBody {
    pub reporting_interval: serde_json::Value,
    pub heartbeat_interval: serde_json::Value,
    pub event_batch_size: serde_json::Value,
    pub enabled_collectors: serde_json::Value,
    pub severity_threshold: serde_json::Value,
}

/// Response body from `GET /api/v1/endpoint/config` (`AgentConfigResponse`,
/// endpoint.rs ~line 854-858, wire body ~line 892-913). **Not** a top-level
/// `heartbeat_secs` — the interval fields are nested under `config`.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    pub agent_id: String,
    pub config: AgentConfigBody,
}
