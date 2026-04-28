#![allow(clippy::cast_precision_loss)]

use std::time::{Duration, Instant};

use serde_json::json;
use stygian_charon::{
    BlockedRatioSlo, RequirementLevel, TargetClass, build_runtime_policy,
    infer_requirements_with_target_class, investigate_har, map_runtime_policy,
};

fn synthetic_har(total_requests: u32, blocked_403: u32, blocked_429: u32) -> String {
    let mut entries = Vec::new();

    for index in 0..total_requests {
        let status = if index < blocked_403 {
            403
        } else if index < blocked_403.saturating_add(blocked_429) {
            429
        } else {
            200
        };

        entries.push(json!({
            "pageref": "page_1",
            "startedDateTime": format!("2026-01-01T00:00:{:02}.000Z", index % 60),
            "time": 0,
            "_resourceType": if index % 5 == 0 { "preflight" } else { "xhr" },
            "request": {
                "method": "GET",
                "url": format!("https://example.com/path/{index}"),
                "httpVersion": "HTTP/2",
                "headers": [],
                "queryString": [],
                "cookies": [],
                "headersSize": -1,
                "bodySize": 0
            },
            "response": {
                "status": status,
                "statusText": "ok",
                "httpVersion": "HTTP/2",
                "headers": [],
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
        }));
    }

    json!({
        "log": {
            "version": "1.2",
            "creator": {"name": "p0-p1-e2e", "version": "1.0"},
            "pages": [{
                "id": "page_1",
                "title": "https://example.com",
                "startedDateTime": "2026-01-01T00:00:00.000Z",
                "pageTimings": {"onLoad": 0}
            }],
            "entries": entries
        }
    })
    .to_string()
}

fn run_pipeline_for_class(
    target_class: TargetClass,
    total_requests: u32,
    blocked_403: u32,
    blocked_429: u32,
) -> Option<(
    f64,
    stygian_charon::RuntimePolicy,
    stygian_charon::AcquisitionPolicy,
    Duration,
)> {
    let har = synthetic_har(total_requests, blocked_403, blocked_429);
    let started = Instant::now();

    let report = investigate_har(&har).ok()?;
    let requirements = infer_requirements_with_target_class(&report, target_class);
    let ratio = if report.total_requests == 0 {
        0.0
    } else {
        report.blocked_requests as f64 / report.total_requests as f64
    };

    let policy = build_runtime_policy(&report, &requirements);
    let acquisition = map_runtime_policy(&policy);
    let elapsed = started.elapsed();

    Some((ratio, policy, acquisition, elapsed))
}

#[test]
fn e2e_happy_path_validates_all_target_classes() {
    let scenarios = [
        (TargetClass::Api, 100_u32, 3_u32, 0_u32),
        (TargetClass::ContentSite, 100_u32, 10_u32, 0_u32),
        (TargetClass::HighSecurity, 100_u32, 25_u32, 0_u32),
    ];

    for (class, total, blocked_403, blocked_429) in scenarios {
        let result = run_pipeline_for_class(class, total, blocked_403, blocked_429);
        assert!(result.is_some(), "HAR should parse for E2E scenario");

        let Some((_ratio, policy, acquisition, elapsed)) = result else {
            return;
        };

        assert!(
            elapsed < Duration::from_secs(1),
            "scenario should complete under 1 second"
        );
        assert!(
            matches!(
                policy.execution_mode,
                stygian_charon::ExecutionMode::Http | stygian_charon::ExecutionMode::Browser
            ),
            "runtime policy should produce a valid execution mode"
        );
        assert!(
            matches!(
                acquisition.mode,
                stygian_charon::AcquisitionModeHint::Fast
                    | stygian_charon::AcquisitionModeHint::Resilient
                    | stygian_charon::AcquisitionModeHint::Hostile
                    | stygian_charon::AcquisitionModeHint::Investigate
            ),
            "acquisition mapping should produce a supported mode"
        );
    }
}

#[test]
fn e2e_zone_transitions_for_api_class() {
    let api_slo = BlockedRatioSlo::api();

    let acceptable = run_pipeline_for_class(TargetClass::Api, 100, 2, 0);
    assert!(acceptable.is_some());
    let warning = run_pipeline_for_class(TargetClass::Api, 100, 7, 0);
    assert!(warning.is_some());
    let critical = run_pipeline_for_class(TargetClass::Api, 100, 20, 0);
    assert!(critical.is_some());

    let Some(acceptable) = acceptable else {
        return;
    };
    let Some(warning) = warning else {
        return;
    };
    let Some(critical) = critical else {
        return;
    };

    let (a_ok, a_warn, a_critical) = api_slo.assess(acceptable.0);
    assert!(a_ok && !a_warn && !a_critical);

    let (w_ok, w_warn, w_critical) = api_slo.assess(warning.0);
    assert!(!w_ok && w_warn && !w_critical);

    let (c_ok, c_warn, c_critical) = api_slo.assess(critical.0);
    assert!(!c_ok && !c_warn && c_critical);
}

#[test]
fn e2e_mixed_signal_edge_case_429_and_403() {
    let result = run_pipeline_for_class(TargetClass::Api, 100, 8, 5);
    assert!(result.is_some());

    let Some((_ratio, policy, _acquisition, elapsed)) = result else {
        return;
    };

    assert!(elapsed < Duration::from_secs(1));
    assert!(
        policy.config_hints.contains_key("retry.respect_429"),
        "mixed 403/429 scenario should activate 429-aware retry hint"
    );
}

#[test]
fn e2e_boundary_threshold_edge_case() {
    let api_slo = BlockedRatioSlo::api();
    let result = run_pipeline_for_class(TargetClass::Api, 100, 5, 0);
    assert!(result.is_some());

    let Some((ratio, _policy, _acquisition, elapsed)) = result else {
        return;
    };

    assert!(elapsed < Duration::from_secs(1));
    let (acceptable, warning, critical) = api_slo.assess(ratio);
    assert!(
        acceptable,
        "exact acceptable threshold should remain acceptable"
    );
    assert!(!warning);
    assert!(!critical);
}

#[test]
fn e2e_requirement_severity_edge_case() {
    let har = synthetic_har(100, 25, 0);
    let report_result = investigate_har(&har);
    assert!(report_result.is_ok(), "HAR should parse");
    let Ok(report) = report_result else {
        return;
    };

    let requirements = infer_requirements_with_target_class(&report, TargetClass::Api);
    let adaptive_requirement = requirements
        .requirements
        .iter()
        .find(|r| r.id == "adaptive_rate_and_retry_budget");

    assert!(
        adaptive_requirement.is_some(),
        "adaptive requirement should be present"
    );
    if let Some(requirement) = adaptive_requirement {
        assert_eq!(requirement.level, RequirementLevel::High);
    }
}
