//! Scoring logic for [`TransportProfile`] × [`TransportObservation`].
//!
//! The [`score`] function is the single entry point that produces a
//! [`TransportRealismReport`] from a profile + observation pair. The
//! report carries a [`TransportCompatibility`] (the headline score,
//! confidence, and coverage markers) plus the structured mismatch
//! list the caller can attach to downstream telemetry.
//!
//! The function is fully deterministic — no I/O, no clock reads —
//! so unit tests can exercise the full state space without spinning
//! up a browser.

use serde::{Deserialize, Serialize};

use crate::tls_validation::compare_http2_settings;

use super::observations::{compare_header_order, TransportObservation};
use super::profile::TransportProfile;

/// Number of HTTP/2 checks the [`TransportProfile`][super::TransportProfile] supports.
///
/// Kept as a const so callers can pre-allocate fixed-size result
/// vectors and so the scoring function is easy to reason about.
pub const HTTP2_CHECK_KIND_COUNT: usize = 3;

/// Stable identifier for a single HTTP/2 check.
///
/// Used in the [`TransportCompatibility::checks`][super::TransportCompatibility::checks]
/// section so downstream telemetry can attribute scores to a
/// specific check kind without depending on enum-variant order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Http2CheckKind {
    /// HTTP/2 SETTINGS frame fingerprint.
    Settings,
    /// HTTP/2 pseudo-header order (`:method`/`:authority`/`:scheme`/`:path`).
    PseudoHeaderOrder,
    /// HTTP/2 regular header order (after pseudo-headers).
    HeaderOrder,
}

impl Http2CheckKind {
    /// Stable string label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Settings => "http2_settings",
            Self::PseudoHeaderOrder => "http2_pseudo_header_order",
            Self::HeaderOrder => "http2_header_order",
        }
    }
}

/// Per-check result attached to a [`TransportCompatibility`][super::TransportCompatibility].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Http2CheckResult {
    /// Which check was evaluated.
    pub kind: Http2CheckKind,
    /// `true` when the observation matched the reference.
    pub matched: bool,
    /// Sub-score in `[0.0, 1.0]`. `0.0` when the observation was not
    /// supplied.
    pub score: f64,
    /// Number of items (settings / pseudo-headers / headers) the
    /// observation supplied.
    pub observed_count: usize,
    /// Number of items the reference carried.
    pub expected_count: usize,
    /// Stable position-match ratio in `[0.0, 1.0]`.
    pub position_match_ratio: f64,
}

/// Per-target compatibility score with confidence/coverage markers.
///
/// # Example
///
/// ```
/// use stygian_browser::transport_realism::{
///     score, TransportObservation, TransportProfile,
/// };
///
/// let profile = TransportProfile::default();
/// let observation = TransportObservation::chrome_136_reference();
/// let report = score(&profile, &observation);
/// assert!(report.compatibility.is_high_confidence());
/// assert!(report.compatibility.is_well_covered());
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransportCompatibility {
    /// Per-check breakdown.
    pub checks: Vec<Http2CheckResult>,
    /// Per-target compatibility score in `[0.0, 1.0]`.
    pub score: f64,
    /// Confidence marker in `[0.0, 1.0]`. Reflects how reliable
    /// the score is given the supplied observations and the
    /// profile's expectations. `0.0` when no HTTP/2 observations
    /// were available.
    pub confidence: f64,
    /// Coverage marker in `[0.0, 1.0]`. Reflects what fraction of
    /// the profile's expected checks were actually observed.
    /// `0.0` when no HTTP/2 observations were available.
    pub coverage: f64,
    /// Number of checks that matched the reference.
    pub matched_count: usize,
    /// Total number of checks the profile expected.
    pub total_checks: usize,
    /// Human-readable mismatch descriptions (empty when all checks
    /// matched).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mismatches: Vec<String>,
}

impl TransportCompatibility {
    /// `true` when coverage is at least `0.5` (i.e. the observation
    /// captured at least half of the expected HTTP/2 checks).
    #[must_use]
    pub fn is_well_covered(&self) -> bool {
        self.coverage >= 0.5
    }

    /// `true` when confidence is at least `0.5` (i.e. the score is
    /// derived from enough observation signal to trust).
    #[must_use]
    pub fn is_high_confidence(&self) -> bool {
        self.confidence >= 0.5
    }

    /// `true` when score is at least `0.95` — a strong match.
    #[must_use]
    pub fn is_strong_match(&self) -> bool {
        self.score >= 0.95
    }
}

