use std::num::NonZeroUsize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::cache::LruTtlStore;
use crate::challenge_feedback::{ChallengeOutcome, EngineKey};
use crate::types::TargetClass;

/// Default TTL for the challenge memory: **10 minutes**.
///
/// This is short enough that one-off escalations decay quickly (so a
/// single transient captcha does not poison the policy for hours)
/// and long enough to span a typical scraping session that might
/// retry the same domain several times before the operator decides
/// to back off entirely.
pub const DEFAULT_CHALLENGE_TTL: Duration = Duration::from_mins(10);

/// Default capacity (in [`EngineKey`] entries) for the challenge
/// memory. Conservative default — most workflows touch only a
/// handful of distinct (engine, `target_class`, `tls_profile`) keys.
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

/// Build a stable cache key for the challenge memory entry keyed
/// by [`EngineKey`].
///
/// The wire shape is
/// `charon:challenge:engine[/version][+tls_profile]:target_class`
/// so it round-trips with [`EngineKey::Display`] and never
/// collides with `charon:pow:...` (T93) or `charon:token_nonce:...`
/// (T91) entries on a shared backing primitive.
///
/// # Example
///
/// ```
/// use stygian_charon::challenge_feedback::{engine_memory_key, EngineKey};
/// use stygian_charon::types::TargetClass;
/// use stygian_charon::vendor_classifier::VendorId;
///
/// let key = EngineKey {
///     engine: VendorId::Cloudflare,
///     version: None,
///     target_class: TargetClass::Api,
///     tls_profile: None,
/// };
/// let wire = engine_memory_key(&key);
/// assert!(wire.starts_with("charon:challenge:cloudflare:api"));
/// ```
#[must_use]
pub fn engine_memory_key(key: &EngineKey) -> String {
    format!("charon:challenge:{key}")
}

/// One entry in the challenge memory.
///
/// An entry represents the **last observed** outcome for a single
/// [`EngineKey`], along with a count of how many times the runner
/// has recorded an outcome for that key (capped at `u32::MAX` for
/// monotonic counters) and the **last URL** the runner saw the
/// outcome on (kept as a secondary debugging index only — the
/// primary key is the engine, not the URL). The TTL is owned by
/// the [`LruTtlStore`] backing the [`ChallengeMemory`] — once the
/// LRU entry expires, the whole entry is dropped and the runner
/// falls back to the unadjusted risk score.
///
/// # Example
///
/// ```
/// use stygian_charon::challenge_feedback::{ChallengeMemoryEntry, ChallengeOutcome, EngineKey};
/// use stygian_charon::types::TargetClass;
/// use stygian_charon::vendor_classifier::VendorId;
///
/// let entry = ChallengeMemoryEntry {
///     key: EngineKey {
///         engine: VendorId::Cloudflare,
///         version: None,
///         target_class: TargetClass::ContentSite,
///         tls_profile: None,
///     },
///     last_observed_url: Some("https://example.com/path".to_string()),
///     last_outcome: ChallengeOutcome::HardChallenge,
///     observation_count: 1,
///     recorded_at_unix_secs: 1_700_000_000,
/// };
/// assert_eq!(entry.risk_delta(), ChallengeOutcome::HardChallenge.risk_delta());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChallengeMemoryEntry {
    /// Primary identity of this entry — the engine, target class,
    /// version, and TLS profile the entry was recorded under.
    pub key: EngineKey,
    /// Optional last URL the outcome was observed on. Kept for
    /// debugging ("where did we last see this engine?") — it is
    /// NOT a primary key. Two different URLs on the same engine
    /// share the same memory entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_observed_url: Option<String>,
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
    pub const fn risk_delta(&self) -> f64 {
        self.last_outcome.risk_delta()
    }

    /// Convenience accessor for the entry's target class.
    /// Mirrors [`EngineKey::target_class`] so callers do not have
    /// to reach through `entry.key.target_class`.
    #[must_use]
    pub const fn target_class(&self) -> TargetClass {
        self.key.target_class
    }

    /// Convenience accessor for the entry's engine. Mirrors
    /// [`EngineKey::engine`] so callers do not have to reach
    /// through `entry.key.engine`.
    #[must_use]
    pub const fn engine(&self) -> crate::vendor_classifier::VendorId {
        self.key.engine
    }
}

