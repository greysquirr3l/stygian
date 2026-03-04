//! Port trait definitions - service abstractions
//!
//! Defines interfaces that adapters must implement.
//! Following hexagonal architecture, these are the "ports" that connect
//! domain logic to external infrastructure.

use crate::domain::error::Result;
use async_trait::async_trait;
use serde_json::Value;

/// Input to a scraping service
///
/// Contains the target URL and service-specific parameters that configure
/// how the scraping operation should be performed.
///
/// # Example
///
/// ```
/// use stygian_graph::ports::ServiceInput;
/// use serde_json::json;
///
/// let input = ServiceInput {
///     url: "https://example.com".to_string(),
///     params: json!({
///         "timeout_ms": 5000,
///         "user_agent": "stygian/1.0"
///     }),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct ServiceInput {
    /// Target URL to scrape
    pub url: String,
    /// Service-specific parameters (timeout, headers, etc.)
    pub params: Value,
}

/// Output from a scraping service
///
/// Contains the raw scraped data and metadata about the operation
/// for downstream processing and debugging.
///
/// # Example
///
/// ```
/// use stygian_graph::ports::ServiceOutput;
/// use serde_json::json;
///
/// let output = ServiceOutput {
///     data: "<html>...</html>".to_string(),
///     metadata: json!({
///         "status_code": 200,
///         "content_type": "text/html",
///         "response_time_ms": 145
///     }),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct ServiceOutput {
    /// Raw scraped data (HTML, JSON, binary, etc.)
    pub data: String,
    /// Metadata about the operation (status, timing, headers)
    pub metadata: Value,
}

/// Primary port: `ScrapingService` trait
///
/// All scraping modules (HTTP, browser, JavaScript rendering) implement this trait.
/// Uses `async_trait` for dyn compatibility with service registry.
///
/// # Example Implementation
///
/// ```no_run
/// use stygian_graph::ports::{ScrapingService, ServiceInput, ServiceOutput};
/// use stygian_graph::error::Result;
/// use async_trait::async_trait;
/// use serde_json::json;
///
/// struct MyService;
///
/// #[async_trait]
/// impl ScrapingService for MyService {
///     async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
///         Ok(ServiceOutput {
///             data: format!("Scraped: {}", input.url),
///             metadata: json!({"status": "ok"}),
///         })
///     }
///     
///     fn name(&self) -> &'static str {
///         "my-service"
///     }
/// }
/// ```
#[async_trait]
pub trait ScrapingService: Send + Sync {
    /// Execute the scraping operation
    ///
    /// # Arguments
    ///
    /// * `input` - Service input containing URL and parameters
    ///
    /// # Returns
    ///
    /// * `Ok(ServiceOutput)` - Successful scraping result
    /// * `Err(StygianError)` - Service error, timeout, or network failure
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::{ScrapingService, ServiceInput};
    /// # use serde_json::json;
    /// # async fn example(service: impl ScrapingService) {
    /// let input = ServiceInput {
    ///     url: "https://example.com".to_string(),
    ///     params: json!({}),
    /// };
    /// let output = service.execute(input).await.unwrap();
    /// println!("Data: {}", output.data);
    /// # }
    /// ```
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput>;

    /// Service name for identification in logs and metrics
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::ScrapingService;
    /// # fn example(service: impl ScrapingService) {
    /// println!("Using service: {}", service.name());
    /// # }
    /// ```
    fn name(&self) -> &'static str;
}

/// Provider capability flags
///
/// Describes the capabilities supported by an AI provider.
///
/// # Example
///
/// ```
/// use stygian_graph::ports::ProviderCapabilities;
///
/// let caps = ProviderCapabilities {
///     streaming: true,
///     vision: false,
///     tool_use: true,
///     json_mode: true,
/// };
/// assert!(caps.streaming, "Provider supports streaming");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProviderCapabilities {
    /// Supports streaming responses
    pub streaming: bool,
    /// Supports image/video analysis
    pub vision: bool,
    /// Supports function calling/tool use
    pub tool_use: bool,
    /// Native JSON output mode
    pub json_mode: bool,
}

