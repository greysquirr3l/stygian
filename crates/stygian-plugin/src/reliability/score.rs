//! `ReliabilityScore` value type and its discrete interpretation band.

use serde::{Deserialize, Serialize};

/// A 0.0–1.0 reliability score for an [`crate::domain::ExtractionResult`].
///
/// The score is the weighted sum of three sub-scores (see module-level docs
/// for the table) and is paired with a discrete [`ReliabilityBand`] so
/// callers can branch on coarse-grained quality without inspecting the
/// continuous `overall` field.
///
/// # Example
///
/// ```
/// use stygian_plugin::reliability::{ReliabilityScore, ReliabilityBand};
///
/// let score = ReliabilityScore {
///     overall: 0.92,
///     schema_completeness: 1.0,
///     transformation_success: 1.0,
///     retry_penalty: 0.0,
///     band: ReliabilityBand::High,
///     reasons: vec!["all regions extracted".to_string()],
/// };
/// assert_eq!(score.band, ReliabilityBand::High);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReliabilityScore {
    /// Weighted sum of the sub-scores, clamped to `[0.0, 1.0]`.
    pub overall: f32,

    /// Fraction of regions that produced data (0.0 = none, 1.0 = all).
    pub schema_completeness: f32,

    /// Fraction of regions whose transformations succeeded without error
    /// (0.0 = none, 1.0 = all).
    pub transformation_success: f32,

    /// Penalty for retries taken to produce this result
    /// (0.0 = no retries, 1.0 = capped retries).
    pub retry_penalty: f32,

    /// Discrete interpretation band.
    pub band: ReliabilityBand,

    /// Human-readable reasons that contributed to the score.
    ///
    /// Rendered as a list of short strings (one per contributing factor) so
    /// log lines, MCP `debug` payloads, and JSON output stay compact.
    pub reasons: Vec<String>,
}

impl ReliabilityScore {
    /// Lower bound of the `High` band (inclusive).
    pub const HIGH_THRESHOLD: f32 = 0.85;

    /// Lower bound of the `Medium` band (inclusive).
    pub const MEDIUM_THRESHOLD: f32 = 0.50;

    /// Construct a `ReliabilityScore` from a raw `overall` value, clamping
    /// it to `[0.0, 1.0]` and computing the matching band.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_plugin::reliability::{ReliabilityScore, ReliabilityBand};
    ///
    /// let score = ReliabilityScore::from_overall(0.93);
    /// assert_eq!(score.band, ReliabilityBand::High);
    ///
    /// let score = ReliabilityScore::from_overall(0.30);
    /// assert_eq!(score.band, ReliabilityBand::Low);
    /// ```
    #[must_use]
    pub fn from_overall(overall: f32) -> Self {
        let overall = clamp_unit(overall);
        Self {
            overall,
            schema_completeness: overall,
            transformation_success: overall,
            retry_penalty: 0.0,
            band: ReliabilityBand::from_overall(overall),
            reasons: Vec::new(),
        }
    }

    /// Attach human-readable reasons to this score, returning the modified
    /// value (consuming builder style).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_plugin::reliability::ReliabilityScore;
    ///
    /// let score = ReliabilityScore::from_overall(0.7).with_reasons(vec!["partial".into()]);
    /// assert_eq!(score.reasons, vec!["partial".to_string()]);
    /// ```
    #[must_use]
    pub fn with_reasons(mut self, reasons: Vec<String>) -> Self {
        self.reasons = reasons;
        self
    }
}

