use crate::challenge_feedback::ChallengeMemory;
use crate::types::{RequirementsProfile, RuntimePolicy, TargetClass};

/// Documented **upper bound** for any single per-key risk-score
/// adjustment the challenge memory can apply.
///
/// The default is **0.20** (twenty percent of the risk-score
/// range). This conservative ceiling is the key safety property of
/// the feedback loop: a single transient outcome can never move the
/// policy into a fundamentally different strategy band. Callers may
/// **lower** the clamp via
/// [`ChallengeFeedbackPolicy::with_max_delta`] but the value is
/// hard-capped at `MAX_RISK_DELTA` to prevent runaway escalation.
pub const MAX_RISK_DELTA: f64 = 0.20;

/// Configurable knobs for the challenge-aware policy feedback loop.
///
/// All fields are bounded by the documented safety constants —
/// [`with_max_delta`][Self::with_max_delta] clamps the supplied
/// value to `[-MAX_RISK_DELTA, +MAX_RISK_DELTA]`.
///
/// # Example
///
/// ```
/// use stygian_charon::challenge_feedback::{ChallengeFeedbackPolicy, MAX_RISK_DELTA};
/// use std::time::Duration;
///
/// let policy = ChallengeFeedbackPolicy::default();
/// assert!(policy.max_delta().abs() <= MAX_RISK_DELTA);
/// assert_eq!(policy.ttl(), Duration::from_mins(10));
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChallengeFeedbackPolicy {
    max_delta: f64,
    ttl: std::time::Duration,
}

impl ChallengeFeedbackPolicy {
    /// Build a feedback policy with a custom clamp and TTL. The
    /// supplied `max_delta` is clamped to `[-MAX_RISK_DELTA,
    /// +MAX_RISK_DELTA]` so callers cannot widen the documented
    /// safety bound.
    #[must_use]
    pub fn new(max_delta: f64, ttl: std::time::Duration) -> Self {
        Self {
            max_delta: max_delta.clamp(-MAX_RISK_DELTA, MAX_RISK_DELTA),
            ttl,
        }
    }

    /// Replace the per-key clamp. Clamped to `[-MAX_RISK_DELTA,
    /// +MAX_RISK_DELTA]`.
    #[must_use]
    pub fn with_max_delta(mut self, max_delta: f64) -> Self {
        self.max_delta = max_delta.clamp(-MAX_RISK_DELTA, MAX_RISK_DELTA);
        self
    }

    /// Replace the memory TTL. Non-positive values fall back to a
    /// one-minute default so the loop cannot accidentally live
    /// forever.
    #[must_use]
    pub const fn with_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.ttl = if ttl.is_zero() {
            std::time::Duration::from_mins(1)
        } else {
            ttl
        };
        self
    }

    /// Configured per-key clamp.
    #[must_use]
    pub const fn max_delta(&self) -> f64 {
        self.max_delta
    }

    /// Configured memory TTL.
    #[must_use]
    pub const fn ttl(&self) -> std::time::Duration {
        self.ttl
    }
}

impl Default for ChallengeFeedbackPolicy {
    fn default() -> Self {
        Self {
            max_delta: MAX_RISK_DELTA,
            ttl: super::memory::DEFAULT_CHALLENGE_TTL,
        }
    }
}

/// Compute the risk-score adjustment a [`ChallengeMemory`] would
/// apply for a `(domain, target_class)` key, using the
/// [`ChallengeFeedbackPolicy::default`] clamp.
///
/// Returns `0.0` when the memory has no entry for the key (the
/// common case on first contact with a new target).
///
/// # Example
///
/// ```
/// use stygian_charon::challenge_feedback::{
///     memory_adjustment_for, ChallengeMemory, ChallengeOutcome,
/// };
/// use stygian_charon::types::TargetClass;
///
/// let memory = ChallengeMemory::with_defaults();
/// memory.record("example.com", TargetClass::ContentSite, ChallengeOutcome::Captcha);
/// let delta = memory_adjustment_for(&memory, "example.com", TargetClass::ContentSite);
/// assert!(delta > 0.0);
/// ```
#[must_use]
pub fn memory_adjustment_for(
    memory: &ChallengeMemory,
    domain: &str,
    target_class: TargetClass,
) -> f64 {
    memory
        .lookup(domain, target_class)
        .map_or(0.0, |entry| {
            clamp_to_policy(&ChallengeFeedbackPolicy::default(), entry.risk_delta())
        })
}

