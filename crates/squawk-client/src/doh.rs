//! Port of `pkg/client/doh_client.go`: a DNS-over-HTTPS client speaking the
//! Google/Cloudflare **JSON REST** dialect (`GET ?name=&type=` with
//! `Accept: application/dns-json`, plus a POST-with-JSON-body variant) —
//! **not** RFC 8484 binary DoH. [`crate::forwarder`] is what needs binary
//! DNS message handling, and does its own encode/decode via `hickory-proto`
//! on the local `:53` side.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, UnixTime};
use rustls::{
    ClientConfig, DigitallySignedStruct, Error as TlsError, RootCertStore, SignatureScheme,
};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::pem;
use crate::tls_support::{ensure_crypto_provider_installed, supported_algorithms};

/// Default per-request timeout — matches the Go client's `30 * time.Second`.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Default delay between retries — matches the Go client's `2 * time.Second`.
const DEFAULT_RETRY_DELAY: Duration = Duration::from_secs(2);
/// Idle-connection cap per host — Go's `http.Transport.MaxIdleConns: 10`
/// is a process-wide total; reqwest's knob is per-host, which is the closer
/// fit here since a `DoHClient` only ever talks to its own configured
/// server set.
const IDLE_CONNECTIONS_PER_HOST: usize = 10;
/// Idle-connection timeout — matches the Go client's `IdleConnTimeout: 30 * time.Second`.
const IDLE_CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);
const USER_AGENT: &str = "Squawk DNS Client (Rust)";

/// Well-known public DoH hostnames [`validate_server_url`] allows through
/// even though they aren't IP literals — copied verbatim from the Go
/// client's `allowedHosts` so existing configs referencing them keep
/// working. The whole point of the IP-literal rule is to stop the client
/// resolving its own upstream through itself; these are pinned exceptions,
/// not a general hostname allowance.
const ALLOWED_DOH_HOSTS: &[&str] = &[
    "dns.google",
    "dns.google.com",
    "cloudflare-dns.com",
    "1.1.1.1",
    "1.0.0.1",
    "dns.quad9.net",
    "dns.opendns.com",
    "doh.opendns.com",
    "dns.nextdns.io",
    "doh.cleanbrowsing.org",
];

/// DNS record types the Go client accepts in a query, uppercased — matches
/// `validRecordTypes` exactly.
const VALID_RECORD_TYPES: &[&str] = &[
    "A", "AAAA", "CNAME", "MX", "TXT", "NS", "SOA", "PTR", "SRV", "CAA", "DNSKEY", "DS", "NAPTR",
    "SSHFP", "TLSA", "ANY",
];

/// Configuration for [`DohClient`], mirroring the Go client's `Config`.
///
/// Unlike the Go struct (a bare `Config{}` zero value has `VerifySSL:
/// false`, i.e. insecure-by-default, because Go has no notion of a
/// constructor running for a zero value), [`Config::default`] here sets
/// `verify_ssl: true` — matching what the real Go `DefaultConfig()` produces
/// once it's actually invoked. Since `pkg/config`'s `DefaultConfig` is out
/// of this crate's scope, this `Default` impl is this crate's equivalent:
/// secure-by-default, with every other field at Go's zero value.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    /// A single DoH server URL. Ignored when `server_urls` is non-empty.
    #[serde(default)]
    pub server_url: String,
    /// Multiple DoH server URLs for round-robin failover. Takes priority
    /// over `server_url` when non-empty — matches Go's `NewDoHClient`.
    #[serde(default)]
    pub server_urls: Vec<String>,
    /// Bearer token sent as `Authorization: Bearer <token>` when non-empty.
    #[serde(default)]
    pub auth_token: String,
    /// Path to a PEM client certificate, for mutual TLS. Requires
    /// `client_key` to also be set.
    #[serde(default)]
    pub client_cert: String,
    /// Path to the PEM private key matching `client_cert`.
    #[serde(default)]
    pub client_key: String,
    /// Path to a PEM CA certificate (bundle) used to verify the server
    /// instead of the default webpki root store.
    #[serde(default)]
    pub ca_cert: String,
    /// Whether to verify the server's TLS certificate at all.
    ///
    /// **Setting this to `false` disables TLS server verification
    /// entirely** — any certificate, from any host, is accepted. This
    /// defeats the entire purpose of TLS (trivially MITM-able) and exists
    /// only for pointing at a self-signed or expired test server; it must
    /// never be `false` in a production configuration.
    #[serde(default = "default_true")]
    pub verify_ssl: bool,
    /// Maximum query attempts across all configured servers. `0` resolves
    /// to `server_urls.len() * 2` at construction time (try each server
    /// twice), matching Go.
    #[serde(default)]
    pub max_retries: usize,
    /// Delay between retries, in seconds. `0` resolves to `2`, matching Go.
    #[serde(default)]
    pub retry_delay: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server_url: String::new(),
            server_urls: Vec::new(),
            auth_token: String::new(),
            client_cert: String::new(),
            client_key: String::new(),
            ca_cert: String::new(),
            verify_ssl: true,
            max_retries: 0,
            retry_delay: 0,
        }
    }
}