/// Top-level transport-realism report attached to acquisition
/// results.
///
/// Carries the [`TransportCompatibility`] score plus the originating
/// [`TransportProfile`][super::TransportProfile] identity so
/// downstream policy mapping (T83 / T85 / T89 / T93) can attribute
/// the score to a specific profile without re-parsing the
/// `AcquisitionRequest`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransportRealismReport {
    /// Profile name carried on the originating request (free-form).
    pub profile_name: String,
    /// Per-target compatibility score.
    pub compatibility: TransportCompatibility,
}

impl TransportRealismReport {
    /// `true` when the report represents a strong match against the
    /// profile.
    #[must_use]
    pub fn is_strong_match(&self) -> bool {
        self.compatibility.is_strong_match()
    }

    /// `true` when the report is derived from enough HTTP/2
    /// observation signal to be trusted.
    #[must_use]
    pub fn is_high_confidence(&self) -> bool {
        self.compatibility.is_high_confidence()
    }
}

/// Score the supplied `observation` against the `profile`.
///
/// Returns a deterministic [`TransportRealismReport`]. When `profile`
/// has at least one HTTP/2 expectation enabled and `observation`
/// carries no HTTP/2 signal, the function returns a report whose
/// `compatibility.score` is
/// [`super::DEFAULT_SCORE_WHEN_HTTP2_UNAVAILABLE`],
/// `compatibility.confidence` is
/// [`super::DEFAULT_CONFIDENCE_WHEN_HTTP2_UNAVAILABLE`], and
/// `compatibility.coverage` is
/// [`super::DEFAULT_COVERAGE_WHEN_HTTP2_UNAVAILABLE`]. The mismatch
/// list always carries a `"http2_observations_unavailable"` entry so
/// downstream automation can detect the "no signal" path
/// deterministically.
///
/// # Example
///
/// ```
/// use stygian_browser::transport_realism::{
///     score, TransportObservation, TransportProfile,
/// };
///
/// let profile = TransportProfile::default();
/// let observation = TransportObservation::chrome_136_reference();
/// let report = score(&profile, &observation);
/// assert!(report.compatibility.score > 0.95);
/// ```
#[must_use]
pub fn score(profile: &TransportProfile, observation: &TransportObservation) -> TransportRealismReport {
    let total_checks = profile.expected_http2_check_count();

    // Fast path: no HTTP/2 expectations enabled → report a perfect
    // score and full coverage (the profile explicitly opted out of
    // every check).
    if total_checks == 0 {
        return TransportRealismReport {
            profile_name: profile.name.clone(),
            compatibility: TransportCompatibility {
                checks: Vec::new(),
                score: 1.0,
                confidence: 1.0,
                coverage: 1.0,
                matched_count: 0,
                total_checks: 0,
                mismatches: Vec::new(),
            },
        };
    }

    // Fast path: no HTTP/2 observations were supplied at all. The
    // score collapses to the documented "no signal" defaults.
    if !observation.has_http2() {
        return TransportRealismReport {
            profile_name: profile.name.clone(),
            compatibility: TransportCompatibility {
                checks: Vec::new(),
                score: super::DEFAULT_SCORE_WHEN_HTTP2_UNAVAILABLE,
                confidence: super::DEFAULT_CONFIDENCE_WHEN_HTTP2_UNAVAILABLE,
                coverage: super::DEFAULT_COVERAGE_WHEN_HTTP2_UNAVAILABLE,
                matched_count: 0,
                total_checks,
                mismatches: vec!["http2_observations_unavailable".to_string()],
            },
        };
    }

    score_with_observations(profile, observation, total_checks)
}

/// Score when observations are available. Extracted so the top-level
/// `score` function stays under the clippy `too_many_lines` ceiling.
fn score_with_observations(
    profile: &TransportProfile,
    observation: &TransportObservation,
    total_checks: usize,
) -> TransportRealismReport {
    let mut state = ScoringState::default();

    if profile.expectations.contains(TransportProfile::SETTINGS) {
        score_settings_check(profile, observation, &mut state);
    }
    if profile
        .expectations
        .contains(TransportProfile::PSEUDO_HEADER_ORDER)
    {
        score_pseudo_header_check(profile, observation, &mut state);
    }
    if profile.expectations.contains(TransportProfile::HEADER_ORDER) {
        score_header_order_check(profile, observation, &mut state);
    }

    state.finalize(profile, total_checks)
}

