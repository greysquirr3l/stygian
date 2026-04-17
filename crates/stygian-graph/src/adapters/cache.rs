//! Cache adapters
//!
//! Three implementations available for different use-cases:
//!
//! | Adapter        | Eviction | TTL | Notes                        |
//! | ---------------- | ---------- | ----- | ------------------------------ |
//! | `MemoryCache`  | None     | No  | Dev/test                     |
//! | `DashMapCache` | None     | Yes | High-concurrency + background cleanup |
//! | `BoundedLruCache` | LRU  | Yes | Capacity-bounded; `LazyLock` singleton |

use crate::domain::error::Result;
use crate::ports::CachePort;
use async_trait::async_trait;
use dashmap::DashMap;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

/// In-memory cache adapter for testing and development
///
/// Uses a simple `HashMap` with `RwLock` for thread-safe access.
/// Does not implement TTL expiration (all entries persist until explicitly invalidated).
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::cache::MemoryCache;
/// use stygian_graph::ports::CachePort;
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let cache = MemoryCache::new();
/// cache.set("key", "value".to_string(), None).await.unwrap();
/// let value = cache.get("key").await.unwrap();
/// assert_eq!(value, Some("value".to_string()));
/// # });
/// ```
pub struct MemoryCache {
    store: Arc<RwLock<HashMap<String, String>>>,
}

impl MemoryCache {
    /// Create a new memory cache
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for MemoryCache {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CachePort for MemoryCache {
    async fn get(&self, key: &str) -> Result<Option<String>> {
        let value = {
            let store = self.store.read();
            store.get(key).cloned()
        };
        Ok(value)
    }

    async fn set(&self, key: &str, value: String, _ttl: Option<Duration>) -> Result<()> {
        {
            let mut store = self.store.write();
            store.insert(key.to_string(), value);
        }
        Ok(())
    }

    async fn invalidate(&self, key: &str) -> Result<()> {
        {
            let mut store = self.store.write();
            store.remove(key);
        }
        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool> {
        let exists = {
            let store = self.store.read();
            store.contains_key(key)
        };
        Ok(exists)
    }
}

// ─── TTL entry ────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct TtlEntry {
    value: String,
    expires_at: Option<Instant>,
}

impl TtlEntry {
    fn new(value: String, ttl: Option<Duration>) -> Self {
        Self {
            value,
            expires_at: ttl.map(|d| Instant::now() + d),
        }
    }

    fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|exp| Instant::now() > exp)
    }
}

// ─── DashMapCache ─────────────────────────────────────────────────────────────

/// High-concurrency in-memory cache using `DashMap` with TTL expiration.
///
/// Backed by [`dashmap::DashMap`] for lock-free concurrent access. A background
/// Tokio task sweeps expired entries at the configured interval.
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::cache::DashMapCache;
/// use stygian_graph::ports::CachePort;
/// use std::time::Duration;
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let cache = DashMapCache::new(Duration::from_secs(60));
/// cache.set("k", "v".to_string(), Some(Duration::from_secs(5))).await.unwrap();
/// assert_eq!(cache.get("k").await.unwrap(), Some("v".to_string()));
/// # });
/// ```
pub struct DashMapCache {
    store: Arc<DashMap<String, TtlEntry>>,
}

