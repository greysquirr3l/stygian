//! Reliability scorer: turns an [`ExtractionResult`] into a [`ReliabilityScore`].

use crate::domain::{ExtractionMetadata, ExtractionResult};
use serde::{Deserialize, Serialize};

use super::score::{ReliabilityBand, ReliabilityScore, clamp_unit};

/// Weight applied to each sub-score when computing the [`ReliabilityScore`].
///
/// Defaults are tuned for production scraping where missing data is worse
/// than occasional retries: schema completeness dominates, transformations
/// matter but are less impactful than missing fields, and retries only
/// subtract.
///
/// # Example
///
/// ```
/// use stygian_plugin::reliability::ScoringWeights;
///
/// let weights = ScoringWeights {
///     schema: 0.5,
///     transformation: 0.3,
///     retry: 0.2,
/// };
/// assert!(weights.validate().is_ok());
///
/// // Out-of-range weights are rejected.
/// let bad = ScoringWeights { schema: 1.5, transformation: 0.0, retry: 0.0 };
/// assert!(bad.validate().is_err());
/// ```
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ScoringWeights {
    /// Weight for `schema_completeness` (must be in `[0.0, 1.0]`).
    pub schema: f32,

    /// Weight for `transformation_success` (must be in `[0.0, 1.0]`).
    pub transformation: f32,

    /// Weight for `retry_penalty` (subtracted; must be in `[0.0, 1.0]`).
    pub retry: f32,
}

impl ScoringWeights {
    /// Validate that every weight lies in `[0.0, 1.0]`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::PluginError::TemplateValidationError`] when
    /// any weight is outside `[0.0, 1.0]` or when any weight is NaN.
    pub fn validate(&self) -> crate::Result<()> {
        for (name, value) in [
            ("schema", self.schema),
            ("transformation", self.transformation),
            ("retry", self.retry),
        ] {
            if value.is_nan() || !(0.0..=1.0).contains(&value) {
                return Err(crate::error::PluginError::TemplateValidationError(format!(
                    "scoring weight '{name}' must be in [0.0, 1.0], got {value}"
                )));
            }
        }
        Ok(())
    }
}

impl Default for ScoringWeights {
    fn default() -> Self {
        Self {
            schema: 0.70,
            transformation: 0.30,
            retry: 0.10,
        }
    }
}

/// Computes a [`ReliabilityScore`] for an [`ExtractionResult`].
///
/// The scorer is pure and deterministic — it derives every sub-score from
/// the result's metadata plus an externally-supplied retry count. There is
/// no I/O and no clock dependency.
///
/// # Example
///
/// ```
/// use stygian_plugin::domain::{ExtractionResult, IdempotencyKey, RegionStatus};
/// use stygian_plugin::reliability::ReliabilityScorer;
/// use std::collections::HashMap;
///
/// let mut result = ExtractionResult::new(IdempotencyKey::new());
/// result.metadata.region_status.insert(
///     "title".to_string(),
///     RegionStatus { success: true, matched_count: 1, error: None },
/// );
///
/// let score = ReliabilityScorer::new().score_extraction(&result, 0);
/// assert!((score.overall - 1.0).abs() < f32::EPSILON);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct ReliabilityScorer {
    weights: ScoringWeights,
}

impl Default for ReliabilityScorer {
    fn default() -> Self {
        Self::new()
    }
}

impl ReliabilityScorer {
    /// Maximum retry count the `retry_penalty` sub-score saturates at.
    ///
    /// Beyond this many retries the `retry_penalty` sub-score is `1.0`
    /// regardless of the actual count, so a single retry-bloated call can
    /// never zero out the rest of the score on its own.
    pub const MAX_RETRIES_FOR_PENALTY: u32 = 5;

