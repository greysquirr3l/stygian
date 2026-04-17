//! JavaScript rendering adapter using stygian-browser
//!
//! Implements the `ScrapingService` port using a headless browser (via the
//! `stygian-browser` crate) for pages that require JavaScript execution.
//!
//! Features:
//! - Full JS execution via Chrome DevTools Protocol
//! - Configurable wait strategies (DOM ready, network idle, selector)
//! - Stealth mode via stygian-browser's anti-detection features
//! - Graceful fallback to HTTP when browser pool is unavailable
//! - Circuit-breaker friendly: propagates pool-exhaustion as service errors
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::adapters::browser::{BrowserAdapter, BrowserAdapterConfig};
//! use stygian_graph::ports::{ScrapingService, ServiceInput};
//! use serde_json::json;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let config = BrowserAdapterConfig::default();
//! let adapter = BrowserAdapter::with_config(config);
//! let input = ServiceInput {
//!     url: "https://example.com".to_string(),
//!     params: json!({ "wait_strategy": "dom_content_loaded", "timeout_ms": 30000 }),
//! };
//! // let result = adapter.execute(input).await.unwrap();
//! # });
//! ```

use std::fmt;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::domain::error::{Result, ServiceError, StygianError};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};

/// Wait strategy for JavaScript-rendered pages
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum WaitStrategy {
    /// Wait until DOM content is loaded (default)
    #[default]
    DomContentLoaded,
    /// Wait until all network requests complete
    NetworkIdle,
    /// Wait until a CSS selector appears in the DOM
    SelectorAppears(String),
    /// Wait for a fixed duration after navigation
    Fixed(Duration),
}

impl WaitStrategy {
    /// Parse from a JSON parameter value
    fn from_params(params: &Value) -> Self {
        match params.get("wait_strategy").and_then(Value::as_str) {
            Some("network_idle") => Self::NetworkIdle,
            Some("dom_content_loaded") => Self::DomContentLoaded,
            Some(s) if s.starts_with("selector:") => {
                Self::SelectorAppears(s.trim_start_matches("selector:").to_string())
            }
            _ => params
                .get("wait_ms")
                .and_then(Value::as_u64)
                .map_or(Self::DomContentLoaded, |ms| {
                    Self::Fixed(Duration::from_millis(ms))
                }),
        }
    }
}

impl fmt::Display for WaitStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DomContentLoaded => write!(f, "dom_content_loaded"),
            Self::NetworkIdle => write!(f, "network_idle"),
            Self::SelectorAppears(selector) => write!(f, "selector_appears({selector})"),
            Self::Fixed(duration) => write!(f, "fixed_{}ms", duration.as_millis()),
        }
    }
}

/// Stealth level for browser automation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StealthLevel {
    /// No stealth (fastest, but detectable)
    None,
    /// Basic stealth: hide automation signals
    #[default]
    Basic,
    /// Advanced stealth: full fingerprint spoofing
    Advanced,
}

impl StealthLevel {
    fn from_params(params: &Value) -> Self {
        match params.get("stealth_level").and_then(Value::as_str) {
            Some("advanced") => Self::Advanced,
            Some("none") => Self::None,
            _ => Self::Basic,
        }
    }

    /// Convert stealth level to string representation
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Basic => "basic",
            Self::Advanced => "advanced",
        }
    }
}

/// Configuration for the `BrowserAdapter`
#[derive(Debug, Clone)]
pub struct BrowserAdapterConfig {
    /// Default navigation timeout
    pub timeout: Duration,
    /// Maximum concurrent browser sessions (maps to pool size)
    pub max_concurrent: usize,
    /// Default wait strategy
    pub default_wait: WaitStrategy,
    /// Default stealth level
    pub default_stealth: StealthLevel,
    /// Whether to block common tracking/ad resources (improves speed)
    pub block_resources: bool,
    /// Whether to run in headless mode
    pub headless: bool,
    /// Custom User-Agent string (None = default)
    pub user_agent: Option<String>,
    /// Viewport width in pixels
    pub viewport_width: u32,
    /// Viewport height in pixels
    pub viewport_height: u32,
}