fn default_true() -> bool {
    true
}

/// A DNS-over-HTTPS JSON response, matching the Google/Cloudflare DoH JSON
/// schema — field names and capitalization match the wire format exactly
/// (`Status`, `TC`, `RD`, ...), same as the Go client's `DNSResponse`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DnsResponse {
    /// The DNS response code: `0` is `NOERROR`, `3` is `NXDOMAIN`, anything
    /// else is treated as a server failure by [`crate::forwarder`].
    #[serde(rename = "Status", default)]
    pub status: i32,
    #[serde(rename = "TC", default)]
    pub tc: bool,
    #[serde(rename = "RD", default)]
    pub rd: bool,
    #[serde(rename = "RA", default)]
    pub ra: bool,
    #[serde(rename = "AD", default)]
    pub ad: bool,
    #[serde(rename = "CD", default)]
    pub cd: bool,
    #[serde(rename = "Question", default)]
    pub question: Vec<DnsRecord>,
    #[serde(rename = "Answer", default)]
    pub answer: Vec<DnsRecord>,
    #[serde(rename = "Comment", default)]
    pub comment: String,
    #[serde(rename = "TTL", default)]
    pub ttl: i32,
}

/// One record in a [`DnsResponse`]'s `Question`/`Answer` list.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DnsRecord {
    pub name: String,
    /// The record type as it arrived over the wire — see [`RecordKind`].
    #[serde(rename = "type")]
    pub kind: RecordKind,
    #[serde(rename = "TTL", default)]
    pub ttl: i64,
    #[serde(default)]
    pub data: String,
}

/// A DoH JSON record's `type` field, which different servers encode
/// differently: RFC-standard servers send the numeric type (`1` for A),
/// some send the mnemonic string directly (`"A"`). `#[serde(untagged)]`
/// tries each variant in order, so this accepts either without losing which
/// one it was — mirrors the Go client's `interface{}`-typed `DNSRecord.Type`
/// plus its `GetTypeString` helper, which [`RecordKind::as_type_str`] is.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum RecordKind {
    Int(i64),
    Text(String),
}

impl RecordKind {
    /// Returns the record type as its mnemonic string (`"A"`, `"CNAME"`,
    /// ...), converting from the numeric form via [`type_int_to_str`] when
    /// necessary. A string value is returned exactly as received, not
    /// uppercased — matches Go's `GetTypeString` exactly (it does not
    /// normalize case on the string branch either).
    pub fn as_type_str(&self) -> String {
        match self {
            RecordKind::Text(text) => text.clone(),
            RecordKind::Int(value) => type_int_to_str(*value),
        }
    }
}

/// Converts a DNS RFC-standard record-type integer to its mnemonic string,
/// matching Go's `recordTypeIntToString` map exactly (including the
/// `TYPE{n}` fallback for anything not in the table).
fn type_int_to_str(type_int: i64) -> String {
    let known = match type_int {
        1 => Some("A"),
        2 => Some("NS"),
        5 => Some("CNAME"),
        6 => Some("SOA"),
        12 => Some("PTR"),
        15 => Some("MX"),
        16 => Some("TXT"),
        28 => Some("AAAA"),
        33 => Some("SRV"),
        _ => None,
    };
    match known {
        Some(name) => name.to_string(),
        None => format!("TYPE{type_int}"),
    }
}

