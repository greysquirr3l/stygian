use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::types::{InvestigationReport, TargetClass};

/// One TTL-bounded entry stored in an [`LruTtlStore`].
#[derive(Debug, Clone)]
struct TtlEntry<V> {
    value: V,
    expires_at: Instant,
}

impl<V> TtlEntry<V> {
    fn new(value: V, ttl: Duration) -> Self {
        Self {
            value,
            expires_at: Instant::now() + ttl,
        }
    }

    fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

/// Generic capacity-bounded LRU store with per-entry TTL.
///
/// The store is the shared backing primitive used by every short-horizon
/// in-memory cache in this crate: the investigation report cache
/// ([`MemoryInvestigationCache`]) and the challenge feedback memory
/// ([`crate::challenge_feedback::ChallengeMemory`]). Centralising the
/// eviction + expiry logic keeps both consumers consistent (LRU at the
/// `max_entries` cap, TTL expiry on read) and avoids introducing a
/// parallel "second cache store" with its own semantics.
///
/// The store is `Send + Sync` so it can sit behind an `Arc` and be
/// shared across async tasks.
pub(crate) struct LruTtlStore<V> {
    ttl: Duration,
    inner: Mutex<lru::LruCache<String, TtlEntry<V>>>,
}

impl<V: Clone> LruTtlStore<V> {
    /// Create a new store with the given capacity (entries) and TTL.
    #[must_use]
    pub(crate) fn new(capacity: NonZeroUsize, ttl: Duration) -> Self {
        Self {
            ttl,
            inner: Mutex::new(lru::LruCache::new(capacity)),
        }
    }

    /// Configured per-entry TTL.
    #[must_use]
    pub(crate) const fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Look up a value by key. Returns `None` if the key is absent or
    /// has expired (in which case the entry is also evicted).
    pub(crate) fn get(&self, key: &str) -> Option<V> {
        let Ok(mut cache) = self.inner.lock() else {
            return None;
        };

        match cache.get(key) {
            Some(entry) if entry.is_expired() => {
                cache.pop(key);
                None
            }
            Some(entry) => Some(entry.value.clone()),
            None => None,
        }
    }

    /// Peek at a value without updating LRU recency. Useful for
    /// "read-modify-write" patterns (e.g. incrementing an observation
    /// counter without bumping the LRU order on the read).
    #[allow(dead_code)]
    pub(crate) fn peek(&self, key: &str) -> Option<V> {
        let Ok(cache) = self.inner.lock() else {
            return None;
        };

        match cache.peek(key) {
            Some(entry) if entry.is_expired() => None,
            Some(entry) => Some(entry.value.clone()),
            None => None,
        }
    }

    /// Insert or replace a value, applying the configured TTL.
    pub(crate) fn put(&self, key: String, value: V) {
        let Ok(mut cache) = self.inner.lock() else {
            return;
        };

        cache.put(key, TtlEntry::new(value, self.ttl));
    }

    /// Invalidate a single key. No-op if the key is absent.
    pub(crate) fn invalidate(&self, key: &str) {
        if let Ok(mut cache) = self.inner.lock() {
            cache.pop(key);
        }
    }

    /// Remove all entries.
    pub(crate) fn clear(&self) {
        if let Ok(mut cache) = self.inner.lock() {
            cache.clear();
        }
    }

    /// Number of entries currently retained (including expired-but-
    /// not-yet-evicted ones; expired entries are dropped on next read).
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.inner.lock().map_or(0, |cache| cache.len())
    }

    /// `true` if the store has zero entries.
    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Cache abstraction for normalized investigation reports.
///
/// Implementations are expected to store cloned [`InvestigationReport`] values keyed by the
/// hashed HAR payload and target class.
pub trait InvestigationReportCache: Send + Sync {
    /// Look up a cached investigation report by cache key.
    fn get(&self, key: &str) -> Option<InvestigationReport>;

    /// Insert or replace a cached investigation report.
    fn put(&self, key: String, report: InvestigationReport);

    /// Invalidate a single cache key.
    fn invalidate(&self, key: &str);

    /// Remove all cached entries.
    fn clear(&self);
}

/// Generate a stable cache key from HAR content and target class.
#[must_use]
pub fn investigation_cache_key(har_json: &str, target_class: TargetClass) -> String {
    let mut hasher = DefaultHasher::new();
    har_json.hash(&mut hasher);
    target_class.hash(&mut hasher);
    format!("charon:investigation:{:016x}", hasher.finish())
}

/// In-memory capacity-bounded LRU cache with TTL for investigation reports.
pub struct MemoryInvestigationCache {
    store: LruTtlStore<InvestigationReport>,
}

impl MemoryInvestigationCache {
    /// Create a new in-memory cache.
    #[must_use]
    pub fn new(capacity: NonZeroUsize, ttl: Duration) -> Self {
        Self {
            store: LruTtlStore::new(capacity, ttl),
        }
    }

    /// Number of entries currently retained.
    #[must_use]
    pub fn len(&self) -> usize {
        self.store.len()
    }

