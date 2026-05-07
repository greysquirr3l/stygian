use serde::{Deserialize, Serialize};

use crate::differential::ModeDifferentialRunReport;
use crate::observatory::ObservatoryReport;
use crate::probe::ProbePackReport;

/// Release risk level derived from a normalized risk score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseRiskLevel {
    /// Risk is low enough for routine rollout.
    Low,
    /// Risk is noticeable and should be watched closely.
    Guarded,
    /// Risk is high enough to require rollout caution.
    Elevated,
    /// Risk is severe and should block rollout.
    Critical,
}

/// Thresholds for classifying release risk scores.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ReleaseRiskThresholds {
    /// Score at or above this value is `Guarded`.
    pub guarded_at: f64,
    /// Score at or above this value is `Elevated`.
    pub elevated_at: f64,
    /// Score at or above this value is `Critical`.
    pub critical_at: f64,
}

impl Default for ReleaseRiskThresholds {
    fn default() -> Self {
        Self {
            guarded_at: 0.30,
            elevated_at: 0.55,
            critical_at: 0.75,
        }
    }
}

impl ReleaseRiskThresholds {
    /// Classify a normalized risk score into a risk level.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_charon::ReleaseRiskLevel;
    /// use stygian_charon::ReleaseRiskThresholds;
    ///
    /// let thresholds = ReleaseRiskThresholds::default();
    /// assert_eq!(thresholds.classify(0.8), ReleaseRiskLevel::Critical);
    /// ```
    #[must_use]
    pub fn classify(&self, score: f64) -> ReleaseRiskLevel {
        if score >= self.critical_at {
            ReleaseRiskLevel::Critical
        } else if score >= self.elevated_at {
            ReleaseRiskLevel::Elevated
        } else if score >= self.guarded_at {
            ReleaseRiskLevel::Guarded
        } else {
            ReleaseRiskLevel::Low
        }
    }
}

/// Weights used to aggregate a release risk score.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ReleaseRiskWeights {
    /// Weight for probe-pack failure ratio.
    pub probe_failures: f64,
    /// Weight for mode-differential drift failure ratio.
    pub drift_failures: f64,
    /// Weight for observatory regression ratio.
    pub observatory_regressions: f64,
    /// Weight for incidents observed in the last 7 days.
    pub incidents_7d: f64,
    /// Weight for incidents observed in the last 30 days.
    pub incidents_30d: f64,
}

impl Default for ReleaseRiskWeights {
    fn default() -> Self {
        Self {
            probe_failures: 0.35,
            drift_failures: 0.25,
            observatory_regressions: 0.20,
            incidents_7d: 0.15,
            incidents_30d: 0.05,
        }
    }
}

/// Input signals used to compute release risk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseRiskInput {
    /// Probe-pack failures.
    pub probe_failures: usize,
    /// Total probes in the pack.
    pub probe_total: usize,
    /// Mode-differential pairs that failed thresholds.
    pub drift_failed_pairs: usize,
    /// Total compared differential pairs.
    pub drift_total_pairs: usize,
    /// Observatory comparisons marked as likely regressions.
    pub observatory_regressions: usize,
    /// Total observatory comparisons.
    pub observatory_total_samples: usize,
    /// Incident count for the last 7 days.
    pub incident_count_7d: usize,
    /// Incident count for the last 30 days.
    pub incident_count_30d: usize,
}

/// Component-level breakdown for a release risk score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseRiskBreakdown {
    /// Probe failure component in [0.0, 1.0].
    pub probe_failure_ratio: f64,
    /// Drift failure component in [0.0, 1.0].
    pub drift_failure_ratio: f64,
    /// Observatory regression component in [0.0, 1.0].
    pub observatory_regression_ratio: f64,
    /// Last-7-day incident component in [0.0, 1.0].
    pub incident_pressure_7d: f64,
    /// Last-30-day incident component in [0.0, 1.0].
    pub incident_pressure_30d: f64,
}

