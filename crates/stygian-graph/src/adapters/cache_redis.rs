//! Redis / Valkey [`CachePort`] adapter
//!
//! Feature-gated behind `redis`. Uses [`deadpool_redis`] for async connection
//! pooling and supports optional key-prefix namespacing so multiple tenants can
//! share a single Redis instance without key collisions.
//!
//! # Quick start
//!
//! ```no_run
//! use stygian_graph::adapters::cache_redis::{RedisCache, RedisCacheConfig};
//! use stygian_graph::ports::CachePort;
//! use std::time::Duration;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let config = RedisCacheConfig {
//!     url: "redis://127.0.0.1:6379".into(),
//!     key_prefix: Some("myapp:".into()),
//!     default_ttl: Some(Duration::from_secs(3600)),
//!     pool_size: 8,
//! };
//! let cache = RedisCache::new(config).expect("redis connection pool");
//! cache.set("page:1", "html".into(), None).await.unwrap();
//! # });
//! ```

use crate::domain::error::{CacheError, Result, StygianError};
use crate::ports::CachePort;
use async_trait::async_trait;
use deadpool_redis::{Config as PoolConfig, Pool, Runtime};
use redis::AsyncCommands;
use std::time::Duration;

// ─── Configuration ────────────────────────────────────────────────────────────

/// Configuration for [`RedisCache`].
///
/// # Fields
///
/// * `url` — Redis connection string (`redis://host:port[/db]`).
/// * `key_prefix` — Optional prefix prepended to every key for namespace isolation.
/// * `default_ttl` — Fallback TTL applied when `set()` is called with `ttl = None`.
///   `None` means keys without an explicit TTL never expire.
/// * `pool_size` — Maximum number of connections in the `deadpool` pool (default `8`).
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::cache_redis::RedisCacheConfig;
/// use std::time::Duration;
///
/// let config = RedisCacheConfig {
///     url: "redis://127.0.0.1:6379".into(),
///     key_prefix: Some("stygian:".into()),
///     default_ttl: Some(Duration::from_secs(300)),
///     pool_size: 16,
/// };
/// ```
#[derive(Debug, Clone)]
pub struct RedisCacheConfig {
    /// Redis connection URL.
    pub url: String,
    /// Optional key prefix for namespace isolation.
    pub key_prefix: Option<String>,
    /// Default TTL applied when `set()` receives `ttl = None`.
    pub default_ttl: Option<Duration>,
    /// Max pool connections (default 8).
    pub pool_size: usize,
}

impl Default for RedisCacheConfig {
    fn default() -> Self {
        Self {
            url: "redis://127.0.0.1:6379".into(),
            key_prefix: None,
            default_ttl: None,
            pool_size: 8,
        }
    }
}

// ─── Adapter ──────────────────────────────────────────────────────────────────

/// Redis / Valkey backed [`CachePort`] adapter.
///
/// Internally uses a [`deadpool_redis::Pool`] for connection management.
/// Keys are stored as Redis strings; TTL is enforced via Redis `PSETEX`.
///
/// # Example
///
/// ```no_run
/// use stygian_graph::adapters::cache_redis::{RedisCache, RedisCacheConfig};
/// use stygian_graph::ports::CachePort;
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let cache = RedisCache::new(RedisCacheConfig::default())
///     .expect("pool creation");
/// cache.set("k", "v".into(), None).await.unwrap();
/// assert_eq!(cache.get("k").await.unwrap(), Some("v".into()));
/// # });
/// ```
pub struct RedisCache {
    pool: Pool,
    key_prefix: Option<String>,
    default_ttl: Option<Duration>,
}

impl RedisCache {
    /// Create a new [`RedisCache`] from the given config.
    ///
    /// Returns a [`CacheError::WriteFailed`] if the connection pool cannot
    /// be created (e.g. the URL is malformed).
    ///
    /// # Errors
    ///
    /// Returns [`StygianError::Cache`] if pool creation fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::cache_redis::{RedisCache, RedisCacheConfig};
    ///
    /// let cache = RedisCache::new(RedisCacheConfig::default())
    ///     .expect("pool creation");
    /// ```
    pub fn new(config: RedisCacheConfig) -> Result<Self> {
        let pool_cfg = PoolConfig::from_url(&config.url);
        let pool = pool_cfg
            .builder()
            .map(|b| b.max_size(config.pool_size))
            .map_err(|e| {
                StygianError::Cache(CacheError::WriteFailed(format!(
                    "failed to build Redis pool: {e}"
                )))
            })?
            .runtime(Runtime::Tokio1)
            .build()
            .map_err(|e| {
                StygianError::Cache(CacheError::WriteFailed(format!(
                    "failed to build Redis pool: {e}"
                )))
            })?;

        Ok(Self {
            pool,
            key_prefix: config.key_prefix,
            default_ttl: config.default_ttl,
        })
    }

    /// Build the full key by prepending the optional prefix.
    fn full_key(&self, key: &str) -> String {
        match &self.key_prefix {
            Some(prefix) => format!("{prefix}{key}"),
            None => key.to_string(),
        }
    }

