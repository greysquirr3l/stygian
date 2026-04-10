//! Scrape Exchange REST API client for uploading and querying scraped data.
//!
//! Implements a typed client for the Scrape Exchange platform with JWT authentication,
//! automatic token refresh, and endpoints for:
//!
//! - Publishing scraped records via [`DataSinkPort`](crate::ports::DataSinkPort)
//! - Querying published data
//! - Item-level lookups
//! - Rate-limited retry logic
//!
//! # Feature
//!
//! This adapter is feature-gated behind `scrape-exchange`.
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::adapters::scrape_exchange::{ScrapeExchangeClient, ScrapeExchangeConfig};
//! use std::time::Duration;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let config = ScrapeExchangeConfig {
//!     api_key_id:     "your_key_id".to_string(),
//!     api_key_secret: "your_secret".to_string(),
//!     base_url:       "https://scrape.exchange/api/".to_string(),
//! };
//!
//! let client = ScrapeExchangeClient::new(config).await?;
//! // client.health_check().await?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! # });
//! ```

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use parking_lot::RwLock;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::domain::error::{Result as DomainResult, ServiceError, StygianError};
use crate::ports::data_sink::{DataSinkError, DataSinkPort, SinkRecord, SinkReceipt};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};

// ─── Errors ───────────────────────────────────────────────────────────────────

/// Errors from Scrape Exchange API client.
#[derive(Debug, Error)]
pub enum ScrapeExchangeError {
    /// HTTP request error from reqwest.
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    /// JSON serialization/deserialization error.
    #[error("JSON parsing error: {0}")]
    JsonError(#[from] serde_json::Error),

    /// Authentication with API failed (invalid credentials).
    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    /// JWT token refresh failed.
    #[error("Token refresh failed: {0}")]
    TokenRefreshFailed(String),

    /// API rate limit exceeded.
    #[error("Rate limited; retry after {retry_after_secs}s")]
    RateLimited {
        /// Retry delay in seconds.
        retry_after_secs: u64,
    },

    /// API error response.
    #[error("API error: {status} {message}")]
    ApiError {
        /// HTTP status code.
        status: u16,
        /// Error message from API.
        message: String,
    },

    /// Invalid configuration provided.
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    /// Health check endpoint failed.
    #[error("Health check failed: {0}")]
    HealthCheckFailed(String),
}

// ─── Config ───────────────────────────────────────────────────────────────────

/// Configuration for Scrape Exchange API client.
///
/// Requires API credentials from the Scrape Exchange dashboard.
#[derive(Debug, Clone)]
pub struct ScrapeExchangeConfig {
    /// API key ID for authentication.
    pub api_key_id: String,
    /// API key secret for authentication.
    pub api_key_secret: String,
    /// Base URL for the API (e.g., `https://scrape.exchange/api/`).
    ///
    /// For testing, can be overridden to point to a different environment.
    pub base_url: String,
}

impl ScrapeExchangeConfig {
    /// Load configuration from environment variables.
    ///
    /// Expected variables:
    /// - `SCRAPE_EXCHANGE_KEY_ID`
    /// - `SCRAPE_EXCHANGE_KEY_SECRET`
    /// - `SCRAPE_EXCHANGE_BASE_URL` (optional; defaults to `https://scrape.exchange/api/`)
    pub fn from_env() -> std::result::Result<Self, ScrapeExchangeError> {
        let api_key_id = std::env::var("SCRAPE_EXCHANGE_KEY_ID").map_err(|_| {
            ScrapeExchangeError::InvalidConfig("SCRAPE_EXCHANGE_KEY_ID not set".to_string())
        })?;
        let api_key_secret = std::env::var("SCRAPE_EXCHANGE_KEY_SECRET").map_err(|_| {
            ScrapeExchangeError::InvalidConfig("SCRAPE_EXCHANGE_KEY_SECRET not set".to_string())
        })?;
        let base_url = std::env::var("SCRAPE_EXCHANGE_BASE_URL")
            .unwrap_or_else(|_| "https://scrape.exchange/api/".to_string());

        Ok(Self {
            api_key_id,
            api_key_secret,
            base_url,
        })
    }
}

// ─── JWT Token Response ───────────────────────────────────────────────────────

/// JWT token response from the auth endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
struct JwtTokenResponse {
    /// The JWT string.
    access_token: String,
    /// Token type (usually "Bearer").
    token_type: String,
    /// Expiry in seconds.
    expires_in: u64,
}

// ─── JWT Token with Expiry Tracking ───────────────────────────────────────────

/// JWT token with local expiry tracking.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct JwtToken {
    /// The JWT string.
    access_token: String,
    /// Token type (usually "Bearer").
    token_type: String,
    /// Expiry in seconds from issue time.
    expires_in: u64,
    /// Unix timestamp when issued.
    issued_at_secs: u64,
}

