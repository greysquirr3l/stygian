//! # stygian-proxy
#![allow(clippy::multiple_crate_versions)]
//!
//! High-performance, resilient proxy rotation for the Stygian scraping ecosystem.
//!
//! ## Features
//!
//! - Pluggable rotation strategies: round-robin, random, weighted, least-used
//! - `bayesian-rotation` feature: Thompson-sampling bandit strategy that
//!   learns per-proxy health online. Cites 76 % success vs 36 % round-robin
//!   on identical proxies in the `ProxyOps` benchmark (549 114 requests / 7 days).
//!   See `crate::strategy::ThompsonStrategy` and the strategy module
//!   docs for the 76 % / 36 % citation.
//! - Per-proxy latency and success-rate tracking via atomics
//! - Async health checker with configurable intervals
//! - Per-proxy circuit breaker (`Closed -> Open -> HalfOpen`)
//! - In-memory proxy pool (no external DB required)
//! - `graph` feature: `ProxyManagerPort` trait for stygian-graph HTTP adapters
//!   (see `crate::graph`; only compiled with `--features graph`)
//! - `browser` feature: per-context proxy binding for stygian-browser
//! - `vendor-stickiness` feature: per-vendor session stickiness policy.
//!   Encodes the 2026 guide anti-bot stickiness matrix
//!   (`Akamai` → 30min sticky, `Cloudflare` → 5min sticky,
//!   `Imperva` → 15min sticky, `PerimeterX` / `Kasada` → fresh per
//!   domain, `DataDome` → fresh per request, everything else → fresh
//!   per request) into a typed [`VendorStickinessMap`] consulted by
//!   [`SessionMap::acquire_session`](crate::session::SessionMap::acquire_session)
//!   and
//!   [`ProxyManager::acquire_for_domain_with_vendor`](crate::manager::ProxyManager::acquire_for_domain_with_vendor).
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
pub mod ports;
pub mod session;
pub mod stickiness;
pub mod storage;
pub mod strategy;
pub mod types;
pub mod vendor_quirks;

#[cfg(feature = "graph")]
pub mod graph;

#[cfg(feature = "browser")]
pub mod browser;

#[cfg(feature = "tls-profiled")]
pub mod http_client;

pub mod routing;

#[cfg(feature = "coherence-validation")]
pub mod adapters;

/// MCP (Model Context Protocol) server — exposes proxy pool tools
#[cfg(feature = "mcp")]
pub mod mcp;

// Top-level re-exports
pub use circuit_breaker::{CircuitBreaker, STATE_CLOSED, STATE_HALF_OPEN, STATE_OPEN};
pub use error::{ProxyError, ProxyResult};
#[cfg(feature = "dns-fetcher")]
pub use fetcher::DnsTxtFetcher;
pub use fetcher::{
    FreeApiProxiesFetcher, FreeListFetcher, FreeListSource, ProxyFetcher, load_from_fetcher,
};
pub use health::{HealthChecker, HealthMap};
pub use manager::{PoolStats, ProxyHandle, ProxyManager, ProxyManagerBuilder};
pub use session::{SessionDecision, SessionMap, StickyPolicy};
pub use stickiness::{StickinessPolicy, VendorStickinessMap};
pub use storage::MemoryProxyStore;
pub use strategy::{
    BayesianObserver, BoxedBayesianObserver, BoxedRotationStrategy, LeastUsedStrategy,
    NoopBayesianObserver, ProxyCandidate, RandomStrategy, RotationStrategy, RoundRobinStrategy,
    WeightedStrategy, capable_healthy_candidates,
};

#[cfg(feature = "bayesian-rotation")]
pub use strategy::ThompsonStrategy;
pub use types::{
    CapabilityRequirement, IpClass, IpClassRequirement, ProfiledRequestMode, Proxy,
    ProxyCapabilities, ProxyConfig, ProxyMetrics, ProxyRecord, ProxyType, RoutingPath,
    TargetVendorCompatibility, TrustTier, VendorId, validate_asn, validate_city,
    validate_postal_code, well_known,
};
pub use vendor_quirks::{
    BRD_SUPERPROXY_QUIRK, CRAWLERA_8011_QUIRK, IPROYAL_QUIRK, ParseError, ProxyUrl, QuirkMatch,
    QuirkSeverity, Scheme, VENDOR_QUIRKS, VendorQuirk, ZYTE_8011_QUIRK, check,
};

#[cfg(feature = "graph")]
pub use graph::{BoxedProxyManager, NoopProxyManager, ProxyManagerPort};

#[cfg(feature = "browser")]
pub use browser::{BrowserProxySource, ProxyManagerBridge};

#[cfg(feature = "tls-profiled")]
pub use http_client::{ProfiledRequester, ProfiledRequesterError};

// Coherence port + adapter re-exports. The trait and supporting types are
// always compiled (so external adapters can implement `CoherencePort` even
// when the default `DefaultCoherenceValidator` is off); the adapter itself
// is feature-gated, mirroring the `BayesianObserver` / `ThompsonStrategy`
// pattern.
pub use ports::coherence::{
    AcceptLanguage, BoxedCoherencePort, CoherenceContext, CoherencePolicy, CoherencePort,
    CoherenceVerdict, IsoCountry, Locale, MismatchField, MismatchSeverity, Tz,
};

#[cfg(feature = "coherence-validation")]
pub use adapters::coherence::DefaultCoherenceValidator;