    /// Check connectivity to the Redis backend.
    ///
    /// Sends a `PING` command and expects `PONG`.
    ///
    /// # Errors
    ///
    /// Returns [`StygianError::Cache`] on connection or protocol failure.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::adapters::cache_redis::{RedisCache, RedisCacheConfig};
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// let cache = RedisCache::new(RedisCacheConfig::default()).unwrap();
    /// cache.healthcheck().await.unwrap();
    /// # });
    /// ```
    pub async fn healthcheck(&self) -> Result<()> {
        let mut conn = self.pool.get().await.map_err(|e| {
            StygianError::Cache(CacheError::ReadFailed(format!("Redis pool error: {e}")))
        })?;
        redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .map_err(|e| {
                StygianError::Cache(CacheError::ReadFailed(format!("Redis PING failed: {e}")))
            })?;
        Ok(())
    }
}

#[async_trait]
impl CachePort for RedisCache {
    /// Retrieve a value from Redis by key.
    ///
    /// Returns `Ok(None)` on a cache miss and `Err` on backend failure.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::adapters::cache_redis::{RedisCache, RedisCacheConfig};
    /// # use stygian_graph::ports::CachePort;
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # let cache = RedisCache::new(RedisCacheConfig::default()).unwrap();
    /// let val = cache.get("mykey").await.unwrap();
    /// # });
    /// ```
    async fn get(&self, key: &str) -> Result<Option<String>> {
        let full_key = self.full_key(key);
        let mut conn = self.pool.get().await.map_err(|e| {
            StygianError::Cache(CacheError::ReadFailed(format!("Redis pool error: {e}")))
        })?;
        let value: Option<String> = conn.get(&full_key).await.map_err(|e| {
            StygianError::Cache(CacheError::ReadFailed(format!("Redis GET failed: {e}")))
        })?;
        Ok(value)
    }

    /// Store a value in Redis with optional TTL.
    ///
    /// If `ttl` is `None`, the adapter's `default_ttl` is used. If both are
    /// `None`, the key is stored without expiration.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::adapters::cache_redis::{RedisCache, RedisCacheConfig};
    /// # use stygian_graph::ports::CachePort;
    /// # use std::time::Duration;
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # let cache = RedisCache::new(RedisCacheConfig::default()).unwrap();
    /// cache.set("k", "v".into(), Some(Duration::from_secs(60))).await.unwrap();
    /// # });
    /// ```
    async fn set(&self, key: &str, value: String, ttl: Option<Duration>) -> Result<()> {
        let full_key = self.full_key(key);
        let effective_ttl = ttl.or(self.default_ttl);
        let mut conn = self.pool.get().await.map_err(|e| {
            StygianError::Cache(CacheError::WriteFailed(format!("Redis pool error: {e}")))
        })?;

        match effective_ttl {
            Some(duration) => {
                let millis = duration.as_millis();
                // PSETEX key milliseconds value
                redis::cmd("PSETEX")
                    .arg(&full_key)
                    .arg(millis as u64)
                    .arg(&value)
                    .query_async::<()>(&mut conn)
                    .await
                    .map_err(|e| {
                        StygianError::Cache(CacheError::WriteFailed(format!(
                            "Redis PSETEX failed: {e}"
                        )))
                    })?;
            }
            None => {
                conn.set::<_, _, ()>(&full_key, &value).await.map_err(|e| {
                    StygianError::Cache(CacheError::WriteFailed(format!("Redis SET failed: {e}")))
                })?;
            }
        }

        Ok(())
    }

    /// Remove a key from Redis.
    ///
    /// Returns `Ok(())` whether the key existed or not.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::adapters::cache_redis::{RedisCache, RedisCacheConfig};
    /// # use stygian_graph::ports::CachePort;
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # let cache = RedisCache::new(RedisCacheConfig::default()).unwrap();
    /// cache.invalidate("stale-key").await.unwrap();
    /// # });
    /// ```
    async fn invalidate(&self, key: &str) -> Result<()> {
        let full_key = self.full_key(key);
        let mut conn = self.pool.get().await.map_err(|e| {
            StygianError::Cache(CacheError::WriteFailed(format!("Redis pool error: {e}")))
        })?;
        conn.del::<_, ()>(&full_key).await.map_err(|e| {
            StygianError::Cache(CacheError::WriteFailed(format!("Redis DEL failed: {e}")))
        })?;
        Ok(())
    }