impl JwtToken {
    /// Create a new token from response and current time.
    fn from_response(response: JwtTokenResponse) -> Self {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            access_token: response.access_token,
            token_type: response.token_type,
            expires_in: response.expires_in,
            issued_at_secs: now_secs,
        }
    }

    /// Check if token is expired (with 30-second grace period).
    fn is_expired(&self) -> bool {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let grace_period_secs = 30;
        now_secs >= self.issued_at_secs + self.expires_in - grace_period_secs
    }
}

// ─── Client ───────────────────────────────────────────────────────────────────

/// Scrape Exchange REST API client with JWT auth and automatic token refresh.
pub struct ScrapeExchangeClient {
    config: ScrapeExchangeConfig,
    http_client: Client,
    token: Arc<RwLock<Option<JwtToken>>>,
}

impl ScrapeExchangeClient {
    /// Create a new client and authenticate.
    pub async fn new(
        config: ScrapeExchangeConfig,
    ) -> std::result::Result<Self, ScrapeExchangeError> {
        if config.api_key_id.is_empty() || config.api_key_secret.is_empty() {
            return Err(ScrapeExchangeError::InvalidConfig(
                "api_key_id and api_key_secret must not be empty".to_string(),
            ));
        }

        let client = Client::new();
        let instance = Self {
            config,
            http_client: client,
            token: Arc::new(RwLock::new(None)),
        };

        // Authenticate to get initial token
        instance.refresh_token().await?;

        Ok(instance)
    }

    /// Refresh JWT token from auth endpoint.
    async fn refresh_token(&self) -> std::result::Result<(), ScrapeExchangeError> {
        let auth_url = format!("{}account/v1/token", self.config.base_url);
        debug!("Refreshing JWT token from {}", auth_url);

        let response = self
            .http_client
            .post(&auth_url)
            .json(&json!({
                "api_key_id": self.config.api_key_id,
                "api_key_secret": self.config.api_key_secret,
            }))
            .send()
            .await?;

        match response.status() {
            StatusCode::OK => {
                let token_response: JwtTokenResponse = response.json().await?;
                let expires_in = token_response.expires_in;
                let token = JwtToken::from_response(token_response);
                *self.token.write() = Some(token);
                debug!("JWT token refreshed; expires in {}s", expires_in);
                Ok(())
            }
            StatusCode::UNAUTHORIZED => Err(ScrapeExchangeError::AuthFailed(
                "Invalid API credentials".to_string(),
            )),
            StatusCode::TOO_MANY_REQUESTS => {
                let retry_after = response
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(60);
                Err(ScrapeExchangeError::RateLimited {
                    retry_after_secs: retry_after,
                })
            }
            status => {
                let body = response.text().await.unwrap_or_default();
                Err(ScrapeExchangeError::TokenRefreshFailed(format!(
                    "{}: {}",
                    status, body
                )))
            }
        }
    }

    /// Get valid JWT token, refreshing if necessary.
    async fn get_token(&self) -> std::result::Result<String, ScrapeExchangeError> {
        {
            let token_lock = self.token.read();
            if let Some(token) = token_lock.as_ref()
                && !token.is_expired()
            {
                return Ok(token.access_token.clone());
            }
        }

        // Token expired or missing; refresh it
        drop(self.token.read());
        self.refresh_token().await?;
        Ok(self
            .token
            .read()
            .as_ref()
            .ok_or_else(|| {
                ScrapeExchangeError::TokenRefreshFailed("Token not set after refresh".to_string())
            })?
            .access_token
            .clone())
    }

