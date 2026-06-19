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

use crate::stickiness::{StickinessPolicy, VendorStickinessMap};
use crate::types::VendorId;

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
    #[must_use]
    pub const fn domain(ttl: Duration) -> Self {
        Self::Domain { ttl }
    }

    /// Create a domain-scoped policy with the default TTL (5 minutes).
    #[must_use]
    pub const fn domain_default() -> Self {
        Self::Domain {
            ttl: Duration::from_secs(DEFAULT_TTL_SECS),
        }
    }

    /// Returns `true` when session stickiness is turned off.
    #[must_use]
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
    #[must_use]
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
    #[must_use]
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
    #[must_use]
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
    #[must_use]
    pub fn active_count(&self) -> usize {
        let Ok(guard) = self.inner.try_read() else {
            return 0;
        };
        guard.values().filter(|s| !s.is_expired()).count()
    }

    /// Acquire (or refresh) a sticky session for `(domain, vendor)`
    /// according to `policy_map`.
    ///
    /// Translates the 2026 guide's per-vendor stickiness matrix
    /// ([`VendorStickinessMap::with_builtin_defaults`]) into a typed
    /// [`SessionDecision`]:
    ///
    /// - [`StickinessPolicy::StickyForever`] and
    ///   [`StickinessPolicy::StickyForTtl`] → reuse an existing binding
    ///   when present, otherwise emit
    ///   [`SessionDecision::AcquireAndBind`] with the policy TTL (or
    ///   [`Duration::MAX`](Duration) for `StickyForever`).
    /// - [`StickinessPolicy::FreshPerDomain`] → evict any existing
    ///   binding for `domain` and emit
    ///   [`SessionDecision::AcquireFresh`].
    /// - [`StickinessPolicy::FreshPerRequest`] → emit
    ///   [`SessionDecision::AcquireFresh`] (no binding to evict).
    /// - [`StickinessPolicy::StickyForRequestCount`] → emitted as
    ///   [`SessionDecision::AcquireFresh`]. The request-count stickiness
    ///   is documented as a no-op at this layer because the [`SessionMap`]
    ///   does not count requests per session; operators that need
    ///   per-request counting should switch to `StickyForTtl` instead.
    ///
    /// Unknown vendors (those without an explicit entry in `policy_map`)
    /// fall back to [`StickinessPolicy::FreshPerRequest`] (the safest
    /// default) and so always emit [`SessionDecision::AcquireFresh`].
    ///
    /// This method is pure — it never blocks on async I/O and never
    /// touches the rotation strategy — so it is safe to call from the
    /// hot acquisition path.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_proxy::session::{SessionDecision, SessionMap};
    /// use stygian_proxy::stickiness::VendorStickinessMap;
    /// use stygian_proxy::types::VendorId;
    /// use std::time::Duration;
    /// use uuid::Uuid;
    ///
    /// let map = SessionMap::new();
    /// let policy = VendorStickinessMap::with_builtin_defaults();
    ///
    /// // Pre-bind as if the manager already acquired a proxy.
    /// let proxy_id = Uuid::new_v4();
    /// map.bind("example.com", proxy_id, Duration::from_mins(30));
    ///
    /// // Subsequent `acquire_session` for `Akamai` reuses the binding.
    /// let decision = map.acquire_session("example.com", VendorId::Akamai, &policy);
    /// assert_eq!(decision, SessionDecision::UseSticky(proxy_id));
    ///
    /// // `PerimeterX` always evicts the binding and asks for fresh
    /// // (FreshPerDomain semantics from the 2026 guide).
    /// let decision = map.acquire_session("example.com", VendorId::PerimeterX, &policy);
    /// assert_eq!(decision, SessionDecision::AcquireFresh);
    /// assert_eq!(map.lookup("example.com"), None, "PerimeterX evicts the binding");
    /// ```
    #[must_use]
    pub fn acquire_session(
        &self,
        domain: &str,
        vendor: VendorId,
        policy_map: &VendorStickinessMap,
    ) -> SessionDecision {
        let policy = policy_map.for_vendor(vendor);
        let ttl = match policy {
            StickinessPolicy::StickyForever => Some(Duration::MAX),
            StickinessPolicy::StickyForTtl { ttl } => Some(ttl),
            StickinessPolicy::StickyForRequestCount { .. }
            | StickinessPolicy::FreshPerRequest
            | StickinessPolicy::FreshPerDomain => None,
        };

        // Sticky path: reuse existing binding, otherwise ask caller to
        // acquire-and-bind with the policy TTL. Fresh path: evict any
        // prior binding for `FreshPerDomain` and always ask caller to
        // acquire fresh — `FreshPerRequest` and
        // `StickyForRequestCount` leave any existing binding untouched
        // (no per-domain semantics).
        let evict_for_fresh_domain = matches!(policy, StickinessPolicy::FreshPerDomain);
        ttl.map_or_else(
            || {
                if evict_for_fresh_domain {
                    self.unbind(domain);
                }
                SessionDecision::AcquireFresh
            },
            |ttl| {
                self.lookup(domain).map_or(
                    SessionDecision::AcquireAndBind(ttl),
                    SessionDecision::UseSticky,
                )
            },
        )
    }
}

