//! Storage port and in-memory adapter for proxy records.

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::ProxyResult;
use crate::types::{Proxy, ProxyRecord};

/// Abstract storage interface for persisting and querying proxy records.
///
/// Implementors must be `Send + Sync + 'static` to support concurrent access
/// across async tasks. The trait is object-safe via [`macro@async_trait`].
///
/// # Example
/// ```rust,no_run
/// use stygian_proxy::storage::ProxyStoragePort;
/// use stygian_proxy::types::{Proxy, ProxyType};
/// use uuid::Uuid;
///
/// async fn demo(store: &dyn ProxyStoragePort) {
///     let proxy = Proxy {
///         url: "http://proxy.example.com:8080".into(),
///         proxy_type: ProxyType::Http,
///         username: None,
///         password: None,
///         weight: 1,
///         tags: vec![],
///     };
///     let record = store.add(proxy).await.unwrap();
///     let _ = store.get(record.id).await.unwrap();
/// }
/// ```
#[async_trait]
pub trait ProxyStoragePort: Send + Sync + 'static {
    /// Add a new proxy to the store and return its [`ProxyRecord`].
    async fn add(&self, proxy: Proxy) -> ProxyResult<ProxyRecord>;

    /// Remove a proxy by its UUID. Returns an error if the ID is not found.
    async fn remove(&self, id: Uuid) -> ProxyResult<()>;

    /// Return all stored proxy records.
    async fn list(&self) -> ProxyResult<Vec<ProxyRecord>>;

    /// Fetch a single proxy record by UUID.
    async fn get(&self, id: Uuid) -> ProxyResult<ProxyRecord>;

    /// Record the outcome of a request through a proxy.
    ///
    /// - `success`: whether the request succeeded.
    /// - `latency_ms`: elapsed time in milliseconds.
    async fn update_metrics(&self, id: Uuid, success: bool, latency_ms: u64) -> ProxyResult<()>;

    /// Return all stored proxy records paired with their live metrics reference.
    ///
    /// Used by [`ProxyManager`](crate::manager::ProxyManager) when building
    /// [`ProxyCandidate`](crate::strategy::ProxyCandidate) slices so that
    /// latency-aware strategies (e.g. least-used) see up-to-date counters.
    async fn list_with_metrics(&self) -> ProxyResult<Vec<(ProxyRecord, Arc<ProxyMetrics>)>>;
}

/// Convenience alias for a heap-allocated, type-erased [`ProxyStoragePort`].
pub type BoxedProxyStorage = Box<dyn ProxyStoragePort>;

// ─────────────────────────────────────────────────────────────────────────────
// URL validation helper
// ─────────────────────────────────────────────────────────────────────────────