impl DashMapCache {
    /// Create a new `DashMapCache`.
    ///
    /// `cleanup_interval` controls how often a background task sweeps and
    /// removes expired entries. The task is spawned immediately.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::cache::DashMapCache;
    /// use std::time::Duration;
    ///
    /// let cache = DashMapCache::new(Duration::from_secs(30));
    /// ```
    pub fn new(cleanup_interval: Duration) -> Self {
        let store: Arc<DashMap<String, TtlEntry>> = Arc::new(DashMap::new());
        let weak = Arc::downgrade(&store);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(cleanup_interval);
            ticker.tick().await; // skip the first immediate tick
            loop {
                ticker.tick().await;
                let Some(map) = weak.upgrade() else { break };
                map.retain(|_, v| !v.is_expired());
            }
        });
        Self { store }
    }

    /// Return the number of live (non-expired) entries.
    pub fn len(&self) -> usize {
        self.store.iter().filter(|e| !e.is_expired()).count()
    }

    /// Returns `true` if the cache contains no live entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl CachePort for DashMapCache {
    async fn get(&self, key: &str) -> Result<Option<String>> {
        match self.store.get(key) {
            None => Ok(None),
            Some(entry) if entry.is_expired() => {
                drop(entry);
                self.store.remove(key);
                Ok(None)
            }
            Some(entry) => Ok(Some(entry.value.clone())),
        }
    }

    async fn set(&self, key: &str, value: String, ttl: Option<Duration>) -> Result<()> {
        self.store
            .insert(key.to_string(), TtlEntry::new(value, ttl));
        Ok(())
    }

    async fn invalidate(&self, key: &str) -> Result<()> {
        self.store.remove(key);
        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool> {
        match self.store.get(key) {
            None => Ok(false),
            Some(entry) if entry.is_expired() => {
                drop(entry);
                self.store.remove(key);
                Ok(false)
            }
            Some(_) => Ok(true),
        }
    }
}

// ─── BoundedLruCache ──────────────────────────────────────────────────────────

/// Capacity-bounded LRU cache with optional TTL per entry.
///
/// Wraps [`lru::LruCache`] behind a `Mutex` for thread safety. When the cache
/// reaches `capacity`, the least-recently-used entry is evicted automatically.
/// TTL is enforced on read: expired entries are treated as misses.
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::cache::BoundedLruCache;
/// use stygian_graph::ports::CachePort;
/// use std::num::NonZeroUsize;
/// use std::time::Duration;
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let cache = BoundedLruCache::new(NonZeroUsize::new(128).unwrap());
/// cache.set("k", "v".to_string(), Some(Duration::from_secs(60))).await.unwrap();
/// assert_eq!(cache.get("k").await.unwrap(), Some("v".to_string()));
/// # });
/// ```
pub struct BoundedLruCache {
    inner: tokio::sync::Mutex<lru::LruCache<String, TtlEntry>>,
}

impl BoundedLruCache {
    /// Create a new bounded LRU cache with the given `capacity`.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::adapters::cache::BoundedLruCache;
    /// use std::num::NonZeroUsize;
    ///
    /// let cache = BoundedLruCache::new(NonZeroUsize::new(256).unwrap());
    /// ```
    pub fn new(capacity: std::num::NonZeroUsize) -> Self {
        Self {
            inner: tokio::sync::Mutex::new(lru::LruCache::new(capacity)),
        }
    }
}

#[async_trait]
impl CachePort for BoundedLruCache {
    async fn get(&self, key: &str) -> Result<Option<String>> {
        let result = {
            let mut cache = self.inner.lock().await;
            match cache.get(key) {
                None => None,
                Some(entry) if entry.is_expired() => {
                    cache.pop(key);
                    None
                }
                Some(entry) => Some(entry.value.clone()),
            }
        };
        Ok(result)
    }

    async fn set(&self, key: &str, value: String, ttl: Option<Duration>) -> Result<()> {
        {
            let mut cache = self.inner.lock().await;
            cache.put(key.to_string(), TtlEntry::new(value, ttl));
        }
        Ok(())
    }

    async fn invalidate(&self, key: &str) -> Result<()> {
        {
            let mut cache = self.inner.lock().await;
            cache.pop(key);
        }
        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool> {
        let result = {
            let mut cache = self.inner.lock().await;
            match cache.get(key) {
                None => false,
                Some(entry) if entry.is_expired() => {
                    cache.pop(key);
                    false
                }
                Some(_) => true,
            }
        };
        Ok(result)
    }
}

// ─── Global singleton ─────────────────────────────────────────────────────────

/// Process-wide default cache singleton backed by `DashMapCache`.
///
/// Initialized once on first access via [`LazyLock`]. Suitable for lightweight
/// shared caching where a dedicated instance is unnecessary.
///
/// # Example
///
/// ```no_run
/// use stygian_graph::adapters::cache::global_cache;
/// # use stygian_graph::ports::CachePort;
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// global_cache().set("session", "abc".to_string(), None).await.unwrap();
/// let v = global_cache().get("session").await.unwrap();
/// assert_eq!(v, Some("abc".to_string()));
/// # });
/// ```
pub fn global_cache() -> &'static DashMapCache {
    static INSTANCE: LazyLock<DashMapCache> =
        LazyLock::new(|| DashMapCache::new(Duration::from_mins(5)));
    &INSTANCE
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // --- DashMapCache ---

