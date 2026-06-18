use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::analyzer::{AnalyzerProfile, AnalyzerVersion};
use crate::classifier::classify_har_with_profile;
use crate::har;
use crate::types::AntiBotProvider;

/// One historical HAR input used for profile backtesting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BacktestCase {
    /// Stable case identifier (fixture name, date, or target key).
    pub case_id: String,
    /// HAR payload to replay for all profiles.
    pub har_json: String,
}

/// One profile result for one backtest case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BacktestSample {
    /// Case identifier from the source corpus.
    pub case_id: String,
    /// Profile identifier used for this run.
    pub profile_id: String,
    /// Analyzer version that executed this sample.
    pub analyzer_version: AnalyzerVersion,
    /// Aggregate provider prediction for the replayed HAR.
    pub provider: AntiBotProvider,
    /// Confidence score for the aggregate prediction.
    pub confidence: f64,
    /// Number of requests classified in this case.
    pub request_count: usize,
    /// Number of suspicious requests (blocked/challenged or non-unknown provider).
    pub suspicious_request_count: usize,
}

/// Disagreement detected across profiles for a single case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BacktestDisagreement {
    /// Case where profiles disagreed.
    pub case_id: String,
    /// Provider selected by each profile.
    pub providers_by_profile: BTreeMap<String, AntiBotProvider>,
}

/// Per-profile aggregate metrics computed from backtest samples.
///
/// Metrics help identify profiles that underperform compared to baseline,
/// enabling data-driven decisions about rule rollout and SLO adjustments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileMetrics {
    /// Profile identifier.
    pub profile_id: String,
    /// Total cases analyzed for this profile.
    pub total_samples: usize,
    /// Average confidence score across all samples (0.0–1.0).
    pub avg_confidence: f64,
    /// Percentage of samples where this profile detected suspicious activity.
    pub detection_rate: f64,
    /// Number of disagreement cases where this profile diverged from other profiles.
    pub disagreement_count: usize,
    /// Number of cases with confidence < 0.5 (potentially false positives).
    pub low_confidence_count: usize,
    /// Ratio of cases with low confidence (0.0–1.0).
    pub low_confidence_rate: f64,
}

/// Aggregate output for profile backtesting over historical HARs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BacktestReport {
    /// Number of cases in the input corpus.
    pub total_cases: usize,
    /// Number of profiles replayed for each case.
    pub total_profiles: usize,
    /// Flattened case x profile matrix of results.
    pub samples: Vec<BacktestSample>,
    /// Cases where profile predictions diverged.
    pub disagreements: Vec<BacktestDisagreement>,
    /// Aggregate metrics per profile (optional; computed on demand).
    #[serde(default)]
    pub profile_metrics: BTreeMap<String, ProfileMetrics>,
}