/// Every way constructing or using a [`DohClient`] can fail.
#[derive(Debug, thiserror::Error)]
pub enum DohError {
    #[error("no server URLs provided")]
    NoServerUrls,
    #[error("invalid server URL at index {index}: {message}")]
    InvalidServerUrl { index: usize, message: String },
    #[error("{0}")]
    InvalidName(String),
    #[error("{0}")]
    InvalidRecordType(String),
    #[error("failed to set up HTTP client: {0}")]
    HttpSetup(String),
    #[error("query cancelled")]
    Cancelled,
    #[error("all DNS servers failed after {attempts} attempts: {}", .errors.join("; "))]
    AllServersFailed {
        attempts: usize,
        errors: Vec<String>,
    },
}

/// A DNS-over-HTTPS client with round-robin failover across
/// [`Config::server_urls`] and optional mTLS.
///
/// All state that changes at runtime (the round-robin index, the timeout)
/// is behind atomics so `query`/`query_with_json` take `&self` — a single
/// client is meant to be shared (typically inside an `Arc`) across every
/// concurrent lookup, matching how [`crate::forwarder`] uses it.
pub struct DohClient {
    server_urls: Vec<String>,
    auth_token: String,
    http: reqwest::Client,
    timeout_millis: AtomicU64,
    max_retries: usize,
    retry_delay: Duration,
    current_index: AtomicUsize,
}

impl DohClient {
    /// Builds a new client: validates and normalizes every configured
    /// server URL, then sets up the HTTP/TLS stack. Fails fast (before any
    /// network I/O) on a bad URL, a missing mTLS key/cert pair, or an
    /// unreadable cert file — matches Go's `NewDoHClient`.
    pub fn new(config: Config) -> Result<DohClient, DohError> {
        let mut server_urls = if !config.server_urls.is_empty() {
            config.server_urls.clone()
        } else if !config.server_url.is_empty() {
            vec![config.server_url.clone()]
        } else {
            return Err(DohError::NoServerUrls);
        };

        for (index, server_url) in server_urls.iter_mut().enumerate() {
            validate_server_url(server_url)
                .map_err(|message| DohError::InvalidServerUrl { index, message })?;
            *server_url = normalize_server_url(server_url);
        }

        let max_retries = if config.max_retries > 0 {
            config.max_retries
        } else {
            server_urls.len() * 2
        };
        let retry_delay = if config.retry_delay > 0 {
            Duration::from_secs(config.retry_delay)
        } else {
            DEFAULT_RETRY_DELAY
        };

        let http = build_http_client(&config).map_err(DohError::HttpSetup)?;

        Ok(DohClient {
            server_urls,
            auth_token: config.auth_token,
            http,
            timeout_millis: AtomicU64::new(DEFAULT_TIMEOUT.as_millis() as u64),
            max_retries,
            retry_delay,
            current_index: AtomicUsize::new(0),
        })
    }

