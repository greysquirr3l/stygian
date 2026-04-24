use std::collections::BTreeMap;

use crate::har;
use crate::investigation::{infer_requirements, investigate_har};
use crate::types::{
    AdapterStrategy, AntiBotProvider, ExecutionMode, IntegrationRecommendation,
    InvestigationBundle, InvestigationReport, RequirementsProfile, RuntimePolicy, SessionMode,
    TelemetryLevel,
};

/// Build a concrete runtime policy from an investigation report and inferred requirements.
#[must_use]
pub fn build_runtime_policy(
    report: &InvestigationReport,
    requirements: &RequirementsProfile,
) -> RuntimePolicy {
    let blocked_ratio = if report.total_requests == 0 {
        0.0
    } else {
        to_f64(report.blocked_requests) / to_f64(report.total_requests)
    };

    let mut policy = RuntimePolicy {
        execution_mode: ExecutionMode::Http,
        session_mode: SessionMode::Stateless,
        telemetry_level: TelemetryLevel::Standard,
        rate_limit_rps: 3.0,
        max_retries: 2,
        backoff_base_ms: 250,
        enable_warmup: false,
        enforce_webrtc_proxy_only: false,
        sticky_session_ttl_secs: None,
        required_stygian_features: requirements
            .recommendation
            .required_stygian_features
            .clone(),
        config_hints: requirements.recommendation.config_hints.clone(),
        risk_score: blocked_ratio,
    };

    apply_strategy_defaults(&mut policy, &requirements.recommendation);

    let has_429 = report.status_histogram.get(&429).copied().unwrap_or(0) > 0;
    if has_429 {
        policy.rate_limit_rps = policy.rate_limit_rps.min(2.0);
        policy.max_retries = policy.max_retries.max(3);
        policy.backoff_base_ms = policy.backoff_base_ms.max(500);
        policy
            .config_hints
            .insert("retry.respect_429".to_string(), "true".to_string());
    }

    if report.blocked_requests > 0 {
        policy.telemetry_level = TelemetryLevel::Deep;
        policy.config_hints.insert(
            "charon.capture_block_template".to_string(),
            "true".to_string(),
        );
    }

    // Clamp and enrich risk score with provider confidence and blocked ratio.
    let provider_weight = match requirements.provider {
        AntiBotProvider::Unknown => 0.1,
        AntiBotProvider::Cloudflare => 0.25,
        AntiBotProvider::DataDome => 0.35,
        AntiBotProvider::Akamai
        | AntiBotProvider::PerimeterX
        | AntiBotProvider::Kasada
        | AntiBotProvider::FingerprintCom => 0.3,
    };

    let mut risk = blocked_ratio * 0.7 + requirements.confidence * provider_weight;
    if has_429 {
        risk += 0.1;
    }
    policy.risk_score = risk.clamp(0.0, 1.0);

    policy
}

/// Perform HAR investigation, infer requirements, and produce a runtime policy in one call.
///
/// # Errors
///
/// Returns [`har::HarError`] when the HAR payload is invalid or malformed.
pub fn analyze_and_plan(har_json: &str) -> Result<InvestigationBundle, har::HarError> {
    let report = investigate_har(har_json)?;
    let requirements = infer_requirements(&report);
    let policy = build_runtime_policy(&report, &requirements);

    Ok(InvestigationBundle {
        report,
        requirements,
        policy,
    })
}

fn apply_strategy_defaults(policy: &mut RuntimePolicy, recommendation: &IntegrationRecommendation) {
    match recommendation.strategy {
        AdapterStrategy::DirectHttp => {
            policy.execution_mode = ExecutionMode::Http;
            policy.session_mode = SessionMode::Stateless;
            policy.rate_limit_rps = 4.0;
        }
        AdapterStrategy::BrowserStealth => {
            policy.execution_mode = ExecutionMode::Browser;
            policy.session_mode = SessionMode::Stateless;
            policy.rate_limit_rps = 2.0;
            policy.max_retries = 3;
            policy.enable_warmup = true;
            policy.enforce_webrtc_proxy_only = true;
        }
        AdapterStrategy::StickyProxy => {
            policy.execution_mode = ExecutionMode::Browser;
            policy.session_mode = SessionMode::Sticky;
            policy.rate_limit_rps = 1.5;
            policy.max_retries = 3;
            policy.backoff_base_ms = 600;
            policy.enable_warmup = true;
            policy.enforce_webrtc_proxy_only = true;
            policy.sticky_session_ttl_secs = Some(600);
            policy
                .required_stygian_features
                .push("stygian-proxy".to_string());
            policy
                .config_hints
                .insert("proxy.rotation".to_string(), "per-domain".to_string());
        }
        AdapterStrategy::SessionWarmup => {
            policy.execution_mode = ExecutionMode::Browser;
            policy.enable_warmup = true;
            policy.rate_limit_rps = 2.0;
            policy.max_retries = 2;
        }
        AdapterStrategy::InvestigateOnly => {
            policy.execution_mode = ExecutionMode::Http;
            policy.telemetry_level = TelemetryLevel::Deep;
            policy.rate_limit_rps = 2.0;
            policy.max_retries = 1;
            policy
                .config_hints
                .insert("charon.mode".to_string(), "investigation".to_string());
        }
    }

    dedupe_required_features(&mut policy.required_stygian_features);
}

fn dedupe_required_features(features: &mut Vec<String>) {
    let mut seen = BTreeMap::new();
    features.retain(|feature| seen.insert(feature.clone(), true).is_none());
}

#[allow(clippy::cast_precision_loss)]
const fn to_f64(value: u64) -> f64 {
    value as f64
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::types::{Detection, InvestigationReport, MarkerCount, RequirementsProfile};

    use super::*;

    #[test]
    fn sticky_proxy_strategy_enables_sticky_policy() {
        let report = InvestigationReport {
            page_title: Some("https://example.com".to_string()),
            total_requests: 100,
            blocked_requests: 30,
            status_histogram: BTreeMap::from([(403, 30)]),
            resource_type_histogram: BTreeMap::new(),
            provider_histogram: BTreeMap::new(),
            top_markers: vec![MarkerCount {
                marker: "x-datadome".to_string(),
                count: 30,
            }],
            hosts: Vec::new(),
            suspicious_requests: Vec::new(),
            aggregate: Detection {
                provider: AntiBotProvider::DataDome,
                confidence: 0.9,
                markers: vec!["x-datadome".to_string()],
            },
        };

        let requirements = RequirementsProfile {
            provider: AntiBotProvider::DataDome,
            confidence: 0.9,
            requirements: Vec::new(),
            recommendation: IntegrationRecommendation {
                strategy: AdapterStrategy::StickyProxy,
                rationale: "test".to_string(),
                required_stygian_features: vec!["stygian-browser".to_string()],
                config_hints: BTreeMap::new(),
            },
        };

        let policy = build_runtime_policy(&report, &requirements);
        assert_eq!(policy.execution_mode, ExecutionMode::Browser);
        assert_eq!(policy.session_mode, SessionMode::Sticky);
        assert!(policy.sticky_session_ttl_secs.is_some());
        assert!(policy.risk_score > 0.0);
    }
}
