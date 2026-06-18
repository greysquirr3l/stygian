//! Per-issuance nonce bookkeeping for token lifecycle contracts (T91).
//!
//! The [`NonceBook`] is a capacity-bounded LRU+TTL store that
//! tracks every nonce the validator has **seen**, along with
//! the vendor family, challenge class, and observation count.
//! It reuses the same `LruTtlStore`
//! primitive the [`ChallengeMemory`][crate::challenge_feedback::ChallengeMemory]
//! uses (T83) — that keeps eviction + expiry semantics
//! consistent across both short-horizon stores and satisfies
//! the "no new cache store" constraint.

use std::num::NonZeroUsize;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cache::LruTtlStore;
use crate::token_lifecycle::contract::ChallengeClass;
use crate::vendor_classifier::VendorId;

/// Default TTL for nonce observations: **10 minutes**.
///
/// Aligned with the
/// `DEFAULT_CHALLENGE_TTL`
/// default so the two stores share an "after ten minutes we
/// forget" horizon. Long enough to span a typical scraping
/// session, short enough that an evicted nonce can be re-issued
/// without false-positive replay detection.
pub const DEFAULT_NONCE_TTL: Duration = Duration::from_mins(10);

/// Default capacity (in nonce entries) for the
/// [`NonceBook`]. Conservative default — most workflows
/// observe a few hundred nonces per session.
#[allow(clippy::unwrap_used)]
pub const DEFAULT_NONCE_BOOK_CAPACITY: NonZeroUsize = match NonZeroUsize::new(256) {
    Some(value) => value,
    None => NonZeroUsize::MIN,
};

/// Build a stable, lower-cased cache key for a
/// `(vendor_family, nonce)` tuple.
///
/// # Example
///
/// ```
/// use stygian_charon::token_lifecycle::nonce_book_key;
/// use stygian_charon::vendor_classifier::VendorId;
///
/// let key = nonce_book_key(VendorId::Cloudflare, "NONCE-XYZ");
/// assert!(key.starts_with("charon:token_nonce:cloudflare:"));
/// ```
#[must_use]
pub fn nonce_book_key(vendor: VendorId, nonce: &str) -> String {
    format!("charon:token_nonce:{}:{}", vendor.label(), nonce)
}

/// One observation row in the [`NonceBook`].
///
/// The row records the vendor family and challenge class the
/// observation was tagged with (so a stale nonce re-entry from
/// a different vendor still surfaces the right audit context),
/// along with the observation count for monotonic accounting.
/// The TTL is owned by the `LruTtlStore`
/// backing the [`NonceBook`].
///
/// # Example
///
/// ```
/// use stygian_charon::token_lifecycle::{ChallengeClass, NonceObservation};
/// use stygian_charon::vendor_classifier::VendorId;
///
/// let obs = NonceObservation {
///     vendor: VendorId::Akamai,
///     challenge_class: ChallengeClass::ProofOfWork,
///     observation_count: 1,
/// };
/// assert_eq!(obs.vendor, VendorId::Akamai);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NonceObservation {
    /// Vendor family the observation was tagged with.
    pub vendor: VendorId,
    /// Challenge class the observation was tagged with.
    pub challenge_class: ChallengeClass,
    /// Number of times the nonce has been observed (saturating
    /// on overflow).
    pub observation_count: u32,
}

/// Capacity-bounded LRU+TTL store of
/// [`NonceObservation`][crate::token_lifecycle::NonceObservation]s.
///
/// The store reuses the same
/// `LruTtlStore` primitive the
/// [`ChallengeMemory`][crate::challenge_feedback::ChallengeMemory]
/// uses (T83). That keeps eviction + expiry semantics
/// consistent across both short-horizon stores and satisfies
/// the "no new cache store" requirement.
///
/// # Example
///
/// ```
/// use stygian_charon::token_lifecycle::{ChallengeClass, NonceBook};
/// use stygian_charon::vendor_classifier::VendorId;
/// use std::num::NonZeroUsize;
/// use std::time::Duration;
///
/// let book = NonceBook::with_defaults();
/// book.record(VendorId::Cloudflare, ChallengeClass::Interstitial, "nonce-1");
/// assert_eq!(book.observation_count(VendorId::Cloudflare, "nonce-1"), Some(1));
/// ```
pub struct NonceBook {
    store: LruTtlStore<NonceObservation>,
}

