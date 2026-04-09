//! Core domain types for proxy management.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The protocol variant of a proxy endpoint.
///
/// # Example
/// ```
/// use stygian_proxy::types::ProxyType;
/// assert_eq!(ProxyType::Http, ProxyType::Http);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyType {
    /// Plain HTTP proxy (CONNECT / forwarding).
    Http,
    /// HTTPS proxy over TLS.
    Https,
    #[cfg(feature = "socks")]
    /// SOCKS4 proxy (requires the `socks` feature).
    Socks4,
    #[cfg(feature = "socks")]
    /// SOCKS5 proxy (requires the `socks` feature).
    Socks5,
}

/// TLS-profiled request mode for proxy-side HTTP operations.
///
/// Used by `tls-profiled` integrations to decide how strictly browser TLS
/// profiles should be mapped onto rustls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfiledRequestMode {
    /// Broad compatibility: skip unknown entries and use safe fallbacks.
    Compatible,
    /// Profile-aware preset selected from the profile name.
    Preset,
    /// Strict cipher-suite mapping with compatibility group fallback.
    Strict,
    /// Strict cipher-suite + group mapping without fallback.
    StrictAll,
}

/// A proxy endpoint with optional authentication credentials.
///
/// `Debug` output masks `password` to prevent accidental credential logging.
///
/// # Example
/// ```
/// use stygian_proxy::types::{Proxy, ProxyType};
/// let p = Proxy {
///     url: "http://proxy.example.com:8080".into(),
///     proxy_type: ProxyType::Http,
///     username: Some("alice".into()),
///     password: Some("secret".into()),
///     weight: 1,
///     tags: vec!["prod".into()],
/// };
/// let debug = format!("{p:?}");
/// assert!(debug.contains("***"), "password must be masked in Debug output");
/// ```
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Proxy {
    /// The proxy URL, e.g. `http://proxy.example.com:8080`.
    pub url: String,
    pub proxy_type: ProxyType,
    pub username: Option<String>,
    pub password: Option<String>,
    /// Relative selection weight for weighted rotation (default: `1`).
    pub weight: u32,
    /// User-defined tags for filtering and grouping.
    pub tags: Vec<String>,
}

impl std::fmt::Debug for Proxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Proxy")
            .field("url", &self.url)
            .field("proxy_type", &self.proxy_type)
            .field("username", &self.username)
            .field("password", &self.password.as_deref().map(|_| "***"))
            .field("weight", &self.weight)
            .field("tags", &self.tags)
            .finish()
    }
}

/// A [`Proxy`] with a stable identity and insertion timestamp.
///
/// # Example
/// ```
/// use stygian_proxy::types::{Proxy, ProxyType, ProxyRecord};
/// let proxy = Proxy {
///     url: "http://proxy.example.com:8080".into(),
///     proxy_type: ProxyType::Http,
///     username: None,
///     password: None,
///     weight: 1,
///     tags: vec![],
/// };
/// let record = ProxyRecord::new(proxy);
/// assert!(!record.id.is_nil());
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ProxyRecord {
    pub id: Uuid,
    pub proxy: Proxy,
    /// Wall-clock time the proxy was added. Not serialized — `Instant` is
    /// not meaningfully portable; defaults to `Instant::now()` on deserialization.
    #[serde(skip, default = "Instant::now")]
    pub added_at: Instant,
}

impl ProxyRecord {
    /// Create a new [`ProxyRecord`] wrapping `proxy` with a freshly generated UUID.
    pub fn new(proxy: Proxy) -> Self {
        Self {
            id: Uuid::new_v4(),
            proxy,
            added_at: Instant::now(),
        }
    }
}

/// Per-proxy runtime metrics using lock-free atomic counters.
///
/// Intended to be shared via `Arc<ProxyMetrics>`.
///
/// # Example
/// ```
/// use stygian_proxy::types::ProxyMetrics;
/// let m = ProxyMetrics::default();
/// assert_eq!(m.success_rate(), 0.0);
/// assert_eq!(m.avg_latency_ms(), 0.0);
/// ```
#[derive(Debug, Default)]
pub struct ProxyMetrics {
    pub requests_total: AtomicU64,
    pub successes: AtomicU64,
    pub failures: AtomicU64,
    pub total_latency_ms: AtomicU64,
}