/// Build a [`RuntimePolicy`] from an investigation report and
/// requirements profile, then apply a bounded challenge-memory
/// adjustment to the risk score.
///
/// The adjustment path is identical to
/// [`adjust_runtime_policy`] — this is a convenience wrapper for
/// the common "rebuild policy from scratch" workflow.
///
/// # Example
///
/// ```
/// use stygian_charon::challenge_feedback::{
///     build_runtime_policy_with_memory, ChallengeMemory, ChallengeOutcome,
/// };
/// use stygian_charon::build_runtime_policy;
/// use stygian_charon::types::{
///     AdapterStrategy, AntiBotProvider, Detection, IntegrationRecommendation,
///     InvestigationReport, RequirementsProfile, TargetClass,
/// };
/// use std::collections::BTreeMap;
///
/// let memory = ChallengeMemory::with_defaults();
/// memory.record("example.com", TargetClass::ContentSite, ChallengeOutcome::Captcha);
/// let report = InvestigationReport {
///     page_title: Some("example.com".to_string()),
///     total_requests: 100,
///     blocked_requests: 0,
///     status_histogram: BTreeMap::new(),
///     resource_type_histogram: BTreeMap::new(),
///     provider_histogram: BTreeMap::new(),
///     marker_histogram: BTreeMap::new(),
///     top_markers: Vec::new(),
///     hosts: Vec::new(),
///     suspicious_requests: Vec::new(),
///     aggregate: Detection {
///         provider: AntiBotProvider::Unknown,
///         confidence: 0.0,
///         markers: Vec::new(),
///     },
///     target_class: Some(TargetClass::ContentSite),
/// };
/// let requirements = RequirementsProfile {
///     provider: AntiBotProvider::Unknown,
///     confidence: 0.0,
///     requirements: Vec::new(),
///     recommendation: IntegrationRecommendation {
///         strategy: AdapterStrategy::DirectHttp,
///         rationale: "test".to_string(),
///         required_stygian_features: Vec::new(),
///         config_hints: BTreeMap::new(),
///     },
/// };
/// let policy = build_runtime_policy(&report, &requirements);
/// let with_memory = build_runtime_policy_with_memory(
///     &report,
///     &requirements,
///     &memory,
///     "example.com",
///     TargetClass::ContentSite,
/// );
/// assert!(with_memory.risk_score >= policy.risk_score);
/// ```
#[must_use]
pub fn build_runtime_policy_with_memory(
    report: &crate::types::InvestigationReport,
    requirements: &RequirementsProfile,
    memory: &ChallengeMemory,
    domain: &str,
    target_class: TargetClass,
) -> RuntimePolicy {
    let policy = crate::policy::build_runtime_policy(report, requirements);
    adjust_runtime_policy(&policy, memory, domain, target_class)
}

/// Apply a bounded challenge-memory adjustment to an existing
/// [`RuntimePolicy`].
///
/// The adjustment is added to `policy.risk_score` and the result is
/// re-clamped to `[0.0, 1.0]`. The adjustment itself is
/// **per-key clamped** to
/// [`ChallengeFeedbackPolicy::max_delta`][ChallengeFeedbackPolicy::max_delta]
/// (default `MAX_RISK_DELTA = 0.20`) before being added, so a single
/// entry can never shift the risk score by more than the documented
/// ceiling.
///
/// # Example
///
/// ```
/// use stygian_charon::challenge_feedback::{
///     adjust_runtime_policy, ChallengeMemory, ChallengeOutcome, MAX_RISK_DELTA,
/// };
/// use stygian_charon::types::{
///     ExecutionMode, RuntimePolicy, SessionMode, TargetClass, TelemetryLevel,
/// };
/// use std::collections::BTreeMap;
///
/// let memory = ChallengeMemory::with_defaults();
/// memory.record("example.com", TargetClass::ContentSite, ChallengeOutcome::Captcha);
///
/// let base = RuntimePolicy {
///     execution_mode: ExecutionMode::Http,
///     session_mode: SessionMode::Stateless,
///     telemetry_level: TelemetryLevel::Standard,
///     rate_limit_rps: 3.0,
///     max_retries: 2,
///     backoff_base_ms: 250,
///     enable_warmup: false,
///     enforce_webrtc_proxy_only: false,
///     sticky_session_ttl_secs: None,
///     required_stygian_features: Vec::new(),
///     config_hints: BTreeMap::new(),
///     risk_score: 0.30,
/// };
/// let adjusted = adjust_runtime_policy(&base, &memory, "example.com", TargetClass::ContentSite);
/// assert!(adjusted.risk_score >= base.risk_score);
/// assert!(adjusted.risk_score <= base.risk_score + MAX_RISK_DELTA);
/// ```
#[must_use]
pub fn adjust_runtime_policy(
    policy: &RuntimePolicy,
    memory: &ChallengeMemory,
    domain: &str,
    target_class: TargetClass,
) -> RuntimePolicy {
    let adjustment = memory_adjustment_for(memory, domain, target_class);
    let mut adjusted = policy.clone();
    adjusted.risk_score = (policy.risk_score + adjustment).clamp(0.0, 1.0);
    adjusted
}