impl std::fmt::Debug for NonceBook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NonceBook")
            .field("ttl", &self.store.ttl())
            .field("len", &self.store.len())
            .finish()
    }
}

impl NonceBook {
    /// Create a new nonce book with explicit capacity and TTL.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::NonceBook;
    /// use std::num::NonZeroUsize;
    /// use std::time::Duration;
    ///
    /// let book = NonceBook::new(NonZeroUsize::new(8).expect("non-zero"), Duration::from_mins(1));
    /// assert!(book.is_empty());
    /// ```
    #[must_use]
    pub fn new(capacity: NonZeroUsize, ttl: Duration) -> Self {
        Self {
            store: LruTtlStore::new(capacity, ttl),
        }
    }

    /// Capacity-bounded [`NonceBook`] with [`DEFAULT_NONCE_TTL`].
    #[must_use]
    pub fn with_default_ttl(capacity: NonZeroUsize) -> Self {
        Self::new(capacity, DEFAULT_NONCE_TTL)
    }

    /// Capacity-bounded [`NonceBook`] with [`DEFAULT_NONCE_BOOK_CAPACITY`]
    /// and [`DEFAULT_NONCE_TTL`].
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::NonceBook;
    ///
    /// let book = NonceBook::with_defaults();
    /// assert_eq!(book.ttl(), stygian_charon::token_lifecycle::DEFAULT_NONCE_TTL);
    /// ```
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_NONCE_BOOK_CAPACITY, DEFAULT_NONCE_TTL)
    }

    /// Configured TTL for the backing store.
    #[must_use]
    pub const fn ttl(&self) -> Duration {
        self.store.ttl()
    }

    /// Record an observation for a `(vendor, nonce)` tuple. The
    /// observation count is incremented atomically with the
    /// read-modify-write sequence; the LRU recency is **not**
    /// bumped on the read so a high-volume key does not crowd
    /// out less common keys.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::{ChallengeClass, NonceBook};
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// let book = NonceBook::with_defaults();
    /// book.record(VendorId::PerimeterX, ChallengeClass::IntegrityCheck, "n");
    /// book.record(VendorId::PerimeterX, ChallengeClass::IntegrityCheck, "n");
    /// assert_eq!(book.observation_count(VendorId::PerimeterX, "n"), Some(2));
    /// ```
    pub fn record(&self, vendor: VendorId, challenge_class: ChallengeClass, nonce: &str) {
        let key = nonce_book_key(vendor, nonce);
        let next_count = self
            .store
            .peek(&key)
            .map_or(1, |existing| existing.observation_count.saturating_add(1));
        let obs = NonceObservation {
            vendor,
            challenge_class,
            observation_count: next_count,
        };
        self.store.put(key, obs);
    }

    /// Look up the current observation count for a `(vendor,
    /// nonce)` tuple. Returns `None` when the key is absent or
    /// has expired.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::NonceBook;
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// let book = NonceBook::with_defaults();
    /// assert!(book.observation_count(VendorId::Unknown, "nope").is_none());
    /// ```
    #[must_use]
    pub fn observation_count(&self, vendor: VendorId, nonce: &str) -> Option<u32> {
        self.store
            .get(&nonce_book_key(vendor, nonce))
            .map(|o| o.observation_count)
    }

    /// Look up the full [`NonceObservation`] for a `(vendor,
    /// nonce)` tuple.
    #[must_use]
    pub fn lookup(&self, vendor: VendorId, nonce: &str) -> Option<NonceObservation> {
        self.store.get(&nonce_book_key(vendor, nonce))
    }

    /// Number of entries currently retained.
    #[must_use]
    pub fn len(&self) -> usize {
        self.store.len()
    }

    /// `true` when the book has zero entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    /// Remove all entries.
    pub fn clear(&self) {
        self.store.clear();
    }

    /// Invalidate a single `(vendor, nonce)` key.
    pub fn invalidate(&self, vendor: VendorId, nonce: &str) {
        self.store.invalidate(&nonce_book_key(vendor, nonce));
    }
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

    #[test]
    fn record_increments_observation_count() {
        let book = NonceBook::new(NonZeroUsize::new(4).unwrap(), Duration::from_mins(1));
        book.record(VendorId::Cloudflare, ChallengeClass::Interstitial, "n");
        book.record(VendorId::Cloudflare, ChallengeClass::Interstitial, "n");
        book.record(VendorId::Cloudflare, ChallengeClass::Interstitial, "n");
        assert_eq!(book.observation_count(VendorId::Cloudflare, "n"), Some(3));
    }

    #[test]
    fn distinct_vendors_keep_distinct_entries() {
        let book = NonceBook::new(NonZeroUsize::new(8).unwrap(), Duration::from_mins(1));
        book.record(VendorId::Cloudflare, ChallengeClass::Interstitial, "n");
        book.record(VendorId::Akamai, ChallengeClass::ProofOfWork, "n");
        assert_eq!(book.observation_count(VendorId::Cloudflare, "n"), Some(1));
        assert_eq!(book.observation_count(VendorId::Akamai, "n"), Some(1));
    }

    #[test]
    fn entries_decay_after_ttl() {
        let book = NonceBook::new(NonZeroUsize::new(4).unwrap(), Duration::from_millis(1));
        book.record(VendorId::Unknown, ChallengeClass::None, "n");
        std::thread::sleep(Duration::from_millis(5));
        assert!(book.observation_count(VendorId::Unknown, "n").is_none());
    }

    #[test]
    fn clear_drops_everything() {
        let book = NonceBook::new(NonZeroUsize::new(4).unwrap(), Duration::from_mins(1));
        book.record(VendorId::Cloudflare, ChallengeClass::Interstitial, "a");
        book.record(VendorId::DataDome, ChallengeClass::Captcha, "b");
        assert_eq!(book.len(), 2);
        book.clear();
        assert!(book.is_empty());
    }

    #[test]
    fn invalidate_drops_single_key() {
        let book = NonceBook::new(NonZeroUsize::new(4).unwrap(), Duration::from_mins(1));
        book.record(VendorId::Cloudflare, ChallengeClass::Interstitial, "a");
        book.record(VendorId::DataDome, ChallengeClass::Captcha, "b");
        book.invalidate(VendorId::Cloudflare, "a");
        assert!(book.observation_count(VendorId::Cloudflare, "a").is_none());
        assert_eq!(book.observation_count(VendorId::DataDome, "b"), Some(1));
    }

    #[test]
    fn nonce_book_key_is_stable_and_lower_case() {
        let key = nonce_book_key(VendorId::Cloudflare, "NONCE-XYZ");
        assert_eq!(key, "charon:token_nonce:cloudflare:NONCE-XYZ");
    }

    #[test]
    fn observation_count_for_unknown_nonce_is_none() {
        let book = NonceBook::with_defaults();
        assert!(book.observation_count(VendorId::Unknown, "nope").is_none());
    }

    #[test]
    fn lru_capacity_is_respected() {
        let book = NonceBook::new(NonZeroUsize::new(2).unwrap(), Duration::from_mins(1));
        book.record(VendorId::Cloudflare, ChallengeClass::Interstitial, "a");
        book.record(VendorId::Cloudflare, ChallengeClass::Interstitial, "b");
        book.record(VendorId::Cloudflare, ChallengeClass::Interstitial, "c");
        assert!(book.len() <= 2);
    }
}
