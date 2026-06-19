//! Per-vendor session stickiness policy.
//!
//! The 2026 scraping guide (see
//! `docs/dev/project/scraping-guide-2026-llm-context.md` §"PROXY PROVIDERS
//! AND TYPES") describes an anti-bot-specific stickiness matrix:
//!
//! | Vendor          | Recommended stickiness                                            |
//! | --------------- | ----------------------------------------------------------------- |
//! | `Akamai`        | Static / ISP IP, sticky for the lifetime of a long session.       |
//! | `PerimeterX`    | Fresh session per domain (Camoufox + residential flow).          |
//! | `DataDome`      | No stickiness — fresh proxy per request; mobile carrier wins.    |
//! | `Kasada`        | Fresh session per domain.                                         |
//! | `Cloudflare`    | Short (≈5 min) sticky window — `cf_clearance` re-issue cadence.  |
//! | `Imperva`       | Medium (≈15 min) sticky window.                                   |
//! | everything else | Fresh proxy per request (safest default for unknown vendors).     |
//!
//! [`VendorStickinessMap`] encodes that matrix as a typed
//! `BTreeMap<VendorId, StickinessPolicy>` so the [`SessionMap`](crate::session::SessionMap)
//! and [`ProxyManager`](crate::manager::ProxyManager) can pick the
//! correct sticky slot automatically when a session is requested for
//! `(domain, vendor)`.
//!
//! ## Feature flag
//!
//! The data types in this module are always compiled (so external
//! adapters can build a [`VendorStickinessMap`] without enabling the
//! feature). The wiring into
//! [`ProxyManager::acquire_for_domain_with_vendor`](crate::manager::ProxyManager::acquire_for_domain_with_vendor)
//! is gated behind the `vendor-stickiness` cargo feature (off by
//! default; wired into the `full` aggregator).
//!
//! ## Example
//!
//! ```rust
//! use std::time::Duration;
//! use stygian_proxy::stickiness::{StickinessPolicy, VendorStickinessMap};
//! use stygian_proxy::types::VendorId;
//!
//! // Built-in defaults from the 2026 guide.
//! let defaults = VendorStickinessMap::with_builtin_defaults();
//! assert_eq!(
//!     defaults.for_vendor(VendorId::Akamai),
//!     StickinessPolicy::StickyForTtl { ttl: Duration::from_mins(30) }
//! );
//! assert_eq!(
//!     defaults.for_vendor(VendorId::DataDome),
//!     StickinessPolicy::FreshPerRequest
//! );
//! assert_eq!(
//!     defaults.for_vendor(VendorId::Unknown),
//!     StickinessPolicy::FreshPerRequest
//!
//! ); // unknown vendors fall back to the safest default
//!
//! // Operators can override individual entries before applying built-ins.
//! let custom = VendorStickinessMap::with_builtin_defaults()
//!     .with_override(VendorId::Akamai, StickinessPolicy::StickyForever);
//! assert_eq!(custom.for_vendor(VendorId::Akamai), StickinessPolicy::StickyForever);
//! ```

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::types::VendorId;

/// Built-in TTL for `Akamai` sticky sessions (2026 guide L2734).
const AKAMAI_STICKY_TTL: Duration = Duration::from_mins(30);
/// Built-in TTL for `Cloudflare` sticky sessions (5 min — `cf_clearance`
/// re-issue cadence per the 2026 guide).
const CLOUDFLARE_STICKY_TTL: Duration = Duration::from_mins(5);
/// Built-in TTL for `Imperva` sticky sessions.
const IMPERVA_STICKY_TTL: Duration = Duration::from_mins(15);

