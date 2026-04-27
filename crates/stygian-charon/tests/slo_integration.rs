#![allow(clippy::expect_used, clippy::cast_precision_loss)]

use stygian_charon::{BlockedRatioSlo, investigate_har};

#[test]
fn slo_assessment_with_har_investigation() {
    // Example HAR response with elevated block ratio
    let har_json = r#"{
        "log": {
            "version": "1.2.0",
            "creator": {"name": "test", "version": "1.0"},
            "pages": [{"id": "page1", "title": "test", "startedDateTime": "2025-01-01T00:00:00Z", "pageTimings": {"onLoad": 0}}],
            "entries": [
                {
                    "pageref": "page1",
                    "startedDateTime": "2025-01-01T00:00:01Z",
                    "time": 0.1,
                    "request": {"method": "GET", "url": "https://example.com/api/data1", "headers": [], "queryString": [], "cookies": [], "headersSize": 0, "bodySize": 0},
                    "response": {"status": 200, "statusText": "OK", "headers": [], "cookies": [], "content": {"size": 100, "mimeType": "application/json"}, "redirectURL": "", "headersSize": 0, "bodySize": 100, "time": 0.05},
                    "cache": {},
                    "timings": {"blocked": 0, "dns": 0, "connect": 0, "send": 0, "wait": 50, "receive": 0}
                },
                {
                    "pageref": "page1",
                    "startedDateTime": "2025-01-01T00:00:02Z",
                    "time": 0.1,
                    "request": {"method": "GET", "url": "https://example.com/api/data2", "headers": [], "queryString": [], "cookies": [], "headersSize": 0, "bodySize": 0},
                    "response": {"status": 429, "statusText": "Too Many Requests", "headers": [], "cookies": [], "content": {"size": 50, "mimeType": "text/html"}, "redirectURL": "", "headersSize": 0, "bodySize": 50, "time": 0.05},
                    "cache": {},
                    "timings": {"blocked": 0, "dns": 0, "connect": 0, "send": 0, "wait": 50, "receive": 0}
                },
                {
                    "pageref": "page1",
                    "startedDateTime": "2025-01-01T00:00:03Z",
                    "time": 0.1,
                    "request": {"method": "GET", "url": "https://example.com/api/data3", "headers": [], "queryString": [], "cookies": [], "headersSize": 0, "bodySize": 0},
                    "response": {"status": 403, "statusText": "Forbidden", "headers": [], "cookies": [], "content": {"size": 50, "mimeType": "text/html"}, "redirectURL": "", "headersSize": 0, "bodySize": 50, "time": 0.05},
                    "cache": {},
                    "timings": {"blocked": 0, "dns": 0, "connect": 0, "send": 0, "wait": 50, "receive": 0}
                }
            ]
        }
    }"#;

    let report = investigate_har(har_json).expect("parse HAR");

    // Verify metrics
    assert_eq!(report.total_requests, 3);
    assert_eq!(report.blocked_requests, 2); // 429 and 403
    let blocked_ratio = report.blocked_requests as f64 / report.total_requests as f64;
    assert!((blocked_ratio - 2.0 / 3.0).abs() < 0.01);

    // Test API SLO assessment
    let api_slo = BlockedRatioSlo::api();
    let (acc, warn, crit) = api_slo.assess(blocked_ratio);
    assert!(!acc); // ~67% > 5%
    assert!(!warn); // ~67% > 10%
    assert!(crit); // ~67% > 15%

    // Test ContentSite SLO assessment
    let content_slo = BlockedRatioSlo::content_site();
    let (acc, warn, crit) = content_slo.assess(blocked_ratio);
    assert!(!acc); // ~67% > 15%
    assert!(!warn); // ~67% > 25%
    assert!(crit); // ~67% > 40%

    // Test HighSecurity SLO assessment
    let high_sec_slo = BlockedRatioSlo::high_security();
    let (acc, warn, crit) = high_sec_slo.assess(blocked_ratio);
    assert!(!acc); // ~67% > 30%
    assert!(!warn); // ~67% > 50%
    assert!(!crit); // ~67% <= 70%
}

#[test]
fn slo_assessment_for_each_target_class() {
    // Test 20% blocked ratio against all SLO classes
    let observed_ratio = 0.20;

    // API: blocked_ratio 20% is warning (10-15% warning zone, but 20 > 10, so not warning, is critical)
    let api_slo = BlockedRatioSlo::api();
    let (acc, warn, crit) = api_slo.assess(observed_ratio);
    assert!(!acc && !warn && crit); // 20% > 15% critical

    // ContentSite: 20% is acceptable (15% acceptable, warning at 25%)
    let content_slo = BlockedRatioSlo::content_site();
    let (acc, warn, crit) = content_slo.assess(observed_ratio);
    assert!(!acc && warn && !crit); // 15% < 20% <= 25%

    // HighSecurity: 20% is acceptable (30% acceptable)
    let high_sec_slo = BlockedRatioSlo::high_security();
    let (acc, warn, crit) = high_sec_slo.assess(observed_ratio);
    assert!(acc && !warn && !crit); // 20% <= 30%
}