    /// The current per-request timeout.
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_millis.load(Ordering::Relaxed))
    }

    /// Updates the per-request timeout used by every subsequent query.
    /// Matches Go's `SetTimeout` — applied per-request here (via
    /// `RequestBuilder::timeout`) rather than by rebuilding the whole HTTP
    /// client, since reqwest supports a per-request override directly.
    pub fn set_timeout(&self, timeout: Duration) {
        self.timeout_millis
            .store(timeout.as_millis() as u64, Ordering::Relaxed);
    }

    /// Queries `domain` for `record_type` via `GET ?name=&type=`, retrying
    /// across every configured server on any failure (a request that fails
    /// to build, a network error, a non-200 status, or unparseable JSON all
    /// count) until `max_retries` attempts are exhausted. The delay between
    /// attempts is cancellable via `cancel`, matching Go's
    /// `select { case <-ctx.Done(): ...; case <-time.After(retryDelay): }`.
    pub async fn query(
        &self,
        cancel: &CancellationToken,
        domain: &str,
        record_type: &str,
    ) -> Result<DnsResponse, DohError> {
        let record_type = normalize_record_type(record_type)?;
        validate_dns_name(domain).map_err(DohError::InvalidName)?;

        let mut errors = Vec::new();
        for attempt in 0..self.max_retries {
            let server_url = self.current_server();
            let outcome = self.try_get(&server_url, domain, &record_type).await;

            match outcome {
                Ok(response) => return Ok(response),
                Err(message) => {
                    errors.push(format!("{server_url}: {message}"));
                    self.advance_server();
                    if attempt + 1 < self.max_retries && !self.wait_for_retry(cancel).await {
                        return Err(DohError::Cancelled);
                    }
                }
            }
        }

        Err(DohError::AllServersFailed {
            attempts: self.max_retries,
            errors,
        })
    }

    /// The POST-with-JSON-body variant of [`query`](Self::query). Unlike
    /// `query`, this does not retry across servers — it queries only the
    /// currently-selected server once, matching Go's `QueryWithJSON`
    /// exactly (it never calls `nextServer` on failure).
    pub async fn query_with_json(
        &self,
        domain: &str,
        record_type: &str,
    ) -> Result<DnsResponse, DohError> {
        let record_type = normalize_record_type(record_type)?;
        validate_dns_name(domain).map_err(DohError::InvalidName)?;

        if self.server_urls.is_empty() {
            return Err(DohError::NoServerUrls);
        }
        let server_url = self.current_server();

        #[derive(Serialize)]
        struct JsonQuery<'a> {
            name: &'a str,
            #[serde(rename = "type")]
            record_type: &'a str,
        }

        let mut request = self
            .http
            .post(&server_url)
            .timeout(self.timeout())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/dns-json")
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .json(&JsonQuery {
                name: domain,
                record_type: &record_type,
            });
        if !self.auth_token.is_empty() {
            request = request.bearer_auth(&self.auth_token);
        }

        let response = request
            .send()
            .await
            .map_err(|err| DohError::AllServersFailed {
                attempts: 1,
                errors: vec![format!("{server_url}: HTTP request failed: {err}")],
            })?;

        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|err| DohError::AllServersFailed {
                attempts: 1,
                errors: vec![format!("{server_url}: failed to read response body: {err}")],
            })?;

        if !status.is_success() {
            return Err(DohError::AllServersFailed {
                attempts: 1,
                errors: vec![format!(
                    "{server_url}: HTTP {status}: {}",
                    String::from_utf8_lossy(&body)
                )],
            });
        }

        serde_json::from_slice(&body).map_err(|err| DohError::AllServersFailed {
            attempts: 1,
            errors: vec![format!("{server_url}: failed to parse DNS response: {err}")],
        })
    }

    /// One GET attempt against `server_url`. Returns the failure as a
    /// message string rather than a typed error since the caller only ever
    /// accumulates these into [`DohError::AllServersFailed`].
    async fn try_get(
        &self,
        server_url: &str,
        domain: &str,
        record_type: &str,
    ) -> Result<DnsResponse, String> {
        let mut request = self
            .http
            .get(server_url)
            .timeout(self.timeout())
            .query(&[("name", domain), ("type", record_type)])
            .header(reqwest::header::ACCEPT, "application/dns-json")
            .header(reqwest::header::USER_AGENT, USER_AGENT);
        if !self.auth_token.is_empty() {
            request = request.bearer_auth(&self.auth_token);
        }

        let response = request
            .send()
            .await
            .map_err(|err| format!("HTTP request failed: {err}"))?;

        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|err| format!("failed to read response body: {err}"))?;

        if !status.is_success() {
            return Err(format!("HTTP {status}: {}", String::from_utf8_lossy(&body)));
        }

        serde_json::from_slice(&body).map_err(|err| format!("failed to parse DNS response: {err}"))
    }

    /// Sleeps for the retry delay, or returns `false` immediately if
    /// `cancel` fires first.
    async fn wait_for_retry(&self, cancel: &CancellationToken) -> bool {
        tokio::select! {
            _ = cancel.cancelled() => false,
            _ = tokio::time::sleep(self.retry_delay) => true,
        }
    }

    fn current_server(&self) -> String {
        let index = self.current_index.load(Ordering::Relaxed) % self.server_urls.len();
        self.server_urls[index].clone()
    }

    fn advance_server(&self) {
        self.current_index.fetch_add(1, Ordering::Relaxed);
    }
}