/// AI Provider port for LLM-based extraction
///
/// All LLM providers (Claude, GPT, Gemini, GitHub Copilot, Ollama) implement this trait.
/// Uses `async_trait` for dyn compatibility with service registry.
///
/// # Example Implementation
///
/// ```no_run
/// use stygian_graph::ports::{AIProvider, ProviderCapabilities};
/// use stygian_graph::domain::error::Result;
/// use async_trait::async_trait;
/// use serde_json::{json, Value};
/// use futures::stream::{Stream, BoxStream};
///
/// struct MyProvider;
///
/// #[async_trait]
/// impl AIProvider for MyProvider {
///     async fn extract(&self, content: String, schema: Value) -> Result<Value> {
///         Ok(json!({"extracted": "data"}))
///     }
///
///     async fn stream_extract(
///         &self,
///         content: String,
///         schema: Value,
///     ) -> Result<BoxStream<'static, Result<Value>>> {
///         unimplemented!("Streaming not supported")
///     }
///
///     fn capabilities(&self) -> ProviderCapabilities {
///         ProviderCapabilities {
///             streaming: false,
///             vision: false,
///             tool_use: true,
///             json_mode: true,
///         }
///     }
///
///     fn name(&self) -> &'static str {
///         "my-provider"
///     }
/// }
/// ```
#[async_trait]
pub trait AIProvider: Send + Sync {
    /// Extract structured data from content using LLM
    ///
    /// # Arguments
    ///
    /// * `content` - Raw content to analyze (text, HTML, etc.)
    /// * `schema` - JSON schema defining expected output structure
    ///
    /// # Returns
    ///
    /// * `Ok(Value)` - Extracted data matching the schema
    /// * `Err(ProviderError)` - API error, token limit, or policy violation
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::AIProvider;
    /// # use serde_json::json;
    /// # async fn example(provider: impl AIProvider) {
    /// let schema = json!({
    ///     "type": "object",
    ///     "properties": {
    ///         "title": {"type": "string"},
    ///         "price": {"type": "number"}
    ///     }
    /// });
    /// let result = provider.extract("<html>...</html>".to_string(), schema).await.unwrap();
    /// println!("Extracted: {}", result);
    /// # }
    /// ```
    async fn extract(&self, content: String, schema: Value) -> Result<Value>;

    /// Stream structured data extraction for real-time processing
    ///
    /// Returns a stream of partial results as they arrive from the LLM.
    /// Only supported by providers with `capabilities().streaming == true`.
    ///
    /// # Arguments
    ///
    /// * `content` - Raw content to analyze
    /// * `schema` - JSON schema defining expected output structure
    ///
    /// # Returns
    ///
    /// * `Ok(BoxStream)` - Stream of partial extraction results
    /// * `Err(ProviderError)` - If streaming is not supported or API error
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::AIProvider;
    /// # use serde_json::json;
    /// # use futures::StreamExt;
    /// # async fn example(provider: impl AIProvider) {
    /// let schema = json!({"type": "object"});
    /// let mut stream = provider.stream_extract("content".to_string(), schema).await.unwrap();
    /// while let Some(result) = stream.next().await {
    ///     match result {
    ///         Ok(partial) => println!("Chunk: {}", partial),
    ///         Err(e) => eprintln!("Stream error: {}", e),
    ///     }
    /// }
    /// # }
    /// ```
    async fn stream_extract(
        &self,
        content: String,
        schema: Value,
    ) -> Result<futures::stream::BoxStream<'static, Result<Value>>>;

    /// Get provider capabilities
    ///
    /// Returns a struct describing what features this provider supports.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::AIProvider;
    /// # fn example(provider: impl AIProvider) {
    /// let caps = provider.capabilities();
    /// if caps.streaming {
    ///     println!("Provider supports streaming");
    /// }
    /// if caps.vision {
    ///     println!("Provider supports image analysis");
    /// }
    /// # }
    /// ```
    fn capabilities(&self) -> ProviderCapabilities;

