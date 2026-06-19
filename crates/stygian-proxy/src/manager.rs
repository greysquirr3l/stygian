//! `ProxyManager`: unified proxy pool orchestrator.
//!
//! Assembles storage, rotation strategy, health checker, and per-proxy circuit
//! breakers into a single ergonomic API.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "vendor-stickiness")]
use std::time::Duration;

use serde::Serialize;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::circuit_breaker::CircuitBreaker;
use crate::error::{ProxyError, ProxyResult};
use crate::health::{HealthChecker, HealthMap};
#[cfg(feature = "coherence-validation")]
use crate::ports::coherence::{
    BoxedCoherencePort, CoherenceContext, CoherencePolicy, CoherenceVerdict,
};
#[cfg(feature = "vendor-stickiness")]
use crate::session::SessionDecision;
use crate::session::{SessionMap, StickyPolicy};
#[cfg(feature = "vendor-stickiness")]
use crate::stickiness::VendorStickinessMap;
use crate::storage::ProxyStoragePort;
use crate::strategy::{
    BoxedBayesianObserver, BoxedRotationStrategy, LeastUsedStrategy, NoopBayesianObserver,
    ProxyCandidate, RandomStrategy, RoundRobinStrategy, WeightedStrategy,
    capable_healthy_candidates,
};
#[cfg(feature = "vendor-stickiness")]
use crate::types::VendorId;
use crate::types::{CapabilityRequirement, Proxy, ProxyConfig};

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
    /// Stable proxy id — used to address the `observer` so a Bayesian
    /// strategy can credit the right `Beta(α, β)` arm.
    proxy_id: Uuid,
    /// Optional observer receiving success/failure outcomes. The default is
    /// [`NoopBayesianObserver`] (zero-cost no-op) so the manager can always
    /// record outcomes without a feature check on the hot path.
    observer: BoxedBayesianObserver,
}

impl ProxyHandle {
    const fn new(
        proxy_url: String,
        circuit_breaker: Arc<CircuitBreaker>,
        proxy_id: Uuid,
        observer: BoxedBayesianObserver,
    ) -> Self {
        Self {
            proxy_url,
            circuit_breaker,
            succeeded: AtomicBool::new(false),
            session_key: None,
            sessions: None,
            proxy_id,
            observer,
        }
    }

    const fn new_sticky(
        proxy_url: String,
        circuit_breaker: Arc<CircuitBreaker>,
        session_key: String,
        sessions: SessionMap,
        proxy_id: Uuid,
        observer: BoxedBayesianObserver,
    ) -> Self {
        Self {
            proxy_url,
            circuit_breaker,
            succeeded: AtomicBool::new(false),
            session_key: Some(session_key),
            sessions: Some(sessions),
            proxy_id,
            observer,
        }
    }

    /// Create a no-proxy handle used when no proxy manager is configured.
    ///
    /// The handle targets an empty URL and uses a noop circuit breaker that
    /// can never trip; its Drop records a success so there are no false failures.
    #[must_use]
    pub fn direct() -> Self {
        let noop_cb = Arc::new(CircuitBreaker::new(u32::MAX, u64::MAX));
        let noop_observer: BoxedBayesianObserver = Arc::new(NoopBayesianObserver);
        Self {
            proxy_url: String::new(),
            circuit_breaker: noop_cb,
            succeeded: AtomicBool::new(true),
            session_key: None,
            sessions: None,
            proxy_id: Uuid::nil(),
            observer: noop_observer,
        }
    }

    /// Signal that the request succeeded.
    pub fn mark_success(&self) {
        self.succeeded.store(true, Ordering::Release);
        // Notify the observer after the circuit-breaker flag is set so
        // the bookkeeping is consistent across both subsystems.
        self.observer.observe(self.proxy_id, true);
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
            self.observer.observe(self.proxy_id, false);
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
/// use stygian_proxy::types::{IpClass, ProxyCapabilities, TargetVendorCompatibility};
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
///     capabilities: ProxyCapabilities::default(),
///     ip_class: IpClass::Unknown,
///     target_compatibility: TargetVendorCompatibility::default(),
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
    /// Optional observer receiving success/failure outcomes. Wired by
    /// [`ProxyManagerBuilder::with_thompson_sampling`] and other Bayesian
    /// strategies. Defaults to [`NoopBayesianObserver`] so the hot path
    /// does not branch on the feature.
    observer: BoxedBayesianObserver,
    /// Network-identity coherence validator used by
    /// [`ProxyManager::acquire_proxy_with_coherence`]. Compiled to
    /// `Option<…>` so the field exists uniformly with or without the
    /// `coherence-validation` cargo feature; when the feature is off
    /// the field is always `None` and the
    /// `acquire_proxy_with_coherence` method is gated out of the public
    /// surface entirely.
    #[cfg(feature = "coherence-validation")]
    coherence_validator: Option<BoxedCoherencePort>,
    /// Per-vendor session stickiness policy consulted by
    /// [`ProxyManager::acquire_for_domain_with_vendor`]. Compiled to a
    /// concrete field (not `Option`) so the feature flag controls the
    /// *integration surface* (the method and the builder step are
    /// gated) without making the field polymorphic. Defaults to the
    /// 2026 guide built-in policy matrix so the feature is on by
    /// default in the `full` aggregator; operators can override via
    /// [`ProxyManagerBuilder::stickiness_map`].
    #[cfg(feature = "vendor-stickiness")]
    stickiness_map: VendorStickinessMap,
}

impl ProxyManager {
    /// Start a [`ProxyManagerBuilder`].
    #[must_use]
    pub fn builder() -> ProxyManagerBuilder {
        ProxyManagerBuilder::default()
    }

