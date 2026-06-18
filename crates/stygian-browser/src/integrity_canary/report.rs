//! Integrity canary risk score + report schema.
//!
//! Turns a list of [`ProbeFinding`] records into an aggregate
//! [`IntegrityRiskScore`] and a stable
//! [`IntegrityRiskClassification`] with documented **Suspected** /
//! **Confirmed** thresholds.
//!
//! ## Risk score formula
//!
//! The aggregate score is a **weighted average** of per-finding
//! severity contributions:
//!
//! ```text
//! score = Σ(weight_i × severity_i) / Σ(weight_i)
//! ```
//!
//! where `severity_i` is the documented
//! [`IntegrityProbeOutcome::severity`] of the finding (Clean=0.0,
//! TrapSuspected=0.5, TrapConfirmed=1.0). **Skipped findings are
//! excluded from both the numerator and the denominator** so that
//! partial probe coverage does not silently deflate the score.
//!
//! The score is clamped to `[0.0, 1.0]`. When every probe is
//! skipped (no signal), the score is `0.0` and the classification
//! is [`IntegrityRiskClassification::Clean`].
//!
//! ## Suspected vs Confirmed thresholds
//!
//! [`IntegrityCanaryPolicy::default`] ships with two thresholds:
//!
//! - [`RISK_SUSPECTED_THRESHOLD_DEFAULT`] = `0.30`
//! - [`RISK_CONFIRMED_THRESHOLD_DEFAULT`] = `0.65`
//!
//! Classification:
//!
//! | Score range | Classification |
//! |---|---|
//! | `[0.0, 0.30)`  | [`Clean`]       |
//! | `[0.30, 0.65)` | [`Suspected`]   |
//! | `[0.65, 1.0]`  | [`Confirmed`]   |
//!
//! Callers can override either threshold via
//! [`IntegrityCanaryPolicy::with_thresholds`] — the
//! `suspected_threshold` MUST be strictly less than the
//! `confirmed_threshold` (validation is enforced by
//! [`IntegrityCanaryPolicy::validate`]).
//!
//! [`Clean`]: IntegrityRiskClassification::Clean
//! [`Suspected`]: IntegrityRiskClassification::Suspected
//! [`Confirmed`]: IntegrityRiskClassification::Confirmed

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::integrity_canary::probes::ProbeFinding;

/// Default lower bound of the **Suspected** risk band.
///
/// Below this threshold the aggregate is classified as
/// [`IntegrityRiskClassification::Clean`].
pub const RISK_SUSPECTED_THRESHOLD_DEFAULT: f64 = 0.30;

/// Default lower bound of the **Confirmed** risk band.
///
/// At or above this threshold the aggregate is classified as
/// [`IntegrityRiskClassification::Confirmed`].
pub const RISK_CONFIRMED_THRESHOLD_DEFAULT: f64 = 0.65;

/// Aggregate risk classification.
///
/// Three bands, mapped from the score via
/// [`IntegrityRiskScore::classify`]:
///
/// - [`Clean`](Self::Clean) — score below the suspected threshold.
/// - [`Suspected`](Self::Suspected) — score at or above the
///   suspected threshold but below the confirmed threshold.
/// - [`Confirmed`](Self::Confirmed) — score at or above the
///   confirmed threshold.
///
/// `Suspected` is the explicit "anti-bot may be probing for stealth
/// artefacts but is not yet blocking" band. `Confirmed` is the
/// "anti-bot has enough signal to block — refresh the session"
/// band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityRiskClassification {
    /// Score is below the suspected threshold. No trap signal.
    Clean,
    /// Score is at or above the suspected threshold but below the
    /// confirmed threshold. Ambiguous trap signal.
    Suspected,
    /// Score is at or above the confirmed threshold. Deterministic
    /// trap signal — treat as a stealth regression.
    Confirmed,
}

impl IntegrityRiskClassification {
    /// Stable `snake_case` label used in telemetry.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Suspected => "suspected",
            Self::Confirmed => "confirmed",
        }
    }
}