/// Mutable accumulator for the scoring loop. Keeps `score` and its
/// helpers under the clippy line ceilings.
#[derive(Default)]
struct ScoringState {
    checks: Vec<Http2CheckResult>,
    mismatches: Vec<String>,
    observed_count: usize,
    matched_count: usize,
    sum_scores: f64,
    sum_weights: f64,
}

impl ScoringState {
    /// Record the SETTINGS-frame check result.
    fn record(&mut self, check: Http2CheckResult, weight: f64, observed: bool, matched: bool) {
        if observed {
            self.observed_count += 1;
        }
        if matched {
            self.matched_count += 1;
        }
        self.sum_scores = weight.mul_add(check.score, self.sum_scores);
        self.sum_weights += weight;
        self.checks.push(check);
    }

    /// Push a mismatch description.
    fn push_mismatch(&mut self, description: String) {
        self.mismatches.push(description);
    }

    /// Finalize the report.
    fn finalize(self, profile: &TransportProfile, total_checks: usize) -> TransportRealismReport {
        let observed_count = self.observed_count;
        let final_score = if self.sum_weights > 0.0 {
            (self.sum_scores / self.sum_weights).clamp(0.0, 1.0)
        } else {
            0.0
        };
        #[allow(clippy::cast_precision_loss)]
        let coverage = if total_checks == 0 {
            1.0
        } else {
            observed_count as f64 / total_checks as f64
        };
        let confidence = coverage.clamp(0.0, 1.0);

        let mut mismatches = self.mismatches;
        if profile.require_http2_observations && observed_count < total_checks {
            mismatches.push("require_http2_observations_unmet".to_string());
        }

        TransportRealismReport {
            profile_name: profile.name.clone(),
            compatibility: TransportCompatibility {
                checks: self.checks,
                score: round4(final_score),
                confidence: round4(confidence),
                coverage: round4(coverage),
                matched_count: self.matched_count,
                total_checks,
                mismatches,
            },
        }
    }
}

/// Score the HTTP/2 SETTINGS frame and push the result onto `state`.
fn score_settings_check(
    profile: &TransportProfile,
    observation: &TransportObservation,
    state: &mut ScoringState,
) {
    let (matched, check_score, observed_count_opt, position_ratio) =
        score_http2_settings(profile, observation);
    let observed = observed_count_opt.is_some();
    if !matched {
        state.push_mismatch(format!(
            "{}: fingerprint mismatch",
            Http2CheckKind::Settings.as_str()
        ));
    }
    state.record(
        Http2CheckResult {
            kind: Http2CheckKind::Settings,
            matched,
            score: check_score,
            observed_count: observed_count_opt.unwrap_or(0),
            expected_count: profile.expected_http2_settings.len(),
            position_match_ratio: position_ratio,
        },
        0.5,
        observed,
        matched,
    );
}

/// Score the HTTP/2 pseudo-header order and push the result.
fn score_pseudo_header_check(
    profile: &TransportProfile,
    observation: &TransportObservation,
    state: &mut ScoringState,
) {
    let Some(observed) = observation.http2_pseudo_header_order.as_deref() else {
        state.push_mismatch(format!(
            "{}: observation missing",
            Http2CheckKind::PseudoHeaderOrder.as_str()
        ));
        state.record(
            Http2CheckResult {
                kind: Http2CheckKind::PseudoHeaderOrder,
                matched: false,
                score: 0.0,
                observed_count: 0,
                expected_count: profile.expected_pseudo_header_order.len(),
                position_match_ratio: 0.0,
            },
            0.2,
            false,
            false,
        );
        return;
    };
    let m = compare_header_order(
        &profile
            .expected_pseudo_header_order
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        observed,
    );
    let matched = m.matched_positions == m.reference_length && m.reference_length > 0;
    let position_ratio = m.position_match_ratio();
    if !matched {
        state.push_mismatch(format!(
            "{}: order mismatch ({}/{} matched)",
            Http2CheckKind::PseudoHeaderOrder.as_str(),
            m.matched_positions,
            m.reference_length
        ));
    }
    state.record(
        Http2CheckResult {
            kind: Http2CheckKind::PseudoHeaderOrder,
            matched,
            score: position_ratio,
            observed_count: m.observed_length,
            expected_count: m.reference_length,
            position_match_ratio: position_ratio,
        },
        0.2,
        true,
        matched,
    );
}

