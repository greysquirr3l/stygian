#![allow(clippy::expect_used, clippy::cast_precision_loss, clippy::float_cmp)]

use stygian_charon::{
    BlockedRatioSlo, TargetClass, investigate_har, infer_requirements_with_target_class,
    build_runtime_policy, RequirementLevel,
};

/// Helper to construct a synthetic HAR with configurable blocked ratio.
/// 
/// This is used for testing the full SLO pipeline without needing real network calls.
fn make_test_har(total_requests: u32, blocked_requests: u32) -> String {
    let total_ms = total_requests as u64 * 100;
    let mut entries = String::new();
    
    for i in 0..total_requests {
        let status = if i < blocked_requests { 403 } else { 200 };
        entries.push_str(&format!(
            r#"{{
                "pageref": "page1",
                "startedDateTime": "2025-01-01T00:00:{:02}Z",
                "time": 0.1,
                "request": {{
                    "method": "GET",
                    "url": "https://example.com/api/resource{}",
                    "headers": [],
                    "queryString": [],
                    "cookies": [],
                    "headersSize": 0,
                    "bodySize": 0
                }},
                "response": {{
                    "status": {},
                    "statusText": "{}",
                    "headers": [],
                    "cookies": [],
                    "content": {{"size": 100, "mimeType": "application/json"}},
                    "redirectURL": "",
                    "headersSize": 0,
                    "bodySize": 100,
                    "time": 0.05
                }},
                "cache": {{}},
                "timings": {{"blocked": -1, "dns": -1, "connect": -1, "send": 0, "wait": 50, "receive": 50}}
            }}"#,
            i % 60,
            i,
            status,
            if status == 403 { "Forbidden" } else { "OK" }
        ));
        
        if i < total_requests - 1 {
            entries.push(',');
        }
    }

    format!(
        r#"{{
            "log": {{
                "version": "1.2.0",
                "creator": {{"name": "test", "version": "1.0"}},
                "pages": [{{
                    "id": "page1",
                    "title": "test",
                    "startedDateTime": "2025-01-01T00:00:00Z",
                    "pageTimings": {{"onLoad": {}}}
                }}],
                "entries": [{}]
            }}
        }}"#,
        total_ms, entries
    )
}

#[test]
fn chr011_api_target_acceptable_zone() {
    // API class: 5% acceptable, 10% warning, 15% critical
    // Test: 3% blocked ratio (acceptable)
    let har = make_test_har(100, 3);
    let report = investigate_har(&har).expect("investigation failed");
    assert_eq!(report.total_requests, 100);
    assert_eq!(report.blocked_requests, 3);

    let slo = BlockedRatioSlo::for_class(TargetClass::Api);
    let blocked_ratio = report.blocked_requests as f64 / report.total_requests as f64;
    let (acceptable, warning, critical) = slo.assess(blocked_ratio);

    assert!(acceptable, "3% should be acceptable for API");
    assert!(!warning, "3% should not trigger warning");
    assert!(!critical, "3% should not trigger critical");

    // Verify no escalation needed
    let requirements = infer_requirements_with_target_class(&report, TargetClass::Api);
    let has_adaptive = requirements
        .requirements
        .iter()
        .any(|r| r.id == "adaptive_rate_and_retry_budget");
    assert!(!has_adaptive, "acceptable SLO should not trigger adaptive requirement");
}

#[test]
fn chr011_api_target_warning_zone() {
    // API class: 5% acceptable, 10% warning, 15% critical
    // Test: 7% blocked ratio (warning)
    let har = make_test_har(100, 7);
    let report = investigate_har(&har).expect("investigation failed");

    let slo = BlockedRatioSlo::for_class(TargetClass::Api);
    let blocked_ratio = report.blocked_requests as f64 / report.total_requests as f64;
    let (acceptable, warning, critical) = slo.assess(blocked_ratio);

    assert!(!acceptable, "7% should exceed API acceptable");
    assert!(warning, "7% should be in warning zone");
    assert!(!critical, "7% should not reach critical");

    // Verify adaptive requirement triggered with Medium level
    let requirements = infer_requirements_with_target_class(&report, TargetClass::Api);
    let adaptive = requirements
        .requirements
        .iter()
        .find(|r| r.id == "adaptive_rate_and_retry_budget")
        .expect("adaptive requirement should be present");
    assert_eq!(adaptive.level, RequirementLevel::Medium, "warning zone should trigger Medium level");
}

