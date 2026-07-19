//! [`LicenseClient`]: the license.penguintech.io HTTP client, its in-memory
//! cache, and the synchronous [`LicenseChecker`] implementation the daemon
//! and modules actually call.
//!
//! Rust port of the Go `internal/licensing.Client` (`go-client/internal/licensing/client.go`).
//! Endpoints, request/response shapes, headers, and defaults are matched
//! exactly; see each item's doc comment for the specific line ported.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime};

use penguin_sdk::LicenseChecker;
use serde::{Deserialize, Serialize};

use crate::cache::{self, CacheFile};

/// Default license server base URL — matches the Go client's
/// `Options.BaseURL` default exactly.
pub const DEFAULT_BASE_URL: &str = "https://license.penguintech.io";

/// Default product name sent with every validate request — matches the Go
/// client's `Options.Product` default exactly.
pub const DEFAULT_PRODUCT: &str = "penguin";

/// Default HTTP timeout for a single validate request — matches the Go
/// client's default `http.Client{Timeout: 10 * time.Second}`.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// The validate endpoint path, appended to `base_url` — matches the Go
/// client's `c.baseURL + "/api/v2/validate"` exactly.
const VALIDATE_PATH: &str = "/api/v2/validate";

/// Constructor options for [`LicenseClient`], mirroring the Go client's
/// `Options` struct. Two fields have no Rust analogue: `Logger` (this port
/// uses the workspace's global `tracing` subscriber instead of an injected
/// logger) and `RefreshInterval` (a parameter of
/// [`LicenseClient::spawn_background_refresh`] instead of stored state).
#[derive(Debug, Default)]
pub struct LicenseClientOptions {
    /// The license key sent as a bearer token. Empty means "no license":
    /// every feature reads as disabled and the tier reads as empty,
    /// matching the Go client's `licenseKey == ""` behavior exactly.
    pub license_key: String,
    /// The product name sent in the validate request body. Empty resolves
    /// to [`DEFAULT_PRODUCT`].
    pub product: String,
    /// The license server base URL. Empty resolves to [`DEFAULT_BASE_URL`].
    pub base_url: String,
    /// Where the offline cache is persisted. `None` disables on-disk
    /// persistence entirely (in-memory only for this process), matching
    /// the Go client's empty-string `CacheDir` behavior.
    pub cache_dir: Option<PathBuf>,
}

/// In-memory license state: the answer [`LicenseChecker`] reads, and the
/// same data mirrored to disk. Held behind one [`Mutex`] so a reader can
/// never observe a tier from one fetch paired with features from another.
struct State {
    tier: String,
    features: HashMap<String, bool>,
    fetched_at: SystemTime,
}

impl State {
    fn empty() -> Self {
        State {
            tier: String::new(),
            features: HashMap::new(),
            fetched_at: SystemTime::UNIX_EPOCH,
        }
    }
}

/// license.penguintech.io client with an offline cache and graceful
/// degradation, implementing [`penguin_sdk::LicenseChecker`].
///
/// [`feature_enabled`](LicenseChecker::feature_enabled) and
/// [`tier`](LicenseChecker::tier) never touch the network or a runtime —
/// they read `state` under a short-lived, poison-tolerant lock. All network
/// I/O happens in [`refresh`](LicenseClient::refresh), called explicitly or
/// periodically by [`spawn_background_refresh`](LicenseClient::spawn_background_refresh).
///
/// The Go client has no domain-based bypass of any kind (the frozen
/// `go-client/internal/licensing` package never inspects `base_url` or any
/// deployment-domain concept), so none is ported here either — the only way
/// to disable checks is the same as Go's: configure an empty `license_key`.
pub struct LicenseClient {
    license_key: String,
    product: String,
    base_url: String,
    cache_dir: Option<PathBuf>,
    http: reqwest::Client,
    state: Mutex<State>,
}

impl LicenseClient {
    /// Builds a new client, applying the same defaults as the Go
    /// constructor (`New` in `client.go`), and synchronously loads any
    /// existing on-disk cache so the very first `feature_enabled`/`tier`
    /// call already reflects the last-known entitlements.
    pub fn new(options: LicenseClientOptions) -> Self {
        let product = if options.product.is_empty() {
            DEFAULT_PRODUCT.to_string()
        } else {
            options.product
        };
        let base_url = if options.base_url.is_empty() {
            DEFAULT_BASE_URL.to_string()
        } else {
            options.base_url
        };

        let mut state = State::empty();
        if let Some(dir) = &options.cache_dir {
            load_into(dir, &mut state);
        }

        LicenseClient {
            license_key: options.license_key,
            product,
            base_url,
            cache_dir: options.cache_dir,
            http: build_http_client(),
            state: Mutex::new(state),
        }
    }