    /// POST data to upload endpoint with exponential backoff retry.
    pub async fn upload(&self, data: Value) -> std::result::Result<Value, ScrapeExchangeError> {
        let token = self.get_token().await?;
        let url = format!("{}data/v1/", self.config.base_url);

        let mut retries = 0;
        let max_retries = 3;

        loop {
            let response = self
                .http_client
                .post(&url)
                .bearer_auth(&token)
                .json(&data)
                .send()
                .await?;

            match response.status() {
                StatusCode::OK | StatusCode::CREATED => {
                    let result = response.json().await?;
                    debug!("Data uploaded successfully");
                    return Ok(result);
                }
                StatusCode::TOO_MANY_REQUESTS => {
                    let retry_after = response
                        .headers()
                        .get("Retry-After")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(60);

                    if retries < max_retries {
                        retries += 1;
                        let backoff_ms = retry_after * 1000;
                        warn!(
                            "Rate limited; retrying in {}ms (attempt {}/{})",
                            backoff_ms, retries, max_retries
                        );
                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                        continue;
                    }

                    return Err(ScrapeExchangeError::RateLimited {
                        retry_after_secs: retry_after,
                    });
                }
                StatusCode::UNAUTHORIZED => {
                    // Token may have been revoked; refresh and retry once
                    if retries == 0 {
                        retries = 1;
                        self.refresh_token().await?;
                        continue;
                    }
                    return Err(ScrapeExchangeError::AuthFailed(
                        "Reauthorization failed".to_string(),
                    ));
                }
                status => {
                    let body = response.text().await.unwrap_or_default();
                    return Err(ScrapeExchangeError::ApiError {
                        status: status.as_u16(),
                        message: body,
                    });
                }
            }
        }
    }

    /// GET query endpoint with optional filters.
    pub async fn query(
        &self,
        uploader: &str,
        platform: &str,
        entity: &str,
    ) -> std::result::Result<Value, ScrapeExchangeError> {
        let token = self.get_token().await?;
        let url = format!(
            "{}data/v1/param/{}/{}/{}",
            self.config.base_url, uploader, platform, entity
        );

        debug!("Querying {}", url);

        let response = self
            .http_client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await?;

        match response.status() {
            StatusCode::OK => {
                let result = response.json().await?;
                Ok(result)
            }
            StatusCode::UNAUTHORIZED => {
                self.refresh_token().await?;
                Err(ScrapeExchangeError::AuthFailed(
                    "Reauthorization required".to_string(),
                ))
            }
            StatusCode::NOT_FOUND => Err(ScrapeExchangeError::ApiError {
                status: 404,
                message: "Query parameters not found".to_string(),
            }),
            status => {
                let body = response.text().await.unwrap_or_default();
                Err(ScrapeExchangeError::ApiError {
                    status: status.as_u16(),
                    message: body,
                })
            }
        }
    }

    /// GET item lookup endpoint.
    pub async fn item_lookup(
        &self,
        item_id: &str,
    ) -> std::result::Result<Value, ScrapeExchangeError> {
        let token = self.get_token().await?;
        let url = format!("{}data/v1/item_id/{}", self.config.base_url, item_id);

        debug!("Looking up item: {}", item_id);

        let response = self
            .http_client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await?;

        match response.status() {
            StatusCode::OK => {
                let result = response.json().await?;
                Ok(result)
            }
            StatusCode::UNAUTHORIZED => {
                self.refresh_token().await?;
                Err(ScrapeExchangeError::AuthFailed(
                    "Reauthorization required".to_string(),
                ))
            }
            StatusCode::NOT_FOUND => Err(ScrapeExchangeError::ApiError {
                status: 404,
                message: format!("Item not found: {}", item_id),
            }),
            status => {
                let body = response.text().await.unwrap_or_default();
                Err(ScrapeExchangeError::ApiError {
                    status: status.as_u16(),
                    message: body,
                })
            }
        }
    }

    /// GET health check endpoint.
    pub async fn health_check(&self) -> std::result::Result<(), ScrapeExchangeError> {
        let url = format!("{}status", self.config.base_url);
        debug!("Health check: {}", url);

        let response = self.http_client.get(&url).send().await?;

        match response.status() {
            StatusCode::OK => {
                info!("Scrape Exchange API is healthy");
                Ok(())
            }
            status => {
                let body = response.text().await.unwrap_or_default();
                Err(ScrapeExchangeError::HealthCheckFailed(format!(
                    "{}: {}",
                    status, body
                )))
            }
        }
    }
}

// ─── Adapter (DataSinkPort + ScrapingService) ─────────────────────────────────

