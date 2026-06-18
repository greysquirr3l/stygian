//! `PoW` capability profile store (T93).
//!
//! The store accumulates [`PowCapabilitySample`]s into
//! [`PowCapabilityProfile`]s keyed by
//! `(domain, target_class, vendor_family)`. It reuses the
//! same [`LruTtlStore`][crate::cache::LruTtlStore] primitive
//! the T83 [`ChallengeMemory`][crate::challenge_feedback::ChallengeMemory]
//! and the T91 [`NonceBook`][crate::token_lifecycle::NonceBook]
//! use — that keeps eviction + expiry semantics consistent
//! across all three short-horizon stores and satisfies the
//! "no new cache store" requirement.
//!
//! ## Key namespace
//!
//! The store keys are namespaced under `charon:pow:...` (see
//! [`pow_profile_key`]) so `PoW` entries never collide with
//! challenge-memory entries (`charon:challenge:...`) or
//! token-nonce entries (`charon:token_nonce:...`) on a
//! shared backing primitive. The namespace is
//! **prefix-stable** so operators can grep for it in a
//! future Redis-backed variant without renaming.
//!
//! ## Sampling window
//!
//! The store's TTL is the per-entry expiry horizon. The
//! **profile's** `observation_window_secs` is independent —
//! it documents how wide a window the profile was built
//! over. Callers that want the profile to also expire after
//! the window elapses can configure a TTL equal to the
//! window (the default TTL of 1 hour matches the default
//! sampling window).

use std::num::NonZeroUsize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::cache::LruTtlStore;
use crate::pow_profile::profile::{PowCapabilityProfile, PowCapabilitySample};
use crate::types::TargetClass;
use crate::vendor_classifier::VendorId;

/// Default TTL for the `PoW` capability store: **1 hour**.
///
/// Matches [`DEFAULT_SAMPLE_WINDOW_SECS`][crate::pow_profile::DEFAULT_SAMPLE_WINDOW_SECS]
/// (the default sampling window) so a profile that was
/// built over the default window expires exactly when the
/// window elapses. Operators that want a longer horizon can
/// call [`PowCapabilityStore::new`] with an explicit TTL.
pub const DEFAULT_POW_TTL: Duration = Duration::from_hours(1);

/// Default capacity (in `(domain, target_class, vendor)` triples)
/// for the `PoW` capability store.
#[allow(clippy::unwrap_used)]
pub const DEFAULT_POW_CAPACITY: NonZeroUsize = match NonZeroUsize::new(128) {
    Some(value) => value,
    None => NonZeroUsize::MIN,
};

/// Default system-clock fallback when wall-clock time is
/// unavailable. Small enough that a zero-second
/// `recorded_at_unix_secs` is distinguishable from a real
/// timestamp while still being a valid serialisation.
const ZERO_FALLBACK_UNIX_SECS: u64 = 0;

/// Build a stable, lower-cased key for the `PoW` capability
/// store.
///
/// The key uses a `charon:pow:...` namespace so `PoW` entries
/// never collide with `charon:challenge:...` (T83) or
/// `charon:token_nonce:...` (T91) on a shared backing
/// primitive.
///
/// # Example
///
/// ```
/// use stygian_charon::pow_profile::pow_profile_key;
/// use stygian_charon::types::TargetClass;
/// use stygian_charon::vendor_classifier::VendorId;
///
/// let key = pow_profile_key("Example.COM", TargetClass::Api, VendorId::Akamai);
/// assert!(key.starts_with("charon:pow:example.com:api:akamai"));
/// ```
#[must_use]
pub fn pow_profile_key(domain: &str, target_class: TargetClass, vendor: VendorId) -> String {
    format!(
        "charon:pow:{}:{}:{}",
        domain.to_ascii_lowercase(),
        target_class_label(target_class),
        vendor.label()
    )
}

