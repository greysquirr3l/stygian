use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::snapshot::NormalizedFingerprintSnapshot;
use crate::snapshot::{
    SnapshotCollectionError, SnapshotDeterminismOptions, SnapshotDriftReport, SnapshotMode,
    SnapshotSignalDriftKind, compare_snapshot_signal_drift,
};

/// One corpus entry containing mode-specific snapshots captured from identical input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModeDifferentialCorpus {
    /// Stable corpus identifier (for example, a fixture or target slug).
    pub corpus_id: String,
    /// Snapshots captured across modes for this corpus.
    pub snapshots: Vec<NormalizedFingerprintSnapshot>,
}

/// One pairwise mode comparison to execute for every corpus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeComparison {
    /// Baseline mode.
    pub baseline: SnapshotMode,
    /// Candidate mode.
    pub candidate: SnapshotMode,
}

/// Thresholds used to determine whether a mode differential run fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModeDifferentialThresholds {
    /// Maximum allowed changed fields in a pairwise comparison.
    pub max_changed: usize,
    /// Maximum allowed added fields in a pairwise comparison.
    pub max_added: usize,
    /// Maximum allowed removed fields in a pairwise comparison.
    pub max_removed: usize,
    /// Maximum allowed total diffs in a pairwise comparison.
    pub max_total: usize,
}

/// Detailed result for one corpus/mode pair comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeDifferentialPairResult {
    /// Corpus id that was compared.
    pub corpus_id: String,
    /// Baseline mode.
    pub baseline_mode: SnapshotMode,
    /// Candidate mode.
    pub candidate_mode: SnapshotMode,
    /// Full drift details.
    pub drift: SnapshotDriftReport,
    /// Number of changed fields.
    pub changed: usize,
    /// Number of added fields.
    pub added: usize,
    /// Number of removed fields.
    pub removed: usize,
    /// Number of total diffs.
    pub total: usize,
    /// `true` when this pair exceeds configured thresholds.
    pub failed_thresholds: bool,
}

/// Aggregated mode differential run output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeDifferentialRunReport {
    /// Pairwise results for all corpora and configured comparisons.
    pub pair_results: Vec<ModeDifferentialPairResult>,
    /// Number of pairwise comparisons that exceeded thresholds.
    pub failing_pairs: usize,
    /// `true` when any pair exceeded configured thresholds.
    pub failed: bool,
}

/// Errors returned by the mode differential runner.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ModeDifferentialError {
    /// Required mode is missing from a corpus.
    #[error("corpus '{corpus_id}' is missing snapshot for mode {mode:?}")]
    MissingMode {
        /// Corpus identifier.
        corpus_id: String,
        /// Missing mode.
        mode: SnapshotMode,
    },
    /// A corpus contains duplicate snapshots for the same mode.
    #[error("corpus '{corpus_id}' contains duplicate snapshot for mode {mode:?}")]
    DuplicateMode {
        /// Corpus identifier.
        corpus_id: String,
        /// Duplicated mode.
        mode: SnapshotMode,
    },
    /// Snapshot collection/comparison failed.
    #[error("snapshot comparison failed: {0}")]
    Snapshot(#[from] SnapshotCollectionError),
}

/// Run pairwise mode differential comparisons across corpora.
///
/// This runner executes the same set of mode comparisons for every corpus,
/// computes signal-level drift with deterministic normalization, and evaluates
/// each pair against configurable thresholds suitable for CI gates.
///
/// # Errors
///
/// Returns [`ModeDifferentialError`] if a corpus contains duplicate modes,
/// is missing a required mode from `comparisons`, or snapshot comparison fails.
pub fn run_mode_differential_regression(
    corpora: &[ModeDifferentialCorpus],
    comparisons: &[ModeComparison],
    options: &SnapshotDeterminismOptions,
    thresholds: ModeDifferentialThresholds,
) -> Result<ModeDifferentialRunReport, ModeDifferentialError> {
    let mut pair_results = Vec::new();
    let mut failing_pairs = 0_usize;

    for corpus in corpora {
        validate_unique_modes(corpus)?;

        for comparison in comparisons {
            let baseline = find_mode_snapshot(corpus, comparison.baseline)?;
            let candidate = find_mode_snapshot(corpus, comparison.candidate)?;

            let drift = compare_snapshot_signal_drift(baseline, candidate, options)?;
            let (changed, added, removed) = count_diffs(&drift);
            let total = drift.diffs.len();
            let failed_thresholds = changed > thresholds.max_changed
                || added > thresholds.max_added
                || removed > thresholds.max_removed
                || total > thresholds.max_total;

            if failed_thresholds {
                failing_pairs = failing_pairs.saturating_add(1);
            }

            pair_results.push(ModeDifferentialPairResult {
                corpus_id: corpus.corpus_id.clone(),
                baseline_mode: comparison.baseline,
                candidate_mode: comparison.candidate,
                drift,
                changed,
                added,
                removed,
                total,
                failed_thresholds,
            });
        }
    }

    Ok(ModeDifferentialRunReport {
        pair_results,
        failing_pairs,
        failed: failing_pairs > 0,
    })
}