/// Session stickiness policy keyed by anti-bot [`VendorId`].
///
/// Different vendors reward different proxy-rotation cadences:
///
/// - `Akamai` accumulates trust on a consistent ISP IP, so the session
///   should be **sticky** for the whole scrape lifetime.
/// - `Cloudflare` re-issues `cf_clearance` cookies on a short cadence,
///   so a 5 min sticky window matches the re-issue cadence and avoids
///   burning the cookie on a single use.
/// - `PerimeterX` / `Kasada` flag frequent proxy changes on the same
///   domain, so each domain request should pick a **fresh** proxy.
/// - `DataDome` doesn't care about stickiness — it scores mobile
///   carriers above all else — so the safe default is **fresh per
///   request**.
/// - Everything else falls back to `FreshPerRequest` so an unknown
///   vendor can never inherit a permissive sticky policy by accident.
///
/// `Copy + Eq + Hash + Display + Debug` for ergonomic logging and use as
/// a value type.
///
/// # Example
/// ```
/// use std::time::Duration;
/// use stygian_proxy::stickiness::StickinessPolicy;
/// assert_eq!(
///     StickinessPolicy::StickyForTtl { ttl: Duration::from_mins(30) },
///     StickinessPolicy::StickyForTtl { ttl: Duration::from_mins(30) }
/// );
/// assert_eq!(format!("{}", StickinessPolicy::FreshPerRequest), "fresh_per_request");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum StickinessPolicy {
    /// Pin the same proxy for the entire process lifetime (or until the
    /// bound proxy fails its circuit breaker).
    StickyForever,
    /// Pin the same proxy for a fixed TTL; pick fresh after expiry.
    StickyForTtl {
        /// How long the domain→proxy binding remains valid.
        #[serde(with = "serde_duration_secs")]
        ttl: Duration,
    },
    /// Pin the same proxy for at most `max_requests` requests; reset on
    /// each request. Not currently enforced at the
    /// [`SessionMap`](crate::session::SessionMap) layer — the policy is
    /// treated as `FreshPerRequest` when consulted by
    /// [`acquire_session`](crate::session::SessionMap::acquire_session).
    /// Kept in the enum for future per-request counters.
    StickyForRequestCount {
        /// Maximum number of requests per binding.
        max_requests: u32,
    },
    /// Pick a fresh proxy for every request. No domain→proxy binding is
    /// created or retained.
    FreshPerRequest,
    /// Pick a fresh proxy per domain request, evicting any prior binding.
    ///
    /// The "per domain" qualifier means a request to a **different**
    /// domain for the same vendor reuses its own binding; a request to
    /// the same domain forces fresh.
    FreshPerDomain,
}

impl std::fmt::Display for StickinessPolicy {
    /// Stable, lower-case wire label for log output.
    ///
    /// # Example
    /// ```
    /// use std::time::Duration;
    /// use stygian_proxy::stickiness::StickinessPolicy;
    /// assert_eq!(format!("{}", StickinessPolicy::FreshPerRequest), "fresh_per_request");
    /// assert_eq!(format!("{}", StickinessPolicy::StickyForever), "sticky_forever");
    /// assert_eq!(
    ///     format!("{}", StickinessPolicy::StickyForTtl { ttl: Duration::from_secs(60) }),
    ///     "sticky_for_ttl(60s)"
    /// );
    /// ```
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StickyForever => f.write_str("sticky_forever"),
            Self::StickyForTtl { ttl } => write!(f, "sticky_for_ttl({}s)", ttl.as_secs()),
            Self::StickyForRequestCount { max_requests } => {
                write!(f, "sticky_for_request_count({max_requests})")
            }
            Self::FreshPerDomain => f.write_str("fresh_per_domain"),
            Self::FreshPerRequest => f.write_str("fresh_per_request"),
        }
    }
}

/// `BTreeMap`-backed stickiness policy keyed by anti-bot [`VendorId`].
///
/// Use [`with_builtin_defaults`](Self::with_builtin_defaults) to seed the
/// map with the 2026 guide defaults, then chain
/// [`with_override`](Self::with_override) to customise individual vendors
/// before installing the result on a
/// [`ProxyManager`](crate::manager::ProxyManager).
///
/// # Example
/// ```
/// use std::time::Duration;
/// use stygian_proxy::stickiness::{StickinessPolicy, VendorStickinessMap};
/// use stygian_proxy::types::VendorId;
///
/// let map = VendorStickinessMap::with_builtin_defaults();
/// assert_eq!(
///     map.for_vendor(VendorId::Akamai),
///     StickinessPolicy::StickyForTtl { ttl: Duration::from_mins(30) }
/// );
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VendorStickinessMap(BTreeMap<VendorId, StickinessPolicy>);

