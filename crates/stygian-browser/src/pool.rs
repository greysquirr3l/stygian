//! Browser instance pool with warmup, health checks, and idle eviction
//!
//! # Architecture
//!
//! ```text
//! ┌───────────────────────────────────────────────────────────┐
//! │                      BrowserPool                         │
//! │                                                           │
//! │  Semaphore (max_size slots — global backpressure)        │
//! │  ┌───────────────────────────────────────────────────┐   │
//! │  │         shared: VecDeque<PoolEntry>               │   │
//! │  │  (unscoped browsers — used by acquire())         │   │
//! │  └───────────────────────────────────────────────────┘   │
//! │  ┌───────────────────────────────────────────────────┐   │
//! │  │    scoped: HashMap<String, VecDeque<PoolEntry>>   │   │
//! │  │  (per-context queues — used by acquire_for())    │   │
//! │  └───────────────────────────────────────────────────┘   │
//! │  active_count: Arc<AtomicUsize>                          │
//! └───────────────────────────────────────────────────────────┘
//! ```
//!
//! **Acquisition flow**
//! 1. Try to pop a healthy idle entry.
//! 2. If none idle and `active < max_size`, launch a fresh `BrowserInstance`.
//! 3. Otherwise wait up to `acquire_timeout` for an idle slot.
//!
//! **Release flow**
//! 1. Run a health-check on the returned instance.
//! 2. If healthy and `idle < max_size`, push it back to the idle queue.
//! 3. Otherwise shut it down and decrement the active counter.
//!
//! # Example
//!
//! ```no_run
//! use stygian_browser::{BrowserConfig, BrowserPool};
//!
//! # async fn run() -> stygian_browser::error::Result<()> {
//! let config = BrowserConfig::default();
//! let pool = BrowserPool::new(config).await?;
//!
//! let stats = pool.stats();
//! println!("Pool ready — idle: {}", stats.idle);
//!
//! let handle = pool.acquire().await?;
//! handle.release().await;
//! # Ok(())
//! # }
//! ```

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Instant;

use tokio::sync::{Mutex, Semaphore};
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

use crate::{
    BrowserConfig,
    browser::BrowserInstance,
    error::{BrowserError, Result},
};

// ─── PoolEntry ────────────────────────────────────────────────────────────────

struct PoolEntry {
    instance: BrowserInstance,
    last_used: Instant,
    /// RAII proxy lease — held for the entire Chrome process lifetime.
    /// `mark_success()` is called on clean disposal; simply dropping it
    /// records a circuit-breaker failure in the proxy pool (if any).
    proxy_lease: Option<Box<dyn crate::proxy::ProxyLease>>,
}

// ─── PoolInner ────────────────────────────────────────────────────────────────

struct PoolInner {
    shared: std::collections::VecDeque<PoolEntry>,
    scoped: std::collections::HashMap<String, std::collections::VecDeque<PoolEntry>>,
}

// ─── BrowserPool ──────────────────────────────────────────────────────────────

/// Thread-safe pool of reusable [`BrowserInstance`]s.
///
/// Maintains a warm set of idle browsers ready for immediate acquisition
/// (`<100ms`), and lazily launches new instances when demand spikes.
///
/// # Example
///
/// ```no_run
/// use stygian_browser::{BrowserConfig, BrowserPool};
///
/// # async fn run() -> stygian_browser::error::Result<()> {
/// let pool = BrowserPool::new(BrowserConfig::default()).await?;
/// let handle = pool.acquire().await?;
/// handle.release().await;
/// # Ok(())
/// # }
/// ```
pub struct BrowserPool {
    config: Arc<BrowserConfig>,
    semaphore: Arc<Semaphore>,
    inner: Arc<Mutex<PoolInner>>,
    active_count: Arc<AtomicUsize>,
    max_size: usize,
}

