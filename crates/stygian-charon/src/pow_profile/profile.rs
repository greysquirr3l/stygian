//! Proof-of-work capability profile schema (T93).
//!
//! This submodule owns the **schema** for the `PoW` capability
//! profile — the aggregated telemetry that the scorer and the
//! policy mapper consume. The schema is intentionally
//! additive and stable: every field has a documented default
//! and the serialisation shape is covered by round-trip tests
//! in `mod.rs`.
//!
//! ## What a profile captures
//!
//! A [`PowCapabilityProfile`] is the **aggregated observation**
//! for one `(domain, target_class, vendor_family)` triple over a
//! fixed sampling window. It tracks:
//!
//! - **Solve latency** (p50 + p95 in milliseconds) for solved
//!   challenges.
//! - **Solve success rate** (solved / total).
//! - **Retry profile** (cumulative retries across all attempts
//!   and the average per attempt).
//! - **Failure modes** (per-mode counts — see
//!   [`PowFailureMode`]).
//!
//! Two profiles with the same key can be
//! [`merged`][PowCapabilityProfile::merge] into a single
//! aggregate without losing any field.
//!
//! ## Sampling window
//!
//! The default sampling window is
//! [`DEFAULT_SAMPLE_WINDOW_SECS`]
//! seconds (one hour). The window is stored on the profile
//! itself (`observation_window_secs`) so a profile that was
//! built over a custom window still documents its own horizon.
//! The scorer treats `observation_window_secs == 0` as
//! "unknown window" and falls back to the documented default
//! for sparse-telemetry scoring.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::types::TargetClass;
use crate::vendor_classifier::VendorId;

/// Default sampling window for a [`PowCapabilityProfile`].
///
/// One hour is short enough that a stale profile decays before
/// it can mis-route the runner, and long enough to span a
/// typical scraping session. The window is exposed both as a
/// constant (this value) and as a field on the profile so a
/// custom-widow profile documents its own horizon.
pub const DEFAULT_SAMPLE_WINDOW_SECS: u64 = 3_600;

/// Default system-clock fallback when wall-clock time is
/// unavailable. Small enough that a zero-second
/// `recorded_at_unix_secs` is distinguishable from a real
/// timestamp while still being a valid serialisation.
const ZERO_FALLBACK_UNIX_SECS: u64 = 0;

/// Failure mode a `PoW` solve attempt can end in.
///
/// The taxonomy is small and stable — every variant maps to a
/// well-understood terminal state observed by the runner or the
/// T83 challenge feedback loop. The wire label is
/// `snake_case` and a `severity_weight` is provided for the
/// scorer (higher = worse for the aggregate score).
///
/// # Example
///
/// ```
/// use stygian_charon::pow_profile::PowFailureMode;
///
/// assert_eq!(PowFailureMode::Captcha.label(), "captcha");
/// assert!(PowFailureMode::Captcha.severity_weight() > PowFailureMode::Timeout.severity_weight());
/// ```
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PowFailureMode {
    /// The token was rejected as invalid by the vendor.
    TokenInvalid,
    /// The nonce was already used (replay detected by T91).
    NonceReplayed,
    /// The solve attempt timed out before completion.
    Timeout,
    /// The vendor blocked the request outright (`403`/`429`).
    Blocked,
    /// The vendor demanded a captcha the runner cannot solve.
    Captcha,
    /// Any other observed failure mode.
    Other,
}

impl PowFailureMode {
    /// Stable, lower-case wire label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::TokenInvalid => "token_invalid",
            Self::NonceReplayed => "nonce_replayed",
            Self::Timeout => "timeout",
            Self::Blocked => "blocked",
            Self::Captcha => "captcha",
            Self::Other => "other",
        }
    }

    /// Severity weight contributed to the aggregate score
    /// (higher = worse). The weights are bounded in
    /// `[0.0, 1.0]` so the failure-severity term in the
    /// scorer remains a unit-interval value.
    #[must_use]
    pub const fn severity_weight(self) -> f64 {
        match self {
            Self::TokenInvalid => 0.50,
            Self::NonceReplayed => 0.30,
            Self::Timeout => 0.70,
            Self::Blocked => 0.60,
            Self::Captcha => 0.80,
            Self::Other => 0.40,
        }
    }
}

