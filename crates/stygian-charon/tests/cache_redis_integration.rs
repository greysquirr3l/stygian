#![cfg(feature = "redis-cache")]

use std::time::Duration;

use stygian_charon::{
    InvestigationReportCache, RedisInvestigationCache, TargetClass,
    investigate_har_cached_with_target_class, investigation_cache_key,
};

fn minimal_har(status: u16) -> String {
    serde_json::json!({
        "log": {
            "version": "1.2",
            "creator": {"name": "redis-cache-test", "version": "1.0"},
            "pages": [{
                "id": "page_1",
                "title": "https://example.com",
                "startedDateTime": "2026-01-01T00:00:00.000Z",
                "pageTimings": {"onLoad": 0}
            }],
            "entries": [{
                "pageref": "page_1",
                "startedDateTime": "2026-01-01T00:00:00.000Z",
                "time": 0,
                "_resourceType": "document",
                "request": {
                    "method": "GET",
                    "url": "https://example.com",
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
            }]
        }
    })
    .to_string()
}

#[test]
#[ignore = "requires running Redis; set STYGIAN_REDIS_URL to execute"]
fn redis_cache_round_trip_and_invalidation() {
    let Ok(redis_url) = std::env::var("STYGIAN_REDIS_URL") else {
        return;
    };

    let cache_result = RedisInvestigationCache::new(&redis_url, Duration::from_secs(60));
    assert!(cache_result.is_ok(), "redis cache should initialize");
    let Ok(cache) = cache_result else {
        return;
    };

    cache.clear();

    let har = minimal_har(403);
    let key = investigation_cache_key(&har, TargetClass::Api);

    let report_result = investigate_har_cached_with_target_class(&har, TargetClass::Api, &cache);
    assert!(report_result.is_ok(), "cached investigation should succeed");

    let cached = cache.get(&key);
    assert!(cached.is_some(), "redis cache should contain stored report");

    cache.invalidate(&key);
    let removed = cache.get(&key);
    assert!(removed.is_none(), "invalidate should remove cached report");

    cache.clear();
}
