//! Canary trend-detection seam (T84 hookup).
//!
//! T84 will add the production canary hard-gate that emits
//! governance-grade CI summaries from per-run canary observations.
//! To keep the T92 surface stable, this module exposes the
//! **observation shape** that T84 will consume without forcing
//! future canary infrastructure to know about the probe catalogue
//! or the scoring formula.
//!
//! [`CanaryTrendObservation::from_report`] is the canonical
//! constructor: callers pass the just-built
//! [`crate::integrity_canary::report::IntegrityCanaryReport`] and
//! receive a deterministic, JSON-stable record keyed by a
//! signature hash so two reports with the same findings produce
//! byte-identical observations (the trend signal collapses to a
//! single bucket per signature).
//!
//! The seam is **default-on** and lives next to the probe set so
//! existing consumers that wire `IntegrityCanaryReport` into the
//! diagnostic payload can immediately stream the same record to a
//! future canary aggregator without further changes.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::freshness::signature_hash;
use crate::integrity_canary::report::{
    IntegrityCanaryReport, IntegrityRiskClassification, IntegrityRiskScore,
};

/// Coarse trend severity band.
///
/// Mirrors [`IntegrityRiskClassification`] but uses a stable
/// telemetry label set so a future T84 aggregator can branch on
/// the band without importing the `integrity_canary` module. The
/// labels are deliberately ASCII-lowercase with underscores (no
/// `snake_case` rename needed — the serde tag handles it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrendSeverity {
    /// Score below the suspected threshold — no trend signal.
    Clean,
    /// Score at or above the suspected threshold but below the
    /// confirmed threshold — soft signal, watch the next runs.
    Suspected,
    /// Score at or above the confirmed threshold — hard signal,
    /// refresh the session.
    Confirmed,
}

impl TrendSeverity {
    /// Stable `snake_case` label used in telemetry.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Suspected => "suspected",
            Self::Confirmed => "confirmed",
        }
    }

    /// Resolve the trend severity for a [`IntegrityRiskScore`].
    #[must_use]
    pub const fn from_score(score: &IntegrityRiskScore) -> Self {
        match score.classification {
            IntegrityRiskClassification::Confirmed => Self::Confirmed,
            IntegrityRiskClassification::Suspected => Self::Suspected,
            IntegrityRiskClassification::Clean => Self::Clean,
        }
    }
}

impl fmt::Display for TrendSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Deterministic, JSON-stable trend observation built from an
/// [`IntegrityCanaryReport`].
///
/// The `signature` field is the canonical trend key — two
/// observations with the same signature describe the same finding
/// set (same probe ids + outcomes + weights), so a future trend
/// aggregator can collapse identical signals without recomputing
/// them.
///
/// # Example
///
/// ```
/// use stygian_browser::integrity_canary::{
///     CanaryTrendObservation, IntegrityCanaryReport, IntegrityProbe,
/// };
///
/// let finding = IntegrityProbe::confirmed_finding(
///     "webdriver_descriptor_native",
///     0.20,
///     "data property leak",
/// );
/// let report = IntegrityCanaryReport::from_findings(vec![finding]);
/// let obs = CanaryTrendObservation::from_report(&report);
/// assert!(obs.signature.starts_with("fnv64:"));
/// assert!(obs.score > 0.0);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanaryTrendObservation {
    /// Stable `fnv64:<hex>` signature over (probe ids + outcomes +
    /// weights). Two reports with the same findings produce the
    /// same signature, so the trend aggregator can bucket by this
    /// key directly.
    pub signature: String,
    /// Aggregate risk score in `[0.0, 1.0]`.
    pub score: f64,
    /// Coarse severity band derived from the score.
    pub severity: TrendSeverity,
    /// Number of findings that contributed to the score.
    pub contributing_findings: usize,
    /// Number of findings that were skipped.
    pub skipped_findings: usize,
    /// Number of trap findings (`Suspected` or `Confirmed`).
    pub trap_count: usize,
    /// Number of confirmed findings.
    pub confirmed_count: usize,
    /// Distinct probe ids that fired with a trap outcome, in
    /// evaluation order.
    pub fired_probe_ids: Vec<String>,
    /// Captured observation timestamp (Unix epoch ms). Populated
    /// via [`crate::freshness::unix_epoch_ms`] when the observation
    /// is built; serialised as a `u64` for downstream automation.
    pub captured_at_epoch_ms: u64,
}