#[test]
fn chr011_api_target_critical_zone() {
    // API class: 5% acceptable, 10% warning, 15% critical
    // Test: 20% blocked ratio (critical)
    let har = make_test_har(100, 20);
    let report = investigate_har(&har).expect("investigation failed");

    let slo = BlockedRatioSlo::for_class(TargetClass::Api);
    let blocked_ratio = report.blocked_requests as f64 / report.total_requests as f64;
    let (acceptable, warning, critical) = slo.assess(blocked_ratio);

    assert!(!acceptable, "20% should exceed API acceptable");
    assert!(!warning, "20% should exceed API warning");
    assert!(critical, "20% should reach critical");

    // Verify adaptive requirement triggered with High level
    let requirements = infer_requirements_with_target_class(&report, TargetClass::Api);
    let adaptive = requirements
        .requirements
        .iter()
        .find(|r| r.id == "adaptive_rate_and_retry_budget")
        .expect("adaptive requirement should be present");
    assert_eq!(adaptive.level, RequirementLevel::High, "critical zone should trigger High level");
}

#[test]
fn chr011_content_site_target_acceptable_zone() {
    // ContentSite class: 15% acceptable, 25% warning, 40% critical
    // Test: 12% blocked ratio (acceptable)
    let har = make_test_har(100, 12);
    let report = investigate_har(&har).expect("investigation failed");

    let slo = BlockedRatioSlo::for_class(TargetClass::ContentSite);
    let blocked_ratio = report.blocked_requests as f64 / report.total_requests as f64;
    let (acceptable, warning, critical) = slo.assess(blocked_ratio);

    assert!(acceptable, "12% should be acceptable for ContentSite");
    assert!(!warning, "12% should not trigger warning");
    assert!(!critical, "12% should not trigger critical");

    let requirements = infer_requirements_with_target_class(&report, TargetClass::ContentSite);
    let has_adaptive = requirements
        .requirements
        .iter()
        .any(|r| r.id == "adaptive_rate_and_retry_budget");
    assert!(!has_adaptive, "acceptable SLO should not trigger adaptive requirement");
}

#[test]
fn chr011_content_site_target_warning_zone() {
    // ContentSite class: 15% acceptable, 25% warning, 40% critical
    // Test: 20% blocked ratio (warning)
    let har = make_test_har(100, 20);
    let report = investigate_har(&har).expect("investigation failed");

    let slo = BlockedRatioSlo::for_class(TargetClass::ContentSite);
    let blocked_ratio = report.blocked_requests as f64 / report.total_requests as f64;
    let (acceptable, warning, critical) = slo.assess(blocked_ratio);

    assert!(!acceptable, "20% should exceed ContentSite acceptable");
    assert!(warning, "20% should be in warning zone");
    assert!(!critical, "20% should not reach critical");

    let requirements = infer_requirements_with_target_class(&report, TargetClass::ContentSite);
    let adaptive = requirements
        .requirements
        .iter()
        .find(|r| r.id == "adaptive_rate_and_retry_budget")
        .expect("adaptive requirement should be present");
    assert_eq!(adaptive.level, RequirementLevel::Medium, "warning zone should trigger Medium level");
}

#[test]
fn chr011_content_site_target_critical_zone() {
    // ContentSite class: 15% acceptable, 25% warning, 40% critical
    // Test: 45% blocked ratio (critical)
    let har = make_test_har(100, 45);
    let report = investigate_har(&har).expect("investigation failed");

    let slo = BlockedRatioSlo::for_class(TargetClass::ContentSite);
    let blocked_ratio = report.blocked_requests as f64 / report.total_requests as f64;
    let (acceptable, warning, critical) = slo.assess(blocked_ratio);

    assert!(!acceptable, "45% should exceed ContentSite acceptable");
    assert!(!warning, "45% should exceed ContentSite warning");
    assert!(critical, "45% should reach critical");

    let requirements = infer_requirements_with_target_class(&report, TargetClass::ContentSite);
    let adaptive = requirements
        .requirements
        .iter()
        .find(|r| r.id == "adaptive_rate_and_retry_budget")
        .expect("adaptive requirement should be present");
    assert_eq!(adaptive.level, RequirementLevel::High, "critical zone should trigger High level");
}

#[test]
fn chr011_high_security_target_acceptable_zone() {
    // HighSecurity class: 30% acceptable, 50% warning, 70% critical
    // Test: 25% blocked ratio (acceptable)
    let har = make_test_har(100, 25);
    let report = investigate_har(&har).expect("investigation failed");

    let slo = BlockedRatioSlo::for_class(TargetClass::HighSecurity);
    let blocked_ratio = report.blocked_requests as f64 / report.total_requests as f64;
    let (acceptable, warning, critical) = slo.assess(blocked_ratio);

    assert!(acceptable, "25% should be acceptable for HighSecurity");
    assert!(!warning, "25% should not trigger warning");
    assert!(!critical, "25% should not trigger critical");

    let requirements = infer_requirements_with_target_class(&report, TargetClass::HighSecurity);
    let has_adaptive = requirements
        .requirements
        .iter()
        .any(|r| r.id == "adaptive_rate_and_retry_budget");
    assert!(!has_adaptive, "acceptable SLO should not trigger adaptive requirement");
}