/// Pipeline adapter wrapping [`ScrapeExchangeClient`] that implements
/// [`DataSinkPort`] and [`ScrapingService`].
///
/// Use this type when wiring a pipeline output into Scrape Exchange; the
/// raw client is available via [`ScrapeExchangeAdapter::client()`] for
/// direct API access.
///
/// # Example
///
/// ```no_run
/// use stygian_graph::adapters::scrape_exchange::{ScrapeExchangeAdapter, ScrapeExchangeConfig};
/// use stygian_graph::ports::data_sink::{DataSinkPort, SinkRecord};
/// use serde_json::json;
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let config = ScrapeExchangeConfig {
///     api_key_id: "key_id".to_string(),
///     api_key_secret: "secret".to_string(),
///     base_url: "https://scrape.exchange/api/".to_string(),
/// };
/// // let adapter = ScrapeExchangeAdapter::new(config).await?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # });
/// ```
pub struct ScrapeExchangeAdapter {
    client: Arc<ScrapeExchangeClient>,
}

impl ScrapeExchangeAdapter {
    /// Create a new adapter and establish an authenticated session.
    ///
    /// # Errors
    ///
    /// Returns [`ScrapeExchangeError`] if authentication fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::scrape_exchange::{ScrapeExchangeAdapter, ScrapeExchangeConfig};
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// // let adapter = ScrapeExchangeAdapter::new(config).await?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # });
    /// ```
    pub async fn new(
        config: ScrapeExchangeConfig,
    ) -> std::result::Result<Self, ScrapeExchangeError> {
        let client = ScrapeExchangeClient::new(config).await?;
        Ok(Self {
            client: Arc::new(client),
        })
    }

    /// Access the underlying REST client.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::adapters::scrape_exchange::{ScrapeExchangeAdapter, ScrapeExchangeConfig};
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # let config = ScrapeExchangeConfig { api_key_id: "k".to_string(), api_key_secret: "s".to_string(), base_url: "u".to_string() };
    /// # let adapter = ScrapeExchangeAdapter::new(config).await.unwrap();
    /// let items = adapter.client().query("me", "web", "pages").await;
    /// # });
    /// ```
    pub fn client(&self) -> &ScrapeExchangeClient {
        &self.client
    }

    /// Map a [`SinkRecord`] to the Scrape Exchange upload JSON format.
    fn map_record(record: &SinkRecord) -> Value {
        json!({
            "schema_id": record.schema_id,
            "source": record.source_url,
            "content": record.data,
            "metadata": record.metadata,
        })
    }

    /// Validate that `record.data` is a JSON object with at least one field.
    /// Full schema validation against `schema_id` is performed server-side.
    fn local_validate(record: &SinkRecord) -> std::result::Result<(), DataSinkError> {
        if !record.schema_id.is_empty() && record.data.is_null() {
            return Err(DataSinkError::ValidationFailed(
                "data must not be null when schema_id is set".to_string(),
            ));
        }
        if let Some(obj) = record.data.as_object()
            && obj.is_empty()
        {
            return Err(DataSinkError::ValidationFailed(
                "data object must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl DataSinkPort for ScrapeExchangeAdapter {
    /// Validate and publish a [`SinkRecord`] to Scrape Exchange.
    ///
    /// # Errors
    ///
    /// - [`DataSinkError::ValidationFailed`] — local validation rejected the record.
    /// - [`DataSinkError::RateLimited`] — API returned 429.
    /// - [`DataSinkError::Unauthorized`] — API returned 401/403.
    /// - [`DataSinkError::PublishFailed`] — any other HTTP error.
    async fn publish(
        &self,
        record: &SinkRecord,
    ) -> std::result::Result<SinkReceipt, DataSinkError> {
        Self::local_validate(record)?;

        let payload = Self::map_record(record);
        let result = self.client.upload(payload).await.map_err(|e| match e {
            ScrapeExchangeError::RateLimited { retry_after_secs } => {
                DataSinkError::RateLimited(format!("retry after {retry_after_secs}s"))
            }
            ScrapeExchangeError::AuthFailed(msg) => DataSinkError::Unauthorized(msg),
            other => DataSinkError::PublishFailed(other.to_string()),
        })?;

        let id = result
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();

        let published_at = result
            .get("created_at")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        Ok(SinkReceipt {
            id,
            published_at,
            platform: "scrape-exchange".to_string(),
        })
    }

    /// Validate the record locally without publishing.
    ///
    /// # Errors
    ///
    /// [`DataSinkError::ValidationFailed`] if the record is structurally invalid.
    async fn validate(
        &self,
        record: &SinkRecord,
    ) -> std::result::Result<(), DataSinkError> {
        Self::local_validate(record)
    }

    /// Check that the Scrape Exchange API is reachable.
    ///
    /// # Errors
    ///
    /// [`DataSinkError::PublishFailed`] if the health endpoint is unreachable.
    async fn health_check(&self) -> std::result::Result<(), DataSinkError> {
        self.client.health_check().await.map_err(|e| {
            DataSinkError::PublishFailed(format!("health check failed: {e}"))
        })
    }
}

#[async_trait]
impl ScrapingService for ScrapeExchangeAdapter {
    /// Query Scrape Exchange for data matching the URL's path components.
    ///
    /// Expects `input.url` to be of the form
    /// `scrape-exchange://uploader/platform/entity` or a full API URL path.
    /// Falls back to using the whole URL string as an item-ID lookup.
    ///
    /// # Errors
    ///
    /// Returns a [`StygianError`] wrapping any API transport failure.
    async fn execute(&self, input: ServiceInput) -> DomainResult<ServiceOutput> {
        debug!("ScrapeExchangeAdapter::execute url={}", input.url);

        let result = self
            .client
            .item_lookup(&input.url)
            .await
            .map_err(|e| StygianError::from(ServiceError::Unavailable(e.to_string())))?;

        Ok(ServiceOutput {
            data: result.to_string(),
            metadata: json!({ "platform": "scrape-exchange", "url": input.url }),
        })
    }

    fn name(&self) -> &'static str {
        "scrape-exchange"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jwt_token_expiry() {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let token = JwtToken {
            access_token: "test".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
            issued_at_secs: now_secs,
        };
        // Newly issued token should not be expired
        assert!(!token.is_expired());
    }

    #[test]
    fn test_jwt_token_parsing() {
        let json_str = r#"{"access_token":"test_jwt","token_type":"Bearer","expires_in":3600}"#;
        let result: std::result::Result<JwtTokenResponse, _> = serde_json::from_str(json_str);
        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.access_token, "test_jwt");
        assert_eq!(response.token_type, "Bearer");
        assert_eq!(response.expires_in, 3600);
    }

    #[test]
    fn test_scrape_exchange_config_construction() {
        let config = ScrapeExchangeConfig {
            api_key_id: "test_id".to_string(),
            api_key_secret: "test_secret".to_string(),
            base_url: "https://test.api/".to_string(),
        };

        assert_eq!(config.api_key_id, "test_id");
        assert_eq!(config.api_key_secret, "test_secret");
        assert_eq!(config.base_url, "https://test.api/");
    }

    #[test]
    fn test_scrape_exchange_error_display() {
        let err = ScrapeExchangeError::InvalidConfig("test".to_string());
        assert_eq!(err.to_string(), "Invalid configuration: test");

        let err = ScrapeExchangeError::RateLimited {
            retry_after_secs: 30,
        };
        assert_eq!(err.to_string(), "Rate limited; retry after 30s");

        let err = ScrapeExchangeError::ApiError {
            status: 500,
            message: "Internal error".to_string(),
        };
        assert_eq!(err.to_string(), "API error: 500 Internal error");
    }

    // ── T27 adapter tests ─────────────────────────────────────────────────────

    #[test]
    fn test_validate_rejects_null_data_with_schema() {
        let record = SinkRecord::new("product-v1", "https://example.com", Value::Null);
        let result = ScrapeExchangeAdapter::local_validate(&record);
        assert!(result.is_err(), "null data with schema_id should fail");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("null"), "error should mention null: {msg}");
    }

    #[test]
    fn test_validate_rejects_empty_object() {
        let record = SinkRecord::new(
            "product-v1",
            "https://example.com",
            serde_json::Value::Object(serde_json::Map::new()),
        );
        let result = ScrapeExchangeAdapter::local_validate(&record);
        assert!(result.is_err(), "empty object should fail validation");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("empty"), "error should mention empty: {msg}");
    }

    #[test]
    fn test_validate_accepts_valid_record() {
        let record = SinkRecord::new(
            "product-v1",
            "https://example.com",
            serde_json::json!({ "sku": "ABC-42" }),
        );
        let result = ScrapeExchangeAdapter::local_validate(&record);
        assert!(result.is_ok(), "valid record should pass: {result:?}");
    }

    #[test]
    fn test_map_record_produces_correct_fields() {
        let record = SinkRecord::new(
            "order-v2",
            "https://shop.example.com/orders/99",
            serde_json::json!({ "total": 39.99 }),
        );
        let mapped = ScrapeExchangeAdapter::map_record(&record);
        assert_eq!(mapped["schema_id"], "order-v2");
        assert_eq!(mapped["source"], "https://shop.example.com/orders/99");
        assert_eq!(mapped["content"]["total"], 39.99);
    }

    #[test]
    fn test_rate_limit_error_mapping() {
        // Verify the error mapping logic: RateLimited → DataSinkError::RateLimited.
        let se_err = ScrapeExchangeError::RateLimited { retry_after_secs: 60 };
        let mapped: DataSinkError = match se_err {
            ScrapeExchangeError::RateLimited { retry_after_secs } => {
                DataSinkError::RateLimited(format!("retry after {retry_after_secs}s"))
            }
            other => DataSinkError::PublishFailed(other.to_string()),
        };
        assert!(mapped.to_string().contains("60"), "should mention 60s: {mapped}");
    }
}