/// Final release risk assessment for one candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseRiskAssessment {
    /// Aggregate normalized score in [0.0, 1.0].
    pub score: f64,
    /// Risk level derived from thresholds.
    pub level: ReleaseRiskLevel,
    /// Whether escalation should block or gate rollout.
    pub requires_escalation: bool,
    /// Human-readable escalation reasons.
    pub escalation_reasons: Vec<String>,
    /// Component-level score breakdown.
    pub breakdown: ReleaseRiskBreakdown,
}

/// Compact snapshot for one release candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseCandidateRiskSnapshot {
    /// Candidate identifier (for example `rc-2026-05-06.1`).
    pub candidate_id: String,
    /// Risk score for this candidate.
    pub risk_score: f64,
    /// Classified risk level.
    pub risk_level: ReleaseRiskLevel,
    /// Whether this candidate requires escalation.
    pub requires_escalation: bool,
    /// Incident count observed during the 7-day lookback.
    pub incident_count_7d: usize,
    /// Observatory regressions observed for this candidate.
    pub observatory_regressions: usize,
}

/// Direction of risk movement between adjacent candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseTrendDirection {
    /// Risk score moved down by a material amount.
    Improving,
    /// Risk score remained effectively flat.
    Stable,
    /// Risk score moved up by a material amount.
    Degrading,
}

/// One trend row in a release-candidate risk timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseTrendPoint {
    /// Candidate identifier.
    pub candidate_id: String,
    /// Candidate risk score.
    pub risk_score: f64,
    /// Score delta from previous candidate (0.0 for first point).
    pub risk_delta: f64,
    /// Candidate risk level.
    pub risk_level: ReleaseRiskLevel,
    /// Whether this candidate requires escalation.
    pub requires_escalation: bool,
    /// Trend direction from the previous candidate.
    pub trend: ReleaseTrendDirection,
}

/// Aggregate trend report across release candidates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseTrendReport {
    /// Ordered trend points.
    pub points: Vec<ReleaseTrendPoint>,
    /// Number of consecutive degrading steps ending at the latest candidate.
    pub degrading_streak: usize,
    /// Whether the trend indicates escalation pressure.
    pub requires_escalation: bool,
}

/// Build release-risk input from existing Charon reports plus incident counts.
///
/// # Example
///
/// ```rust,no_run
/// use stygian_charon::release_risk_input_from_reports;
///
/// # let probe_report = todo!("probe report");
/// # let differential_report = todo!("differential report");
/// # let observatory_report = todo!("observatory report");
/// let input = release_risk_input_from_reports(
///     &probe_report,
///     &differential_report,
///     &observatory_report,
///     1,
///     2,
/// );
/// assert!(input.probe_total >= input.probe_failures);
/// ```
#[must_use]
pub fn release_risk_input_from_reports(
    probe_report: &ProbePackReport,
    differential_report: &ModeDifferentialRunReport,
    observatory_report: &ObservatoryReport,
    incident_count_7d: usize,
    incident_count_30d: usize,
) -> ReleaseRiskInput {
    let observatory_regressions = observatory_report
        .comparisons
        .iter()
        .filter(|comparison| comparison.recommended_action == "investigate_regression")
        .count();

    ReleaseRiskInput {
        probe_failures: probe_report.failed,
        probe_total: probe_report.total,
        drift_failed_pairs: differential_report.failing_pairs,
        drift_total_pairs: differential_report.pair_results.len(),
        observatory_regressions,
        observatory_total_samples: observatory_report.comparisons.len(),
        incident_count_7d,
        incident_count_30d,
    }
}

