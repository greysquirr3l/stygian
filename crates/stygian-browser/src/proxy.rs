//! Proxy source port for browser context pools.
//!
//! This module provides the [`ProxySource`] and [`ProxyLease`] traits that
//! decouple `stygian-browser` from any concrete proxy implementation.  The
//! browser crate owns the trait definitions; `stygian-proxy` implements them.
//!
//! Wire in a real proxy pool by setting [`BrowserConfig::proxy_source`] to an
//! `Arc<dyn ProxySource>`.  `stygian-proxy` provides a ready-made
//! implementation via `ProxyManagerBridge` when compiled with its `browser`
//! feature.
//!
//! # Example
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use async_trait::async_trait;
//! use stygian_browser::proxy::{ProxyLease, ProxySource, DirectLease};
//! use stygian_browser::error::Result;
//!
//! #[derive(Debug)]
//! struct StaticProxy {
//!     url: String,
//! }
//!
//! #[async_trait]
//! impl ProxySource for StaticProxy {
//!     async fn bind_proxy(&self) -> Result<(String, Box<dyn ProxyLease>)> {
//!         Ok((self.url.clone(), Box::new(DirectLease)))
//!     }
//! }
//!
//! let cfg = stygian_browser::BrowserConfig::builder()
//!     .proxy_source(Arc::new(StaticProxy { url: "http://proxy.example.com:8080".into() }))
//!     .build();
//! ```

use std::fmt;

use async_trait::async_trait;

use crate::error::Result;

// ─── ProxyLease ───────────────────────────────────────────────────────────────

/// RAII guard for a proxy acquired from a [`ProxySource`].
///
/// Held for the lifetime of the browser instance using the proxy.  Call
/// [`mark_success`](ProxyLease::mark_success) when the browser session
/// completes cleanly.  Dropping without calling it signals a failure to the
/// underlying circuit breaker (if any).
///
/// # Example
///
/// ```
/// use stygian_browser::proxy::{ProxyLease, DirectLease};
/// let lease: Box<dyn ProxyLease> = Box::new(DirectLease);
/// lease.mark_success(); // no-op for DirectLease
/// ```
pub trait ProxyLease: Send + 'static {
    /// Record that the browser session using this proxy completed successfully.
    fn mark_success(&self);
}

// ─── DirectLease ─────────────────────────────────────────────────────────────

/// A no-op [`ProxyLease`] for use when no proxy is configured.
///
/// All methods are no-ops.  Use this as the lease type in [`ProxySource`]
/// implementations that do not need circuit-breaker tracking.
///
/// # Example
///
/// ```
/// use stygian_browser::proxy::{ProxyLease, DirectLease};
/// let lease = DirectLease;
/// lease.mark_success(); // no-op
/// ```
pub struct DirectLease;

impl ProxyLease for DirectLease {
    fn mark_success(&self) {}
}

// ─── ProxySource ──────────────────────────────────────────────────────────────

/// Source of proxies for browser context pools.
///
/// Implement this trait and pass an `Arc<dyn ProxySource>` to
/// [`BrowserConfig::builder().proxy_source(...)`](crate::config::BrowserConfigBuilder::proxy_source)
/// to enable per-context proxy rotation with circuit-breaker support.
///
/// Each call to [`bind_proxy`](ProxySource::bind_proxy) acquires a proxy URL
/// and an RAII [`ProxyLease`] that must be held for the lifetime of the
/// browser instance.
///
/// `stygian-proxy` provides a ready-made implementation via
/// `ProxyManagerBridge` when compiled with the `browser` feature.
///
/// # Example
///
/// ```rust,no_run
/// use std::sync::Arc;
/// use async_trait::async_trait;
/// use stygian_browser::proxy::{ProxyLease, ProxySource, DirectLease};
/// use stygian_browser::error::Result;
///
/// #[derive(Debug)]
/// struct RoundRobinProxy {
///     urls: Vec<String>,
/// }
///
/// #[async_trait]
/// impl ProxySource for RoundRobinProxy {
///     async fn bind_proxy(&self) -> Result<(String, Box<dyn ProxyLease>)> {
///         let url = self.urls[0].clone(); // simplified — real impl would rotate
///         Ok((url, Box::new(DirectLease)))
///     }
/// }
///
/// let source = Arc::new(RoundRobinProxy { urls: vec!["http://p.example.com:8080".into()] });
/// let cfg = stygian_browser::BrowserConfig::builder()
///     .proxy_source(source)
///     .build();
/// ```
#[async_trait]
pub trait ProxySource: Send + Sync + fmt::Debug + 'static {
    /// Acquire the next proxy URL and an RAII lease handle.
    ///
    /// The returned `(url, lease)` pair must be used together:
    /// - `url` is passed as the `--proxy-server` Chrome launch argument.
    /// - `lease` must be held for the lifetime of that browser instance.
    ///   Call [`ProxyLease::mark_success`] on clean session exit.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::BrowserError::ProxyUnavailable`] if no proxy
    /// is currently available (e.g. circuit breaker open, pool empty).
    async fn bind_proxy(&self) -> Result<(String, Box<dyn ProxyLease>)>;
}