/// Uppercases and validates `record_type`, defaulting an empty string to
/// `"A"` — matches Go's `Query`/`QueryWithJSON` preamble exactly.
fn normalize_record_type(record_type: &str) -> Result<String, DohError> {
    let candidate = if record_type.is_empty() {
        "A"
    } else {
        record_type
    };
    let upper = candidate.to_ascii_uppercase();
    validate_record_type(&upper).map_err(DohError::InvalidRecordType)?;
    Ok(upper)
}

/// Validates a DNS record type against [`VALID_RECORD_TYPES`], matching
/// Go's `validateRecordType`. `record_type` must already be uppercased.
fn validate_record_type(record_type: &str) -> Result<(), String> {
    if VALID_RECORD_TYPES.contains(&record_type) {
        Ok(())
    } else {
        Err(format!(
            "invalid DNS record type '{record_type}': must be one of {VALID_RECORD_TYPES:?}"
        ))
    }
}

/// Validates a DNS domain name per RFC 1035 label rules, matching Go's
/// `validateDNSName` exactly — including its `.arpa`-TLD and
/// punycode (`xn--`) carve-outs.
fn validate_dns_name(domain: &str) -> Result<(), String> {
    if domain.is_empty() {
        return Err("DNS name cannot be empty".to_string());
    }
    if domain.len() > 253 {
        return Err(format!(
            "DNS name too long: {} characters (max 253)",
            domain.len()
        ));
    }

    let trimmed = domain.strip_suffix('.').unwrap_or(domain);
    const INVALID_CHARS: &str = " !@#$%^&*()+={}[]|\\:;\"'<>,?/`~";
    if trimmed.contains(|c: char| INVALID_CHARS.contains(c)) {
        return Err("DNS name contains invalid characters".to_string());
    }

    let labels: Vec<&str> = trimmed.split('.').collect();
    let last_index = labels.len().saturating_sub(1);
    for (index, label) in labels.iter().enumerate() {
        if label.is_empty() {
            return Err(format!("DNS name contains empty label at position {index}"));
        }
        if label.len() > 63 {
            return Err(format!(
                "DNS label '{label}' too long: {} characters (max 63)",
                label.len()
            ));
        }
        if index == last_index && *label == "arpa" {
            continue;
        }
        // Punycode labels are exempt from both the format check below and
        // the "no consecutive hyphens" check that follows it — mirrors the
        // net effect of Go's two separate `xn--`-prefix carve-outs.
        if label.starts_with("xn--") {
            continue;
        }
        if !is_valid_label(label) {
            return Err(format!(
                "invalid DNS label '{label}': must start/end with alphanumeric and contain only letters, digits, and hyphens"
            ));
        }
        if label.contains("--") {
            return Err(format!(
                "invalid DNS label '{label}': contains consecutive hyphens"
            ));
        }
    }

    Ok(())
}

/// Checks one label against the RFC 1035 character-class rule (the Go
/// client's `dnsLabelRegex`): alphanumeric or hyphen throughout,
/// alphanumeric at both ends.
fn is_valid_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    let Some(&first) = bytes.first() else {
        return false;
    };
    let Some(&last) = bytes.last() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() || !last.is_ascii_alphanumeric() {
        return false;
    }
    bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
}

/// Ensures a DoH server URL's host is either an IP literal, `localhost`, or
/// one of [`ALLOWED_DOH_HOSTS`] — matches Go's `validateServerURL`. This
/// exists to prevent the client resolving its own DoH server's hostname
/// through itself (a bootstrap deadlock), so an arbitrary hostname is
/// rejected even though it would otherwise be a perfectly valid URL.
fn validate_server_url(server_url: &str) -> Result<(), String> {
    if server_url.is_empty() {
        return Err("server URL cannot be empty".to_string());
    }

    let parsed =
        url::Url::parse(server_url).map_err(|err| format!("invalid server URL format: {err}"))?;

    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        return Err(format!(
            "server URL must use http or https scheme, got: {}",
            parsed.scheme()
        ));
    }

    let Some(host) = parsed.host_str() else {
        return Err("server URL must include a host".to_string());
    };

    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(());
    }

    let host_lower = host.to_ascii_lowercase();
    for allowed in ALLOWED_DOH_HOSTS {
        let is_exact = host_lower == *allowed;
        let is_subdomain = host_lower.starts_with(&format!("{allowed}."));
        if is_exact || is_subdomain {
            return Ok(());
        }
    }

    Err(format!(
        "server URL must use an IP address (not hostname '{host}') to prevent DNS resolution loops. Use the IP address of your DNS server instead"
    ))
}

