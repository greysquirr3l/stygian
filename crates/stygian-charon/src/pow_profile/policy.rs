//! Policy mapping from `PoW` capability score to runtime-policy
//! adjustments (T93).
//!
//! The mapper consumes a [`PowCapabilityScore`]
//! (a unit-interval score plus a coarse band) and a
//! [`RuntimePolicy`][crate::types::RuntimePolicy] and returns
//! a **new** policy with deterministic escalation / pacing
//! adjustments. Adjustments are bounded by
//! [`MAX_POW_RISK_DELTA`] — the same safety-clamp pattern
//! the T83 [`MAX_RISK_DELTA`][crate::challenge_feedback::MAX_RISK_DELTA]
//! uses — so a single `PoW` profile can never shift the
//! risk score by more than the documented ceiling.
//!
//! ## Band → adjustment table
//!
//! | Band       | Execution mode        | Session mode     | Pacing                                    | Retries | Risk delta |
//! |------------|----------------------|------------------|-------------------------------------------|---------|------------|
//! | `Strong`   | unchanged            | unchanged        | rate floor at 80% of current              | current | `0.0`      |
//! | `Degraded` | unchanged            | unchanged        | unchanged                                 | current | `0.0`      |
//! | `Weak`     | escalate to Browser  | escalate to Sticky | rate floor at 1.0 rps, backoff ≥ 1000 ms | +1      | `+0.10`    |
//! | `Unknown`  | unchanged            | unchanged        | unchanged                                 | current | `0.0`      |
//!
//! The `Strong` band reduces `rate_limit_rps` (but never
//! below `1.0` rps) because a profile that consistently
//! solves fast is safe to drive at a higher rate. The
//! `Weak` band escalates execution mode (when not already
//! browser+sticky), tightens pacing, and adds a
//! `+MAX_POW_RISK_DELTA` risk-score lift.
//!
//! ## Why a clamp?
//!
//! A feedback loop that can shift the risk score
//! arbitrarily would amplify noise. The clamp mirrors the
//! T83 pattern: a single `PoW` profile can never move the
//! risk score by more than [`MAX_POW_RISK_DELTA`], and the
//! final risk score is re-clamped to `[0.0, 1.0]` after
//! the adjustment.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::pow_profile::scorer::{PowCapabilityBand, SPARSE_FALLBACK_SCORE};
use crate::types::{ExecutionMode, RuntimePolicy, SessionMode};

/// Documented **upper bound** for the risk-score lift the
/// `PoW` policy mapper can apply to a single
/// [`RuntimePolicy`][crate::types::RuntimePolicy].
///
/// The default is **0.10** (half of the T83
/// [`MAX_RISK_DELTA`][crate::challenge_feedback::MAX_RISK_DELTA]).
/// The `PoW` profile is a *secondary* signal — the primary
/// risk driver is the T83 challenge memory and the T91
/// token lifecycle. Operators may **lower** the clamp via
/// [`PowPolicyThresholds::with_max_risk_delta`] but cannot
/// raise it above this documented safety bound.
pub const MAX_POW_RISK_DELTA: f64 = 0.10;

/// Configurable thresholds for the `PoW` policy mapper.
///
/// The defaults match the band boundaries the
/// [`PowCapabilityScorer`][crate::pow_profile::PowCapabilityScorer]
/// uses (`strong` ≥ `0.75`, `degraded` ≥ `0.40`). The struct
/// is `Copy` so it can live in a static configuration
/// struct.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PowPolicyThresholds {
    /// Lower edge of the `Strong` band (inclusive).
    pub strong_floor: f64,
    /// Lower edge of the `Degraded` band (inclusive).
    pub degraded_floor: f64,
    /// Maximum risk-score lift the mapper may apply
    /// (clamped to `[0.0, MAX_POW_RISK_DELTA]`).
    pub max_risk_delta: f64,
    /// Pacing floor (rps) for the `Strong` band — the
    /// adjusted policy never drops below
    /// `strong_rate_floor_rps` even if the input policy had
    /// a higher rate. Defaults to `1.0` so a previously
    /// high-rate policy does not get silently slowed to
    /// nothing by the mapper.
    pub strong_rate_floor_rps: f64,
    /// Pacing ceiling (rps) for the `Weak` band — the
    /// adjusted policy never exceeds
    /// `weak_rate_ceiling_rps` even if the input policy had
    /// a higher rate.
    pub weak_rate_ceiling_rps: f64,
    /// Backoff floor (ms) for the `Weak` band.
    pub weak_backoff_floor_ms: u64,
}

