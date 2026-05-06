use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::analyzer::AnalyzerProfile;
use crate::har;
use crate::infer_requirements_with_target_class;
use crate::investigation::investigate_har_with_profile;
use crate::policy::build_runtime_policy;
use crate::types::{AntiBotProvider, ExecutionMode, SessionMode, TargetClass, TelemetryLevel};

/// One external observatory HAR input to compare against a baseline HAR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservatoryCase {
    /// Stable source identifier (for example, an observatory name or region key).
    pub source_id: String,
    /// HAR payload captured by the external source.
    pub har_json: String,
}

/// Escalation level derived from inferred adaptive requirements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservatoryEscalation {
    /// No adaptive escalation requirement detected.
    Acceptable,
    /// Warning zone escalation.
    Warning,
    /// Critical zone escalation.
    Critical,
}

/// Summarized outcome for one baseline/external observatory case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservatorySample {
    /// Source identifier for this sample.
    pub source_id: String,
    /// Aggregate provider predicted from the HAR.
    pub provider: AntiBotProvider,
    /// Aggregate provider confidence in [0.0, 1.0].
    pub confidence: f64,
    /// Total requests in this HAR.
    pub total_requests: u64,
    /// Blocked/challenged requests in this HAR.
    pub blocked_requests: u64,
    /// Blocked ratio in [0.0, 1.0].
    pub blocked_ratio: f64,
    /// Derived escalation level for this source.
    pub escalation: ObservatoryEscalation,
    /// Planned execution mode for this source.
    pub execution_mode: ExecutionMode,
    /// Planned session mode for this source.
    pub session_mode: SessionMode,
    /// Planned telemetry level for this source.
    pub telemetry_level: TelemetryLevel,
    /// Planned risk score in [0.0, 1.0].
    pub risk_score: f64,
}

/// One baseline-vs-external comparison row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservatoryComparison {
    /// External source identifier.
    pub source_id: String,
    /// Whether provider prediction matches baseline.
    pub provider_matches_baseline: bool,
    /// External blocked ratio minus baseline blocked ratio.
    pub blocked_ratio_delta: f64,
    /// External confidence minus baseline confidence.
    pub confidence_delta: f64,
    /// External risk score minus baseline risk score.
    pub risk_score_delta: f64,
    /// Whether escalation level changed compared with baseline.
    pub escalation_changed: bool,
    /// Recommended action for this comparison.
    pub recommended_action: String,
}

/// Aggregate report for one observatory run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservatoryReport {
    /// Baseline sample built from the baseline HAR.
    pub baseline: ObservatorySample,
    /// One sample per external observatory source.
    pub external: Vec<ObservatorySample>,
    /// Baseline-vs-external comparisons.
    pub comparisons: Vec<ObservatoryComparison>,
    /// Number of comparisons with provider disagreement.
    pub provider_disagreements: usize,
    /// True when any comparison indicates likely regression.
    pub has_regression: bool,
}

/// Errors returned by observatory runners.
#[derive(Debug, Error)]
pub enum ObservatoryError {
    /// No external observatory cases were provided.
    #[error("observatory run requires at least one external case")]
    EmptyExternalCases,
    /// HAR parsing/investigation failed.
    #[error(transparent)]
    Har(#[from] har::HarError),
    /// Live HTTP capture failed.
    #[cfg(feature = "live-validation")]
    #[error("live observatory capture failed for source '{source_id}': {message}")]
    LiveCapture {
        /// Source identifier for the failed capture.
        source_id: String,
        /// Human-readable transport error.
        message: String,
    },
}

/// Compare a baseline HAR against external observatory HAR captures.
///
/// # Errors
///
/// Returns [`ObservatoryError::EmptyExternalCases`] when `external_cases` is empty,
/// and [`ObservatoryError::Har`] when any HAR payload is invalid.
pub fn run_external_observatory_from_hars(
    baseline_har: &str,
    external_cases: &[ObservatoryCase],
    profile: &AnalyzerProfile,
    target_class: TargetClass,
) -> Result<ObservatoryReport, ObservatoryError> {
    if external_cases.is_empty() {
        return Err(ObservatoryError::EmptyExternalCases);
    }

    let baseline = evaluate_case("baseline", baseline_har, profile, target_class)?;

    let mut external = Vec::with_capacity(external_cases.len());
    for case in external_cases {
        external.push(evaluate_case(
            &case.source_id,
            &case.har_json,
            profile,
            target_class,
        )?);
    }

    let comparisons = external
        .iter()
        .map(|sample| compare_sample(&baseline, sample))
        .collect::<Vec<_>>();

    let provider_disagreements = comparisons
        .iter()
        .filter(|comparison| !comparison.provider_matches_baseline)
        .count();

    let has_regression = comparisons
        .iter()
        .any(|comparison| comparison.recommended_action == "investigate_regression");

    Ok(ObservatoryReport {
        baseline,
        external,
        comparisons,
        provider_disagreements,
        has_regression,
    })
}

/// One live observatory probe configuration.
#[cfg(feature = "live-validation")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveObservatoryProbe {
    /// Stable source identifier for this probe.
    pub source_id: String,
    /// User-Agent sent for this probe.
    pub user_agent: String,
    /// Additional request headers.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Request timeout in milliseconds.
    pub timeout_ms: u64,
}

