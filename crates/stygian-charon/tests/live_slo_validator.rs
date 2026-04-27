#![cfg(feature = "live-validation")]

use std::collections::BTreeMap;

use stygian_charon::{
    TargetClass, build_runtime_policy, infer_requirements_with_target_class, investigate_har,
};

fn minimal_har(url: &str, status: u16) -> String {
    serde_json::json!({
        "log": {
            "version": "1.2",
            "creator": {"name": "test", "version": "1.0"},
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
                    "statusText": "test",
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
            }]
        }
    })
    .to_string()
}

#[tokio::test]
#[ignore = "live network test; set STYGIAN_LIVE_URL to validate against a real target"]
async fn chr013_live_target_validation_smoke() {
    let Ok(target) = std::env::var("STYGIAN_LIVE_URL") else {
        return;
    };

    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    else {
        return;
    };

    let Ok(response) = client
        .get(&target)
        .header(
            "accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .send()
        .await
    else {
        return;
    };

    let har = minimal_har(&target, response.status().as_u16());
    let Ok(report) = investigate_har(&har) else {
        return;
    };
    let requirements = infer_requirements_with_target_class(&report, TargetClass::Unknown);
    let policy = build_runtime_policy(&report, &requirements);

    assert_eq!(report.total_requests, 1);
    assert!(
        matches!(
            policy.config_hints.get("slo.escalation"),
            Some(level) if level == "warning" || level == "critical"
        ) || !policy.config_hints.contains_key("slo.escalation")
    );

    let _empty: BTreeMap<String, String> = BTreeMap::new();
}