impl VendorStickinessMap {
    /// Empty map — every vendor falls back to
    /// [`StickinessPolicy::FreshPerRequest`] at lookup time.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::stickiness::{StickinessPolicy, VendorStickinessMap};
    /// use stygian_proxy::types::VendorId;
    /// let map = VendorStickinessMap::new();
    /// assert!(map.is_empty());
    /// assert_eq!(map.for_vendor(VendorId::Akamai), StickinessPolicy::FreshPerRequest);
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Built-in defaults from the 2026 scraping guide:
    ///
    /// - `Akamai` → [`StickyForTtl`](StickinessPolicy::StickyForTtl) 30 min
    /// - `Cloudflare` → [`StickyForTtl`](StickinessPolicy::StickyForTtl) 5 min
    /// - `Imperva` → [`StickyForTtl`](StickinessPolicy::StickyForTtl) 15 min
    /// - `PerimeterX` → [`FreshPerDomain`](StickinessPolicy::FreshPerDomain)
    /// - `Kasada` → [`FreshPerDomain`](StickinessPolicy::FreshPerDomain)
    /// - `DataDome` → [`FreshPerRequest`](StickinessPolicy::FreshPerRequest)
    /// - everything else → [`FreshPerRequest`](StickinessPolicy::FreshPerRequest)
    ///   (safest default for unknown vendors)
    ///
    /// # Example
    /// ```
    /// use std::time::Duration;
    /// use stygian_proxy::stickiness::{StickinessPolicy, VendorStickinessMap};
    /// use stygian_proxy::types::VendorId;
    ///
    /// let map = VendorStickinessMap::with_builtin_defaults();
    /// assert_eq!(
    ///     map.for_vendor(VendorId::Akamai),
    ///     StickinessPolicy::StickyForTtl { ttl: Duration::from_mins(30) }
    /// );
    /// assert_eq!(map.for_vendor(VendorId::DataDome), StickinessPolicy::FreshPerRequest);
    /// assert_eq!(
    ///     map.for_vendor(VendorId::PerimeterX),
    ///     StickinessPolicy::FreshPerDomain
    /// );
    /// ```
    #[must_use]
    pub fn with_builtin_defaults() -> Self {
        let mut entries = BTreeMap::new();
        entries.insert(
            VendorId::Akamai,
            StickinessPolicy::StickyForTtl {
                ttl: AKAMAI_STICKY_TTL,
            },
        );
        entries.insert(
            VendorId::Cloudflare,
            StickinessPolicy::StickyForTtl {
                ttl: CLOUDFLARE_STICKY_TTL,
            },
        );
        entries.insert(VendorId::DataDome, StickinessPolicy::FreshPerRequest);
        entries.insert(
            VendorId::Imperva,
            StickinessPolicy::StickyForTtl {
                ttl: IMPERVA_STICKY_TTL,
            },
        );
        entries.insert(VendorId::Kasada, StickinessPolicy::FreshPerDomain);
        entries.insert(VendorId::PerimeterX, StickinessPolicy::FreshPerDomain);
        Self(entries)
    }

    /// Look up the policy for `vendor`.
    ///
    /// Unknown vendors (including [`VendorId::Unknown`]) fall back to
    /// [`StickinessPolicy::FreshPerRequest`] — the safest default.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::stickiness::{StickinessPolicy, VendorStickinessMap};
    /// use stygian_proxy::types::VendorId;
    /// let map = VendorStickinessMap::with_builtin_defaults();
    /// assert_eq!(map.for_vendor(VendorId::DataDome), StickinessPolicy::FreshPerRequest);
    /// assert_eq!(map.for_vendor(VendorId::Unknown), StickinessPolicy::FreshPerRequest);
    /// ```
    #[must_use]
    pub fn for_vendor(&self, vendor: VendorId) -> StickinessPolicy {
        self.0
            .get(&vendor)
            .copied()
            .unwrap_or(StickinessPolicy::FreshPerRequest)
    }

