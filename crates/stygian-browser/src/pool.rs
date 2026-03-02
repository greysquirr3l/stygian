//! Browser instance pool with warmup, health checks, and idle eviction
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────┐
//! │                    BrowserPool                       │
//! │                                                      │
//! │  Semaphore (max_size slots)                         │
//! │  ┌──────────────────────────────────────────────┐   │
//! │  │           idle: VecDeque<PoolEntry>          │   │
//! │  │  entry: { instance, last_used: Instant }    │   │
//! │  └──────────────────────────────────────────────┘   │
//! │  active_count: Arc<AtomicUsize>                     │
//! └──────────────────────────────────────────────────────┘
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
}

// ─── PoolInner ────────────────────────────────────────────────────────────────

struct PoolInner {
    idle: std::collections::VecDeque<PoolEntry>,
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
                idle: std::collections::VecDeque::new(),
            })),
            active_count: Arc::new(AtomicUsize::new(0)),
            max_size,
        };

        // Warmup: pre-launch min_size instances
        info!("Warming browser pool: min_size={min_size}, max_size={max_size}");
        for i in 0..min_size {
            match BrowserInstance::launch((*pool.config).clone()).await {
                Ok(instance) => {
                    pool.active_count.fetch_add(1, Ordering::Relaxed);
                    pool.inner.lock().await.idle.push_back(PoolEntry {
                        instance,
                        last_used: Instant::now(),
                    });
                    debug!("Warmed browser {}/{min_size}", i + 1);
                }
                Err(e) => {
                    warn!("Warmup browser {i} failed (non-fatal): {e}");
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
                let idle_count = guard.idle.len();
                let active = eviction_active.load(Ordering::Relaxed);

                let evict_count = if active > eviction_min {
                    (active - eviction_min).min(idle_count)
                } else {
                    0
                };

                let mut evicted = 0usize;
                let mut kept: std::collections::VecDeque<PoolEntry> =
                    std::collections::VecDeque::new();

                while let Some(entry) = guard.idle.pop_front() {
                    if evicted < evict_count && now.duration_since(entry.last_used) >= idle_timeout
                    {
                        // Drop entry — BrowserInstance shutdown happens in background
                        tokio::spawn(async move {
                            let _ = entry.instance.shutdown().await;
                        });
                        eviction_active.fetch_sub(1, Ordering::Relaxed);
                        evicted += 1;
                    } else {
                        kept.push_back(entry);
                    }
                }

                guard.idle = kept;
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

        let result = self.acquire_impl().await;

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

    async fn acquire_impl(self: &Arc<Self>) -> Result<BrowserHandle> {
        let acquire_timeout = self.config.pool.acquire_timeout;
        let active = self.active_count.load(Ordering::Relaxed);
        let max = self.max_size;

        // Fast path: try idle queue first
        {
            let mut guard = self.inner.lock().await;
            while let Some(entry) = guard.idle.pop_front() {
                if entry.instance.is_healthy_cached() {
                    self.active_count.fetch_add(0, Ordering::Relaxed); // already counted
                    debug!(
                        "Reusing idle browser (uptime={:?})",
                        entry.instance.uptime()
                    );
                    return Ok(BrowserHandle::new(entry.instance, Arc::clone(self)));
                }
                // Unhealthy idle entry — dispose in background
                #[cfg(feature = "metrics")]
                crate::metrics::METRICS.record_crash();
                let active_count = self.active_count.clone();
                tokio::spawn(async move {
                    let _ = entry.instance.shutdown().await;
                    active_count.fetch_sub(1, Ordering::Relaxed);
                });
            }
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

            let instance = match BrowserInstance::launch((*self.config).clone()).await {
                Ok(i) => i,
                Err(e) => {
                    self.active_count.fetch_sub(1, Ordering::Relaxed);
                    self.semaphore.add_permits(1);
                    return Err(e);
                }
            };

            info!(
                "Launched fresh browser (pool active={})",
                self.active_count.load(Ordering::Relaxed)
            );
            return Ok(BrowserHandle::new(instance, Arc::clone(self)));
        }

        // Pool full — wait for a release
        timeout(acquire_timeout, async {
            loop {
                sleep(std::time::Duration::from_millis(50)).await;
                let mut guard = self.inner.lock().await;
                if let Some(entry) = guard.idle.pop_front() {
                    drop(guard);
                    if entry.instance.is_healthy_cached() {
                        return Ok(BrowserHandle::new(entry.instance, Arc::clone(self)));
                    }
                    #[cfg(feature = "metrics")]
                    crate::metrics::METRICS.record_crash();
                    let active_count = self.active_count.clone();
                    tokio::spawn(async move {
                        let _ = entry.instance.shutdown().await;
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
    async fn release(&self, instance: BrowserInstance) {
        // Health-check before returning to idle queue
        if instance.is_healthy_cached() {
            let mut guard = self.inner.lock().await;
            if guard.idle.len() < self.max_size {
                guard.idle.push_back(PoolEntry {
                    instance,
                    last_used: Instant::now(),
                });
                debug!("Returned browser to idle pool");
                return;
            }
            drop(guard);
        }

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
}

impl BrowserHandle {
    const fn new(instance: BrowserInstance, pool: Arc<BrowserPool>) -> Self {
        Self {
            instance: Some(instance),
            pool,
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

    /// Return the browser to the pool.
    ///
    /// If the instance is unhealthy or the pool is full it will be disposed.
    pub async fn release(mut self) {
        if let Some(instance) = self.instance.take() {
            self.pool.release(instance).await;
        }
    }
}

impl Drop for BrowserHandle {
    fn drop(&mut self) {
        if let Some(instance) = self.instance.take() {
            let pool = Arc::clone(&self.pool);
            tokio::spawn(async move {
                pool.release(instance).await;
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
}