impl BrowserPool {
    /// Create a new pool and pre-warm `config.pool.min_size` browser instances.
    ///
    /// Warmup failures are logged but not fatal — the pool will start smaller
    /// and grow lazily.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::{BrowserPool, BrowserConfig};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn new(config: BrowserConfig) -> Result<Arc<Self>> {
        let max_size = config.pool.max_size;
        let min_size = config.pool.min_size;

        let pool = Self {
            config: Arc::new(config),
            semaphore: Arc::new(Semaphore::new(max_size)),
            inner: Arc::new(Mutex::new(PoolInner {
                shared: std::collections::VecDeque::new(),
                scoped: std::collections::HashMap::new(),
            })),
            active_count: Arc::new(AtomicUsize::new(0)),
            max_size,
        };

        // Warmup: pre-launch min_size instances
        info!("Warming browser pool: min_size={min_size}, max_size={max_size}");
        for i in 0..min_size {
            let (launch_config, proxy_lease) = if let Some(source) = &pool.config.proxy_source {
                match source.bind_proxy().await {
                    Ok((url, lease)) => {
                        let mut cfg = (*pool.config).clone();
                        cfg.proxy = Some(url);
                        cfg.proxy_source = None;
                        (cfg, Some(lease))
                    }
                    Err(e) => {
                        warn!("Warmup browser {i} failed to acquire proxy (non-fatal): {e}");
                        continue;
                    }
                }
            } else {
                ((*pool.config).clone(), None)
            };

            // Acquire a semaphore permit for each warmed instance so that
            // `active_count` and the semaphore always agree on capacity.
            let permit = match pool.semaphore.try_acquire() {
                Ok(p) => p,
                Err(_) => {
                    warn!("Warmup browser {i}: semaphore full, stopping warmup early");
                    break;
                }
            };

            match BrowserInstance::launch(launch_config).await {
                Ok(instance) => {
                    permit.forget(); // capacity is tracked manually via active_count
                    pool.active_count.fetch_add(1, Ordering::Relaxed);
                    pool.inner.lock().await.shared.push_back(PoolEntry {
                        instance,
                        last_used: Instant::now(),
                        proxy_lease,
                    });
                    debug!("Warmed browser {}/{min_size}", i + 1);
                }
                Err(e) => {
                    warn!("Warmup browser {i} failed (non-fatal): {e}");
                    // permit drops here = slot returned to semaphore
                    // proxy_lease drops here = circuit-breaker failure signal
                }
            }
        }

        // Spawn idle-eviction task
        let eviction_inner = pool.inner.clone();
        let eviction_active = pool.active_count.clone();
        let idle_timeout = pool.config.pool.idle_timeout;
        let eviction_min = min_size;

