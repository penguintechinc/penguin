//! Static agent auth headers for the SkausWatch agent API.
//!
//! The Manager authenticates every ENDPOINT agent request via two headers
//! — `x-agent-id` and `x-api-key` — verified by the `EndpointAgent`
//! extractor (`services/manager/src/routes/endpoint.rs` ~line 180-213).
//! `x-api-key` is expected to equal `hex(HMAC-SHA256(ENDPOINT_API_SECRET,
//! agent_id))`, but that HMAC is computed and constant-time-compared
//! **server-side** (`expected_api_key`, ~line 157-162) — the agent is
//! provisioned with the resulting `api_key` string out-of-band and sends it
//! verbatim. There is no per-request signature over the request body; a
//! prior version of this client computed one client-side and it never
//! matched the real Manager.

/// Returns the static `x-agent-id`/`x-api-key` header pair for the given
/// provisioned identity. Both values are sent verbatim — never hashed,
/// signed, or otherwise transformed by this client.
pub fn agent_headers(agent_id: &str, api_key: &str) -> [(&'static str, String); 2] {
    [
        ("x-agent-id", agent_id.to_string()),
        ("x-api-key", api_key.to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::agent_headers;

    #[test]
    fn agent_headers_returns_the_static_pair_verbatim() {
        let headers = agent_headers("agent-7", "static-key-abc");
        assert_eq!(headers[0], ("x-agent-id", "agent-7".to_string()));
        assert_eq!(headers[1], ("x-api-key", "static-key-abc".to_string()));
    }

    #[test]
    fn agent_headers_never_transforms_the_api_key() {
        // The api_key is the credential itself, sent as-is — no HMAC, no
        // hashing, regardless of what's passed as the "body" of a request.
        let headers = agent_headers("a", "not-a-hash-just-a-provisioned-token");
        assert_eq!(headers[1].1, "not-a-hash-just-a-provisioned-token");
    }

    #[test]
    fn agent_headers_is_stable_across_calls() {
        // Unlike the removed HmacSigner, the same (agent_id, api_key) pair
        // always yields the same headers, regardless of any request body.
        let a = agent_headers("agent-1", "key-1");
        let b = agent_headers("agent-1", "key-1");
        assert_eq!(a, b);
    }
}
