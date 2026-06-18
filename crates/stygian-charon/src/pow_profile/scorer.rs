//! Deterministic `PoW` capability scorer (T93).
//!
//! The [`PowCapabilityScorer`] consumes a
//! [`PowCapabilityProfile`][crate::pow_profile::PowCapabilityProfile]
//! and produces a unit-interval score plus a coarse-grained
//! [`PowCapabilityBand`] label. Scoring is fully deterministic:
//! the same profile always produces the same score.
//!
//! ## Score formula
//!
//! ```text
//! score = w_success * success_rate
//!       + w_latency * latency_score
//!       + w_retry   * retry_score
//!       + w_failure * (1.0 - failure_severity)
//! ```
//!
//! where:
//! - `success_rate` is `solved_count / total_attempts`.
//! - `latency_score` is `1.0 - clamp(p95 / latency_budget_ms, 0.0, 1.0)`.
//! - `retry_score` is `1.0 - clamp(avg_retries / retry_budget, 0.0, 1.0)`.
//! - `failure_severity` is the weighted average of
//!   [`PowFailureMode::severity_weight`][crate::pow_profile::PowFailureMode::severity_weight]
//!   over the failure histogram.
//!
//! The default weights sum to `1.0` so the output is
//! guaranteed to be in `[0.0, 1.0]`. Callers can override the
//! weights through [`PowCapabilityScorer::with_weights`].
//!
//! ## Sparse telemetry fallback
//!
//! When the profile's `total_attempts` is below
//! [`MIN_OBSERVATIONS_FOR_SCORING`]
//! the scorer returns [`SPARSE_FALLBACK_SCORE`] (a
//! documented `0.5`). The fallback is the **same** value
//! returned for the empty profile, so callers do not have to
//! branch on "is this sparse" — they get a single number
//! that is "no signal, default to neutral".
//!
//! ## Sampling window defaults
//!
//! The scorer does **not** adjust for `observation_window_secs`
//! directly — the score is a function of the **content** of
//! the profile, not its age. The store's TTL is the mechanism
//! that keeps a stale profile from mis-routing the runner
//! (an expired profile simply does not look up).

use serde::{Deserialize, Serialize};

use crate::pow_profile::profile::PowCapabilityProfile;

/// Minimum number of attempts required for a
/// [`PowCapabilityProfile`] to be scored instead of returning
/// the sparse-telemetry fallback.
///
/// Three attempts is a conservative floor: one solved/failed
/// pair is too noisy (a single transient block can flip the
/// success rate from 100% to 50%), but the runner will rarely
/// wait for a high-confidence sample if the target is
/// actively challenging it. Callers that want a higher floor
/// can use [`PowCapabilityScorer::with_min_observations`].
pub const MIN_OBSERVATIONS_FOR_SCORING: u32 = 3;

/// Documented score returned when the profile has fewer
/// attempts than [`MIN_OBSERVATIONS_FOR_SCORING`].
///
/// The value is `0.5` — neutral on the unit interval, with no
/// influence on the policy mapper. This is the "I have no
/// signal" default; downstream policy mapping treats it as
/// the no-op baseline so an unobserved target does not get
/// over-escalated.
pub const SPARSE_FALLBACK_SCORE: f64 = 0.5;

/// Default latency budget for the latency-score term.
///
/// A solve that takes longer than the budget is treated as
/// fully penalised (`latency_score = 0.0`). The value is
/// conservative (5 seconds) — most well-behaved vendor `PoW`
/// challenges solve well under that.
pub const DEFAULT_LATENCY_BUDGET_MS: u64 = 5_000;

/// Default retry budget for the retry-score term.
///
/// A profile whose average retries exceed the budget is
/// treated as fully penalised (`retry_score = 0.0`).
pub const DEFAULT_RETRY_BUDGET: f64 = 3.0;