/// Execute live observatory probes and compare them with the baseline HAR.
///
/// This helper is feature-gated behind `live-validation` and performs HTTP
/// requests using `reqwest`, synthesizing one HAR payload per probe.
///
/// # Errors
///
/// Returns [`ObservatoryError::EmptyExternalCases`] when no probes are supplied,
/// [`ObservatoryError::LiveCapture`] when a probe request fails, and
/// [`ObservatoryError::Har`] when generated HAR payloads cannot be parsed.
#[cfg(feature = "live-validation")]
pub async fn run_external_observatory_live(
    baseline_har: &str,
    target_url: &str,
    probes: &[LiveObservatoryProbe],
    profile: &AnalyzerProfile,
    target_class: TargetClass,
) -> Result<ObservatoryReport, ObservatoryError> {
    if probes.is_empty() {
        return Err(ObservatoryError::EmptyExternalCases);
    }

    let mut external_cases = Vec::with_capacity(probes.len());
    for probe in probes {
        let har_json = capture_probe_har(target_url, probe).await?;
        external_cases.push(ObservatoryCase {
            source_id: probe.source_id.clone(),
            har_json,
        });
    }

    run_external_observatory_from_hars(baseline_har, &external_cases, profile, target_class)
}

fn evaluate_case(
    source_id: &str,
    har_json: &str,
    profile: &AnalyzerProfile,
    target_class: TargetClass,
) -> Result<ObservatorySample, ObservatoryError> {
    let report = investigate_har_with_profile(har_json, profile)?;
    let requirements = infer_requirements_with_target_class(&report, target_class);
    let policy = build_runtime_policy(&report, &requirements);

    let blocked_ratio = blocked_ratio(report.blocked_requests, report.total_requests);

    Ok(ObservatorySample {
        source_id: source_id.to_string(),
        provider: report.aggregate.provider,
        confidence: report.aggregate.confidence,
        total_requests: report.total_requests,
        blocked_requests: report.blocked_requests,
        blocked_ratio,
        escalation: escalation_from_requirements(&requirements),
        execution_mode: policy.execution_mode,
        session_mode: policy.session_mode,
        telemetry_level: policy.telemetry_level,
        risk_score: policy.risk_score,
    })
}

fn escalation_from_requirements(
    requirements: &crate::types::RequirementsProfile,
) -> ObservatoryEscalation {
    let adaptive = requirements
        .requirements
        .iter()
        .find(|requirement| requirement.id == "adaptive_rate_and_retry_budget");

    match adaptive.map(|requirement| requirement.level) {
        Some(crate::types::RequirementLevel::High) => ObservatoryEscalation::Critical,
        Some(crate::types::RequirementLevel::Medium) => ObservatoryEscalation::Warning,
        _ => ObservatoryEscalation::Acceptable,
    }
}

fn compare_sample(
    baseline: &ObservatorySample,
    sample: &ObservatorySample,
) -> ObservatoryComparison {
    let provider_matches_baseline = sample.provider == baseline.provider;
    let blocked_ratio_delta = sample.blocked_ratio - baseline.blocked_ratio;
    let confidence_delta = sample.confidence - baseline.confidence;
    let risk_score_delta = sample.risk_score - baseline.risk_score;
    let escalation_changed = sample.escalation != baseline.escalation;

    let recommended_action = if escalation_changed || blocked_ratio_delta >= 0.05 {
        "investigate_regression".to_string()
    } else if !provider_matches_baseline && confidence_delta >= 0.15 {
        "investigate_provider_drift".to_string()
    } else if blocked_ratio_delta <= -0.05 && risk_score_delta <= -0.10 {
        "improved_stability".to_string()
    } else {
        "monitor".to_string()
    };

    ObservatoryComparison {
        source_id: sample.source_id.clone(),
        provider_matches_baseline,
        blocked_ratio_delta,
        confidence_delta,
        risk_score_delta,
        escalation_changed,
        recommended_action,
    }
}

#[cfg(feature = "live-validation")]
async fn capture_probe_har(
    target_url: &str,
    probe: &LiveObservatoryProbe,
) -> Result<String, ObservatoryError> {
    let timeout = std::time::Duration::from_millis(probe.timeout_ms);
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(probe.user_agent.clone())
        .build()
        .map_err(|error| ObservatoryError::LiveCapture {
            source_id: probe.source_id.clone(),
            message: format!("failed to build client: {error}"),
        })?;

    let mut request = client.get(target_url).header(
        "accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
    );

    for (name, value) in &probe.headers {
        request = request.header(name, value);
    }

    let response = request
        .send()
        .await
        .map_err(|error| ObservatoryError::LiveCapture {
            source_id: probe.source_id.clone(),
            message: error.to_string(),
        })?;

    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|parsed| (name.as_str().to_string(), parsed.to_string()))
        })
        .collect::<BTreeMap<_, _>>();

    Ok(build_single_request_har(target_url, status, &headers))
}