    /// `true` if the cache has zero entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }
}

impl InvestigationReportCache for MemoryInvestigationCache {
    fn get(&self, key: &str) -> Option<InvestigationReport> {
        self.store.get(key)
    }

    fn put(&self, key: String, report: InvestigationReport) {
        self.store.put(key, report);
    }

    fn invalidate(&self, key: &str) {
        self.store.invalidate(key);
    }

    fn clear(&self) {
        self.store.clear();
    }
}

/// Redis-backed investigation cache.
#[cfg(feature = "redis-cache")]
pub struct RedisInvestigationCache {
    client: redis::Client,
    ttl: Duration,
    key_prefix: String,
}

#[cfg(feature = "redis-cache")]
impl RedisInvestigationCache {
    /// Create a new Redis-backed cache using the provided URL.
    ///
    /// # Errors
    ///
    /// Returns a Redis error if the client cannot be created from `redis_url`.
    pub fn new(redis_url: &str, ttl: Duration) -> redis::RedisResult<Self> {
        let client = redis::Client::open(redis_url)?;
        Ok(Self {
            client,
            ttl,
            key_prefix: "charon:investigation".to_string(),
        })
    }

    fn prefixed_key(&self, key: &str) -> String {
        format!("{}:{}", self.key_prefix, key)
    }
}

#[cfg(feature = "redis-cache")]
impl InvestigationReportCache for RedisInvestigationCache {
    fn get(&self, key: &str) -> Option<InvestigationReport> {
        let mut connection = self.client.get_connection().ok()?;
        let payload: Option<String> = redis::cmd("GET")
            .arg(self.prefixed_key(key))
            .query(&mut connection)
            .ok()?;
        payload.and_then(|value| serde_json::from_str::<InvestigationReport>(&value).ok())
    }

    fn put(&self, key: String, report: InvestigationReport) {
        let Ok(payload) = serde_json::to_string(&report) else {
            return;
        };
        let ttl_seconds = self.ttl.as_secs();
        let Ok(mut connection) = self.client.get_connection() else {
            return;
        };
        let _: redis::RedisResult<()> = redis::cmd("SETEX")
            .arg(self.prefixed_key(&key))
            .arg(ttl_seconds)
            .arg(payload)
            .query(&mut connection);
    }

    fn invalidate(&self, key: &str) {
        let Ok(mut connection) = self.client.get_connection() else {
            return;
        };
        let _: redis::RedisResult<()> = redis::cmd("DEL")
            .arg(self.prefixed_key(key))
            .query(&mut connection);
    }

    fn clear(&self) {
        let pattern = format!("{}:*", self.key_prefix);
        let Ok(mut connection) = self.client.get_connection() else {
            return;
        };
        let keys: redis::RedisResult<Vec<String>> =
            redis::cmd("KEYS").arg(pattern).query(&mut connection);
        if let Ok(keys) = keys
            && !keys.is_empty()
        {
            let _: redis::RedisResult<()> = redis::cmd("DEL").arg(keys).query(&mut connection);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AntiBotProvider, Detection, InvestigationReport};
    use std::collections::BTreeMap;

    fn sample_report() -> InvestigationReport {
        InvestigationReport {
            page_title: Some("https://example.com".to_string()),
            total_requests: 10,
            blocked_requests: 2,
            status_histogram: BTreeMap::from([(200, 8), (403, 2)]),
            resource_type_histogram: BTreeMap::new(),
            provider_histogram: BTreeMap::new(),
            marker_histogram: BTreeMap::new(),
            top_markers: Vec::new(),
            hosts: Vec::new(),
            suspicious_requests: Vec::new(),
            aggregate: Detection {
                provider: AntiBotProvider::Unknown,
                confidence: 0.1,
                markers: Vec::new(),
            },
            target_class: Some(TargetClass::Api),
        }
    }

    #[test]
    fn memory_cache_round_trips_report() {
        let capacity = NonZeroUsize::new(2).unwrap_or(NonZeroUsize::MIN);
        let cache = MemoryInvestigationCache::new(capacity, Duration::from_mins(1));
        let key = investigation_cache_key("{\"log\":{}}", TargetClass::Api);
        let report = sample_report();

        cache.put(key.clone(), report.clone());

        assert_eq!(cache.get(&key), Some(report));
    }

    #[test]
    fn memory_cache_expires_entries() {
        let capacity = NonZeroUsize::new(2).unwrap_or(NonZeroUsize::MIN);
        let cache = MemoryInvestigationCache::new(capacity, Duration::from_millis(1));
        let key = investigation_cache_key("{\"log\":{}}", TargetClass::Api);
        cache.put(key.clone(), sample_report());
        std::thread::sleep(Duration::from_millis(5));
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn cache_key_changes_by_target_class() {
        let har = "{\"log\":{\"entries\":[]}}";
        let api = investigation_cache_key(har, TargetClass::Api);
        let high = investigation_cache_key(har, TargetClass::HighSecurity);
        assert_ne!(api, high);
    }
}