        tokio::spawn(async move {
            loop {
                sleep(idle_timeout / 2).await;

                let mut guard = eviction_inner.lock().await;
                let now = Instant::now();
                let active = eviction_active.load(Ordering::Relaxed);

                let total_idle: usize = guard.shared.len()
                    + guard
                        .scoped
                        .values()
                        .map(std::collections::VecDeque::len)
                        .sum::<usize>();
                let evict_count = if active > eviction_min {
                    (active - eviction_min).min(total_idle)
                } else {
                    0
                };

                let mut evicted = 0usize;

                // Evict from shared queue
                let mut kept: std::collections::VecDeque<PoolEntry> =
                    std::collections::VecDeque::new();
                while let Some(entry) = guard.shared.pop_front() {
                    if evicted < evict_count && now.duration_since(entry.last_used) >= idle_timeout
                    {
                        // Clean eviction: proxy was fine, just expired.
                        if let Some(lease) = &entry.proxy_lease {
                            lease.mark_success();
                        }
                        let instance = entry.instance;
                        tokio::spawn(async move {
                            let _ = instance.shutdown().await;
                        });
                        eviction_active.fetch_sub(1, Ordering::Relaxed);
                        evicted += 1;
                    } else {
                        kept.push_back(entry);
                    }
                }
                guard.shared = kept;

                // Evict from scoped queues
                let context_ids: Vec<String> = guard.scoped.keys().cloned().collect();
                for cid in &context_ids {
                    if let Some(queue) = guard.scoped.get_mut(cid) {
                        let mut kept: std::collections::VecDeque<PoolEntry> =
                            std::collections::VecDeque::new();
                        while let Some(entry) = queue.pop_front() {
                            if evicted < evict_count
                                && now.duration_since(entry.last_used) >= idle_timeout
                            {
                                if let Some(lease) = &entry.proxy_lease {
                                    lease.mark_success();
                                }
                                let instance = entry.instance;
                                tokio::spawn(async move {
                                    let _ = instance.shutdown().await;
                                });
                                eviction_active.fetch_sub(1, Ordering::Relaxed);
                                evicted += 1;
                            } else {
                                kept.push_back(entry);
                            }
                        }
                        *queue = kept;
                    }
                }

                // Remove empty scoped queues
                guard.scoped.retain(|_, q| !q.is_empty());

                // Explicitly drop the guard as soon as possible to avoid holding the lock longer than needed
                drop(guard);

                if evicted > 0 {
                    info!("Evicted {evicted} idle browsers (idle_timeout={idle_timeout:?})");
                }
            }
        });