    #[tokio::test]
    async fn dashmap_set_get() -> Result<()> {
        let c = DashMapCache::new(Duration::from_mins(1));
        c.set("a", "1".to_string(), None).await?;
        assert_eq!(c.get("a").await?, Some("1".to_string()));
        Ok(())
    }

    #[tokio::test]
    async fn dashmap_miss_returns_none() -> Result<()> {
        let c = DashMapCache::new(Duration::from_mins(1));
        assert_eq!(c.get("missing").await?, None);
        Ok(())
    }

    #[tokio::test]
    async fn dashmap_invalidate() -> Result<()> {
        let c = DashMapCache::new(Duration::from_mins(1));
        c.set("b", "2".to_string(), None).await?;
        c.invalidate("b").await?;
        assert_eq!(c.get("b").await?, None);
        Ok(())
    }

    #[tokio::test]
    async fn dashmap_ttl_expires() -> Result<()> {
        let c = DashMapCache::new(Duration::from_mins(1));
        // 1ns TTL — effectively already expired after one tokio yield
        c.set("x", "y".to_string(), Some(Duration::from_nanos(1)))
            .await?;
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(c.get("x").await?, None);
        Ok(())
    }

    #[tokio::test]
    async fn dashmap_exists() -> Result<()> {
        let c = DashMapCache::new(Duration::from_mins(1));
        c.set("e", "z".to_string(), None).await?;
        assert!(c.exists("e").await?);
        assert!(!c.exists("nope").await?);
        Ok(())
    }

    // --- BoundedLruCache ---

    #[tokio::test]
    async fn lru_set_get() -> Result<()> {
        let c = BoundedLruCache::new(std::num::NonZeroUsize::MIN.saturating_add(3));
        c.set("a", "1".to_string(), None).await?;
        assert_eq!(c.get("a").await?, Some("1".to_string()));
        Ok(())
    }

    #[tokio::test]
    async fn lru_evicts_on_capacity() -> Result<()> {
        let c = BoundedLruCache::new(std::num::NonZeroUsize::MIN.saturating_add(1));
        c.set("k1", "v1".to_string(), None).await?;
        c.set("k2", "v2".to_string(), None).await?;
        // Access k1 to make it recently used
        c.get("k1").await?;
        // Insert k3 — k2 is LRU and should be evicted
        c.set("k3", "v3".to_string(), None).await?;
        assert_eq!(c.get("k2").await?, None);
        assert_eq!(c.get("k1").await?, Some("v1".to_string()));
        assert_eq!(c.get("k3").await?, Some("v3".to_string()));
        Ok(())
    }

    #[tokio::test]
    async fn lru_ttl_expires() -> Result<()> {
        let c = BoundedLruCache::new(std::num::NonZeroUsize::MIN.saturating_add(7));
        c.set("t", "val".to_string(), Some(Duration::from_nanos(1)))
            .await?;
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(c.get("t").await?, None);
        Ok(())
    }

    #[tokio::test]
    async fn lru_invalidate() -> Result<()> {
        let c = BoundedLruCache::new(std::num::NonZeroUsize::MIN.saturating_add(3));
        c.set("x", "y".to_string(), None).await?;
        c.invalidate("x").await?;
        assert!(!c.exists("x").await?);
        Ok(())
    }

    // --- global_cache ---

    #[tokio::test]
    async fn global_cache_roundtrip() -> Result<()> {
        global_cache()
            .set("gc_test", "hello".to_string(), None)
            .await?;
        let v = global_cache().get("gc_test").await?;
        assert_eq!(v, Some("hello".to_string()));
        global_cache().invalidate("gc_test").await?;
        Ok(())
    }
}
