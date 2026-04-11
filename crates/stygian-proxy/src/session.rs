//! Domain-scoped proxy session stickiness.
//!
//! A *sticky session* binds a target domain to a specific proxy for a
//! configurable TTL. Requests to the same domain reuse the same proxy,
//! preserving IP consistency for anti-bot fingerprint checks while still
//! rotating across different domains.
//!
//! # Example
//!
//! ```
//! use stygian_proxy::session::{SessionMap, StickyPolicy};
//! use std::time::Duration;
//! use uuid::Uuid;
//!
//! let map = SessionMap::new();
//! let ttl = Duration::from_secs(300);
//! let proxy_id = Uuid::new_v4();
//!
//! map.bind("example.com", proxy_id, ttl);
//! assert_eq!(map.lookup("example.com"), Some(proxy_id));
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

/// Default session TTL: 5 minutes.
const DEFAULT_TTL_SECS: u64 = 300;

// ── StickyPolicy ─────────────────────────────────────────────────────────────

/// Policy controlling when and how proxy sessions are pinned to a key.
///
/// # Example
///
/// ```
/// use stygian_proxy::session::StickyPolicy;
/// use std::time::Duration;
///
/// let policy = StickyPolicy::domain(Duration::from_secs(600));
/// assert!(!policy.is_disabled());
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
#[non_exhaustive]
pub enum StickyPolicy {
    /// No session stickiness — every request may use a different proxy.
    #[default]
    Disabled,
    /// Pin by domain with a fixed TTL per binding.
    Domain {
        /// How long a domain→proxy binding remains valid.
        #[serde(with = "serde_duration_secs")]
        ttl: Duration,
    },
}

impl StickyPolicy {
    /// Create a domain-scoped policy with the given TTL.
    pub const fn domain(ttl: Duration) -> Self {
        Self::Domain { ttl }
    }

    /// Create a domain-scoped policy with the default TTL (5 minutes).
    pub const fn domain_default() -> Self {
        Self::Domain {
            ttl: Duration::from_secs(DEFAULT_TTL_SECS),
        }
    }

    /// Returns `true` when session stickiness is turned off.
    pub const fn is_disabled(&self) -> bool {
        matches!(self, Self::Disabled)
    }
}

// ── ProxySession ─────────────────────────────────────────────────────────────

/// A single domain→proxy binding with an expiration deadline.
#[derive(Debug, Clone)]
struct ProxySession {
    /// The proxy this session is bound to.
    proxy_id: Uuid,
    /// When this session was created.
    bound_at: Instant,
    /// How long the binding is valid.
    ttl: Duration,
}

impl ProxySession {
    /// Returns `true` when `bound_at + ttl` has elapsed.
    fn is_expired(&self) -> bool {
        self.bound_at.elapsed() >= self.ttl
    }
}

// ── SessionMap ───────────────────────────────────────────────────────────────

/// Thread-safe map of session keys (typically domains) to proxy bindings.
///
/// All operations acquire short-lived locks to minimise contention.
///
/// # Example
///
/// ```
/// use stygian_proxy::session::SessionMap;
/// use std::time::Duration;
/// use uuid::Uuid;
///
/// let map = SessionMap::new();
/// let id = Uuid::new_v4();
/// map.bind("example.com", id, Duration::from_secs(60));
/// assert_eq!(map.lookup("example.com"), Some(id));
/// ```
#[derive(Debug, Clone)]
pub struct SessionMap {
    inner: Arc<RwLock<HashMap<String, ProxySession>>>,
}

impl Default for SessionMap {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionMap {
    /// Create an empty session map.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Look up the proxy bound to `key`, returning `None` when no session
    /// exists or the existing session has expired.
    ///
    /// Expired entries are lazily removed on the next [`bind`](Self::bind)
    /// or [`purge_expired`](Self::purge_expired) call.
    pub fn lookup(&self, key: &str) -> Option<Uuid> {
        // try_read avoids blocking if a write is in progress.
        let guard = self.inner.try_read().ok()?;
        guard
            .get(key)
            .filter(|s| !s.is_expired())
            .map(|s| s.proxy_id)
    }