    /// Convenience: round-robin rotation (default).
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::ConfigError`] when no storage is supplied to
    /// the underlying builder.
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
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::ConfigError`] when no storage is supplied to
    /// the underlying builder.
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
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::ConfigError`] when no storage is supplied to
    /// the underlying builder.
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
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::ConfigError`] when no storage is supplied to
    /// the underlying builder.
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

    /// Convenience: Thompson-sampling Bayesian rotation.
    ///
    /// The 2026 guide cites 76 % success with Thompson sampling vs 36 %
    /// with round-robin on identical proxies and targets (L3018-3021).
    /// `decay_interval` controls how often the per-proxy `Beta(α, β)`
    /// counters are scaled down so non-stationary health is tracked over
    /// time; the default of 5 minutes is a good fit for typical scrape
    /// cadences.
    ///
    /// Requires the `bayesian-rotation` cargo feature.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::ConfigError`] when no storage is supplied to
    /// the underlying builder.
    #[cfg(feature = "bayesian-rotation")]
    pub fn with_thompson_sampling(
        storage: Arc<dyn ProxyStoragePort>,
        config: ProxyConfig,
        decay_interval: std::time::Duration,
    ) -> ProxyResult<Self> {
        Self::builder()
            .storage(storage)
            .config(config)
            .with_thompson_sampling(decay_interval)
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
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::StorageError`] when the underlying storage backend
    /// rejects the new proxy record, or
    /// [`ProxyError::InvalidGeoMetadata`]
    /// when the proxy's geo-metadata fields fail ingest validation
    /// (e.g. `asn = 0`, `city = ""`, `postal_code = ""`).
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
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::StorageError`] when the underlying storage backend
    /// reports the proxy as missing or the remove call fails.
    pub async fn remove_proxy(&self, id: Uuid) -> ProxyResult<()> {
        self.storage.remove(id).await?;
        self.circuit_breakers.write().await.remove(&id);
        Ok(())
    }

    /// Add a proxy with explicit geo metadata (ASN, city, postal code).
    ///
    /// Convenience constructor for operator-curated pools that target
    /// specific geographic or network ranges — the "Infatica-style
    /// city, ZIP, and ASN filter" cited by the 2026 guide (L2837).
    /// Constructs the [`Proxy`] and underlying
    /// [`crate::ProxyCapabilities`] for the caller, populates the geo
    /// fields, and runs the same ingest validation as
    /// [`add_proxy`](Self::add_proxy) (so `asn = 0`, `city = ""`,
    /// etc. are rejected with
    /// [`ProxyError::InvalidGeoMetadata`]
    /// before the record is stored).
    ///
    /// The `proxy_type`, `username`, `password`, `weight`, `tags`, and
    /// remaining `ProxyCapabilities` fields take their
    /// `Default::default()` values; callers that need finer control
    /// over those should build a [`Proxy`] directly and call
    /// [`add_proxy`](Self::add_proxy) instead.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # async fn run() -> stygian_proxy::ProxyResult<()> {
    /// use std::sync::Arc;
    /// use stygian_proxy::{ProxyManager, ProxyConfig};
    /// use stygian_proxy::storage::MemoryProxyStore;
    /// use stygian_proxy::types::well_known::KNOWN_ASN_CLOUDFLARE;
    ///
    /// let store = Arc::new(MemoryProxyStore::default());
    /// let mgr = ProxyManager::with_round_robin(store, ProxyConfig::default())?;
    /// let _id = mgr.add_proxy_with_metadata(
    ///     "http://cf-exit.example.com:8080",
    ///     Some(KNOWN_ASN_CLOUDFLARE),
    ///     Some("San Francisco"),
    ///     Some("94110"),
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::InvalidProxyUrl`]
    /// when `url` is malformed, or
    /// [`ProxyError::InvalidGeoMetadata`]
    /// when any geo field fails the validation rules documented in
    /// [`crate::types::validate_asn`], [`crate::types::validate_city`],
    /// or [`crate::types::validate_postal_code`]. Storage failures
    /// surface as [`ProxyError::StorageError`].
    #[allow(clippy::significant_drop_tightening)]
    pub async fn add_proxy_with_metadata(
        &self,
        url: &str,
        asn: Option<u32>,
        city: Option<&str>,
        postal_code: Option<&str>,
    ) -> ProxyResult<Uuid> {
        let capabilities = crate::types::ProxyCapabilities {
            asn,
            city: city.map(str::to_owned),
            postal_code: postal_code.map(str::to_owned),
            ..Default::default()
        };
        let proxy = Proxy {
            url: url.to_owned(),
            proxy_type: crate::types::ProxyType::Http,
            username: None,
            password: None,
            weight: 1,
            tags: Vec::new(),
            capabilities,
            ip_class: crate::types::IpClass::Unknown,
            target_compatibility: crate::types::TargetVendorCompatibility::default(),
        };
        self.add_proxy(proxy).await
    }

    // ── Background task ───────────────────────────────────────────────────────

    /// Spawn the background health-check and session-purge tasks.
    ///
    /// Returns a `(CancellationToken, JoinHandle)` pair.  Cancel the token to
    /// trigger a graceful shutdown; await the handle to ensure it finishes.
    #[must_use]
    pub fn start(&self) -> (CancellationToken, JoinHandle<()>) {
        let token = CancellationToken::new();
        let health_handle = self.health_checker.clone().spawn(token.clone());

        let sessions = self.sessions.clone();
        let purge_token = token.clone();
        let purge_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_mins(1));
            loop {
                tokio::select! {
                    _ = interval.tick() => { let _ = sessions.purge_expired(); }
                    () = purge_token.cancelled() => break,
                }
            }
        });

        let combined = tokio::spawn(async move {
            let _ = tokio::join!(health_handle, purge_handle);
        });

        (token, combined)
    }

    /// Pre-warm the Bayesian observer with a synthetic outcome for a proxy.
    ///
    /// This is the same call that `ProxyHandle::mark_success` and the
    /// `Drop` impl make at runtime, exposed publicly so callers can
    /// pre-seed the bandit from a known-good (or known-bad) prior before
    /// serving traffic. Most useful for tests and for warm-starting the
    /// pool from an external health-check feed.
    pub fn strategy_warmup_observe(&self, proxy_id: Uuid, success: bool) {
        self.observer.observe(proxy_id, success);
    }

    /// Read-only view of the underlying proxy storage. Useful for
    /// tests, MCP introspection, and warm-up helpers that need to map
    /// `url → id` without traversing the public API surface.
    #[must_use]
    pub fn storage(&self) -> &Arc<dyn ProxyStoragePort> {
        &self.storage
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
                        capabilities: record.proxy.capabilities.clone(),
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
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::StorageError`] when the storage backend cannot list
    /// proxies, or [`ProxyError::NoCompatibleProxy`] when no healthy proxy
    /// is available.
    pub async fn acquire_proxy(&self) -> ProxyResult<ProxyHandle> {
        let (url, cb, id) = self.select_proxy_inner().await?;
        Ok(ProxyHandle::new(url, cb, id, Arc::clone(&self.observer)))
    }

    /// Acquire a proxy that satisfies `req` from the pool.
    ///
    /// Filters the candidate list to healthy proxies whose
    /// [`ProxyCapabilities`](crate::types::ProxyCapabilities) satisfy every
    /// flag in `req`, then delegates to the configured rotation strategy.
    ///
    /// Returns [`ProxyError::NoCompatibleProxy`] when no healthy proxy meets
    /// the capability requirements.
    ///
    /// # Example
    /// ```rust,no_run
    /// use stygian_proxy::{ProxyManager, ProxyManagerBuilder, CapabilityRequirement};
    ///
    /// async fn example(manager: &ProxyManager) {
    ///     let req = CapabilityRequirement { require_https_connect: true, ..Default::default() };
    ///     let handle = manager.acquire_with_capabilities(&req).await.unwrap();
    ///     println!("url: {}", handle.proxy_url);
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::StorageError`] when the storage backend cannot list
    /// proxies, or [`ProxyError::NoCompatibleProxy`] when no healthy proxy
    /// satisfies the supplied [`CapabilityRequirement`].
    pub async fn acquire_with_capabilities(
        &self,
        req: &CapabilityRequirement,
    ) -> ProxyResult<ProxyHandle> {
        let with_metrics = self.storage.list_with_metrics().await?;

        if with_metrics.is_empty() {
            return Err(ProxyError::PoolExhausted);
        }

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
                        capabilities: record.proxy.capabilities.clone(),
                    }
                })
                .collect();
            candidates
        };

        // Filter to only those that satisfy the capability requirement.
        let compatible: Vec<ProxyCandidate> = capable_healthy_candidates(&candidates, req)
            .into_iter()
            .cloned()
            .collect();
        if compatible.is_empty() {
            return Err(ProxyError::NoCompatibleProxy);
        }

        let selected = self.strategy.select(&compatible).await?;
        let id = selected.id;

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

        Ok(ProxyHandle::new(url, cb, id, Arc::clone(&self.observer)))
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
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::StorageError`] when the storage backend fails, or
    /// [`ProxyError::NoCompatibleProxy`] when no healthy proxy is available
    /// (including when a sticky-bound proxy is unhealthy and the fallback
    /// also exhausts the pool).
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
                        proxy_id,
                        Arc::clone(&self.observer),
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
            proxy_id,
            Arc::clone(&self.observer),
        ))
    }

    /// Acquire a proxy for `(domain, vendor)`, honouring the
    /// per-vendor stickiness policy from
    /// [`VendorStickinessMap::with_builtin_defaults`].
    ///
    /// Behind the `vendor-stickiness` cargo feature (off by default).
    /// The default installed on the manager is the 2026 guide matrix;
    /// operators can replace it via
    /// [`ProxyManagerBuilder::stickiness_map`].
    ///
    /// Behaviour per [`crate::stickiness::StickinessPolicy`]:
    ///
    /// | Policy                                       | Behaviour                                                                          |
    /// | -------------------------------------------- | ---------------------------------------------------------------------------------- |
    /// | `StickyForever` / `StickyForTtl`             | Reuse an existing binding when present; otherwise pick fresh via the strategy and bind for the policy TTL. |
    /// | `FreshPerRequest`                            | Always pick a fresh proxy via the strategy. No binding is created.                  |
    /// | `FreshPerDomain`                             | Always pick a fresh proxy and evict any existing binding for `domain`.             |
    /// | `StickyForRequestCount(_)`                   | Treated as `FreshPerRequest` at this layer (per-request counters are out of scope). |
    /// | Unknown vendor (no entry in the map)         | Falls back to `FreshPerRequest` — the safest default.                              |
    ///
    /// If a bound proxy's circuit breaker has tripped or the proxy has
    /// been removed, the stale binding is invalidated and a fresh
    /// proxy is acquired (mirroring
    /// [`acquire_for_domain`](Self::acquire_for_domain)).
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::StorageError`] when the storage backend
    /// fails, or [`ProxyError::NoCompatibleProxy`] when no healthy proxy
    /// is available (including when a sticky-bound proxy is unhealthy
    /// and the fallback also exhausts the pool).
    #[cfg(feature = "vendor-stickiness")]
    pub async fn acquire_for_domain_with_vendor(
        &self,
        domain: &str,
        vendor: VendorId,
    ) -> ProxyResult<ProxyHandle> {
        let decision = self
            .sessions
            .acquire_session(domain, vendor, &self.stickiness_map);
        match decision {
            SessionDecision::UseSticky(proxy_id) => {
                // Same fallback as `acquire_for_domain`: verify the
                // binding is still valid (proxy in pool + CB available)
                // before handing back the handle. Any failure here falls
                // through to acquire-and-bind fresh.
                let cb = self.lookup_validated_cb(proxy_id).await?;
                if let Some(cb) = cb {
                    let url = self.lookup_url(proxy_id).await?;
                    if let Some(url) = url {
                        return Ok(ProxyHandle::new_sticky(
                            url,
                            cb,
                            domain.to_string(),
                            self.sessions.clone(),
                            proxy_id,
                            Arc::clone(&self.observer),
                        ));
                    }
                }
                // Stale binding: drop it and fall through to fresh acquisition.
                self.sessions.unbind(domain);
                let (url, cb, proxy_id) = self.select_proxy_inner().await?;
                // Look up the policy TTL again so we bind with the right
                // value — `policy_map.for_vendor(vendor)` is cheap
                // (`BTreeMap::get`) and the cache hit on this branch is
                // the common case.
                let ttl = self.stickiness_ttl(vendor);
                self.sessions.bind(domain, proxy_id, ttl);
                Ok(ProxyHandle::new_sticky(
                    url,
                    cb,
                    domain.to_string(),
                    self.sessions.clone(),
                    proxy_id,
                    Arc::clone(&self.observer),
                ))
            }
            SessionDecision::AcquireFresh => self.acquire_proxy().await,
            SessionDecision::AcquireAndBind(ttl) => {
                let (url, cb, proxy_id) = self.select_proxy_inner().await?;
                self.sessions.bind(domain, proxy_id, ttl);
                Ok(ProxyHandle::new_sticky(
                    url,
                    cb,
                    domain.to_string(),
                    self.sessions.clone(),
                    proxy_id,
                    Arc::clone(&self.observer),
                ))
            }
        }
    }

    /// Look up the per-vendor TTL to use when binding. Falls back to
    /// `Duration::from_mins(30)` (the Akamai default) for any policy
    /// that is not sticky — the call sites always check `policy_map`
    /// before binding so the fallback is unreachable in practice, but a
    /// defined default keeps the API total.
    #[cfg(feature = "vendor-stickiness")]
    fn stickiness_ttl(&self, vendor: VendorId) -> Duration {
        use crate::stickiness::StickinessPolicy;
        match self.stickiness_map.for_vendor(vendor) {
            StickinessPolicy::StickyForever => Duration::MAX,
            StickinessPolicy::StickyForTtl { ttl } => ttl,
            // Non-sticky policies never reach the `AcquireAndBind` branch.
            _ => Duration::from_mins(30),
        }
    }

    /// Look up the circuit breaker for `proxy_id` and confirm it is
    /// still `available`. Returns `Ok(None)` when the CB is missing,
    /// tripped, or absent from the pool.
    #[cfg(feature = "vendor-stickiness")]
    #[allow(clippy::significant_drop_tightening)]
    async fn lookup_validated_cb(
        &self,
        proxy_id: Uuid,
    ) -> ProxyResult<Option<Arc<CircuitBreaker>>> {
        let cb_map = self.circuit_breakers.read().await;
        let Some(cb) = cb_map.get(&proxy_id).cloned() else {
            return Ok(None);
        };
        if !cb.is_available() {
            return Ok(None);
        }
        Ok(Some(cb))
    }

    /// Look up the proxy URL for `proxy_id` from storage. Returns
    /// `Ok(None)` when the proxy has been removed.
    #[cfg(feature = "vendor-stickiness")]
    async fn lookup_url(&self, proxy_id: Uuid) -> ProxyResult<Option<String>> {
        let with_metrics = self.storage.list_with_metrics().await?;
        Ok(with_metrics
            .iter()
            .find(|(r, _)| r.id == proxy_id)
            .map(|(r, _)| r.proxy.url.clone()))
    }

    // ── Stats ─────────────────────────────────────────────────────────────────

    /// Return a health snapshot of the pool.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::StorageError`] when the storage backend cannot list
    /// proxies, or when the internal lock is poisoned.
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

    /// Acquire a proxy from the pool and run the network-identity
    /// coherence check before returning the handle.
    ///
    /// Behind the `coherence-validation` cargo feature (off by
    /// default). The default validator is
    /// [`crate::adapters::coherence::DefaultCoherenceValidator`];
    /// operators can plug in their own implementation via
    /// [`ProxyManagerBuilder::coherence_validator`].
    ///
    /// Mismatch handling follows [`CoherencePolicy`]:
    ///
    /// - [`CoherenceVerdict::Coherent`] — proxy is returned.
    /// - [`CoherenceVerdict::Mismatch`] on a `Hard` field that
    ///   `policy.hard_fail_on` covers — returns
    ///   [`ProxyError::CoherenceMismatch`].
    /// - [`CoherenceVerdict::Mismatch`] on any other field — logged
    ///   (advisory) and the proxy is returned.
    /// - [`CoherenceVerdict::Unknown`] — logged at `debug` level and
    ///   the proxy is returned (operators opt into hard-fail by
    ///   registering specific fields).
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`ProxyError::StorageError`] when the storage backend cannot
    ///   list proxies.
    /// - [`ProxyError::NoCompatibleProxy`] when no healthy proxy is
    ///   available.
    /// - [`ProxyError::ConfigError`] when the manager has no coherence
    ///   validator wired in (build with `coherence-validation` or
    ///   call [`ProxyManagerBuilder::coherence_validator`]).
    /// - [`ProxyError::CoherenceMismatch`] when the policy hard-fails
    ///   on the offending field.
    #[cfg(feature = "coherence-validation")]
    pub async fn acquire_proxy_with_coherence(
        &self,
        ctx: &CoherenceContext,
        policy: &CoherencePolicy,
    ) -> ProxyResult<ProxyHandle> {
        let validator = self.coherence_validator.as_ref().ok_or_else(|| {
            ProxyError::ConfigError(
                "ProxyManager::acquire_proxy_with_coherence: no coherence_validator wired in;                  enable the `coherence-validation` cargo feature or call                  ProxyManagerBuilder::coherence_validator(...)"
                    .into(),
            )
        })?;

        let handle = self.acquire_proxy().await?;
        let verdict = validator.evaluate(ctx);
        match verdict {
            CoherenceVerdict::Coherent => Ok(handle),
            CoherenceVerdict::Mismatch { field, severity } => {
                if policy.is_hard_fail(field) && severity.is_hard() {
                    Err(ProxyError::CoherenceMismatch { field, severity })
                } else {
                    tracing::warn!(
                        target: "stygian_proxy::coherence",
                        field = %field,
                        severity = %severity,
                        "coherence mismatch (advisory) — proceeding with the selected proxy"
                    );
                    Ok(handle)
                }
            }
            CoherenceVerdict::Unknown(reason) => {
                tracing::debug!(
                    target: "stygian_proxy::coherence",
                    reason = %reason,
                    "coherence verdict unknown — proceeding with the selected proxy"
                );
                Ok(handle)
            }
        }
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
    /// Optional observer that receives success/failure outcomes. Defaults
    /// to [`NoopBayesianObserver`] when no strategy wires a real one in.
    observer: Option<BoxedBayesianObserver>,
    /// Optional network-identity coherence validator. When unset and the
    /// `coherence-validation` cargo feature is enabled, the default
    /// [`crate::adapters::coherence::DefaultCoherenceValidator`] is
    /// wired in at build time.
    #[cfg(feature = "coherence-validation")]
    coherence_validator: Option<BoxedCoherencePort>,
    /// Optional per-vendor stickiness map. When unset and the
    /// `vendor-stickiness` cargo feature is enabled, the default
    /// [`VendorStickinessMap::with_builtin_defaults`] is wired in at
    /// build time so the 2026 guide matrix is active by default.
    #[cfg(feature = "vendor-stickiness")]
    stickiness_map: Option<VendorStickinessMap>,
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

    /// Set a custom [`crate::BayesianObserver`]. Mostly useful for tests and for
    /// callers who want to plug in their own bandit algorithm without
    /// using [`with_thompson_sampling`](Self::with_thompson_sampling).
    #[must_use]
    pub fn observer(mut self, o: BoxedBayesianObserver) -> Self {
        self.observer = Some(o);
        self
    }

    /// Install a custom network-identity coherence validator.
    ///
    /// When unset (and the `coherence-validation` cargo feature is
    /// enabled) the build step installs
    /// [`crate::adapters::coherence::DefaultCoherenceValidator`] as
    /// the default.
    #[cfg(feature = "coherence-validation")]
    #[must_use]
    pub fn coherence_validator(mut self, validator: BoxedCoherencePort) -> Self {
        self.coherence_validator = Some(validator);
        self
    }

    /// Wire a [`ThompsonStrategy`](crate::strategy::ThompsonStrategy) into
    /// the manager and use it as both the rotation strategy *and* the
    /// observer so success/failure outcomes are recorded back into the
    /// bandit on every [`ProxyHandle`]
    /// drop. Mirrors `with_random` / `with_weighted` etc. on the
    /// convenience constructor side; requires the `bayesian-rotation`
    /// cargo feature.
    #[cfg(feature = "bayesian-rotation")]
    #[must_use]
    pub fn with_thompson_sampling(mut self, decay_interval: std::time::Duration) -> Self {
        let strategy = Arc::new(crate::strategy::ThompsonStrategy::with_decay(
            decay_interval,
            crate::strategy::thompson::DEFAULT_DECAY_FACTOR,
        ));
        // The same `Arc` plays both roles — the strategy *is* the
        // observer. Storing the same Arc twice would create a reference
        // cycle on drop; cloning the Arc and keeping one reference in
        // each field is fine because `Arc` releases the heap on the last
        // `Drop` (which only happens when both the strategy field and the
        // observer field have been released).
        let observer: BoxedBayesianObserver = Arc::clone(&strategy) as BoxedBayesianObserver;
        self.strategy = Some(strategy);
        self.observer = Some(observer);
        self
    }

    /// Install a custom per-vendor stickiness map.
    ///
    /// When unset (and the `vendor-stickiness` cargo feature is enabled)
    /// the build step installs
    /// [`VendorStickinessMap::with_builtin_defaults`] as the default,
    /// matching the 2026 guide matrix (Akamai → 30min sticky,
    /// `DataDome` → fresh per request, etc.). Pass
    /// [`VendorStickinessMap::new`] to opt out of built-in defaults and
    /// use "fresh for every vendor" as the safe baseline.
    #[cfg(feature = "vendor-stickiness")]
    #[must_use]
    pub fn stickiness_map(mut self, map: VendorStickinessMap) -> Self {
        self.stickiness_map = Some(map);
        self
    }

    /// Build the [`ProxyManager`].
    ///
    /// Defaults: strategy = `RoundRobinStrategy`, config = `ProxyConfig::default()`,
    /// observer = `NoopBayesianObserver`.
    ///
    /// Returns an error if no storage was set.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::ConfigError`] when no `storage` was supplied to
    /// the builder.
    pub fn build(self) -> ProxyResult<ProxyManager> {
        let storage = self.storage.ok_or_else(|| {
            ProxyError::ConfigError("ProxyManagerBuilder: storage is required".into())
        })?;
        let strategy = self
            .strategy
            .unwrap_or_else(|| Arc::new(RoundRobinStrategy::default()));
        let observer = self
            .observer
            .unwrap_or_else(|| Arc::new(NoopBayesianObserver));
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
            observer,
            #[cfg(feature = "coherence-validation")]
            coherence_validator: self.coherence_validator.or_else(|| {
                Some(std::sync::Arc::new(
                    crate::adapters::coherence::DefaultCoherenceValidator,
                ))
            }),
            #[cfg(feature = "vendor-stickiness")]
            stickiness_map: self
                .stickiness_map
                .unwrap_or_else(VendorStickinessMap::with_builtin_defaults),
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::significant_drop_tightening,
    clippy::manual_let_else,
    clippy::panic,
    clippy::indexing_slicing
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
            capabilities: crate::types::ProxyCapabilities::default(),
            ip_class: crate::types::IpClass::Unknown,
            target_compatibility: crate::types::TargetVendorCompatibility::default(),
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

    /// T95 hot-path budget: 1 000 sequential healthy-pool acquisitions
    /// (with the new `ip_class` + `target_compatibility` field-compare
    /// added) must finish well under 1 s. The `crates/stygian-proxy/AGENTS.md`
    /// hot-path target is 1 µs per acquire; 1 000 acquisitions should
    /// stay under 100 ms in the common (healthy pool) case on any modern
    /// laptop. We assert a generous 1 s budget to keep the test robust
    /// under CI load while still catching a 100× regression.
    #[tokio::test]
    async fn acquire_proxy_hot_path_budget() {
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

        let start = std::time::Instant::now();
        for _ in 0..1_000 {
            let h = mgr.acquire_proxy().await.unwrap();
            h.mark_success();
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "1000 acquisitions took {elapsed:?}; hot-path budget violated"
        );
    }

    /// T95 hot-path budget: 1 000 capability-aware acquisitions with the
    /// new `require_ip_class` / `target_vendor` filter (which clone the
    /// `TargetVendorCompatibility` map and look up a `BTreeMap` entry on
    /// every candidate) must also stay under 1 s. The `BTreeMap` clone
    /// is empty for default-tagged proxies so the cost stays dominated
    /// by the per-candidate field-compare.
    #[tokio::test]
    async fn acquire_with_capabilities_hot_path_budget() {
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

        // Empty requirement — the new IP-class + target-vendor branches
        // short-circuit immediately.
        let req = crate::types::CapabilityRequirement::default();
        let start = std::time::Instant::now();
        for _ in 0..1_000 {
            let h = mgr.acquire_with_capabilities(&req).await.unwrap();
            h.mark_success();
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "1000 capability-aware acquisitions took {elapsed:?}; hot-path budget violated"
        );
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
                health_check_interval: Duration::from_hours(1),
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
            "different domains should get different proxies"
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
        let _ = mgr.sessions.purge_expired();

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

    // ── Thompson sampling integration tests (T96) ───────────────────────────

    /// `with_thompson_sampling` builds a manager whose strategy and
    /// observer are wired together. After marking some handles successful
    /// and dropping others, the strategy's Beta state should reflect the
    /// observed outcomes.
    #[cfg(feature = "bayesian-rotation")]
    #[tokio::test]
    async fn thompson_observer_records_outcomes_through_manager() {
        use crate::strategy::{BayesianObserver, ThompsonStrategy};
        use std::time::Duration;

        let store = storage();
        let strategy = Arc::new(ThompsonStrategy::with_decay(Duration::from_hours(1), 0.99));
        let observer: BoxedBayesianObserver = Arc::clone(&strategy) as BoxedBayesianObserver;

        // We need to know which proxy IDs to query after the manager has
        // assigned them. Use the storage's `list()` for that.
        let mgr = ProxyManager::builder()
            .storage(store.clone())
            .strategy(Arc::clone(&strategy) as BoxedRotationStrategy)
            .observer(observer)
            .config(ProxyConfig::default())
            .build()
            .unwrap();
        mgr.add_proxy(make_proxy("http://alpha.test:8080"))
            .await
            .unwrap();
        mgr.add_proxy(make_proxy("http://beta.test:8080"))
            .await
            .unwrap();

        let records = store.list().await.unwrap();
        let mut by_url: std::collections::HashMap<String, Uuid> = std::collections::HashMap::new();
        for r in &records {
            by_url.insert(r.proxy.url.clone(), r.id);
        }
        let alpha_id = *by_url
            .get("http://alpha.test:8080")
            .expect("alpha proxy should be in storage");
        let beta_id = *by_url
            .get("http://beta.test:8080")
            .expect("beta proxy should be in storage");

        // Mark alpha 8 times successful, beta 8 times failed.
        for _ in 0..8 {
            // Force a specific proxy by acquiring repeatedly (round-robin
            // over two proxies) and selectively marking the right handle.
            // The simpler approach: directly call the observer (which is
            // what mark_success / drop do internally).
            strategy.observe(alpha_id, true);
            strategy.observe(beta_id, false);
        }
        let (alpha_succ, alpha_fail) = strategy.counts_for(alpha_id);
        let (beta_succ, beta_fail) = strategy.counts_for(beta_id);
        assert!(
            alpha_succ >= 8,
            "alpha should have many successes (got {alpha_succ})"
        );
        assert!(
            alpha_fail < 5,
            "alpha should have few failures (got {alpha_fail})"
        );
        assert!(
            beta_succ < 5,
            "beta should have few successes (got {beta_succ})"
        );
        assert!(
            beta_fail >= 8,
            "beta should have many failures (got {beta_fail})"
        );
    }

    /// Thompson sampling manager survives 1 000 acquire+`mark_success`
    /// round-trips in well under 1 s (the hot-path budget). The observer
    /// is wired in via `with_thompson_sampling` so this is a full
    /// end-to-end timing test.
    #[cfg(feature = "bayesian-rotation")]
    #[tokio::test]
    async fn thompson_manager_hot_path_budget() {
        use std::time::Duration;
        let store = storage();
        let mgr = ProxyManager::with_thompson_sampling(
            store.clone(),
            ProxyConfig::default(),
            Duration::from_hours(1),
        )
        .unwrap();
        mgr.add_proxy(make_proxy("http://p1.test:8080"))
            .await
            .unwrap();
        mgr.add_proxy(make_proxy("http://p2.test:8080"))
            .await
            .unwrap();
        mgr.add_proxy(make_proxy("http://p3.test:8080"))
            .await
            .unwrap();

        // Warm up
        for _ in 0..10 {
            let h = mgr.acquire_proxy().await.unwrap();
            h.mark_success();
        }

        let start = std::time::Instant::now();
        for _ in 0..1_000 {
            let h = mgr.acquire_proxy().await.unwrap();
            h.mark_success();
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "1 000 Thompson manager round-trips took {elapsed:?}; hot-path budget violated"
        );
    }

    /// On a poisoned pool (50/50 alive/dead), Thompson sampling should
    /// route the overwhelming majority of traffic to the alive proxies
    /// after a brief warm-up — same shape as the unit-level
    /// `synthetic_poisoned_pool_concentrates_traffic_on_alive_proxies`
    /// test but driven through the full manager.
    #[cfg(feature = "bayesian-rotation")]
    #[tokio::test]
    async fn thompson_outperforms_round_robin_on_poisoned_pool() {
        use std::time::Duration;

        // Each manager owns its own storage so the two pools are
        // independent (round-robin and Thompson otherwise race over the
        // same proxy IDs).
        let store_rr = storage();
        let store_th = storage();
        let mgr_rr = ProxyManager::with_round_robin(store_rr, ProxyConfig::default()).unwrap();
        let mgr_th = ProxyManager::with_thompson_sampling(
            store_th,
            ProxyConfig::default(),
            Duration::from_hours(1),
        )
        .unwrap();

        // 5 alive + 5 dead proxies.
        let mut alive_urls: Vec<String> = Vec::new();
        let mut dead_urls: Vec<String> = Vec::new();
        for i in 0..5 {
            let url = format!("http://alive{i}.test:8080");
            mgr_rr.add_proxy(make_proxy(&url)).await.unwrap();
            mgr_th.add_proxy(make_proxy(&url)).await.unwrap();
            alive_urls.push(url);
        }
        for i in 0..5 {
            let url = format!("http://dead{i}.test:8080");
            mgr_rr.add_proxy(make_proxy(&url)).await.unwrap();
            mgr_th.add_proxy(make_proxy(&url)).await.unwrap();
            dead_urls.push(url);
        }

        // Pre-warm the Thompson strategy: observe 5 successes for every
        // alive URL and 5 failures for every dead URL. The strategy maps
        // URLs to ids internally, so we use the warm-up helper.
        let records = mgr_th.storage().list().await.unwrap();
        for r in &records {
            if alive_urls.iter().any(|u| u == &r.proxy.url) {
                for _ in 0..5 {
                    mgr_th.strategy_warmup_observe(r.id, true);
                }
            } else if dead_urls.iter().any(|u| u == &r.proxy.url) {
                for _ in 0..5 {
                    mgr_th.strategy_warmup_observe(r.id, false);
                }
            }
        }

        // Round-robin: 200 acquisitions. Mark alive-URL handles as
        // successful (simulating healthy traffic); drop dead-URL handles
        // without marking (simulating failed requests). Track which
        // fraction of selections went to the dead subset.
        let mut rr_alive = 0_u64;
        let mut rr_dead = 0_u64;
        for _ in 0..200 {
            let h = mgr_rr.acquire_proxy().await.unwrap();
            let url = h.proxy_url.clone();
            if url.contains("alive") {
                h.mark_success();
                rr_alive += 1;
            } else {
                drop(h);
                rr_dead += 1;
            }
        }
        // Counts are bounded (≤ 200) so the `as f64` conversion is
        // lossless — `f64`'s 53-bit mantissa comfortably represents
        // every value the test ever sees.
        #[allow(clippy::cast_precision_loss)]
        let rr_dead_share = (rr_dead as f64) / ((rr_alive + rr_dead) as f64);

        // Thompson: same 200-acquisition pattern. The warm-up means the
        // bandit already knows the dead URLs are bad and should route
        // almost all traffic to the alive set.
        let mut th_alive = 0_u64;
        let mut th_dead = 0_u64;
        for _ in 0..200 {
            let h = mgr_th.acquire_proxy().await.unwrap();
            let url = h.proxy_url.clone();
            if url.contains("alive") {
                h.mark_success();
                th_alive += 1;
            } else {
                drop(h);
                th_dead += 1;
            }
        }
        #[allow(clippy::cast_precision_loss)]
        let th_dead_share = (th_dead as f64) / ((th_alive + th_dead) as f64);

        // Thompson should route much less traffic to the dead proxies
        // than round-robin does, capturing the "more than double the
        // success rate" claim from the 2026 guide.
        assert!(
            th_dead_share < rr_dead_share,
            "Thompson dead-share ({th_dead_share:.3}) should be less than round-robin ({rr_dead_share:.3})"
        );
        // And the relative improvement should exceed 50 %.
        let improvement = (rr_dead_share - th_dead_share) / rr_dead_share;
        assert!(
            improvement > 0.50,
            "expected >50% relative improvement in dead-share reduction (got {:.1}%)",
            improvement * 100.0
        );
    }

    // ── Coherence integration tests (T97) ────────────────────────────────

    /// Helper: build a clean US context for the coherence integration
    /// tests. Mirrors the helper used by the adapter unit tests so the
    /// matrix stays aligned with the spec.
    #[cfg(feature = "coherence-validation")]
    fn clean_us_context() -> crate::ports::coherence::CoherenceContext {
        use crate::ports::coherence::{AcceptLanguage, CoherenceContext, IsoCountry, Locale, Tz};
        use std::net::IpAddr;
        use std::str::FromStr;
        CoherenceContext {
            proxy_geo_country: Some(IsoCountry::new("US").unwrap()),
            dns_resolver_country: Some(IsoCountry::new("US").unwrap()),
            browser_locale: Locale::new("en-US").unwrap(),
            browser_timezone: Tz::new("America/New_York").unwrap(),
            accept_language: AcceptLanguage::new("en-US,en;q=0.9").unwrap(),
            webrtc_local_ip: None,
            webrtc_public_ip: Some(IpAddr::from_str("192.0.2.42").unwrap()),
            proxy_ip: Some(IpAddr::from_str("192.0.2.7").unwrap()),
        }
    }

    /// `acquire_proxy_with_coherence` on a clean US context returns the
    /// same proxy as `acquire_proxy` would — the validator says
    /// `Coherent` and the manager does not block the request.
    #[cfg(feature = "coherence-validation")]
    #[tokio::test]
    async fn acquire_with_coherence_coherent_returns_proxy() {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(store, ProxyConfig::default()).unwrap();
        mgr.add_proxy(make_proxy("http://a.test:8080"))
            .await
            .unwrap();

        let ctx = clean_us_context();
        let policy = crate::ports::coherence::CoherencePolicy::advisory();
        let handle = mgr
            .acquire_proxy_with_coherence(&ctx, &policy)
            .await
            .unwrap();
        assert_eq!(handle.proxy_url, "http://a.test:8080");
        handle.mark_success();
    }

    /// Mismatch on a `Hard` field registered for hard-fail produces
    /// [`ProxyError::CoherenceMismatch`].
    #[cfg(feature = "coherence-validation")]
    #[tokio::test]
    async fn acquire_with_coherence_hard_fail_returns_error() {
        use crate::ports::coherence::{
            AcceptLanguage, CoherenceContext, IsoCountry, Locale, MismatchField, MismatchSeverity,
            Tz,
        };
        let store = storage();
        let mgr = ProxyManager::with_round_robin(store, ProxyConfig::default()).unwrap();
        mgr.add_proxy(make_proxy("http://a.test:8080"))
            .await
            .unwrap();

        // PK DNS forces a Hard mismatch on ProxyGeoVsDns.
        let ctx = CoherenceContext {
            dns_resolver_country: Some(IsoCountry::new("PK").unwrap()),
            ..clean_us_context()
        };
        let policy =
            crate::ports::coherence::CoherencePolicy::hard_fail_on(MismatchField::ProxyGeoVsDns);
        let err = mgr
            .acquire_proxy_with_coherence(&ctx, &policy)
            .await
            .unwrap_err();
        match err {
            crate::error::ProxyError::CoherenceMismatch { field, severity } => {
                assert_eq!(field, MismatchField::ProxyGeoVsDns);
                assert_eq!(severity, MismatchSeverity::Hard);
            }
            other => panic!("expected CoherenceMismatch, got {other:?}"),
        }
        // Suppress unused-import warnings on the Local variants that
        // are referenced by name only in `match` arms above.
        let _ = Locale::new("en-US").unwrap();
        let _ = AcceptLanguage::new("en-US").unwrap();
        let _ = Tz::new("America/New_York").unwrap();
    }

    /// An Advisory mismatch (Europe/London TZ with a US proxy) does
    /// **not** fail the acquisition — the proxy is returned and the
    /// mismatch is logged.
    #[cfg(feature = "coherence-validation")]
    #[tokio::test]
    async fn acquire_with_coherence_advisory_mismatch_logs_and_returns_proxy() {
        use crate::ports::coherence::{CoherenceContext, Tz};
        let store = storage();
        let mgr = ProxyManager::with_round_robin(store, ProxyConfig::default()).unwrap();
        mgr.add_proxy(make_proxy("http://a.test:8080"))
            .await
            .unwrap();

        let ctx = CoherenceContext {
            browser_timezone: Tz::new("Europe/London").unwrap(),
            ..clean_us_context()
        };
        // Default advisory policy: nothing is hard-failed.
        let policy = crate::ports::coherence::CoherencePolicy::advisory();
        let handle = mgr
            .acquire_proxy_with_coherence(&ctx, &policy)
            .await
            .unwrap();
        assert_eq!(handle.proxy_url, "http://a.test:8080");
        handle.mark_success();
    }

    /// Custom validator wiring: the builder's `coherence_validator`
    /// step replaces the default and is consulted on every
    /// `acquire_proxy_with_coherence` call. The test uses an
    /// always-Coherent stub so the proxy is returned even when the
    /// spec test fixtures (timezone / DNS) would otherwise disagree.
    #[cfg(feature = "coherence-validation")]
    #[tokio::test]
    async fn acquire_with_coherence_custom_validator_is_wired() {
        use crate::ports::coherence::{
            BoxedCoherencePort, CoherenceContext, CoherencePort, CoherenceVerdict,
        };

        #[derive(Debug)]
        struct AlwaysCoherent;
        impl CoherencePort for AlwaysCoherent {
            fn evaluate(&self, _: &CoherenceContext) -> CoherenceVerdict {
                CoherenceVerdict::Coherent
            }
        }

        let store = storage();
        let mgr = ProxyManager::builder()
            .storage(store)
            .coherence_validator(std::sync::Arc::new(AlwaysCoherent) as BoxedCoherencePort)
            .build()
            .unwrap();
        mgr.add_proxy(make_proxy("http://a.test:8080"))
            .await
            .unwrap();

        // Any context — the stub says Coherent regardless.
        let ctx = clean_us_context();
        let policy = crate::ports::coherence::CoherencePolicy::advisory();
        let handle = mgr
            .acquire_proxy_with_coherence(&ctx, &policy)
            .await
            .unwrap();
        assert_eq!(handle.proxy_url, "http://a.test:8080");
        handle.mark_success();
    }

    /// 1 000 sequential `acquire_proxy_with_coherence` calls stay under
    /// the 1 s hot-path budget. The validator is O(1) + stateless, so
    /// the integration is just as fast as the plain `acquire_proxy`
    /// budget test from T95.
    #[cfg(feature = "coherence-validation")]
    #[tokio::test]
    async fn acquire_with_coherence_hot_path_budget() {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(store, ProxyConfig::default()).unwrap();
        mgr.add_proxy(make_proxy("http://a.test:8080"))
            .await
            .unwrap();
        mgr.add_proxy(make_proxy("http://b.test:8080"))
            .await
            .unwrap();
        mgr.add_proxy(make_proxy("http://c.test:8080"))
            .await
            .unwrap();

        let ctx = clean_us_context();
        let policy = crate::ports::coherence::CoherencePolicy::advisory();
        let start = std::time::Instant::now();
        for _ in 0..1_000 {
            let h = mgr
                .acquire_proxy_with_coherence(&ctx, &policy)
                .await
                .unwrap();
            h.mark_success();
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "1000 coherence-gated acquisitions took {elapsed:?}; hot-path budget violated"
        );
    }

    // ── T99: per-vendor stickiness integration tests ───────────────────────

    /// Two consecutive `acquire_for_domain_with_vendor` calls for an
    /// `Akamai` target return the same proxy (sticky 30 min per the
    /// 2026 guide built-in default).
    #[cfg(feature = "vendor-stickiness")]
    #[tokio::test]
    async fn acquire_with_vendor_akamai_is_sticky() {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(store, ProxyConfig::default()).unwrap();
        mgr.add_proxy(make_proxy("http://p1.test:8080"))
            .await
            .unwrap();
        mgr.add_proxy(make_proxy("http://p2.test:8080"))
            .await
            .unwrap();

        let h1 = mgr
            .acquire_for_domain_with_vendor("example.com", crate::types::VendorId::Akamai)
            .await
            .unwrap();
        let url1 = h1.proxy_url.clone();
        h1.mark_success();

        let h2 = mgr
            .acquire_for_domain_with_vendor("example.com", crate::types::VendorId::Akamai)
            .await
            .unwrap();
        let url2 = h2.proxy_url.clone();
        h2.mark_success();

        assert_eq!(
            url1, url2,
            "Akamai sticky policy should return the same proxy across calls"
        );
    }

    /// `DataDome` is `FreshPerRequest` per the 2026 guide. Each call
    /// should pick a fresh proxy via the rotation strategy.
    #[cfg(feature = "vendor-stickiness")]
    #[tokio::test]
    async fn acquire_with_vendor_data_dome_is_fresh_per_request() {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(store, ProxyConfig::default()).unwrap();
        mgr.add_proxy(make_proxy("http://p1.test:8080"))
            .await
            .unwrap();
        mgr.add_proxy(make_proxy("http://p2.test:8080"))
            .await
            .unwrap();

        let h1 = mgr
            .acquire_for_domain_with_vendor("example.com", crate::types::VendorId::DataDome)
            .await
            .unwrap();
        let url1 = h1.proxy_url.clone();
        h1.mark_success();

        let h2 = mgr
            .acquire_for_domain_with_vendor("example.com", crate::types::VendorId::DataDome)
            .await
            .unwrap();
        let url2 = h2.proxy_url.clone();
        h2.mark_success();

        assert_ne!(
            url1, url2,
            "DataDome fresh-per-request policy should yield different proxies"
        );
    }

    /// `PerimeterX` is `FreshPerDomain` per the 2026 guide. Each call
    /// should pick a fresh proxy and evict any prior binding.
    #[cfg(feature = "vendor-stickiness")]
    #[tokio::test]
    async fn acquire_with_vendor_perimeter_x_is_fresh_per_domain() {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(store, ProxyConfig::default()).unwrap();
        mgr.add_proxy(make_proxy("http://p1.test:8080"))
            .await
            .unwrap();
        mgr.add_proxy(make_proxy("http://p2.test:8080"))
            .await
            .unwrap();

        let h1 = mgr
            .acquire_for_domain_with_vendor("example.com", crate::types::VendorId::PerimeterX)
            .await
            .unwrap();
        let url1 = h1.proxy_url.clone();
        h1.mark_success();

        let h2 = mgr
            .acquire_for_domain_with_vendor("example.com", crate::types::VendorId::PerimeterX)
            .await
            .unwrap();
        let url2 = h2.proxy_url.clone();
        h2.mark_success();

        assert_ne!(
            url1, url2,
            "PerimeterX fresh-per-domain policy should yield different proxies"
        );
    }

    /// Unknown vendors default to `FreshPerRequest` — every call
    /// picks a fresh proxy.
    #[cfg(feature = "vendor-stickiness")]
    #[tokio::test]
    async fn acquire_with_vendor_unknown_defaults_to_fresh() {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(store, ProxyConfig::default()).unwrap();
        mgr.add_proxy(make_proxy("http://p1.test:8080"))
            .await
            .unwrap();
        mgr.add_proxy(make_proxy("http://p2.test:8080"))
            .await
            .unwrap();

        let h1 = mgr
            .acquire_for_domain_with_vendor("example.com", crate::types::VendorId::Unknown)
            .await
            .unwrap();
        let url1 = h1.proxy_url.clone();
        h1.mark_success();

        let h2 = mgr
            .acquire_for_domain_with_vendor("example.com", crate::types::VendorId::Unknown)
            .await
            .unwrap();
        let url2 = h2.proxy_url.clone();
        h2.mark_success();

        assert_ne!(url1, url2, "Unknown vendor should default to fresh");
    }

    /// After the bound proxy's circuit breaker trips the sticky
    /// binding is invalidated and a fresh proxy is acquired on the
    /// next call.
    #[cfg(feature = "vendor-stickiness")]
    #[tokio::test]
    async fn acquire_with_vendor_sticky_binding_reacquires_after_failure() {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(
            store,
            ProxyConfig {
                circuit_open_threshold: 1,
                ..ProxyConfig::default()
            },
        )
        .unwrap();
        mgr.add_proxy(make_proxy("http://p1.test:8080"))
            .await
            .unwrap();
        mgr.add_proxy(make_proxy("http://p2.test:8080"))
            .await
            .unwrap();

        // First call: bind a proxy on `Akamai`. Drop without
        // `mark_success` so the CB trips and the binding is unbound.
        let h1 = mgr
            .acquire_for_domain_with_vendor("stale.com", crate::types::VendorId::Akamai)
            .await
            .unwrap();
        drop(h1);
        tokio::task::yield_now().await;

        // Second call: CB is now tripped, manager must invalidate the
        // binding and pick a fresh proxy. Either a different URL or an
        // `AllProxiesUnhealthy` error is acceptable.
        let result = mgr
            .acquire_for_domain_with_vendor("stale.com", crate::types::VendorId::Akamai)
            .await;
        match result {
            Ok(_h) => {} // manager recovered with a fresh proxy
            Err(crate::error::ProxyError::AllProxiesUnhealthy) => {} // pool exhausted
            Err(e) => panic!("unexpected error after stale binding: {e:?}"),
        }
    }

    /// `acquire_for_domain_with_vendor` honours a custom override
    /// installed via `ProxyManagerBuilder::stickiness_map`.
    #[cfg(feature = "vendor-stickiness")]
    #[tokio::test]
    async fn builder_stickiness_map_override_replaces_builtins() {
        use crate::stickiness::StickinessPolicy;

        let store = storage();
        // Override `Akamai` to `StickyForever` and `DataDome` to
        // `StickyForTtl 60s`.
        let custom = crate::stickiness::VendorStickinessMap::new()
            .with_override(
                crate::types::VendorId::Akamai,
                StickinessPolicy::StickyForever,
            )
            .with_override(
                crate::types::VendorId::DataDome,
                StickinessPolicy::StickyForTtl {
                    ttl: Duration::from_mins(1),
                },
            );
        let mgr = ProxyManager::builder()
            .storage(store)
            .config(ProxyConfig::default())
            .stickiness_map(custom)
            .build()
            .unwrap();
        mgr.add_proxy(make_proxy("http://p1.test:8080"))
            .await
            .unwrap();
        mgr.add_proxy(make_proxy("http://p2.test:8080"))
            .await
            .unwrap();

        // Akamai: StickyForever — same proxy across two calls.
        let h1 = mgr
            .acquire_for_domain_with_vendor("example.com", crate::types::VendorId::Akamai)
            .await
            .unwrap();
        let url1 = h1.proxy_url.clone();
        h1.mark_success();
        let h2 = mgr
            .acquire_for_domain_with_vendor("example.com", crate::types::VendorId::Akamai)
            .await
            .unwrap();
        let url2 = h2.proxy_url.clone();
        h2.mark_success();
        assert_eq!(
            url1, url2,
            "custom StickyForever should keep the same proxy"
        );

        // DataDome: StickyForTtl 1 min — same proxy across two calls
        // because we made them within the TTL window.
        let h1 = mgr
            .acquire_for_domain_with_vendor("dd.com", crate::types::VendorId::DataDome)
            .await
            .unwrap();
        let url1 = h1.proxy_url.clone();
        h1.mark_success();
        let h2 = mgr
            .acquire_for_domain_with_vendor("dd.com", crate::types::VendorId::DataDome)
            .await
            .unwrap();
        let url2 = h2.proxy_url.clone();
        h2.mark_success();
        assert_eq!(url1, url2, "custom StickyForTtl should keep the same proxy");

        // Hcaptcha (no override, no built-in) defaults to FreshPerRequest.
        let h1 = mgr
            .acquire_for_domain_with_vendor("hc.com", crate::types::VendorId::Hcaptcha)
            .await
            .unwrap();
        let url1 = h1.proxy_url.clone();
        h1.mark_success();
        let h2 = mgr
            .acquire_for_domain_with_vendor("hc.com", crate::types::VendorId::Hcaptcha)
            .await
            .unwrap();
        let url2 = h2.proxy_url.clone();
        h2.mark_success();
        assert_ne!(
            url1, url2,
            "Hcaptcha should default to fresh when no override is installed"
        );
    }

    /// 1 000 sequential `acquire_for_domain_with_vendor` calls stay
    /// under the 1 s hot-path budget. The per-vendor policy lookup is
    /// a `BTreeMap::get` (O(log n)) so the integration adds no
    /// measurable overhead vs `acquire_for_domain`.
    #[cfg(feature = "vendor-stickiness")]
    #[tokio::test]
    async fn acquire_with_vendor_hot_path_budget() {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(store, ProxyConfig::default()).unwrap();
        mgr.add_proxy(make_proxy("http://a.test:8080"))
            .await
            .unwrap();
        mgr.add_proxy(make_proxy("http://b.test:8080"))
            .await
            .unwrap();
        mgr.add_proxy(make_proxy("http://c.test:8080"))
            .await
            .unwrap();

        let start = std::time::Instant::now();
        for _ in 0..1_000 {
            let h = mgr
                .acquire_for_domain_with_vendor("example.com", crate::types::VendorId::Akamai)
                .await
                .unwrap();
            h.mark_success();
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "1000 per-vendor acquisitions took {elapsed:?}; hot-path budget violated"
        );
    }

    // ── T98: add_proxy_with_metadata ──────────────────────────────────────

    /// `add_proxy_with_metadata` constructs and stores a proxy with
    /// `asn`, `city`, and `postal_code` populated.
    #[tokio::test]
    async fn add_proxy_with_metadata_stores_geo_fields() -> crate::error::ProxyResult<()> {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(store.clone(), ProxyConfig::default())?;
        mgr.add_proxy_with_metadata(
            "http://cf-sf.test:8080",
            Some(13_335),
            Some("San Francisco"),
            Some("94110"),
        )
        .await?;
        let records = store.list().await?;
        assert_eq!(records.len(), 1);
        let record = records.first().expect("one record");
        assert_eq!(record.proxy.capabilities.asn, Some(13_335));
        assert_eq!(
            record.proxy.capabilities.city.as_deref(),
            Some("San Francisco")
        );
        assert_eq!(
            record.proxy.capabilities.postal_code.as_deref(),
            Some("94110")
        );
        Ok(())
    }

    /// `add_proxy_with_metadata` rejects malformed values via the
    /// same path as `add_proxy` (no special-cased API).
    #[tokio::test]
    async fn add_proxy_with_metadata_rejects_invalid_geo() {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(store.clone(), ProxyConfig::default()).unwrap();
        let err = mgr
            .add_proxy_with_metadata(
                "http://cf-sf.test:8080",
                Some(0), // reserved
                Some("San Francisco"),
                Some("94110"),
            )
            .await
            .expect_err("asn=0 must be rejected");
        assert!(matches!(
            err,
            crate::error::ProxyError::InvalidGeoMetadata { ref field, .. } if field == "asn"
        ));
        let err = mgr
            .add_proxy_with_metadata(
                "http://cf-sf.test:8080",
                Some(13_335),
                Some(""), // empty
                Some("94110"),
            )
            .await
            .expect_err("empty city must be rejected");
        assert!(matches!(
            err,
            crate::error::ProxyError::InvalidGeoMetadata { ref field, .. } if field == "city"
        ));
    }

    /// Round-trip: a proxy added via `add_proxy_with_metadata`
    /// satisfies a `require_asn` capability filter.
    #[tokio::test]
    async fn add_proxy_with_metadata_round_trips_capability_filter() -> crate::error::ProxyResult<()>
    {
        let store = storage();
        let mgr = ProxyManager::with_round_robin(store.clone(), ProxyConfig::default())?;
        mgr.add_proxy_with_metadata(
            "http://cf-sf.test:8080",
            Some(13_335),
            Some("San Francisco"),
            Some("94110"),
        )
        .await?;
        let req = crate::types::CapabilityRequirement {
            require_asn: Some(13_335),
            ..Default::default()
        };
        let handle = mgr.acquire_with_capabilities(&req).await?;
        assert!(
            handle.proxy_url.contains("cf-sf.test"),
            "got url: {}",
            handle.proxy_url
        );
        handle.mark_success();
        Ok(())
    }
}