/// Compute a release risk assessment from normalized signals.
///
/// # Example
///
/// ```rust
/// use stygian_charon::ReleaseRiskInput;
/// use stygian_charon::assess_release_risk;
///
/// let assessment = assess_release_risk(
///     &ReleaseRiskInput {
///         probe_failures: 1,
///         probe_total: 10,
///         drift_failed_pairs: 0,
///         drift_total_pairs: 4,
///         observatory_regressions: 1,
///         observatory_total_samples: 4,
///         incident_count_7d: 0,
///         incident_count_30d: 1,
///     },
///     None,
///     None,
/// );
/// assert!((0.0..=1.0).contains(&assessment.score));
/// ```
#[must_use]
pub fn assess_release_risk(
    input: &ReleaseRiskInput,
    thresholds: Option<ReleaseRiskThresholds>,
    weights: Option<ReleaseRiskWeights>,
) -> ReleaseRiskAssessment {
    let thresholds = thresholds.unwrap_or_default();
    let weights = weights.unwrap_or_default();

    let breakdown = ReleaseRiskBreakdown {
        probe_failure_ratio: ratio(input.probe_failures, input.probe_total),
        drift_failure_ratio: ratio(input.drift_failed_pairs, input.drift_total_pairs),
        observatory_regression_ratio: ratio(
            input.observatory_regressions,
            input.observatory_total_samples,
        ),
        incident_pressure_7d: scaled_incident_pressure(input.incident_count_7d, 3),
        incident_pressure_30d: scaled_incident_pressure(input.incident_count_30d, 10),
    };

    let raw_score = breakdown.probe_failure_ratio * weights.probe_failures
        + breakdown.drift_failure_ratio * weights.drift_failures
        + breakdown.observatory_regression_ratio * weights.observatory_regressions
        + breakdown.incident_pressure_7d * weights.incidents_7d
        + breakdown.incident_pressure_30d * weights.incidents_30d;

    let score = raw_score.clamp(0.0, 1.0);
    let level = thresholds.classify(score);

    let mut escalation_reasons = Vec::new();

    if matches!(level, ReleaseRiskLevel::Critical) {
        escalation_reasons.push("aggregate score reached critical threshold".to_string());
    }
    if breakdown.probe_failure_ratio >= 0.10 {
        escalation_reasons.push("probe pack failure ratio is at least 10%".to_string());
    }
    if breakdown.drift_failure_ratio >= 0.20 {
        escalation_reasons.push("drift threshold failures are at least 20%".to_string());
    }
    if breakdown.observatory_regression_ratio >= 0.25 {
        escalation_reasons.push("observatory regression ratio is at least 25%".to_string());
    }
    if input.incident_count_7d >= 3 {
        escalation_reasons.push("incident count in last 7 days is at least 3".to_string());
    }

    ReleaseRiskAssessment {
        score,
        level,
        requires_escalation: !escalation_reasons.is_empty(),
        escalation_reasons,
        breakdown,
    }
}