    /// Build a scorer with the default [`ScoringWeights`].
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_plugin::reliability::ReliabilityScorer;
    /// let _scorer = ReliabilityScorer::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            weights: ScoringWeights::default(),
        }
    }

    /// Build a scorer with custom weights.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::PluginError::TemplateValidationError`] when
    /// any weight is outside `[0.0, 1.0]` (see [`ScoringWeights::validate`]).
    pub fn with_weights(weights: ScoringWeights) -> crate::Result<Self> {
        weights.validate()?;
        Ok(Self { weights })
    }

    /// Score an extraction result. `retry_count` is the number of retries
    /// the caller had to take to produce this result.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_plugin::domain::{ExtractionResult, IdempotencyKey, RegionStatus};
    /// use stygian_plugin::reliability::{ReliabilityScorer, ReliabilityBand};
    /// use std::collections::HashMap;
    ///
    /// let mut result = ExtractionResult::new(IdempotencyKey::new());
    /// result.metadata.region_status.insert(
    ///     "title".to_string(),
    ///     RegionStatus { success: false, matched_count: 0, error: Some("missing".into()) },
    /// );
    ///
    /// let score = ReliabilityScorer::new().score_extraction(&result, 0);
    /// assert_eq!(score.band, ReliabilityBand::Low);
    /// ```
    #[must_use]
    pub fn score_extraction(
        &self,
        result: &ExtractionResult,
        retry_count: u32,
    ) -> ReliabilityScore {
        self.score_metadata(&result.metadata, retry_count)
    }

    /// Score from raw [`ExtractionMetadata`] without needing the full result.
    ///
    /// Useful when the metadata has been serialized over the wire (e.g.
    /// into a MCP `debug` payload) and the caller only has the metadata.
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        reason = "region counts are small enough to be safe as f32"
    )]
    pub fn score_metadata(
        &self,
        metadata: &ExtractionMetadata,
        retry_count: u32,
    ) -> ReliabilityScore {
        let total = metadata.region_status.len();
        let successful = metadata
            .region_status
            .values()
            .filter(|s| s.success)
            .count();

        let schema_completeness = if total == 0 {
            1.0
        } else {
            successful as f32 / total as f32
        };

        let transformation_success = if total == 0 {
            1.0
        } else {
            let transformation_failures = metadata
                .errors
                .iter()
                .filter(|msg| is_transformation_error(msg))
                .count();
            let bounded_failures = transformation_failures.min(total);
            1.0 - (bounded_failures as f32 / total as f32)
        };

        let retry_penalty = if retry_count == 0 {
            0.0
        } else {
            let capped = retry_count.min(Self::MAX_RETRIES_FOR_PENALTY) as f32;
            capped / Self::MAX_RETRIES_FOR_PENALTY as f32
        };

        let weighted = schema_completeness * self.weights.schema
            + transformation_success * self.weights.transformation
            - retry_penalty * self.weights.retry;
        let overall = clamp_unit(weighted);

        let reasons = build_reasons(
            schema_completeness,
            transformation_success,
            retry_penalty,
            total,
            successful,
        );

        ReliabilityScore {
            overall,
            schema_completeness,
            transformation_success,
            retry_penalty,
            band: ReliabilityBand::from_overall(overall),
            reasons,
        }
    }
}

/// Heuristic: a `Region 'X': ...` error message that mentions a
/// transformation keyword is treated as a transformation failure.
fn is_transformation_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("transformation")
        || lower.contains("regex")
        || lower.contains("coerce")
        || lower.contains("filter")
}