impl Default for PowPolicyThresholds {
    fn default() -> Self {
        Self {
            strong_floor: 0.75,
            degraded_floor: 0.40,
            max_risk_delta: MAX_POW_RISK_DELTA,
            strong_rate_floor_rps: 1.0,
            weak_rate_ceiling_rps: 1.0,
            weak_backoff_floor_ms: 1_000,
        }
    }
}

impl PowPolicyThresholds {
    /// Replace the `max_risk_delta` clamp. Clamped to
    /// `[0.0, MAX_POW_RISK_DELTA]` so callers cannot
    /// widen the documented safety bound.
    #[must_use]
    pub const fn with_max_risk_delta(mut self, max_risk_delta: f64) -> Self {
        let clamped = if max_risk_delta < 0.0 {
            0.0
        } else if max_risk_delta > MAX_POW_RISK_DELTA {
            MAX_POW_RISK_DELTA
        } else {
            max_risk_delta
        };
        self.max_risk_delta = clamped;
        self
    }

    /// Replace the `strong_rate_floor_rps` floor. Non-finite
    /// or non-positive values fall back to the documented
    /// default so the mapper cannot silently disable
    /// pacing.
    #[must_use]
    pub fn with_strong_rate_floor_rps(mut self, floor: f64) -> Self {
        if floor.is_finite() && floor > 0.0 {
            self.strong_rate_floor_rps = floor;
        }
        self
    }

    /// Replace the `weak_rate_ceiling_rps` ceiling.
    /// Non-finite or non-positive values fall back to the
    /// documented default.
    #[must_use]
    pub fn with_weak_rate_ceiling_rps(mut self, ceiling: f64) -> Self {
        if ceiling.is_finite() && ceiling > 0.0 {
            self.weak_rate_ceiling_rps = ceiling;
        }
        self
    }

    /// Replace the `weak_backoff_floor_ms` floor. Zero
    /// values fall back to the documented default.
    #[must_use]
    pub const fn with_weak_backoff_floor_ms(mut self, floor: u64) -> Self {
        if floor == 0 {
            self.weak_backoff_floor_ms = 1_000;
        } else {
            self.weak_backoff_floor_ms = floor;
        }
        self
    }
}

/// A `PoW` capability score plus the band label that the
/// scorer derived from it.
///
/// This is the **unit** the policy mapper consumes — the
/// mapper does not need to know which scorer produced the
/// score. Helpers like
/// [`score_from_profile`][crate::pow_profile::score_from_profile]
/// build a [`PowCapabilityScore`] from a profile + scorer
/// pair so callers can chain the two.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PowCapabilityScore {
    /// Unit-interval score in `[0.0, 1.0]`.
    pub value: f64,
    /// Coarse band derived from `value`.
    pub band: PowCapabilityBand,
}

impl PowCapabilityScore {
    /// Build a score + band directly (no profile required).
    /// The band is recomputed from the score so the two
    /// fields cannot drift out of sync. A value equal to
    /// [`SPARSE_FALLBACK_SCORE`] (the documented "no
    /// signal" default) always maps to
    /// [`PowCapabilityBand::Unknown`].
    #[must_use]
    pub fn new(value: f64) -> Self {
        let clamped = if value.is_nan() {
            SPARSE_FALLBACK_SCORE
        } else {
            value.clamp(0.0, 1.0)
        };
        let band = if (clamped - SPARSE_FALLBACK_SCORE).abs() < 1e-9 {
            PowCapabilityBand::Unknown
        } else {
            band_for_score(clamped)
        };
        Self {
            value: clamped,
            band,
        }
    }