/// Build an ordered release-candidate trend report from candidate snapshots.
///
/// # Example
///
/// ```rust
/// use stygian_charon::{
///     ReleaseCandidateRiskSnapshot, ReleaseRiskLevel, build_release_trend_report,
/// };
///
/// let trend = build_release_trend_report(&[
///     ReleaseCandidateRiskSnapshot {
///         candidate_id: "rc1".to_string(),
///         risk_score: 0.20,
///         risk_level: ReleaseRiskLevel::Low,
///         requires_escalation: false,
///         incident_count_7d: 0,
///         observatory_regressions: 0,
///     },
///     ReleaseCandidateRiskSnapshot {
///         candidate_id: "rc2".to_string(),
///         risk_score: 0.35,
///         risk_level: ReleaseRiskLevel::Guarded,
///         requires_escalation: false,
///         incident_count_7d: 1,
///         observatory_regressions: 1,
///     },
/// ]);
/// assert_eq!(trend.points.len(), 2);
/// ```
#[must_use]
pub fn build_release_trend_report(
    candidates: &[ReleaseCandidateRiskSnapshot],
) -> ReleaseTrendReport {
    let mut points = Vec::with_capacity(candidates.len());

    let mut previous_score: Option<f64> = None;
    for candidate in candidates {
        let risk_delta = previous_score.map_or(0.0, |previous| candidate.risk_score - previous);
        let trend = classify_trend_delta(risk_delta);

        points.push(ReleaseTrendPoint {
            candidate_id: candidate.candidate_id.clone(),
            risk_score: candidate.risk_score,
            risk_delta,
            risk_level: candidate.risk_level,
            requires_escalation: candidate.requires_escalation,
            trend,
        });

        previous_score = Some(candidate.risk_score);
    }

    let degrading_streak = trailing_degrading_streak(&points);
    let latest_requires_escalation = points
        .last()
        .is_some_and(|latest| latest.requires_escalation);

    ReleaseTrendReport {
        points,
        degrading_streak,
        requires_escalation: latest_requires_escalation || degrading_streak >= 3,
    }
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn scaled_incident_pressure(incidents: usize, saturation_point: usize) -> f64 {
    if saturation_point == 0 {
        return 0.0;
    }

    (incidents as f64 / saturation_point as f64).clamp(0.0, 1.0)
}

fn classify_trend_delta(risk_delta: f64) -> ReleaseTrendDirection {
    if risk_delta >= 0.03 {
        ReleaseTrendDirection::Degrading
    } else if risk_delta <= -0.03 {
        ReleaseTrendDirection::Improving
    } else {
        ReleaseTrendDirection::Stable
    }
}

fn trailing_degrading_streak(points: &[ReleaseTrendPoint]) -> usize {
    let mut streak = 0_usize;

    for point in points.iter().rev() {
        if point.trend == ReleaseTrendDirection::Degrading {
            streak = streak.saturating_add(1);
        } else {
            break;
        }
    }

    streak
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assess_release_risk_sets_escalation_reasons_for_threshold_breaches() {
        let assessment = assess_release_risk(
            &ReleaseRiskInput {
                probe_failures: 3,
                probe_total: 10,
                drift_failed_pairs: 2,
                drift_total_pairs: 8,
                observatory_regressions: 2,
                observatory_total_samples: 6,
                incident_count_7d: 3,
                incident_count_30d: 7,
            },
            None,
            None,
        );

        assert!(assessment.requires_escalation);
        assert!(!assessment.escalation_reasons.is_empty());
        assert!((0.0..=1.0).contains(&assessment.score));
    }

    #[test]
    fn assess_release_risk_is_low_for_clean_inputs() {
        let assessment = assess_release_risk(
            &ReleaseRiskInput {
                probe_failures: 0,
                probe_total: 10,
                drift_failed_pairs: 0,
                drift_total_pairs: 8,
                observatory_regressions: 0,
                observatory_total_samples: 6,
                incident_count_7d: 0,
                incident_count_30d: 0,
            },
            None,
            None,
        );

        assert_eq!(assessment.level, ReleaseRiskLevel::Low);
        assert!(!assessment.requires_escalation);
        assert!(assessment.escalation_reasons.is_empty());
    }

    #[test]
    fn release_trend_report_tracks_degrading_streak() {
        let report = build_release_trend_report(&[
            ReleaseCandidateRiskSnapshot {
                candidate_id: "rc1".to_string(),
                risk_score: 0.20,
                risk_level: ReleaseRiskLevel::Low,
                requires_escalation: false,
                incident_count_7d: 0,
                observatory_regressions: 0,
            },
            ReleaseCandidateRiskSnapshot {
                candidate_id: "rc2".to_string(),
                risk_score: 0.28,
                risk_level: ReleaseRiskLevel::Low,
                requires_escalation: false,
                incident_count_7d: 0,
                observatory_regressions: 0,
            },
            ReleaseCandidateRiskSnapshot {
                candidate_id: "rc3".to_string(),
                risk_score: 0.34,
                risk_level: ReleaseRiskLevel::Guarded,
                requires_escalation: false,
                incident_count_7d: 1,
                observatory_regressions: 1,
            },
            ReleaseCandidateRiskSnapshot {
                candidate_id: "rc4".to_string(),
                risk_score: 0.40,
                risk_level: ReleaseRiskLevel::Guarded,
                requires_escalation: false,
                incident_count_7d: 1,
                observatory_regressions: 1,
            },
        ]);

        assert_eq!(report.points.len(), 4);
        assert_eq!(report.degrading_streak, 3);
        assert!(report.requires_escalation);
    }
}