/// Fills in the standard DoH path for known providers when the URL doesn't
/// already specify one — matches Go's `normalizeServerURL`. Cloudflare and
/// Quad9 both resolve to the same `/dns-query` the fallback already uses;
/// they're called out explicitly here only for parity/documentation, same
/// as the Go source.
fn normalize_server_url(server_url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(server_url) else {
        return server_url.to_string();
    };

    let path_is_default = parsed.path().is_empty() || parsed.path() == "/";
    if !path_is_default {
        return parsed.to_string();
    }

    let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    if host.contains("dns.google") {
        parsed.set_path("/resolve");
    } else {
        parsed.set_path("/dns-query");
    }
    parsed.to_string()
}

/// Builds the reqwest client: rustls with the aws-lc-rs crypto provider
/// (installed process-wide) and either the default webpki root store or a
/// custom CA, with optional client-certificate mTLS and an optional
/// disable-verification escape hatch — see [`Config::verify_ssl`].
fn build_http_client(config: &Config) -> Result<reqwest::Client, String> {
    ensure_crypto_provider_installed();

    let tls_config = build_tls_config(config)?;

    // NOT wrapped in `Some(...)` — see penguin-licensing's identical
    // comment: `use_preconfigured_tls` wraps its argument in `Some(...)`
    // itself before downcasting, so pre-wrapping here breaks the downcast.
    reqwest::Client::builder()
        .use_preconfigured_tls(tls_config)
        .pool_max_idle_per_host(IDLE_CONNECTIONS_PER_HOST)
        .pool_idle_timeout(IDLE_CONNECTION_TIMEOUT)
        .build()
        .map_err(|err| err.to_string())
}

fn build_tls_config(config: &Config) -> Result<ClientConfig, String> {
    let builder = ClientConfig::builder();
    let with_verifier = if config.verify_ssl {
        builder.with_root_certificates(build_root_store(config)?)
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(DangerousNoVerifier))
    };

    let has_client_cert = !config.client_cert.is_empty();
    let has_client_key = !config.client_key.is_empty();
    if has_client_cert != has_client_key {
        return Err(
            "client_cert and client_key must both be set, or both be empty, for mTLS".to_string(),
        );
    }

    if has_client_cert && has_client_key {
        let (chain, key) = load_client_identity(config)?;
        with_verifier
            .with_client_auth_cert(chain, key)
            .map_err(|err| format!("invalid client certificate/key: {err}"))
    } else {
        Ok(with_verifier.with_no_client_auth())
    }
}

fn build_root_store(config: &Config) -> Result<RootCertStore, String> {
    if config.ca_cert.is_empty() {
        return Ok(RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        });
    }

    let certs = pem::load_certificate_chain(&config.ca_cert).map_err(|err| err.to_string())?;
    let mut store = RootCertStore::empty();
    for cert in certs {
        store
            .add(cert)
            .map_err(|err| format!("invalid CA certificate: {err}"))?;
    }
    Ok(store)
}

type ClientIdentity = (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>);

fn load_client_identity(config: &Config) -> Result<ClientIdentity, String> {
    let chain = pem::load_certificate_chain(&config.client_cert).map_err(|err| err.to_string())?;
    let key = pem::load_private_key(&config.client_key).map_err(|err| err.to_string())?;
    Ok((chain, key))
}

/// Accepts any server certificate unconditionally — the implementation
/// behind [`Config::verify_ssl`] `== false`. See that field's doc comment
/// for why this is dangerous and only ever appropriate for testing.
#[derive(Debug)]
struct DangerousNoVerifier;

