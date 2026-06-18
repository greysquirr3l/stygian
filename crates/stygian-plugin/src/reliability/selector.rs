//! Score-weighted selection over a list of `(name, score)` candidates.
//!
//! The selector is the hook that lets a fallback chain (or any other
//! higher-level orchestrator) consult the reliability score. The pattern:
//!
//! 1. Each candidate runs its extraction and produces a
//!    [`ReliabilityScore`].
//! 2. The orchestrator hands all `(name, score)` pairs to
//!    [`ScoreWeightedSelector::pick_best`].
//! 3. The candidate with the highest `overall` score wins; ties are
//!    broken by the first registered candidate (preserving the registration
//!    order so callers retain control).
//!
//! The selector is pure and stateless — pass it into any orchestrator
//! without wiring concerns.

use serde::{Deserialize, Serialize};

use super::score::ReliabilityScore;

/// A `(name, score)` candidate for [`ScoreWeightedSelector`].
///
/// `name` is an arbitrary caller-supplied label (typically the chain entry
/// name or the candidate's URL). `score` is the [`ReliabilityScore`]
/// produced by the candidate's extraction.
///
/// # Example
///
/// ```
/// use stygian_plugin::reliability::{ReliabilityScore, ScoredCandidate};
///
/// let candidate = ScoredCandidate {
///     name: "primary".to_string(),
///     score: ReliabilityScore::from_overall(0.7),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredCandidate {
    /// Caller-supplied label (typically chain entry name).
    pub name: String,

    /// Reliability score produced by the candidate's extraction.
    pub score: ReliabilityScore,
}

impl ScoredCandidate {
    /// Construct a new candidate with the supplied name and score.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_plugin::reliability::{ReliabilityScore, ScoredCandidate};
    ///
    /// let candidate = ScoredCandidate::new("primary", ReliabilityScore::from_overall(0.9));
    /// assert_eq!(candidate.name, "primary");
    /// ```
    #[must_use]
    pub fn new(name: impl Into<String>, score: ReliabilityScore) -> Self {
        Self {
            name: name.into(),
            score,
        }
    }
}

/// Stateless selector that picks the highest-scoring [`ScoredCandidate`].
///
/// Ties are broken by registration order (the first candidate in the input
/// vector wins). An empty input returns `None`.
///
/// # Example
///
/// ```
/// use stygian_plugin::reliability::{ReliabilityScore, ScoreWeightedSelector, ScoredCandidate};
///
/// let candidates = vec![
///     ScoredCandidate::new("primary", ReliabilityScore::from_overall(0.6)),
///     ScoredCandidate::new("plugin", ReliabilityScore::from_overall(0.9)),
/// ];
/// let winner = ScoreWeightedSelector::pick_best(candidates).unwrap();
/// assert_eq!(winner.name, "plugin");
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct ScoreWeightedSelector;

impl ScoreWeightedSelector {
    /// Pick the highest-scoring candidate. Returns `None` for an empty input.
    ///
    /// The first candidate with the maximum score wins on ties — this
    /// preserves the registration order so callers retain deterministic
    /// control over tie-breaking.
    #[must_use]
    pub fn pick_best(candidates: Vec<ScoredCandidate>) -> Option<ScoredCandidate> {
        candidates.into_iter().reduce(|best, current| {
            if current.score.overall > best.score.overall {
                current
            } else {
                best
            }
        })
    }

    /// Pick the highest-scoring candidate by reference. Useful when the
    /// caller already owns the candidates and does not want to move them.
    #[must_use]
    pub fn pick_best_ref(candidates: &[ScoredCandidate]) -> Option<&ScoredCandidate> {
        candidates
            .iter()
            .reduce(|best, current| {
                if current.score.overall > best.score.overall {
                    current
                } else {
                    best
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pick_best_empty_input_returns_none() {
        assert!(ScoreWeightedSelector::pick_best(vec![]).is_none());
        let empty: Vec<ScoredCandidate> = vec![];
        assert!(ScoreWeightedSelector::pick_best_ref(&empty).is_none());
    }

    #[test]
    fn test_pick_best_single_candidate() {
        let candidates = vec![ScoredCandidate::new("only", ReliabilityScore::from_overall(0.5))];
        let winner = ScoreWeightedSelector::pick_best(candidates).unwrap();
        assert_eq!(winner.name, "only");
    }

    #[test]
    fn test_pick_best_picks_highest() {
        let candidates = vec![
            ScoredCandidate::new("low", ReliabilityScore::from_overall(0.3)),
            ScoredCandidate::new("high", ReliabilityScore::from_overall(0.9)),
            ScoredCandidate::new("mid", ReliabilityScore::from_overall(0.6)),
        ];
        let winner = ScoreWeightedSelector::pick_best(candidates).unwrap();
        assert_eq!(winner.name, "high");
    }

    #[test]
    fn test_pick_best_tie_broken_by_first() {
        let candidates = vec![
            ScoredCandidate::new("first", ReliabilityScore::from_overall(0.7)),
            ScoredCandidate::new("second", ReliabilityScore::from_overall(0.7)),
        ];
        let winner = ScoreWeightedSelector::pick_best(candidates).unwrap();
        assert_eq!(
            winner.name, "first",
            "ties must be broken by registration order"
        );
    }

    #[test]
    fn test_pick_best_ref_matches_pick_best() {
        let candidates = vec![
            ScoredCandidate::new("a", ReliabilityScore::from_overall(0.4)),
            ScoredCandidate::new("b", ReliabilityScore::from_overall(0.8)),
        ];
        let winner_ref = ScoreWeightedSelector::pick_best_ref(&candidates).unwrap();
        assert_eq!(winner_ref.name, "b");
    }
}