    /// Provider name (claude, gpt, gemini, etc.)
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::AIProvider;
    /// # fn example(provider: impl AIProvider) {
    /// println!("Using provider: {}", provider.name());
    /// # }
    /// ```
    fn name(&self) -> &'static str;
}

/// Cache port for storing/retrieving data with idempotency key support
///
/// Provides a key-value store interface for caching scraped content,
/// API responses, and idempotency tracking.
///
/// # Example Implementation
///
/// ```no_run
/// use stygian_graph::ports::CachePort;
/// use stygian_graph::domain::error::Result;
/// use async_trait::async_trait;
/// use std::time::Duration;
///
/// struct MyCache;
///
/// #[async_trait]
/// impl CachePort for MyCache {
///     async fn get(&self, key: &str) -> Result<Option<String>> {
///         // Fetch from cache backend
///         Ok(Some("cached_value".to_string()))
///     }
///
///     async fn set(&self, key: &str, value: String, ttl: Option<Duration>) -> Result<()> {
///         // Store in cache backend
///         Ok(())
///     }
///
///     async fn invalidate(&self, key: &str) -> Result<()> {
///         // Remove from cache
///         Ok(())
///     }
///
///     async fn exists(&self, key: &str) -> Result<bool> {
///         // Check existence
///         Ok(true)
///     }
/// }
/// ```
#[async_trait]
pub trait CachePort: Send + Sync {
    /// Get value from cache
    ///
    /// # Arguments
    ///
    /// * `key` - Cache key (URL, idempotency key, etc.)
    ///
    /// # Returns
    ///
    /// * `Ok(Some(String))` - Cache hit
    /// * `Ok(None)` - Cache miss
    /// * `Err(CacheError)` - Backend failure
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::CachePort;
    /// # async fn example(cache: impl CachePort) {
    /// match cache.get("page:123").await {
    ///     Ok(Some(content)) => println!("Cache hit: {}", content),
    ///     Ok(None) => println!("Cache miss"),
    ///     Err(e) => eprintln!("Cache error: {}", e),
    /// }
    /// # }
    /// ```
    async fn get(&self, key: &str) -> Result<Option<String>>;

    /// Set value in cache with optional TTL
    ///
    /// # Arguments
    ///
    /// * `key` - Cache key
    /// * `value` - Value to store (JSON, HTML, etc.)
    /// * `ttl` - Optional expiration duration
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Successfully stored
    /// * `Err(CacheError)` - Backend failure
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::CachePort;
    /// # use std::time::Duration;
    /// # async fn example(cache: impl CachePort) {
    /// cache.set(
    ///     "page:123",
    ///     "<html>...</html>".to_string(),
    ///     Some(Duration::from_secs(3600))
    /// ).await.unwrap();
    /// # }
    /// ```
    async fn set(&self, key: &str, value: String, ttl: Option<std::time::Duration>) -> Result<()>;

    /// Invalidate cache entry
    ///
    /// # Arguments
    ///
    /// * `key` - Cache key to invalidate
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Successfully invalidated (or key didn't exist)
    /// * `Err(CacheError)` - Backend failure
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::CachePort;
    /// # async fn example(cache: impl CachePort) {
    /// cache.invalidate("page:123").await.unwrap();
    /// # }
    /// ```
    async fn invalidate(&self, key: &str) -> Result<()>;

    /// Check if key exists in cache
    ///
    /// # Arguments
    ///
    /// * `key` - Cache key to check
    ///
    /// # Returns
    ///
    /// * `Ok(true)` - Key exists
    /// * `Ok(false)` - Key does not exist or expired
    /// * `Err(CacheError)` - Backend failure
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::CachePort;
    /// # async fn example(cache: impl CachePort) {
    /// if cache.exists("page:123").await.unwrap() {
    ///     println!("Page is cached");
    /// }
    /// # }
    /// ```
    async fn exists(&self, key: &str) -> Result<bool>;
}

/// Circuit breaker state
///
/// Represents the current state of a circuit breaker following
/// the standard circuit breaker pattern.
///
/// # State Transitions
///
/// ```text
/// Closed ---(too many failures)---> Open
/// Open -----(timeout elapsed)-----> HalfOpen
/// HalfOpen --(success)-----------> Closed
/// HalfOpen --(failure)-----------> Open
/// ```
///
/// # Example
///
/// ```
/// use stygian_graph::ports::CircuitState;
///
/// let state = CircuitState::Closed;
/// assert!(matches!(state, CircuitState::Closed));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation, requests pass through
    Closed,
    /// Circuit is open, requests fail fast
    Open,
    /// Testing if service recovered, limited requests allowed
    HalfOpen,
}