#[test]
fn chr011_high_security_target_warning_zone() {
    // HighSecurity class: 30% acceptable, 50% warning, 70% critical
    // Test: 40% blocked ratio (warning)
    let har = make_test_har(100, 40);
    let report = investigate_har(&har).expect("investigation failed");

    let slo = BlockedRatioSlo::for_class(TargetClass::HighSecurity);
    let blocked_ratio = report.blocked_requests as f64 / report.total_requests as f64;
    let (acceptable, warning, critical) = slo.assess(blocked_ratio);

    assert!(!acceptable, "40% should exceed HighSecurity acceptable");
    assert!(warning, "40% should be in warning zone");
    assert!(!critical, "40% should not reach critical");

    let requirements = infer_requirements_with_target_class(&report, TargetClass::HighSecurity);
    let adaptive = requirements
        .requirements
        .iter()
        .find(|r| r.id == "adaptive_rate_and_retry_budget")
        .expect("adaptive requirement should be present");
    assert_eq!(adaptive.level, RequirementLevel::Medium, "warning zone should trigger Medium level");
}

#[test]
fn chr011_high_security_target_critical_zone() {
    // HighSecurity class: 30% acceptable, 50% warning, 70% critical
    // Test: 75% blocked ratio (critical)
    let har = make_test_har(100, 75);
    let report = investigate_har(&har).expect("investigation failed");

    let slo = BlockedRatioSlo::for_class(TargetClass::HighSecurity);
    let blocked_ratio = report.blocked_requests as f64 / report.total_requests as f64;
    let (acceptable, warning, critical) = slo.assess(blocked_ratio);

    assert!(!acceptable, "75% should exceed HighSecurity acceptable");
    assert!(!warning, "75% should exceed HighSecurity warning");
    assert!(critical, "75% should reach critical");

    let requirements = infer_requirements_with_target_class(&report, TargetClass::HighSecurity);
    let adaptive = requirements
        .requirements
        .iter()
        .find(|r| r.id == "adaptive_rate_and_retry_budget")
        .expect("adaptive requirement should be present");
    assert_eq!(adaptive.level, RequirementLevel::High, "critical zone should trigger High level");
}

#[test]
fn chr011_escalation_policy_builds_correctly() {
    // Test that escalation logic in policy.rs applies SLO-based adjustments
    let har = make_test_har(100, 20); // 20% blocked (API critical)
    let report = investigate_har(&har).expect("investigation failed");
    let requirements = infer_requirements_with_target_class(&report, TargetClass::Api);

    // Build policy with escalation
    let policy = build_runtime_policy(&report, &requirements);

    // In critical zone, escalation should:
    // - Reduce rate_limit_rps to min(1.5) 
    // - Increase max_retries
    // - Increase backoff_base_ms
    // - Enable sticky session
    assert!(
        policy.rate_limit_rps <= 1.5,
        "critical escalation should reduce RPS to <= 1.5, got {}",
        policy.rate_limit_rps
    );
    assert!(
        policy.max_retries >= 5,
        "critical escalation should ensure max_retries >= 5, got {}",
        policy.max_retries
    );
    assert!(
        policy.backoff_base_ms >= 600,
        "critical escalation should ensure backoff_base_ms >= 600, got {}",
        policy.backoff_base_ms
    );
    assert!(
        policy.sticky_session_ttl_secs.is_some(),
        "critical escalation should enable sticky session"
    );
}

#[test]
fn chr011_boundary_acceptable_to_warning_transition() {
    // Test exact boundary: API 5% acceptable → 5.001% warning
    let har_boundary = make_test_har(1000, 50); // 5% exactly
    let report_boundary = investigate_har(&har_boundary).expect("investigation failed");
    let slo = BlockedRatioSlo::api();
    let blocked_ratio = report_boundary.blocked_requests as f64 / report_boundary.total_requests as f64;
    let (acceptable, warning, _) = slo.assess(blocked_ratio);

    assert!(acceptable, "exactly 5% should be acceptable (<=)");
    assert!(!warning, "exactly 5% should not trigger warning (>)");

    // Now test just over: 51/1000 = 5.1%
    let har_over = make_test_har(1000, 51);
    let report_over = investigate_har(&har_over).expect("investigation failed");
    let blocked_ratio_over = report_over.blocked_requests as f64 / report_over.total_requests as f64;
    let (acceptable_over, warning_over, _) = slo.assess(blocked_ratio_over);

    assert!(!acceptable_over, "5.1% should exceed acceptable");
    assert!(warning_over, "5.1% should trigger warning");
}