/// Validate a proxy URL: scheme must be recognised, host must be non-empty,
/// and the explicit port (if present) must be in [1, 65535].
fn validate_proxy_url(url: &str) -> ProxyResult<()> {
    use crate::error::ProxyError;

    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| ProxyError::InvalidProxyUrl {
            url: url.to_owned(),
            reason: "missing scheme separator '://'".into(),
        })?;

    match scheme {
        "http" | "https" => {}
        #[cfg(feature = "socks")]
        "socks4" | "socks5" => {}
        other => {
            return Err(ProxyError::InvalidProxyUrl {
                url: url.to_owned(),
                reason: format!("unsupported scheme '{other}'"),
            });
        }
    }

    // Strip any path/query, then strip user:pass@ if present.
    let authority = rest.split('/').next().unwrap_or("");
    let host_and_port = authority.split('@').next_back().unwrap_or("");

    // Split host from port, handling IPv6 brackets.
    let (host, port_str) = if host_and_port.starts_with('[') {
        let close = host_and_port.find(']').unwrap_or(host_and_port.len());
        let after = &host_and_port[close + 1..];
        let port = after.strip_prefix(':').unwrap_or("");
        (&host_and_port[..=close], port)
    } else {
        match host_and_port.rsplit_once(':') {
            Some((h, p)) => (h, p),
            None => (host_and_port, ""),
        }
    };

    if host.is_empty() || host == "[]" {
        return Err(ProxyError::InvalidProxyUrl {
            url: url.to_owned(),
            reason: "empty host".into(),
        });
    }

    if !port_str.is_empty() {
        let port: u32 = port_str.parse().map_err(|_| ProxyError::InvalidProxyUrl {
            url: url.to_owned(),
            reason: format!("non-numeric port '{port_str}'"),
        })?;
        if port == 0 || port > 65535 {
            return Err(ProxyError::InvalidProxyUrl {
                url: url.to_owned(),
                reason: format!("port {port} is out of range [1, 65535]"),
            });
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// MemoryProxyStore
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;
use tokio::sync::RwLock;

use crate::types::ProxyMetrics;
use std::sync::Arc;

type StoreMap = HashMap<Uuid, (ProxyRecord, Arc<ProxyMetrics>)>;

/// In-memory implementation of [`ProxyStoragePort`].
///
/// Uses a `tokio::sync::RwLock`-guarded `HashMap` for thread-safe access.
/// Metrics are updated via atomic operations, so only a **read** lock is
/// needed for [`update_metrics`](MemoryProxyStore::update_metrics) calls —
/// write contention stays low even under heavy concurrent load.
///
/// # Example
/// ```
/// # tokio_test::block_on(async {
/// use stygian_proxy::storage::{MemoryProxyStore, ProxyStoragePort};
/// use stygian_proxy::types::{Proxy, ProxyType};
///
/// let store = MemoryProxyStore::default();
/// let proxy = Proxy { url: "http://proxy.example.com:8080".into(), proxy_type: ProxyType::Http,
///                     username: None, password: None, weight: 1, tags: vec![] };
/// let record = store.add(proxy).await.unwrap();
/// assert_eq!(store.list().await.unwrap().len(), 1);
/// store.remove(record.id).await.unwrap();
/// assert!(store.list().await.unwrap().is_empty());
/// # })
/// ```
#[derive(Debug, Default, Clone)]
pub struct MemoryProxyStore {
    inner: Arc<RwLock<StoreMap>>,
}

impl MemoryProxyStore {
    /// Build a store pre-populated with `proxies`, validating each URL.
    ///
    /// Returns an error on the first invalid URL encountered.
    pub async fn with_proxies(proxies: Vec<Proxy>) -> ProxyResult<Self> {
        let store = Self::default();
        for proxy in proxies {
            store.add(proxy).await?;
        }
        Ok(store)
    }
}

#[async_trait]
impl ProxyStoragePort for MemoryProxyStore {
    async fn add(&self, proxy: Proxy) -> ProxyResult<ProxyRecord> {
        validate_proxy_url(&proxy.url)?;
        let record = ProxyRecord::new(proxy);
        let metrics = Arc::new(ProxyMetrics::default());
        self.inner
            .write()
            .await
            .insert(record.id, (record.clone(), metrics));
        Ok(record)
    }

    async fn remove(&self, id: Uuid) -> ProxyResult<()> {
        self.inner
            .write()
            .await
            .remove(&id)
            .map(|_| ())
            .ok_or_else(|| crate::error::ProxyError::StorageError(format!("proxy {id} not found")))
    }

    async fn list(&self) -> ProxyResult<Vec<ProxyRecord>> {
        Ok(self
            .inner
            .read()
            .await
            .values()
            .map(|(r, _)| r.clone())
            .collect())
    }

    async fn get(&self, id: Uuid) -> ProxyResult<ProxyRecord> {
        self.inner
            .read()
            .await
            .get(&id)
            .map(|(r, _)| r.clone())
            .ok_or_else(|| crate::error::ProxyError::StorageError(format!("proxy {id} not found")))
    }

    async fn list_with_metrics(&self) -> ProxyResult<Vec<(ProxyRecord, Arc<ProxyMetrics>)>> {
        Ok(self
            .inner
            .read()
            .await
            .values()
            .map(|(r, m)| (r.clone(), Arc::clone(m)))
            .collect())
    }

    async fn update_metrics(&self, id: Uuid, success: bool, latency_ms: u64) -> ProxyResult<()> {
        use std::sync::atomic::Ordering;

        let metrics = self
            .inner
            .read()
            .await
            .get(&id)
            .map(|(_, m)| Arc::clone(m))
            .ok_or_else(|| {
                crate::error::ProxyError::StorageError(format!("proxy {id} not found"))
            })?;

        // Lock released before the atomic updates — no long critical section.
        metrics.requests_total.fetch_add(1, Ordering::Relaxed);
        if success {
            metrics.successes.fetch_add(1, Ordering::Relaxed);
        } else {
            metrics.failures.fetch_add(1, Ordering::Relaxed);
        }
        metrics
            .total_latency_ms
            .fetch_add(latency_ms, Ordering::Relaxed);
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ProxyType;
    use std::sync::atomic::Ordering;

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
    async fn add_list_remove() -> crate::error::ProxyResult<()> {
        let store = MemoryProxyStore::default();
        let r1 = store.add(make_proxy("http://a.test:8080")).await?;
        let r2 = store.add(make_proxy("http://b.test:8080")).await?;
        let r3 = store.add(make_proxy("http://c.test:8080")).await?;
        assert_eq!(store.list().await?.len(), 3);
        store.remove(r2.id).await?;
        let remaining = store.list().await?;
        assert_eq!(remaining.len(), 2);
        let ids: Vec<_> = remaining.iter().map(|r| r.id).collect();
        assert!(ids.contains(&r1.id));
        assert!(ids.contains(&r3.id));
        Ok(())
    }

    #[tokio::test]
    async fn invalid_url_rejected() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryProxyStore::default();
        let err = store
            .add(make_proxy("not-a-url"))
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("invalid URL should be rejected"))?;
        assert!(matches!(
            err,
            crate::error::ProxyError::InvalidProxyUrl { .. }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn invalid_url_empty_host() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryProxyStore::default();
        let err = store
            .add(make_proxy("http://:8080"))
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("empty host URL should be rejected"))?;
        assert!(matches!(
            err,
            crate::error::ProxyError::InvalidProxyUrl { .. }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_metrics_updates() -> std::result::Result<(), Box<dyn std::error::Error>> {
        use tokio::task::JoinSet;

        let store = Arc::new(MemoryProxyStore::default());
        let record = store
            .add(make_proxy("http://proxy.test:3128"))
            .await
            .map_err(|e| std::io::Error::other(format!("failed to add proxy: {e}")))?;
        let id = record.id;

        let mut tasks = JoinSet::new();
        for i in 0u64..50 {
            let s = Arc::clone(&store);
            tasks.spawn(async move { s.update_metrics(id, i % 2 == 0, i * 10).await });
        }
        while let Some(res) = tasks.join_next().await {
            let inner = res.map_err(|e| std::io::Error::other(format!("join failed: {e}")))?;
            inner.map_err(|e| std::io::Error::other(format!("update_metrics failed: {e}")))?;
        }

        // Verify totals are internally consistent.
        let guard = store.inner.read().await;
        let metrics = guard
            .get(&id)
            .map(|(_, m)| Arc::clone(m))
            .ok_or_else(|| std::io::Error::other("missing metrics for inserted proxy"))?;
        drop(guard);

        let total = metrics.requests_total.load(Ordering::Relaxed);
        let successes = metrics.successes.load(Ordering::Relaxed);
        let failures = metrics.failures.load(Ordering::Relaxed);
        assert_eq!(total, 50);
        assert_eq!(successes + failures, 50);
        Ok(())
    }
}
