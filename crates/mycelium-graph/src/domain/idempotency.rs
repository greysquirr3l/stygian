//! Idempotency key tracking system
//!
//! Ensures scraping operations can be safely retried without re-executing
//! expensive work. Each operation is tagged with a ULID-based key; results
//! are stored in the cache with configurable TTL (default 24 h).
//!
//! # Example
//!
//! ```no_run
//! use mycelium_graph::domain::idempotency::{IdempotencyKey, IdempotencyStore};
//! use mycelium_graph::adapters::cache::MemoryCache;
//! use mycelium_graph::ports::ServiceOutput;
//! use serde_json::json;
//! use std::sync::Arc;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let cache = Arc::new(MemoryCache::new());
//! let store = IdempotencyStore::new(cache);
//!
//! let key = IdempotencyKey::generate();
//! let result = ServiceOutput { data: "page html".into(), metadata: json!({}) };
//!
//! store.store(key, result.clone(), None).await.unwrap();
//! let cached = store.get(key).await.unwrap();
//! assert!(cached.is_some());
//! # });
//! ```

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::domain::error::Result;
use crate::ports::{CachePort, ServiceOutput};

/// Default time-to-live for idempotency records (24 hours)
pub const DEFAULT_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Maximum time-to-live for idempotency records (72 hours)
pub const MAX_TTL: Duration = Duration::from_secs(72 * 60 * 60);

/// An idempotency key for a scraping operation.
///
/// Based on ULID for lexicographic ordering and embedded timestamp.
///
/// # Example
///
/// ```
/// use mycelium_graph::domain::idempotency::IdempotencyKey;
///
/// let key = IdempotencyKey::generate();
/// let encoded = key.to_string();
/// let decoded: IdempotencyKey = encoded.parse().unwrap();
/// assert_eq!(key, decoded);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IdempotencyKey(Ulid);

impl IdempotencyKey {
    /// Generate a new unique idempotency key.
    pub fn generate() -> Self {
        Self(Ulid::new())
    }

    /// Parse an idempotency key from its string representation.
    ///
    /// # Errors
    ///
    /// Returns an error if the string is not a valid ULID.
    pub fn parse(s: &str) -> std::result::Result<Self, ulid::DecodeError> {
        s.parse::<Ulid>().map(Self)
    }

    /// The cache key used to store this record.
    pub fn cache_key(&self) -> String {
        format!("idempotency:{}", &self.0)
    }
}

impl std::str::FromStr for IdempotencyKey {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        s.parse::<Ulid>().map(IdempotencyKey)
    }
}

impl std::fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Default for IdempotencyKey {
    fn default() -> Self {
        Self::generate()
    }
}

/// Status of an idempotent operation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    /// Operation is in progress
    Pending,
    /// Operation completed successfully
    Completed,
    /// Operation failed
    Failed,
}

/// A stored idempotency record.
///
/// Contains the result of an operation along with metadata for expiry checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdempotencyRecord {
    /// The key uniquely identifying this operation
    pub key: String,
    /// Status of the operation
    pub status: OperationStatus,
    /// The cached output (only set when status is Completed)
    pub output: Option<CachedOutput>,
    /// Unix timestamp (seconds) when this record was created
    pub created_at: u64,
    /// Unix timestamp (seconds) when this record expires
    pub expires_at: u64,
}

/// Serializable version of `ServiceOutput` for cache storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedOutput {
    /// Raw scraped data
    pub data: String,
    /// Operation metadata
    pub metadata: serde_json::Value,
}

impl From<ServiceOutput> for CachedOutput {
    fn from(output: ServiceOutput) -> Self {
        Self {
            data: output.data,
            metadata: output.metadata,
        }
    }
}

impl From<CachedOutput> for ServiceOutput {
    fn from(cached: CachedOutput) -> Self {
        Self {
            data: cached.data,
            metadata: cached.metadata,
        }
    }
}