    /// `true` if the score is the sparse-telemetry default
    /// ([`SPARSE_FALLBACK_SCORE`]) or the band is
    /// [`PowCapabilityBand::Unknown`].
    #[must_use]
    pub fn is_unknown(&self) -> bool {
        matches!(self.band, PowCapabilityBand::Unknown)
            || (self.value - SPARSE_FALLBACK_SCORE).abs() < 1e-9
    }
}

fn band_for_score(score: f64) -> PowCapabilityBand {
    if score >= 0.75 {
        PowCapabilityBand::Strong
    } else if score >= 0.40 {
        PowCapabilityBand::Degraded
    } else {
        PowCapabilityBand::Weak
    }
}

/// Map a [`PowCapabilityScore`] to a deterministic
/// [`RuntimePolicy`][crate::types::RuntimePolicy] adjustment.
///
/// The mapper is **non-mutating** — it clones the input
/// policy and returns a new one with the documented
/// adjustments applied. The risk score is re-clamped to
/// `[0.0, 1.0]` after the lift so the final value is
/// always in the unit interval.
///
/// `Strong` and `Degraded` bands produce **non-escalating**
/// adjustments (`Strong` reduces the rate-limit floor,
/// `Degraded` is a no-op besides the config hint). `Weak`
/// is the only band that escalates the policy.
#[must_use]
pub fn adjust_runtime_policy_for_pow(
    policy: &RuntimePolicy,
    score: &PowCapabilityScore,
    thresholds: &PowPolicyThresholds,
) -> RuntimePolicy {
    let mut adjusted = policy.clone();
    match score.band {
        PowCapabilityBand::Strong => apply_strong(&mut adjusted, thresholds),
        PowCapabilityBand::Degraded => apply_degraded(&mut adjusted),
        PowCapabilityBand::Weak => apply_weak(&mut adjusted, thresholds),
        PowCapabilityBand::Unknown => apply_unknown(&mut adjusted),
    }
    adjusted.risk_score = (adjusted.risk_score).clamp(0.0, 1.0);
    adjusted
}

fn apply_strong(policy: &mut RuntimePolicy, thresholds: &PowPolicyThresholds) {
    // The "Strong" adjustment: keep the operator's pacing
    // unless it is below the documented floor. We never
    // raise the rate above the policy's existing value —
    // the runner already picked a sensible rate for the
    // target; we only ensure it does not silently drop
    // below the floor.
    if policy.rate_limit_rps < thresholds.strong_rate_floor_rps {
        policy.rate_limit_rps = thresholds.strong_rate_floor_rps;
    }
    insert_capability_hint(&mut policy.config_hints, "strong", "strong");
}

fn apply_degraded(policy: &mut RuntimePolicy) {
    insert_capability_hint(&mut policy.config_hints, "degraded", "degraded");
}

fn apply_weak(policy: &mut RuntimePolicy, thresholds: &PowPolicyThresholds) {
    if policy.execution_mode != ExecutionMode::Browser {
        policy.execution_mode = ExecutionMode::Browser;
    }
    if policy.session_mode != SessionMode::Sticky {
        policy.session_mode = SessionMode::Sticky;
    }
    if policy.rate_limit_rps > thresholds.weak_rate_ceiling_rps {
        policy.rate_limit_rps = thresholds.weak_rate_ceiling_rps;
    }
    policy.backoff_base_ms = policy.backoff_base_ms.max(thresholds.weak_backoff_floor_ms);
    policy.max_retries = policy.max_retries.saturating_add(1);
    if policy.sticky_session_ttl_secs.is_none() {
        policy.sticky_session_ttl_secs = Some(600);
    }
    if !policy
        .required_stygian_features
        .iter()
        .any(|f| f == "stygian-proxy")
    {
        policy
            .required_stygian_features
            .push("stygian-proxy".to_string());
    }
    insert_capability_hint(&mut policy.config_hints, "weak", "weak");
    insert_pow_escalation_hint(&mut policy.config_hints, "weak");
    let lift = thresholds.max_risk_delta;
    policy.risk_score = (policy.risk_score + lift).clamp(0.0, 1.0);
}