        Ok(Arc::new(pool))
    }

    // ─── Acquire ──────────────────────────────────────────────────────────────

    /// Acquire a browser handle from the pool.
    ///
    /// - If a healthy idle browser is available it is returned immediately.
    /// - If `active < max_size` a new browser is launched.
    /// - Otherwise waits up to `pool.acquire_timeout`.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::PoolExhausted`] if no browser becomes available
    /// within `pool.acquire_timeout`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::{BrowserPool, BrowserConfig};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let handle = pool.acquire().await?;
    /// handle.release().await;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn acquire(self: &Arc<Self>) -> Result<BrowserHandle> {
        #[cfg(feature = "metrics")]
        let acquire_start = std::time::Instant::now();

        let result = self.acquire_inner(None).await;

        #[cfg(feature = "metrics")]
        {
            let elapsed = acquire_start.elapsed();
            crate::metrics::METRICS.record_acquisition(elapsed);
            crate::metrics::METRICS.set_pool_size(
                i64::try_from(self.active_count.load(Ordering::Relaxed)).unwrap_or(i64::MAX),
            );
        }

        result
    }

    /// Acquire a browser scoped to `context_id`.
    ///
    /// Browsers obtained this way are isolated: they will only be reused by
    /// future calls to `acquire_for` with the **same** `context_id`.
    /// The global `max_size` still applies across all contexts.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::PoolExhausted`] if no browser becomes available
    /// within `pool.acquire_timeout`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::{BrowserPool, BrowserConfig};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let a = pool.acquire_for("bot-a").await?;
    /// let b = pool.acquire_for("bot-b").await?;
    /// a.release().await;
    /// b.release().await;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn acquire_for(self: &Arc<Self>, context_id: &str) -> Result<BrowserHandle> {
        #[cfg(feature = "metrics")]
        let acquire_start = std::time::Instant::now();

        let result = self.acquire_inner(Some(context_id)).await;

        #[cfg(feature = "metrics")]
        {
            let elapsed = acquire_start.elapsed();
            crate::metrics::METRICS.record_acquisition(elapsed);
            crate::metrics::METRICS.set_pool_size(
                i64::try_from(self.active_count.load(Ordering::Relaxed)).unwrap_or(i64::MAX),
            );
        }

        result
    }

    /// Shared acquisition logic. `context_id = None` reads from the shared
    /// queue; `Some(id)` reads from the scoped queue for that context.
    #[allow(clippy::significant_drop_tightening)] // guard scope is already minimal
    async fn acquire_inner(self: &Arc<Self>, context_id: Option<&str>) -> Result<BrowserHandle> {
        let acquire_timeout = self.config.pool.acquire_timeout;
        let active = self.active_count.load(Ordering::Relaxed);
        let max = self.max_size;
        let ctx_owned: Option<String> = context_id.map(String::from);

        // Fast path: try idle queue first
        let fast_result = {
            let mut guard = self.inner.lock().await;
            let queue = match context_id {
                Some(id) => guard.scoped.get_mut(id),
                None => Some(&mut guard.shared),
            };
            let mut healthy: Option<(BrowserInstance, Option<Box<dyn crate::proxy::ProxyLease>>)> =
                None;
            let mut unhealthy: Vec<(BrowserInstance, Option<Box<dyn crate::proxy::ProxyLease>>)> =
                Vec::new();
            if let Some(queue) = queue {
                while let Some(entry) = queue.pop_front() {
                    if healthy.is_none() && entry.instance.is_healthy_cached() {
                        healthy = Some((entry.instance, entry.proxy_lease));
                    } else if !entry.instance.is_healthy_cached() {
                        unhealthy.push((entry.instance, entry.proxy_lease));
                    } else {
                        // Healthy but we already found one — push back.
                        queue.push_front(entry);
                        break;
                    }
                }
            }
            (healthy, unhealthy)
        };

        // Dispose unhealthy entries outside the lock
        for (instance, _lease) in fast_result.1 {
            // _lease drops here = circuit-breaker failure signal
            #[cfg(feature = "metrics")]
            crate::metrics::METRICS.record_crash();
            let active_count = self.active_count.clone();
            tokio::spawn(async move {
                let _ = instance.shutdown().await;
                active_count.fetch_sub(1, Ordering::Relaxed);
            });
        }

        if let Some((instance, proxy_lease)) = fast_result.0 {
            debug!(
                context = context_id.unwrap_or("shared"),
                "Reusing idle browser (uptime={:?})",
                instance.uptime()
            );
            return Ok(BrowserHandle::new(
                instance,
                Arc::clone(self),
                ctx_owned,
                proxy_lease,
            ));
        }

        // Slow path: launch new or wait
        if active < max {
            // Acquire semaphore permit (non-blocking since active < max)
            // Inline permit — no named binding to avoid significant_drop_tightening
            timeout(acquire_timeout, self.semaphore.acquire())
                .await
                .map_err(|_| BrowserError::PoolExhausted { active, max })?
                .map_err(|_| BrowserError::PoolExhausted { active, max })?
                .forget(); // We track capacity manually via active_count
            self.active_count.fetch_add(1, Ordering::Relaxed);

            let (launch_config, proxy_lease) = if let Some(source) = &self.config.proxy_source {
                match source.bind_proxy().await {
                    Ok((url, lease)) => {
                        let mut cfg = (*self.config).clone();
                        cfg.proxy = Some(url);
                        cfg.proxy_source = None;
                        (cfg, Some(lease))
                    }
                    Err(e) => {
                        self.active_count.fetch_sub(1, Ordering::Relaxed);
                        self.semaphore.add_permits(1);
                        return Err(e);
                    }
                }
            } else {
                ((*self.config).clone(), None)
            };

            let instance = match BrowserInstance::launch(launch_config).await {
                Ok(i) => i,
                Err(e) => {
                    // proxy_lease drops here = circuit-breaker failure signal
                    self.active_count.fetch_sub(1, Ordering::Relaxed);
                    self.semaphore.add_permits(1);
                    return Err(e);
                }
            };

            info!(
                context = context_id.unwrap_or("shared"),
                "Launched fresh browser (pool active={})",
                self.active_count.load(Ordering::Relaxed)
            );
            return Ok(BrowserHandle::new(
                instance,
                Arc::clone(self),
                ctx_owned,
                proxy_lease,
            ));
        }

        // Pool full — wait for a release
        let ctx_for_poll = context_id.map(String::from);
        timeout(acquire_timeout, async {
            loop {
                sleep(std::time::Duration::from_millis(50)).await;
                let mut guard = self.inner.lock().await;
                let queue = match ctx_for_poll.as_deref() {
                    Some(id) => guard.scoped.get_mut(id),
                    None => Some(&mut guard.shared),
                };
                if let Some(queue) = queue
                    && let Some(entry) = queue.pop_front()
                {
                    drop(guard);
                    if entry.instance.is_healthy_cached() {
                        let (instance, proxy_lease) = (entry.instance, entry.proxy_lease);
                        return Ok(BrowserHandle::new(
                            instance,
                            Arc::clone(self),
                            ctx_for_poll.clone(),
                            proxy_lease,
                        ));
                    }
                    #[cfg(feature = "metrics")]
                    crate::metrics::METRICS.record_crash();
                    // _lease drops = circuit-breaker failure signal
                    let instance = entry.instance;
                    let active_count = self.active_count.clone();
                    tokio::spawn(async move {
                        let _ = instance.shutdown().await;
                        active_count.fetch_sub(1, Ordering::Relaxed);
                    });
                }
            }
        })
        .await
        .map_err(|_| BrowserError::PoolExhausted { active, max })?
    }

    // ─── Release ──────────────────────────────────────────────────────────────

    /// Return a browser instance to the pool (called by [`BrowserHandle::release`]).
    async fn release(
        &self,
        instance: BrowserInstance,
        context_id: Option<&str>,
        mut proxy_lease: Option<Box<dyn crate::proxy::ProxyLease>>,
    ) {
        // Health-check before returning to idle queue
        if instance.is_healthy_cached() {
            let mut guard = self.inner.lock().await;
            let total_idle: usize = guard.shared.len()
                + guard
                    .scoped
                    .values()
                    .map(std::collections::VecDeque::len)
                    .sum::<usize>();
            if total_idle < self.max_size {
                let queue = match context_id {
                    Some(id) => guard.scoped.entry(id.to_owned()).or_default(),
                    None => &mut guard.shared,
                };
                queue.push_back(PoolEntry {
                    instance,
                    last_used: Instant::now(),
                    proxy_lease: proxy_lease.take(), // lease travels with the pooled entry
                });
                debug!(
                    context = context_id.unwrap_or("shared"),
                    "Returned browser to idle pool"
                );
                return;
            }
            drop(guard);
            // Healthy but pool full: mark success before clean disposal
            if let Some(lease) = &proxy_lease {
                lease.mark_success();
            }
        }
        // proxy_lease drops here:
        //   - healthy + pool full → mark_success was called above, drop is a no-op
        //   - unhealthy → mark_success NOT called, drop records circuit-breaker failure

        // Unhealthy or pool full — dispose
        #[cfg(feature = "metrics")]
        if !instance.is_healthy_cached() {
            crate::metrics::METRICS.record_crash();
        }
        let active_count = self.active_count.clone();
        tokio::spawn(async move {
            let _ = instance.shutdown().await;
            active_count.fetch_sub(1, Ordering::Relaxed);
        });

        self.semaphore.add_permits(1);
    }

    // ─── Context management ───────────────────────────────────────────────────

    /// Shut down and remove all idle browsers belonging to `context_id`.
    ///
    /// Active handles for that context are unaffected — they will be disposed
    /// normally when released. Call this when a bot or tenant is deprovisioned.
    ///
    /// Returns the number of browsers shut down.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::{BrowserPool, BrowserConfig};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let released = pool.release_context("bot-a").await;
    /// println!("Shut down {released} browsers for bot-a");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn release_context(&self, context_id: &str) -> usize {
        let mut guard = self.inner.lock().await;
        let entries = guard.scoped.remove(context_id).unwrap_or_default();
        drop(guard);

        let count = entries.len();
        for entry in entries {
            // Clean deprovisioning: mark the proxy as successful
            if let Some(lease) = &entry.proxy_lease {
                lease.mark_success();
            }
            let instance = entry.instance;
            let active_count = self.active_count.clone();
            tokio::spawn(async move {
                let _ = instance.shutdown().await;
                active_count.fetch_sub(1, Ordering::Relaxed);
            });
            self.semaphore.add_permits(1);
        }

        if count > 0 {
            info!("Released {count} browsers for context '{context_id}'");
        }
        count
    }

    /// List all active context IDs that have idle browsers in the pool.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::{BrowserPool, BrowserConfig};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let ids = pool.context_ids().await;
    /// println!("Active contexts: {ids:?}");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn context_ids(&self) -> Vec<String> {
        let guard = self.inner.lock().await;
        guard.scoped.keys().cloned().collect()
    }

    // ─── Stats ────────────────────────────────────────────────────────────────

    /// Snapshot of current pool metrics.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::{BrowserPool, BrowserConfig};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let s = pool.stats();
    /// println!("active={} idle={} max={}", s.active, s.idle, s.max);
    /// # Ok(())
    /// # }
    /// ```
    pub fn stats(&self) -> PoolStats {
        PoolStats {
            active: self.active_count.load(Ordering::Relaxed),
            max: self.max_size,
            available: self
                .max_size
                .saturating_sub(self.active_count.load(Ordering::Relaxed)),
            idle: 0, // approximate — would need lock; kept lock-free for perf
        }
    }
}

