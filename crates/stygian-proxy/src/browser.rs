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
    #[must_use]
    pub const fn new(manager: Arc<ProxyManager>) -> Self {
        Self { manager }
    }
}

impl std::fmt::Debug for ProxyManagerBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyManagerBridge").finish_non_exhaustive()
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

// ─── stygian_browser::ProxySource impl ───────────────────────────────────────

/// Wraps a [`ProxyHandle`] as a [`stygian_browser::proxy::ProxyLease`].
///
/// Calling [`mark_success`](stygian_browser::proxy::ProxyLease::mark_success)
/// delegates to [`ProxyHandle::mark_success`], signalling a successful session
/// to the circuit breaker.  Dropping without calling it records a failure.
struct ProxyLeaseAdapter(ProxyHandle);

impl stygian_browser::proxy::ProxyLease for ProxyLeaseAdapter {
    fn mark_success(&self) {
        self.0.mark_success();
    }
}

/// Implements [`stygian_browser::proxy::ProxySource`] for [`ProxyManagerBridge`].
///
/// This is the primary integration point: pass `Arc::new(ProxyManagerBridge::new(manager))`
/// to [`stygian_browser::BrowserConfig::builder().proxy_source(...)`](stygian_browser::config::BrowserConfigBuilder::proxy_source).
///
/// # Example
///
/// ```rust,no_run
/// use std::sync::Arc;
/// use stygian_browser::BrowserConfig;
/// use stygian_proxy::{ProxyConfig, ProxyManager};
/// use stygian_proxy::storage::MemoryProxyStore;
/// use stygian_proxy::browser::ProxyManagerBridge;
///
/// # async fn run() -> stygian_proxy::ProxyResult<()> {
/// let storage = Arc::new(MemoryProxyStore::default());
/// let manager = Arc::new(ProxyManager::with_round_robin(storage, ProxyConfig::default())?);
/// let bridge = Arc::new(ProxyManagerBridge::new(manager));
///
/// let config = BrowserConfig::builder()
///     .proxy_source(bridge)
///     .build();
/// # Ok(())
/// # }
/// ```
#[async_trait]
impl stygian_browser::proxy::ProxySource for ProxyManagerBridge {
    async fn bind_proxy(
        &self,
    ) -> stygian_browser::error::Result<(String, Box<dyn stygian_browser::proxy::ProxyLease>)> {
        let handle = self.manager.acquire_proxy().await.map_err(|e| {
            stygian_browser::error::BrowserError::ProxyUnavailable {
                reason: e.to_string(),
            }
        })?;
        let url = handle.proxy_url.clone();
        Ok((url, Box::new(ProxyLeaseAdapter(handle))))
    }

    async fn bind_proxy_with_tls_profile(
        &self,
        profile: Option<&str>,
    ) -> stygian_browser::error::Result<(String, Box<dyn stygian_browser::proxy::ProxyLease>)> {
        let Some(profile) = profile else {
            return stygian_browser::proxy::ProxySource::bind_proxy(self).await;
        };
        let req = crate::types::CapabilityRequirement {
            require_tls_profile: Some(profile.to_string()),
            ..Default::default()
        };
        let handle = self
            .manager
            .acquire_with_capabilities(&req)
            .await
            .map_err(|e| stygian_browser::error::BrowserError::ProxyUnavailable {
                reason: e.to_string(),
            })?;
        let url = handle.proxy_url.clone();
        Ok((url, Box::new(ProxyLeaseAdapter(handle))))
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
            capabilities: crate::types::ProxyCapabilities::default(),
            ip_class: crate::types::IpClass::Unknown,
            target_compatibility: crate::types::TargetVendorCompatibility::default(),
        }
    }

    #[tokio::test]
    async fn bridge_returns_proxy_url_and_handle() -> Result<(), Box<dyn std::error::Error>> {
        let storage = Arc::new(MemoryProxyStore::default());
        let mgr = Arc::new(ProxyManager::with_round_robin(
            storage.clone(),
            ProxyConfig::default(),
        )?);
        mgr.add_proxy(make_proxy("http://p.test:8080")).await?;

        let bridge = ProxyManagerBridge::new(mgr);
        let (url, handle) = bridge.bind_proxy().await?;
        assert_eq!(url, "http://p.test:8080");
        handle.mark_success();
        Ok(())
    }

    /// Simulates a browser crash: drop handle without success → circuit opens.
    #[tokio::test]
    async fn crash_records_failure() -> Result<(), Box<dyn std::error::Error>> {
        let storage = Arc::new(MemoryProxyStore::default());
        let mgr = Arc::new(ProxyManager::with_round_robin(
            storage.clone(),
            ProxyConfig {
                circuit_open_threshold: 1,
                ..ProxyConfig::default()
            },
        )?);
        mgr.add_proxy(make_proxy("http://q.test:8080")).await?;

        let bridge = ProxyManagerBridge::new(Arc::clone(&mgr));
        {
            let (_url, _handle) = bridge.bind_proxy().await?;
            // Drop without mark_success → simulates a crash.
        }

        // After one failure (threshold = 1) the circuit should be open.
        let stats = mgr.pool_stats().await?;
        assert_eq!(
            stats.open, 1,
            "circuit should open after crash (open = {})",
            stats.open
        );
        Ok(())
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