/// Errors returned by [`run_profile_backtest`].
#[derive(Debug, Error)]
pub enum BacktestError {
    /// No cases were provided.
    #[error("backtest corpus must contain at least one case")]
    EmptyCorpus,
    /// No profiles were provided.
    #[error("backtest must include at least one analyzer profile")]
    EmptyProfiles,
    /// HAR payload parsing/classification failed.
    #[error(transparent)]
    Har(#[from] har::HarError),
}

/// Replay historical HAR cases against existing analyzer profiles.
///
/// # Errors
///
/// Returns [`BacktestError::EmptyCorpus`] when no cases are supplied,
/// [`BacktestError::EmptyProfiles`] when no profiles are supplied, and
/// [`BacktestError::Har`] when any HAR payload cannot be parsed.
///
/// # Example
///
/// ```rust
/// use stygian_charon::AnalyzerProfile;
/// use stygian_charon::AnalyzerVersion;
/// use stygian_charon::BacktestCase;
/// use stygian_charon::run_profile_backtest;
///
/// let corpus = vec![BacktestCase {
///     case_id: "fixture-a".to_string(),
///     har_json: r#"{"log":{"version":"1.2","creator":{"name":"test","version":"0"},"entries":[{"startedDateTime":"2026-01-01T00:00:00Z","time":1,"request":{"method":"GET","url":"https://example.com","httpVersion":"HTTP/1.1","headers":[],"queryString":[],"cookies":[],"headersSize":-1,"bodySize":-1},"response":{"status":403,"statusText":"Forbidden","httpVersion":"HTTP/1.1","headers":[{"name":"cf-ray","value":"abc"}],"cookies":[],"content":{"size":0,"mimeType":"text/html","text":"Attention Required! | Cloudflare"},"redirectURL":"","headersSize":-1,"bodySize":-1},"cache":{},"timings":{"send":0,"wait":1,"receive":0}}]}}"#.to_string(),
/// }];
///
/// let profiles = vec![AnalyzerProfile {
///     profile_id: "default".to_string(),
///     analyzer_version: AnalyzerVersion::V1,
/// }];
///
/// let report = run_profile_backtest(&corpus, &profiles).unwrap();
/// assert_eq!(report.samples.len(), 1);
/// ```
pub fn run_profile_backtest(
    corpus: &[BacktestCase],
    profiles: &[AnalyzerProfile],
) -> Result<BacktestReport, BacktestError> {
    if corpus.is_empty() {
        return Err(BacktestError::EmptyCorpus);
    }
    if profiles.is_empty() {
        return Err(BacktestError::EmptyProfiles);
    }

    let mut samples = Vec::new();

    for case in corpus {
        for profile in profiles {
            let report = classify_har_with_profile(&case.har_json, profile)?;
            let suspicious_request_count = report
                .requests
                .iter()
                .filter(|request| {
                    request.status == 403
                        || request.status == 429
                        || request.detection.provider != AntiBotProvider::Unknown
                })
                .count();

            samples.push(BacktestSample {
                case_id: case.case_id.clone(),
                profile_id: profile.profile_id.clone(),
                analyzer_version: profile.analyzer_version,
                provider: report.aggregate.provider,
                confidence: report.aggregate.confidence,
                request_count: report.requests.len(),
                suspicious_request_count,
            });
        }
    }

    let disagreements = compute_disagreements(&samples);
    let profile_metrics = compute_profile_metrics(&samples, &disagreements);

    Ok(BacktestReport {
        total_cases: corpus.len(),
        total_profiles: profiles.len(),
        samples,
        disagreements,
        profile_metrics,
    })
}

fn compute_disagreements(samples: &[BacktestSample]) -> Vec<BacktestDisagreement> {
    let mut by_case: BTreeMap<&str, BTreeMap<String, AntiBotProvider>> = BTreeMap::new();

    for sample in samples {
        let entry = by_case.entry(&sample.case_id).or_default();
        let _ = entry.insert(sample.profile_id.clone(), sample.provider);
    }

    by_case
        .into_iter()
        .filter_map(|(case_id, providers_by_profile)| {
            let unique: BTreeSet<AntiBotProvider> =
                providers_by_profile.values().copied().collect();
            (unique.len() > 1).then(|| BacktestDisagreement {
                case_id: case_id.to_string(),
                providers_by_profile,
            })
        })
        .collect()
}

fn usize_to_f64_saturating(value: usize) -> f64 {
    f64::from(u32::try_from(value).unwrap_or(u32::MAX))
}

fn ratio_from_counts(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        usize_to_f64_saturating(numerator) / usize_to_f64_saturating(denominator)
    }
}