fn apply_unknown(policy: &mut RuntimePolicy) {
    insert_capability_hint(&mut policy.config_hints, "unknown", "unknown");
}

fn insert_capability_hint(hints: &mut BTreeMap<String, String>, label: &str, value: &str) {
    hints.insert("pow.capability".to_string(), value.to_string());
    // Also tag the band label for downstream tools that
    // want a stable enum string rather than the raw score.
    hints.insert("pow.capability_band".to_string(), label.to_string());
}

fn insert_pow_escalation_hint(hints: &mut BTreeMap<String, String>, level: &str) {
    hints.insert("pow.escalation".to_string(), level.to_string());
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

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn base_policy() -> RuntimePolicy {
        RuntimePolicy {
            execution_mode: ExecutionMode::Http,
            session_mode: SessionMode::Stateless,
            telemetry_level: crate::types::TelemetryLevel::Standard,
            rate_limit_rps: 3.0,
            max_retries: 2,
            backoff_base_ms: 250,
            enable_warmup: false,
            enforce_webrtc_proxy_only: false,
            sticky_session_ttl_secs: None,
            required_stygian_features: Vec::new(),
            config_hints: BTreeMap::new(),
            risk_score: 0.30,
        }
    }

    #[test]
    fn strong_band_keeps_default_and_floors_rate() {
        let score = PowCapabilityScore::new(0.90);
        let thresholds = PowPolicyThresholds::default();
        let policy = RuntimePolicy {
            rate_limit_rps: 0.5,
            ..base_policy()
        };
        let adjusted = adjust_runtime_policy_for_pow(&policy, &score, &thresholds);
        assert_eq!(adjusted.execution_mode, ExecutionMode::Http);
        assert!(adjusted.rate_limit_rps >= 1.0);
        assert!(approx_eq(adjusted.risk_score, policy.risk_score));
        assert_eq!(
            adjusted.config_hints.get("pow.capability"),
            Some(&"strong".to_string())
        );
    }

    #[test]
    fn degraded_band_is_a_no_op() {
        let score = PowCapabilityScore::new(0.55);
        let thresholds = PowPolicyThresholds::default();
        let policy = base_policy();
        let adjusted = adjust_runtime_policy_for_pow(&policy, &score, &thresholds);
        assert_eq!(adjusted.execution_mode, policy.execution_mode);
        assert_eq!(adjusted.session_mode, policy.session_mode);
        assert!(approx_eq(adjusted.rate_limit_rps, policy.rate_limit_rps));
        assert!(approx_eq(adjusted.risk_score, policy.risk_score));
        assert_eq!(
            adjusted.config_hints.get("pow.capability"),
            Some(&"degraded".to_string())
        );
    }

    #[test]
    fn weak_band_escalates_to_browser_sticky() {
        let score = PowCapabilityScore::new(0.20);
        let thresholds = PowPolicyThresholds::default();
        let policy = base_policy();
        let adjusted = adjust_runtime_policy_for_pow(&policy, &score, &thresholds);
        assert_eq!(adjusted.execution_mode, ExecutionMode::Browser);
        assert_eq!(adjusted.session_mode, SessionMode::Sticky);
        assert!(adjusted.rate_limit_rps <= thresholds.weak_rate_ceiling_rps);
        assert!(adjusted.backoff_base_ms >= thresholds.weak_backoff_floor_ms);
        assert!(adjusted.max_retries > policy.max_retries);
        assert!(adjusted.sticky_session_ttl_secs.is_some());
        assert!(adjusted
            .required_stygian_features
            .contains(&"stygian-proxy".to_string()));
        assert!(approx_eq(
            adjusted.risk_score,
            (policy.risk_score + MAX_POW_RISK_DELTA).clamp(0.0, 1.0)
        ));
        assert_eq!(
            adjusted.config_hints.get("pow.escalation"),
            Some(&"weak".to_string())
        );
    }

    #[test]
    fn unknown_band_is_a_no_op_with_hint() {
        let score = PowCapabilityScore::new(SPARSE_FALLBACK_SCORE);
        let thresholds = PowPolicyThresholds::default();
        let policy = base_policy();
        let adjusted = adjust_runtime_policy_for_pow(&policy, &score, &thresholds);
        assert_eq!(adjusted.execution_mode, policy.execution_mode);
        assert_eq!(adjusted.session_mode, policy.session_mode);
        assert!(approx_eq(adjusted.risk_score, policy.risk_score));
        assert_eq!(
            adjusted.config_hints.get("pow.capability"),
            Some(&"unknown".to_string())
        );
    }

    #[test]
    fn weak_band_respects_already_browser_sticky_policy() {
        let score = PowCapabilityScore::new(0.10);
        let thresholds = PowPolicyThresholds::default();
        let policy = RuntimePolicy {
            execution_mode: ExecutionMode::Browser,
            session_mode: SessionMode::Sticky,
            max_retries: 5,
            ..base_policy()
        };
        let adjusted = adjust_runtime_policy_for_pow(&policy, &score, &thresholds);
        assert_eq!(adjusted.execution_mode, ExecutionMode::Browser);
        assert_eq!(adjusted.session_mode, SessionMode::Sticky);
        // stygian-proxy must not be added twice.
        let proxy_count = adjusted
            .required_stygian_features
            .iter()
            .filter(|f| f.as_str() == "stygian-proxy")
            .count();
        assert_eq!(proxy_count, 1);
    }

    #[test]
    fn max_risk_delta_cannot_exceed_documented_max() {
        let thresholds = PowPolicyThresholds::default().with_max_risk_delta(0.95);
        assert!(thresholds.max_risk_delta <= MAX_POW_RISK_DELTA);

        let narrowed = PowPolicyThresholds::default().with_max_risk_delta(0.02);
        assert!(approx_eq(narrowed.max_risk_delta, 0.02));
    }

    #[test]
    fn strong_rate_floor_ignores_non_finite_or_non_positive() {
        let thresholds = PowPolicyThresholds::default().with_strong_rate_floor_rps(0.0);
        assert!(thresholds.strong_rate_floor_rps > 0.0);
        let thresholds = PowPolicyThresholds::default().with_strong_rate_floor_rps(f64::NAN);
        assert!(thresholds.strong_rate_floor_rps.is_finite());
    }

    #[test]
    fn weak_backoff_floor_ignores_zero() {
        let thresholds = PowPolicyThresholds::default().with_weak_backoff_floor_ms(0);
        assert_eq!(thresholds.weak_backoff_floor_ms, 1_000);
    }

    #[test]
    fn unknown_score_constructor_returns_unknown_band() {
        let score = PowCapabilityScore::new(SPARSE_FALLBACK_SCORE);
        assert!(score.is_unknown());
        assert_eq!(score.band, PowCapabilityBand::Unknown);
    }

    #[test]
    fn strong_score_constructor_returns_strong_band() {
        let score = PowCapabilityScore::new(0.95);
        assert!(!score.is_unknown());
        assert_eq!(score.band, PowCapabilityBand::Strong);
    }

    #[test]
    fn score_constructor_clamps_and_clamps_nan() {
        let s = PowCapabilityScore::new(f64::NAN);
        assert!(approx_eq(s.value, SPARSE_FALLBACK_SCORE));
        let s = PowCapabilityScore::new(2.0);
        assert!(approx_eq(s.value, 1.0));
        let s = PowCapabilityScore::new(-0.5);
        assert!(approx_eq(s.value, 0.0));
    }

    #[test]
    fn risk_score_is_clamped_to_unit_interval_after_lift() {
        // Even an extreme input risk + the max lift must
        // not exceed 1.0.
        let score = PowCapabilityScore::new(0.10);
        let thresholds = PowPolicyThresholds::default();
        let policy = RuntimePolicy {
            risk_score: 0.99,
            ..base_policy()
        };
        let adjusted = adjust_runtime_policy_for_pow(&policy, &score, &thresholds);
        assert!(adjusted.risk_score <= 1.0);
    }
}
