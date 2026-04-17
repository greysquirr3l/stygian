//! # stygian-proxy
#![allow(clippy::multiple_crate_versions)]
//!
//! High-performance, resilient proxy rotation for the Stygian scraping ecosystem.
//!
//! ## Features
//!
//! - Pluggable rotation strategies: round-robin, random, weighted, least-used
//! - Per-proxy latency and success-rate tracking via atomics
//! - Async health checker with configurable intervals
//! - Per-proxy circuit breaker (`Closed -> Open -> HalfOpen`)
//! - In-memory proxy pool (no external DB required)
//! - `graph` feature: [`ProxyManagerPort`] trait for stygian-graph HTTP adapters
//! - `browser` feature: per-context proxy binding for stygian-browser
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use stygian_proxy::error::ProxyResult;
//!
//! fn main() -> ProxyResult<()> {
//!     // ProxyManager construction added in T12 (proxy-manager task)
//!     Ok(())
//! }
//! ```

pub mod circuit_breaker;
pub mod error;
pub mod fetcher;
pub mod health;
pub mod manager;
pub mod session;
pub mod storage;
pub mod strategy;
pub mod types;

#[cfg(feature = "graph")]
pub mod graph;

#[cfg(feature = "browser")]
pub mod browser;

#[cfg(feature = "tls-profiled")]
pub mod http_client;

pub mod routing;

/// MCP (Model Context Protocol) server — exposes proxy pool tools
#[cfg(feature = "mcp")]
pub mod mcp;

// Top-level re-exports
pub use circuit_breaker::{CircuitBreaker, STATE_CLOSED, STATE_HALF_OPEN, STATE_OPEN};
pub use error::{ProxyError, ProxyResult};
pub use fetcher::{FreeListFetcher, FreeListSource, ProxyFetcher, load_from_fetcher};
pub use health::{HealthChecker, HealthMap};
pub use manager::{PoolStats, ProxyHandle, ProxyManager, ProxyManagerBuilder};
pub use session::{SessionMap, StickyPolicy};
pub use storage::MemoryProxyStore;
pub use strategy::{
    BoxedRotationStrategy, LeastUsedStrategy, ProxyCandidate, RandomStrategy, RotationStrategy,
    RoundRobinStrategy, WeightedStrategy, capable_healthy_candidates,
};
pub use types::{
    CapabilityRequirement, ProfiledRequestMode, Proxy, ProxyCapabilities, ProxyConfig,
    ProxyMetrics, ProxyRecord, ProxyType, RoutingPath,
};

#[cfg(feature = "graph")]
pub use graph::{BoxedProxyManager, NoopProxyManager, ProxyManagerPort};

#[cfg(feature = "browser")]
pub use browser::{BrowserProxySource, ProxyManagerBridge};

#[cfg(feature = "tls-profiled")]
pub use http_client::{ProfiledRequester, ProfiledRequesterError};