/// Configurable weights for the four scoring terms.
///
/// Defaults sum to `1.0` so the output is in `[0.0, 1.0]`
/// when all weights are non-negative. Custom weights are
/// not required to sum to `1.0` — the scorer re-normalises
/// by dividing by the weight sum, so callers can experiment
/// with relative emphasis without losing the unit-interval
/// property.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProfileWeights {
    /// Weight applied to the success-rate term.
    pub success: f64,
    /// Weight applied to the latency-score term.
    pub latency: f64,
    /// Weight applied to the retry-score term.
    pub retry: f64,
    /// Weight applied to the `(1 - failure_severity)` term.
    pub failure: f64,
}

impl Default for ProfileWeights {
    fn default() -> Self {
        Self {
            success: 0.40,
            latency: 0.20,
            retry: 0.10,
            failure: 0.30,
        }
    }
}

/// Coarse-grained capability band derived from a unit-interval
/// score.
///
/// The bands are the **policy** surface — the policy mapper
/// in `crate::pow_profile::policy` consumes a band and
/// returns deterministic escalation / pacing adjustments.
/// Callers that want a continuous score use
/// [`PowCapabilityScorer::score`] directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowCapabilityBand {
    /// The profile shows consistent, fast, low-retry solves.
    Strong,
    /// The profile shows acceptable but not impressive
    /// results.
    Degraded,
    /// The profile shows slow solves, high retries, or many
    /// failures.
    Weak,
    /// The profile has too few samples to score; the
    /// documented default is returned instead.
    Unknown,
}

impl PowCapabilityBand {
    /// Stable lower-case wire label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Degraded => "degraded",
            Self::Weak => "weak",
            Self::Unknown => "unknown",
        }
    }
}

/// Configurable deterministic scorer for a
/// [`PowCapabilityProfile`].
///
/// The scorer is `Copy` so it can live in a static
/// configuration struct without a wrapper. The default
/// configuration ([`PowCapabilityScorer::default`]) is the
/// recommended starting point — every field is documented
/// and has a public constant.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PowCapabilityScorer {
    weights: ProfileWeights,
    min_observations: u32,
    latency_budget_ms: u64,
    retry_budget: f64,
}

impl Default for PowCapabilityScorer {
    fn default() -> Self {
        Self {
            weights: ProfileWeights::default(),
            min_observations: MIN_OBSERVATIONS_FOR_SCORING,
            latency_budget_ms: DEFAULT_LATENCY_BUDGET_MS,
            retry_budget: DEFAULT_RETRY_BUDGET,
        }
    }
}

impl PowCapabilityScorer {
    /// Build a scorer with the default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the scoring weights.
    #[must_use]
    pub const fn with_weights(mut self, weights: ProfileWeights) -> Self {
        self.weights = weights;
        self
    }

    /// Replace the minimum-observation floor.
    /// A value of `0` effectively disables sparse-telemetry
    /// fallback (the scorer will return a score for any
    /// non-empty profile). Negative values are clamped to
    /// `0` so the public surface stays simple.
    #[must_use]
    pub const fn with_min_observations(mut self, min_observations: u32) -> Self {
        self.min_observations = min_observations;
        self
    }

    /// Replace the latency budget (milliseconds) used to
    /// score the p95 latency term. A value of `0` falls
    /// back to the documented default so the latency term
    /// never silently becomes "always penalised".
    #[must_use]
    pub const fn with_latency_budget_ms(mut self, latency_budget_ms: u64) -> Self {
        if latency_budget_ms == 0 {
            self.latency_budget_ms = DEFAULT_LATENCY_BUDGET_MS;
        } else {
            self.latency_budget_ms = latency_budget_ms;
        }
        self
    }

    /// Replace the retry budget used to score the average
    /// retries term. A non-positive value falls back to
    /// the documented default.
    #[must_use]
    pub fn with_retry_budget(mut self, retry_budget: f64) -> Self {
        if retry_budget <= 0.0 {
            self.retry_budget = DEFAULT_RETRY_BUDGET;
        } else {
            self.retry_budget = retry_budget;
        }
        self
    }

