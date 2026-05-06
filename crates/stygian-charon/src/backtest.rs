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

    Ok(BacktestReport {
        total_cases: corpus.len(),
        total_profiles: profiles.len(),
        samples,
        disagreements,
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
            let unique: BTreeSet<AntiBotProvider> = providers_by_profile.values().copied().collect();
            (unique.len() > 1).then(|| BacktestDisagreement {
                case_id: case_id.to_string(),
                providers_by_profile,
            })
        })
        .collect()
}

#[cfg(test)]
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
            assert_eq!(report.samples[0].provider, AntiBotProvider::Cloudflare);
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
        assert_eq!(disagreements[0].case_id, "case-1");
    }
}