fn validate_unique_modes(corpus: &ModeDifferentialCorpus) -> Result<(), ModeDifferentialError> {
    let mut seen = Vec::new();
    for snapshot in &corpus.snapshots {
        if seen.contains(&snapshot.mode) {
            return Err(ModeDifferentialError::DuplicateMode {
                corpus_id: corpus.corpus_id.clone(),
                mode: snapshot.mode,
            });
        }
        seen.push(snapshot.mode);
    }
    Ok(())
}

fn find_mode_snapshot(
    corpus: &ModeDifferentialCorpus,
    mode: SnapshotMode,
) -> Result<&NormalizedFingerprintSnapshot, ModeDifferentialError> {
    corpus
        .snapshots
        .iter()
        .find(|snapshot| snapshot.mode == mode)
        .ok_or_else(|| ModeDifferentialError::MissingMode {
            corpus_id: corpus.corpus_id.clone(),
            mode,
        })
}

fn count_diffs(drift: &SnapshotDriftReport) -> (usize, usize, usize) {
    let mut changed = 0_usize;
    let mut added = 0_usize;
    let mut removed = 0_usize;

    for diff in &drift.diffs {
        match diff.kind {
            SnapshotSignalDriftKind::Changed => changed = changed.saturating_add(1),
            SnapshotSignalDriftKind::Added => added = added.saturating_add(1),
            SnapshotSignalDriftKind::Removed => removed = removed.saturating_add(1),
        }
    }

    (changed, added, removed)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn parse_snapshot(path: &str) -> NormalizedFingerprintSnapshot {
        serde_json::from_str::<NormalizedFingerprintSnapshot>(path)
            .expect("example snapshot should deserialize")
    }

    #[test]
    fn mode_differential_runner_reports_failures_against_thresholds() {
        let mut browser = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-browser.json"
        ));
        browser.signals.user_agent = "different-agent".to_string();

        let corpus = ModeDifferentialCorpus {
            corpus_id: "fixture-a".to_string(),
            snapshots: vec![
                parse_snapshot(include_str!(
                    "../docs/examples/fingerprint-snapshot-v1-http.json"
                )),
                browser,
            ],
        };

        let report = run_mode_differential_regression(
            &[corpus],
            &[ModeComparison {
                baseline: SnapshotMode::Http,
                candidate: SnapshotMode::Browser,
            }],
            &SnapshotDeterminismOptions::default(),
            ModeDifferentialThresholds::default(),
        )
        .expect("runner should execute");

        assert!(report.failed);
        assert_eq!(report.failing_pairs, 1);
        assert_eq!(report.pair_results.len(), 1);
        let first = report
            .pair_results
            .first()
            .expect("expected exactly one pair result");
        assert!(first.failed_thresholds);
        assert!(first.total > 0);
    }

    #[test]
    fn mode_differential_runner_errors_on_missing_mode() {
        let corpus = ModeDifferentialCorpus {
            corpus_id: "fixture-b".to_string(),
            snapshots: vec![parse_snapshot(include_str!(
                "../docs/examples/fingerprint-snapshot-v1-http.json"
            ))],
        };

        let err = run_mode_differential_regression(
            &[corpus],
            &[ModeComparison {
                baseline: SnapshotMode::Http,
                candidate: SnapshotMode::Browser,
            }],
            &SnapshotDeterminismOptions::default(),
            ModeDifferentialThresholds::default(),
        )
        .expect_err("missing mode must fail");

        assert_eq!(
            err,
            ModeDifferentialError::MissingMode {
                corpus_id: "fixture-b".to_string(),
                mode: SnapshotMode::Browser,
            }
        );
    }

    #[test]
    fn mode_differential_runner_errors_on_duplicate_mode() {
        let http = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        let corpus = ModeDifferentialCorpus {
            corpus_id: "fixture-c".to_string(),
            snapshots: vec![http.clone(), http],
        };

        let err = run_mode_differential_regression(
            &[corpus],
            &[ModeComparison {
                baseline: SnapshotMode::Http,
                candidate: SnapshotMode::Browser,
            }],
            &SnapshotDeterminismOptions::default(),
            ModeDifferentialThresholds::default(),
        )
        .expect_err("duplicate mode must fail");

        assert_eq!(
            err,
            ModeDifferentialError::DuplicateMode {
                corpus_id: "fixture-c".to_string(),
                mode: SnapshotMode::Http,
            }
        );
    }
}