impl fmt::Display for IntegrityRiskClassification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Aggregate integrity risk score in `[0.0, 1.0]`.
///
/// Stored as an `f64` so JSON / TOML serialization round-trips
/// without precision loss. The companion [`Self::classification`]
/// field records the threshold-derived classification so consumers
/// can branch on the enum without recomputing it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IntegrityRiskScore {
    /// Numeric risk score in `[0.0, 1.0]`.
    pub value: f64,
    /// Threshold-derived classification.
    pub classification: IntegrityRiskClassification,
    /// Number of findings that contributed to the numerator
    /// (i.e. non-skipped findings).
    pub contributing_findings: usize,
    /// Number of skipped findings excluded from the denominator.
    pub skipped_findings: usize,
}

impl IntegrityRiskScore {
    /// A deterministic clean score (0.0, Clean, zero findings).
    #[must_use]
    pub const fn clean() -> Self {
        Self {
            value: 0.0,
            classification: IntegrityRiskClassification::Clean,
            contributing_findings: 0,
            skipped_findings: 0,
        }
    }

    /// Numeric value in `[0.0, 1.0]`.
    #[must_use]
    pub const fn value(&self) -> f64 {
        self.value
    }

    /// Threshold-derived classification.
    #[must_use]
    pub const fn classification(&self) -> IntegrityRiskClassification {
        self.classification
    }

    /// `true` when the classification is
    /// [`IntegrityRiskClassification::Suspected`] or
    /// [`IntegrityRiskClassification::Confirmed`].
    #[must_use]
    pub const fn is_trap_signal(&self) -> bool {
        !matches!(self.classification, IntegrityRiskClassification::Clean)
    }

    /// `true` when the classification is
    /// [`IntegrityRiskClassification::Confirmed`].
    #[must_use]
    pub const fn is_confirmed(&self) -> bool {
        matches!(self.classification, IntegrityRiskClassification::Confirmed)
    }

    /// Compute the score + classification for a slice of
    /// [`ProbeFinding`]s under the supplied [`IntegrityCanaryPolicy`].
    ///
    /// `findings` is iterated twice — once for the numerator /
    /// denominator, once for the skipped count — so the function is
    /// O(n) and trivially deterministic.
    #[must_use]
    pub fn compute(findings: &[ProbeFinding], policy: &IntegrityCanaryPolicy) -> Self {
        let mut numerator = 0.0;
        let mut denominator = 0.0;
        let mut contributing = 0usize;
        let mut skipped = 0usize;
        for f in findings {
            if f.outcome.contributes() {
                numerator += f.weight * f.outcome.severity();
                denominator += f.weight;
                contributing += 1;
            } else {
                skipped += 1;
            }
        }
        let raw = if denominator <= 0.0 {
            0.0
        } else {
            numerator / denominator
        };
        let value = clamp_unit(raw);
        let classification = policy.classify(value);
        Self {
            value,
            classification,
            contributing_findings: contributing,
            skipped_findings: skipped,
        }
    }

    /// Map a raw score to its classification under the supplied
    /// policy. Exposed so callers can re-classify a score without
    /// recomputing it (e.g. after overriding policy thresholds at
    /// runtime).
    #[must_use]
    pub fn classify_for(
        value: f64,
        policy: &IntegrityCanaryPolicy,
    ) -> IntegrityRiskClassification {
        policy.classify(value)
    }
}

fn clamp_unit(value: f64) -> f64 {
    // NaN handling first because f64::clamp returns NaN for NaN
    // input, but our contract maps NaN to 0.0 explicitly. After the
    // NaN short-circuit, `f64::clamp(0.0, 1.0)` is the cleanest
    // expression of the unit-interval mapping.
    if value.is_nan() {
        return 0.0;
    }
    value.clamp(0.0, 1.0)
}

/// Configurable thresholds for the canary risk bands.
///
/// The defaults match the documented
/// [`RISK_SUSPECTED_THRESHOLD_DEFAULT`] and
/// [`RISK_CONFIRMED_THRESHOLD_DEFAULT`] constants. Override via
/// [`IntegrityCanaryPolicy::with_thresholds`] when callers need
/// stricter or more permissive gating.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IntegrityCanaryPolicy {
    /// Lower bound of the **Suspected** band.
    pub suspected_threshold: f64,
    /// Lower bound of the **Confirmed** band.
    pub confirmed_threshold: f64,
}