/// Compute per-profile aggregate metrics from backtest samples and disagreements.
///
/// Metrics include detection rate, average confidence, and disagreement frequency,
/// which inform acceptance decisions during rule rollout.
fn compute_profile_metrics(
    samples: &[BacktestSample],
    disagreements: &[BacktestDisagreement],
) -> BTreeMap<String, ProfileMetrics> {
    let mut by_profile: BTreeMap<String, Vec<&BacktestSample>> = BTreeMap::new();

    // Group samples by profile
    for sample in samples {
        by_profile
            .entry(sample.profile_id.clone())
            .or_default()
            .push(sample);
    }

    // Count disagreements per profile
    let mut disagreement_counts: BTreeMap<String, usize> = BTreeMap::new();
    for disagreement in disagreements {
        for profile_id in disagreement.providers_by_profile.keys() {
            *disagreement_counts.entry(profile_id.clone()).or_insert(0) += 1;
        }
    }

    // Compute metrics for each profile
    by_profile
        .into_iter()
        .map(|(profile_id, profile_samples)| {
            let total_samples = profile_samples.len();

            // Detection rate: percentage of cases with non-Unknown provider
            let detected_count = profile_samples
                .iter()
                .filter(|s| s.provider != AntiBotProvider::Unknown)
                .count();
            let detection_rate = ratio_from_counts(detected_count, total_samples);

            // Average confidence score
            let avg_confidence = if total_samples > 0 {
                profile_samples.iter().map(|s| s.confidence).sum::<f64>()
                    / usize_to_f64_saturating(total_samples)
            } else {
                0.0
            };

            // Low confidence (potential false positive indicator)
            let low_confidence_count = profile_samples
                .iter()
                .filter(|s| s.confidence < 0.5)
                .count();
            let low_confidence_rate = ratio_from_counts(low_confidence_count, total_samples);

            // Disagreement count for this profile
            let disagreement_count = disagreement_counts.get(&profile_id).copied().unwrap_or(0);

            (
                profile_id.clone(),
                ProfileMetrics {
                    profile_id,
                    total_samples,
                    avg_confidence,
                    detection_rate,
                    disagreement_count,
                    low_confidence_count,
                    low_confidence_rate,
                },
            )
        })
        .collect()
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

    const CLOUDFLARE_HAR: &str = r#"{"log":{"version":"1.2","creator":{"name":"test","version":"0"},"entries":[{"startedDateTime":"2026-01-01T00:00:00Z","time":1,"request":{"method":"GET","url":"https://example.com","httpVersion":"HTTP/1.1","headers":[],"queryString":[],"cookies":[],"headersSize":-1,"bodySize":-1},"response":{"status":403,"statusText":"Forbidden","httpVersion":"HTTP/1.1","headers":[{"name":"cf-ray","value":"abc"},{"name":"server","value":"cloudflare"}],"cookies":[],"content":{"size":0,"mimeType":"text/html","text":"Attention Required! | Cloudflare"},"redirectURL":"","headersSize":-1,"bodySize":-1},"cache":{},"timings":{"send":0,"wait":1,"receive":0}}]}}"#;

    #[test]
    fn backtest_generates_case_profile_matrix() {
        let corpus = vec![BacktestCase {
            case_id: "case-1".to_string(),
            har_json: CLOUDFLARE_HAR.to_string(),
        }];

        let profiles = vec![AnalyzerProfile {
            profile_id: "default".to_string(),
            analyzer_version: AnalyzerVersion::V1,
        }];

        let result = run_profile_backtest(&corpus, &profiles);
        assert!(result.is_ok());

        if let Ok(report) = result {
            assert_eq!(report.total_cases, 1);
            assert_eq!(report.total_profiles, 1);
            assert_eq!(report.samples.len(), 1);
            let first = report.samples.first();
            assert!(first.is_some(), "expected at least one sample");
            if let Some(first_sample) = first {
                assert_eq!(first_sample.provider, AntiBotProvider::Cloudflare);
            }
            assert!(report.disagreements.is_empty());
        }
    }

    #[test]
    fn backtest_rejects_empty_inputs() {
        let profiles = vec![AnalyzerProfile {
            profile_id: "default".to_string(),
            analyzer_version: AnalyzerVersion::V1,
        }];

        let no_cases = run_profile_backtest(&[], &profiles);
        assert!(matches!(no_cases, Err(BacktestError::EmptyCorpus)));

        let corpus = vec![BacktestCase {
            case_id: "case-1".to_string(),
            har_json: CLOUDFLARE_HAR.to_string(),
        }];
        let no_profiles = run_profile_backtest(&corpus, &[]);
        assert!(matches!(no_profiles, Err(BacktestError::EmptyProfiles)));
    }

    #[test]
    fn compute_disagreements_flags_divergent_predictions() {
        let samples = vec![
            BacktestSample {
                case_id: "case-1".to_string(),
                profile_id: "profile-a".to_string(),
                analyzer_version: AnalyzerVersion::V1,
                provider: AntiBotProvider::Cloudflare,
                confidence: 0.9,
                request_count: 1,
                suspicious_request_count: 1,
            },
            BacktestSample {
                case_id: "case-1".to_string(),
                profile_id: "profile-b".to_string(),
                analyzer_version: AnalyzerVersion::V1Legacy,
                provider: AntiBotProvider::DataDome,
                confidence: 0.8,
                request_count: 1,
                suspicious_request_count: 1,
            },
        ];

        let disagreements = compute_disagreements(&samples);
        assert_eq!(disagreements.len(), 1);
        let first = disagreements.first();
        assert!(first.is_some(), "expected one disagreement");
        if let Some(first_disagreement) = first {
            assert_eq!(first_disagreement.case_id, "case-1");
        }
    }
}