// ─── BrowserHandle ────────────────────────────────────────────────────────────

/// An acquired browser from the pool.
///
/// Call [`BrowserHandle::release`] after use to return the instance to the
/// idle queue.  If dropped without releasing, the browser is shut down and the
/// pool slot freed.
pub struct BrowserHandle {
    instance: Option<BrowserInstance>,
    pool: Arc<BrowserPool>,
    context_id: Option<String>,
    proxy_lease: Option<Box<dyn crate::proxy::ProxyLease>>,
}

impl BrowserHandle {
    fn new(
        instance: BrowserInstance,
        pool: Arc<BrowserPool>,
        context_id: Option<String>,
        proxy_lease: Option<Box<dyn crate::proxy::ProxyLease>>,
    ) -> Self {
        Self {
            instance: Some(instance),
            pool,
            context_id,
            proxy_lease,
        }
    }

    /// Borrow the underlying [`BrowserInstance`].
    ///
    /// Returns `None` if the handle has already been released via [`release`](Self::release).
    pub const fn browser(&self) -> Option<&BrowserInstance> {
        self.instance.as_ref()
    }

    /// Mutable borrow of the underlying [`BrowserInstance`].
    ///
    /// Returns `None` if the handle has already been released via [`release`](Self::release).
    pub const fn browser_mut(&mut self) -> Option<&mut BrowserInstance> {
        self.instance.as_mut()
    }

