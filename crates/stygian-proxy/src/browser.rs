//! Per-context proxy binding for `stygian-browser`.
//!
//! Enabled by the `browser` feature flag.
//!
//! # Integration pattern
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use stygian_proxy::browser::{BrowserProxySource, ProxyManagerBridge};
//! use stygian_proxy::{ProxyConfig, ProxyManager};
//! use stygian_proxy::storage::MemoryProxyStore;
//!
//! # async fn run() -> stygian_proxy::ProxyResult<()> {
//! let storage = Arc::new(MemoryProxyStore::default());
//! let manager = Arc::new(ProxyManager::with_round_robin(storage, ProxyConfig::default())?);
//! let bridge = ProxyManagerBridge::new(manager);
//!
//! // Inside BrowserPool when spawning a new context:
//! let (proxy_url, handle) = bridge.bind_proxy().await?;
//! // Pass proxy_url to --proxy-server Chrome arg.
//! // Keep handle alive for the context's lifetime; mark success on clean exit.
//! // handle.mark_success();
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::ProxyResult;
use crate::manager::{ProxyHandle, ProxyManager};

// ─────────────────────────────────────────────────────────────────────────────
// BrowserProxySource trait
// ─────────────────────────────────────────────────────────────────────────────

/// Proxy binding interface for browser context pools.
///
/// Implemented by [`ProxyManagerBridge`] for real proxy rotation.  A noop
/// implementation can return `("", ProxyHandle::direct())` to skip proxying.
#[async_trait]
pub trait BrowserProxySource: Send + Sync + 'static {
    /// Acquire a proxy and return its URL together with the RAII tracking handle.
    ///
    /// The caller must keep the returned [`ProxyHandle`] alive for the duration
    /// of the browser context.  Call [`ProxyHandle::mark_success`] on clean
    /// context exit; let it drop normally on crash / error so the circuit
    /// breaker records a failure.
    async fn bind_proxy(&self) -> ProxyResult<(String, ProxyHandle)>;
}

// ─────────────────────────────────────────────────────────────────────────────
// ProxyManagerBridge
// ─────────────────────────────────────────────────────────────────────────────

/// Wraps an `Arc<ProxyManager>` and implements [`BrowserProxySource`].
///
/// Each call to [`bind_proxy`](BrowserProxySource::bind_proxy) acquires the
/// next proxy from the pool, returning the proxy URL and a live circuit-breaker
/// handle.
pub struct ProxyManagerBridge {
    manager: Arc<ProxyManager>,
}

impl ProxyManagerBridge {
    /// Create a new bridge backed by `manager`.
    pub fn new(manager: Arc<ProxyManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl BrowserProxySource for ProxyManagerBridge {
    async fn bind_proxy(&self) -> ProxyResult<(String, ProxyHandle)> {
        let handle = self.manager.acquire_proxy().await?;
        let url = handle.proxy_url.clone();
        Ok((url, handle))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::storage::MemoryProxyStore;
    use crate::types::{Proxy, ProxyConfig, ProxyType};

    fn make_proxy(url: &str) -> Proxy {
        Proxy {
            url: url.into(),
            proxy_type: ProxyType::Http,
            username: None,
            password: None,
            weight: 1,
            tags: vec![],
        }
    }

    #[tokio::test]
    async fn bridge_returns_proxy_url_and_handle() {
        let storage = Arc::new(MemoryProxyStore::default());
        let mgr = Arc::new(
            ProxyManager::with_round_robin(storage.clone(), ProxyConfig::default()).unwrap(),
        );
        mgr.add_proxy(make_proxy("http://p.test:8080"))
            .await
            .unwrap();

        let bridge = ProxyManagerBridge::new(mgr);
        let (url, handle) = bridge.bind_proxy().await.unwrap();
        assert_eq!(url, "http://p.test:8080");
        handle.mark_success();
    }

    /// Simulates a browser crash: drop handle without success → circuit opens.
    #[tokio::test]
    async fn crash_records_failure() {
        let storage = Arc::new(MemoryProxyStore::default());
        let mgr = Arc::new(
            ProxyManager::with_round_robin(
                storage.clone(),
                ProxyConfig {
                    circuit_open_threshold: 1,
                    ..ProxyConfig::default()
                },
            )
            .unwrap(),
        );
        mgr.add_proxy(make_proxy("http://q.test:8080"))
            .await
            .unwrap();

        let bridge = ProxyManagerBridge::new(Arc::clone(&mgr));
        {
            let (_url, _handle) = bridge.bind_proxy().await.unwrap();
            // Drop without mark_success → simulates a crash.
        }

        // After one failure (threshold = 1) the circuit should be open.
        let stats = mgr.pool_stats().await.unwrap();
        assert_eq!(
            stats.open, 1,
            "circuit should open after crash (open = {})",
            stats.open
        );
    }

    /// `ProxyHandle::direct()` is usable as a no-proxy binding.
    #[test]
    fn direct_handle_is_valid_noop_binding() {
        let handle = ProxyHandle::direct();
        assert!(handle.proxy_url.is_empty());
        handle.mark_success();
        // drop without panic
    }
}