#[cfg(feature = "live-validation")]
fn build_single_request_har(url: &str, status: u16, headers: &BTreeMap<String, String>) -> String {
    let response_headers = headers
        .iter()
        .map(|(name, value)| {
            serde_json::json!({
                "name": name,
                "value": value,
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "log": {
            "version": "1.2",
            "creator": {"name": "stygian-charon-observatory", "version": "0.1"},
            "pages": [{
                "id": "page_1",
                "title": url,
                "startedDateTime": "2026-01-01T00:00:00.000Z",
                "pageTimings": {"onLoad": 0}
            }],
            "entries": [{
                "pageref": "page_1",
                "startedDateTime": "2026-01-01T00:00:00.000Z",
                "time": 0,
                "request": {
                    "method": "GET",
                    "url": url,
                    "httpVersion": "HTTP/2",
                    "headers": [],
                    "queryString": [],
                    "cookies": [],
                    "headersSize": -1,
                    "bodySize": 0
                },
                "response": {
                    "status": status,
                    "statusText": "live-capture",
                    "httpVersion": "HTTP/2",
                    "headers": response_headers,
                    "cookies": [],
                    "content": {"size": 0, "mimeType": "text/html", "text": ""},
                    "redirectURL": "",
                    "headersSize": -1,
                    "bodySize": 0
                },
                "cache": {},
                "timings": {
                    "blocked": 0,
                    "dns": 0,
                    "connect": 0,
                    "send": 0,
                    "wait": 0,
                    "receive": 0,
                    "ssl": 0
                }
            }]
        }
    })
    .to_string()
}

#[allow(clippy::cast_precision_loss)]
const fn to_f64(value: u64) -> f64 {
    value as f64
}

const fn blocked_ratio(blocked_requests: u64, total_requests: u64) -> f64 {
    if total_requests == 0 {
        0.0
    } else {
        to_f64(blocked_requests) / to_f64(total_requests)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HAR_OK: &str = r#"{"log":{"version":"1.2","creator":{"name":"test","version":"0"},"entries":[{"startedDateTime":"2026-01-01T00:00:00Z","time":1,"request":{"method":"GET","url":"https://example.com","httpVersion":"HTTP/1.1","headers":[],"queryString":[],"cookies":[],"headersSize":-1,"bodySize":-1},"response":{"status":200,"statusText":"OK","httpVersion":"HTTP/1.1","headers":[],"cookies":[],"content":{"size":0,"mimeType":"text/html","text":""},"redirectURL":"","headersSize":-1,"bodySize":-1},"cache":{},"timings":{"send":0,"wait":1,"receive":0}}]}}"#;

    const HAR_BLOCKED: &str = r#"{"log":{"version":"1.2","creator":{"name":"test","version":"0"},"entries":[{"startedDateTime":"2026-01-01T00:00:00Z","time":1,"request":{"method":"GET","url":"https://example.com","httpVersion":"HTTP/1.1","headers":[],"queryString":[],"cookies":[],"headersSize":-1,"bodySize":-1},"response":{"status":403,"statusText":"Forbidden","httpVersion":"HTTP/1.1","headers":[{"name":"cf-ray","value":"abc"},{"name":"server","value":"cloudflare"}],"cookies":[],"content":{"size":0,"mimeType":"text/html","text":"Attention Required! | Cloudflare"},"redirectURL":"","headersSize":-1,"bodySize":-1},"cache":{},"timings":{"send":0,"wait":1,"receive":0}}]}}"#;

    fn default_profile() -> AnalyzerProfile {
        AnalyzerProfile::default()
    }

    #[test]
    fn observatory_runner_rejects_empty_external_cases() {
        let result = run_external_observatory_from_hars(
            HAR_OK,
            &[],
            &default_profile(),
            TargetClass::Unknown,
        );

        assert!(matches!(result, Err(ObservatoryError::EmptyExternalCases)));
    }

    #[test]
    fn observatory_runner_reports_regression_when_blocked_ratio_jumps() {
        let external = vec![ObservatoryCase {
            source_id: "ext-1".to_string(),
            har_json: HAR_BLOCKED.to_string(),
        }];

        let result = run_external_observatory_from_hars(
            HAR_OK,
            &external,
            &default_profile(),
            TargetClass::Api,
        );

        assert!(result.is_ok());

        if let Ok(report) = result {
            assert!(report.has_regression);
            assert_eq!(report.comparisons.len(), 1);
            assert_eq!(
                report.comparisons[0].recommended_action,
                "investigate_regression"
            );
        }
    }

    #[test]
    fn observatory_runner_reports_monitor_for_similar_inputs() {
        let external = vec![ObservatoryCase {
            source_id: "ext-1".to_string(),
            har_json: HAR_OK.to_string(),
        }];

        let result = run_external_observatory_from_hars(
            HAR_OK,
            &external,
            &default_profile(),
            TargetClass::Unknown,
        );

        assert!(result.is_ok());

        if let Ok(report) = result {
            assert!(!report.has_regression);
            assert_eq!(report.comparisons[0].recommended_action, "monitor");
        }
    }
}