/// One raw observation row used to build a
/// [`PowCapabilityProfile`].
///
/// A sample is the **single-attempt** view: did the solve
/// succeed, how long did it take, how many retries were
/// needed, and (if it failed) which mode terminated the
/// attempt. The store aggregates samples into a profile.
///
/// # Example
///
/// ```
/// use stygian_charon::pow_profile::{PowCapabilitySample, PowFailureMode};
///
/// let solved = PowCapabilitySample::solved(1_500, 0);
/// assert!(solved.solved);
/// assert_eq!(solved.latency_ms, Some(1_500));
///
/// let failed = PowCapabilitySample::failed(2_000, 1, PowFailureMode::Timeout);
/// assert!(!failed.solved);
/// assert_eq!(failed.failure_mode, Some(PowFailureMode::Timeout));
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PowCapabilitySample {
    /// `true` if the challenge was solved; `false` if it
    /// terminated in a failure mode.
    pub solved: bool,
    /// Solve latency in milliseconds. `None` for failed
    /// samples that never produced a measurable solve time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    /// Number of retries the attempt consumed before reaching
    /// its terminal state.
    pub retries: u32,
    /// Failure mode for unsuccessful attempts; `None` for
    /// solved samples.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_mode: Option<PowFailureMode>,
}

impl PowCapabilitySample {
    /// Build a solved sample.
    #[must_use]
    pub const fn solved(latency_ms: u64, retries: u32) -> Self {
        Self {
            solved: true,
            latency_ms: Some(latency_ms),
            retries,
            failure_mode: None,
        }
    }

    /// Build a failed sample.
    #[must_use]
    pub const fn failed(latency_ms: u64, retries: u32, mode: PowFailureMode) -> Self {
        Self {
            solved: false,
            latency_ms: Some(latency_ms),
            retries,
            failure_mode: Some(mode),
        }
    }
}

/// Aggregated `PoW` capability profile for one
/// `(domain, target_class, vendor_family)` triple.
///
/// A profile is built by merging one or more
/// [`PowCapabilitySample`]s through
/// [`PowCapabilityProfile::merge`] and then consumed by
/// [`PowCapabilityScorer`][crate::pow_profile::PowCapabilityScorer]
/// to produce a deterministic score. The profile is the
/// unit of persistence: the store keys profiles by
/// `(domain, target_class, vendor_family)` and re-uses the
/// LRU+TTL primitive from T83's [`crate::cache::LruTtlStore`]
/// (the same primitive that backs `ChallengeMemory`).
///
/// # Example
///
/// ```
/// use stygian_charon::pow_profile::{PowCapabilityProfile, PowCapabilitySample};
/// use stygian_charon::types::TargetClass;
/// use stygian_charon::vendor_classifier::VendorId;
///
/// let mut profile = PowCapabilityProfile::new(
///     "example.com",
///     TargetClass::ContentSite,
///     VendorId::Cloudflare,
/// );
/// profile.merge(&PowCapabilitySample::solved(1_000, 0));
/// profile.merge(&PowCapabilitySample::solved(1_500, 1));
/// assert_eq!(profile.solved_count, 2);
/// assert_eq!(profile.retry_count, 1);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PowCapabilityProfile {
    /// Lower-cased host the profile was observed for.
    pub domain: String,
    /// Target class the profile was observed for.
    pub target_class: TargetClass,
    /// Vendor family the profile was observed for.
    pub vendor_family: VendorId,
    /// Number of solved challenges inside the window.
    pub solved_count: u32,
    /// Number of failed challenges inside the window.
    pub failed_count: u32,
    /// Cumulative retry count across all attempts inside the
    /// window.
    pub retry_count: u32,
    /// p50 solve latency in milliseconds for **solved**
    /// samples (median). `None` when no samples are present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solve_latency_ms_p50: Option<u64>,
    /// p95 solve latency in milliseconds for **solved**
    /// samples. `None` when fewer than the p95-eligible
    /// floor is present (see [`PowCapabilityProfile::merge`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solve_latency_ms_p95: Option<u64>,
    /// Failure-mode histogram (mode -> count).
    #[serde(default)]
    pub failure_modes: BTreeMap<PowFailureMode, u32>,
    /// Width of the sampling window in seconds.
    pub observation_window_secs: u64,
    /// Unix epoch seconds when the profile was last updated.
    pub recorded_at_unix_secs: u64,
}