/// Capacity-bounded LRU+TTL store of [`ChallengeMemoryEntry`]s
/// keyed by [`EngineKey`].
///
/// The store reuses the same [`LruTtlStore`] primitive the
/// investigation cache and the `PoW` / token-nonce stores use (see
/// [`crate::cache::LruTtlStore`]). That keeps eviction + expiry
/// semantics consistent across every short-horizon store in the
/// crate and satisfies the "no new cache store" rule.
///
/// ## Why engine-keyed?
///
/// The primary key is the **engine** (Akamai, Cloudflare, `DataDome`,
/// …), **not** the URL. A self-healing patch recorded against one
/// URL on one engine heals every URL on that engine — a captcha
/// workaround learned on `example.com/cloudflare/page1` is
/// immediately applied to `example.com/cloudflare/page2` and to
/// every other Cloudflare-fronted URL the runner sees. The URL
/// is kept on each entry only as a secondary debugging index
/// (see [`ChallengeMemoryEntry::last_observed_url`]).
///
/// # Example
///
/// ```
/// use stygian_charon::challenge_feedback::{ChallengeMemory, ChallengeOutcome, EngineKey};
/// use stygian_charon::types::TargetClass;
/// use stygian_charon::vendor_classifier::VendorId;
/// use std::num::NonZeroUsize;
/// use std::time::Duration;
///
/// let memory =
///     ChallengeMemory::new(NonZeroUsize::new(8).expect("non-zero"), Duration::from_mins(5));
/// let key = EngineKey {
///     engine: VendorId::Cloudflare,
///     version: None,
///     target_class: TargetClass::ContentSite,
///     tls_profile: None,
/// };
/// memory.record(&key, Some("https://example.com/a"), ChallengeOutcome::Captcha);
/// let entry = memory.lookup(&key).expect("entry");
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
    /// [`DEFAULT_CHALLENGE_CAPACITY`] and [`DEFAULT_CHALLENGE_TTL`].
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

    /// Record a challenge outcome for an [`EngineKey`] and
    /// optionally the URL the outcome was observed on. The URL is
    /// stored only as a secondary debugging index — the primary
    /// key is the engine.
    ///
    /// The read-modify-write sequence (peek current observation
    /// count → build new entry → put) is **atomic** under
    /// concurrency: two simultaneous `record` calls always observe
    /// each other's prior increments. See
    /// [`LruTtlStore::mutate`][crate::cache::LruTtlStore] for the
    /// locking primitive.
    ///
    /// Expired entries start a fresh observation at count=1 (the
    /// underlying `mutate` evicts expired entries first).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::challenge_feedback::{ChallengeMemory, ChallengeOutcome, EngineKey};
    /// use stygian_charon::types::TargetClass;
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// let memory = ChallengeMemory::with_defaults();
    /// let key = EngineKey {
    ///     engine: VendorId::Cloudflare,
    ///     version: None,
    ///     target_class: TargetClass::Api,
    ///     tls_profile: None,
    /// };
    /// memory.record(&key, None, ChallengeOutcome::Pass);
    /// let entry = memory.lookup(&key).expect("entry");
    /// assert_eq!(entry.last_outcome, ChallengeOutcome::Pass);
    /// assert_eq!(entry.observation_count, 1);
    /// ```
    pub fn record(&self, key: &EngineKey, observed_url: Option<&str>, outcome: ChallengeOutcome) {
        let cache_key = engine_memory_key(key);
        let entry_key = key.clone();
        let observed_url_owned = observed_url.map(str::to_string);
        self.store.mutate(cache_key, |existing| {
            let next_count = existing.map_or(1, |prev| prev.observation_count.saturating_add(1));
            ChallengeMemoryEntry {
                key: entry_key,
                last_observed_url: observed_url_owned,
                last_outcome: outcome,
                observation_count: next_count,
                recorded_at_unix_secs: current_unix_secs(),
            }
        });
    }

    /// Look up the current entry for an [`EngineKey`]. Returns
    /// `None` if the key is absent or has expired.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::challenge_feedback::{ChallengeMemory, EngineKey};
    /// use stygian_charon::types::TargetClass;
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// let memory = ChallengeMemory::with_defaults();
    /// let key = EngineKey {
    ///     engine: VendorId::DataDome,
    ///     version: None,
    ///     target_class: TargetClass::HighSecurity,
    ///     tls_profile: None,
    /// };
    /// assert!(memory.lookup(&key).is_none());
    /// ```
    #[must_use]
    pub fn lookup(&self, key: &EngineKey) -> Option<ChallengeMemoryEntry> {
        self.store.get(&engine_memory_key(key))
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

    /// Invalidate a single [`EngineKey`].
    pub fn invalidate(&self, key: &EngineKey) {
        self.store.invalidate(&engine_memory_key(key));
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

    fn cf_api() -> EngineKey {
        EngineKey {
            engine: crate::vendor_classifier::VendorId::Cloudflare,
            version: None,
            target_class: TargetClass::Api,
            tls_profile: None,
        }
    }

    fn cf_content() -> EngineKey {
        EngineKey {
            target_class: TargetClass::ContentSite,
            ..cf_api()
        }
    }

    fn cf_api_tls_chrome136() -> EngineKey {
        EngineKey {
            tls_profile: Some("chrome136".to_string()),
            ..cf_api()
        }
    }

    #[test]
    fn record_overwrites_last_outcome_and_increments_count() {
        let memory = ChallengeMemory::new(NonZeroUsize::new(4).unwrap(), Duration::from_mins(1));
        let key = cf_content();

        memory.record(&key, None, ChallengeOutcome::Pass);
        memory.record(&key, None, ChallengeOutcome::HardChallenge);
        memory.record(&key, None, ChallengeOutcome::Captcha);

        let entry = memory.lookup(&key).expect("entry present");
        assert_eq!(entry.last_outcome, ChallengeOutcome::Captcha);
        assert_eq!(entry.observation_count, 3);
        assert_eq!(entry.key, key);
    }

    #[test]
    fn entries_decay_after_ttl() {
        let memory = ChallengeMemory::new(NonZeroUsize::new(4).unwrap(), Duration::from_millis(1));
        memory.record(&cf_api(), None, ChallengeOutcome::Blocked);
        thread::sleep(Duration::from_millis(5));
        assert!(memory.lookup(&cf_api()).is_none());
    }

    #[test]
    fn distinct_target_classes_keep_distinct_entries() {
        let memory = ChallengeMemory::new(NonZeroUsize::new(8).unwrap(), Duration::from_mins(1));

        memory.record(&cf_api(), None, ChallengeOutcome::Pass);
        memory.record(&cf_content(), None, ChallengeOutcome::Captcha);

        let api = memory.lookup(&cf_api()).unwrap();
        let content = memory.lookup(&cf_content()).unwrap();

        assert_eq!(api.last_outcome, ChallengeOutcome::Pass);
        assert_eq!(content.last_outcome, ChallengeOutcome::Captcha);
    }

    #[test]
    fn clear_drops_everything() {
        let memory = ChallengeMemory::new(NonZeroUsize::new(4).unwrap(), Duration::from_mins(1));
        memory.record(&cf_api(), None, ChallengeOutcome::Pass);
        let other = EngineKey {
            engine: crate::vendor_classifier::VendorId::Akamai,
            ..cf_api()
        };
        memory.record(&other, None, ChallengeOutcome::Blocked);
        assert_eq!(memory.len(), 2);
        memory.clear();
        assert!(memory.is_empty());
    }

    #[test]
    fn risk_delta_uses_last_outcome() {
        let memory = ChallengeMemory::new(NonZeroUsize::new(4).unwrap(), Duration::from_mins(1));
        memory.record(&cf_api(), None, ChallengeOutcome::HardChallenge);
        let entry = memory.lookup(&cf_api()).unwrap();
        assert!((entry.risk_delta() - ChallengeOutcome::HardChallenge.risk_delta()).abs() < 1e-9);
    }

    #[test]
    fn lru_capacity_is_respected() {
        let memory = ChallengeMemory::new(NonZeroUsize::new(2).unwrap(), Duration::from_mins(1));
        for (i, vendor) in [
            crate::vendor_classifier::VendorId::Akamai,
            crate::vendor_classifier::VendorId::Cloudflare,
            crate::vendor_classifier::VendorId::DataDome,
        ]
        .into_iter()
        .enumerate()
        {
            let key = EngineKey {
                engine: vendor,
                ..cf_api()
            };
            // Differentiate by target_class on the last iteration
            // to avoid the same-key overwrite collapsing entries.
            let key = if i == 2 {
                EngineKey {
                    target_class: TargetClass::HighSecurity,
                    ..key
                }
            } else {
                key
            };
            memory.record(&key, None, ChallengeOutcome::Pass);
        }
        assert!(memory.len() <= 2);
    }

    // ----------------------------------------------------------------
    // T110 guard tests
    // ----------------------------------------------------------------

    /// Guard test (T110): a self-healing patch recorded against
    /// URL A on engine E propagates to URL B on engine E. The URL
    /// is not the key; the engine is the key.
    #[test]
    fn same_engine_different_url_propagates_patch() {
        let memory = ChallengeMemory::new(NonZeroUsize::new(4).unwrap(), Duration::from_mins(1));
        let key = cf_api();

        memory.record(
            &key,
            Some("https://example.com/cloudflare/page1"),
            ChallengeOutcome::Captcha,
        );

        // Re-read with a *different* URL — same engine, same
        // target class, no TLS profile. The patch must propagate.
        let entry = memory.lookup(&key).expect("entry present");
        assert_eq!(
            entry.last_outcome,
            ChallengeOutcome::Captcha,
            "captcha recorded on URL A must heal the engine for URL B too"
        );
        assert_eq!(
            entry.observation_count, 1,
            "observation_count must reflect the single record (URLs are not keys)"
        );
        assert_eq!(
            entry.last_observed_url.as_deref(),
            Some("https://example.com/cloudflare/page1"),
            "last_observed_url records the most-recent URL we saw the engine on"
        );

        // A subsequent record with a different URL updates the
        // observed_url but keeps the same engine entry.
        memory.record(
            &key,
            Some("https://example.com/cloudflare/page2"),
            ChallengeOutcome::Pass,
        );
        let entry = memory.lookup(&key).expect("entry present");
        assert_eq!(entry.observation_count, 2);
        assert_eq!(
            entry.last_observed_url.as_deref(),
            Some("https://example.com/cloudflare/page2")
        );
    }

    /// Guard test (T110): two `EngineKey`s differing only by
    /// `target_class` keep separate memory. The `target_class`
    /// field participates in the key.
    #[test]
    fn same_engine_different_target_class_keeps_separate_memory() {
        let memory = ChallengeMemory::new(NonZeroUsize::new(4).unwrap(), Duration::from_mins(1));
        let api = cf_api();
        let content = cf_content();

        memory.record(&api, None, ChallengeOutcome::Pass);
        memory.record(&content, None, ChallengeOutcome::Captcha);

        assert_eq!(
            memory.lookup(&api).unwrap().last_outcome,
            ChallengeOutcome::Pass
        );
        assert_eq!(
            memory.lookup(&content).unwrap().last_outcome,
            ChallengeOutcome::Captcha
        );
        assert_ne!(
            memory.lookup(&api),
            memory.lookup(&content),
            "entries differing only by target_class must be distinct"
        );
    }

    /// Guard test (T110): two `EngineKey`s differing only by
    /// `tls_profile` keep separate memory. Encoding the wrong
    /// combination is structurally impossible.
    #[test]
    fn same_engine_different_tls_profile_keeps_separate_memory() {
        let memory = ChallengeMemory::new(NonZeroUsize::new(4).unwrap(), Duration::from_mins(1));
        let baseline = cf_api();
        let tls = cf_api_tls_chrome136();

        memory.record(&baseline, None, ChallengeOutcome::Pass);
        memory.record(&tls, None, ChallengeOutcome::HardChallenge);

        let base_entry = memory.lookup(&baseline).expect("baseline entry");
        let tls_entry = memory.lookup(&tls).expect("tls entry");
        assert_eq!(base_entry.last_outcome, ChallengeOutcome::Pass);
        assert_eq!(tls_entry.last_outcome, ChallengeOutcome::HardChallenge);
        assert_ne!(base_entry.key, tls_entry.key);
        assert_eq!(memory.len(), 2, "two distinct keys => two entries");
    }

    /// Guard test (T110): `EngineKey` round-trips through both
    /// `Display`/`FromStr` and `Serialize`/`Deserialize`, and
    /// the `engine_memory_key` wire format round-trips with
    /// `EngineKey::Display` for every supported field shape.
    #[test]
    fn engine_key_round_trips_through_display_fromstr_and_serde() {
        let samples = [
            EngineKey {
                engine: crate::vendor_classifier::VendorId::Cloudflare,
                version: None,
                target_class: TargetClass::Api,
                tls_profile: None,
            },
            EngineKey {
                engine: crate::vendor_classifier::VendorId::Akamai,
                version: Some("bot-manager-v3".to_string()),
                target_class: TargetClass::HighSecurity,
                tls_profile: None,
            },
            EngineKey {
                engine: crate::vendor_classifier::VendorId::DataDome,
                version: None,
                target_class: TargetClass::ContentSite,
                tls_profile: Some("firefox130".to_string()),
            },
            EngineKey {
                engine: crate::vendor_classifier::VendorId::PerimeterX,
                version: Some("human-v1".to_string()),
                target_class: TargetClass::HighSecurity,
                tls_profile: Some("chrome136".to_string()),
            },
        ];

        for key in &samples {
            let wire = engine_memory_key(key);
            // The wire form embeds EngineKey::Display.
            assert!(
                wire.starts_with("charon:challenge:"),
                "namespace prefix must be stable (got {wire})"
            );

            // Display <-> FromStr round-trip.
            let rendered = key.to_string();
            let parsed: EngineKey = rendered.parse().expect("FromStr round-trip");
            assert_eq!(&parsed, key, "Display <-> FromStr round-trip failed");

            // Serde <-> Serde round-trip via JSON.
            let json = serde_json::to_string(key).expect("Serialize");
            let de: EngineKey = serde_json::from_str(&json).expect("Deserialize");
            assert_eq!(&de, key, "JSON round-trip failed for {key:?}");
        }
    }
}