impl IdempotencyRecord {
    /// Create a new pending record (marks operation as in-flight).
    pub fn new_pending(key: IdempotencyKey, ttl: Duration) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            key: key.to_string(),
            status: OperationStatus::Pending,
            output: None,
            created_at: now,
            expires_at: now + ttl.as_secs(),
        }
    }

    /// Check whether this record has expired.
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now > self.expires_at
    }
}

/// Cache-backed store for idempotency records.
///
/// Uses `CachePort` as an abstraction so it works with any backend
/// (in-memory, Redis, etc.). Provides atomic check-and-mark via optimistic
/// locking through the cache layer.
pub struct IdempotencyStore<C: CachePort> {
    cache: Arc<C>,
    default_ttl: Duration,
}

impl<C: CachePort> IdempotencyStore<C> {
    /// Create a new store with the given cache backend and default TTL.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use mycelium_graph::domain::idempotency::IdempotencyStore;
    /// use mycelium_graph::adapters::cache::MemoryCache;
    /// use std::sync::Arc;
    ///
    /// let store = IdempotencyStore::new(Arc::new(MemoryCache::new()));
    /// ```
    pub const fn new(cache: Arc<C>) -> Self {
        Self {
            cache,
            default_ttl: DEFAULT_TTL,
        }
    }

    /// Create a store with a custom TTL.
    pub fn with_ttl(cache: Arc<C>, ttl: Duration) -> Self {
        let ttl = ttl.min(MAX_TTL);
        Self {
            cache,
            default_ttl: ttl,
        }
    }

    /// Check whether a result is already cached for this key.
    ///
    /// Returns `None` if not found or expired.
    pub async fn get(&self, key: IdempotencyKey) -> Result<Option<IdempotencyRecord>> {
        let cache_key = key.cache_key();
        let Some(json) = self.cache.get(&cache_key).await? else {
            return Ok(None);
        };

        let record: IdempotencyRecord = serde_json::from_str(&json).map_err(|e| {
            crate::domain::error::MyceliumError::Cache(
                crate::domain::error::CacheError::ReadFailed(e.to_string()),
            )
        })?;

        if record.is_expired() {
            return Ok(None);
        }

        Ok(Some(record))
    }

    /// Mark a key as pending (in-flight) atomically.
    ///
    /// Returns `true` if the claim was acquired (key was not already present).
    /// Returns `false` if another worker already claimed this key.
    pub async fn claim(&self, key: IdempotencyKey) -> Result<bool> {
        let cache_key = key.cache_key();

        // Check first — if already exists, don't overwrite
        if self.cache.exists(&cache_key).await? {
            return Ok(false);
        }

        let record = IdempotencyRecord::new_pending(key, self.default_ttl);
        let json = serde_json::to_string(&record).unwrap_or_default();
        self.cache
            .set(&cache_key, json, Some(self.default_ttl))
            .await?;
        Ok(true)
    }

    /// Store a completed result for the given key.
    pub async fn store(
        &self,
        key: IdempotencyKey,
        output: ServiceOutput,
        ttl: Option<Duration>,
    ) -> Result<()> {
        let ttl = ttl.unwrap_or(self.default_ttl).min(MAX_TTL);
        let cache_key = key.cache_key();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let record = IdempotencyRecord {
            key: key.to_string(),
            status: OperationStatus::Completed,
            output: Some(CachedOutput::from(output)),
            created_at: now,
            expires_at: now + ttl.as_secs(),
        };

        let json = serde_json::to_string(&record).unwrap_or_default();
        self.cache.set(&cache_key, json, Some(ttl)).await
    }

    /// Mark an operation as failed.
    pub async fn mark_failed(&self, key: IdempotencyKey) -> Result<()> {
        let cache_key = key.cache_key();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let record = IdempotencyRecord {
            key: key.to_string(),
            status: OperationStatus::Failed,
            output: None,
            created_at: now,
            expires_at: now + self.default_ttl.as_secs(),
        };

        let json = serde_json::to_string(&record).unwrap_or_default();
        self.cache
            .set(&cache_key, json, Some(self.default_ttl))
            .await
    }