/// Build the per-candidate human-readable reasons that contributed to a score.
#[must_use]
fn build_reasons(
    schema_completeness: f32,
    transformation_success: f32,
    retry_penalty: f32,
    total: usize,
    successful: usize,
) -> Vec<String> {
    let mut reasons = Vec::new();
    if total == 0 {
        reasons.push("no regions defined (vacuously complete)".to_string());
    } else {
        reasons.push(format!(
            "{successful}/{total} regions succeeded ({:.0}%)",
            schema_completeness * 100.0
        ));
    }
    if transformation_success < 1.0 && total > 0 {
        reasons.push(format!(
            "{:.0}% transformation success",
            transformation_success * 100.0
        ));
    }
    if retry_penalty > 0.0 {
        reasons.push(format!(
            "retry penalty applied ({:.0}%)",
            retry_penalty * 100.0
        ));
    }
    reasons
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
    use crate::domain::{IdempotencyKey, RegionStatus};
    use std::collections::HashMap;

    fn region_status(success: bool) -> RegionStatus {
        RegionStatus {
            success,
            matched_count: usize::from(success),
            error: if success {
                None
            } else {
                Some("selector matched no elements".to_string())
            },
        }
    }

    #[test]
    fn test_empty_metadata_scores_as_high() {
        let metadata = ExtractionMetadata {
            idempotency_key: IdempotencyKey::new(),
            completed_at: chrono::Utc::now(),
            elapsed_ms: 0,
            selector_success_rate: 100.0,
            region_status: HashMap::new(),
            errors: vec![],
            reliability: None,
        };
        let score = ReliabilityScorer::new().score_metadata(&metadata, 0);
        assert!((score.overall - 1.0).abs() < f32::EPSILON);
        assert_eq!(score.band, ReliabilityBand::High);
        assert!(
            score
                .reasons
                .iter()
                .any(|r| r.contains("vacuously complete")),
            "empty template should report vacuous completeness"
        );
    }

    #[test]
    fn test_complete_extraction_scores_high() {
        let mut metadata = ExtractionMetadata {
            idempotency_key: IdempotencyKey::new(),
            completed_at: chrono::Utc::now(),
            elapsed_ms: 0,
            selector_success_rate: 100.0,
            region_status: HashMap::new(),
            errors: vec![],
            reliability: None,
        };
        metadata
            .region_status
            .insert("title".to_string(), region_status(true));
        metadata
            .region_status
            .insert("price".to_string(), region_status(true));
        let score = ReliabilityScorer::new().score_metadata(&metadata, 0);
        assert!((score.overall - 1.0).abs() < f32::EPSILON);
        assert_eq!(score.band, ReliabilityBand::High);
        assert!(score.reasons.iter().any(|r| r.contains("2/2")));
    }

    #[test]
    fn test_partial_extraction_scores_medium() {
        let mut metadata = ExtractionMetadata {
            idempotency_key: IdempotencyKey::new(),
            completed_at: chrono::Utc::now(),
            elapsed_ms: 0,
            selector_success_rate: 50.0,
            region_status: HashMap::new(),
            errors: vec![],
            reliability: None,
        };
        metadata
            .region_status
            .insert("title".to_string(), region_status(true));
        metadata
            .region_status
            .insert("price".to_string(), region_status(false));
        let score = ReliabilityScorer::new().score_metadata(&metadata, 0);
        assert!(score.overall < 1.0);
        assert!(score.overall >= 0.5);
        assert_eq!(score.band, ReliabilityBand::Medium);
        assert!((score.schema_completeness - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_failed_extraction_scores_low() {
        let mut metadata = ExtractionMetadata {
            idempotency_key: IdempotencyKey::new(),
            completed_at: chrono::Utc::now(),
            elapsed_ms: 0,
            selector_success_rate: 0.0,
            region_status: HashMap::new(),
            errors: vec![],
            reliability: None,
        };
        metadata
            .region_status
            .insert("title".to_string(), region_status(false));
        metadata
            .region_status
            .insert("price".to_string(), region_status(false));
        let score = ReliabilityScorer::new().score_metadata(&metadata, 0);
        assert!(score.overall < 0.5);
        assert_eq!(score.band, ReliabilityBand::Low);
        assert!((score.schema_completeness - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_transformation_failure_reduces_sub_score() {
        let mut metadata = ExtractionMetadata {
            idempotency_key: IdempotencyKey::new(),
            completed_at: chrono::Utc::now(),
            elapsed_ms: 0,
            selector_success_rate: 100.0,
            region_status: HashMap::new(),
            errors: vec!["Region 'price': transformation failed".to_string()],
            reliability: None,
        };
        metadata
            .region_status
            .insert("price".to_string(), region_status(true));
        let score = ReliabilityScorer::new().score_metadata(&metadata, 0);
        // Schema still 1.0 (region "succeeded" but transformation sub-score 0.0)
        assert!((score.schema_completeness - 1.0).abs() < f32::EPSILON);
        assert!(score.transformation_success < 1.0);
    }

    #[test]
    fn test_retry_penalty_reduces_overall() {
        let metadata = ExtractionMetadata {
            idempotency_key: IdempotencyKey::new(),
            completed_at: chrono::Utc::now(),
            elapsed_ms: 0,
            selector_success_rate: 100.0,
            region_status: HashMap::new(),
            errors: vec![],
            reliability: None,
        };
        let no_retry = ReliabilityScorer::new().score_metadata(&metadata, 0);
        let max_retries = ReliabilityScorer::new().score_metadata(&metadata, 99);
        assert!(
            max_retries.overall < no_retry.overall,
            "retries must lower the overall score (no_retry={}, max_retries={})",
            no_retry.overall,
            max_retries.overall
        );
        assert!((max_retries.retry_penalty - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_custom_weights_override_defaults() {
        let weights = ScoringWeights {
            schema: 0.0,
            transformation: 1.0,
            retry: 0.0,
        };
        let scorer = ReliabilityScorer::with_weights(weights).unwrap();
        let mut metadata = ExtractionMetadata {
            idempotency_key: IdempotencyKey::new(),
            completed_at: chrono::Utc::now(),
            elapsed_ms: 0,
            selector_success_rate: 100.0,
            region_status: HashMap::new(),
            errors: vec!["Region 'price': transformation failed".to_string()],
            reliability: None,
        };
        // Need at least one region for transformation_success to be
        // meaningful — when total == 0, transformation_success is vacuously 1.0.
        metadata
            .region_status
            .insert("price".to_string(), region_status(true));
        let score = scorer.score_metadata(&metadata, 0);
        // schema weight 0 → schema_completeness doesn't contribute;
        // transformation weight 1 → transformation_success is the whole score.
        assert!(
            score.overall < 1.0,
            "transformation failure must lower the overall score (got {})",
            score.overall
        );
        assert!(
            (score.transformation_success - 0.0).abs() < f32::EPSILON,
            "transformation_success should be 0.0 with one error and one region"
        );
    }

    #[test]
    fn test_invalid_weights_rejected() {
        let bad = ScoringWeights {
            schema: 1.5,
            transformation: 0.0,
            retry: 0.0,
        };
        assert!(ReliabilityScorer::with_weights(bad).is_err());

        let nan = ScoringWeights {
            schema: f32::NAN,
            transformation: 0.0,
            retry: 0.0,
        };
        assert!(ReliabilityScorer::with_weights(nan).is_err());
    }

    #[test]
    fn test_is_transformation_error_heuristic() {
        assert!(is_transformation_error(
            "Region 'price': transformation failed"
        ));
        assert!(is_transformation_error("Invalid regex pattern"));
        assert!(is_transformation_error("Cannot coerce value"));
        assert!(is_transformation_error("Filter rejected the value"));
        assert!(!is_transformation_error("No elements matched"));
        assert!(!is_transformation_error("selector parse error"));
    }
}