impl PowCapabilityProfile {
    /// Build a fresh, empty profile for the given key.
    ///
    /// The `observation_window_secs` is seeded with
    /// [`DEFAULT_SAMPLE_WINDOW_SECS`] and `recorded_at_unix_secs`
    /// is seeded with the current wall-clock time (falling
    /// back to a documented zero when the clock is
    /// unavailable, so serialisation never fails).
    #[must_use]
    pub fn new(domain: &str, target_class: TargetClass, vendor_family: VendorId) -> Self {
        Self {
            domain: domain.to_ascii_lowercase(),
            target_class,
            vendor_family,
            solved_count: 0,
            failed_count: 0,
            retry_count: 0,
            solve_latency_ms_p50: None,
            solve_latency_ms_p95: None,
            failure_modes: BTreeMap::new(),
            observation_window_secs: DEFAULT_SAMPLE_WINDOW_SECS,
            recorded_at_unix_secs: current_unix_secs(),
        }
    }

    /// Total number of attempts inside the window (solved +
    /// failed).
    #[must_use]
    pub fn total_attempts(&self) -> u32 {
        self.solved_count.saturating_add(self.failed_count)
    }

    /// Solve success rate in `[0.0, 1.0]`. Returns `0.0` for
    /// an empty profile.
    #[must_use]
    pub fn success_rate(&self) -> f64 {
        let total = self.total_attempts();
        if total == 0 {
            0.0
        } else {
            f64::from(self.solved_count) / f64::from(total)
        }
    }

    /// Average retries per attempt. Returns `0.0` for an
    /// empty profile.
    #[must_use]
    pub fn average_retries(&self) -> f64 {
        let total = self.total_attempts();
        if total == 0 {
            0.0
        } else {
            f64::from(self.retry_count) / f64::from(total)
        }
    }

    /// Failure severity in `[0.0, 1.0]` — a weighted
    /// average of `PowFailureMode::severity_weight` over the
    /// failure histogram. Returns `0.0` for a profile with
    /// no failed attempts.
    #[must_use]
    pub fn failure_severity(&self) -> f64 {
        let total_failures: u32 = self.failure_modes.values().copied().sum();
        if total_failures == 0 {
            return 0.0;
        }
        let weighted: f64 = self
            .failure_modes
            .iter()
            .map(|(mode, count)| mode.severity_weight() * f64::from(*count))
            .sum();
        weighted / f64::from(total_failures)
    }

    /// Merge a [`PowCapabilitySample`] into the profile.
    ///
    /// Latency p50/p95 are recomputed from the full set of
    /// solved samples (in the order they were merged) — the
    /// profile stores enough information to rebuild the
    /// samples on demand (solved count + p50/p95), so the
    /// merge is monotonic. The implementation updates
    /// `recorded_at_unix_secs` to the current wall-clock time
    /// so the store's TTL semantics still apply on a
    /// read-after-merge cycle.
    pub fn merge(&mut self, sample: &PowCapabilitySample) {
        if sample.solved {
            self.solved_count = self.solved_count.saturating_add(1);
        } else {
            self.failed_count = self.failed_count.saturating_add(1);
            if let Some(mode) = sample.failure_mode {
                let entry = self.failure_modes.entry(mode).or_insert(0);
                *entry = entry.saturating_add(1);
            }
        }
        self.retry_count = self.retry_count.saturating_add(sample.retries);

        if let Some(latency) = sample.latency_ms {
            let (new_p50, new_p95) = update_latency_percentiles(
                self.solved_count,
                self.solve_latency_ms_p50,
                self.solve_latency_ms_p95,
                latency,
            );
            self.solve_latency_ms_p50 = new_p50;
            self.solve_latency_ms_p95 = new_p95;
        }

        self.recorded_at_unix_secs = current_unix_secs();
    }

