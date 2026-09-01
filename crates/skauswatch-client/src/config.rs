//! SkausWatch agent API client configuration.

/// Configuration for connecting to the SkausWatch Manager API.
///
/// `agent_id` and `api_key` are provisioned **out-of-band** (config file,
/// secret store, whatever process seeds `endpoint_agents` for this agent)
/// rather than obtained from the wire. The Manager computes
/// `hex(HMAC-SHA256(ENDPOINT_API_SECRET, agent_id))` server-side and
/// compares it constant-time against the `x-api-key` header
/// (`services/manager/src/routes/endpoint.rs`'s `EndpointAgent` extractor,
/// `expected_api_key`) — this client never computes that HMAC itself, it
/// only holds the resulting static string and sends it verbatim.
///
/// `POST /api/v1/endpoint/register` is a check-in/upsert against an
/// `agent_id` (see [`crate::client::SkausWatchClient::register`]'s doc).
/// `enrollment_token` is **optional** and only consulted by the real
/// Manager when `agent_id` is genuinely new to it — it resolves which
/// tenant the row is created under (`RegisterBody::enrollment_token`,
/// endpoint.rs ~line 315-328; `register_agent`'s branch split
/// ~line 461-471). Re-registration of an already-known `agent_id` ignores
/// it even if present. Leave it `None` for the steady-state check-in path
/// once an operator has already provisioned the `endpoint_agents` row
/// out-of-band; set it `Some(...)` for a brand-new agent's first-ever
/// `register()` call.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Base URL of the SkausWatch Manager API (e.g., https://manager.example.com).
    pub base_url: String,
    /// This agent's provisioned identity, sent as the `x-agent-id` header
    /// on every request.
    pub agent_id: String,
    /// The static credential sent verbatim as the `x-api-key` header.
    pub api_key: String,
    /// Per-tenant enrollment token, sent in the `register` body only when
    /// `Some` — see this struct's doc for when it's needed.
    pub enrollment_token: Option<String>,
}

impl ClientConfig {
    /// Creates a new client configuration from out-of-band provisioned
    /// credentials. `enrollment_token` should be `None` for a normal
    /// check-in against an already-provisioned `agent_id`, or `Some(...)`
    /// when registering a brand-new agent for the first time.
    pub fn new(
        base_url: String,
        agent_id: String,
        api_key: String,
        enrollment_token: Option<String>,
    ) -> Self {
        Self {
            base_url,
            agent_id,
            api_key,
            enrollment_token,
        }
    }
}
