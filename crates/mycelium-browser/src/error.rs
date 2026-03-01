//! Error types for browser automation operations
//!
//! All error variants carry structured context so callers can select retry
//! strategies or surface meaningful diagnostics without string parsing.

use thiserror::Error;

/// Result type alias for browser operations.
pub type Result<T> = std::result::Result<T, BrowserError>;

/// Errors that can occur during browser automation.
///
/// Every variant carries enough structured context to decide on a retry policy
/// or surface a useful diagnostic message without string-parsing.
#[derive(Error, Debug)]
pub enum BrowserError {
    /// Browser process failed to start.
    #[error("Browser launch failed: {reason}")]
    LaunchFailed {
        /// Human-readable explanation of the failure.
        reason: String,
    },

    /// Chrome `DevTools` Protocol (CDP) operation failed.
    #[error("CDP error during '{operation}': {message}")]
    CdpError {
        /// The CDP method or operation that failed.
        operation: String,
        /// Error detail from the protocol layer.
        message: String,
    },

    /// All pool slots are occupied and the wait timeout elapsed.
    #[error("Browser pool exhausted (active={active}, max={max})")]
    PoolExhausted {
        /// Current number of active browser instances.
        active: usize,
        /// Pool capacity limit.
        max: usize,
    },

    /// An operation exceeded its configured timeout.
    #[error("Timeout after {duration_ms}ms during '{operation}'")]
    Timeout {
        /// The operation that timed out.
        operation: String,
        /// Elapsed time in milliseconds.
        duration_ms: u64,
    },

    /// Page navigation failed.
    #[error("Navigation to '{url}' failed: {reason}")]
    NavigationFailed {
        /// Target URL.
        url: String,
        /// Failure reason.
        reason: String,
    },

    /// JavaScript evaluation failed.
    #[error("Script execution failed: {reason}")]
    ScriptExecutionFailed {
        /// Abbreviated script text (first 120 chars).
        script: String,
        /// Error detail.
        reason: String,
    },

    /// WebSocket / transport connection error.
    #[error("Browser connection error: {reason}")]
    ConnectionError {
        /// Connection endpoint (ws:// URL or socket path).
        url: String,
        /// Failure reason.
        reason: String,
    },

    /// Invalid configuration value.
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Underlying I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<chromiumoxide::error::CdpError> for BrowserError {
    fn from(err: chromiumoxide::error::CdpError) -> Self {
        Self::CdpError {
            operation: "unknown".to_string(),
            message: err.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_failed_display() {
        let e = BrowserError::LaunchFailed {
            reason: "binary not found".to_string(),
        };
        assert!(e.to_string().contains("binary not found"));
    }

    #[test]
    fn pool_exhausted_display() {
        let e = BrowserError::PoolExhausted {
            active: 10,
            max: 10,
        };
        assert!(e.to_string().contains("10"));
    }

    #[test]
    fn navigation_failed_includes_url() {
        let e = BrowserError::NavigationFailed {
            url: "https://example.com".to_string(),
            reason: "DNS failure".to_string(),
        };
        assert!(e.to_string().contains("example.com"));
        assert!(e.to_string().contains("DNS failure"));
    }

    #[test]
    fn timeout_display() {
        let e = BrowserError::Timeout {
            operation: "page.load".to_string(),
            duration_ms: 30_000,
        };
        assert!(e.to_string().contains("30000"));
    }

    #[test]
    fn cdp_error_display() {
        let e = BrowserError::CdpError {
            operation: "Page.navigate".to_string(),
            message: "Target closed".to_string(),
        };
        let s = e.to_string();
        assert!(s.contains("Page.navigate"));
        assert!(s.contains("Target closed"));
    }

    #[test]
    fn script_execution_failed_display() {
        let e = BrowserError::ScriptExecutionFailed {
            script: "document.title".to_string(),
            reason: "Execution context destroyed".to_string(),
        };
        assert!(e.to_string().contains("Execution context destroyed"));
    }

    #[test]
    fn connection_error_display() {
        let e = BrowserError::ConnectionError {
            url: "ws://127.0.0.1:9222/json/version".to_string(),
            reason: "connection refused".to_string(),
        };
        let s = e.to_string();
        assert!(s.contains("connection refused"));
    }

    #[test]
    fn config_error_display() {
        let e = BrowserError::ConfigError("pool.max_size must be >= 1".to_string());
        assert!(e.to_string().contains("pool.max_size"));
    }

    #[test]
    fn io_error_wraps_std() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let e = BrowserError::Io(io);
        assert!(e.to_string().contains("file not found"));
    }

    #[test]
    fn launch_failed_is_debug_printable() {
        let e = BrowserError::LaunchFailed { reason: "test".to_string() };
        assert!(!format!("{e:?}").is_empty());
    }

    #[test]
    fn pool_exhausted_reports_both_counts() {
        let e = BrowserError::PoolExhausted { active: 5, max: 5 };
        let s = e.to_string();
        assert!(s.contains("active=5"));
        assert!(s.contains("max=5"));
    }
}

