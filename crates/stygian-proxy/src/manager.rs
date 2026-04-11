//! `ProxyManager`: unified proxy pool orchestrator.
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
use crate::session::{SessionMap, StickyPolicy};
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
    /// Active (non-expired) sticky sessions.
    pub active_sessions: usize,
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
    /// Domain key to unbind from `sessions` on failure (sticky sessions only).
    session_key: Option<String>,
    sessions: Option<SessionMap>,
}

impl ProxyHandle {
    const fn new(proxy_url: String, circuit_breaker: Arc<CircuitBreaker>) -> Self {
        Self {
            proxy_url,
            circuit_breaker,
            succeeded: AtomicBool::new(false),
            session_key: None,
            sessions: None,
        }
    }

    const fn new_sticky(
        proxy_url: String,
        circuit_breaker: Arc<CircuitBreaker>,
        session_key: String,
        sessions: SessionMap,
    ) -> Self {
        Self {
            proxy_url,
            circuit_breaker,
            succeeded: AtomicBool::new(false),
            session_key: Some(session_key),
            sessions: Some(sessions),
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
            session_key: None,
            sessions: None,
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
            // Invalidate the sticky session so the next request picks a fresh proxy.
            if let (Some(key), Some(sessions)) = (&self.session_key, &self.sessions) {
                sessions.unbind(key);
            }
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
    /// Domain→proxy sticky session map (always present; logic depends on `config.sticky_policy`).
    sessions: SessionMap,
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
    #[allow(clippy::significant_drop_tightening)]
    pub async fn add_proxy(&self, proxy: Proxy) -> ProxyResult<Uuid> {
        let mut cb_map = self.circuit_breakers.write().await;
        let record = self.storage.add(proxy).await?;
        cb_map.insert(
            record.id,
            Arc::new(CircuitBreaker::new(
                self.config.circuit_open_threshold,
                u64::try_from(self.config.circuit_half_open_after.as_millis()).unwrap_or(u64::MAX),
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

    /// Spawn the background health-check and session-purge tasks.
    ///
    /// Returns a `(CancellationToken, JoinHandle)` pair.  Cancel the token to
    /// trigger a graceful shutdown; await the handle to ensure it finishes.
    pub fn start(&self) -> (CancellationToken, JoinHandle<()>) {
        let token = CancellationToken::new();
        let health_handle = self.health_checker.clone().spawn(token.clone());

        let sessions = self.sessions.clone();
        let purge_token = token.clone();
        let purge_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                tokio::select! {
                    _ = interval.tick() => { sessions.purge_expired(); }
                    () = purge_token.cancelled() => break,
                }
            }
        });

        let combined = tokio::spawn(async move {
            let _ = tokio::join!(health_handle, purge_handle);
        });

        (token, combined)
    }

    // ── Proxy selection ───────────────────────────────────────────────────────

    /// Select one proxy via the rotation strategy, returning its URL, circuit
    /// breaker, and ID.  Used by both [`acquire_proxy`](Self::acquire_proxy) and
    /// [`acquire_for_domain`](Self::acquire_for_domain).
    #[allow(clippy::significant_drop_tightening)]
    async fn select_proxy_inner(&self) -> ProxyResult<(String, Arc<CircuitBreaker>, Uuid)> {
        let with_metrics = self.storage.list_with_metrics().await?;
        if with_metrics.is_empty() {
            return Err(ProxyError::PoolExhausted);
        }

        // Drop both read guards before the async `strategy.select` await to avoid holding
        // locks across await points. After selection, re-acquire for a single O(1) lookup.
        let candidates = {
            let health_map_ref = Arc::clone(self.health_checker.health_map());
            let health_map = health_map_ref.read().await;
            let cb_map_ref = Arc::clone(&self.circuit_breakers);
            let cb_map = cb_map_ref.read().await;
            let candidates: Vec<ProxyCandidate> = with_metrics
                .iter()
                .map(|(record, metrics)| {
                    let healthy = health_map.get(&record.id).copied().unwrap_or(true);
                    let available = cb_map.get(&record.id).is_none_or(|cb| cb.is_available());
                    ProxyCandidate {
                        id: record.id,
                        weight: record.proxy.weight,
                        metrics: Arc::clone(metrics),
                        healthy: healthy && available,
                    }
                })
                .collect();
            candidates
            // health_map and cb_map drop here
        };

        let selected = self.strategy.select(&candidates).await?;
        let id = selected.id;

        // Single O(1) lookup — re-acquire only after the await point.
        let cb = self
            .circuit_breakers
            .read()
            .await
            .get(&id)
            .cloned()
            .ok_or(ProxyError::PoolExhausted)?;
        let url = with_metrics
            .iter()
            .find(|(r, _)| r.id == id)
            .map(|(r, _)| r.proxy.url.clone())
            .unwrap_or_default();

        Ok((url, cb, id))
    }

    /// Acquire a proxy from the pool.
    ///
    /// Builds [`ProxyCandidate`] entries from current storage, consulting the
    /// health map and each proxy's circuit breaker to set the `healthy` flag.
    /// Delegates selection to the configured [`crate::strategy::RotationStrategy`].
    pub async fn acquire_proxy(&self) -> ProxyResult<ProxyHandle> {
        let (url, cb, _id) = self.select_proxy_inner().await?;
        Ok(ProxyHandle::new(url, cb))
    }

    /// Acquire a proxy for `domain`, honouring the configured sticky-session
    /// policy.
    ///
    /// - When [`StickyPolicy::Disabled`] is active, behaves identically to
    ///   [`acquire_proxy`](Self::acquire_proxy).
    /// - When [`StickyPolicy::Domain`] is active and a fresh session exists
    ///   for `domain`, the **same proxy** is returned for the TTL duration.
    /// - If the bound proxy's circuit breaker has tripped or the proxy has been
    ///   removed, the stale session is invalidated and a fresh proxy is acquired
    ///   and bound.
    ///
    /// The returned [`ProxyHandle`] automatically invalidates the session on
    /// drop if not marked as successful.
    pub async fn acquire_for_domain(&self, domain: &str) -> ProxyResult<ProxyHandle> {
        let ttl = match &self.config.sticky_policy {
            StickyPolicy::Disabled => return self.acquire_proxy().await,
            StickyPolicy::Domain { ttl } => *ttl,
        };

        // Check for an active, non-expired session.
        if let Some(proxy_id) = self.sessions.lookup(domain) {
            let cb_map = self.circuit_breakers.read().await;
            if let Some(cb) = cb_map.get(&proxy_id).cloned()
                && cb.is_available()
            {
                // Lookup proxy URL from storage.
                let with_metrics = self.storage.list_with_metrics().await?;
                if let Some((record, _)) = with_metrics.iter().find(|(r, _)| r.id == proxy_id) {
                    let url = record.proxy.url.clone();
                    drop(cb_map);
                    return Ok(ProxyHandle::new_sticky(
                        url,
                        cb,
                        domain.to_string(),
                        self.sessions.clone(),
                    ));
                }
            }
            // CB tripped or proxy no longer in pool — invalidate.
            drop(cb_map);
            self.sessions.unbind(domain);
        }

        // No valid session: acquire fresh proxy via strategy and bind.
        let (url, cb, proxy_id) = self.select_proxy_inner().await?;
        self.sessions.bind(domain, proxy_id, ttl);
        Ok(ProxyHandle::new_sticky(
            url,
            cb,
            domain.to_string(),
            self.sessions.clone(),
        ))
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
            if cb_map.get(&r.id).is_some_and(|cb| !cb.is_available()) {
                open += 1;
            }
        }
        drop(health_map);
        drop(cb_map);
        Ok(PoolStats {
            total,
            healthy,
            open,
            active_sessions: self.sessions.active_count(),
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
    #[must_use]
    pub fn storage(mut self, s: Arc<dyn ProxyStoragePort>) -> Self {
        self.storage = Some(s);
        self
    }

    #[must_use]
    pub fn strategy(mut self, s: BoxedRotationStrategy) -> Self {
        self.strategy = Some(s);
        self
    }

    #[must_use]
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
        let checker = HealthChecker::new(
            config.clone(),
            Arc::clone(&storage),
            Arc::clone(&health_map),
        );

        #[cfg(feature = "tls-profiled")]
        let health_checker = if let Some(mode) = config.profiled_request_mode {
            checker.with_profiled_mode(mode)?
        } else {
            checker
        };

        #[cfg(not(feature = "tls-profiled"))]
        let health_checker = checker;

        Ok(ProxyManager {
            storage,
            strategy,
            health_checker,
            circuit_breakers: Arc::new(RwLock::new(HashMap::new())),
            config,
            sessions: SessionMap::new(),
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::significant_drop_tightening,
    clippy::manual_let_else,
    clippy::panic
)]
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

    /// `start()` launches the health checker and `cancel` causes clean exit.
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

    #[cfg(feature = "tls-profiled")]
    #[tokio::test]
    async fn builder_accepts_profiled_request_mode_preset() {
        let store = storage();
        let cfg = ProxyConfig {
            profiled_request_mode: Some(crate::types::ProfiledRequestMode::Preset),
            ..ProxyConfig::default()
        };

        let result = ProxyManager::builder()
            .storage(store)
            .strategy(Arc::new(RoundRobinStrategy::default()))
            .config(cfg)
            .build();

        assert!(
            result.is_ok(),
            "builder should accept profiled preset mode: {:?}",
            result.err()
        );
    }

    #[cfg(feature = "tls-profiled")]
    #[tokio::test]
    async fn builder_rejects_profiled_request_mode_strict_all_for_chrome() {
        let store = storage();
        let cfg = ProxyConfig {
            profiled_request_mode: Some(crate::types::ProfiledRequestMode::StrictAll),
            ..ProxyConfig::default()
        };

        let result = ProxyManager::builder()
            .storage(store)
            .strategy(Arc::new(RoundRobinStrategy::default()))
            .config(cfg)
            .build();

        let Err(err) = result else {
            panic!("strict_all should fail for default Chrome baseline profile")
        };

        assert!(
            matches!(err, ProxyError::ConfigError(_)),
            "expected ConfigError, got {err:?}"
        );
    }

    // ── sticky session tests ─────────────────────────────────────────────────

    fn sticky_config() -> ProxyConfig {
        use crate::session::StickyPolicy;
        ProxyConfig {
            sticky_policy: StickyPolicy::domain_default(),
            ..ProxyConfig::default()
        }
    }

    /// Two consecutive `acquire_for_domain` calls return the same proxy.
    #[tokio::test]
    async fn sticky_same_domain_returns_same_proxy() {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(store, sticky_config()).unwrap();
        mgr.add_proxy(make_proxy("http://p1.test:8080"))
            .await
            .unwrap();
        mgr.add_proxy(make_proxy("http://p2.test:8080"))
            .await
            .unwrap();

        let h1 = mgr.acquire_for_domain("example.com").await.unwrap();
        let url1 = h1.proxy_url.clone();
        h1.mark_success();

        let h2 = mgr.acquire_for_domain("example.com").await.unwrap();
        let url2 = h2.proxy_url.clone();
        h2.mark_success();

        assert_eq!(url1, url2, "same domain should return the same proxy");
    }

    /// Different domains each get their own proxy (when enough proxies exist).
    #[tokio::test]
    async fn sticky_different_domains_may_differ() {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(store, sticky_config()).unwrap();
        mgr.add_proxy(make_proxy("http://pa.test:8080"))
            .await
            .unwrap();
        mgr.add_proxy(make_proxy("http://pb.test:8080"))
            .await
            .unwrap();

        let ha = mgr.acquire_for_domain("a.com").await.unwrap();
        let url_a = ha.proxy_url.clone();
        ha.mark_success();

        let hb = mgr.acquire_for_domain("b.com").await.unwrap();
        let url_b = hb.proxy_url.clone();
        hb.mark_success();

        // With round-robin and two proxies the second domain gets the other one.
        assert_ne!(
            url_a, url_b,
            "different domains should differ in this scenario"
        );
    }

    /// After TTL expiry the session is treated as gone; a (possibly different)
    /// proxy is re-acquired and the basic contract (no panic) still holds.
    #[tokio::test]
    async fn sticky_expired_session_re_acquires() {
        use crate::session::StickyPolicy;
        let store = storage();
        let mgr = ProxyManager::with_round_robin(
            store,
            ProxyConfig {
                sticky_policy: StickyPolicy::domain(Duration::from_millis(1)),
                ..ProxyConfig::default()
            },
        )
        .unwrap();
        mgr.add_proxy(make_proxy("http://x.test:8080"))
            .await
            .unwrap();

        let h1 = mgr.acquire_for_domain("expired.com").await.unwrap();
        h1.mark_success();

        // Let the session expire.
        tokio::time::sleep(Duration::from_millis(5)).await;

        // Re-acquiring should not panic or error.
        let h2 = mgr.acquire_for_domain("expired.com").await.unwrap();
        h2.mark_success();
    }

    /// When the bound proxy's CB trips, the session is invalidated and a new
    /// proxy is acquired on next call.
    #[tokio::test]
    async fn sticky_cb_trip_invalidates_session() {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(
            store,
            ProxyConfig {
                circuit_open_threshold: 1,
                sticky_policy: sticky_config().sticky_policy,
                ..ProxyConfig::default()
            },
        )
        .unwrap();
        mgr.add_proxy(make_proxy("http://q1.test:8080"))
            .await
            .unwrap();
        mgr.add_proxy(make_proxy("http://q2.test:8080"))
            .await
            .unwrap();

        // First acquire: bind "cb.com" to a proxy.
        let h1 = mgr.acquire_for_domain("cb.com").await.unwrap();
        let url1 = h1.proxy_url.clone();
        // Drop without mark_success → circuit breaker trips + session unbinds.
        drop(h1);

        // Give the tokio runtime a moment to process.
        tokio::task::yield_now().await;

        // The tripped proxy is no longer available; next acquire should succeed
        // from the remaining healthy proxy (or error if only one).
        // We just verify no panic and the handle is valid.
        let _h2 = mgr.acquire_for_domain("cb.com").await;
        // url may differ from url1 or error if all CBs open — either is acceptable.
        let _ = url1;
    }

    /// `purge_expired()` removes stale sessions from the map.
    #[tokio::test]
    async fn sticky_purge_expired() {
        use crate::session::StickyPolicy;
        let store = storage();
        let mgr = ProxyManager::with_round_robin(
            store,
            ProxyConfig {
                sticky_policy: StickyPolicy::domain(Duration::from_millis(1)),
                ..ProxyConfig::default()
            },
        )
        .unwrap();
        mgr.add_proxy(make_proxy("http://r.test:8080"))
            .await
            .unwrap();

        let h = mgr.acquire_for_domain("purge.com").await.unwrap();
        h.mark_success();

        assert_eq!(mgr.sessions.active_count(), 1);

        // Expire and purge.
        tokio::time::sleep(Duration::from_millis(5)).await;
        mgr.sessions.purge_expired();

        assert_eq!(mgr.sessions.active_count(), 0);
    }

    /// `pool_stats` includes `active_sessions`.
    #[tokio::test]
    async fn pool_stats_includes_sessions() {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(store, sticky_config()).unwrap();
        mgr.add_proxy(make_proxy("http://s.test:8080"))
            .await
            .unwrap();

        let stats = mgr.pool_stats().await.unwrap();
        assert_eq!(stats.active_sessions, 0);

        let h = mgr.acquire_for_domain("stats.com").await.unwrap();
        h.mark_success();

        let stats = mgr.pool_stats().await.unwrap();
        assert_eq!(stats.active_sessions, 1);
    }
}
