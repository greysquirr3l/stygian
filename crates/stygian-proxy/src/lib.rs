//! # stygian-proxy
//!
//! High-performance, resilient proxy rotation for the Stygian scraping ecosystem.
//!
//! ## Features
//!
//! - Pluggable rotation strategies: round-robin, random, weighted, least-used
//! - Per-proxy latency and success-rate tracking via atomics
//! - Async health checker with configurable intervals
//! - Per-proxy circuit breaker (Closed → Open → HalfOpen)
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
pub mod health;
pub mod manager;
pub mod storage;
pub mod strategy;
pub mod types;

#[cfg(feature = "graph")]
pub mod graph;

#[cfg(feature = "browser")]
pub mod browser;

// Top-level re-exports
pub use error::{ProxyError, ProxyResult};
pub use types::{Proxy, ProxyConfig, ProxyMetrics, ProxyRecord, ProxyType};
