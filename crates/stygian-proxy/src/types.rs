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

/// Protocol-level capabilities advertised by a proxy endpoint.
///
/// These flags are set when the proxy is registered and consulted during
/// capability-aware selection (see [`crate::manager::ProxyManager::acquire_with_capabilities`]).
///
/// # Example
/// ```
/// use stygian_proxy::types::ProxyCapabilities;
/// let caps = ProxyCapabilities::default();
/// assert!(!caps.supports_https_connect);
/// assert!(!caps.supports_socks5_udp);
/// assert!(!caps.supports_http3_tunnel);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ProxyCapabilities {
    /// Proxy supports the `CONNECT` method for HTTPS tunnelling.
    #[serde(default)]
    pub supports_https_connect: bool,
    /// Proxy supports SOCKS5 with UDP relay (for UDP-based transports).
    #[serde(default)]
    pub supports_socks5_udp: bool,
    /// Proxy supports HTTP/3 (QUIC) tunnelling — future-compatible flag.
    #[serde(default)]
    pub supports_http3_tunnel: bool,
    /// Optional ISO-3166-1 alpha-2 country code for the proxy egress location.
    #[serde(default)]
    pub geo_country: Option<String>,
    /// Confidence score `[0.0, 1.0]` for the geo-location data.
    ///
    /// `None` means the provider did not supply confidence metadata.
    #[serde(default)]
    pub geo_confidence: Option<f32>,
}

impl ProxyCapabilities {
    /// Returns `true` if every required flag in `req` is satisfied by `self`.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::{ProxyCapabilities, CapabilityRequirement};
    /// let caps = ProxyCapabilities { supports_https_connect: true, ..Default::default() };
    /// let req = CapabilityRequirement { require_https_connect: true, ..Default::default() };
    /// assert!(caps.satisfies(&req));
    /// let req2 = CapabilityRequirement { require_socks5_udp: true, ..Default::default() };
    /// assert!(!caps.satisfies(&req2));
    /// ```
    pub fn satisfies(&self, req: &CapabilityRequirement) -> bool {
        if req.require_https_connect && !self.supports_https_connect {
            return false;
        }
        if req.require_socks5_udp && !self.supports_socks5_udp {
            return false;
        }
        if req.require_http3_tunnel && !self.supports_http3_tunnel {
            return false;
        }
        if let Some(ref required_country) = req.require_geo_country
            && self.geo_country.as_deref() != Some(required_country.as_str())
        {
            return false;
        }
        true
    }
}

/// Required capability set used as a filter when acquiring a proxy.
///
/// All fields default to `false`/`None` — an empty requirement matches any proxy.
///
/// # Example
/// ```
/// use stygian_proxy::types::CapabilityRequirement;
/// let req = CapabilityRequirement::default();
/// // empty requirement — any proxy qualifies
/// assert!(!req.require_https_connect);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CapabilityRequirement {
    /// Require `supports_https_connect`.
    #[serde(default)]
    pub require_https_connect: bool,
    /// Require `supports_socks5_udp`.
    #[serde(default)]
    pub require_socks5_udp: bool,
    /// Require `supports_http3_tunnel`.
    #[serde(default)]
    pub require_http3_tunnel: bool,
    /// Require a specific egress country (ISO-3166-1 alpha-2).
    #[serde(default)]
    pub require_geo_country: Option<String>,
}

/// The protocol routing path resolved for an outbound request.
///
/// Returned by [`crate::routing::resolve_routing_path`] to indicate how the
/// proxy should forward the connection.
///
/// # Example
/// ```
/// use stygian_proxy::types::RoutingPath;
/// let path = RoutingPath::H1H2OverTcp;
/// assert_eq!(format!("{path:?}"), "H1H2OverTcp");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingPath {
    /// HTTP/1.1 or HTTP/2 multiplexed over a TCP CONNECT tunnel.
    H1H2OverTcp,
    /// HTTP/3 (QUIC) over a UDP relay — requires `supports_http3_tunnel`.
    H3OverUdp,
}

/// A proxy endpoint with optional authentication credentials.
///
/// `Debug` output masks `password` to prevent accidental credential logging.
///
/// # Example
/// ```
/// use stygian_proxy::types::{Proxy, ProxyType, ProxyCapabilities};
/// let p = Proxy {
///     url: "http://proxy.example.com:8080".into(),
///     proxy_type: ProxyType::Http,
///     username: Some("alice".into()),
///     password: Some("secret".into()),
///     weight: 1,
///     tags: vec!["prod".into()],
///     capabilities: ProxyCapabilities::default(),
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
    /// Protocol-level capabilities advertised by this proxy.
    #[serde(default)]
    pub capabilities: ProxyCapabilities,
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
            .field("capabilities", &self.capabilities)
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
///     capabilities: Default::default(),
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
    /// Cast a `u64` counter to `f64` for ratio computation.
    ///
    /// `u64` can represent values up to ~1.8 × 10¹⁹; `f64` has 53-bit
    /// mantissa, so precision loss begins around 9 × 10¹⁵.  For long-running
    /// proxies that number is never reached in practice, and direct casting
    /// preserves ratios correctly (unlike saturating to `u32::MAX`).
    #[allow(clippy::cast_precision_loss)]
    const fn u64_as_f64(value: u64) -> f64 {
        value as f64
    }

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
        Self::u64_as_f64(self.successes.load(Ordering::Relaxed)) / Self::u64_as_f64(total)
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
        Self::u64_as_f64(self.total_latency_ms.load(Ordering::Relaxed)) / Self::u64_as_f64(total)
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
/// assert!(cfg.profiled_request_mode.is_none());
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
            health_check_interval: Duration::from_mins(1),
            health_check_timeout: Duration::from_secs(5),
            circuit_open_threshold: 5,
            circuit_half_open_after: Duration::from_secs(30),
            sticky_policy: crate::session::StickyPolicy::default(),
            profiled_request_mode: None,
        }
    }
}