    /// Fetches current entitlements from the server and updates the cache.
    /// Never surfaces an error to the caller: an unreachable server, a
    /// non-200 status, or malformed JSON all leave the previous cached
    /// state untouched — graceful degradation, matching the Go client's
    /// `Validate`. A missing license key clears the cache in memory
    /// without ever calling the server, also matching Go (it does not
    /// persist that cleared state to disk either).
    pub async fn refresh(&self) {
        if self.license_key.is_empty() {
            let mut state = self.lock_state();
            state.tier.clear();
            state.features.clear();
            return;
        }

        let info = match self.fetch().await {
            Ok(info) => info,
            Err(err) => {
                tracing::debug!(error = %err, "failed to fetch license");
                return;
            }
        };

        self.update_cache(info);
    }

    /// Starts a background task that calls [`refresh`](Self::refresh)
    /// immediately and then again every `interval`, until the returned
    /// [`RefreshHandle`] is stopped. The Rust equivalent of the Go client's
    /// `Start`/`Stop` pair.
    pub fn spawn_background_refresh(
        self: &std::sync::Arc<Self>,
        interval: Duration,
    ) -> RefreshHandle {
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();
        let client = std::sync::Arc::clone(self);
        let task = tokio::spawn(async move {
            client.refresh().await;
            let mut ticker = tokio::time::interval(interval);
            ticker.tick().await; // the first tick fires immediately; the line above already did that fetch
            loop {
                tokio::select! {
                    _ = ticker.tick() => client.refresh().await,
                    _ = &mut stop_rx => break,
                }
            }
        });
        RefreshHandle {
            stop: stop_tx,
            task,
        }
    }

    /// Performs the HTTP POST to the license server. Every failure mode —
    /// connection error, non-200 status, unreadable body, malformed JSON —
    /// becomes a [`FetchError`] for [`refresh`](Self::refresh) to treat as
    /// "keep serving the cache".
    async fn fetch(&self) -> Result<ValidateResponse, FetchError> {
        let url = format!("{}{VALIDATE_PATH}", self.base_url);
        let payload = ValidateRequest {
            product: &self.product,
        };

        let response = self
            .http
            .post(&url)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.license_key),
            )
            .json(&payload)
            .send()
            .await
            .map_err(FetchError::Request)?;

        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(FetchError::Body)?;

        if status != 200 {
            let snippet = String::from_utf8_lossy(&body).into_owned();
            return Err(FetchError::Status(status, snippet));
        }

        serde_json::from_slice(&body).map_err(FetchError::Decode)
    }

    /// Replaces the cached tier/features with `info`'s and best-effort
    /// persists to disk. A persistence failure is logged, never
    /// propagated: an unwritable cache directory must not stop the daemon
    /// from using the freshly-fetched entitlements already in memory.
    fn update_cache(&self, info: ValidateResponse) {
        let mut features = HashMap::new();
        for feature in info.features {
            features.insert(feature.name, feature.entitled);
        }
        let fetched_at = SystemTime::now();

        {
            let mut state = self.lock_state();
            state.tier = info.tier.clone();
            state.features = features.clone();
            state.fetched_at = fetched_at;
        }

        let Some(dir) = &self.cache_dir else {
            return;
        };
        let file = CacheFile {
            tier: info.tier,
            features,
            fetched_at: cache::unix_seconds(fetched_at),
        };
        if let Err(err) = cache::persist(dir, &file) {
            tracing::debug!(error = %err, "failed to persist license cache");
        }
    }

    /// Locks `state`, recovering from a poisoned lock instead of
    /// propagating the panic — a panic while holding this lock must never
    /// turn every later `feature_enabled`/`tier` call into a crash.
    fn lock_state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl LicenseChecker for LicenseClient {
    /// Reports whether `key` is entitled. Matches the Go client exactly: an
    /// empty license key disables everything, and a flag never seen in a
    /// successful fetch (or restored cache) defaults to `false`.
    fn feature_enabled(&self, key: &str) -> bool {
        if self.license_key.is_empty() {
            return false;
        }
        self.lock_state()
            .features
            .get(key)
            .copied()
            .unwrap_or(false)
    }

    /// Returns the cached tier, or an empty string if none has ever been
    /// fetched or restored — matches the Go client's `cachedTier` default.
    fn tier(&self) -> String {
        self.lock_state().tier.clone()
    }
}

