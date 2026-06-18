#![allow(
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::float_cmp,
    clippy::cast_lossless,
    clippy::similar_names,
    clippy::uninlined_format_args,
    clippy::format_push_string
)]

//! T89 — Vendor fingerprinting confidence classifier integration tests.
//!
//! Exercises the end-to-end vendor classification path:
//! - The classifier matches baseline Tier 1 vendor signals in cookies,
//!   headers, challenge URLs, and body markers.
//! - Multi-vendor ranking produces a deterministic order
//!   (top score DESC, then `VendorId` discriminant ASC).
//! - The classification is attached to the [`DiagnosticBundle`] via
//!   the new additive `vendor_classification` field.
//!
//! The `#[ignore]`-gated `vendor_classification_appears_in_diagnostic_payload`
//! test confirms the wire-level contract: the JSON form of a bundle
//! produced from a Cloudflare-flavored HAR contains the
//! `vendor_classification` field with the expected top vendor.

use serde_json::json;
use stygian_charon::bundle::{build_diagnostic_bundle, BundleRedactionPolicy};
use stygian_charon::vendor_classifier::{
    VendorClassifier, VendorId, DEFAULT_HIGH_CONFIDENCE_THRESHOLD, EvidenceSource,
};
use std::collections::BTreeMap;

fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn headers_kv(pairs: &[(&str, &str)]) -> serde_json::Value {
    let arr: Vec<serde_json::Value> = pairs
        .iter()
        .map(|(k, v)| json!({"name": k, "value": v}))
        .collect();
    serde_json::Value::Array(arr)
}

fn make_entry(
    url: &str,
    status: u16,
    response_headers: &serde_json::Value,
    cookies: &[(&str, &str)],
    body: &str,
) -> serde_json::Value {
    // Cookies are surfaced to the classifier via the
    // `set-cookie` header. The HAR parser only consumes
    // `response.headers` for the headers map, so the separate
    // `response.cookies` array is dropped.
    let mut headers = response_headers
        .as_array()
        .cloned()
        .unwrap_or_default();
    for (name, value) in cookies {
        headers.push(json!({"name": "set-cookie", "value": format!("{name}={value}; Path=/")}));
    }
    let headers_value = serde_json::Value::Array(headers);
    json!({
        "request": {
            "method": "GET",
            "url": url,
            "httpVersion": "HTTP/1.1",
            "headers": [],
            "queryString": [],
            "cookies": [],
            "headersSize": -1,
            "bodySize": -1,
        },
        "response": {
            "status": status,
            "statusText": "",
            "httpVersion": "HTTP/1.1",
            "headers": headers_value,
            "cookies": [],
            "content": {
                "size": body.len(),
                "mimeType": "text/html",
                "text": body,
            },
            "redirectURL": "",
            "headersSize": -1,
            "bodySize": body.len(),
        },
        "cache": {},
        "timings": {"send": 0, "wait": 100, "receive": 0},
    })
}

