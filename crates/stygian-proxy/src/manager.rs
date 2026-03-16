//! ProxyManager: unified proxy pool orchestrator.
//!
//! Assembles storage, rotation strategy, health checker, and per-proxy circuit
//! breakers into a single ergonomic API.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::circuit_breaker::CircuitBreaker;
use crate::error::{ProxyError, ProxyResult};
use crate::health::{HealthChecker, HealthMap};
use crate::storage::ProxyStoragePort;
use crate::strategy::{
    BoxedRotationStrategy, LeastUsedStrategy, ProxyCandidate, RandomStrategy, RoundRobinStrategy,
    WeightedStrategy,
};
use crate::types::{Proxy, ProxyConfig};

// ─────────────────────────────────────────────────────────────────────────────
// PoolStats
// ─────────────────────────────────────────────────────────────────────────────

/// A snapshot of pool health at a point in time.
#[derive(Debug, Serialize)]
pub struct PoolStats {
    /// Total proxies in the pool.
    pub total: usize,
    /// Proxies that passed the last health check.
    pub healthy: usize,
    /// Proxies whose circuit breaker is currently Open.
    pub open: usize,
}

// ─────────────────────────────────────────────────────────────────────────────
// ProxyHandle
// ─────────────────────────────────────────────────────────────────────────────

/// RAII guard returned from [`ProxyManager::acquire_proxy`].
///
/// Call [`mark_success`](ProxyHandle::mark_success) once the request using
/// this proxy completes successfully.  If the handle is dropped without a
/// success mark the circuit breaker is notified of a failure.
pub struct ProxyHandle {
    /// URL of the selected proxy.
    pub proxy_url: String,
    circuit_breaker: Arc<CircuitBreaker>,
    succeeded: AtomicBool,
}

impl ProxyHandle {
    fn new(proxy_url: String, circuit_breaker: Arc<CircuitBreaker>) -> Self {
        Self {
            proxy_url,
            circuit_breaker,
            succeeded: AtomicBool::new(false),
        }
    }

    /// Create a no-proxy handle used when no proxy manager is configured.
    ///
    /// The handle targets an empty URL and uses a noop circuit breaker that
    /// can never trip; its Drop records a success so there are no false failures.
    pub fn direct() -> Self {
        let noop_cb = Arc::new(CircuitBreaker::new(u32::MAX, u64::MAX));
        Self {
            proxy_url: String::new(),
            circuit_breaker: noop_cb,
            succeeded: AtomicBool::new(true),
        }
    }

    /// Signal that the request succeeded.
    pub fn mark_success(&self) {
        self.succeeded.store(true, Ordering::Release);
    }
}

impl std::fmt::Debug for ProxyHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyHandle")
            .field("proxy_url", &self.proxy_url)
            .finish_non_exhaustive()
    }
}