impl CanaryTrendObservation {
    /// Build a trend observation from an [`IntegrityCanaryReport`].
    ///
    /// The signature is computed over the probe ids, outcomes, and
    /// weights — two reports with identical findings produce
    /// identical signatures so the trend aggregator can dedupe
    /// without recomputing.
    #[must_use]
    pub fn from_report(report: &IntegrityCanaryReport) -> Self {
        let fired_probe_ids: Vec<String> = report
            .trap_findings
            .iter()
            .map(|f| f.id.clone())
            .collect();

        let mut signature_parts: Vec<String> = Vec::with_capacity(report.findings.len() * 3);
        for f in &report.findings {
            signature_parts.push(f.id.clone());
            signature_parts.push(f.outcome.label().to_string());
            signature_parts.push(format!("{:.6}", f.weight));
        }
        let borrowed: Vec<&str> = signature_parts.iter().map(String::as_str).collect();
        let signature = signature_hash(&borrowed);

        Self {
            signature,
            score: report.score.value,
            severity: TrendSeverity::from_score(&report.score),
            contributing_findings: report.score.contributing_findings,
            skipped_findings: report.score.skipped_findings,
            trap_count: report.trap_count(),
            confirmed_count: report.confirmed_count(),
            fired_probe_ids,
            captured_at_epoch_ms: crate::freshness::unix_epoch_ms(),
        }
    }

    /// `true` when the observation carries a trap signal
    /// (Suspected or Confirmed).
    #[must_use]
    pub const fn has_trap_signal(&self) -> bool {
        !matches!(self.severity, TrendSeverity::Clean)
    }

