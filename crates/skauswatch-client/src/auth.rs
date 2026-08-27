//! Request signing for the SkausWatch agent API: HMAC-SHA256 of the exact
//! request body, keyed by the api_key the Manager returned at register, sent
//! as the `x-api-key` header alongside `x-agent-id`. Mirrors the Manager's
//! HMAC check in `services/manager/src/routes/endpoint.rs`.
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Signs agent requests with the per-agent HMAC key.
pub struct HmacSigner {
    agent_id: String,
    api_key: Vec<u8>,
}

impl HmacSigner {
    /// Builds a signer from the identity the Manager issued at register.
    pub fn new(agent_id: String, api_key: Vec<u8>) -> HmacSigner {
        HmacSigner { agent_id, api_key }
    }

    /// Returns the auth headers for a request carrying `body`.
    pub fn headers(&self, body: &[u8]) -> Vec<(String, String)> {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.api_key).expect("HMAC accepts any key length");
        mac.update(body);
        let digest = mac.finalize().into_bytes();
        let mut hex = String::with_capacity(digest.len() * 2);
        for byte in digest {
            hex.push_str(&format!("{byte:02x}"));
        }
        vec![
            ("x-agent-id".to_string(), self.agent_id.clone()),
            ("x-api-key".to_string(), hex),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::HmacSigner;

    #[test]
    fn headers_are_stable_hex_hmac_over_body() {
        let signer = HmacSigner::new("agent-7".to_string(), b"secret-key".to_vec());
        let h = signer.headers(br#"{"ping":true}"#);
        let get = |k: &str| h.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("x-agent-id").as_deref(), Some("agent-7"));
        // HMAC-SHA256("secret-key", {"ping":true}) — 64 lowercase hex chars, deterministic.
        let sig = get("x-api-key").expect("x-api-key present");
        assert_eq!(sig.len(), 64);
        assert_eq!(
            sig,
            signer.headers(br#"{"ping":true}"#)[1].1,
            "same body -> same sig"
        );
        assert_ne!(
            sig,
            signer.headers(br#"{"ping":false}"#)[1].1,
            "body change -> sig change"
        );
    }
}