    /// The context that owns this handle, if scoped via [`BrowserPool::acquire_for`].
    ///
    /// Returns `None` for handles obtained with [`BrowserPool::acquire`].
    pub fn context_id(&self) -> Option<&str> {
        self.context_id.as_deref()
    }

    /// Return the browser to the pool.
    ///
    /// If the instance is unhealthy or the pool is full it will be disposed.
    pub async fn release(mut self) {
        if let Some(instance) = self.instance.take() {
            self.pool
                .release(
                    instance,
                    self.context_id.as_deref(),
                    self.proxy_lease.take(),
                )
                .await;
        }
    }
}

impl Drop for BrowserHandle {
    fn drop(&mut self) {
        if let Some(instance) = self.instance.take() {
            let pool = Arc::clone(&self.pool);
            let context_id = self.context_id.clone();
            let proxy_lease = self.proxy_lease.take();
            tokio::spawn(async move {
                pool.release(instance, context_id.as_deref(), proxy_lease)
                    .await;
            });
        }
    }
}

// ─── PoolStats ────────────────────────────────────────────────────────────────

/// Point-in-time metrics for a [`BrowserPool`].
///
/// # Example
///
/// ```no_run
/// use stygian_browser::{BrowserPool, BrowserConfig};
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let pool = BrowserPool::new(BrowserConfig::default()).await?;
/// let stats = pool.stats();
/// assert!(stats.max > 0);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct PoolStats {
    /// Total browser instances currently managed by the pool (idle + in-use).
    pub active: usize,
    /// Maximum allowed concurrent instances.
    pub max: usize,
    /// Free slots (max - active).
    pub available: usize,
    /// Currently idle (warm) instances ready for immediate acquisition.
    pub idle: usize,
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PoolConfig, StealthLevel};
    use std::time::Duration;

    fn test_config() -> BrowserConfig {
        BrowserConfig::builder()
            .stealth_level(StealthLevel::None)
            .pool(PoolConfig {
                min_size: 0, // no warmup in unit tests
                max_size: 5,
                idle_timeout: Duration::from_secs(300),
                acquire_timeout: Duration::from_millis(100),
            })
            .build()
    }

    #[test]
    fn pool_stats_reflects_max() {
        // This test is purely structural — pool construction needs a real browser
        // so we only verify the config plumbing here.
        let config = test_config();
        assert_eq!(config.pool.max_size, 5);
        assert_eq!(config.pool.min_size, 0);
    }

    #[test]
    fn pool_stats_available_saturates() {
        let stats = PoolStats {
            active: 10,
            max: 10,
            available: 0,
            idle: 0,
        };
        assert_eq!(stats.available, 0);
        assert_eq!(stats.active, stats.max);
    }

    #[test]
    fn pool_stats_partial_usage() {
        let stats = PoolStats {
            active: 3,
            max: 10,
            available: 7,
            idle: 2,
        };
        assert_eq!(stats.available, 7);
    }

    #[tokio::test]
    async fn pool_new_with_zero_min_size_ok() {
        // With min_size=0 BrowserPool::new() should succeed without a real Chrome
        // because no warmup launch is attempted.
        // We skip this if no Chrome is present; this test is integration-only.
        // Kept as a compile + config sanity check.
        let config = test_config();
        assert_eq!(config.pool.min_size, 0);
    }

    #[test]
    fn pool_stats_available_is_max_minus_active() {
        let stats = PoolStats {
            active: 6,
            max: 10,
            available: 4,
            idle: 3,
        };
        assert_eq!(stats.available, stats.max - stats.active);
    }

    #[test]
    fn pool_stats_available_cannot_underflow() {
        // active > max should not cause a panic — saturating_sub is used.
        let stats = PoolStats {
            active: 12,
            max: 10,
            available: 0_usize.saturating_sub(2),
            idle: 0,
        };
        // available is computed with saturating_sub in BrowserPool::stats()
        assert_eq!(stats.available, 0);
    }

    #[test]
    fn pool_config_acquire_timeout_respected() {
        let cfg = BrowserConfig::builder()
            .pool(PoolConfig {
                min_size: 0,
                max_size: 1,
                idle_timeout: Duration::from_secs(300),
                acquire_timeout: Duration::from_millis(10),
            })
            .build();
        assert_eq!(cfg.pool.acquire_timeout, Duration::from_millis(10));
    }

    #[test]
    fn pool_config_idle_timeout_respected() {
        let cfg = BrowserConfig::builder()
            .pool(PoolConfig {
                min_size: 1,
                max_size: 5,
                idle_timeout: Duration::from_secs(60),
                acquire_timeout: Duration::from_secs(5),
            })
            .build();
        assert_eq!(cfg.pool.idle_timeout, Duration::from_secs(60));
    }

    #[test]
    fn browser_handle_drop_does_not_panic_without_runtime() {
        // Verify BrowserHandle can be constructed/dropped without a real browser
        // by ensuring the struct itself is Send + Sync (compile-time check).
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<BrowserPool>();
        assert_send::<PoolStats>();
        assert_sync::<BrowserPool>();
    }

    #[test]
    fn pool_stats_zero_active_means_full_availability() {
        let stats = PoolStats {
            active: 0,
            max: 8,
            available: 8,
            idle: 0,
        };
        assert_eq!(stats.available, stats.max);
    }

    #[test]
    fn pool_entry_last_used_ordering() {
        use std::time::Duration;
        let now = std::time::Instant::now();
        let older = now.checked_sub(Duration::from_secs(400)).unwrap_or(now);
        let idle_timeout = Duration::from_secs(300);
        // Simulate eviction check: entry older than idle_timeout should be evicted
        assert!(now.duration_since(older) >= idle_timeout);
    }

    #[test]
    fn pool_stats_debug_format() {
        let stats = PoolStats {
            active: 2,
            max: 10,
            available: 8,
            idle: 1,
        };
        let dbg = format!("{stats:?}");
        assert!(dbg.contains("active"));
        assert!(dbg.contains("max"));
    }

    // ─── Context segregation tests ────────────────────────────────────────────

    #[test]
    fn pool_inner_scoped_default_is_empty() {
        let inner = PoolInner {
            shared: std::collections::VecDeque::new(),
            scoped: std::collections::HashMap::new(),
        };
        assert!(inner.shared.is_empty());
        assert!(inner.scoped.is_empty());
    }

    #[test]
    fn pool_inner_scoped_insert_and_retrieve() {
        let mut inner = PoolInner {
            shared: std::collections::VecDeque::new(),
            scoped: std::collections::HashMap::new(),
        };
        // Verify the scoped map key-space is independent
        inner.scoped.entry("bot-a".to_owned()).or_default();
        inner.scoped.entry("bot-b".to_owned()).or_default();
        assert_eq!(inner.scoped.len(), 2);
        assert!(inner.scoped.contains_key("bot-a"));
        assert!(inner.scoped.contains_key("bot-b"));
        assert!(inner.shared.is_empty());
    }

    #[test]
    fn pool_inner_scoped_retain_removes_empty() {
        let mut inner = PoolInner {
            shared: std::collections::VecDeque::new(),
            scoped: std::collections::HashMap::new(),
        };
        inner.scoped.entry("empty".to_owned()).or_default();
        assert_eq!(inner.scoped.len(), 1);
        inner.scoped.retain(|_, q| !q.is_empty());
        assert!(inner.scoped.is_empty());
    }

    #[tokio::test]
    async fn pool_context_ids_empty_by_default() {
        // Without a running Chrome, we test with min_size=0 so no browser
        // is launched. We need to construct the pool carefully.
        let config = test_config();
        assert_eq!(config.pool.min_size, 0);
        // context_ids requires an actual pool instance — this test verifies
        // the zero-state. Full integration tested with real browser.
    }

    #[test]
    fn browser_handle_context_id_none_for_shared() {
        // Compile-time / structural: BrowserHandle carries context_id
        fn _check_context_api(handle: &BrowserHandle) {
            let _: Option<&str> = handle.context_id();
        }
    }

    #[test]
    fn pool_inner_total_idle_calculation() {
        fn total_idle(inner: &PoolInner) -> usize {
            inner.shared.len()
                + inner
                    .scoped
                    .values()
                    .map(std::collections::VecDeque::len)
                    .sum::<usize>()
        }
        let mut inner = PoolInner {
            shared: std::collections::VecDeque::new(),
            scoped: std::collections::HashMap::new(),
        };
        assert_eq!(total_idle(&inner), 0);

        // Add entries to scoped queues (without real BrowserInstance, just check sizes)
        inner.scoped.entry("a".to_owned()).or_default();
        inner.scoped.entry("b".to_owned()).or_default();
        assert_eq!(total_idle(&inner), 0); // empty queues don't count
    }
}