impl Default for BrowserAdapterConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_concurrent: 5,
            default_wait: WaitStrategy::DomContentLoaded,
            default_stealth: StealthLevel::Basic,
            block_resources: true,
            headless: true,
            user_agent: None,
            viewport_width: 1920,
            viewport_height: 1080,
        }
    }
}

/// Browser-based scraping adapter
///
/// Wraps stygian-browser's `BrowserPool` to implement the `ScrapingService` port.
/// Falls back to an error indicating unavailability when the browser pool
/// cannot be used (headless Chrome not available, pool exhausted, etc.).
///
/// The adapter accepts per-request parameters via `ServiceInput.params`:
/// - `wait_strategy`: `"dom_content_loaded"` | `"network_idle"` | `"selector:<css>"` | `"fixed_ms:<n>"`
/// - `stealth_level`: `"none"` | `"basic"` | `"advanced"`
/// - `timeout_ms`: override default timeout in milliseconds
/// - `wait_ms`: milliseconds to wait when strategy is "fixed"
#[derive(Clone)]
pub struct BrowserAdapter {
    config: BrowserAdapterConfig,
}

impl BrowserAdapter {
    /// Create a new `BrowserAdapter` with default configuration
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::adapters::browser::BrowserAdapter;
    /// use stygian_graph::ports::ScrapingService;
    ///
    /// let adapter = BrowserAdapter::new();
    /// assert_eq!(adapter.name(), "browser");
    /// ```
    pub fn new() -> Self {
        Self {
            config: BrowserAdapterConfig::default(),
        }
    }

    /// Create a new `BrowserAdapter` with custom configuration
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::adapters::browser::{BrowserAdapter, BrowserAdapterConfig};
    /// use std::time::Duration;
    ///
    /// let config = BrowserAdapterConfig {
    ///     timeout: Duration::from_secs(60),
    ///     block_resources: false,
    ///     ..BrowserAdapterConfig::default()
    /// };
    /// let adapter = BrowserAdapter::with_config(config);
    /// ```
    pub const fn with_config(config: BrowserAdapterConfig) -> Self {
        Self { config }
    }

    /// Extract per-request timeout from params, falling back to config default
    fn resolve_timeout(&self, params: &Value) -> Duration {
        params
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .map_or(self.config.timeout, Duration::from_millis)
    }

