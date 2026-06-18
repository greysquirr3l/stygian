//! Extraction reliability scoring
//!
//! Computes a 0.0–1.0 reliability score for [`ExtractionResult`] outputs so
//! fallback chains can optimize for *data quality*, not only fetch success.
//!
//! # Score components
//!
//! The score is the weighted sum of three sub-scores, clamped to `[0.0, 1.0]`:
//!
//! | Sub-score              | Default weight | Source                                           |
//! |------------------------|---------------:|--------------------------------------------------|
//! | `schema_completeness`  |           0.70 | `successful_regions / total_regions`             |
//! | `transformation_success`|          0.30 | `1 − transformation_failure_rate`                |
//! | `retry_penalty`        |           0.10 | `retry_count / max_retries` (clamped)            |
//!
//! `retry_penalty` is **subtracted** from the weighted sum — it represents a
//! quality discount for results that took many retries to produce.
//!
//! # Interpretation bands
//!
//! The continuous `overall` score is bucketed into three discrete bands so
//! callers can branch on a single [`ReliabilityBand`] value:
//!
//! | Band    | Score range   | Interpretation                                  |
//! |---------|---------------|-------------------------------------------------|
//! | `High`  | `0.85..=1.00` | Production-ready; trust the result.             |
//! | `Medium`| `0.50..0.85`  | Partial; treat as best-effort, retry if needed. |
//! | `Low`   | `0.00..0.50`  | Unreliable; prefer an alternative fallback.     |
//!
//! # Selection policy
//!
//! The [`ScoreWeightedSelector`] helper ranks a list of `(name, score)`
//! candidates and picks the highest-scoring one. Callers can apply a custom
//! [`ScoringWeights`] to tune the importance of each sub-score for their
//! target class.
//!
//! # Example
//!
//! ```
//! use stygian_plugin::domain::{ExtractionResult, IdempotencyKey};
//! use stygian_plugin::reliability::{ReliabilityScorer, ReliabilityBand};
//!
//! let result = ExtractionResult::new(IdempotencyKey::new());
//! let score = ReliabilityScorer::new().score_extraction(&result, 0);
//! assert_eq!(score.band, ReliabilityBand::High); // empty template -> vacuously complete
//! assert!((score.overall - 1.0).abs() < f32::EPSILON);
//! ```

pub mod score;
pub mod scorer;
pub mod selector;

pub use score::{ReliabilityBand, ReliabilityScore};
pub use scorer::{ReliabilityScorer, ScoringWeights};
pub use selector::{ScoreWeightedSelector, ScoredCandidate};