/// Discrete interpretation band for a [`ReliabilityScore`].
///
/// Boundaries: `Low` for `[0.0, 0.50)`, `Medium` for `[0.50, 0.85)`,
/// `High` for `[0.85, 1.00]`. The enum serializes as a lowercase string
/// (`"low"`, `"medium"`, `"high"`) so MCP and JSON consumers can branch
/// without parsing the float `overall` field.
///
/// # Example
///
/// ```
/// use stygian_plugin::reliability::ReliabilityBand;
///
/// assert_eq!(ReliabilityBand::from_overall(0.95), ReliabilityBand::High);
/// assert_eq!(ReliabilityBand::from_overall(0.60), ReliabilityBand::Medium);
/// assert_eq!(ReliabilityBand::from_overall(0.10), ReliabilityBand::Low);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReliabilityBand {
    /// Score in `[0.85, 1.00]` — production-ready.
    High,

    /// Score in `[0.50, 0.85)` — best-effort.
    Medium,

    /// Score in `[0.00, 0.50)` — unreliable; consider fallback.
    Low,
}

impl ReliabilityBand {
    /// Map a raw `overall` score to its [`ReliabilityBand`].
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_plugin::reliability::ReliabilityBand;
    ///
    /// assert_eq!(ReliabilityBand::from_overall(1.0), ReliabilityBand::High);
    /// assert_eq!(ReliabilityBand::from_overall(0.8499), ReliabilityBand::Medium);
    /// ```
    #[must_use]
    pub fn from_overall(overall: f32) -> Self {
        if overall >= ReliabilityScore::HIGH_THRESHOLD {
            Self::High
        } else if overall >= ReliabilityScore::MEDIUM_THRESHOLD {
            Self::Medium
        } else {
            Self::Low
        }
    }
}

impl std::fmt::Display for ReliabilityBand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::High => f.write_str("high"),
            Self::Medium => f.write_str("medium"),
            Self::Low => f.write_str("low"),
        }
    }
}

/// Clamp `value` to `[0.0, 1.0]`. NaN maps to `0.0`.
#[inline]
#[must_use]
pub(crate) fn clamp_unit(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < f32::EPSILON
    }

    #[test]
    fn test_from_overall_clamps_to_unit_interval() {
        assert!(approx_eq(
            ReliabilityScore::from_overall(-0.5).overall,
            0.0
        ));
        assert!(approx_eq(
            ReliabilityScore::from_overall(1.5).overall,
            1.0
        ));
        assert!(approx_eq(
            ReliabilityScore::from_overall(0.42).overall,
            0.42
        ));
    }

    #[test]
    fn test_from_overall_nan_maps_to_zero() {
        assert!(approx_eq(
            ReliabilityScore::from_overall(f32::NAN).overall,
            0.0
        ));
    }

    #[test]
    fn test_band_boundaries() {
        assert_eq!(
            ReliabilityBand::from_overall(ReliabilityScore::HIGH_THRESHOLD),
            ReliabilityBand::High
        );
        assert_eq!(
            ReliabilityBand::from_overall(ReliabilityScore::MEDIUM_THRESHOLD),
            ReliabilityBand::Medium
        );
        assert_eq!(
            ReliabilityBand::from_overall(ReliabilityScore::MEDIUM_THRESHOLD - 0.01),
            ReliabilityBand::Low
        );
    }

    #[test]
    fn test_band_serde_is_lowercase() {
        let json = serde_json::to_string(&ReliabilityBand::High).unwrap();
        assert_eq!(json, "\"high\"");
        let roundtrip: ReliabilityBand = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip, ReliabilityBand::High);
    }

    #[test]
    fn test_with_reasons_preserves_other_fields() {
        let mut score = ReliabilityScore::from_overall(0.6);
        score.schema_completeness = 0.5;
        score.transformation_success = 0.7;
        let updated = score.clone().with_reasons(vec!["a".into(), "b".into()]);
        assert_eq!(updated.reasons.len(), 2);
        assert!((updated.overall - score.overall).abs() < f32::EPSILON);
        assert!((updated.schema_completeness - 0.5).abs() < f32::EPSILON);
        assert!((updated.transformation_success - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn test_band_display() {
        assert_eq!(ReliabilityBand::High.to_string(), "high");
        assert_eq!(ReliabilityBand::Medium.to_string(), "medium");
        assert_eq!(ReliabilityBand::Low.to_string(), "low");
    }
}