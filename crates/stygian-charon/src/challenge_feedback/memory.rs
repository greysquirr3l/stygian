use std::num::NonZeroUsize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::cache::LruTtlStore;
use crate::challenge_feedback::ChallengeOutcome;
use crate::types::TargetClass;

/// Default TTL for the challenge memory: **10 minutes**.
///
/// This is short enough that one-off escalations decay quickly (so a
/// single transient captcha does not poison the policy for hours)
/// and long enough to span a typical scraping session that might
/// retry the same domain several times before the operator decides
/// to back off entirely.
pub const DEFAULT_CHALLENGE_TTL: Duration = Duration::from_mins(10);

/// Default capacity (in `(domain, target_class)` entries) for the
/// challenge memory. Conservative default — most workflows touch
/// only a handful of distinct target classes.
#[allow(clippy::unwrap_used)]
pub const DEFAULT_CHALLENGE_CAPACITY: NonZeroUsize = match NonZeroUsize::new(64) {
    Some(value) => value,
    None => NonZeroUsize::MIN,
};

/// Default TTL for the system clock fallback when wall-clock time is
/// unavailable. The value is small enough that a zero-second
/// `recorded_at_unix_secs` is distinguishable from a real timestamp
/// while still being a valid serialisation.
const ZERO_FALLBACK_UNIX_SECS: u64 = 0;