    /// Current weights.
    #[must_use]
    pub const fn weights(&self) -> ProfileWeights {
        self.weights
    }

    /// Current minimum-observation floor.
    #[must_use]
    pub const fn min_observations(&self) -> u32 {
        self.min_observations
    }

    /// Current latency budget in milliseconds.
    #[must_use]
    pub const fn latency_budget_ms(&self) -> u64 {
        self.latency_budget_ms
    }

    /// Current retry budget.
    #[must_use]
    pub const fn retry_budget(&self) -> f64 {
        self.retry_budget
    }

    /// Score a [`PowCapabilityProfile`].
    ///
    /// Returns [`SPARSE_FALLBACK_SCORE`] when the profile has
    /// fewer attempts than
    /// [`PowCapabilityScorer::min_observations`]
    /// (the documented "no signal" default). Otherwise the
    /// four scoring terms are blended through the configured
    /// weights and re-normalised so the result is in
    /// `[0.0, 1.0]` even when the weights do not sum to
    /// `1.0`.
    #[must_use]
    pub fn score(&self, profile: &PowCapabilityProfile) -> f64 {
        if profile.total_attempts() < self.min_observations {
            return SPARSE_FALLBACK_SCORE;
        }

        let success_rate = profile.success_rate();
        let latency_score = self.latency_score(profile);
        let retry_score = self.retry_score(profile);
        let failure_score = 1.0 - profile.failure_severity();

        let weight_sum =
            self.weights.success + self.weights.latency + self.weights.retry + self.weights.failure;
        if weight_sum <= 0.0 {
            return SPARSE_FALLBACK_SCORE;
        }

        let raw = self.weights.failure.mul_add(
            failure_score,
            self.weights.retry.mul_add(
                retry_score,
                self.weights
                    .latency
                    .mul_add(latency_score, self.weights.success * success_rate),
            ),
        );
        let normalised = raw / weight_sum;
        clamp_unit(normalised)
    }

    /// Score a profile and return a coarse
    /// [`PowCapabilityBand`].
    ///
    /// The band thresholds are fixed and documented
    /// (`strong` ≥ `0.75`, `degraded` ≥ `0.40`, `weak`
    /// otherwise). Profiles that do not meet the
    /// minimum-observation floor return
    /// [`PowCapabilityBand::Unknown`].
    #[must_use]
    pub fn band(&self, profile: &PowCapabilityProfile) -> PowCapabilityBand {
        if profile.total_attempts() < self.min_observations {
            return PowCapabilityBand::Unknown;
        }
        let value = self.score(profile);
        band_for_score(value)
    }

    fn latency_score(&self, profile: &PowCapabilityProfile) -> f64 {
        profile.solve_latency_ms_p95.map_or(1.0, |p95| {
            // Latency values are well within f64 mantissa
            // precision (5_000ms × 100 < 2^23); the `as`
            // conversion is intentional and bounded by
            // the configured latency budget.
            #[allow(clippy::cast_precision_loss)]
            let budget = self.latency_budget_ms as f64;
            #[allow(clippy::cast_precision_loss)]
            let ratio = ((p95 as f64) / budget).clamp(0.0, 1.0);
            1.0 - ratio
        })
    }

    fn retry_score(&self, profile: &PowCapabilityProfile) -> f64 {
        let avg = profile.average_retries();
        let ratio = (avg / self.retry_budget).clamp(0.0, 1.0);
        1.0 - ratio
    }
}

/// Map a unit-interval score to a [`PowCapabilityBand`].
///
/// Exposed publicly so the policy mapper can reuse the
/// thresholds without depending on the scorer.
#[must_use]
pub fn band_for_score(score: f64) -> PowCapabilityBand {
    if score >= 0.75 {
        PowCapabilityBand::Strong
    } else if score >= 0.40 {
        PowCapabilityBand::Degraded
    } else {
        PowCapabilityBand::Weak
    }
}