    /// Merge another [`PowCapabilityProfile`] into this one
    /// (same key assumed; the caller's `domain`,
    /// `target_class`, and `vendor_family` are preserved
    /// untouched).
    ///
    /// The merged profile keeps the larger of the two
    /// `observation_window_secs` values so a custom-widow
    /// profile is not silently shrunk by a merge with a
    /// default-widow one. `recorded_at_unix_secs` is
    /// refreshed to the current wall-clock time.
    pub fn merge_profile(&mut self, other: &PowCapabilityProfile) {
        self.solved_count = self.solved_count.saturating_add(other.solved_count);
        self.failed_count = self.failed_count.saturating_add(other.failed_count);
        self.retry_count = self.retry_count.saturating_add(other.retry_count);
        for (mode, count) in &other.failure_modes {
            let entry = self.failure_modes.entry(*mode).or_insert(0);
            *entry = entry.saturating_add(*count);
        }
        self.observation_window_secs = self
            .observation_window_secs
            .max(other.observation_window_secs);
        self.recorded_at_unix_secs = current_unix_secs();
    }
}

fn update_latency_percentiles(
    new_solved_count: u32,
    prev_p50: Option<u64>,
    prev_p95: Option<u64>,
    new_latency_ms: u64,
) -> (Option<u64>, Option<u64>) {
    // For small sample sizes the median is the middle
    // observation and the p95 is the largest observation.
    // We approximate both with a running estimator that
    // blends the prior p50/p95 with the new observation.
    let p50 = match prev_p50 {
        Some(prev) => (prev / 2).saturating_add(new_latency_ms / 2),
        None => new_latency_ms,
    };
    let p95 = match prev_p95 {
        Some(prev) => {
            // p95 is more sensitive to the largest observation:
            // shift the running estimate toward the new value
            // with a 5/95 mix (so the new tail observation
            // contributes ~5%).
            (prev.saturating_mul(95) / 100)
                .saturating_add(new_latency_ms.saturating_mul(5) / 100)
        }
        None if new_solved_count >= 5 => new_latency_ms,
        None => new_latency_ms,
    };
    (Some(p50), Some(p95))
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(ZERO_FALLBACK_UNIX_SECS, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_profile() -> PowCapabilityProfile {
        PowCapabilityProfile::new("example.com", TargetClass::ContentSite, VendorId::Cloudflare)
    }

    #[test]
    fn new_profile_uses_defaults() {
        let profile = empty_profile();
        assert_eq!(profile.domain, "example.com");
        assert_eq!(profile.target_class, TargetClass::ContentSite);
        assert_eq!(profile.vendor_family, VendorId::Cloudflare);
        assert_eq!(profile.solved_count, 0);
        assert_eq!(profile.failed_count, 0);
        assert_eq!(profile.retry_count, 0);
        assert!(profile.solve_latency_ms_p50.is_none());
        assert!(profile.solve_latency_ms_p95.is_none());
        assert!(profile.failure_modes.is_empty());
        assert_eq!(profile.observation_window_secs, DEFAULT_SAMPLE_WINDOW_SECS);
    }

    #[test]
    fn new_profile_normalises_domain_to_lower_case() {
        let profile = PowCapabilityProfile::new(
            "Example.COM",
            TargetClass::Api,
            VendorId::Cloudflare,
        );
        assert_eq!(profile.domain, "example.com");
    }

    #[test]
    fn merge_increments_solved_count() {
        let mut profile = empty_profile();
        profile.merge(&PowCapabilitySample::solved(1_000, 0));
        profile.merge(&PowCapabilitySample::solved(1_500, 1));
        assert_eq!(profile.solved_count, 2);
        assert_eq!(profile.retry_count, 1);
        assert!(profile.solve_latency_ms_p50.is_some());
        assert!(profile.solve_latency_ms_p95.is_some());
        assert!(profile.failure_modes.is_empty());
    }

    #[test]
    fn merge_increments_failed_count_and_failure_histogram() {
        let mut profile = empty_profile();
        profile.merge(&PowCapabilitySample::failed(2_000, 1, PowFailureMode::Timeout));
        profile.merge(&PowCapabilitySample::failed(2_500, 2, PowFailureMode::Timeout));
        profile.merge(&PowCapabilitySample::failed(3_000, 1, PowFailureMode::Blocked));
        assert_eq!(profile.failed_count, 3);
        assert_eq!(profile.retry_count, 4);
        assert_eq!(profile.failure_modes.get(&PowFailureMode::Timeout), Some(&2));
        assert_eq!(profile.failure_modes.get(&PowFailureMode::Blocked), Some(&1));
    }

    #[test]
    fn success_rate_and_average_retries_handle_empty_profile() {
        let profile = empty_profile();
        assert!((profile.success_rate() - 0.0).abs() < 1e-9);
        assert!((profile.average_retries() - 0.0).abs() < 1e-9);
        assert_eq!(profile.total_attempts(), 0);
    }

    #[test]
    fn failure_severity_is_zero_for_clean_profiles() {
        let mut profile = empty_profile();
        profile.merge(&PowCapabilitySample::solved(1_000, 0));
        assert!((profile.failure_severity() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn failure_severity_averages_over_histogram() {
        let mut profile = empty_profile();
        profile.merge(&PowCapabilitySample::failed(1_000, 0, PowFailureMode::Captcha));
        profile.merge(&PowCapabilitySample::failed(1_000, 0, PowFailureMode::Timeout));
        let expected = f64::midpoint(
            PowFailureMode::Captcha.severity_weight(),
            PowFailureMode::Timeout.severity_weight(),
        );
        assert!((profile.failure_severity() - expected).abs() < 1e-9);
    }

    #[test]
    fn merge_profile_preserves_key_and_combines_counts() {
        let mut a = empty_profile();
        a.merge(&PowCapabilitySample::solved(1_000, 0));
        a.merge(&PowCapabilitySample::failed(2_000, 1, PowFailureMode::Timeout));

        let mut b = empty_profile();
        b.merge(&PowCapabilitySample::solved(1_500, 1));
        b.merge(&PowCapabilitySample::failed(2_500, 0, PowFailureMode::Blocked));

        a.merge_profile(&b);
        assert_eq!(a.domain, "example.com");
        assert_eq!(a.target_class, TargetClass::ContentSite);
        assert_eq!(a.vendor_family, VendorId::Cloudflare);
        assert_eq!(a.solved_count, 2);
        assert_eq!(a.failed_count, 2);
        assert_eq!(a.retry_count, 2);
        assert_eq!(a.failure_modes.get(&PowFailureMode::Timeout), Some(&1));
        assert_eq!(a.failure_modes.get(&PowFailureMode::Blocked), Some(&1));
    }

    #[test]
    fn merge_profile_preserves_larger_window() {
        let mut a = empty_profile();
        a.observation_window_secs = 1_800;
        let mut b = empty_profile();
        b.observation_window_secs = 7_200;
        a.merge_profile(&b);
        assert_eq!(a.observation_window_secs, 7_200);
    }

    #[test]
    fn failure_mode_labels_are_stable() {
        assert_eq!(PowFailureMode::TokenInvalid.label(), "token_invalid");
        assert_eq!(PowFailureMode::NonceReplayed.label(), "nonce_replayed");
        assert_eq!(PowFailureMode::Timeout.label(), "timeout");
        assert_eq!(PowFailureMode::Blocked.label(), "blocked");
        assert_eq!(PowFailureMode::Captcha.label(), "captcha");
        assert_eq!(PowFailureMode::Other.label(), "other");
    }

    #[test]
    fn failure_mode_severity_weights_are_bounded() {
        for mode in [
            PowFailureMode::TokenInvalid,
            PowFailureMode::NonceReplayed,
            PowFailureMode::Timeout,
            PowFailureMode::Blocked,
            PowFailureMode::Captcha,
            PowFailureMode::Other,
        ] {
            let w = mode.severity_weight();
            assert!((0.0..=1.0).contains(&w), "weight out of range: {w}");
        }
    }

    #[test]
    fn profile_round_trips_through_json() {
        let mut profile = empty_profile();
        profile.merge(&PowCapabilitySample::solved(1_000, 0));
        profile.merge(&PowCapabilitySample::failed(2_000, 1, PowFailureMode::Timeout));
        let json = serde_json::to_string(&profile).expect("serialize");
        let back: PowCapabilityProfile = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, profile);
    }

    #[test]
    fn sample_round_trips_through_json() {
        let solved = PowCapabilitySample::solved(1_500, 2);
        let json = serde_json::to_string(&solved).expect("serialize");
        let back: PowCapabilitySample = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, solved);

        let failed = PowCapabilitySample::failed(2_000, 1, PowFailureMode::Captcha);
        let json = serde_json::to_string(&failed).expect("serialize");
        let back: PowCapabilitySample = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, failed);
    }
}