impl Default for IntegrityCanaryPolicy {
    fn default() -> Self {
        Self {
            suspected_threshold: RISK_SUSPECTED_THRESHOLD_DEFAULT,
            confirmed_threshold: RISK_CONFIRMED_THRESHOLD_DEFAULT,
        }
    }
}

impl IntegrityCanaryPolicy {
    /// Build a policy with custom thresholds.
    ///
    /// # Panics
    ///
    /// Panics if `suspected_threshold` is `NaN`, `confirmed_threshold`
    /// is `NaN`, or `suspected_threshold >= confirmed_threshold` —
    /// use [`Self::try_with_thresholds`] for a fallible variant.
    #[must_use]
    pub fn with_thresholds(suspected_threshold: f64, confirmed_threshold: f64) -> Self {
        Self::try_with_thresholds(suspected_threshold, confirmed_threshold)
            .expect("integrity canary thresholds must be finite and strictly ordered")
    }

    /// Build a policy with custom thresholds, returning an error
    /// when the inputs are invalid.
    ///
    /// # Errors
    ///
    /// Returns a [`IntegrityCanaryPolicyError::InvalidThresholds`]
    /// when either threshold is `NaN` or when the suspected
    /// threshold is greater than or equal to the confirmed
    /// threshold.
    pub fn try_with_thresholds(
        suspected_threshold: f64,
        confirmed_threshold: f64,
    ) -> Result<Self, IntegrityCanaryPolicyError> {
        if suspected_threshold.is_nan() || confirmed_threshold.is_nan() {
            return Err(IntegrityCanaryPolicyError::InvalidThresholds(format!(
                "thresholds must be finite (suspected={suspected_threshold}, confirmed={confirmed_threshold})"
            )));
        }
        // `partial_cmp` is the documented escape hatch from
        // `clippy::neg_cmp_op_on_partial_ord` for floats — explicit
        // about the fact that two NaN values are not comparable.
        match suspected_threshold.partial_cmp(&confirmed_threshold) {
            Some(std::cmp::Ordering::Less) => {}
            _ => {
                return Err(IntegrityCanaryPolicyError::InvalidThresholds(format!(
                    "suspected_threshold ({suspected_threshold}) must be strictly less than confirmed_threshold ({confirmed_threshold})"
                )));
            }
        }
        Ok(Self {
            suspected_threshold: clamp_unit(suspected_threshold),
            confirmed_threshold: clamp_unit(confirmed_threshold),
        })
    }

    /// Map a `value` in `[0.0, 1.0]` to its classification under
    /// this policy's thresholds.
    #[must_use]
    pub fn classify(&self, value: f64) -> IntegrityRiskClassification {
        let v = clamp_unit(value);
        if v.is_nan() {
            return IntegrityRiskClassification::Clean;
        }
        if v >= self.confirmed_threshold {
            IntegrityRiskClassification::Confirmed
        } else if v >= self.suspected_threshold {
            IntegrityRiskClassification::Suspected
        } else {
            IntegrityRiskClassification::Clean
        }
    }

    /// Validate the policy (used by deserialisation paths).
    ///
    /// # Errors
    ///
    /// Returns [`IntegrityCanaryPolicyError::InvalidThresholds`]
    /// when `suspected_threshold >= confirmed_threshold`.
    pub fn validate(&self) -> Result<(), IntegrityCanaryPolicyError> {
        if self.suspected_threshold.is_nan() || self.confirmed_threshold.is_nan() {
            return Err(IntegrityCanaryPolicyError::InvalidThresholds(format!(
                "thresholds must be finite (suspected={}, confirmed={})",
                self.suspected_threshold, self.confirmed_threshold
            )));
        }
        // `partial_cmp` is the documented escape hatch from
        // `clippy::neg_cmp_op_on_partial_ord` for floats — explicit
        // about the fact that two NaN values are not comparable.
        match self.suspected_threshold.partial_cmp(&self.confirmed_threshold) {
            Some(std::cmp::Ordering::Less) => Ok(()),
            _ => Err(IntegrityCanaryPolicyError::InvalidThresholds(format!(
                "suspected_threshold ({}) must be strictly less than confirmed_threshold ({})",
                self.suspected_threshold, self.confirmed_threshold
            ))),
        }
    }
}