fn clamp_to_policy(policy: &ChallengeFeedbackPolicy, raw_delta: f64) -> f64 {
    let bound = policy.max_delta().abs();
    if bound <= 0.0 {
        0.0
    } else if raw_delta > bound {
        bound
    } else if raw_delta < -bound {
        -bound
    } else {
        raw_delta
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
    use crate::challenge_feedback::ChallengeOutcome;
    use crate::types::{
        AdapterStrategy, AntiBotProvider, Detection, ExecutionMode, IntegrationRecommendation,
        InvestigationReport, RuntimePolicy, SessionMode, TelemetryLevel,
    };
    use std::collections::BTreeMap;
    use std::num::NonZeroUsize;
    use std::time::Duration;

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn base_policy() -> RuntimePolicy {
        RuntimePolicy {
            execution_mode: ExecutionMode::Http,
            session_mode: SessionMode::Stateless,
            telemetry_level: TelemetryLevel::Standard,
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

    fn empty_report(target_class: TargetClass) -> InvestigationReport {
        InvestigationReport {
            page_title: Some("example.com".to_string()),
            total_requests: 10,
            blocked_requests: 0,
            status_histogram: BTreeMap::new(),
            resource_type_histogram: BTreeMap::new(),
            provider_histogram: BTreeMap::new(),
            marker_histogram: BTreeMap::new(),
            top_markers: Vec::new(),
            hosts: Vec::new(),
            suspicious_requests: Vec::new(),
            aggregate: Detection {
                provider: AntiBotProvider::Unknown,
                confidence: 0.0,
                markers: Vec::new(),
            },
            target_class: Some(target_class),
        }
    }

    fn empty_requirements() -> RequirementsProfile {
        RequirementsProfile {
            provider: AntiBotProvider::Unknown,
            confidence: 0.0,
            requirements: Vec::new(),
            recommendation: IntegrationRecommendation {
                strategy: AdapterStrategy::DirectHttp,
                rationale: "test".to_string(),
                required_stygian_features: Vec::new(),
                config_hints: BTreeMap::new(),
            },
        }
    }

    #[test]
    fn policy_with_no_memory_returns_base() {
        let memory = ChallengeMemory::with_defaults();
        let policy = base_policy();
        let adjusted = adjust_runtime_policy(
            &policy,
            &memory,
            "example.com",
            TargetClass::ContentSite,
        );
        assert!(approx_eq(adjusted.risk_score, policy.risk_score));
    }

    #[test]
    fn positive_outcome_lifts_risk_score_within_clamp() {
        let memory = ChallengeMemory::new(NonZeroUsize::new(4).unwrap(), Duration::from_mins(1));
        memory.record(
            "example.com",
            TargetClass::ContentSite,
            ChallengeOutcome::HardChallenge,
        );

        let policy = base_policy();
        let adjusted = adjust_runtime_policy(
            &policy,
            &memory,
            "example.com",
            TargetClass::ContentSite,
        );

        let expected_delta = ChallengeOutcome::HardChallenge.risk_delta();
        assert!(adjusted.risk_score >= policy.risk_score);
        assert!(approx_eq(
            adjusted.risk_score,
            (policy.risk_score + expected_delta).clamp(0.0, 1.0)
        ));
        assert!(adjusted.risk_score <= policy.risk_score + MAX_RISK_DELTA);
    }

    #[test]
    fn negative_outcome_lowers_risk_score_within_clamp() {
        let memory = ChallengeMemory::new(NonZeroUsize::new(4).unwrap(), Duration::from_mins(1));
        memory.record(
            "example.com",
            TargetClass::ContentSite,
            ChallengeOutcome::Pass,
        );

        let policy = base_policy();
        let adjusted = adjust_runtime_policy(
            &policy,
            &memory,
            "example.com",
            TargetClass::ContentSite,
        );

        assert!(adjusted.risk_score <= policy.risk_score);
        assert!(adjusted.risk_score >= (policy.risk_score - MAX_RISK_DELTA).max(0.0));
    }

    #[test]
    fn risk_score_clamps_to_unit_interval_under_extreme_inputs() {
        let memory = ChallengeMemory::with_defaults();
        memory.record(
            "example.com",
            TargetClass::ContentSite,
            ChallengeOutcome::Captcha,
        );

        let high = RuntimePolicy {
            risk_score: 0.95,
            ..base_policy()
        };
        let adjusted = adjust_runtime_policy(
            &high,
            &memory,
            "example.com",
            TargetClass::ContentSite,
        );
        assert!(adjusted.risk_score <= 1.0);
        // Single Captcha adds 0.20, so 0.95 + 0.20 = 1.15 clamps to 1.0
        assert!(approx_eq(adjusted.risk_score, 1.0));

        let low = RuntimePolicy {
            risk_score: 0.05,
            ..base_policy()
        };
        // No memory entry — the low baseline is unchanged.
        let no_memory = ChallengeMemory::with_defaults();
        let low_adjusted =
            adjust_runtime_policy(&low, &no_memory, "nope.example", TargetClass::ContentSite);
        assert!(approx_eq(low_adjusted.risk_score, low.risk_score));
    }

    #[test]
    fn risk_score_adjustment_is_bounded_by_max_risk_delta() {
        // Even an outcome that is the largest possible (Blocked/Captcha = 0.20)
        // must never push the adjustment beyond MAX_RISK_DELTA.
        let memory = ChallengeMemory::with_defaults();
        memory.record(
            "example.com",
            TargetClass::ContentSite,
            ChallengeOutcome::Blocked,
        );

        let policy = RuntimePolicy {
            risk_score: 0.0,
            ..base_policy()
        };
        let adjusted = adjust_runtime_policy(
            &policy,
            &memory,
            "example.com",
            TargetClass::ContentSite,
        );

        let lift = adjusted.risk_score - policy.risk_score;
        assert!(lift >= 0.0);
        assert!(lift <= MAX_RISK_DELTA + 1e-9);
        assert!(approx_eq(lift, ChallengeOutcome::Blocked.risk_delta()));
    }

    #[test]
    fn feedback_policy_max_delta_cannot_exceed_documented_max() {
        let widened = ChallengeFeedbackPolicy::default().with_max_delta(0.95);
        assert!(widened.max_delta() <= MAX_RISK_DELTA);

        let narrowed = ChallengeFeedbackPolicy::default().with_max_delta(0.05);
        assert!(approx_eq(narrowed.max_delta(), 0.05));
    }

    #[test]
    fn feedback_policy_zero_ttl_falls_back_to_one_minute() {
        let policy = ChallengeFeedbackPolicy::default().with_ttl(Duration::from_millis(0));
        assert_eq!(policy.ttl(), Duration::from_mins(1));
    }

    #[test]
    fn build_runtime_policy_with_memory_includes_adjustment() {
        let memory = ChallengeMemory::with_defaults();
        memory.record(
            "example.com",
            TargetClass::ContentSite,
            ChallengeOutcome::Captcha,
        );

        let report = empty_report(TargetClass::ContentSite);
        let requirements = empty_requirements();
        let base = crate::policy::build_runtime_policy(&report, &requirements);
        let adjusted = build_runtime_policy_with_memory(
            &report,
            &requirements,
            &memory,
            "example.com",
            TargetClass::ContentSite,
        );

        assert!(adjusted.risk_score >= base.risk_score);
    }

    #[test]
    fn memory_adjustment_for_returns_zero_when_absent() {
        let memory = ChallengeMemory::with_defaults();
        assert!(approx_eq(
            memory_adjustment_for(&memory, "nope.example", TargetClass::ContentSite),
            0.0
        ));
    }
}