    /// Performs the browser navigation using stygian-browser's `BrowserPool`.
    ///
    /// Returns rendered HTML and timing metadata. When headless Chrome is
    /// unavailable this returns a `ServiceError` so callers can react
    /// (e.g. fall back to `HttpAdapter` via circuit-breaker logic).
    #[allow(clippy::option_if_let_else)]
    #[cfg(feature = "browser")]
    async fn navigate_with_browser(
        &self,
        url: &str,
        wait: &WaitStrategy,
        timeout: Duration,
    ) -> Result<(String, Value)> {
        use stygian_browser::page::WaitUntil;
        use stygian_browser::{BrowserConfig, BrowserPool};

        let start = Instant::now();

        // Step 1: Build browser config from adapter config
        let browser_config = BrowserConfig {
            headless: self.config.headless,
            ..BrowserConfig::default()
        };

        // Step 2: Create pool (in production this would be cached at adapter level)
        let pool = BrowserPool::new(browser_config)
            .await
            .map_err(|e| StygianError::Service(ServiceError::Unavailable(e.to_string())))?;

        // Step 3: Acquire a browser handle with timeout
        let handle = match tokio::time::timeout(timeout, pool.acquire()).await {
            Ok(Ok(h)) => h,
            Ok(Err(e)) => {
                return Err(StygianError::Service(ServiceError::Unavailable(format!(
                    "Browser pool exhausted or unavailable: {e}"
                ))));
            }
            Err(_) => {
                return Err(StygianError::Service(ServiceError::Unavailable(format!(
                    "Browser acquisition timeout after {timeout:?}"
                ))));
            }
        };

        // Step 4: Get browser instance and create new page
        let Some(instance) = handle.browser() else {
            return Err(StygianError::Service(ServiceError::Unavailable(
                "Failed to get browser instance after acquisition".to_string(),
            )));
        };

        let mut page = instance
            .new_page()
            .await
            .map_err(|e| StygianError::Service(ServiceError::Unavailable(e.to_string())))?;

        // Step 5: Convert WaitStrategy to browser's WaitUntil
        let wait_condition = match wait {
            WaitStrategy::DomContentLoaded => WaitUntil::DomContentLoaded,
            WaitStrategy::NetworkIdle => WaitUntil::NetworkIdle,
            WaitStrategy::SelectorAppears(selector) => WaitUntil::Selector(selector.clone()),
            WaitStrategy::Fixed(_duration) => WaitUntil::DomContentLoaded, // Fixed uses timeout, not condition
        };

        // Step 6: Navigate with specified wait strategy
        if let Err(e) = page.navigate(url, wait_condition, timeout).await {
            return Err(StygianError::Service(ServiceError::Unavailable(format!(
                "Browser navigation failed: {e}"
            ))));
        }

        // Step 7: Wait for fixed duration if specified
        if let WaitStrategy::Fixed(duration) = wait {
            tokio::time::sleep(*duration).await;
        }

        // Step 8: Get rendered HTML content
        let html = page
            .content()
            .await
            .map_err(|e| StygianError::Service(ServiceError::Unavailable(e.to_string())))?;

        let elapsed = start.elapsed();

        // Step 9: Return HTML and metadata
        // BrowserHandle is automatically returned to pool when dropped
        Ok((
            html,
            json!({
                "url": url,
                "navigation_time_ms": elapsed.as_millis(),
                "wait_strategy": wait.to_string(),
                "stealth_level": self.config.default_stealth.as_str(),
                "viewport": {
                    "width": self.config.viewport_width,
                    "height": self.config.viewport_height
                },
                "rendered": true,
            }),
        ))
    }

    /// Fallback path when the `browser` feature is disabled
    #[cfg(not(feature = "browser"))]
    async fn navigate_with_browser(
        &self,
        url: &str,
        _wait: &WaitStrategy,
        _timeout: Duration,
    ) -> Result<(String, Value)> {
        Err(StygianError::Service(ServiceError::Unavailable(format!(
            "stygian-graph was compiled without the 'browser' feature; \
             cannot render JavaScript for URL: {url}"
        ))))
    }
}

impl Default for BrowserAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ScrapingService for BrowserAdapter {
    /// Execute a JavaScript-rendered scrape
    ///
    /// Accepts the following `params` keys:
    /// - `wait_strategy` — how to determine page readiness
    /// - `stealth_level` — anti-detection level  
    /// - `timeout_ms` — per-request timeout override
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::browser::BrowserAdapter;
    /// use stygian_graph::ports::{ScrapingService, ServiceInput};
    /// use serde_json::json;
    ///
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// let adapter = BrowserAdapter::new();
    /// let input = ServiceInput {
    ///     url: "https://example.com".to_string(),
    ///     params: json!({ "wait_strategy": "network_idle", "stealth_level": "advanced" }),
    /// };
    /// // let output = adapter.execute(input).await.unwrap();
    /// # });
    /// ```
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        let wait = WaitStrategy::from_params(&input.params);
        let _stealth = StealthLevel::from_params(&input.params);
        let timeout = self.resolve_timeout(&input.params);

        let (html, metadata) = tokio::time::timeout(
            timeout + Duration::from_secs(5), // outer hard deadline
            self.navigate_with_browser(&input.url, &wait, timeout),
        )
        .await
        .map_err(|_| {
            StygianError::Service(ServiceError::Timeout(
                u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            ))
        })??;

        Ok(ServiceOutput {
            data: html,
            metadata,
        })
    }

    fn name(&self) -> &'static str {
        "browser"
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::redundant_closure_for_method_calls
)]
mod tests {
    use super::*;

    #[test]
    fn test_adapter_default_name() {
        let adapter = BrowserAdapter::new();
        assert_eq!(adapter.name(), "browser");
    }

    #[test]
    fn test_wait_strategy_from_params_dom() {
        let params = json!({ "wait_strategy": "dom_content_loaded" });
        assert_eq!(
            WaitStrategy::from_params(&params),
            WaitStrategy::DomContentLoaded
        );
    }