    /// Insert or replace the policy for `vendor`. Builder-style: takes
    /// `self` by value and returns the updated map so calls can be
    /// chained.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::stickiness::{StickinessPolicy, VendorStickinessMap};
    /// use stygian_proxy::types::VendorId;
    ///
    /// let map = VendorStickinessMap::with_builtin_defaults()
    ///     .with_override(VendorId::Akamai, StickinessPolicy::StickyForever);
    /// assert_eq!(map.for_vendor(VendorId::Akamai), StickinessPolicy::StickyForever);
    /// ```
    #[must_use]
    pub fn with_override(mut self, vendor: VendorId, policy: StickinessPolicy) -> Self {
        self.0.insert(vendor, policy);
        self
    }

    /// Returns `true` when no vendor policies have been registered.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::stickiness::VendorStickinessMap;
    /// assert!(VendorStickinessMap::new().is_empty());
    /// assert!(!VendorStickinessMap::with_builtin_defaults().is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the number of registered vendor policies.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::stickiness::VendorStickinessMap;
    /// use stygian_proxy::types::VendorId;
    /// let map = VendorStickinessMap::with_builtin_defaults();
    /// assert_eq!(map.len(), 6);
    /// let map = VendorStickinessMap::new()
    ///     .with_override(VendorId::Akamai, stygian_proxy::stickiness::StickinessPolicy::StickyForever);
    /// assert_eq!(map.len(), 1);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Iterate `(vendor, policy)` pairs in deterministic (sorted) order.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::stickiness::{StickinessPolicy, VendorStickinessMap};
    /// use stygian_proxy::types::VendorId;
    ///
    /// let map = VendorStickinessMap::with_builtin_defaults();
    /// let entries: Vec<_> = map.iter().collect();
    /// // Sorted by VendorId discriminant order — `Akamai` < `Cloudflare`.
    /// assert_eq!(entries.first().map(|(v, _)| *v), Some(VendorId::Akamai));
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = (VendorId, StickinessPolicy)> + '_ {
        self.0.iter().map(|(v, p)| (*v, *p))
    }
}

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

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn new_is_empty() {
        let map = VendorStickinessMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn default_is_empty() {
        let map = VendorStickinessMap::default();
        assert!(map.is_empty());
    }

    #[test]
    fn for_vendor_unknown_returns_fresh_per_request() {
        let map = VendorStickinessMap::new();
        assert_eq!(
            map.for_vendor(VendorId::Unknown),
            StickinessPolicy::FreshPerRequest
        );
        assert_eq!(
            map.for_vendor(VendorId::Akamai),
            StickinessPolicy::FreshPerRequest
        );
    }

    #[test]
    fn with_override_inserts_entry() {
        let map = VendorStickinessMap::new()
            .with_override(VendorId::Akamai, StickinessPolicy::StickyForever);
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.for_vendor(VendorId::Akamai),
            StickinessPolicy::StickyForever
        );
    }

    #[test]
    fn with_override_replaces_existing_entry() {
        let map = VendorStickinessMap::new()
            .with_override(VendorId::Akamai, StickinessPolicy::StickyForever)
            .with_override(VendorId::Akamai, StickinessPolicy::FreshPerDomain);
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.for_vendor(VendorId::Akamai),
            StickinessPolicy::FreshPerDomain
        );
    }

    #[test]
    fn built_in_defaults_akamai_is_30min_sticky() {
        let map = VendorStickinessMap::with_builtin_defaults();
        assert_eq!(
            map.for_vendor(VendorId::Akamai),
            StickinessPolicy::StickyForTtl {
                ttl: Duration::from_mins(30)
            }
        );
    }

    #[test]
    fn built_in_defaults_cloudflare_is_5min_sticky() {
        let map = VendorStickinessMap::with_builtin_defaults();
        assert_eq!(
            map.for_vendor(VendorId::Cloudflare),
            StickinessPolicy::StickyForTtl {
                ttl: Duration::from_mins(5)
            }
        );
    }

    #[test]
    fn built_in_defaults_imperva_is_15min_sticky() {
        let map = VendorStickinessMap::with_builtin_defaults();
        assert_eq!(
            map.for_vendor(VendorId::Imperva),
            StickinessPolicy::StickyForTtl {
                ttl: Duration::from_mins(15)
            }
        );
    }

    #[test]
    fn built_in_defaults_perimeter_x_is_fresh_per_domain() {
        let map = VendorStickinessMap::with_builtin_defaults();
        assert_eq!(
            map.for_vendor(VendorId::PerimeterX),
            StickinessPolicy::FreshPerDomain
        );
    }

    #[test]
    fn built_in_defaults_kasada_is_fresh_per_domain() {
        let map = VendorStickinessMap::with_builtin_defaults();
        assert_eq!(
            map.for_vendor(VendorId::Kasada),
            StickinessPolicy::FreshPerDomain
        );
    }

    #[test]
    fn built_in_defaults_data_dome_is_fresh_per_request() {
        let map = VendorStickinessMap::with_builtin_defaults();
        assert_eq!(
            map.for_vendor(VendorId::DataDome),
            StickinessPolicy::FreshPerRequest
        );
    }

    #[test]
    fn built_in_defaults_unknown_vendor_falls_back_to_fresh_per_request() {
        let map = VendorStickinessMap::with_builtin_defaults();
        assert_eq!(
            map.for_vendor(VendorId::Unknown),
            StickinessPolicy::FreshPerRequest
        );
        assert_eq!(
            map.for_vendor(VendorId::Hcaptcha),
            StickinessPolicy::FreshPerRequest
        );
        assert_eq!(
            map.for_vendor(VendorId::ShapeSecurity),
            StickinessPolicy::FreshPerRequest
        );
    }

    #[test]
    fn built_in_defaults_has_six_entries() {
        let map = VendorStickinessMap::with_builtin_defaults();
        assert_eq!(map.len(), 6);
    }

    #[test]
    fn built_in_defaults_iterates_in_sorted_order() {
        let map = VendorStickinessMap::with_builtin_defaults();
        let entries: Vec<_> = map.iter().map(|(v, _)| v).collect();
        // BTreeMap orders by VendorId discriminant — PerimeterX (3)
        // precedes Kasada (6) precedes Imperva (9).
        assert_eq!(
            entries,
            vec![
                VendorId::Akamai,
                VendorId::Cloudflare,
                VendorId::DataDome,
                VendorId::PerimeterX,
                VendorId::Kasada,
                VendorId::Imperva,
            ]
        );
    }

    #[test]
    fn override_chained_before_builtins_replaces_entry() {
        // Per the spec, operators chain `with_override` BEFORE
        // `with_builtin_defaults` so the override takes precedence.
        // We allow chaining in either order via the builder-style API;
        // here we verify the documented usage shape.
        let map = VendorStickinessMap::with_builtin_defaults()
            .with_override(VendorId::Akamai, StickinessPolicy::StickyForever);
        assert_eq!(
            map.for_vendor(VendorId::Akamai),
            StickinessPolicy::StickyForever
        );
        // Other entries remain at their built-in defaults.
        assert_eq!(
            map.for_vendor(VendorId::Cloudflare),
            StickinessPolicy::StickyForTtl {
                ttl: Duration::from_mins(5)
            }
        );
    }

    #[test]
    fn stickiness_policy_is_copy() {
        let policy = StickinessPolicy::StickyForTtl {
            ttl: Duration::from_mins(30),
        };
        let copy = policy;
        assert_eq!(policy, copy);
    }

    #[test]
    fn stickiness_policy_is_hash_eq() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(StickinessPolicy::StickyForever);
        set.insert(StickinessPolicy::StickyForTtl {
            ttl: Duration::from_mins(30),
        });
        set.insert(StickinessPolicy::FreshPerRequest);
        assert_eq!(set.len(), 3);
        assert!(set.contains(&StickinessPolicy::StickyForever));
    }

    #[test]
    fn stickiness_policy_display_matches_snake_case_label() {
        assert_eq!(
            format!("{}", StickinessPolicy::StickyForever),
            "sticky_forever"
        );
        assert_eq!(
            format!(
                "{}",
                StickinessPolicy::StickyForTtl {
                    ttl: Duration::from_mins(1)
                }
            ),
            "sticky_for_ttl(60s)"
        );
        assert_eq!(
            format!(
                "{}",
                StickinessPolicy::StickyForRequestCount { max_requests: 5 }
            ),
            "sticky_for_request_count(5)"
        );
        assert_eq!(
            format!("{}", StickinessPolicy::FreshPerDomain),
            "fresh_per_domain"
        );
        assert_eq!(
            format!("{}", StickinessPolicy::FreshPerRequest),
            "fresh_per_request"
        );
    }

    #[test]
    fn stickiness_policy_round_trips_through_json() {
        let policies = [
            StickinessPolicy::StickyForever,
            StickinessPolicy::StickyForTtl {
                ttl: Duration::from_mins(30),
            },
            StickinessPolicy::StickyForRequestCount { max_requests: 7 },
            StickinessPolicy::FreshPerDomain,
            StickinessPolicy::FreshPerRequest,
        ];
        for policy in policies {
            let json = serde_json::to_string(&policy).expect("serialize");
            let parsed: StickinessPolicy = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed, policy, "round-trip for {policy:?}");
        }
    }

    #[test]
    fn stickiness_policy_round_trips_through_toml() {
        let policies = [
            StickinessPolicy::StickyForever,
            StickinessPolicy::StickyForTtl {
                ttl: Duration::from_mins(30),
            },
            StickinessPolicy::StickyForRequestCount { max_requests: 7 },
            StickinessPolicy::FreshPerDomain,
            StickinessPolicy::FreshPerRequest,
        ];
        for policy in policies {
            let toml_str = toml::to_string(&policy).expect("serialize toml");
            let parsed: StickinessPolicy = toml::from_str(&toml_str).expect("deserialize toml");
            assert_eq!(parsed, policy, "round-trip for {policy:?}");
        }
    }

    #[test]
    fn vendor_stickiness_map_round_trips_through_json() {
        let map = VendorStickinessMap::with_builtin_defaults()
            .with_override(VendorId::Akamai, StickinessPolicy::StickyForever);
        let json = serde_json::to_string(&map).expect("serialize");
        let parsed: VendorStickinessMap = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, map);
    }

    #[test]
    fn vendor_stickiness_map_round_trips_through_toml() {
        let map = VendorStickinessMap::with_builtin_defaults()
            .with_override(VendorId::DataDome, StickinessPolicy::StickyForever);
        let toml_str = toml::to_string(&map).expect("serialize toml");
        let parsed: VendorStickinessMap = toml::from_str(&toml_str).expect("deserialize toml");
        assert_eq!(parsed, map);
    }

    #[test]
    fn vendor_stickiness_map_transparent_serde_orders_by_vendor_id() {
        // `#[serde(transparent)]` on `VendorStickinessMap` means the wire
        // form is the `BTreeMap` directly. Verify that the BTreeMap
        // ordering on the wire matches the sorted `VendorId`
        // discriminant.
        let map = VendorStickinessMap::with_builtin_defaults();
        let json = serde_json::to_string(&map).expect("serialize");
        let akamai_pos = json.find("\"akamai\"").expect("akamai present");
        let cloudflare_pos = json.find("\"cloudflare\"").expect("cloudflare present");
        assert!(
            akamai_pos < cloudflare_pos,
            "expected sorted order on wire: {json}"
        );
    }

    // ── integration with `StickyForRequestCount` (treated as fresh) ─────────

    #[test]
    fn stickiness_policy_for_request_count_variant_exists() {
        // Documented fallback: SessionMap treats `StickyForRequestCount`
        // as fresh. The variant must still round-trip cleanly through
        // serde so operators can persist policies that include it.
        let policy = StickinessPolicy::StickyForRequestCount { max_requests: 5 };
        let json = serde_json::to_string(&policy).expect("serialize");
        let parsed: StickinessPolicy = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, policy);
    }
}