/// Errors produced by integrity-canary policy construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrityCanaryPolicyError {
    /// `suspected_threshold` and `confirmed_threshold` are
    /// invalid (NaN or out of order).
    InvalidThresholds(String),
}

impl fmt::Display for IntegrityCanaryPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidThresholds(msg) => {
                write!(f, "integrity canary thresholds invalid: {msg}")
            }
        }
    }
}

impl std::error::Error for IntegrityCanaryPolicyError {}

/// Aggregate integrity canary report.
///
/// Produced by [`IntegrityCanaryReport::from_findings`] (and the
/// `with_policy` variant) and attached to
/// [`crate::diagnostic::DiagnosticReport`] via
/// [`crate::diagnostic::DiagnosticReport::with_integrity_canary`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntegrityCanaryReport {
    /// Aggregate score and classification.
    pub score: IntegrityRiskScore,
    /// Policy used to derive the classification. Always populated
    /// so consumers can inspect the thresholds without having to
    /// thread the policy through their own state.
    pub policy: IntegrityCanaryPolicy,
    /// Individual probe findings in evaluation order.
    pub findings: Vec<ProbeFinding>,
    /// Aggregated mitigation hints (one entry per finding that
    /// fired with a `Suspected` or `Confirmed` outcome). Surfaced
    /// as a separate field so consumers can render hints without
    /// re-iterating `findings`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mitigation_hints: Vec<MitigationHint>,
    /// Trap findings (`Suspected` or `Confirmed`) only — a thin
    /// view over `findings` for callers that only care about
    /// fired traps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trap_findings: Vec<ProbeFinding>,
}

impl IntegrityCanaryReport {
    /// Build a report from a list of findings using the default
    /// policy.
    #[must_use]
    pub fn from_findings(findings: Vec<ProbeFinding>) -> Self {
        Self::with_policy(findings, IntegrityCanaryPolicy::default())
    }

    /// Build a report from a list of findings + a custom policy.
    ///
    /// Computes the aggregate score, classifies it under `policy`,
    /// and populates the [`Self::mitigation_hints`] and
    /// [`Self::trap_findings`] helper fields.
    #[must_use]
    pub fn with_policy(findings: Vec<ProbeFinding>, policy: IntegrityCanaryPolicy) -> Self {
        let score = IntegrityRiskScore::compute(&findings, &policy);
        let trap_findings: Vec<ProbeFinding> =
            findings.iter().filter(|f| f.is_trap()).cloned().collect();
        let mitigation_hints: Vec<MitigationHint> = trap_findings
            .iter()
            .filter(|f| !f.mitigation_hint.is_empty())
            .map(|f| MitigationHint {
                probe_id: f.id.clone(),
                outcome: f.outcome,
                hint: f.mitigation_hint.clone(),
            })
            .collect();
        Self {
            score,
            policy,
            findings,
            mitigation_hints,
            trap_findings,
        }
    }

    /// `true` when the aggregate classification is
    /// [`IntegrityRiskClassification::Confirmed`].
    #[must_use]
    pub const fn is_confirmed(&self) -> bool {
        self.score.is_confirmed()
    }

    /// `true` when the aggregate classification is
    /// [`IntegrityRiskClassification::Suspected`] or
    /// [`IntegrityRiskClassification::Confirmed`].
    #[must_use]
    pub const fn has_trap_signal(&self) -> bool {
        self.score.is_trap_signal()
    }

    /// Number of findings that fired with a trap outcome.
    #[must_use]
    pub fn trap_count(&self) -> usize {
        self.trap_findings.len()
    }

    /// Number of findings that produced a `Confirmed` outcome.
    #[must_use]
    pub fn confirmed_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.is_confirmed())
            .count()
    }
}