    /// `true` when the observation is a Confirmed trend entry.
    #[must_use]
    pub const fn is_confirmed(&self) -> bool {
        matches!(self.severity, TrendSeverity::Confirmed)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::integrity_canary::probes::{
        IntegrityProbe, IntegrityProbeOutcome, ProbeFinding,
    };
    use crate::integrity_canary::report::{
        IntegrityCanaryPolicy, IntegrityCanaryReport, IntegrityRiskClassification,
    };

    #[test]
    fn observation_signature_is_deterministic_for_same_findings() {
        let report = IntegrityCanaryReport::from_findings(vec![
            IntegrityProbe::confirmed_finding("a", 0.5, "x"),
            IntegrityProbe::confirmed_finding("b", 0.5, "y"),
        ]);
        let obs_a = CanaryTrendObservation::from_report(&report);
        let obs_b = CanaryTrendObservation::from_report(&report);
        // captured_at_epoch_ms differs at the millisecond level in
        // some runs; verify signature + score equality only.
        assert_eq!(obs_a.signature, obs_b.signature);
        assert!((obs_a.score - obs_b.score).abs() < 1e-9);
    }

    #[test]
    fn observation_signature_changes_with_finding_set() {
        let report_a = IntegrityCanaryReport::from_findings(vec![
            IntegrityProbe::confirmed_finding("a", 0.5, "x"),
        ]);
        let report_b = IntegrityCanaryReport::from_findings(vec![
            IntegrityProbe::confirmed_finding("b", 0.5, "y"),
        ]);
        let obs_a = CanaryTrendObservation::from_report(&report_a);
        let obs_b = CanaryTrendObservation::from_report(&report_b);
        assert_ne!(obs_a.signature, obs_b.signature);
    }

    #[test]
    fn observation_severity_tracks_classification() {
        // Clean report → Clean trend
        let report = IntegrityCanaryReport::from_findings(Vec::new());
        let obs = CanaryTrendObservation::from_report(&report);
        assert_eq!(obs.severity, TrendSeverity::Clean);
        assert!(!obs.has_trap_signal());
        assert!(!obs.is_confirmed());

        // Confirmed report → Confirmed trend
        let report = IntegrityCanaryReport::from_findings(vec![
            IntegrityProbe::confirmed_finding("a", 1.0, "x"),
        ]);
        let obs = CanaryTrendObservation::from_report(&report);
        assert_eq!(obs.severity, TrendSeverity::Confirmed);
        assert!(obs.has_trap_signal());
        assert!(obs.is_confirmed());
    }

    #[test]
    fn observation_carries_fired_probe_ids_in_evaluation_order() {
        let findings = vec![
            ProbeFinding {
                id: "probe_one".to_string(),
                outcome: IntegrityProbeOutcome::TrapConfirmed,
                weight: 0.20,
                evidence: "x".to_string(),
                mitigation_hint: String::new(),
            },
            ProbeFinding {
                id: "probe_two".to_string(),
                outcome: IntegrityProbeOutcome::TrapSuspected,
                weight: 0.15,
                evidence: "y".to_string(),
                mitigation_hint: String::new(),
            },
            ProbeFinding {
                id: "probe_three".to_string(),
                outcome: IntegrityProbeOutcome::Clean,
                weight: 0.10,
                evidence: "z".to_string(),
                mitigation_hint: String::new(),
            },
        ];
        let report = IntegrityCanaryReport::from_findings(findings);
        let obs = CanaryTrendObservation::from_report(&report);
        assert_eq!(
            obs.fired_probe_ids,
            vec!["probe_one".to_string(), "probe_two".to_string()]
        );
        assert_eq!(obs.confirmed_count, 1);
        assert_eq!(obs.trap_count, 2);
    }

    #[test]
    fn observation_skipped_count_reflects_findings() {
        let findings = vec![
            ProbeFinding {
                id: "a".to_string(),
                outcome: IntegrityProbeOutcome::Skipped,
                weight: 0.5,
                evidence: "x".to_string(),
                mitigation_hint: String::new(),
            },
            ProbeFinding {
                id: "b".to_string(),
                outcome: IntegrityProbeOutcome::TrapConfirmed,
                weight: 0.5,
                evidence: "y".to_string(),
                mitigation_hint: String::new(),
            },
        ];
        let report = IntegrityCanaryReport::from_findings(findings);
        let obs = CanaryTrendObservation::from_report(&report);
        assert_eq!(obs.skipped_findings, 1);
        assert_eq!(obs.contributing_findings, 1);
        assert_eq!(obs.confirmed_count, 1);
    }

    #[test]
    fn observation_handles_strict_thresholds() {
        let policy = IntegrityCanaryPolicy::try_with_thresholds(0.10, 0.20).expect("policy");
        let findings = vec![ProbeFinding {
            id: "a".to_string(),
            outcome: IntegrityProbeOutcome::TrapSuspected,
            weight: 1.0,
            evidence: "x".to_string(),
            mitigation_hint: String::new(),
        }];
        let report = IntegrityCanaryReport::with_policy(findings, policy);
        let obs = CanaryTrendObservation::from_report(&report);
        // 0.5 (suspected severity) is above the strict confirmed threshold (0.20).
        assert_eq!(obs.severity, TrendSeverity::Confirmed);
    }

    #[test]
    fn observation_serializes_with_snake_case_keys() {
        let report = IntegrityCanaryReport::from_findings(vec![
            IntegrityProbe::confirmed_finding("a", 0.5, "x"),
        ]);
        let obs = CanaryTrendObservation::from_report(&report);
        let json = serde_json::to_string(&obs).expect("serialize");
        assert!(json.contains("\"signature\""), "got: {json}");
        assert!(json.contains("\"score\""), "got: {json}");
        assert!(json.contains("\"severity\""), "got: {json}");
        assert!(json.contains("\"trap_count\""), "got: {json}");
        assert!(json.contains("\"confirmed_count\""), "got: {json}");
        assert!(
            json.contains("\"fired_probe_ids\""),
            "got: {json}"
        );
        assert!(
            json.contains("\"captured_at_epoch_ms\""),
            "got: {json}"
        );
    }

    #[test]
    fn trend_severity_labels_are_stable() {
        assert_eq!(TrendSeverity::Clean.label(), "clean");
        assert_eq!(TrendSeverity::Suspected.label(), "suspected");
        assert_eq!(TrendSeverity::Confirmed.label(), "confirmed");
    }

    #[test]
    fn trend_severity_from_score_round_trips_classification() {
        let mut score = IntegrityRiskScore::clean();
        assert_eq!(
            TrendSeverity::from_score(&score),
            TrendSeverity::Clean
        );
        score.classification = IntegrityRiskClassification::Suspected;
        assert_eq!(
            TrendSeverity::from_score(&score),
            TrendSeverity::Suspected
        );
        score.classification = IntegrityRiskClassification::Confirmed;
        assert_eq!(
            TrendSeverity::from_score(&score),
            TrendSeverity::Confirmed
        );
    }
}