impl Drop for ProxyHandle {
    fn drop(&mut self) {
        if self.succeeded.load(Ordering::Acquire) {
            self.circuit_breaker.record_success();
        } else {
            self.circuit_breaker.record_failure();
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ProxyManager
// ─────────────────────────────────────────────────────────────────────────────

/// Unified proxy pool orchestrator.
///
/// Manage proxies via [`add_proxy`](ProxyManager::add_proxy) and
/// [`remove_proxy`](ProxyManager::remove_proxy), acquire one via
/// [`acquire_proxy`](ProxyManager::acquire_proxy), and start background
/// health checking with [`start`](ProxyManager::start).
///
/// # Quick start
///
/// ```rust,no_run
/// # async fn run() -> stygian_proxy::ProxyResult<()> {
/// use std::sync::Arc;
/// use stygian_proxy::{ProxyManager, ProxyConfig, Proxy, ProxyType};
/// use stygian_proxy::storage::MemoryProxyStore;
///
/// let storage = Arc::new(MemoryProxyStore::default());
/// let mgr = ProxyManager::with_round_robin(storage, ProxyConfig::default())?;
/// let (token, _handle) = mgr.start();
/// let proxy = mgr.add_proxy(Proxy {
///     url: "http://proxy.example.com:8080".into(),
///     proxy_type: ProxyType::Http,
///     username: None,
///     password: None,
///     weight: 1,
///     tags: vec![],
/// }).await?;
/// let handle = mgr.acquire_proxy().await?;
/// handle.mark_success();
/// token.cancel();
/// # Ok(())
/// # }
/// ```
pub struct ProxyManager {
    storage: Arc<dyn ProxyStoragePort>,
    strategy: BoxedRotationStrategy,
    health_checker: HealthChecker,
    circuit_breakers: Arc<RwLock<HashMap<Uuid, Arc<CircuitBreaker>>>>,
    config: ProxyConfig,
}

impl ProxyManager {
    /// Start a [`ProxyManagerBuilder`].
    pub fn builder() -> ProxyManagerBuilder {
        ProxyManagerBuilder::default()
    }

    /// Convenience: round-robin rotation (default).
    pub fn with_round_robin(
        storage: Arc<dyn ProxyStoragePort>,
        config: ProxyConfig,
    ) -> ProxyResult<Self> {
        Self::builder()
            .storage(storage)
            .strategy(Arc::new(RoundRobinStrategy::default()))
            .config(config)
            .build()
    }

    /// Convenience: random rotation.
    pub fn with_random(
        storage: Arc<dyn ProxyStoragePort>,
        config: ProxyConfig,
    ) -> ProxyResult<Self> {
        Self::builder()
            .storage(storage)
            .strategy(Arc::new(RandomStrategy))
            .config(config)
            .build()
    }

    /// Convenience: weighted rotation.
    pub fn with_weighted(
        storage: Arc<dyn ProxyStoragePort>,
        config: ProxyConfig,
    ) -> ProxyResult<Self> {
        Self::builder()
            .storage(storage)
            .strategy(Arc::new(WeightedStrategy))
            .config(config)
            .build()
    }

    /// Convenience: least-used rotation.
    pub fn with_least_used(
        storage: Arc<dyn ProxyStoragePort>,
        config: ProxyConfig,
    ) -> ProxyResult<Self> {
        Self::builder()
            .storage(storage)
            .strategy(Arc::new(LeastUsedStrategy))
            .config(config)
            .build()
    }

    // ── Pool mutations ────────────────────────────────────────────────────────

    /// Add a proxy and register a circuit breaker for it.  Returns the new ID.
    ///
    /// The `circuit_breakers` write lock is held for the duration of the storage
    /// write.  This is intentional: [`acquire_proxy`](Self::acquire_proxy) holds
    /// a read lock on the same map while it inspects candidates, so it cannot
    /// proceed past that point until both the storage record *and* its CB entry
    /// exist.  Without this ordering a concurrent `acquire_proxy` could select
    /// the new proxy before its CB was registered, breaking failure accounting.
    pub async fn add_proxy(&self, proxy: Proxy) -> ProxyResult<Uuid> {
        let mut cb_map = self.circuit_breakers.write().await;
        let record = self.storage.add(proxy).await?;
        cb_map.insert(
            record.id,
            Arc::new(CircuitBreaker::new(
                self.config.circuit_open_threshold,
                self.config.circuit_half_open_after.as_millis() as u64,
            )),
        );
        Ok(record.id)
    }

    /// Remove a proxy from the pool and drop its circuit breaker.
    pub async fn remove_proxy(&self, id: Uuid) -> ProxyResult<()> {
        self.storage.remove(id).await?;
        self.circuit_breakers.write().await.remove(&id);
        Ok(())
    }

    // ── Background task ───────────────────────────────────────────────────────

    /// Spawn the background health-check task.
    ///
    /// Returns a `(CancellationToken, JoinHandle)` pair.  Cancel the token to
    /// trigger a graceful shutdown; await the handle to ensure it finishes.
    pub fn start(&self) -> (CancellationToken, JoinHandle<()>) {
        let token = CancellationToken::new();
        let handle = self.health_checker.clone().spawn(token.clone());
        (token, handle)
    }

    // ── Proxy selection ───────────────────────────────────────────────────────

    /// Acquire a proxy from the pool.
    ///
    /// Builds [`ProxyCandidate`] entries from current storage, consulting the
    /// health map and each proxy's circuit breaker to set the `healthy` flag.
    /// Delegates selection to the configured [`RotationStrategy`].
    pub async fn acquire_proxy(&self) -> ProxyResult<ProxyHandle> {
        let with_metrics = self.storage.list_with_metrics().await?;
        if with_metrics.is_empty() {
            return Err(ProxyError::PoolExhausted);
        }

        let health_map: tokio::sync::RwLockReadGuard<'_, _> =
            self.health_checker.health_map().read().await;
        let cb_map = self.circuit_breakers.read().await;

        let candidates: Vec<ProxyCandidate> = with_metrics
            .iter()
            .map(|(record, metrics)| {
                // New proxies default to healthy until the first check fails.
                let healthy = health_map.get(&record.id).copied().unwrap_or(true);
                let available = cb_map
                    .get(&record.id)
                    .map(|cb| cb.is_available())
                    .unwrap_or(true);
                ProxyCandidate {
                    id: record.id,
                    weight: record.proxy.weight,
                    metrics: Arc::clone(metrics),
                    healthy: healthy && available,
                }
            })
            .collect();

        drop(health_map);
        let selected = self.strategy.select(&candidates).await?;
        let id = selected.id;

        // add_proxy() holds the circuit_breakers write lock for the full duration
        // of its storage write, so every proxy visible in candidates is guaranteed
        // to have a CB entry by the time we reach here.
        let cb = cb_map
            .get(&id)
            .cloned()
            .ok_or(ProxyError::PoolExhausted)?;

        let url = with_metrics
            .iter()
            .find(|(r, _)| r.id == id)
            .map(|(r, _)| r.proxy.url.clone())
            .unwrap_or_default();

        Ok(ProxyHandle::new(url, cb))
    }

    // ── Stats ─────────────────────────────────────────────────────────────────

    /// Return a health snapshot of the pool.
    pub async fn pool_stats(&self) -> ProxyResult<PoolStats> {
        let records = self.storage.list().await?;
        let total = records.len();
        let health_map = self.health_checker.health_map().read().await;
        let cb_map = self.circuit_breakers.read().await;

        let mut healthy = 0usize;
        let mut open = 0usize;
        for r in &records {
            if health_map.get(&r.id).copied().unwrap_or(true) {
                healthy += 1;
            }
            if cb_map
                .get(&r.id)
                .map(|cb| !cb.is_available())
                .unwrap_or(false)
            {
                open += 1;
            }
        }
        Ok(PoolStats {
            total,
            healthy,
            open,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ProxyManagerBuilder
// ─────────────────────────────────────────────────────────────────────────────

/// Fluent builder for [`ProxyManager`].
#[derive(Default)]
pub struct ProxyManagerBuilder {
    storage: Option<Arc<dyn ProxyStoragePort>>,
    strategy: Option<BoxedRotationStrategy>,
    config: Option<ProxyConfig>,
}

impl ProxyManagerBuilder {
    pub fn storage(mut self, s: Arc<dyn ProxyStoragePort>) -> Self {
        self.storage = Some(s);
        self
    }

    pub fn strategy(mut self, s: BoxedRotationStrategy) -> Self {
        self.strategy = Some(s);
        self
    }

    pub fn config(mut self, c: ProxyConfig) -> Self {
        self.config = Some(c);
        self
    }

    /// Build the [`ProxyManager`].
    ///
    /// Defaults: strategy = `RoundRobinStrategy`, config = `ProxyConfig::default()`.
    ///
    /// Returns an error if no storage was set.
    pub fn build(self) -> ProxyResult<ProxyManager> {
        let storage = self.storage.ok_or_else(|| {
            ProxyError::ConfigError("ProxyManagerBuilder: storage is required".into())
        })?;
        let strategy = self
            .strategy
            .unwrap_or_else(|| Arc::new(RoundRobinStrategy::default()));
        let config = self.config.unwrap_or_default();
        let health_map: HealthMap = Arc::new(RwLock::new(HashMap::new()));
        let health_checker = HealthChecker::new(
            config.clone(),
            Arc::clone(&storage),
            Arc::clone(&health_map),
        );
        Ok(ProxyManager {
            storage,
            strategy,
            health_checker,
            circuit_breakers: Arc::new(RwLock::new(HashMap::new())),
            config,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::time::Duration;

    use super::*;
    use crate::circuit_breaker::{STATE_CLOSED, STATE_OPEN};
    use crate::storage::MemoryProxyStore;
    use crate::types::ProxyType;

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

    fn storage() -> Arc<MemoryProxyStore> {
        Arc::new(MemoryProxyStore::default())
    }

    /// Round-robin across 3 proxies × 10 acquisitions should hit all three.
    #[tokio::test]
    async fn round_robin_distribution() {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(store.clone(), ProxyConfig::default()).unwrap();
        mgr.add_proxy(make_proxy("http://a.test:8080"))
            .await
            .unwrap();
        mgr.add_proxy(make_proxy("http://b.test:8080"))
            .await
            .unwrap();
        mgr.add_proxy(make_proxy("http://c.test:8080"))
            .await
            .unwrap();

        let mut seen = HashSet::new();
        for _ in 0..10 {
            let h = mgr.acquire_proxy().await.unwrap();
            h.mark_success();
            seen.insert(h.proxy_url.clone());
        }
        assert_eq!(seen.len(), 3, "all three proxies should have been selected");
    }

    /// When all circuit breakers are open the manager returns `AllProxiesUnhealthy`.
    #[tokio::test]
    async fn all_open_returns_error() {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(
            store.clone(),
            ProxyConfig {
                circuit_open_threshold: 1,
                ..ProxyConfig::default()
            },
        )
        .unwrap();
        let id = mgr
            .add_proxy(make_proxy("http://x.test:8080"))
            .await
            .unwrap();

        // Manually trip the circuit breaker.
        {
            let map = mgr.circuit_breakers.read().await;
            let cb = map.get(&id).unwrap();
            cb.record_failure();
        }

        let err = mgr.acquire_proxy().await.unwrap_err();
        assert!(
            matches!(err, ProxyError::AllProxiesUnhealthy),
            "expected AllProxiesUnhealthy, got {err:?}"
        );
    }

    /// Dropping a handle without `mark_success` records a failure.
    #[tokio::test]
    async fn handle_drop_records_failure() {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(
            store.clone(),
            ProxyConfig {
                circuit_open_threshold: 1,
                ..ProxyConfig::default()
            },
        )
        .unwrap();
        let id = mgr
            .add_proxy(make_proxy("http://y.test:8080"))
            .await
            .unwrap();

        {
            let _h = mgr.acquire_proxy().await.unwrap();
            // drop without mark_success → failure recorded
        }

        let cb_map = mgr.circuit_breakers.read().await;
        let cb = cb_map.get(&id).unwrap();
        assert_eq!(cb.state(), STATE_OPEN);
    }

    /// A handle marked as successful keeps the circuit breaker Closed.
    #[tokio::test]
    async fn handle_success_keeps_closed() {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(store.clone(), ProxyConfig::default()).unwrap();
        let id = mgr
            .add_proxy(make_proxy("http://z.test:8080"))
            .await
            .unwrap();

        let h = mgr.acquire_proxy().await.unwrap();
        h.mark_success();
        drop(h);

        let cb_map = mgr.circuit_breakers.read().await;
        let cb = cb_map.get(&id).unwrap();
        assert_eq!(cb.state(), STATE_CLOSED);
    }

    /// start() launches the health checker and cancel causes clean exit.
    #[tokio::test]
    async fn start_and_graceful_shutdown() {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(
            store,
            ProxyConfig {
                health_check_interval: Duration::from_secs(3600),
                ..ProxyConfig::default()
            },
        )
        .unwrap();
        let (token, handle) = mgr.start();
        token.cancel();
        let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(result.is_ok(), "health checker task should exit within 1s");
    }
}