    /// Bind `key` to `proxy_id` with the given TTL. Overwrites any existing
    /// session for the same key.
    pub fn bind(&self, key: &str, proxy_id: Uuid, ttl: Duration) {
        let session = ProxySession {
            proxy_id,
            bound_at: Instant::now(),
            ttl,
        };
        if let Ok(mut guard) = self.inner.try_write() {
            guard.insert(key.to_string(), session);
        }
    }

    /// Remove all expired sessions, returning the number removed.
    pub fn purge_expired(&self) -> usize {
        let Ok(mut guard) = self.inner.try_write() else {
            return 0;
        };
        let before = guard.len();
        guard.retain(|_, s| !s.is_expired());
        before - guard.len()
    }

    /// Remove a specific session by key.
    pub fn unbind(&self, key: &str) {
        if let Ok(mut guard) = self.inner.try_write() {
            guard.remove(key);
        }
    }

    /// Returns the number of active (non-expired) sessions.
    pub fn active_count(&self) -> usize {
        let Ok(guard) = self.inner.try_read() else {
            return 0;
        };
        guard.values().filter(|s| !s.is_expired()).count()
    }
}

// ── serde helper ─────────────────────────────────────────────────────────────

mod serde_duration_secs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        d.as_secs().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        Ok(Duration::from_secs(u64::deserialize(d)?))
    }
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn same_domain_returns_same_proxy() {
        let map = SessionMap::new();
        let id = Uuid::new_v4();
        map.bind("example.com", id, Duration::from_secs(60));
        assert_eq!(map.lookup("example.com"), Some(id));
        assert_eq!(map.lookup("example.com"), Some(id));
    }

    #[test]
    fn different_domains_independent() {
        let map = SessionMap::new();
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        map.bind("a.com", id_a, Duration::from_secs(60));
        map.bind("b.com", id_b, Duration::from_secs(60));
        assert_eq!(map.lookup("a.com"), Some(id_a));
        assert_eq!(map.lookup("b.com"), Some(id_b));
    }

    #[test]
    fn expired_session_returns_none() {
        let map = SessionMap::new();
        let id = Uuid::new_v4();
        // TTL of 0 means it expires immediately.
        map.bind("example.com", id, Duration::ZERO);
        // Spin-wait a tiny bit to ensure the instant has elapsed.
        std::thread::sleep(Duration::from_millis(1));
        assert_eq!(map.lookup("example.com"), None);
    }

    #[test]
    fn purge_removes_expired() {
        let map = SessionMap::new();
        map.bind("expired.com", Uuid::new_v4(), Duration::ZERO);
        map.bind("active.com", Uuid::new_v4(), Duration::from_secs(300));
        std::thread::sleep(Duration::from_millis(1));

        let removed = map.purge_expired();
        assert_eq!(removed, 1);
        assert_eq!(map.active_count(), 1);
    }

    #[test]
    fn unbind_removes_session() {
        let map = SessionMap::new();
        map.bind("example.com", Uuid::new_v4(), Duration::from_secs(60));
        map.unbind("example.com");
        assert_eq!(map.lookup("example.com"), None);
    }

    #[test]
    fn rebind_overwrites_previous() {
        let map = SessionMap::new();
        let old_id = Uuid::new_v4();
        let new_id = Uuid::new_v4();
        map.bind("example.com", old_id, Duration::from_secs(60));
        map.bind("example.com", new_id, Duration::from_secs(60));
        assert_eq!(map.lookup("example.com"), Some(new_id));
    }

    #[test]
    fn policy_domain_default_ttl() {
        let policy = StickyPolicy::domain_default();
        assert!(matches!(policy, StickyPolicy::Domain { ttl } if ttl == Duration::from_secs(300)));
    }

    #[test]
    fn policy_disabled_by_default() {
        let policy = StickyPolicy::default();
        assert!(policy.is_disabled());
    }

    #[test]
    fn policy_serde_roundtrip() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let policy = StickyPolicy::domain(Duration::from_secs(120));
        let json = serde_json::to_string(&policy)?;
        let back: StickyPolicy = serde_json::from_str(&json)?;
        assert!(matches!(back, StickyPolicy::Domain { ttl } if ttl == Duration::from_secs(120)));
        Ok(())
    }
}
