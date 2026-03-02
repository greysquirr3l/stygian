//! HTTP scraping adapter with anti-bot features
//!
//! Implements the `ScrapingService` port using reqwest with:
//! - Realistic browser headers and User-Agent rotation
//! - Cookie jar persistence across requests in a session
//! - Exponential backoff retry (up to 3 attempts)
//! - Configurable timeouts
//! - Optional proxy support
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::adapters::http::{HttpAdapter, HttpConfig};
//! use stygian_graph::ports::{ScrapingService, ServiceInput};
//! use serde_json::json;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let adapter = HttpAdapter::with_config(HttpConfig::default());
//! let input = ServiceInput {
//!     url: "https://httpbin.org/get".to_string(),
//!     params: json!({}),
//! };
//! // let result = adapter.execute(input).await.unwrap();
//! # });
//! ```

use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Client, Proxy, header};

use crate::domain::error::{StygianError, Result, ServiceError};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};

/// Rotating pool of realistic browser User-Agent strings
static USER_AGENTS: &[&str] = &[
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:133.0) Gecko/20100101 Firefox/133.0",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 14.7; rv:133.0) Gecko/20100101 Firefox/133.0",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_7_1) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15",
];

/// Configuration for the HTTP adapter
#[derive(Debug, Clone)]
pub struct HttpConfig {
    /// Request timeout (default: 30 seconds)
    pub timeout: Duration,
    /// Number of retry attempts on transient failures (default: 3)
    pub max_retries: u32,
    /// Base delay for exponential backoff (default: 1 second)
    pub retry_base_delay: Duration,
    /// Optional HTTP/SOCKS5 proxy URL
    pub proxy_url: Option<String>,
    /// Whether to rotate User-Agent header on each request
    pub rotate_user_agent: bool,
    /// Index into `USER_AGENTS` for round-robin rotation (wraps)
    pub(crate) ua_counter: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_base_delay: Duration::from_secs(1),
            proxy_url: None,
            rotate_user_agent: true,
            ua_counter: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }
}

/// HTTP client adapter with anti-bot features.
///
/// Thread-safe and cheaply cloneable — the internal `reqwest::Client` uses
/// an `Arc` internally and maintains a shared cookie jar.
#[derive(Clone)]
pub struct HttpAdapter {
    client: Client,
    config: HttpConfig,
}

impl HttpAdapter {
    /// Create a new HTTP adapter with default configuration.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::http::HttpAdapter;
    /// let adapter = HttpAdapter::new();
    /// ```
    pub fn new() -> Self {
        Self::with_config(HttpConfig::default())
    }

    /// Create an HTTP adapter with custom configuration.
    ///
    /// # Panics
    ///
    /// Panics only if TLS configuration is unavailable (extremely rare).
    pub fn with_config(config: HttpConfig) -> Self {
        let mut builder = Client::builder()
            .timeout(config.timeout)
            .cookie_store(true)
            .gzip(true)
            .brotli(true)
            .use_rustls_tls()
            .default_headers(Self::default_headers());

        if let Some(ref proxy_url) = config.proxy_url
            && let Ok(proxy) = Proxy::all(proxy_url)
        {
            builder = builder.proxy(proxy);
        }

        // SAFETY: TLS via rustls is always available; build() can only fail if
        // TLS backend is completely absent, which cannot happen with use_rustls_tls().
        #[allow(clippy::expect_used)]
        let client = builder.build().expect("TLS backend unavailable");

        Self { client, config }
    }

