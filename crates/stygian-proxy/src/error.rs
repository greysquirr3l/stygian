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
}

/// Convenience result alias for all stygian-proxy operations.
pub type ProxyResult<T> = Result<T, ProxyError>;
