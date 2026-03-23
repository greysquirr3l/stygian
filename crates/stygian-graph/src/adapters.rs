//! Adapter implementations - infrastructure concerns
//!
//! Concrete implementations of port traits:
//! - HTTP client with anti-bot features
//! - AI providers (Claude, GPT, Gemini, Ollama, Copilot)
//! - Storage backends
//! - Cache backends

/// HTTP scraping adapter with anti-bot capabilities
pub mod http;

/// REST API adapter — JSON APIs with auth, pagination, and data extraction
pub mod rest_api;

/// AI provider adapters
pub mod ai;

/// Storage adapters (file, S3, database)
pub mod storage;

/// Cache adapters (memory, Redis)
pub mod cache;

/// Resilience adapters (circuit breaker, retry)
pub mod resilience;

/// No-op service for testing
pub mod noop;

/// JavaScript rendering adapter (headless browser via stygian-browser)
#[cfg(feature = "browser")]
pub mod browser;

/// Multi-modal content extraction (CSV, JSON, XML, images, PDFs)
pub mod multimodal;

/// Mock AI provider for testing
pub mod mock_ai;

/// GraphQL API adapter — generic ScrapingService for any GraphQL endpoint
pub mod graphql;

/// OpenAPI 3.x introspection adapter — resolves operations from an OpenAPI spec and delegates to RestApiAdapter
pub mod openapi;

pub mod graphql_rate_limit;
/// Proactive cost-throttle management for GraphQL APIs
pub mod graphql_throttle;

/// Distributed work queue and executor adapters
pub mod distributed;

/// GraphQL target plugin implementations (one file per API target)
pub mod graphql_plugins;

/// WASM plugin adapter (feature = "wasm-plugins")
pub mod wasm_plugin;

/// Cloudflare Browser Rendering crawl adapter (feature = "cloudflare-crawl")
#[cfg(feature = "cloudflare-crawl")]
pub mod cloudflare_crawl;

/// Output format helpers — CSV, JSONL, JSON
pub mod output_format;

/// Request signing adapters — Noop passthrough and HTTP sidecar bridge.
/// Covers Frida RPC, AWS Sig V4, OAuth 1.0a, custom HMAC, and device attestation.
pub mod signing;

/// OpenAPI spec generator from API discovery reports
pub mod openapi_gen;

/// PostgreSQL database source adapter (feature = "postgres")
#[cfg(feature = "postgres")]
pub mod database;

/// File system / document source adapter
pub mod document;

/// Server-Sent Events stream source adapter
pub mod stream;

/// LLM agent source adapter — wraps AIProvider as a pipeline node
pub mod agent_source;

/// Redis / Valkey cache adapter (feature = "redis")
#[cfg(feature = "redis")]
pub mod cache_redis;

/// Sitemap / sitemap-index source adapter
pub mod sitemap;