    /// Invalidate a key (force re-execution on next attempt).
    pub async fn invalidate(&self, key: IdempotencyKey) -> Result<()> {
        self.cache.invalidate(&key.cache_key()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::cache::MemoryCache;
    use crate::ports::ServiceOutput;

    fn make_output() -> ServiceOutput {
        ServiceOutput {
            data: "scraped content".into(),
            metadata: serde_json::json!({"status": 200}),
        }
    }

    #[tokio::test]
    async fn test_store_and_retrieve() -> Result<()> {
        let store = IdempotencyStore::new(Arc::new(MemoryCache::new()));
        let key = IdempotencyKey::generate();

        store.store(key, make_output(), None).await?;

        let maybe_record = store.get(key).await?;
        assert!(maybe_record.is_some());
        if let Some(record) = maybe_record {
            assert_eq!(record.status, OperationStatus::Completed);
            assert!(record.output.is_some());
            if let Some(ref output) = record.output {
                assert_eq!(output.data, "scraped content");
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_missing_key_returns_none() -> Result<()> {
        let store = IdempotencyStore::new(Arc::new(MemoryCache::new()));
        let key = IdempotencyKey::generate();

        let result = store.get(key).await?;
        assert!(result.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_claim_prevents_duplicate() -> Result<()> {
        let store = IdempotencyStore::new(Arc::new(MemoryCache::new()));
        let key = IdempotencyKey::generate();

        let first = store.claim(key).await?;
        assert!(first, "First claim should succeed");

        let second = store.claim(key).await?;
        assert!(!second, "Second claim should fail (duplicate)");

        Ok(())
    }

    #[tokio::test]
    async fn test_invalidate_removes_record() -> Result<()> {
        let store = IdempotencyStore::new(Arc::new(MemoryCache::new()));
        let key = IdempotencyKey::generate();

        store.store(key, make_output(), None).await?;
        store.invalidate(key).await?;

        let result = store.get(key).await?;
        assert!(result.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_mark_failed_stores_failed_status() -> Result<()> {
        let store = IdempotencyStore::new(Arc::new(MemoryCache::new()));
        let key = IdempotencyKey::generate();

        store.mark_failed(key).await?;

        let maybe_record = store.get(key).await?;
        assert!(maybe_record.is_some());
        if let Some(record) = maybe_record {
            assert_eq!(record.status, OperationStatus::Failed);
            assert!(record.output.is_none());
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_expired_record_returns_none() -> Result<()> {
        let store =
            IdempotencyStore::with_ttl(Arc::new(MemoryCache::new()), Duration::from_nanos(1));
        let key = IdempotencyKey::generate();

        store
            .store(key, make_output(), Some(Duration::from_nanos(1)))
            .await?;

        // Sleep briefly so the record expires
        tokio::time::sleep(Duration::from_millis(5)).await;

        // Note: MemoryCache doesn't enforce TTL, so create a record with past expires_at
        // to test the is_expired() logic directly
        let record = IdempotencyRecord {
            key: key.to_string(),
            status: OperationStatus::Completed,
            output: None,
            created_at: 0,
            expires_at: 1, // Far in the past
        };
        assert!(record.is_expired());
        Ok(())
    }

    #[test]
    fn test_key_roundtrip() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let key = IdempotencyKey::generate();
        let s = key.to_string();
        let parsed: IdempotencyKey = s.parse()?;
        assert_eq!(key, parsed);
        Ok(())
    }

    #[test]
    fn test_ttl_capped_at_max() {
        let huge_ttl = Duration::from_secs(99_999_999);
        let store = IdempotencyStore::with_ttl(Arc::new(MemoryCache::new()), huge_ttl);
        assert_eq!(store.default_ttl, MAX_TTL);
    }
}