/// Per-probe mitigation hint surfaced in the diagnostic payload.
///
/// Hints are derived from the
/// [`IntegrityProbe::mitigation_hint`][crate::integrity_canary::probes::IntegrityProbe::mitigation_hint]
/// field at report-construction time so they stay in sync with the
/// catalogue. Consumers can render this field directly without
/// re-iterating over [`IntegrityCanaryReport::findings`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MitigationHint {
    /// Probe identifier (`snake_case` label).
    pub probe_id: String,
    /// Resolved outcome (`Suspected` or `Confirmed`).
    pub outcome: crate::integrity_canary::probes::IntegrityProbeOutcome,
    /// Actionable mitigation text.
    pub hint: String,
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::integrity_canary::probes::{
        all_probes, IntegrityProbe, IntegrityProbeId, IntegrityProbeOutcome, ProbeFinding,
    };

    fn finding(
        id: &str,
        weight: f64,
        outcome: IntegrityProbeOutcome,
        hint: &str,
    ) -> ProbeFinding {
        ProbeFinding {
            id: id.to_string(),
            outcome,
            weight,
            evidence: "test".to_string(),
            mitigation_hint: hint.to_string(),
        }
    }

    fn trap_finding(id: &str, weight: f64) -> ProbeFinding {
        finding(id, weight, IntegrityProbeOutcome::TrapConfirmed, "hint")
    }

    fn suspected_finding(id: &str, weight: f64) -> ProbeFinding {
        finding(id, weight, IntegrityProbeOutcome::TrapSuspected, "hint")
    }

    #[test]
    fn empty_findings_produces_clean_score() {
        let report = IntegrityCanaryReport::from_findings(Vec::new());
        assert_eq!(report.score.value, 0.0);
        assert_eq!(
            report.score.classification,
            IntegrityRiskClassification::Clean
        );
        assert!(!report.has_trap_signal());
        assert!(report.mitigation_hints.is_empty());
        assert!(report.trap_findings.is_empty());
    }

    #[test]
    fn all_clean_findings_produces_zero_score() {
        let findings = all_probes()
            .iter()
            .map(|p| finding(p.id.label(), p.weight, IntegrityProbeOutcome::Clean, ""))
            .collect();
        let report = IntegrityCanaryReport::from_findings(findings);
        assert_eq!(report.score.value, 0.0);
        assert_eq!(
            report.score.classification,
            IntegrityRiskClassification::Clean
        );
        assert_eq!(report.score.contributing_findings, 8);
        assert_eq!(report.score.skipped_findings, 0);
    }

    #[test]
    fn all_confirmed_findings_produces_full_score() {
        let findings = all_probes()
            .iter()
            .map(|p| finding(p.id.label(), p.weight, IntegrityProbeOutcome::TrapConfirmed, ""))
            .collect();
        let report = IntegrityCanaryReport::from_findings(findings);
        assert!(
            (report.score.value - 1.0).abs() < 1e-9,
            "score must be 1.0, got: {}",
            report.score.value
        );
        assert_eq!(
            report.score.classification,
            IntegrityRiskClassification::Confirmed
        );
        assert_eq!(report.confirmed_count(), 8);
        assert!(report.is_confirmed());
    }

    #[test]
    fn mixed_outcomes_weighted_average() {
        // 4 clean + 4 confirmed: numerator = sum of weights of the 4
        // confirmed probes (the last 4: 0.08 + 0.10 + 0.14 + 0.12 = 0.44).
        // Denominator = sum of all 8 weights = 1.0. Score = 0.44.
        let mut findings = Vec::new();
        for (i, p) in all_probes().iter().enumerate() {
            if i < 4 {
                findings.push(finding(
                    p.id.label(),
                    p.weight,
                    IntegrityProbeOutcome::Clean,
                    "",
                ));
            } else {
                findings.push(trap_finding(p.id.label(), p.weight));
            }
        }
        let report = IntegrityCanaryReport::from_findings(findings);
        assert!(
            (report.score.value - 0.44).abs() < 1e-6,
            "4 clean + 4 confirmed: score must be 0.44, got: {}",
            report.score.value
        );
        assert_eq!(
            report.score.classification,
            IntegrityRiskClassification::Suspected
        );
    }

    #[test]
    fn suspected_findings_yield_half_score() {
        // All 8 probes suspect: numerator = Σ(weight × 0.5), denominator = Σ(weight) = 1.0
        // Result = 0.5
        let findings = all_probes()
            .iter()
            .map(|p| suspected_finding(p.id.label(), p.weight))
            .collect();
        let report = IntegrityCanaryReport::from_findings(findings);
        assert!(
            (report.score.value - 0.5).abs() < 1e-9,
            "all-suspected must be 0.5, got: {}",
            report.score.value
        );
    }

    #[test]
    fn single_confirmed_on_largest_probe_gives_weighted_score() {
        // Single confirmed on webdriver_descriptor_native (weight 0.20):
        // numerator = 0.20, denominator = 1.0 (all 8 probes counted),
        // score = 0.20. This is intentionally Clean — a single
        // confirmed trap on the largest-weight probe is below the
        // 0.30 suspected threshold. The Suspected/Confirmed bands
        // are calibrated so a single probe can never push the
        // score into Suspected by itself; callers need to see
        // multiple fires to escalate.
        let mut findings = Vec::new();
        for (i, p) in all_probes().iter().enumerate() {
            if i == 0 {
                findings.push(trap_finding(p.id.label(), p.weight));
            } else {
                findings.push(finding(
                    p.id.label(),
                    p.weight,
                    IntegrityProbeOutcome::Clean,
                    "",
                ));
            }
        }
        let report = IntegrityCanaryReport::from_findings(findings);
        assert!(
            (report.score.value - 0.20).abs() < 1e-9,
            "single confirmed trap on the largest-weight probe: score must be 0.20, got: {}",
            report.score.value
        );
        assert_eq!(
            report.score.classification,
            IntegrityRiskClassification::Clean,
            "single confirmed trap on the largest-weight probe must remain Clean (below 0.30)"
        );
        assert_eq!(report.confirmed_count(), 1);
    }

    #[test]
    fn skipped_findings_excluded_from_denominator() {
        // 4 confirmed (total weight = X) + 4 skipped.
        // Numerator = X*1.0, Denominator = X (skipped excluded).
        // score = 1.0 — same as if no skipped findings existed.
        let mut findings = Vec::new();
        let mut weights_kept = 0.0;
        for (i, p) in all_probes().iter().enumerate() {
            if i < 4 {
                findings.push(trap_finding(p.id.label(), p.weight));
                weights_kept += p.weight;
            } else {
                findings.push(finding(
                    p.id.label(),
                    p.weight,
                    IntegrityProbeOutcome::Skipped,
                    "",
                ));
            }
        }
        let report = IntegrityCanaryReport::from_findings(findings);
        assert!(
            (report.score.value - 1.0).abs() < 1e-9,
            "skipped findings must not pull score toward 0, got: {}",
            report.score.value
        );
        assert_eq!(report.score.skipped_findings, 4);
        assert_eq!(report.score.contributing_findings, 4);
        let _ = weights_kept; // silence unused warning if compiler reorders
    }

    #[test]
    fn threshold_distinguishes_suspected_from_confirmed() {
        // Score ≈ 0.40: above 0.30 (suspected) but below 0.65 (confirmed).
        // Build findings summing to numerator=0.40, denominator=1.0.
        // 8 findings at weight 0.125 (close to even), 4 clean + 4 suspected.
        let findings = vec![
            finding("a", 0.125, IntegrityProbeOutcome::Clean, ""),
            finding("b", 0.125, IntegrityProbeOutcome::Clean, ""),
            finding("c", 0.125, IntegrityProbeOutcome::Clean, ""),
            finding("d", 0.125, IntegrityProbeOutcome::Clean, ""),
            suspected_finding("e", 0.125),
            suspected_finding("f", 0.125),
            suspected_finding("g", 0.125),
            suspected_finding("h", 0.125),
        ];
        let report = IntegrityCanaryReport::from_findings(findings);
        // Numerator = 4 × 0.125 × 0.5 = 0.25. Denominator = 8 × 0.125 = 1.0. Score = 0.25.
        assert!(
            (report.score.value - 0.25).abs() < 1e-9,
            "score must be 0.25, got: {}",
            report.score.value
        );
        // Below the suspected threshold (0.30) → Clean.
        assert_eq!(
            report.score.classification,
            IntegrityRiskClassification::Clean
        );

        // Now bump 3 of the clean findings to confirmed (extra 0.125 × 1.0 = 0.375).
        // New numerator = 0.25 + 0.375 = 0.625. Still below 0.65 → Suspected.
        let findings = vec![
            finding("a", 0.125, IntegrityProbeOutcome::TrapConfirmed, ""),
            finding("b", 0.125, IntegrityProbeOutcome::TrapConfirmed, ""),
            finding("c", 0.125, IntegrityProbeOutcome::TrapConfirmed, ""),
            finding("d", 0.125, IntegrityProbeOutcome::Clean, ""),
            suspected_finding("e", 0.125),
            suspected_finding("f", 0.125),
            suspected_finding("g", 0.125),
            suspected_finding("h", 0.125),
        ];
        let report = IntegrityCanaryReport::from_findings(findings);
        assert!(
            (report.score.value - 0.625).abs() < 1e-9,
            "score must be 0.625, got: {}",
            report.score.value
        );
        assert_eq!(
            report.score.classification,
            IntegrityRiskClassification::Suspected,
            "0.625 must be Suspected (above 0.30, below 0.65)"
        );

        // Flip one more clean → confirmed. New score = 0.75 → Confirmed.
        let findings = vec![
            finding("a", 0.125, IntegrityProbeOutcome::TrapConfirmed, ""),
            finding("b", 0.125, IntegrityProbeOutcome::TrapConfirmed, ""),
            finding("c", 0.125, IntegrityProbeOutcome::TrapConfirmed, ""),
            finding("d", 0.125, IntegrityProbeOutcome::TrapConfirmed, ""),
            suspected_finding("e", 0.125),
            suspected_finding("f", 0.125),
            suspected_finding("g", 0.125),
            suspected_finding("h", 0.125),
        ];
        let report = IntegrityCanaryReport::from_findings(findings);
        assert!(
            (report.score.value - 0.75).abs() < 1e-9,
            "score must be 0.75, got: {}",
            report.score.value
        );
        assert_eq!(
            report.score.classification,
            IntegrityRiskClassification::Confirmed,
            "0.75 must be Confirmed (above 0.65)"
        );
    }

    #[test]
    fn policy_with_lower_thresholds_tightens_classification() {
        let policy = IntegrityCanaryPolicy::try_with_thresholds(0.10, 0.40).expect("policy");
        let findings = vec![finding("a", 1.0, IntegrityProbeOutcome::Clean, "")];
        let score = IntegrityRiskScore::compute(&findings, &policy);
        assert_eq!(score.value, 0.0);
        assert_eq!(score.classification, IntegrityRiskClassification::Clean);

        // Score 0.45 → above confirmed threshold (0.40) with strict thresholds.
        let findings = vec![finding("a", 1.0, IntegrityProbeOutcome::TrapSuspected, "")];
        let score = IntegrityRiskScore::compute(&findings, &policy);
        assert!((score.value - 0.5).abs() < 1e-9);
        assert_eq!(score.classification, IntegrityRiskClassification::Confirmed);
    }

    #[test]
    fn policy_rejects_reversed_or_equal_thresholds() {
        let err = IntegrityCanaryPolicy::try_with_thresholds(0.50, 0.50).unwrap_err();
        assert!(matches!(err, IntegrityCanaryPolicyError::InvalidThresholds(_)));
        let err = IntegrityCanaryPolicy::try_with_thresholds(0.70, 0.30).unwrap_err();
        assert!(matches!(err, IntegrityCanaryPolicyError::InvalidThresholds(_)));
    }

    #[test]
    fn policy_rejects_nan_thresholds() {
        let err = IntegrityCanaryPolicy::try_with_thresholds(f64::NAN, 0.65).unwrap_err();
        assert!(matches!(err, IntegrityCanaryPolicyError::InvalidThresholds(_)));
        let err = IntegrityCanaryPolicy::try_with_thresholds(0.30, f64::NAN).unwrap_err();
        assert!(matches!(err, IntegrityCanaryPolicyError::InvalidThresholds(_)));
    }

    #[test]
    fn trap_findings_and_hints_populated_for_fired_probes() {
        let findings = vec![
            trap_finding("webdriver_descriptor_native", 0.20),
            ProbeFinding {
                id: "performance_now_resolution".to_string(),
                outcome: IntegrityProbeOutcome::TrapSuspected,
                weight: 0.14,
                evidence: "deviation detected".to_string(),
                mitigation_hint: "Apply continuous jitter".to_string(),
            },
            finding("error_to_string_native", 0.08, IntegrityProbeOutcome::Clean, ""),
        ];
        let report = IntegrityCanaryReport::from_findings(findings);
        assert_eq!(report.trap_count(), 2);
        assert_eq!(report.confirmed_count(), 1);
        assert_eq!(report.mitigation_hints.len(), 2);
        // Clean findings never carry a mitigation hint entry.
        let clean_hint = report
            .mitigation_hints
            .iter()
            .find(|h| h.probe_id == "error_to_string_native");
        assert!(clean_hint.is_none());
    }

    #[test]
    fn nan_score_classifies_as_clean() {
        let policy = IntegrityCanaryPolicy::default();
        assert_eq!(
            IntegrityRiskScore::classify_for(f64::NAN, &policy),
            IntegrityRiskClassification::Clean
        );
        assert_eq!(
            policy.classify(f64::NAN),
            IntegrityRiskClassification::Clean
        );
    }

    #[test]
    fn score_outside_unit_interval_is_clamped() {
        let policy = IntegrityCanaryPolicy::default();
        assert_eq!(
            policy.classify(1.5),
            IntegrityRiskClassification::Confirmed
        );
        assert_eq!(policy.classify(-0.5), IntegrityRiskClassification::Clean);
    }

    #[test]
    fn confirmed_finding_helper_attaches_hint() {
        let probe = IntegrityProbe::confirmed_finding("test_probe", 0.10, "evidence");
        let report = IntegrityCanaryReport::from_findings(vec![probe]);
        assert!(report.is_confirmed());
        assert_eq!(report.trap_count(), 1);
    }

    #[test]
    fn probe_id_label_is_stable_for_trend_seam() {
        let id = IntegrityProbeId::WebDriverDescriptorNative;
        assert_eq!(id.label(), "webdriver_descriptor_native");
    }

    #[test]
    fn mitigation_hints_carry_outcome_label() {
        let findings = vec![ProbeFinding {
            id: "test".to_string(),
            outcome: IntegrityProbeOutcome::TrapConfirmed,
            weight: 0.20,
            evidence: "x".to_string(),
            mitigation_hint: "apply native descriptor".to_string(),
        }];
        let report = IntegrityCanaryReport::from_findings(findings);
        assert_eq!(report.mitigation_hints.len(), 1);
        assert_eq!(
            report.mitigation_hints[0].outcome,
            IntegrityProbeOutcome::TrapConfirmed
        );
        assert_eq!(report.mitigation_hints[0].probe_id, "test");
    }

    #[test]
    fn report_serializes_with_snake_case_keys() {
        let report = IntegrityCanaryReport::from_findings(vec![trap_finding("a", 1.0)]);
        let json = serde_json::to_string(&report).expect("serialize");
        assert!(json.contains("\"score\""), "got: {json}");
        assert!(json.contains("\"classification\""), "got: {json}");
        assert!(json.contains("\"contributing_findings\""), "got: {json}");
        assert!(json.contains("\"skipped_findings\""), "got: {json}");
        assert!(json.contains("\"findings\""), "got: {json}");
        assert!(json.contains("\"trap_findings\""), "got: {json}");
        assert!(json.contains("\"mitigation_hints\""), "got: {json}");
        assert!(json.contains("\"suspected_threshold\""), "got: {json}");
        assert!(json.contains("\"confirmed_threshold\""), "got: {json}");
    }

    #[test]
    fn report_roundtrips_through_json() {
        let findings = vec![
            trap_finding("a", 0.5),
            finding("b", 0.5, IntegrityProbeOutcome::Clean, ""),
        ];
        let report = IntegrityCanaryReport::from_findings(findings);
        let json = serde_json::to_string(&report).expect("serialize");
        let restored: IntegrityCanaryReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored, report);
    }

    #[test]
    fn empty_report_omits_helper_fields_in_json() {
        let report = IntegrityCanaryReport::from_findings(Vec::new());
        let json = serde_json::to_string(&report).expect("serialize");
        assert!(
            !json.contains("mitigation_hints"),
            "empty report must omit mitigation_hints: {json}"
        );
        assert!(
            !json.contains("trap_findings"),
            "empty report must omit trap_findings: {json}"
        );
    }
}