#[test]
fn chr011_unknown_target_class_falls_back_to_api() {
    // Unknown class should use same thresholds as API
    let har = make_test_har(100, 7);
    let report = investigate_har(&har).expect("investigation failed");

    let slo_api = BlockedRatioSlo::api();
    let slo_unknown = BlockedRatioSlo::for_class(TargetClass::Unknown);

    assert_eq!(slo_api.acceptable, slo_unknown.acceptable);
    assert_eq!(slo_api.warning, slo_unknown.warning);
    assert_eq!(slo_api.critical, slo_unknown.critical);

    // Both should assess the same way
    let blocked_ratio = report.blocked_requests as f64 / report.total_requests as f64;
    let api_result = slo_api.assess(blocked_ratio);
    let unknown_result = slo_unknown.assess(blocked_ratio);

    assert_eq!(api_result, unknown_result);
}

#[test]
fn chr011_mixed_status_codes_counted_correctly() {
    // Test that the HAR parser counts blocked (403, 429) correctly
    // Note: 500 is not counted as blocked (it's a server error, not anti-bot blocking)
    // Make a HAR with mixed responses: 200, 403, 429, 500
    let har = r#"{
        "log": {
            "version": "1.2.0",
            "creator": {"name": "test", "version": "1.0"},
            "pages": [{"id": "page1", "title": "test", "startedDateTime": "2025-01-01T00:00:00Z", "pageTimings": {"onLoad": 0}}],
            "entries": [
                {"pageref": "page1", "startedDateTime": "2025-01-01T00:00:01Z", "time": 0.1, "request": {"method": "GET", "url": "https://example.com/1", "headers": [], "queryString": [], "cookies": [], "headersSize": 0, "bodySize": 0}, "response": {"status": 200, "statusText": "OK", "headers": [], "cookies": [], "content": {"size": 100, "mimeType": "application/json"}, "redirectURL": "", "headersSize": 0, "bodySize": 100, "time": 0.05}, "cache": {}, "timings": {"blocked": -1, "dns": -1, "connect": -1, "send": 0, "wait": 50, "receive": 50}},
                {"pageref": "page1", "startedDateTime": "2025-01-01T00:00:02Z", "time": 0.1, "request": {"method": "GET", "url": "https://example.com/2", "headers": [], "queryString": [], "cookies": [], "headersSize": 0, "bodySize": 0}, "response": {"status": 403, "statusText": "Forbidden", "headers": [], "cookies": [], "content": {"size": 100, "mimeType": "application/json"}, "redirectURL": "", "headersSize": 0, "bodySize": 100, "time": 0.05}, "cache": {}, "timings": {"blocked": -1, "dns": -1, "connect": -1, "send": 0, "wait": 50, "receive": 50}},
                {"pageref": "page1", "startedDateTime": "2025-01-01T00:00:03Z", "time": 0.1, "request": {"method": "GET", "url": "https://example.com/3", "headers": [], "queryString": [], "cookies": [], "headersSize": 0, "bodySize": 0}, "response": {"status": 429, "statusText": "Too Many Requests", "headers": [], "cookies": [], "content": {"size": 100, "mimeType": "application/json"}, "redirectURL": "", "headersSize": 0, "bodySize": 100, "time": 0.05}, "cache": {}, "timings": {"blocked": -1, "dns": -1, "connect": -1, "send": 0, "wait": 50, "receive": 50}},
                {"pageref": "page1", "startedDateTime": "2025-01-01T00:00:04Z", "time": 0.1, "request": {"method": "GET", "url": "https://example.com/4", "headers": [], "queryString": [], "cookies": [], "headersSize": 0, "bodySize": 0}, "response": {"status": 500, "statusText": "Internal Server Error", "headers": [], "cookies": [], "content": {"size": 100, "mimeType": "application/json"}, "redirectURL": "", "headersSize": 0, "bodySize": 100, "time": 0.05}, "cache": {}, "timings": {"blocked": -1, "dns": -1, "connect": -1, "send": 0, "wait": 50, "receive": 50}}
            ]
        }
    }"#;

    let report = investigate_har(har).expect("investigation failed");
    assert_eq!(report.total_requests, 4);
    assert_eq!(report.blocked_requests, 2, "only 403 and 429 should count as blocked (not 500)");
}
