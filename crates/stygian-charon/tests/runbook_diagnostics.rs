#![allow(clippy::cast_precision_loss)]

use serde_json::json;
use stygian_charon::{
    AntiBotProvider, RequirementLevel, TargetClass, infer_requirements_with_target_class,
    investigate_har,
};

fn diagnostic_output(
    category: &str,
    signal_id: &str,
    escalation: RequirementLevel,
    resolution_path: &str,
) -> serde_json::Value {
    json!({
        "category": category,
        "signal_id": signal_id,
        "escalation_level": format!("{escalation:?}"),
        "resolution_path": resolution_path,
        "actions": [
            "collect_har",
            "run_investigate_har",
            "apply_recommended_escalation"
        ]
    })
}

fn build_har(entries: &[serde_json::Value]) -> String {
    json!({
        "log": {
            "version": "1.2",
            "creator": {"name": "runbook-diagnostics", "version": "1.0"},
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

fn har_entry(
    index: u32,
    status: u16,
    resource_type: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> serde_json::Value {
    let response_headers = headers
        .iter()
        .map(|(name, value)| json!({"name": name, "value": value}))
        .collect::<Vec<_>>();

    json!({
        "pageref": "page_1",
        "startedDateTime": format!("2026-01-01T00:00:{:02}.000Z", index % 60),
        "time": 0,
        "_resourceType": resource_type,
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
            "headers": response_headers,
            "cookies": [],
            "content": {"size": body.len(), "mimeType": "text/html", "text": body},
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
    })
}

#[test]
fn runbook_category_a_detects_fingerprint_identity_regression_path() {
    let entries = vec![
        har_entry(
            0,
            403,
            "document",
            &[("x-datadome", "blocked"), ("x-dd-b", "v1")],
            "captcha-delivery.com challenge",
        ),
        har_entry(1, 200, "xhr", &[], "ok"),
    ];
    let har = build_har(&entries);

    let report_result = investigate_har(&har);
    assert!(report_result.is_ok(), "category A HAR should parse");
    let Ok(report) = report_result else {
        return;
    };

    assert_eq!(report.aggregate.provider, AntiBotProvider::DataDome);

    let profile = infer_requirements_with_target_class(&report, TargetClass::HighSecurity);
    let fingerprint_req = profile
        .requirements
        .iter()
        .find(|requirement| requirement.id == "fingerprint_and_identity_consistency");

    assert!(
        fingerprint_req.is_some(),
        "expected identity requirement for Category A"
    );
    if let Some(requirement) = fingerprint_req {
        assert_eq!(requirement.level, RequirementLevel::High);

        let output = diagnostic_output(
            "A",
            "fingerprint_and_identity_consistency",
            requirement.level,
            "browser-fingerprint-and-identity",
        );
        assert_eq!(output.get("category"), Some(&json!("A")));
        assert_eq!(
            output.get("signal_id"),
            Some(&json!("fingerprint_and_identity_consistency"))
        );
        assert_eq!(
            output.get("resolution_path"),
            Some(&json!("browser-fingerprint-and-identity"))
        );
        assert!(
            output
                .get("actions")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|actions| !actions.is_empty()),
            "diagnostic output should include actionable steps"
        );
    }
}

#[test]
fn runbook_category_b_detects_rate_limit_spike_path() {
    let mut entries = Vec::new();
    for index in 0..10 {
        let status = if index < 4 { 429 } else { 200 };
        entries.push(har_entry(index, status, "xhr", &[], "ok"));
    }

    let har = build_har(&entries);
    let report_result = investigate_har(&har);
    assert!(report_result.is_ok(), "category B HAR should parse");
    let Ok(report) = report_result else {
        return;
    };

    assert_eq!(report.total_requests, 10);
    assert_eq!(report.blocked_requests, 4);

    let profile = infer_requirements_with_target_class(&report, TargetClass::Api);
    let adaptive_req = profile
        .requirements
        .iter()
        .find(|requirement| requirement.id == "adaptive_rate_and_retry_budget");
    let backoff_req = profile
        .requirements
        .iter()
        .find(|requirement| requirement.id == "rate_limit_backoff");

    assert!(
        adaptive_req.is_some(),
        "expected adaptive requirement for Category B"
    );
    assert!(
        backoff_req.is_some(),
        "expected rate_limit_backoff requirement for Category B"
    );
    if let Some(requirement) = adaptive_req {
        assert_eq!(requirement.level, RequirementLevel::High);

        let output = diagnostic_output(
            "B",
            "adaptive_rate_and_retry_budget",
            requirement.level,
            "rate-limit-and-pacing",
        );
        assert_eq!(output.get("category"), Some(&json!("B")));
        assert_eq!(
            output.get("signal_id"),
            Some(&json!("adaptive_rate_and_retry_budget"))
        );
        assert_eq!(
            output.get("resolution_path"),
            Some(&json!("rate-limit-and-pacing"))
        );
    }
}

#[test]
fn runbook_category_c_detects_preflight_header_fidelity_path() {
    let entries = vec![
        har_entry(
            0,
            200,
            "preflight",
            &[("access-control-allow-origin", "*")],
            "",
        ),
        har_entry(1, 200, "xhr", &[], "ok"),
    ];
    let har = build_har(&entries);

    let report_result = investigate_har(&har);
    assert!(report_result.is_ok(), "category C HAR should parse");
    let Ok(report) = report_result else {
        return;
    };

    let preflight_count = report
        .resource_type_histogram
        .get("preflight")
        .copied()
        .unwrap_or_default();
    assert_eq!(preflight_count, 1);

    let profile = infer_requirements_with_target_class(&report, TargetClass::ContentSite);
    let cors_req = profile
        .requirements
        .iter()
        .find(|requirement| requirement.id == "cors_and_header_fidelity");
    assert!(
        cors_req.is_some(),
        "expected CORS/header fidelity requirement for Category C"
    );
    if cors_req.is_some() {
        let output = diagnostic_output(
            "C",
            "cors_and_header_fidelity",
            RequirementLevel::Medium,
            "cors-and-header-fidelity",
        );
        assert_eq!(output.get("category"), Some(&json!("C")));
        assert_eq!(
            output.get("signal_id"),
            Some(&json!("cors_and_header_fidelity"))
        );
        assert_eq!(
            output.get("resolution_path"),
            Some(&json!("cors-and-header-fidelity"))
        );
    }
}