/// Score the HTTP/2 regular header order and push the result.
fn score_header_order_check(
    profile: &TransportProfile,
    observation: &TransportObservation,
    state: &mut ScoringState,
) {
    let Some(observed) = observation.http2_header_order.as_deref() else {
        state.push_mismatch(format!(
            "{}: observation missing",
            Http2CheckKind::HeaderOrder.as_str()
        ));
        state.record(
            Http2CheckResult {
                kind: Http2CheckKind::HeaderOrder,
                matched: false,
                score: 0.0,
                observed_count: 0,
                expected_count: profile.expected_header_order.len(),
                position_match_ratio: 0.0,
            },
            0.3,
            false,
            false,
        );
        return;
    };
    let m = compare_header_order(
        &profile
            .expected_header_order
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        observed,
    );
    let matched = m.matched_positions == m.reference_length && m.reference_length > 0;
    let position_ratio = m.position_match_ratio();
    if !matched {
        state.push_mismatch(format!(
            "{}: order mismatch ({}/{} matched)",
            Http2CheckKind::HeaderOrder.as_str(),
            m.matched_positions,
            m.reference_length
        ));
    }
    state.record(
        Http2CheckResult {
            kind: Http2CheckKind::HeaderOrder,
            matched,
            score: position_ratio,
            observed_count: m.observed_length,
            expected_count: m.reference_length,
            position_match_ratio: position_ratio,
        },
        0.3,
        true,
        matched,
    );
}

/// Compare an observed HTTP/2 SETTINGS frame against the profile
/// reference and return `(matched, score, observed_count,
/// position_ratio)`.
///
/// The comparison delegates to the same logic
/// [`crate::tls_validation::compare_http2_settings`] uses for
/// matching the JA3 reference SETTINGS — this module reuses the
/// helper rather than duplicating the comparison rules.
fn score_http2_settings(
    profile: &TransportProfile,
    observation: &TransportObservation,
) -> (bool, f64, Option<usize>, f64) {
    let Some(observed) = observation.http2_settings.as_deref() else {
        return (false, 0.0, None, 0.0);
    };
    let (matched, issues) =
        compare_http2_settings(&profile.expected_http2_settings, observed);
    let position_ratio = if profile.expected_http2_settings.is_empty() {
        1.0
    } else {
        let expected_ids: std::collections::HashSet<u32> = profile
            .expected_http2_settings
            .iter()
            .map(|(id, _)| *id)
            .collect();
        let observed_ids: std::collections::HashSet<u32> =
            observed.iter().map(|(id, _)| *id).collect();
        let intersection = expected_ids.intersection(&observed_ids).count();
        #[allow(clippy::cast_precision_loss)]
        let ratio = intersection as f64 / expected_ids.len() as f64;
        ratio
    };
    let score = if matched {
        1.0
    } else {
        #[allow(clippy::cast_precision_loss)]
        let discount = (issues.len() as f64) * 0.1;
        (1.0 - discount).max(0.0)
    };
    (matched, round4(score), Some(observed.len()), round4(position_ratio))
}

