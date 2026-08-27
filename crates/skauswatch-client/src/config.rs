//! SkausWatch agent API client configuration.

/// Configuration for connecting to the SkausWatch Manager API.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Base URL of the SkausWatch Manager API (e.g., https://manager.example.com).
    pub base_url: String,
    /// Enrollment token issued by the Manager at agent registration.
    pub enrollment_token: String,
}

impl ClientConfig {
    /// Creates a new client configuration.
    pub fn new(base_url: String, enrollment_token: String) -> Self {
        Self {
            base_url,
            enrollment_token,
        }
    }
}