impl ServerCertVerifier for DangerousNoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &supported_algorithms())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &supported_algorithms())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        supported_algorithms().supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_kind_int_maps_known_types() {
        assert_eq!(RecordKind::Int(1).as_type_str(), "A");
        assert_eq!(RecordKind::Int(28).as_type_str(), "AAAA");
        assert_eq!(RecordKind::Int(15).as_type_str(), "MX");
        assert_eq!(RecordKind::Int(9999).as_type_str(), "TYPE9999");
    }

    #[test]
    fn record_kind_text_passes_through_unmodified() {
        assert_eq!(RecordKind::Text("A".to_string()).as_type_str(), "A");
        // Go's GetTypeString does not uppercase the string branch either.
        assert_eq!(RecordKind::Text("cname".to_string()).as_type_str(), "cname");
    }

    #[test]
    fn record_kind_deserializes_both_shapes() {
        let from_int: DnsRecord =
            serde_json::from_str(r#"{"name":"example.com.","type":1,"TTL":300,"data":"1.2.3.4"}"#)
                .unwrap();
        assert_eq!(from_int.kind, RecordKind::Int(1));

        let from_str: DnsRecord = serde_json::from_str(
            r#"{"name":"example.com.","type":"A","TTL":300,"data":"1.2.3.4"}"#,
        )
        .unwrap();
        assert_eq!(from_str.kind, RecordKind::Text("A".to_string()));
    }

    #[test]
    fn validate_dns_name_accepts_ordinary_names() {
        assert!(validate_dns_name("example.com").is_ok());
        assert!(validate_dns_name("www.example.com.").is_ok());
        assert!(validate_dns_name("a-b-c.example.com").is_ok());
        assert!(validate_dns_name("1.0.0.127.in-addr.arpa").is_ok());
        assert!(validate_dns_name("xn--nxasmq6b.example").is_ok());
    }

    #[test]
    fn validate_dns_name_rejects_bad_names() {
        assert!(validate_dns_name("").is_err());
        assert!(validate_dns_name(&"a".repeat(254)).is_err());
        assert!(validate_dns_name("has a space.com").is_err());
        assert!(validate_dns_name("bad..label.com").is_err());
        assert!(validate_dns_name("-startshyphen.com").is_err());
        assert!(validate_dns_name("endshyphen-.com").is_err());
        assert!(validate_dns_name("has--hyphens.com").is_err());
    }

    #[test]
    fn validate_record_type_accepts_known_types() {
        for rt in VALID_RECORD_TYPES {
            assert!(validate_record_type(rt).is_ok(), "{rt} should be valid");
        }
    }

    #[test]
    fn validate_record_type_rejects_unknown() {
        assert!(validate_record_type("BOGUS").is_err());
    }

    #[test]
    fn validate_server_url_accepts_ip_literals() {
        assert!(validate_server_url("https://192.0.2.1/dns-query").is_ok());
        assert!(validate_server_url("http://127.0.0.1:8080/resolve").is_ok());
    }

    #[test]
    fn validate_server_url_accepts_localhost() {
        assert!(validate_server_url("https://localhost:8443/dns-query").is_ok());
        assert!(validate_server_url("https://LOCALHOST/dns-query").is_ok());
    }

    #[test]
    fn validate_server_url_accepts_every_allowed_host() {
        for host in ALLOWED_DOH_HOSTS {
            let candidate = format!("https://{host}/dns-query");
            assert!(
                validate_server_url(&candidate).is_ok(),
                "{host} should be allowed"
            );
        }
    }

    #[test]
    fn validate_server_url_rejects_an_arbitrary_hostname() {
        let err = validate_server_url("https://my-internal-dns.example.net/dns-query");
        assert!(err.is_err());
    }

    #[test]
    fn validate_server_url_rejects_empty_and_bad_scheme() {
        assert!(validate_server_url("").is_err());
        assert!(validate_server_url("ftp://1.1.1.1/dns-query").is_err());
    }

    #[test]
    fn normalize_server_url_fills_google_resolve_path() {
        assert_eq!(
            normalize_server_url("https://dns.google"),
            "https://dns.google/resolve"
        );
    }

    #[test]
    fn normalize_server_url_fills_default_dns_query_path() {
        assert_eq!(
            normalize_server_url("https://1.1.1.1"),
            "https://1.1.1.1/dns-query"
        );
        assert_eq!(
            normalize_server_url("https://192.0.2.1/"),
            "https://192.0.2.1/dns-query"
        );
    }

    #[test]
    fn normalize_server_url_leaves_an_explicit_path_alone() {
        assert_eq!(
            normalize_server_url("https://192.0.2.1/custom-path"),
            "https://192.0.2.1/custom-path"
        );
    }
}