/// Round to 4 decimal places to keep JSON serialisation deterministic
/// across platforms (different IEEE-754 implementations can produce
/// different representations of the same f64 otherwise).
fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls_validation::{CHROME_136_HTTP2_SETTINGS, CHROME_136_JA4};
    use crate::transport_realism::observations::{
        HEADER_ORDER_CHROME_136, PSEUDO_HEADER_ORDER_CHROME_136,
    };

    fn chrome_obs() -> TransportObservation {
        TransportObservation::chrome_136_reference()
    }

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn chrome_136_observation_against_chrome_136_profile_scores_high() {
        let profile = TransportProfile::default();
        let report = score(&profile, &chrome_obs());
        assert!(
            report.compatibility.score > 0.95,
            "chrome 136 reference vs chrome 136 observation should be a strong match, got: {}",
            report.compatibility.score
        );
        assert_eq!(report.compatibility.matched_count, 3);
        assert_eq!(report.compatibility.total_checks, 3);
        assert!(report.compatibility.mismatches.is_empty());
        assert!(report.compatibility.is_high_confidence());
        assert!(report.compatibility.is_well_covered());
        assert!(report.is_strong_match());
    }

    #[test]
    fn mismatched_settings_score_below_one() {
        let profile = TransportProfile::default();
        let observed = TransportObservation {
            http2_settings: Some(vec![(1, 1), (2, 0)]),
            ..chrome_obs()
        };
        let report = score(&profile, &observed);
        assert!(
            report.compatibility.score < 1.0,
            "settings mismatch must reduce score, got: {}",
            report.compatibility.score
        );
        assert!(
            report
                .compatibility
                .mismatches
                .iter()
                .any(|m| m.contains(Http2CheckKind::Settings.as_str())),
            "settings mismatch must be reported, got: {:?}",
            report.compatibility.mismatches
        );
    }

    #[test]
    fn mismatched_header_order_reduces_score() {
        let profile = TransportProfile::default();
        let observed = TransportObservation {
            http2_header_order: Some(vec!["host".into(), "accept".into()]),
            ..chrome_obs()
        };
        let report = score(&profile, &observed);
        assert!(report.compatibility.score < 1.0);
        assert!(
            report
                .compatibility
                .mismatches
                .iter()
                .any(|m| m.contains(Http2CheckKind::HeaderOrder.as_str())),
            "header order mismatch must be reported"
        );
    }

    #[test]
    fn missing_http2_observations_uses_known_default_markers() {
        let profile = TransportProfile::default();
        let report = score(&profile, &TransportObservation::default());
        assert!(
            approx_eq(
                report.compatibility.score,
                super::super::DEFAULT_SCORE_WHEN_HTTP2_UNAVAILABLE
            ),
            "score default mismatch, got: {}",
            report.compatibility.score
        );
        assert!(
            approx_eq(
                report.compatibility.confidence,
                super::super::DEFAULT_CONFIDENCE_WHEN_HTTP2_UNAVAILABLE
            ),
            "confidence default mismatch, got: {}",
            report.compatibility.confidence
        );
        assert!(
            approx_eq(
                report.compatibility.coverage,
                super::super::DEFAULT_COVERAGE_WHEN_HTTP2_UNAVAILABLE
            ),
            "coverage default mismatch, got: {}",
            report.compatibility.coverage
        );
        assert!(
            report
                .compatibility
                .mismatches
                .iter()
                .any(|m| m == "http2_observations_unavailable"),
            "missing observations must emit the deterministic mismatch tag, got: {:?}",
            report.compatibility.mismatches
        );
        assert!(!report.is_strong_match());
    }

    #[test]
    fn require_http2_observations_surfaces_partial_observation() {
        let profile = TransportProfile::default().with_require_http2_observations(true);
        let observed = TransportObservation {
            http2_settings: Some(CHROME_136_HTTP2_SETTINGS.to_vec()),
            ..TransportObservation::default()
        };
        let report = score(&profile, &observed);
        assert!(
            report
                .compatibility
                .mismatches
                .iter()
                .any(|m| m == "require_http2_observations_unmet"),
            "partial observation must surface unmet requirement, got: {:?}",
            report.compatibility.mismatches
        );
        assert!(report.compatibility.coverage < 1.0);
    }

    #[test]
    fn profile_with_no_expectations_reports_perfect_score() {
        let profile = TransportProfile::default().with_expectation_bits(0);
        let report = score(&profile, &TransportObservation::default());
        assert!(approx_eq(report.compatibility.score, 1.0));
        assert!(approx_eq(report.compatibility.confidence, 1.0));
        assert!(approx_eq(report.compatibility.coverage, 1.0));
        assert_eq!(report.compatibility.total_checks, 0);
    }

    #[test]
    fn profile_name_is_propagated_to_report() {
        let profile = TransportProfile::default().with_name("firefox-130");
        let report = score(&profile, &chrome_obs());
        assert_eq!(report.profile_name, "firefox-130");
    }

    #[test]
    fn references_used_in_tests_are_stable() {
        // Surface unexpected removal of the references this module
        // depends on as a compile-time test failure.
        assert!(CHROME_136_JA4.starts_with('t'));
        assert!(CHROME_136_HTTP2_SETTINGS.iter().any(|(id, _)| *id == 4));
        assert!(HEADER_ORDER_CHROME_136.contains(&"host"));
        assert!(PSEUDO_HEADER_ORDER_CHROME_136.contains(&":method"));
    }

    #[test]
    fn per_check_kind_results_carry_kind_label() {
        let profile = TransportProfile::default();
        let report = score(&profile, &chrome_obs());
        for result in &report.compatibility.checks {
            assert!(!result.kind.as_str().is_empty());
        }
    }
}