/// Circuit breaker port for resilience
///
/// Implements the circuit breaker pattern to prevent cascading failures
/// when external services are unavailable. Uses interior mutability
/// for state management.
///
/// # Example Implementation
///
/// ```no_run
/// use stygian_graph::ports::{CircuitBreaker, CircuitState};
/// use stygian_graph::domain::error::Result;
/// use parking_lot::RwLock;
/// use std::sync::Arc;
///
/// struct MyCircuitBreaker {
///     state: Arc<RwLock<CircuitState>>,
/// }
///
/// impl CircuitBreaker for MyCircuitBreaker {
///     fn state(&self) -> CircuitState {
///         *self.state.read()
///     }
///
///     fn record_success(&self) {
///         let mut state = self.state.write();
///         *state = CircuitState::Closed;
///     }
///
///     fn record_failure(&self) {
///         let mut state = self.state.write();
///         *state = CircuitState::Open;
///     }
///
///     fn attempt_reset(&self) -> bool {
///         let mut state = self.state.write();
///         if matches!(*state, CircuitState::Open) {
///             *state = CircuitState::HalfOpen;
///             true
///         } else {
///             false
///         }
///     }
/// }
/// ```
pub trait CircuitBreaker: Send + Sync {
    /// Get current circuit breaker state
    ///
    /// # Returns
    ///
    /// Current state (`Closed`, `Open`, or `HalfOpen`)
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::{CircuitBreaker, CircuitState};
    /// # fn example(cb: impl CircuitBreaker) {
    /// match cb.state() {
    ///     CircuitState::Closed => println!("Normal operation"),
    ///     CircuitState::Open => println!("Circuit is open, failing fast"),
    ///     CircuitState::HalfOpen => println!("Testing recovery"),
    /// }
    /// # }
    /// ```
    fn state(&self) -> CircuitState;

    /// Record successful operation
    ///
    /// Transitions `HalfOpen` -> `Closed`, maintains `Closed` state.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::CircuitBreaker;
    /// # fn example(cb: impl CircuitBreaker) {
    /// // After successful API call
    /// cb.record_success();
    /// # }
    /// ```
    fn record_success(&self);

    /// Record failed operation
    ///
    /// May transition `Closed` -> `Open` or `HalfOpen` -> `Open` depending on
    /// failure threshold configuration.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::CircuitBreaker;
    /// # fn example(cb: impl CircuitBreaker) {
    /// // After failed API call
    /// cb.record_failure();
    /// # }
    /// ```
    fn record_failure(&self);

    /// Attempt to reset circuit from `Open` to `HalfOpen`
    ///
    /// Called after timeout period to test if service recovered.
    ///
    /// # Returns
    ///
    /// * `true` - Successfully transitioned to `HalfOpen`
    /// * `false` - Already in `Closed` or `HalfOpen` state
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::CircuitBreaker;
    /// # fn example(cb: impl CircuitBreaker) {
    /// if cb.attempt_reset() {
    ///     println!("Circuit breaker now in HalfOpen state");
    /// }
    /// # }
    /// ```
    fn attempt_reset(&self) -> bool;
}