    /// Check whether a key exists in Redis.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::adapters::cache_redis::{RedisCache, RedisCacheConfig};
    /// # use stygian_graph::ports::CachePort;
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # let cache = RedisCache::new(RedisCacheConfig::default()).unwrap();
    /// if cache.exists("k").await.unwrap() {
    ///     println!("hit");
    /// }
    /// # });
    /// ```
    async fn exists(&self, key: &str) -> Result<bool> {
        let full_key = self.full_key(key);
        let mut conn = self.pool.get().await.map_err(|e| {
            StygianError::Cache(CacheError::ReadFailed(format!("Redis pool error: {e}")))
        })?;
        let count: u32 = conn.exists(&full_key).await.map_err(|e| {
            StygianError::Cache(CacheError::ReadFailed(format!("Redis EXISTS failed: {e}")))
        })?;
        Ok(count > 0)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_key_without_prefix() {
        let cache = RedisCache::new(RedisCacheConfig {
            url: "redis://127.0.0.1:6379".into(),
            key_prefix: None,
            default_ttl: None,
            pool_size: 1,
        })
        .expect("pool creation");
        assert_eq!(cache.full_key("abc"), "abc");
    }

    #[test]
    fn full_key_with_prefix() {
        let cache = RedisCache::new(RedisCacheConfig {
            url: "redis://127.0.0.1:6379".into(),
            key_prefix: Some("ns:".into()),
            default_ttl: None,
            pool_size: 1,
        })
        .expect("pool creation");
        assert_eq!(cache.full_key("abc"), "ns:abc");
    }

    #[test]
    fn default_config_values() {
        let cfg = RedisCacheConfig::default();
        assert_eq!(cfg.url, "redis://127.0.0.1:6379");
        assert!(cfg.key_prefix.is_none());
        assert!(cfg.default_ttl.is_none());
        assert_eq!(cfg.pool_size, 8);
    }

    #[test]
    fn pool_creation_with_bad_url_fails() {
        let result = RedisCache::new(RedisCacheConfig {
            url: "not-a-url".into(),
            ..Default::default()
        });
        assert!(result.is_err());
    }

    // ── Integration tests against live Valkey ────────────────────────────

    #[tokio::test]
    #[ignore = "requires running Redis/Valkey (docker-compose up -d valkey)"]
    async fn integration_set_get_invalidate_cycle() {
        let cache = RedisCache::new(RedisCacheConfig {
            url: "redis://127.0.0.1:6379".into(),
            key_prefix: Some("test:integ:".into()),
            ..Default::default()
        })
        .expect("pool");

        // healthcheck
        cache.healthcheck().await.expect("ping");

        let key = "integration_cycle";

        // set
        cache
            .set(key, "hello".into(), Some(Duration::from_secs(30)))
            .await
            .expect("set");

        // get
        let val = cache.get(key).await.expect("get");
        assert_eq!(val, Some("hello".into()));

        // exists
        assert!(cache.exists(key).await.expect("exists"));

        // invalidate
        cache.invalidate(key).await.expect("invalidate");
        let val = cache.get(key).await.expect("get after invalidate");
        assert_eq!(val, None);
        assert!(!cache.exists(key).await.expect("exists after invalidate"));
    }

    #[tokio::test]
    #[ignore = "requires running Redis/Valkey (docker-compose up -d valkey)"]
    async fn integration_ttl_expiration() {
        let cache = RedisCache::new(RedisCacheConfig {
            url: "redis://127.0.0.1:6379".into(),
            key_prefix: Some("test:ttl:".into()),
            ..Default::default()
        })
        .expect("pool");

        let key = "short_lived";
        cache
            .set(key, "expires".into(), Some(Duration::from_millis(200)))
            .await
            .expect("set");

        // immediately present
        assert_eq!(cache.get(key).await.expect("get"), Some("expires".into()));

        // wait for expiration
        tokio::time::sleep(Duration::from_millis(350)).await;

        // gone
        assert_eq!(cache.get(key).await.expect("get after ttl"), None);
    }

    #[tokio::test]
    #[ignore = "requires running Redis/Valkey (docker-compose up -d valkey)"]
    async fn integration_key_namespacing_isolation() {
        let cache_a = RedisCache::new(RedisCacheConfig {
            url: "redis://127.0.0.1:6379".into(),
            key_prefix: Some("ns_a:".into()),
            ..Default::default()
        })
        .expect("pool a");

        let cache_b = RedisCache::new(RedisCacheConfig {
            url: "redis://127.0.0.1:6379".into(),
            key_prefix: Some("ns_b:".into()),
            ..Default::default()
        })
        .expect("pool b");

        let key = "shared_name";

        cache_a
            .set(key, "alpha".into(), Some(Duration::from_secs(30)))
            .await
            .expect("set a");
        cache_b
            .set(key, "beta".into(), Some(Duration::from_secs(30)))
            .await
            .expect("set b");

        assert_eq!(cache_a.get(key).await.expect("get a"), Some("alpha".into()));
        assert_eq!(cache_b.get(key).await.expect("get b"), Some("beta".into()));

        // cleanup
        cache_a.invalidate(key).await.expect("del a");
        cache_b.invalidate(key).await.expect("del b");
    }

    #[tokio::test]
    #[ignore = "requires running Redis/Valkey (docker-compose up -d valkey)"]
    async fn integration_default_ttl_applied() {
        let cache = RedisCache::new(RedisCacheConfig {
            url: "redis://127.0.0.1:6379".into(),
            key_prefix: Some("test:dttl:".into()),
            default_ttl: Some(Duration::from_millis(200)),
            pool_size: 2,
        })
        .expect("pool");

        let key = "default_ttl_key";
        cache
            .set(key, "has_default".into(), None)
            .await
            .expect("set");

        assert!(cache.exists(key).await.expect("exists"));
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert!(!cache.exists(key).await.expect("expired"));
    }
}