fn build_har(entries: &[serde_json::Value]) -> String {
    json!({
        "log": {
            "version": "1.2",
            "creator": {"name": "t89-fixture", "version": "1.0"},
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

// =====================================================================
// Single-vendor scenarios
// =====================================================================

#[test]
fn datadome_classification_from_cookies_and_headers() {
    let classifier = VendorClassifier::with_builtin_defaults();
    let cookies = vec![
        "datadome=abc123; Path=/".to_string(),
        "dd_test=1; Path=/".to_string(),
    ];
    let mut headers = BTreeMap::new();
    headers.insert("x-datadome".to_string(), "protected".to_string());
    headers.insert("x-datadome-cid".to_string(), "abc".to_string());
    let c = classifier.classify(&cookies, &headers, Some("captcha-delivery.com iframe"), "https://www.example.com/");
    assert_eq!(c.top_vendor, VendorId::DataDome);
    assert!(c.is_high_confidence);
    assert!(c.confidence > 0.5);
    assert!(c.evidence.source_summary.contains_key(&EvidenceSource::Cookie));
    assert!(c.evidence.source_summary.contains_key(&EvidenceSource::Header));
    assert!(c.evidence.source_summary.contains_key(&EvidenceSource::BodyMarker));
}

#[test]
fn cloudflare_classification_from_challenge_url_and_ray_header() {
    let classifier = VendorClassifier::with_builtin_defaults();
    let cookies = vec!["__cf_bm=xyz; Path=/".to_string()];
    let mut headers = BTreeMap::new();
    headers.insert("cf-ray".to_string(), "abc-LHR".to_string());
    headers.insert("server".to_string(), "cloudflare".to_string());
    let c = classifier.classify(
        &cookies,
        &headers,
        Some("Attention required! | cloudflare"),
        "https://example.com/cdn-cgi/challenge-platform/orchestrate/jschl/abc",
    );
    assert_eq!(c.top_vendor, VendorId::Cloudflare);
    assert!(c.is_high_confidence);
    assert!(c.evidence.source_summary.contains_key(&EvidenceSource::ChallengeUrl));
}

#[test]
fn akamai_classification_from_abck_and_bm_sz() {
    let classifier = VendorClassifier::with_builtin_defaults();
    let cookies = vec![
        "_abck=abc123; Path=/".to_string(),
        "bm_sz=12345; Path=/".to_string(),
    ];
    let mut headers = BTreeMap::new();
    headers.insert("bm_sv".to_string(), "abc".to_string());
    let c = classifier.classify(&cookies, &headers, Some("akamaibot detection"), "https://example.com/_bm/v3/abc");
    assert_eq!(c.top_vendor, VendorId::Akamai);
    assert!(c.is_high_confidence);
    assert!(c.evidence.source_summary.contains_key(&EvidenceSource::Cookie));
}

#[test]
fn perimeter_x_classification_from_px3_and_body() {
    let classifier = VendorClassifier::with_builtin_defaults();
    let cookies = vec!["_px3=abc; Path=/".to_string()];
    let mut headers = BTreeMap::new();
    headers.insert("x-px".to_string(), "abc".to_string());
    let c = classifier.classify(&cookies, &headers, Some("perimeterx / humansecurity challenge"), "https://example.com/1/captcha/abc");
    assert_eq!(c.top_vendor, VendorId::PerimeterX);
    assert!(c.is_high_confidence);
    assert!(c.evidence.source_summary.contains_key(&EvidenceSource::Cookie));
}

// =====================================================================
// Multi-vendor ranking + tie-break
// =====================================================================

#[test]
fn multi_vendor_classification_with_deterministic_tie_break() {
    let classifier = VendorClassifier::with_builtin_defaults();
    // Construct input that hits both DataDome (x-datadome) and
    // Cloudflare (cf-ray) at the same weight. The VendorId
    // discriminant order tie-break should make Cloudflare win
    // (lower discriminant than DataDome).
    let mut headers = BTreeMap::new();
    headers.insert("x-datadome".to_string(), "1".to_string());
    headers.insert("cf-ray".to_string(), "1".to_string());
    let c = classifier.classify(&[], &headers, None, "https://example.com/");
    assert_eq!(c.top_vendor, VendorId::Cloudflare);
    assert!((c.confidence - 0.5).abs() < 1e-9);
    // The ranked list should still carry both vendors with the
    // correct scores so downstream observers can audit the result.
    assert!(c.ranked.iter().any(|s| s.vendor == VendorId::DataDome && s.score == 5));
    assert!(c.ranked.iter().any(|s| s.vendor == VendorId::Cloudflare && s.score == 5));
}

#[test]
fn no_signals_yields_unknown_classification() {
    let classifier = VendorClassifier::with_builtin_defaults();
    let c = classifier.classify(&[], &BTreeMap::new(), Some("benign html"), "https://example.com/");
    assert!(c.is_unknown());
    assert!(!c.is_high_confidence);
    assert_eq!(c.top_vendor, VendorId::Unknown);
}

// =====================================================================
// Threshold + edge cases
// =====================================================================

#[test]
fn below_threshold_classification_is_not_high_confidence() {
    let classifier = VendorClassifier::with_builtin_defaults().with_threshold(0.99);
    let mut headers = BTreeMap::new();
    headers.insert("x-datadome".to_string(), "1".to_string());
    headers.insert("cf-ray".to_string(), "1".to_string());
    let c = classifier.classify(&[], &headers, None, "https://example.com/");
    // 0.5 confidence is below 0.99 threshold.
    assert!(!c.is_high_confidence);
}

#[test]
fn default_threshold_matches_documented_constant() {
    let classifier = VendorClassifier::with_builtin_defaults();
    assert!(approx_eq(classifier.threshold(), DEFAULT_HIGH_CONFIDENCE_THRESHOLD));
    assert!(approx_eq(classifier.threshold(), 0.60));
}

#[test]
fn classification_is_deterministic_for_same_input() {
    let classifier = VendorClassifier::with_builtin_defaults();
    let cookies = vec!["datadome=abc; Path=/".to_string()];
    let mut headers = BTreeMap::new();
    headers.insert("x-datadome".to_string(), "1".to_string());
    let c1 = classifier.classify(&cookies, &headers, Some("captcha-delivery.com"), "https://example.com/");
    let c2 = classifier.classify(&cookies, &headers, Some("captcha-delivery.com"), "https://example.com/");
    assert_eq!(c1.top_vendor, c2.top_vendor);
    assert!(approx_eq(c1.confidence, c2.confidence));
    assert_eq!(c1.ranked.len(), c2.ranked.len());
}

// =====================================================================
// DiagnosticBundle additive field
// =====================================================================

#[test]
fn diagnostic_bundle_carries_vendor_classification_for_datadome_input() {
    let entry = make_entry(
        "https://www.g2.com/products",
        403,
        &headers_kv(&[("x-datadome", "protected"), ("x-datadome-cid", "abc")]),
        &[("datadome", "xyz")],
        "Please enable JS and complete the captcha-delivery.com iframe.",
    );
    let har = build_har(&[entry]);
    let bundle = build_diagnostic_bundle(&har, BundleRedactionPolicy::None)
        .expect("bundle");
    let classification = bundle
        .vendor_classification
        .as_ref()
        .expect("vendor classification populated");
    assert_eq!(classification.top_vendor, VendorId::DataDome);
    assert!(classification.is_high_confidence);
    assert!(classification.confidence > 0.0);
    // Evidence bundle records at least one cookie + one header hit.
    let summary = &classification.evidence.source_summary;
    assert!(summary.contains_key(&EvidenceSource::Cookie));
    assert!(summary.contains_key(&EvidenceSource::Header));
}

#[test]
fn diagnostic_bundle_omits_vendor_classification_for_clean_har() {
    let entry = make_entry(
        "https://example.com/page",
        200,
        &headers_kv(&[("content-type", "text/html")]),
        &[],
        "harmless content",
    );
    let har = build_har(&[entry]);
    let bundle = build_diagnostic_bundle(&har, BundleRedactionPolicy::None)
        .expect("bundle");
    // The classifier reports `Unknown` with no evidence for a
    // benign HAR. The bundle builder drops the field rather
    // than emit `"vendor_classification": null`.
    assert!(bundle.vendor_classification.is_none());
}

#[test]
fn diagnostic_bundle_vendor_classification_is_backward_compatible_with_omitted_field() {
    // Older JSON payloads (pre-T89) lack the
    // `vendor_classification` field. The field is marked
    // `#[serde(default, skip_serializing_if = "Option::is_none")]`
    // so deserialisation should still succeed.
    let json = json!({
        "metadata": {
            "schema_version": "1.0",
            "assembled_at": "unix:0",
            "redaction_policy": "Standard",
            "annotations": {},
        },
        "report": {
            "page_title": null,
            "total_requests": 0,
            "blocked_requests": 0,
            "status_histogram": {},
            "resource_type_histogram": {},
            "provider_histogram": {},
            "marker_histogram": {},
            "top_markers": [],
            "hosts": [],
            "suspicious_requests": [],
            "aggregate": {"provider": "Unknown", "confidence": 0.0, "markers": []},
            "target_class": null,
        },
        "requirements": {
            "provider": "Unknown",
            "confidence": 0.0,
            "requirements": [],
            "recommendation": {
                "strategy": "InvestigateOnly",
                "rationale": "test",
                "required_stygian_features": [],
                "config_hints": {},
            }
        },
        "policy": {
            "execution_mode": "http",
            "session_mode": "stateless",
            "telemetry_level": "standard",
            "rate_limit_rps": 1.0,
            "max_retries": 2,
            "backoff_base_ms": 250,
            "enable_warmup": false,
            "enforce_webrtc_proxy_only": false,
            "sticky_session_ttl_secs": null,
            "required_stygian_features": [],
            "config_hints": {},
            "risk_score": 0.0,
        },
        "probe_report": {"total": 0, "passed": 0, "failed": 0, "results": [], "all_passed": true},
        "coherence_violations": []
    })
    .to_string();
    let bundle: stygian_charon::bundle::DiagnosticBundle =
        serde_json::from_str(&json).expect("deserialize pre-T89 bundle");
    assert!(bundle.vendor_classification.is_none());
}

// =====================================================================
// #[ignore] — full wire-level contract test
// =====================================================================

/// T89 acceptance criterion: the vendor classification must
/// appear in the wire form of the diagnostic bundle. This test
/// is `#[ignore]`-gated so it only runs in CI / when an
/// operator opts in via `--ignored`. It complements the
/// non-ignored `diagnostic_bundle_carries_vendor_classification_for_datadome_input`
/// test by serialising the bundle and asserting on the
/// **JSON shape** rather than the struct shape.
///
/// Run with:
///
/// ```sh
/// cargo test -p stygian-charon --test vendor_classifier_integration \
///     vendor_classification_appears_in_diagnostic_payload -- --ignored --nocapture
/// ```
#[test]
#[ignore = "verifies T89 acceptance criterion: classifier output appears in diagnostics payload"]
fn vendor_classification_appears_in_diagnostic_payload() {
    let entry = make_entry(
        "https://www.example.com/cdn-cgi/challenge-platform",
        403,
        &headers_kv(&[("cf-ray", "abc-ORD"), ("server", "cloudflare")]),
        &[("__cf_bm", "xyz")],
        "Attention required! | cloudflare",
    );
    let har = build_har(&[entry]);
    let bundle = build_diagnostic_bundle(&har, BundleRedactionPolicy::None)
        .expect("bundle");
    assert!(
        bundle.vendor_classification.is_some(),
        "vendor classification must be populated for a Cloudflare-flavoured HAR"
    );
    let classification = bundle
        .vendor_classification
        .as_ref()
        .expect("classification present");
    assert_eq!(classification.top_vendor, VendorId::Cloudflare);
    assert!(classification.is_high_confidence);
    assert!(classification.confidence > 0.0);

    // Serialise to JSON and assert the field shows up in the wire
    // form. The `vendor_classification` field is on the
    // `DiagnosticBundle` and is NOT marked
    // `skip_serializing_if = "Option::is_none"` for this assertion
    // — the contract is "the field is present in the payload",
    // not "the field is sometimes present".
    let json: serde_json::Value =
        serde_json::to_value(&bundle).expect("serialize bundle");
    assert!(
        json.get("vendor_classification").is_some(),
        "diagnostic payload JSON must contain a `vendor_classification` field, got: {json}"
    );
    let vendor_json = &json["vendor_classification"];
    assert_eq!(
        vendor_json["top_vendor"], "cloudflare",
        "top_vendor must be the snake_case VendorId label"
    );
    assert!(
        vendor_json["confidence"].as_f64().is_some_and(|c| c > 0.0),
        "confidence must be a positive number, got: {vendor_json:?}"
    );
    assert!(
        vendor_json["is_high_confidence"].as_bool() == Some(true),
        "is_high_confidence must be true for a Cloudflare challenge"
    );
    assert!(
        vendor_json["ranked"].as_array().is_some_and(|a| !a.is_empty()),
        "ranked scoreboard must not be empty"
    );
    assert!(
        vendor_json["evidence"]["items"].as_array().is_some_and(|a| !a.is_empty()),
        "evidence.items must contain at least one match"
    );
    assert!(
        vendor_json["evidence"]["source_summary"]
            .as_object()
            .is_some_and(|o| !o.is_empty()),
        "evidence.source_summary must be populated"
    );
}
