use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::types::{InvestigationReport, TargetClass};

#[derive(Debug, Clone)]
struct CacheEntry {
    report: InvestigationReport,
    expires_at: Instant,
}

impl CacheEntry {
    fn new(report: InvestigationReport, ttl: Duration) -> Self {
        Self {
            report,
            expires_at: Instant::now() + ttl,
        }
    }

    fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
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
    ttl: Duration,
    inner: Mutex<lru::LruCache<String, CacheEntry>>,
}

impl MemoryInvestigationCache {
    /// Create a new in-memory cache.
    #[must_use]
    pub fn new(capacity: NonZeroUsize, ttl: Duration) -> Self {
        Self {
            ttl,
            inner: Mutex::new(lru::LruCache::new(capacity)),
        }
    }
}

impl InvestigationReportCache for MemoryInvestigationCache {
    fn get(&self, key: &str) -> Option<InvestigationReport> {
        let Ok(mut cache) = self.inner.lock() else {
            return None;
        };

        match cache.get(key) {
            Some(entry) if entry.is_expired() => {
                cache.pop(key);
                None
            }
            Some(entry) => Some(entry.report.clone()),
            None => None,
        }
    }

    fn put(&self, key: String, report: InvestigationReport) {
        let Ok(mut cache) = self.inner.lock() else {
            return;
        };

        cache.put(key, CacheEntry::new(report, self.ttl));
    }

    fn invalidate(&self, key: &str) {
        if let Ok(mut cache) = self.inner.lock() {
            cache.pop(key);
        }
    }

    fn clear(&self) {
        if let Ok(mut cache) = self.inner.lock() {
            cache.clear();
        }
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