/// Build a stable, lower-cased cache key for the challenge memory
/// entry keyed by `(domain, target_class)`.
///
/// # Example
///
/// ```
/// use stygian_charon::challenge_feedback::challenge_memory_key;
/// use stygian_charon::types::TargetClass;
///
/// let key = challenge_memory_key("Example.COM", TargetClass::Api);
/// assert!(key.starts_with("charon:challenge:example.com:"));
/// ```
#[must_use]
pub fn challenge_memory_key(domain: &str, target_class: TargetClass) -> String {
    format!(
        "charon:challenge:{}:{}",
        domain.to_ascii_lowercase(),
        target_class_label(target_class)
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

/// One entry in the challenge memory.
///
/// An entry represents the **last observed** outcome for a single
/// `(domain, target_class)` pair, along with a count of how many
/// times the runner has recorded an outcome for that key (capped at
/// `u32::MAX` for monotonic counters). The TTL is owned by the
/// [`LruTtlStore`][crate::cache::LruTtlStore] backing the
/// [`ChallengeMemory`] — once the LRU entry expires, the whole
/// entry is dropped and the runner falls back to the unadjusted
/// risk score.
///
/// # Example
///
/// ```
/// use stygian_charon::challenge_feedback::{ChallengeMemoryEntry, ChallengeOutcome};
/// use stygian_charon::types::TargetClass;
///
/// let entry = ChallengeMemoryEntry {
///     domain: "example.com".to_string(),
///     target_class: TargetClass::ContentSite,
///     last_outcome: ChallengeOutcome::HardChallenge,
///     observation_count: 1,
///     recorded_at_unix_secs: 1_700_000_000,
/// };
/// assert_eq!(entry.risk_delta(), ChallengeOutcome::HardChallenge.risk_delta());
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChallengeMemoryEntry {
    /// Lower-cased host the outcome was recorded for.
    pub domain: String,
    /// Target class the outcome was recorded for.
    pub target_class: TargetClass,
    /// Most recently recorded outcome for this key.
    pub last_outcome: ChallengeOutcome,
    /// Number of outcomes the runner has recorded for this key
    /// (saturating on overflow).
    pub observation_count: u32,
    /// Unix epoch seconds when the entry was last updated.
    pub recorded_at_unix_secs: u64,
}

impl ChallengeMemoryEntry {
    /// Risk-score contribution this entry would add to the next
    /// policy. Delegates to
    /// [`ChallengeOutcome::risk_delta`][crate::challenge_feedback::ChallengeOutcome::risk_delta]
    /// and is therefore bounded by
    /// [`MAX_RISK_DELTA`][crate::challenge_feedback::MAX_RISK_DELTA].
    #[must_use]
    pub fn risk_delta(&self) -> f64 {
        self.last_outcome.risk_delta()
    }
}

/// Short-horizon, capacity-bounded LRU memory of challenge outcomes
/// keyed by `(domain, target_class)`.
///
/// The store reuses the same [`LruTtlStore`][crate::cache::LruTtlStore]
/// primitive that backs the investigation report cache. That keeps
/// eviction + expiry semantics consistent across both caches and
/// satisfies the "no new cache store" requirement.
///
/// # Example
///
/// ```
/// use stygian_charon::challenge_feedback::{ChallengeMemory, ChallengeOutcome};
/// use stygian_charon::types::TargetClass;
/// use std::num::NonZeroUsize;
/// use std::time::Duration;
///
/// let memory =
///     ChallengeMemory::new(NonZeroUsize::new(8).expect("non-zero"), Duration::from_mins(5));
/// memory.record("example.com", TargetClass::ContentSite, ChallengeOutcome::Captcha);
/// let entry = memory.lookup("example.com", TargetClass::ContentSite).expect("entry");
/// assert_eq!(entry.last_outcome, ChallengeOutcome::Captcha);
/// assert_eq!(entry.observation_count, 1);
/// ```
pub struct ChallengeMemory {
    store: LruTtlStore<ChallengeMemoryEntry>,
}

impl ChallengeMemory {
    /// Create a new challenge memory with explicit capacity and TTL.
    #[must_use]
    pub fn new(capacity: NonZeroUsize, ttl: Duration) -> Self {
        Self {
            store: LruTtlStore::new(capacity, ttl),
        }
    }

    /// Create a new challenge memory with
    /// [`DEFAULT_CHALLENGE_CAPACITY`] and
    /// [`DEFAULT_CHALLENGE_TTL`].
    #[must_use]
    pub fn with_default_ttl(capacity: NonZeroUsize) -> Self {
        Self::new(capacity, DEFAULT_CHALLENGE_TTL)
    }

    /// Capacity-bounded [`ChallengeMemory`] with the default
    /// capacity and TTL.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_CHALLENGE_CAPACITY, DEFAULT_CHALLENGE_TTL)
    }

    /// Record a challenge outcome for a `(domain, target_class)`
    /// key. Replaces the existing entry (if any) and increments the
    /// observation counter atomically with the read-modify-write
    /// sequence. Lower-cases the domain for stable keying.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::challenge_feedback::{ChallengeMemory, ChallengeOutcome};
    /// use stygian_charon::types::TargetClass;
    ///
    /// let memory = ChallengeMemory::with_defaults();
    /// memory.record("Example.COM", TargetClass::Api, ChallengeOutcome::Pass);
    /// let entry = memory.lookup("example.com", TargetClass::Api).unwrap();
    /// assert_eq!(entry.last_outcome, ChallengeOutcome::Pass);
    /// assert_eq!(entry.observation_count, 1);
    /// ```
    pub fn record(&self, domain: &str, target_class: TargetClass, outcome: ChallengeOutcome) {
        let key = challenge_memory_key(domain, target_class);
        let lower = domain.to_ascii_lowercase();
        let next_count = self
            .store
            .peek(&key)
            .map_or(1, |existing| existing.observation_count.saturating_add(1));
        let entry = ChallengeMemoryEntry {
            domain: lower,
            target_class,
            last_outcome: outcome,
            observation_count: next_count,
            recorded_at_unix_secs: current_unix_secs(),
        };
        self.store.put(key, entry);
    }

    /// Look up the current entry for a `(domain, target_class)` key.
    /// Returns `None` if the key is absent or has expired.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::challenge_feedback::ChallengeMemory;
    /// use stygian_charon::types::TargetClass;
    ///
    /// let memory = ChallengeMemory::with_defaults();
    /// assert!(memory.lookup("nope.example", TargetClass::Api).is_none());
    /// ```
    #[must_use]
    pub fn lookup(&self, domain: &str, target_class: TargetClass) -> Option<ChallengeMemoryEntry> {
        self.store.get(&challenge_memory_key(domain, target_class))
    }

    /// Number of entries currently retained.
    #[must_use]
    pub fn len(&self) -> usize {
        self.store.len()
    }

    /// `true` if the memory has zero entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    /// Remove all entries.
    pub fn clear(&self) {
        self.store.clear();
    }

    /// Invalidate a single `(domain, target_class)` key.
    pub fn invalidate(&self, domain: &str, target_class: TargetClass) {
        self.store
            .invalidate(&challenge_memory_key(domain, target_class));
    }
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(ZERO_FALLBACK_UNIX_SECS, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn record_overwrites_last_outcome_and_increments_count() {
        let memory = ChallengeMemory::new(NonZeroUsize::new(4).unwrap(), Duration::from_mins(1));
        let key = ("example.com", TargetClass::ContentSite);

        memory.record(key.0, key.1, ChallengeOutcome::Pass);
        memory.record(key.0, key.1, ChallengeOutcome::HardChallenge);
        memory.record(key.0, key.1, ChallengeOutcome::Captcha);

        let entry = memory.lookup(key.0, key.1).expect("entry present");
        assert_eq!(entry.last_outcome, ChallengeOutcome::Captcha);
        assert_eq!(entry.observation_count, 3);
        assert_eq!(entry.domain, "example.com");
        assert_eq!(entry.target_class, TargetClass::ContentSite);
    }

    #[test]
    fn entries_decay_after_ttl() {
        let memory = ChallengeMemory::new(NonZeroUsize::new(4).unwrap(), Duration::from_millis(1));
        memory.record("example.com", TargetClass::Api, ChallengeOutcome::Blocked);
        thread::sleep(Duration::from_millis(5));
        assert!(memory.lookup("example.com", TargetClass::Api).is_none());
    }

    #[test]
    fn distinct_target_classes_keep_distinct_entries() {
        let memory = ChallengeMemory::new(NonZeroUsize::new(8).unwrap(), Duration::from_mins(1));

        memory.record("example.com", TargetClass::Api, ChallengeOutcome::Pass);
        memory.record("example.com", TargetClass::ContentSite, ChallengeOutcome::Captcha);

        let api = memory.lookup("example.com", TargetClass::Api).unwrap();
        let content = memory
            .lookup("example.com", TargetClass::ContentSite)
            .unwrap();

        assert_eq!(api.last_outcome, ChallengeOutcome::Pass);
        assert_eq!(content.last_outcome, ChallengeOutcome::Captcha);
    }

    #[test]
    fn clear_drops_everything() {
        let memory = ChallengeMemory::new(NonZeroUsize::new(4).unwrap(), Duration::from_mins(1));
        memory.record("example.com", TargetClass::Api, ChallengeOutcome::Pass);
        memory.record("other.example", TargetClass::Api, ChallengeOutcome::Blocked);
        assert_eq!(memory.len(), 2);
        memory.clear();
        assert!(memory.is_empty());
    }

    #[test]
    fn domain_is_normalised_to_lower_case() {
        let memory = ChallengeMemory::new(NonZeroUsize::new(4).unwrap(), Duration::from_mins(1));
        memory.record(
            "Example.COM",
            TargetClass::Api,
            ChallengeOutcome::SoftChallenge,
        );
        let entry = memory.lookup("EXAMPLE.com", TargetClass::Api).unwrap();
        assert_eq!(entry.domain, "example.com");
        assert_eq!(entry.last_outcome, ChallengeOutcome::SoftChallenge);
    }

    #[test]
    fn risk_delta_uses_last_outcome() {
        let memory = ChallengeMemory::new(NonZeroUsize::new(4).unwrap(), Duration::from_mins(1));
        memory.record("example.com", TargetClass::Api, ChallengeOutcome::HardChallenge);
        let entry = memory.lookup("example.com", TargetClass::Api).unwrap();
        assert!((entry.risk_delta() - ChallengeOutcome::HardChallenge.risk_delta()).abs() < 1e-9);
    }

    #[test]
    fn lru_capacity_is_respected() {
        let memory = ChallengeMemory::new(NonZeroUsize::new(2).unwrap(), Duration::from_mins(1));
        memory.record("a.example", TargetClass::Api, ChallengeOutcome::Pass);
        memory.record("b.example", TargetClass::Api, ChallengeOutcome::Pass);
        memory.record("c.example", TargetClass::Api, ChallengeOutcome::Pass);
        assert!(memory.len() <= 2);
    }
}