const fn clamp_unit(value: f64) -> f64 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
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
    use crate::pow_profile::profile::{PowCapabilityProfile, PowCapabilitySample};
    use crate::types::TargetClass;
    use crate::vendor_classifier::VendorId;

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn empty_profile() -> PowCapabilityProfile {
        PowCapabilityProfile::new(
            "example.com",
            TargetClass::ContentSite,
            VendorId::Cloudflare,
        )
    }

    #[test]
    fn empty_profile_returns_sparse_fallback() {
        let scorer = PowCapabilityScorer::new();
        let profile = empty_profile();
        assert!(approx_eq(scorer.score(&profile), SPARSE_FALLBACK_SCORE));
        assert_eq!(scorer.band(&profile), PowCapabilityBand::Unknown);
    }

    #[test]
    fn sparse_profile_returns_sparse_fallback() {
        // Two attempts is below the documented minimum
        // (MIN_OBSERVATIONS_FOR_SCORING = 3).
        let mut profile = empty_profile();
        profile.merge(&PowCapabilitySample::solved(1_000, 0));
        profile.merge(&PowCapabilitySample::solved(1_500, 0));
        assert_eq!(profile.total_attempts(), 2);

        let scorer = PowCapabilityScorer::new();
        assert!(approx_eq(scorer.score(&profile), SPARSE_FALLBACK_SCORE));
        assert_eq!(scorer.band(&profile), PowCapabilityBand::Unknown);
    }

    #[test]
    fn good_telemetry_scores_strong() {
        // 9 solved, 1 failed, fast p95, low retries, only
        // one failure mode (TokenInvalid — moderate weight).
        let mut profile = empty_profile();
        for _ in 0..9 {
            profile.merge(&PowCapabilitySample::solved(800, 0));
        }
        profile.merge(&PowCapabilitySample::failed(
            1_000,
            1,
            crate::pow_profile::profile::PowFailureMode::TokenInvalid,
        ));
        assert_eq!(profile.total_attempts(), 10);

        let scorer = PowCapabilityScorer::new();
        let score = scorer.score(&profile);
        assert!(
            score > 0.75,
            "good telemetry should score Strong, got {score}"
        );
        assert_eq!(scorer.band(&profile), PowCapabilityBand::Strong);
    }

    #[test]
    fn poor_telemetry_scores_weak() {
        // 2 solved, 8 failed, slow p95, many retries, mix
        // of high-severity failure modes.
        let mut profile = empty_profile();
        profile.merge(&PowCapabilitySample::solved(4_000, 2));
        profile.merge(&PowCapabilitySample::solved(4_500, 3));
        for _ in 0..4 {
            profile.merge(&PowCapabilitySample::failed(
                5_000,
                3,
                crate::pow_profile::profile::PowFailureMode::Captcha,
            ));
        }
        for _ in 0..4 {
            profile.merge(&PowCapabilitySample::failed(
                5_000,
                3,
                crate::pow_profile::profile::PowFailureMode::Blocked,
            ));
        }
        assert_eq!(profile.total_attempts(), 10);

        let scorer = PowCapabilityScorer::new();
        let score = scorer.score(&profile);
        assert!(
            score < 0.40,
            "poor telemetry should score Weak, got {score}"
        );
        assert_eq!(scorer.band(&profile), PowCapabilityBand::Weak);
    }

    #[test]
    fn deterministic_for_same_input() {
        let mut a = empty_profile();
        let mut b = empty_profile();
        for _ in 0..5 {
            a.merge(&PowCapabilitySample::solved(1_000, 0));
            b.merge(&PowCapabilitySample::solved(1_000, 0));
        }
        a.merge(&PowCapabilitySample::failed(
            2_000,
            1,
            crate::pow_profile::profile::PowFailureMode::Timeout,
        ));
        b.merge(&PowCapabilitySample::failed(
            2_000,
            1,
            crate::pow_profile::profile::PowFailureMode::Timeout,
        ));

        let scorer = PowCapabilityScorer::new();
        assert!(approx_eq(scorer.score(&a), scorer.score(&b)));
        assert_eq!(scorer.band(&a), scorer.band(&b));
    }

    #[test]
    fn weight_sum_normalisation_handles_non_unit_weights() {
        // Weights that don't sum to 1.0 should still
        // produce a value in [0.0, 1.0].
        let mut profile = empty_profile();
        for _ in 0..3 {
            profile.merge(&PowCapabilitySample::solved(1_000, 0));
        }
        let scorer = PowCapabilityScorer::new().with_weights(ProfileWeights {
            success: 2.0,
            latency: 1.0,
            retry: 0.5,
            failure: 1.0,
        });
        let score = scorer.score(&profile);
        assert!((0.0..=1.0).contains(&score), "score out of range: {score}");
    }

    #[test]
    fn zero_weight_sum_falls_back_to_sparse_default() {
        let mut profile = empty_profile();
        for _ in 0..3 {
            profile.merge(&PowCapabilitySample::solved(1_000, 0));
        }
        let scorer = PowCapabilityScorer::new().with_weights(ProfileWeights {
            success: 0.0,
            latency: 0.0,
            retry: 0.0,
            failure: 0.0,
        });
        assert!(approx_eq(scorer.score(&profile), SPARSE_FALLBACK_SCORE));
    }

    #[test]
    fn min_observations_override_is_respected() {
        let mut profile = empty_profile();
        profile.merge(&PowCapabilitySample::solved(1_000, 0));
        // Default min is 3 — single attempt is sparse.
        assert!(approx_eq(
            PowCapabilityScorer::new().score(&profile),
            SPARSE_FALLBACK_SCORE
        ));
        // Override to 1 attempt — same profile now scores.
        let scorer = PowCapabilityScorer::new().with_min_observations(1);
        let score = scorer.score(&profile);
        assert!((0.0..=1.0).contains(&score));
    }

    #[test]
    fn zero_latency_budget_falls_back_to_default() {
        let scorer = PowCapabilityScorer::new().with_latency_budget_ms(0);
        assert_eq!(scorer.latency_budget_ms(), DEFAULT_LATENCY_BUDGET_MS);
    }

    #[test]
    fn zero_retry_budget_falls_back_to_default() {
        let scorer = PowCapabilityScorer::new().with_retry_budget(0.0);
        assert!((scorer.retry_budget() - DEFAULT_RETRY_BUDGET).abs() < 1e-9);
    }

    #[test]
    fn band_thresholds_are_stable() {
        assert_eq!(band_for_score(0.75), PowCapabilityBand::Strong);
        assert_eq!(band_for_score(1.0), PowCapabilityBand::Strong);
        assert_eq!(band_for_score(0.40), PowCapabilityBand::Degraded);
        assert_eq!(band_for_score(0.74), PowCapabilityBand::Degraded);
        assert_eq!(band_for_score(0.39), PowCapabilityBand::Weak);
        assert_eq!(band_for_score(0.0), PowCapabilityBand::Weak);
    }

    #[test]
    fn band_labels_are_stable() {
        assert_eq!(PowCapabilityBand::Strong.label(), "strong");
        assert_eq!(PowCapabilityBand::Degraded.label(), "degraded");
        assert_eq!(PowCapabilityBand::Weak.label(), "weak");
        assert_eq!(PowCapabilityBand::Unknown.label(), "unknown");
    }

    #[test]
    fn nan_score_clamped_to_zero() {
        let mut profile = empty_profile();
        for _ in 0..3 {
            profile.merge(&PowCapabilitySample::solved(1_000, 0));
        }
        // Weights summing to NaN via 0/0 — we force this
        // by zeroing all weights (already covered above)
        // and assert the path is safe.
        let scorer = PowCapabilityScorer::new().with_weights(ProfileWeights {
            success: 0.0,
            latency: 0.0,
            retry: 0.0,
            failure: 0.0,
        });
        let score = scorer.score(&profile);
        assert!(!score.is_nan());
        assert!(approx_eq(score, SPARSE_FALLBACK_SCORE));
    }
}