const fn target_class_label(c: TargetClass) -> &'static str {
    match c {
        TargetClass::Api => "api",
        TargetClass::ContentSite => "content_site",
        TargetClass::HighSecurity => "high_security",
        TargetClass::Unknown => "unknown",
    }
}

/// Capacity-bounded LRU+TTL store of
/// [`PowCapabilityProfile`]s.
///
/// Reuses the same [`LruTtlStore`][crate::cache::LruTtlStore]
/// primitive the T83 [`ChallengeMemory`][crate::challenge_feedback::ChallengeMemory]
/// and the T91 [`NonceBook`][crate::token_lifecycle::NonceBook]
/// use. That keeps eviction + expiry semantics consistent
/// across all three short-horizon stores and satisfies the
/// "no new cache store" constraint.
///
/// # Example
///
/// ```
/// use stygian_charon::pow_profile::{PowCapabilitySample, PowCapabilityStore};
/// use stygian_charon::types::TargetClass;
/// use stygian_charon::vendor_classifier::VendorId;
///
/// let store = PowCapabilityStore::with_defaults();
/// store.record_sample(
///     "example.com",
///     TargetClass::ContentSite,
///     VendorId::Cloudflare,
///     &PowCapabilitySample::solved(1_000, 0),
/// );
/// let profile = store
///     .lookup("example.com", TargetClass::ContentSite, VendorId::Cloudflare)
///     .expect("profile");
/// assert_eq!(profile.solved_count, 1);
/// ```
pub struct PowCapabilityStore {
    store: LruTtlStore<PowCapabilityProfile>,
}

impl std::fmt::Debug for PowCapabilityStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PowCapabilityStore")
            .field("ttl", &self.store.ttl())
            .field("len", &self.store.len())
            .finish()
    }
}

impl PowCapabilityStore {
    /// Create a new store with explicit capacity and TTL.
    #[must_use]
    pub fn new(capacity: NonZeroUsize, ttl: Duration) -> Self {
        Self {
            store: LruTtlStore::new(capacity, ttl),
        }
    }

    /// Capacity-bounded [`PowCapabilityStore`] with
    /// [`DEFAULT_POW_TTL`].
    #[must_use]
    pub fn with_default_ttl(capacity: NonZeroUsize) -> Self {
        Self::new(capacity, DEFAULT_POW_TTL)
    }