    /// Build a realistic set of browser-like default headers.
    fn default_headers() -> header::HeaderMap {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
            ),
        );
        headers.insert(
            header::ACCEPT_LANGUAGE,
            header::HeaderValue::from_static("en-US,en;q=0.5"),
        );
        headers.insert(
            header::ACCEPT_ENCODING,
            header::HeaderValue::from_static("gzip, deflate, br"),
        );
        headers.insert("DNT", header::HeaderValue::from_static("1"));
        headers.insert(
            "Upgrade-Insecure-Requests",
            header::HeaderValue::from_static("1"),
        );
        headers
    }

    /// Pick the next User-Agent via round-robin.
    fn next_user_agent(&self) -> &'static str {
        let idx = self
            .config
            .ua_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let len = USER_AGENTS.len();
        USER_AGENTS.get(idx % len).copied().unwrap_or("")
    }

    /// Execute a single HTTP GET with the provided URL and return raw content.
    async fn fetch(&self, url: &str) -> Result<(String, serde_json::Value)> {
        let ua = if self.config.rotate_user_agent {
            self.next_user_agent()
        } else {
            USER_AGENTS.first().copied().unwrap_or("")
        };

        let response = self
            .client
            .get(url)
            .header(header::USER_AGENT, ua)
            .send()
            .await
            .map_err(|e| StygianError::Service(ServiceError::Unavailable(e.to_string())))?;

        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/plain")
            .to_string();

        if !status.is_success() {
            return Err(StygianError::Service(ServiceError::Unavailable(format!(
                "HTTP {status} for {url}"
            ))));
        }

        let body = response
            .text()
            .await
            .map_err(|e| StygianError::Service(ServiceError::Unavailable(e.to_string())))?;

        let metadata = serde_json::json!({
            "status_code": status.as_u16(),
            "content_type": content_type,
            "user_agent": ua,
            "url": url,
        });

        Ok((body, metadata))
    }

    /// Check whether a status code is a transient error worth retrying.
    const fn is_retryable_status(code: u16) -> bool {
        matches!(code, 429 | 500 | 502 | 503 | 504)
    }
}

impl Default for HttpAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ScrapingService for HttpAdapter {
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        let mut last_err: Option<StygianError> = None;

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                // Exponential backoff: 1s, 2s, 4s, …
                let delay = self.config.retry_base_delay * 2u32.saturating_pow(attempt - 1);
                tokio::time::sleep(delay).await;
            }

            match self.fetch(&input.url).await {
                Ok((data, metadata)) => {
                    return Ok(ServiceOutput { data, metadata });
                }
                Err(StygianError::Service(ServiceError::Unavailable(ref msg))) => {
                    // Check if we got a retryable HTTP status embedded in the message
                    let retryable = msg
                        .split_whitespace()
                        .find_map(|w| w.parse::<u16>().ok())
                        .is_none_or(Self::is_retryable_status);

                    if retryable && attempt < self.config.max_retries {
                        last_err = Some(StygianError::Service(ServiceError::Unavailable(
                            msg.clone(),
                        )));
                        continue;
                    }
                    return Err(StygianError::Service(ServiceError::Unavailable(
                        msg.clone(),
                    )));
                }
                Err(e) => return Err(e),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            StygianError::Service(ServiceError::Unavailable("Max retries exceeded".into()))
        }))
    }

    fn name(&self) -> &'static str {
        "http"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HttpConfig::default();
        assert_eq!(config.max_retries, 3);
        assert!(config.rotate_user_agent);
        assert!(config.proxy_url.is_none());
    }

    #[test]
    fn test_user_agent_rotation() {
        let adapter = HttpAdapter::new();
        let ua1 = adapter.next_user_agent();
        let ua2 = adapter.next_user_agent();
        // Both should be in the pool
        assert!(USER_AGENTS.contains(&ua1));
        assert!(USER_AGENTS.contains(&ua2));
        // Consecutive calls return different agents
        assert_ne!(ua1, ua2);
    }

    #[test]
    fn test_user_agent_wraps_around() {
        let adapter = HttpAdapter::new();
        // Exhaust one full rotation
        for _ in 0..USER_AGENTS.len() {
            adapter.next_user_agent();
        }
        // Still valid after wrap
        let ua = adapter.next_user_agent();
        assert!(USER_AGENTS.contains(&ua));
    }

    #[test]
    fn test_retryable_status_codes() {
        assert!(HttpAdapter::is_retryable_status(429));
        assert!(HttpAdapter::is_retryable_status(503));
        assert!(!HttpAdapter::is_retryable_status(404));
        assert!(!HttpAdapter::is_retryable_status(200));
    }

    #[test]
    fn test_adapter_name() {
        let adapter = HttpAdapter::new();
        assert_eq!(adapter.name(), "http");
    }
}