/// Loads `cache_dir`'s on-disk cache into `state`, if present and valid. A
/// missing or corrupt cache leaves `state` at its zero value — never an
/// error, matching the Go client's `loadCache`.
fn load_into(cache_dir: &std::path::Path, state: &mut State) {
    let Some(file) = cache::load(cache_dir) else {
        return;
    };
    state.tier = file.tier;
    state.features = file.features;
    state.fetched_at = cache::system_time(file.fetched_at);
}

/// Builds the reqwest client used for every license-server request: rustls
/// with the aws-lc-rs crypto provider (installed once, process-wide) and
/// root certificates supplied manually from `webpki-roots`.
///
/// This is deliberately manual rather than using reqwest's default TLS
/// setup: the workspace is pinned to aws-lc-rs everywhere because the
/// go-plugin P-521 certificate verification elsewhere in this workspace
/// cannot use rustls' default `ring` provider at all. Letting reqwest pull
/// in `ring` here would put two independent crypto backends in the same
/// binary — `cargo tree -p penguin-licensing -i ring` staying empty is the
/// gate that proves this function never regresses that.
fn build_http_client() -> reqwest::Client {
    ensure_crypto_provider_installed();

    let root_store = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    // NOT wrapped in `Some(...)`: reqwest's `use_preconfigured_tls` wraps its
    // argument in `Some(...)` itself before downcasting to
    // `Option<rustls::ClientConfig>`, so passing an already-wrapped `Option`
    // here makes the downcast target `Option<Option<ClientConfig>>` and
    // silently falls through to "Unknown TLS backend".
    reqwest::Client::builder()
        .use_preconfigured_tls(tls_config)
        .timeout(DEFAULT_TIMEOUT)
        .build()
        .expect("license HTTP client config is static and always valid")
}

/// Installs the aws-lc-rs crypto provider as the process default, exactly
/// once. Idempotent: losing the install race to another initializer
/// elsewhere in the daemon (e.g. the go-plugin TLS setup) is not an error,
/// since the whole workspace is pinned to aws-lc-rs.
fn ensure_crypto_provider_installed() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// A running background refresh loop started by
/// [`LicenseClient::spawn_background_refresh`]. Dropping it leaves the loop
/// running (matching a bare Go `context.Context` leak) — call
/// [`stop`](Self::stop) to actually halt it, the equivalent of the Go
/// client's `Stop`.
pub struct RefreshHandle {
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

impl RefreshHandle {
    /// Signals the loop to stop and waits for it to exit.
    pub async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.task.await;
    }
}

/// The `/api/v2/validate` request body — matches the Go client's payload
/// (`map[string]string{"product": c.product}`) exactly.
#[derive(Serialize)]
struct ValidateRequest<'a> {
    product: &'a str,
}

/// The subset of the license server's validate response this client reads.
/// The server returns more fields (customer, expiry, limits, server ID,
/// ...) but nothing in this client consumes them, so only `tier` and
/// `features` are declared — every field here stays live (no dead-code
/// allowances needed) and serde silently ignores whatever else is present.
#[derive(Deserialize)]
struct ValidateResponse {
    #[serde(default)]
    tier: String,
    #[serde(default)]
    features: Vec<ValidateFeature>,
}

/// One entry of `ValidateResponse.features` — matches the Go client's
/// `Feature.Name`/`Feature.Entitled` (the only two fields it reads out of
/// the wire `Feature` struct).
#[derive(Deserialize)]
struct ValidateFeature {
    name: String,
    entitled: bool,
}

/// Every way [`LicenseClient::fetch`] can fail. All variants are treated
/// identically by [`LicenseClient::refresh`] — logged, never surfaced —
/// but distinct variants make that logging useful.
#[derive(Debug, thiserror::Error)]
enum FetchError {
    #[error("license server unreachable: {0}")]
    Request(reqwest::Error),
    #[error("failed to read response body: {0}")]
    Body(reqwest::Error),
    #[error("license server returned {0}: {1}")]
    Status(u16, String),
    #[error("failed to parse response: {0}")]
    Decode(serde_json::Error),
}