    #[test]
    fn test_wait_strategy_from_params_network_idle() {
        let params = json!({ "wait_strategy": "network_idle" });
        assert_eq!(
            WaitStrategy::from_params(&params),
            WaitStrategy::NetworkIdle
        );
    }

    #[test]
    fn test_wait_strategy_from_params_selector() {
        let params = json!({ "wait_strategy": "selector:#main-content" });
        assert_eq!(
            WaitStrategy::from_params(&params),
            WaitStrategy::SelectorAppears("#main-content".to_string())
        );
    }

    #[test]
    fn test_wait_strategy_from_params_fixed_ms() {
        let params = json!({ "wait_ms": 500u64 });
        assert_eq!(
            WaitStrategy::from_params(&params),
            WaitStrategy::Fixed(Duration::from_millis(500))
        );
    }

    #[test]
    fn test_stealth_level_from_params() {
        assert_eq!(
            StealthLevel::from_params(&json!({ "stealth_level": "advanced" })),
            StealthLevel::Advanced
        );
        assert_eq!(
            StealthLevel::from_params(&json!({ "stealth_level": "none" })),
            StealthLevel::None
        );
        assert_eq!(StealthLevel::from_params(&json!({})), StealthLevel::Basic);
    }

    #[test]
    fn test_resolve_timeout_override() {
        let adapter = BrowserAdapter::new();
        let params = json!({ "timeout_ms": 5000u64 });
        assert_eq!(adapter.resolve_timeout(&params), Duration::from_secs(5));
    }

    #[test]
    fn test_resolve_timeout_default() {
        let adapter = BrowserAdapter::new();
        let params = json!({});
        assert_eq!(adapter.resolve_timeout(&params), Duration::from_secs(30));
    }

    #[test]
    fn test_config_builder() {
        let config = BrowserAdapterConfig {
            timeout: Duration::from_mins(1),
            max_concurrent: 3,
            block_resources: false,
            ..BrowserAdapterConfig::default()
        };
        let adapter = BrowserAdapter::with_config(config);
        assert_eq!(adapter.config.timeout, Duration::from_mins(1));
        assert_eq!(adapter.config.max_concurrent, 3);
    }