/// Rate limit configuration
///
/// Defines the rate limiting parameters using a token bucket approach.
///
/// # Example
///
/// ```
/// use stygian_graph::ports::RateLimitConfig;
/// use std::time::Duration;
///
/// // Allow 100 requests per minute
/// let config = RateLimitConfig {
///     max_requests: 100,
///     window: Duration::from_secs(60),
/// };
/// ```
#[derive(Debug, Clone, Copy)]
pub struct RateLimitConfig {
    /// Maximum number of requests allowed in the time window
    pub max_requests: u32,
    /// Time window for rate limiting
    pub window: std::time::Duration,
}

/// Rate limiter port
///
/// Implements rate limiting to prevent overwhelming external services
/// or exceeding API quotas. Supports per-key rate limiting for
/// multi-tenant scenarios.
///
/// # Example Implementation
///
/// ```no_run
/// use stygian_graph::ports::RateLimiter;
/// use stygian_graph::domain::error::Result;
/// use async_trait::async_trait;
///
/// struct MyRateLimiter;
///
/// #[async_trait]
/// impl RateLimiter for MyRateLimiter {
///     async fn check_rate_limit(&self, key: &str) -> Result<bool> {
///         // Check if key is within rate limit
///         Ok(true)
///     }
///
///     async fn record_request(&self, key: &str) -> Result<()> {
///         // Record request for rate limiting
///         Ok(())
///     }
/// }
/// ```
#[async_trait]
pub trait RateLimiter: Send + Sync {
    /// Check if key is within rate limit
    ///
    /// # Arguments
    ///
    /// * `key` - Rate limit key (service name, API endpoint, user ID, etc.)
    ///
    /// # Returns
    ///
    /// * `Ok(true)` - Request allowed
    /// * `Ok(false)` - Rate limit exceeded
    /// * `Err(RateLimitError)` - Backend failure
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::RateLimiter;
    /// # async fn example(limiter: impl RateLimiter) {
    /// if limiter.check_rate_limit("api:openai").await.unwrap() {
    ///     println!("Request allowed");
    /// } else {
    ///     println!("Rate limit exceeded, retry later");
    /// }
    /// # }
    /// ```
    async fn check_rate_limit(&self, key: &str) -> Result<bool>;

    /// Record a request for rate limiting
    ///
    /// Should be called after successful operation to update the rate limit counter.
    ///
    /// # Arguments
    ///
    /// * `key` - Rate limit key
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Request recorded
    /// * `Err(RateLimitError)` - Backend failure
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::RateLimiter;
    /// # async fn example(limiter: impl RateLimiter) {
    /// // After making API call
    /// limiter.record_request("api:openai").await.unwrap();
    /// # }
    /// ```
    async fn record_request(&self, key: &str) -> Result<()>;
}

// ─────────────────────────────────────────────────────────────────────────────
// GraphQL auth types
// ─────────────────────────────────────────────────────────────────────────────

/// Authentication strategy for a GraphQL request.
///
/// # Example
///
/// ```rust
/// use stygian_graph::ports::{GraphQlAuth, GraphQlAuthKind};
///
/// let auth = GraphQlAuth {
///     kind: GraphQlAuthKind::Bearer,
///     token: "${env:MY_TOKEN}".to_string(),
///     header_name: None,
/// };
/// ```
#[derive(Debug, Clone)]
pub struct GraphQlAuth {
    /// The authentication strategy to apply
    pub kind: GraphQlAuthKind,
    /// The token value (supports `${env:VAR}` expansion)
    pub token: String,
    /// Custom header name (required when `kind == Header`)
    pub header_name: Option<String>,
}

/// Discriminant for `GraphQlAuth`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphQlAuthKind {
    /// `Authorization: Bearer <token>`
    Bearer,
    /// `X-Api-Key: <token>`
    ApiKey,
    /// Arbitrary header specified by `header_name`
    Header,
    /// No authentication
    None,
}

/// GraphQL plugin sub-module
pub mod graphql_plugin;

/// Work queue port — distributed task execution
pub mod work_queue;

/// WASM plugin port — dynamic plugin loading
pub mod wasm_plugin;

/// Storage port — persist and retrieve pipeline results
pub mod storage;

/// Auth port — runtime token loading, expiry checking, and refresh.
pub mod auth;