// ── SessionDecision ──────────────────────────────────────────────────────────

/// Outcome of [`SessionMap::acquire_session`].
///
/// Describes whether the caller should reuse an existing sticky binding,
/// acquire a fresh proxy (without binding), or acquire a fresh proxy and
/// bind it for a TTL. Returned by `acquire_session` so the caller can
/// drive the rotation-strategy call itself (the
/// [`SessionMap`](Self) stays pure — no async, no strategy dependency).
///
/// # Example
///
/// ```
/// use stygian_proxy::session::SessionDecision;
/// use std::time::Duration;
/// use uuid::Uuid;
///
/// let id = Uuid::new_v4();
/// let decision = SessionDecision::UseSticky(id);
/// assert!(matches!(decision, SessionDecision::UseSticky(_)));
/// let decision = SessionDecision::AcquireAndBind(Duration::from_mins(30));
/// assert!(matches!(decision, SessionDecision::AcquireAndBind(_)));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionDecision {
    /// Reuse the existing sticky binding for `proxy_id`. No fresh
    /// acquisition is needed.
    UseSticky(Uuid),
    /// Acquire a fresh proxy via the rotation strategy; do **not** bind
    /// it. Used for `FreshPerRequest` (and as the safe fallback for
    /// `StickyForRequestCount`).
    AcquireFresh,
    /// Acquire a fresh proxy via the rotation strategy and bind it for
    /// `ttl`. Used for `StickyForTtl` and `StickyForever` when no
    /// binding currently exists.
    AcquireAndBind(Duration),
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
        map.bind("example.com", id, Duration::from_mins(1));
        assert_eq!(map.lookup("example.com"), Some(id));
        assert_eq!(map.lookup("example.com"), Some(id));
    }

    #[test]
    fn different_domains_independent() {
        let map = SessionMap::new();
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        map.bind("a.com", id_a, Duration::from_mins(1));
        map.bind("b.com", id_b, Duration::from_mins(1));
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
        map.bind("active.com", Uuid::new_v4(), Duration::from_mins(5));
        std::thread::sleep(Duration::from_millis(1));

        let removed = map.purge_expired();
        assert_eq!(removed, 1);
        assert_eq!(map.active_count(), 1);
    }

    #[test]
    fn unbind_removes_session() {
        let map = SessionMap::new();
        map.bind("example.com", Uuid::new_v4(), Duration::from_mins(1));
        map.unbind("example.com");
        assert_eq!(map.lookup("example.com"), None);
    }

    #[test]
    fn rebind_overwrites_previous() {
        let map = SessionMap::new();
        let old_id = Uuid::new_v4();
        let new_id = Uuid::new_v4();
        map.bind("example.com", old_id, Duration::from_mins(1));
        map.bind("example.com", new_id, Duration::from_mins(1));
        assert_eq!(map.lookup("example.com"), Some(new_id));
    }

    #[test]
    fn policy_domain_default_ttl() {
        let policy = StickyPolicy::domain_default();
        assert!(matches!(policy, StickyPolicy::Domain { ttl } if ttl == Duration::from_mins(5)));
    }

    #[test]
    fn policy_disabled_by_default() {
        let policy = StickyPolicy::default();
        assert!(policy.is_disabled());
    }

    #[test]
    fn policy_serde_roundtrip() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let policy = StickyPolicy::domain(Duration::from_mins(2));
        let json = serde_json::to_string(&policy)?;
        let back: StickyPolicy = serde_json::from_str(&json)?;
        assert!(matches!(back, StickyPolicy::Domain { ttl } if ttl == Duration::from_mins(2)));
        Ok(())
    }

    // ── T99: per-vendor acquire_session ─────────────────────────────────────

    use crate::stickiness::{StickinessPolicy, VendorStickinessMap};

    fn vendor_policy_map() -> VendorStickinessMap {
        VendorStickinessMap::with_builtin_defaults()
    }

    #[test]
    fn acquire_session_akamai_no_binding_returns_acquire_and_bind_30min() {
        let map = SessionMap::new();
        let policy = vendor_policy_map();

        let decision = map.acquire_session("example.com", VendorId::Akamai, &policy);
        assert_eq!(
            decision,
            SessionDecision::AcquireAndBind(Duration::from_mins(30))
        );
    }

    #[test]
    fn acquire_session_akamai_with_existing_binding_returns_sticky() {
        let map = SessionMap::new();
        let policy = vendor_policy_map();
        let proxy_id = Uuid::new_v4();

        // Simulate the manager having bound the proxy on a prior call.
        map.bind("example.com", proxy_id, Duration::from_mins(30));

        let decision = map.acquire_session("example.com", VendorId::Akamai, &policy);
        assert_eq!(decision, SessionDecision::UseSticky(proxy_id));
    }

    #[test]
    fn acquire_session_akamai_100_calls_within_ttl_return_same_proxy() {
        let map = SessionMap::new();
        let policy = vendor_policy_map();
        let proxy_id = Uuid::new_v4();
        map.bind("example.com", proxy_id, Duration::from_mins(30));

        for _ in 0..100 {
            assert_eq!(
                map.acquire_session("example.com", VendorId::Akamai, &policy),
                SessionDecision::UseSticky(proxy_id)
            );
        }
    }

    #[test]
    fn acquire_session_akamai_expired_binding_returns_acquire_and_bind() {
        let map = SessionMap::new();
        let policy = vendor_policy_map();
        let stale_id = Uuid::new_v4();

        // TTL of 0 expires immediately.
        map.bind("example.com", stale_id, Duration::ZERO);
        std::thread::sleep(Duration::from_millis(1));

        let decision = map.acquire_session("example.com", VendorId::Akamai, &policy);
        assert_eq!(
            decision,
            SessionDecision::AcquireAndBind(Duration::from_mins(30))
        );
    }

    #[test]
    fn acquire_session_cloudflare_no_binding_returns_acquire_and_bind_5min() {
        let map = SessionMap::new();
        let policy = vendor_policy_map();

        let decision = map.acquire_session("example.com", VendorId::Cloudflare, &policy);
        assert_eq!(
            decision,
            SessionDecision::AcquireAndBind(Duration::from_mins(5))
        );
    }

    #[test]
    fn acquire_session_imperva_no_binding_returns_acquire_and_bind_15min() {
        let map = SessionMap::new();
        let policy = vendor_policy_map();

        let decision = map.acquire_session("example.com", VendorId::Imperva, &policy);
        assert_eq!(
            decision,
            SessionDecision::AcquireAndBind(Duration::from_mins(15))
        );
    }

    #[test]
    fn acquire_session_data_dome_always_returns_acquire_fresh() {
        let map = SessionMap::new();
        let policy = vendor_policy_map();

        // Even with a binding in place, DataDome says "fresh per request".
        map.bind("example.com", Uuid::new_v4(), Duration::from_hours(1));
        let decision = map.acquire_session("example.com", VendorId::DataDome, &policy);
        assert_eq!(decision, SessionDecision::AcquireFresh);
    }

    #[test]
    fn acquire_session_perimeter_x_evicts_existing_binding() {
        let map = SessionMap::new();
        let policy = vendor_policy_map();
        map.bind("example.com", Uuid::new_v4(), Duration::from_hours(1));

        let decision = map.acquire_session("example.com", VendorId::PerimeterX, &policy);
        assert_eq!(decision, SessionDecision::AcquireFresh);
        // FreshPerDomain must evict the prior binding so the next call
        // also acquires fresh.
        assert_eq!(map.lookup("example.com"), None);
    }

    #[test]
    fn acquire_session_perimeter_x_no_existing_binding_returns_fresh() {
        let map = SessionMap::new();
        let policy = vendor_policy_map();
        let decision = map.acquire_session("example.com", VendorId::PerimeterX, &policy);
        assert_eq!(decision, SessionDecision::AcquireFresh);
    }

    #[test]
    fn acquire_session_kasada_evicts_existing_binding() {
        let map = SessionMap::new();
        let policy = vendor_policy_map();
        map.bind("example.com", Uuid::new_v4(), Duration::from_hours(1));

        let decision = map.acquire_session("example.com", VendorId::Kasada, &policy);
        assert_eq!(decision, SessionDecision::AcquireFresh);
        assert_eq!(map.lookup("example.com"), None);
    }

    #[test]
    fn acquire_session_unknown_vendor_defaults_to_fresh() {
        let map = SessionMap::new();
        let policy = vendor_policy_map();
        map.bind("example.com", Uuid::new_v4(), Duration::from_hours(1));

        // Unknown vendors inherit the safest default (FreshPerRequest),
        // which leaves any binding alone but asks the caller to acquire
        // fresh — different from FreshPerDomain, which would evict the
        // binding.
        let decision = map.acquire_session("example.com", VendorId::Unknown, &policy);
        assert_eq!(decision, SessionDecision::AcquireFresh);
        // FreshPerRequest does not evict prior bindings.
        assert!(map.lookup("example.com").is_some());
    }

    #[test]
    fn acquire_session_sticky_forever_uses_max_duration() {
        let map = SessionMap::new();
        let custom = VendorStickinessMap::new()
            .with_override(VendorId::Akamai, StickinessPolicy::StickyForever);

        let decision = map.acquire_session("example.com", VendorId::Akamai, &custom);
        assert_eq!(decision, SessionDecision::AcquireAndBind(Duration::MAX));
    }

    #[test]
    fn acquire_session_sticky_for_request_count_treated_as_fresh() {
        let map = SessionMap::new();
        let custom = VendorStickinessMap::new().with_override(
            VendorId::Akamai,
            StickinessPolicy::StickyForRequestCount { max_requests: 5 },
        );

        let decision = map.acquire_session("example.com", VendorId::Akamai, &custom);
        assert_eq!(decision, SessionDecision::AcquireFresh);
    }

    #[test]
    fn acquire_session_sticky_forever_uses_existing_binding() {
        let map = SessionMap::new();
        let custom = VendorStickinessMap::new()
            .with_override(VendorId::Akamai, StickinessPolicy::StickyForever);
        let proxy_id = Uuid::new_v4();
        map.bind("example.com", proxy_id, Duration::from_hours(1));

        let decision = map.acquire_session("example.com", VendorId::Akamai, &custom);
        assert_eq!(decision, SessionDecision::UseSticky(proxy_id));
    }

    #[test]
    fn acquire_session_override_replaces_builtin_akamai_policy() {
        let map = SessionMap::new();
        let policy = vendor_policy_map().with_override(
            VendorId::Akamai,
            StickinessPolicy::StickyForTtl {
                ttl: Duration::from_mins(2),
            },
        );

        let decision = map.acquire_session("example.com", VendorId::Akamai, &policy);
        assert_eq!(
            decision,
            SessionDecision::AcquireAndBind(Duration::from_mins(2))
        );
    }

    #[test]
    fn acquire_session_empty_map_defaults_all_to_fresh() {
        // A `VendorStickinessMap::new()` (no built-ins) is the operator's
        // way of saying "fresh for everything". This must be safe even
        // when no entries are present.
        let map = SessionMap::new();
        let empty = VendorStickinessMap::new();
        map.bind("example.com", Uuid::new_v4(), Duration::from_hours(1));

        for vendor in [
            VendorId::Akamai,
            VendorId::Cloudflare,
            VendorId::DataDome,
            VendorId::PerimeterX,
            VendorId::Kasada,
            VendorId::Imperva,
            VendorId::Unknown,
            VendorId::Hcaptcha,
        ] {
            assert_eq!(
                map.acquire_session("example.com", vendor, &empty),
                SessionDecision::AcquireFresh,
                "{vendor:?} should default to fresh when no entry exists"
            );
        }
    }
}
