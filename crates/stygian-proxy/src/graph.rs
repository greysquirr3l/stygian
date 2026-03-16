//! [`ProxyManagerPort`] — abstract proxy source for `stygian-graph` HTTP adapters.
//!
//! Enabled by the `graph` feature flag.
//!
//! # Integration pattern
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use stygian_proxy::graph::{BoxedProxyManager, NoopProxyManager, ProxyManagerPort};
//! use stygian_proxy::{ProxyConfig, ProxyHandle, ProxyManager};
//! use stygian_proxy::storage::MemoryProxyStore;
//!
//! // Option A — use a real ProxyManager
//! let storage = Arc::new(MemoryProxyStore::default());
//! let manager: Arc<ProxyManager> =
//!     Arc::new(ProxyManager::with_round_robin(storage, ProxyConfig::default()).unwrap());
//! let boxed: BoxedProxyManager = manager;
//!
//! // Option B — no proxy, pass-through
//! let noop: BoxedProxyManager = Arc::new(NoopProxyManager);
//!
//! // Inside an HTTP adapter: acquire before each request.
//! // let handle = boxed.acquire_proxy().await?;
//! // ... make request ...
//! // handle.mark_success();
//! ```

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::ProxyResult;
use crate::manager::{ProxyHandle, ProxyManager};

// ─────────────────────────────────────────────────────────────────────────────
// ProxyManagerPort trait
// ─────────────────────────────────────────────────────────────────────────────

/// Abstract proxy source consumed by HTTP adapters in the graph pipeline.
///
/// The blanket implementation for [`ProxyManager`] routes calls through the
/// full pool (rotation strategy + circuit breakers).  [`NoopProxyManager`]
/// provides a pass-through implementation for pipelines that don't need proxy
/// rotation.
#[async_trait]
pub trait ProxyManagerPort: Send + Sync + 'static {
    /// Acquire the next proxy from the pool.
    ///
    /// Callers should call [`ProxyHandle::mark_success`] on the returned handle
    /// if the request succeeded, then drop it.  Dropping without marking success
    /// counts as a failure for the circuit breaker.
    async fn acquire_proxy(&self) -> ProxyResult<ProxyHandle>;
}

/// Convenience alias for a type-erased, heap-allocated [`ProxyManagerPort`].
pub type BoxedProxyManager = Arc<dyn ProxyManagerPort>;

// ─────────────────────────────────────────────────────────────────────────────
// Blanket impl for ProxyManager
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl ProxyManagerPort for ProxyManager {
    async fn acquire_proxy(&self) -> ProxyResult<ProxyHandle> {
        ProxyManager::acquire_proxy(self).await
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// NoopProxyManager
// ─────────────────────────────────────────────────────────────────────────────

/// A no-op [`ProxyManagerPort`] that always returns a "direct" (unproxied) handle.
///
/// Use this as a drop-in when proxy rotation is not required, or in tests that
/// need a `BoxedProxyManager` without a real pool.
pub struct NoopProxyManager;

#[async_trait]
impl ProxyManagerPort for NoopProxyManager {
    async fn acquire_proxy(&self) -> ProxyResult<ProxyHandle> {
        Ok(ProxyHandle::direct())
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
    async fn noop_returns_direct_handle() {
        let noop = NoopProxyManager;
        let handle = noop.acquire_proxy().await.unwrap();
        assert!(
            handle.proxy_url.is_empty(),
            "direct handle should have empty URL"
        );
        // Dropping without mark_success should NOT trip any circuit breaker
        // (the noop CB has threshold u32::MAX).
        drop(handle);
    }

    #[tokio::test]
    async fn proxy_manager_implements_port() {
        let storage = Arc::new(MemoryProxyStore::default());
        let mgr = Arc::new(
            ProxyManager::with_round_robin(storage.clone(), ProxyConfig::default()).unwrap(),
        );
        mgr.add_proxy(make_proxy("http://a.test:8080"))
            .await
            .unwrap();

        // Access through the trait object.
        let port: &dyn ProxyManagerPort = mgr.as_ref();
        let handle = port.acquire_proxy().await.unwrap();
        assert!(!handle.proxy_url.is_empty());
        handle.mark_success();
    }
}