    /// Capacity-bounded [`PowCapabilityStore`] with
    /// [`DEFAULT_POW_CAPACITY`] and [`DEFAULT_POW_TTL`].
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_POW_CAPACITY, DEFAULT_POW_TTL)
    }

    /// Configured TTL for the backing store.
    #[must_use]
    pub const fn ttl(&self) -> Duration {
        self.store.ttl()
    }

    /// Record a [`PowCapabilitySample`] for a
    /// `(domain, target_class, vendor)` triple. The store
    /// looks up the existing profile (if any), merges the
    /// sample into it, and re-inserts the updated profile
    /// under the LRU+TTL semantics shared with
    /// [`crate::cache::LruTtlStore`].
    ///
    /// The LRU recency is **not** bumped on the read so a
    /// high-volume key does not crowd out less common keys
    /// (matches the T83 / T91 pattern).
    pub fn record_sample(
        &self,
        domain: &str,
        target_class: TargetClass,
        vendor: VendorId,
        sample: &PowCapabilitySample,
    ) {
        let key = pow_profile_key(domain, target_class, vendor);
        let mut profile = self.store.peek(&key).unwrap_or_else(|| {
            PowCapabilityProfile::new(domain, target_class, vendor)
        });
        profile.merge(sample);
        // Refresh the recorded_at_unix_secs to the current
        // wall clock so the merge timestamp stays useful
        // even when peek returned a freshly-built default
        // (whose merge() already set the timestamp, but
        // the new path is explicit for readability).
        profile.recorded_at_unix_secs = current_unix_secs();
        self.store.put(key, profile);
    }

    /// Look up the current profile for a
    /// `(domain, target_class, vendor)` triple. Returns
    /// `None` if the key is absent or has expired.
    #[must_use]
    pub fn lookup(
        &self,
        domain: &str,
        target_class: TargetClass,
        vendor: VendorId,
    ) -> Option<PowCapabilityProfile> {
        self.store
            .get(&pow_profile_key(domain, target_class, vendor))
    }

    /// Number of profiles currently retained.
    #[must_use]
    pub fn len(&self) -> usize {
        self.store.len()
    }

    /// `true` when the store has zero profiles.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    /// Remove all profiles.
    pub fn clear(&self) {
        self.store.clear();
    }

    /// Invalidate a single `(domain, target_class, vendor)`
    /// key.
    pub fn invalidate(&self, domain: &str, target_class: TargetClass, vendor: VendorId) {
        self.store
            .invalidate(&pow_profile_key(domain, target_class, vendor));
    }
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(ZERO_FALLBACK_UNIX_SECS, |duration| duration.as_secs())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn record_sample_creates_new_profile_on_first_call() {
        let store = PowCapabilityStore::new(NonZeroUsize::new(4).unwrap(), DEFAULT_POW_TTL);
        store.record_sample(
            "example.com",
            TargetClass::ContentSite,
            VendorId::Cloudflare,
            &PowCapabilitySample::solved(1_000, 0),
        );
        let profile = store
            .lookup("example.com", TargetClass::ContentSite, VendorId::Cloudflare)
            .expect("profile");
        assert_eq!(profile.domain, "example.com");
        assert_eq!(profile.solved_count, 1);
        assert_eq!(profile.failed_count, 0);
        assert_eq!(profile.vendor_family, VendorId::Cloudflare);
    }

    #[test]
    fn record_sample_merges_into_existing_profile() {
        let store = PowCapabilityStore::new(NonZeroUsize::new(4).unwrap(), DEFAULT_POW_TTL);
        let key = ("example.com", TargetClass::ContentSite, VendorId::Cloudflare);
        store.record_sample(
            key.0,
            key.1,
            key.2,
            &PowCapabilitySample::solved(1_000, 0),
        );
        store.record_sample(
            key.0,
            key.1,
            key.2,
            &PowCapabilitySample::solved(1_500, 1),
        );
        store.record_sample(
            key.0,
            key.1,
            key.2,
            &PowCapabilitySample::failed(2_000, 1, crate::pow_profile::profile::PowFailureMode::Timeout),
        );
        let profile = store.lookup(key.0, key.1, key.2).expect("profile");
        assert_eq!(profile.solved_count, 2);
        assert_eq!(profile.failed_count, 1);
        assert_eq!(profile.retry_count, 2);
        assert_eq!(
            profile.failure_modes.get(&crate::pow_profile::profile::PowFailureMode::Timeout),
            Some(&1)
        );
    }

    #[test]
    fn distinct_keys_keep_distinct_profiles() {
        let store = PowCapabilityStore::new(NonZeroUsize::new(8).unwrap(), DEFAULT_POW_TTL);
        store.record_sample(
            "example.com",
            TargetClass::ContentSite,
            VendorId::Cloudflare,
            &PowCapabilitySample::solved(1_000, 0),
        );
        store.record_sample(
            "example.com",
            TargetClass::Api,
            VendorId::Cloudflare,
            &PowCapabilitySample::solved(2_000, 0),
        );
        store.record_sample(
            "example.com",
            TargetClass::ContentSite,
            VendorId::Akamai,
            &PowCapabilitySample::solved(3_000, 0),
        );
        let cs_cf = store
            .lookup("example.com", TargetClass::ContentSite, VendorId::Cloudflare)
            .unwrap();
        let api_cf = store
            .lookup("example.com", TargetClass::Api, VendorId::Cloudflare)
            .unwrap();
        let cs_ak = store
            .lookup("example.com", TargetClass::ContentSite, VendorId::Akamai)
            .unwrap();
        assert_eq!(cs_cf.solve_latency_ms_p50, Some(1_000));
        assert_eq!(api_cf.solve_latency_ms_p50, Some(2_000));
        assert_eq!(cs_ak.solve_latency_ms_p50, Some(3_000));
    }

    #[test]
    fn domain_is_normalised_to_lower_case() {
        let store = PowCapabilityStore::new(NonZeroUsize::new(4).unwrap(), DEFAULT_POW_TTL);
        store.record_sample(
            "Example.COM",
            TargetClass::Api,
            VendorId::Cloudflare,
            &PowCapabilitySample::solved(1_000, 0),
        );
        let profile = store
            .lookup("EXAMPLE.com", TargetClass::Api, VendorId::Cloudflare)
            .expect("profile");
        assert_eq!(profile.domain, "example.com");
    }

    #[test]
    fn entries_decay_after_ttl() {
        let store = PowCapabilityStore::new(NonZeroUsize::new(4).unwrap(), Duration::from_millis(1));
        store.record_sample(
            "example.com",
            TargetClass::Api,
            VendorId::Cloudflare,
            &PowCapabilitySample::solved(1_000, 0),
        );
        thread::sleep(Duration::from_millis(5));
        assert!(store
            .lookup("example.com", TargetClass::Api, VendorId::Cloudflare)
            .is_none());
    }

    #[test]
    fn clear_drops_everything() {
        let store = PowCapabilityStore::new(NonZeroUsize::new(4).unwrap(), DEFAULT_POW_TTL);
        store.record_sample(
            "a.example",
            TargetClass::Api,
            VendorId::Cloudflare,
            &PowCapabilitySample::solved(1_000, 0),
        );
        store.record_sample(
            "b.example",
            TargetClass::Api,
            VendorId::Cloudflare,
            &PowCapabilitySample::solved(1_000, 0),
        );
        assert_eq!(store.len(), 2);
        store.clear();
        assert!(store.is_empty());
    }

    #[test]
    fn invalidate_drops_single_key() {
        let store = PowCapabilityStore::new(NonZeroUsize::new(4).unwrap(), DEFAULT_POW_TTL);
        store.record_sample(
            "a.example",
            TargetClass::Api,
            VendorId::Cloudflare,
            &PowCapabilitySample::solved(1_000, 0),
        );
        store.record_sample(
            "b.example",
            TargetClass::Api,
            VendorId::Cloudflare,
            &PowCapabilitySample::solved(1_000, 0),
        );
        store.invalidate("a.example", TargetClass::Api, VendorId::Cloudflare);
        assert!(store
            .lookup("a.example", TargetClass::Api, VendorId::Cloudflare)
            .is_none());
        assert!(store
            .lookup("b.example", TargetClass::Api, VendorId::Cloudflare)
            .is_some());
    }

    #[test]
    fn lru_capacity_is_respected() {
        let store = PowCapabilityStore::new(NonZeroUsize::new(2).unwrap(), DEFAULT_POW_TTL);
        store.record_sample(
            "a.example",
            TargetClass::Api,
            VendorId::Cloudflare,
            &PowCapabilitySample::solved(1_000, 0),
        );
        store.record_sample(
            "b.example",
            TargetClass::Api,
            VendorId::Cloudflare,
            &PowCapabilitySample::solved(1_000, 0),
        );
        store.record_sample(
            "c.example",
            TargetClass::Api,
            VendorId::Cloudflare,
            &PowCapabilitySample::solved(1_000, 0),
        );
        assert!(store.len() <= 2);
    }

    #[test]
    fn key_namespace_is_pow_prefixed() {
        let key = pow_profile_key("Example.COM", TargetClass::Api, VendorId::Akamai);
        assert_eq!(key, "charon:pow:example.com:api:akamai");
    }

    #[test]
    fn default_ttl_matches_default_sample_window() {
        assert_eq!(
            DEFAULT_POW_TTL.as_secs(),
            crate::pow_profile::DEFAULT_SAMPLE_WINDOW_SECS
        );
    }
}
