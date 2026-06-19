/// Proxy error types and result alias.
use thiserror::Error;

/// Errors that can occur within the stygian-proxy library.
///
/// # Examples
///
/// ```rust
/// use stygian_proxy::error::{ProxyError, ProxyResult};
///
/// fn example() -> ProxyResult<()> {
///     Err(ProxyError::PoolExhausted)
/// }
/// ```
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProxyError {
    /// The proxy pool has no available proxies to hand out.
    #[error("proxy pool is exhausted")]
    PoolExhausted,

    /// Every proxy in the pool is currently unhealthy or has an open circuit.
    #[error("all proxies are unhealthy")]
    AllProxiesUnhealthy,

    /// A supplied proxy URL failed validation.
    #[error("invalid proxy URL `{url}`: {reason}")]
    InvalidProxyUrl {
        /// The URL that was rejected.
        url: String,
        /// Human-readable explanation of the validation failure.
        reason: String,
    },

    /// A health check request for a proxy failed.
    #[error("health check failed for proxy `{proxy}`: {message}")]
    HealthCheckFailed {
        /// Display form of the proxy URL (credentials redacted).
        proxy: String,
        /// Description of the underlying error.
        message: String,
    },

    /// The circuit breaker for this proxy is open — calls are being rejected fast.
    #[error("circuit breaker is open for proxy `{proxy}`")]
    CircuitOpen {
        /// Display form of the proxy URL (credentials redacted).
        proxy: String,
    },

    /// An error from the underlying storage layer.
    #[error("storage error: {0}")]
    StorageError(String),

    /// A configuration error.
    #[error("configuration error: {0}")]
    ConfigError(String),

    /// A remote proxy list could not be fetched or parsed.
    #[error("proxy fetch failed from `{origin}`: {message}")]
    FetchFailed {
        /// URL or identifier of the remote source.
        origin: String,
        /// Human-readable description of the failure.
        message: String,
    },

    /// No proxy in the pool satisfies the requested capability set.
    #[error("no proxy satisfies the requested capabilities")]
    NoCompatibleProxy,

    /// Coherence validator emitted a hard mismatch on a field registered
    /// for hard-fail behaviour in [`crate::ports::coherence::CoherencePolicy`].
    ///
    /// Only emitted by
    /// [`crate::manager::ProxyManager::acquire_proxy_with_coherence`] when
    /// the `coherence-validation` cargo feature is enabled and the
    /// configured policy lists the offending field as a hard-fail
    /// vector.
    #[error("coherence mismatch on `{field}` ({severity})")]
    CoherenceMismatch {
        /// Which vector disagreed (proxy geo vs DNS, WebRTC /16, …).
        field: crate::ports::coherence::MismatchField,
        /// Severity classification from the validator.
        severity: crate::ports::coherence::MismatchSeverity,
    },

    /// A supplied geo-metadata field on
    /// [`crate::types::ProxyCapabilities`] failed ingest-time
    /// validation.
    ///
    /// Emitted by the storage adapter's `add` path (and by
    /// [`crate::manager::ProxyManager::add_proxy_with_metadata`]) when
    /// `asn`, `city`, or `postal_code` is malformed. Examples:
    /// `asn = 0`, `asn = u32::MAX` (both reserved), `city = ""`,
    /// `postal_code = ""`, or any field exceeding the documented
    /// length ceiling.
    #[error("invalid geo metadata on `{field}`: {reason}")]
    InvalidGeoMetadata {
        /// Which geo field was rejected (`"asn"`, `"city"`, or
        /// `"postal_code"`).
        field: String,
        /// Human-readable explanation of the validation failure.
        reason: String,
    },
}

/// Convenience result alias for all stygian-proxy operations.
pub type ProxyResult<T> = Result<T, ProxyError>;