impl ProxyMetrics {
    /// Returns the fraction of requests that succeeded, in `[0.0, 1.0]`.
    ///
    /// Returns `0.0` when no requests have been recorded.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::ProxyMetrics;
    /// use std::sync::atomic::Ordering;
    /// let m = ProxyMetrics::default();
    /// m.requests_total.store(10, Ordering::Relaxed);
    /// m.successes.store(8, Ordering::Relaxed);
    /// assert!((m.success_rate() - 0.8).abs() < f64::EPSILON);
    /// ```
    pub fn success_rate(&self) -> f64 {
        let total = self.requests_total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        self.successes.load(Ordering::Relaxed) as f64 / total as f64
    }

    /// Returns the average request latency in milliseconds.
    ///
    /// Returns `0.0` when no requests have been recorded.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::ProxyMetrics;
    /// use std::sync::atomic::Ordering;
    /// let m = ProxyMetrics::default();
    /// m.requests_total.store(4, Ordering::Relaxed);
    /// m.total_latency_ms.store(400, Ordering::Relaxed);
    /// assert!((m.avg_latency_ms() - 100.0).abs() < f64::EPSILON);
    /// ```
    pub fn avg_latency_ms(&self) -> f64 {
        let total = self.requests_total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        self.total_latency_ms.load(Ordering::Relaxed) as f64 / total as f64
    }
}

mod serde_duration_secs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        d.as_secs().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        Ok(Duration::from_secs(u64::deserialize(d)?))
    }
}

/// Configuration governing health checking and circuit-breaker behaviour.
///
/// Duration fields serialize as integer seconds for TOML/JSON compatibility.
///
/// # Example
/// ```
/// use stygian_proxy::types::ProxyConfig;
/// use std::time::Duration;
/// let cfg = ProxyConfig::default();
/// assert_eq!(cfg.health_check_url, "https://httpbin.org/ip");
/// assert_eq!(cfg.health_check_interval, Duration::from_secs(60));
/// assert_eq!(cfg.health_check_timeout, Duration::from_secs(5));
/// assert_eq!(cfg.circuit_open_threshold, 5);
/// assert_eq!(cfg.circuit_half_open_after, Duration::from_secs(30));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ProxyConfig {
    /// URL called during health checks to verify proxy liveness.
    pub health_check_url: String,
    /// How often to run health checks (seconds).
    #[serde(with = "serde_duration_secs")]
    pub health_check_interval: Duration,
    /// Per-probe HTTP timeout (seconds).
    #[serde(with = "serde_duration_secs")]
    pub health_check_timeout: Duration,
    /// Consecutive failures before the circuit trips to OPEN.
    pub circuit_open_threshold: u32,
    /// How long to wait in OPEN before transitioning to HALF-OPEN (seconds).
    #[serde(with = "serde_duration_secs")]
    pub circuit_half_open_after: Duration,
    /// Sticky-session policy for domain→proxy binding.
    #[serde(default)]
    pub sticky_policy: crate::session::StickyPolicy,
    /// Optional default mode for TLS-profiled helper clients.
    ///
    /// When set and `tls-profiled` is enabled, `ProxyManager` initializes its
    /// `HealthChecker` with a Chrome-profiled requester using this mode.
    ///
    /// Ignored when `tls-profiled` is disabled.
    #[serde(default)]
    pub profiled_request_mode: Option<ProfiledRequestMode>,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            health_check_url: "https://httpbin.org/ip".into(),
            health_check_interval: Duration::from_secs(60),
            health_check_timeout: Duration::from_secs(5),
            circuit_open_threshold: 5,
            circuit_half_open_after: Duration::from_secs(30),
            sticky_policy: crate::session::StickyPolicy::default(),
            profiled_request_mode: None,
        }
    }
}
