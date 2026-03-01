//! Adapter implementations - infrastructure concerns
//!
//! Concrete implementations of port traits:
//! - HTTP client with anti-bot features
//! - AI providers (Claude, GPT, Gemini, Ollama, Copilot)
//! - Storage backends
//! - Cache backends

/// HTTP scraping adapter with anti-bot capabilities
pub mod http;

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

/// JavaScript rendering adapter (headless browser via mycelium-browser)
#[cfg(feature = "browser")]
pub mod browser;

/// Multi-modal content extraction (CSV, JSON, XML, images, PDFs)
pub mod multimodal;

/// Mock AI provider for testing
pub mod mock_ai;

/// GraphQL API adapter — generic ScrapingService for any GraphQL endpoint
pub mod graphql;

/// Distributed work queue and executor adapters
pub mod distributed;

/// GraphQL target plugin implementations (one file per API target)
pub mod graphql_plugins;

/// WASM plugin adapter (feature = "wasm-plugins")
pub mod wasm_plugin;

/// Output format helpers — CSV, JSONL, JSON
pub mod output_format;
