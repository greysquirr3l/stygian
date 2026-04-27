use std::collections::BTreeMap;

use crate::har;
use crate::investigation::{infer_requirements, investigate_har};
use crate::types::{
    AdapterStrategy, AntiBotProvider, ExecutionMode, IntegrationRecommendation,
    InvestigationBundle, InvestigationReport, RequirementLevel, RequirementsProfile, RuntimePolicy,
    SessionMode, TelemetryLevel,
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

    // Apply SLO-based escalation before other adjustments
    apply_slo_escalation(&mut policy, requirements);

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
    Ok(plan_from_report(report))
}

/// Infer requirements and build a runtime policy from an existing investigation report.
#[must_use]
pub fn plan_from_report(report: InvestigationReport) -> InvestigationBundle {
    let requirements = infer_requirements(&report);
    let policy = build_runtime_policy(&report, &requirements);

    InvestigationBundle {
        report,
        requirements,
        policy,
    }
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

/// Apply SLO-based escalation to the runtime policy.
///
/// When the `adaptive_rate_and_retry_budget` requirement is in the warning zone (Medium level),
/// increases retry budget and enables warmup for improved resilience.
/// When in the critical zone (High level), escalates to browser mode with sticky sessions
/// for maximum anti-bot posture.
fn apply_slo_escalation(policy: &mut RuntimePolicy, requirements: &RequirementsProfile) {
    // Find the adaptive_rate_and_retry_budget requirement
    let adaptive_req = requirements
        .requirements
        .iter()
        .find(|r| r.id == "adaptive_rate_and_retry_budget");

    match adaptive_req {
        Some(req) => match req.level {
            RequirementLevel::Medium => {
                // Warning zone: increase retries and enable warmup for resilience
                policy.max_retries = policy.max_retries.max(4);
                policy.backoff_base_ms = policy.backoff_base_ms.max(400);
                if policy.execution_mode == ExecutionMode::Http {
                    policy.enable_warmup = true;
                    policy
                        .config_hints
                        .insert("slo.escalation".to_string(), "warning".to_string());
                }
            }
            RequirementLevel::High => {
                // Critical zone: escalate to browser mode with sticky sessions if not already
                if policy.execution_mode != ExecutionMode::Browser
                    || policy.session_mode != SessionMode::Sticky
                {
                    policy.execution_mode = ExecutionMode::Browser;
                    policy.session_mode = SessionMode::Sticky;
                    policy.rate_limit_rps = policy.rate_limit_rps.min(1.5);
                    policy.max_retries = policy.max_retries.max(5);
                    policy.backoff_base_ms = policy.backoff_base_ms.max(600);
                    policy.enable_warmup = true;
                    policy.enforce_webrtc_proxy_only = true;
                    policy.sticky_session_ttl_secs = Some(600);
                    policy
                        .required_stygian_features
                        .push("stygian-proxy".to_string());
                    policy
                        .config_hints
                        .insert("slo.escalation".to_string(), "critical".to_string());
                    dedupe_required_features(&mut policy.required_stygian_features);
                }
            }
            RequirementLevel::Low => {
                // Below threshold: no escalation needed
            }
        },
        None => {
            // No adaptive requirement: policy stands as-is
        }
    }
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
            marker_histogram: BTreeMap::from([("x-datadome".to_string(), 30)]),
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
            target_class: None,
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

    #[test]
    fn plan_from_report_preserves_report_and_builds_bundle() {
        let report = InvestigationReport {
            page_title: Some("https://example.com".to_string()),
            total_requests: 10,
            blocked_requests: 4,
            status_histogram: BTreeMap::from([(200, 6), (403, 4)]),
            resource_type_histogram: BTreeMap::from([("document".to_string(), 10)]),
            provider_histogram: BTreeMap::from([(AntiBotProvider::Cloudflare, 4)]),
            marker_histogram: BTreeMap::from([
                ("cf-ray".to_string(), 4),
                ("__cf_bm".to_string(), 4),
            ]),
            top_markers: vec![
                MarkerCount {
                    marker: "cf-ray".to_string(),
                    count: 4,
                },
                MarkerCount {
                    marker: "__cf_bm".to_string(),
                    count: 4,
                },
            ],
            hosts: Vec::new(),
            suspicious_requests: Vec::new(),
            aggregate: Detection {
                provider: AntiBotProvider::Cloudflare,
                confidence: 0.8,
                markers: vec!["cf-ray".to_string(), "__cf_bm".to_string()],
            },
            target_class: None,
        };

        let bundle = plan_from_report(report.clone());
        assert_eq!(bundle.report, report);
        assert_eq!(bundle.requirements.provider, AntiBotProvider::Cloudflare);
        assert_eq!(bundle.policy.execution_mode, ExecutionMode::Browser);
    }

    #[test]
    fn slo_escalation_warning_increases_retries_and_warmup() {
        // A requirement at Medium level (warning zone) should increase retries and enable warmup
        use crate::types::{AntiBotRequirement, RequirementLevel};

        let report = InvestigationReport {
            page_title: None,
            total_requests: 100,
            blocked_requests: 20,
            status_histogram: BTreeMap::new(),
            resource_type_histogram: BTreeMap::new(),
            provider_histogram: BTreeMap::new(),
            marker_histogram: BTreeMap::new(),
            top_markers: Vec::new(),
            hosts: Vec::new(),
            suspicious_requests: Vec::new(),
            aggregate: Detection {
                provider: AntiBotProvider::Unknown,
                confidence: 0.0,
                markers: Vec::new(),
            },
            target_class: None,
        };

        let requirements = RequirementsProfile {
            provider: AntiBotProvider::Unknown,
            confidence: 0.0,
            requirements: vec![AntiBotRequirement {
                id: "adaptive_rate_and_retry_budget".to_string(),
                title: "Apply adaptive pacing".to_string(),
                why: "Block ratio in warning zone".to_string(),
                evidence: vec!["ratio=0.20".to_string()],
                level: RequirementLevel::Medium,
            }],
            recommendation: IntegrationRecommendation {
                strategy: AdapterStrategy::DirectHttp,
                rationale: "test".to_string(),
                required_stygian_features: Vec::new(),
                config_hints: BTreeMap::new(),
            },
        };

        let policy = build_runtime_policy(&report, &requirements);
        assert!(policy.max_retries >= 4);
        assert!(policy.backoff_base_ms >= 400);
        assert!(policy.enable_warmup);
        assert_eq!(
            policy.config_hints.get("slo.escalation"),
            Some(&"warning".to_string())
        );
    }

    #[test]
    fn slo_escalation_critical_escalates_to_browser_sticky() {
        // A requirement at High level (critical zone) should escalate to browser + sticky session
        use crate::types::{AntiBotRequirement, RequirementLevel};

        let report = InvestigationReport {
            page_title: None,
            total_requests: 100,
            blocked_requests: 25,
            status_histogram: BTreeMap::new(),
            resource_type_histogram: BTreeMap::new(),
            provider_histogram: BTreeMap::new(),
            marker_histogram: BTreeMap::new(),
            top_markers: Vec::new(),
            hosts: Vec::new(),
            suspicious_requests: Vec::new(),
            aggregate: Detection {
                provider: AntiBotProvider::Unknown,
                confidence: 0.0,
                markers: Vec::new(),
            },
            target_class: None,
        };

        let requirements = RequirementsProfile {
            provider: AntiBotProvider::Unknown,
            confidence: 0.0,
            requirements: vec![AntiBotRequirement {
                id: "adaptive_rate_and_retry_budget".to_string(),
                title: "Apply adaptive pacing".to_string(),
                why: "Block ratio in critical zone".to_string(),
                evidence: vec!["ratio=0.25".to_string()],
                level: RequirementLevel::High,
            }],
            recommendation: IntegrationRecommendation {
                strategy: AdapterStrategy::DirectHttp,
                rationale: "test".to_string(),
                required_stygian_features: Vec::new(),
                config_hints: BTreeMap::new(),
            },
        };

        let policy = build_runtime_policy(&report, &requirements);
        assert_eq!(policy.execution_mode, ExecutionMode::Browser);
        assert_eq!(policy.session_mode, SessionMode::Sticky);
        assert!(policy.max_retries >= 5);
        assert!(policy.backoff_base_ms >= 600);
        assert!(policy.enable_warmup);
        assert!(policy.sticky_session_ttl_secs.is_some());
        assert!(
            policy
                .required_stygian_features
                .contains(&"stygian-proxy".to_string())
        );
        assert_eq!(
            policy.config_hints.get("slo.escalation"),
            Some(&"critical".to_string())
        );
    }

    #[test]
    fn slo_escalation_respects_already_escalated_policies() {
        // If already in browser/sticky, critical escalation should not downgrade
        use crate::types::{AntiBotRequirement, RequirementLevel};

        let report = InvestigationReport {
            page_title: None,
            total_requests: 100,
            blocked_requests: 25,
            status_histogram: BTreeMap::new(),
            resource_type_histogram: BTreeMap::new(),
            provider_histogram: BTreeMap::new(),
            marker_histogram: BTreeMap::new(),
            top_markers: Vec::new(),
            hosts: Vec::new(),
            suspicious_requests: Vec::new(),
            aggregate: Detection {
                provider: AntiBotProvider::Unknown,
                confidence: 0.0,
                markers: Vec::new(),
            },
            target_class: None,
        };

        let requirements = RequirementsProfile {
            provider: AntiBotProvider::Unknown,
            confidence: 0.0,
            requirements: vec![AntiBotRequirement {
                id: "adaptive_rate_and_retry_budget".to_string(),
                title: "Apply adaptive pacing".to_string(),
                why: "Block ratio in critical zone".to_string(),
                evidence: vec!["ratio=0.25".to_string()],
                level: RequirementLevel::High,
            }],
            recommendation: IntegrationRecommendation {
                strategy: AdapterStrategy::StickyProxy,
                rationale: "test".to_string(),
                required_stygian_features: Vec::new(),
                config_hints: BTreeMap::new(),
            },
        };

        let policy = build_runtime_policy(&report, &requirements);
        assert_eq!(policy.execution_mode, ExecutionMode::Browser);
        assert_eq!(policy.session_mode, SessionMode::Sticky);
        // Should not add stygian-proxy twice
        let proxy_count = policy
            .required_stygian_features
            .iter()
            .filter(|f| f.as_str() == "stygian-proxy")
            .count();
        assert_eq!(proxy_count, 1);
    }

    #[test]
    fn slo_escalation_no_requirement_means_no_escalation() {
        // Without the adaptive requirement, no escalation should happen
        let report = InvestigationReport {
            page_title: None,
            total_requests: 100,
            blocked_requests: 3,
            status_histogram: BTreeMap::new(),
            resource_type_histogram: BTreeMap::new(),
            provider_histogram: BTreeMap::new(),
            marker_histogram: BTreeMap::new(),
            top_markers: Vec::new(),
            hosts: Vec::new(),
            suspicious_requests: Vec::new(),
            aggregate: Detection {
                provider: AntiBotProvider::Unknown,
                confidence: 0.0,
                markers: Vec::new(),
            },
            target_class: None,
        };

        let requirements = RequirementsProfile {
            provider: AntiBotProvider::Unknown,
            confidence: 0.0,
            requirements: Vec::new(), // No adaptive requirement
            recommendation: IntegrationRecommendation {
                strategy: AdapterStrategy::DirectHttp,
                rationale: "test".to_string(),
                required_stygian_features: Vec::new(),
                config_hints: BTreeMap::new(),
            },
        };

        let policy = build_runtime_policy(&report, &requirements);
        assert_eq!(policy.execution_mode, ExecutionMode::Http);
        assert_eq!(policy.session_mode, SessionMode::Stateless);
        assert_eq!(policy.max_retries, 2);
        assert!(!policy.enable_warmup);
        assert_eq!(policy.config_hints.get("slo.escalation"), None);
    }
}