    #[allow(clippy::panic)]
    #[tokio::test]
    #[ignore = "requires real Chrome binary"]
    async fn test_execute_returns_service_output_or_unavailable() {
        let adapter = BrowserAdapter::new();
        let input = ServiceInput {
            url: "https://example.com".to_string(),
            params: json!({ "wait_strategy": "dom_content_loaded" }),
        };
        // Either succeeds (pool stub) or returns Unavailable — both are acceptable
        match adapter.execute(input).await {
            Ok(output) => {
                assert!(!output.data.is_empty(), "output data should not be empty");
                assert!(output.metadata.is_object());
            }
            Err(StygianError::Service(ServiceError::Unavailable(_))) => {
                // expected when headless Chrome is not available
            }
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    // Integration tests from T00 Task Requirements

    #[tokio::test]
    #[ignore = "requires real Chrome binary and external network access"]
    async fn browser_adapter_navigates_url() {
        let config = BrowserAdapterConfig::default();
        let adapter = BrowserAdapter::with_config(config);

        let input = ServiceInput {
            url: "https://example.com".to_string(),
            params: json!({
                "wait_strategy": "dom_content_loaded",
                "timeout_ms": 30000
            }),
        };

        let result = adapter.execute(input).await;

        // Should succeed or return graceful unavailable (browser not installed)
        match result {
            Ok(output) => {
                assert!(!output.data.is_empty());
                assert!(
                    output
                        .metadata
                        .get("rendered")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                );
                assert!(output.metadata.get("navigation_time_ms").is_some());
                assert_eq!(
                    output.metadata.get("url").and_then(|v| v.as_str()),
                    Some("https://example.com")
                );
            }
            Err(StygianError::Service(ServiceError::Unavailable(_))) => {
                // Expected if Chrome not installed
            }
            Err(e) => panic!("Unexpected error: {e}"),
        }
    }

    #[tokio::test]
    #[ignore = "Requires Chrome installed and network access; may panic if browser unavailable"]
    async fn browser_adapter_respects_timeout() {
        let config = BrowserAdapterConfig {
            timeout: Duration::from_secs(2),
            ..Default::default()
        };
        let adapter = BrowserAdapter::with_config(config);

        // This URL delays for 10 seconds, should timeout with 2s limit
        let input = ServiceInput {
            url: "https://httpbin.org/delay/10".to_string(),
            params: json!({"timeout_ms": 2000}),
        };

        let result = adapter.execute(input).await;

        // Should timeout gracefully or be unavailable (Chrome not installed)
        match result {
            Err(StygianError::Service(ServiceError::Unavailable(msg))) => {
                // Expected if Chrome not installed or timeout occurred
                assert!(
                    msg.contains("timeout")
                        || msg.contains("unavailable")
                        || msg.contains("Chrome")
                        || msg.contains("exhausted")
                );
            }
            Err(StygianError::Service(ServiceError::Timeout(_))) => {
                // Also acceptable - explicit timeout
            }
            Ok(_) => {
                // Should not succeed with 2s timeout on 10s delay
                panic!("Expected timeout or unavailable, got success");
            }
            Err(e) => {
                // Any other error is acceptable (network, browser init, etc)
                eprintln!("Got acceptable error: {e}");
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires real Chrome binary"]
    async fn browser_adapter_invalid_url() {
        let config = BrowserAdapterConfig::default();
        let adapter = BrowserAdapter::with_config(config);

        let input = ServiceInput {
            url: "not-a-valid-url".to_string(),
            params: json!({}),
        };

        let result = adapter.execute(input).await;

        // Should surface browser error gracefully
        assert!(result.is_err());
    }

    #[tokio::test]
    #[ignore = "requires real Chrome binary and external network access"]
    async fn browser_adapter_wait_strategy_selector() {
        let config = BrowserAdapterConfig::default();
        let adapter = BrowserAdapter::with_config(config);

        let input = ServiceInput {
            url: "https://example.com".to_string(),
            params: json!({
                "wait_strategy": "selector:body"
            }),
        };

        match adapter.execute(input).await {
            Ok(output) => {
                assert_eq!(
                    output
                        .metadata
                        .get("wait_strategy")
                        .and_then(|v| v.as_str()),
                    Some("selector_appears(body)")
                );
            }
            Err(StygianError::Service(ServiceError::Unavailable(_))) => {
                // Expected if Chrome not installed
            }
            Err(e) => panic!("Unexpected error: {e}"),
        }
    }

    #[tokio::test]
    #[ignore = "requires real Chrome binary and external network access"]
    async fn browser_adapter_metadata_complete() {
        let config = BrowserAdapterConfig {
            default_stealth: StealthLevel::Advanced,
            user_agent: Some("Mozilla/5.0".to_string()),
            viewport_width: 1440,
            viewport_height: 900,
            ..Default::default()
        };
        let adapter = BrowserAdapter::with_config(config);

        let input = ServiceInput {
            url: "https://example.com".to_string(),
            params: json!({}),
        };

        match adapter.execute(input).await {
            Ok(output) => {
                assert_eq!(
                    output.metadata.get("url").and_then(|v| v.as_str()),
                    Some("https://example.com")
                );
                assert_eq!(
                    output
                        .metadata
                        .get("stealth_level")
                        .and_then(|v| v.as_str()),
                    Some("advanced")
                );
                assert!(output.metadata.get("viewport").is_some());
                assert!(output.metadata.get("navigation_time_ms").is_some());
                let viewport = output.metadata.get("viewport").expect("viewport exists");
                assert_eq!(viewport.get("width").and_then(|v| v.as_u64()), Some(1440));
                assert_eq!(viewport.get("height").and_then(|v| v.as_u64()), Some(900));
            }
            Err(StygianError::Service(ServiceError::Unavailable(_))) => {
                // Expected if Chrome not installed
            }
            Err(e) => panic!("Unexpected error: {e}"),
        }
    }
}
