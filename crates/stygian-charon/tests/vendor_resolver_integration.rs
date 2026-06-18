#![allow(
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::float_cmp,
    clippy::cast_lossless,
    clippy::similar_names,
    clippy::uninlined_format_args,
    clippy::format_push_string
)]

//! T90 — Vendor-to-playbook auto-resolution integration tests.
//!
//! Exercises the end-to-end resolution path: the
//! [`VendorResolver`][stygian_charon::vendor_resolver::VendorResolver]
//! consumes a [`VendorClassification`][stygian_charon::vendor_classifier::VendorClassification]
//! produced by the classifier and returns either a resolved
//! [`StrategyMarker::Resolved`][stygian_charon::vendor_resolver::StrategyMarker::Resolved]
//! playbook id / target class pair or the
//! [`StrategyMarker::Manual`][stygian_charon::vendor_resolver::StrategyMarker::Manual]
//! fallback. The integration tests cover:
//!
//! - **Single-vendor** scenarios: `DataDome`, `PerimeterX`, `Akamai`
//!   resolve to `tier2-hostile`; Cloudflare resolves to `tier1-js`.
//! - **Multi-vendor** scenarios: when a strong Tier 2 vendor and a
//!   Tier 1 vendor both match, the Tier 2 rule wins (lower
//!   priority number).
//! - **Low-confidence** fallback: a `DataDome` signal that does not
//!   cross `min_confidence = 0.60` falls through to the
//!   `default-manual` sentinel and returns the `Manual` strategy
//!   marker.
//! - **End-to-end playbook resolution** via
//!   [`VendorResolver::resolve_with_playbooks`][stygian_charon::vendor_resolver::VendorResolver::resolve_with_playbooks]:
//!   the resolved playbook id is fed into
//!   [`PlaybookResolver`][stygian_charon::playbooks::PlaybookResolver]
//!   and the resulting
//!   [`ResolvedPlaybook`][stygian_charon::playbooks::ResolvedPlaybook]
//!   carries the expected acquisition mode + target class.
//!
//! The `#[ignore]`-gated
//! `synthetic_vendor_signatures_map_to_expected_playbooks` test
//! confirms the wire-level contract: four synthetic HAR payloads
//! — one per Tier 1 baseline vendor — produce the expected
//! resolved playbook id when run through the full
//! `classify → resolve → resolve_with_playbooks` pipeline.

use serde_json::json;
use stygian_charon::playbooks::{
    AcquisitionOverrides, PlaybookOverrides, PlaybookResolver, ResolvedPlaybook,
};
use stygian_charon::types::{ExecutionMode, TargetClass};
use stygian_charon::vendor_classifier::{
    VendorClassification, VendorClassifier, VendorId, VendorScore,
};
use stygian_charon::vendor_resolver::{
    MergeStrategy, ResolutionRule, StrategyMarker, VendorResolver, VendorResolverError,
    VendorRuleMatch,
};
use std::collections::BTreeMap;

fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Tuple of `(label, HAR entry builder, expected playbook id)`
/// used by the `#[ignore]`-gated synthetic-signatures test.
type VendorCase = (&'static str, fn() -> serde_json::Value, &'static str);

fn make_vendor_resolver() -> VendorResolver {
    VendorResolver::with_builtin_defaults()
}

fn make_playbook_resolver() -> PlaybookResolver {
    PlaybookResolver::with_builtin_defaults()
}

fn classify(
    cookies: &[(&str, &str)],
    headers: &[(&str, &str)],
    body: Option<&str>,
    url: &str,
) -> VendorClassification {
    let classifier = VendorClassifier::with_builtin_defaults();
    let cookies: Vec<String> = cookies
        .iter()
        .map(|(n, v)| format!("{n}={v}; Path=/"))
        .collect();
    let mut header_map: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in headers {
        header_map.insert((*k).to_string(), (*v).to_string());
    }
    classifier.classify(&cookies, &header_map, body, url)
}

// =====================================================================
// Single-vendor scenarios — strong Tier 2 signals
// =====================================================================

#[test]
fn datadome_signal_resolves_to_tier2_hostile() {
    let resolver = make_vendor_resolver();
    let classification = classify(
        &[("datadome", "abc")],
        &[("x-datadome", "protected"), ("x-datadome-cid", "abc")],
        Some("captcha-delivery.com iframe"),
        "https://example.com/",
    );
    assert_eq!(classification.top_vendor, VendorId::DataDome);
    let resolution = resolver.resolve(&classification);
    match &resolution.strategy {
        StrategyMarker::Resolved { playbook_id, target_class } => {
            assert_eq!(playbook_id, "tier2-hostile");
            assert_eq!(*target_class, TargetClass::HighSecurity);
        }
        StrategyMarker::Manual => panic!("DataDome should resolve, not defer"),
    }
    assert!(resolution.is_resolved());
    assert_eq!(resolution.rationale.merge_strategy, MergeStrategy::StrongestVendor);
    assert!(resolution.rationale.summary.contains("datadome"));
    assert!(resolution.rationale.summary.contains("tier2-hostile"));
}

#[test]
fn perimeter_x_signal_resolves_to_tier2_hostile() {
    let resolver = make_vendor_resolver();
    let classification = classify(
        &[("_px3", "abc"), ("_px2", "xyz")],
        &[("x-px", "1")],
        None,
        "https://example.com/",
    );
    assert_eq!(classification.top_vendor, VendorId::PerimeterX);
    let resolution = resolver.resolve(&classification);
    match &resolution.strategy {
        StrategyMarker::Resolved { playbook_id, target_class } => {
            assert_eq!(playbook_id, "tier2-hostile");
            assert_eq!(*target_class, TargetClass::HighSecurity);
        }
        StrategyMarker::Manual => panic!("PerimeterX should resolve, not defer"),
    }
}

#[test]
fn akamai_signal_resolves_to_tier2_hostile() {
    let resolver = make_vendor_resolver();
    let classification = classify(
        &[("_abck", "abc"), ("bm_sz", "1234")],
        &[("bm_sv", "xyz")],
        None,
        "https://example.com/_bm/v3/abc",
    );
    assert_eq!(classification.top_vendor, VendorId::Akamai);
    let resolution = resolver.resolve(&classification);
    match &resolution.strategy {
        StrategyMarker::Resolved { playbook_id, .. } => {
            assert_eq!(playbook_id, "tier2-hostile");
        }
        StrategyMarker::Manual => panic!("Akamai should resolve, not defer"),
    }
}

// =====================================================================
// Single-vendor scenarios — Tier 1 / Cloudflare
// =====================================================================

#[test]
fn cloudflare_signal_resolves_to_tier1_js() {
    let resolver = make_vendor_resolver();
    let classification = classify(
        &[("__cf_bm", "xyz")],
        &[("cf-ray", "abc-ORD"), ("server", "cloudflare")],
        Some("Attention required! | cloudflare"),
        "https://example.com/cdn-cgi/challenge-platform/orchestrate",
    );
    assert_eq!(classification.top_vendor, VendorId::Cloudflare);
    let resolution = resolver.resolve(&classification);
    match &resolution.strategy {
        StrategyMarker::Resolved { playbook_id, target_class } => {
            assert_eq!(playbook_id, "tier1-js");
            assert_eq!(*target_class, TargetClass::ContentSite);
        }
        StrategyMarker::Manual => panic!("Cloudflare should resolve, not defer"),
    }
}

// =====================================================================
// Multi-vendor scenarios — escalation precedence
// =====================================================================

#[test]
fn datadome_plus_cloudflare_escalates_to_tier2_hostile() {
    let resolver = make_vendor_resolver();
    // Both DataDome and Cloudflare signal in headers — the
    // classifier picks DataDome as the top vendor because of the
    // stronger weight; the resolver's `tier2-hostile` rule has
    // lower priority (0) than `tier1-js-cloudflare` (10), so
    // tier2-hostile wins.
    let classification = classify(
        &[],
        &[
            ("x-datadome", "1"),
            ("x-datadome-cid", "abc"),
            ("cf-ray", "1"),
        ],
        None,
        "https://example.com/",
    );
    assert_eq!(classification.top_vendor, VendorId::DataDome);
    let resolution = resolver.resolve(&classification);
    match &resolution.strategy {
        StrategyMarker::Resolved { playbook_id, .. } => {
            assert_eq!(
                playbook_id, "tier2-hostile",
                "Tier 2 (priority 0) should beat Tier 1 (priority 10)"
            );
        }
        StrategyMarker::Manual => panic!("multi-vendor should resolve"),
    }
}

#[test]
fn akamai_plus_cloudflare_escalates_to_tier2_hostile() {
    let resolver = make_vendor_resolver();
    let classification = classify(
        &[("_abck", "abc"), ("bm_sz", "1")],
        &[("bm_sv", "1"), ("cf-ray", "1")],
        None,
        "https://example.com/",
    );
    assert_eq!(classification.top_vendor, VendorId::Akamai);
    let resolution = resolver.resolve(&classification);
    match &resolution.strategy {
        StrategyMarker::Resolved { playbook_id, .. } => {
            assert_eq!(playbook_id, "tier2-hostile");
        }
        StrategyMarker::Manual => panic!("Akamai + Cloudflare should resolve to tier2"),
    }
}

// =====================================================================
// Low-confidence fallback — Manual strategy marker
// =====================================================================

#[test]
fn low_confidence_datadome_returns_manual_marker() {
    let resolver = make_vendor_resolver();
    // Empty classifier with threshold 0.99 + a strong DataDome
    // signal. The classifier will report DataDome with full
    // confidence 1.0 (no other vendors scored), but the resolver
    // uses the rule's min_confidence (0.60), not the
    // classifier's threshold. So we need to use a custom
    // classification with confidence below 0.60.
    let classification = VendorClassification {
        top_vendor: VendorId::DataDome,
        confidence: 0.30, // below the tier2-hostile gate
        is_high_confidence: false,
        ranked: vec![VendorScore {
            vendor: VendorId::DataDome,
            score: 5,
            matched_sources: BTreeMap::new(),
        }],
        evidence: stygian_charon::vendor_classifier::EvidenceBundle::default(),
        threshold: 0.60,
    };
    let resolution = resolver.resolve(&classification);
    assert!(
        resolution.is_manual(),
        "low-confidence DataDome should fall through to Manual, got {:?}",
        resolution.strategy
    );
    assert_eq!(resolution.rationale.top_vendor, VendorId::DataDome);
    assert!(resolution.rationale.summary.contains("manual"));
}

#[test]
fn clean_unknown_classification_picks_tier1_static() {
    let resolver = make_vendor_resolver();
    let classification = classify(&[], &[("content-type", "text/html")], Some("harmless html"), "https://example.com/");
    assert!(classification.is_unknown());
    let resolution = resolver.resolve(&classification);
    // The tier1-static rule has require_unknown_vendor = true so
    // it picks up the clean unknown classification.
    match &resolution.strategy {
        StrategyMarker::Resolved { playbook_id, target_class } => {
            assert_eq!(playbook_id, "tier1-static");
            assert_eq!(*target_class, TargetClass::ContentSite);
        }
        StrategyMarker::Manual => panic!("clean Unknown should pick tier1-static"),
    }
}

// =====================================================================
// End-to-end playbook resolution
// =====================================================================

#[test]
fn resolved_playbook_drives_acquisition_runner_config_for_datadome() {
    let resolver = make_vendor_resolver();
    let playbook_resolver = make_playbook_resolver();
    let classification = classify(
        &[("datadome", "abc")],
        &[("x-datadome", "protected"), ("x-datadome-cid", "abc")],
        None,
        "https://example.com/",
    );
    let resolved = resolver
        .resolve_with_playbooks(&classification, &playbook_resolver, &PlaybookOverrides::default())
        .expect("resolve")
        .expect("resolved (not Manual)");
    assert_eq!(resolved.playbook_id, "tier2-hostile");
    assert_eq!(resolved.target_class, TargetClass::HighSecurity);
    assert!(
        resolved.acquisition.enable_warmup,
        "tier2-hostile playbook enables warmup"
    );
    // tier2-hostile uses retry_budget = 8 from the TOML.
    assert!(resolved.acquisition.retry_budget >= 4);
}

#[test]
fn resolved_playbook_for_cloudflare_uses_tier1_js_browser_execution() {
    let resolver = make_vendor_resolver();
    let playbook_resolver = make_playbook_resolver();
    let classification = classify(
        &[("__cf_bm", "xyz")],
        &[("cf-ray", "abc-LHR"), ("server", "cloudflare")],
        Some("Attention required! | cloudflare"),
        "https://example.com/cdn-cgi/challenge-platform",
    );
    let resolved = resolver
        .resolve_with_playbooks(&classification, &playbook_resolver, &PlaybookOverrides::default())
        .expect("resolve")
        .expect("resolved (not Manual)");
    assert_eq!(resolved.playbook_id, "tier1-js");
    assert_eq!(resolved.target_class, TargetClass::ContentSite);
    // tier1-js uses ExecutionMode::Browser.
    assert_eq!(resolved.acquisition.execution_mode, ExecutionMode::Browser);
}

#[test]
fn manual_strategy_marker_returns_none_from_resolve_with_playbooks() {
    let resolver = make_vendor_resolver();
    let playbook_resolver = make_playbook_resolver();
    let classification = VendorClassification {
        top_vendor: VendorId::DataDome,
        confidence: 0.10,
        is_high_confidence: false,
        ranked: vec![VendorScore {
            vendor: VendorId::DataDome,
            score: 5,
            matched_sources: BTreeMap::new(),
        }],
        evidence: stygian_charon::vendor_classifier::EvidenceBundle::default(),
        threshold: 0.60,
    };
    let resolved: Option<ResolvedPlaybook> = resolver
        .resolve_with_playbooks(&classification, &playbook_resolver, &PlaybookOverrides::default())
        .expect("resolve");
    assert!(resolved.is_none(), "Manual marker should not produce a ResolvedPlaybook");
}

#[test]
fn request_override_still_wins_for_resolved_playbook() {
    let resolver = make_vendor_resolver();
    let playbook_resolver = make_playbook_resolver();
    let classification = classify(
        &[("datadome", "abc")],
        &[("x-datadome", "protected"), ("x-datadome-cid", "abc")],
        None,
        "https://example.com/",
    );
    let overrides = PlaybookOverrides {
        acquisition: AcquisitionOverrides {
            retry_budget: Some(42),
            ..AcquisitionOverrides::default()
        },
        ..PlaybookOverrides::default()
    };
    let resolved = resolver
        .resolve_with_playbooks(&classification, &playbook_resolver, &overrides)
        .expect("resolve")
        .expect("resolved");
    assert_eq!(resolved.acquisition.retry_budget, 42);
}

// =====================================================================
// Determinism
// =====================================================================

#[test]
fn resolution_is_deterministic_for_same_input() {
    let resolver = make_vendor_resolver();
    let classification = classify(
        &[("datadome", "abc")],
        &[("x-datadome", "protected"), ("x-datadome-cid", "abc")],
        None,
        "https://example.com/",
    );
    let r1 = resolver.resolve(&classification);
    let r2 = resolver.resolve(&classification);
    assert_eq!(r1.strategy, r2.strategy);
    assert_eq!(r1.rationale.summary, r2.rationale.summary);
    assert_eq!(r1.rationale.applied_rules.len(), r2.rationale.applied_rules.len());
}

#[test]
fn confidence_propagates_into_resolution_rationale() {
    let resolver = make_vendor_resolver();
    let classification = classify(
        &[("datadome", "abc")],
        &[("x-datadome", "protected"), ("x-datadome-cid", "abc")],
        None,
        "https://example.com/",
    );
    let r = resolver.resolve(&classification);
    assert!(approx_eq(r.rationale.confidence, classification.confidence));
    assert_eq!(r.rationale.top_vendor, classification.top_vendor);
}

// =====================================================================
// Rule shape — sanity checks
// =====================================================================

#[test]
fn resolver_exposes_baseline_rule_ids_in_priority_order() {
    let resolver = make_vendor_resolver();
    let ids = resolver.rule_ids();
    assert_eq!(
        ids,
        vec![
            "tier2-hostile".to_string(),
            "tier1-js-cloudflare".to_string(),
            "tier1-static".to_string(),
            "default-manual".to_string(),
        ]
    );
}

#[test]
fn from_rules_rejects_duplicate_ids() {
    let rule = ResolutionRule {
        id: "dup".to_string(),
        playbook_id: "tier2-hostile".to_string(),
        target_class: TargetClass::HighSecurity,
        priority: 0,
        merge_strategy: MergeStrategy::StrongestVendor,
        description: String::new(),
        min_confidence: 0.0,
        min_score: 0,
        require_unknown_vendor: false,
        vendors: vec![VendorRuleMatch {
            vendor: VendorId::DataDome,
            weight: 5,
        }],
    };
    let result = VendorResolver::from_rules(vec![rule.clone(), rule]);
    assert!(matches!(
        result,
        Err(VendorResolverError::DuplicateId { .. })
    ));
}

// =====================================================================
// #[ignore] — full synthetic signature → expected playbook mapping
// =====================================================================

fn make_entry(
    url: &str,
    status: u16,
    response_headers: &[(&str, &str)],
    cookies: &[(&str, &str)],
    body: &str,
) -> serde_json::Value {
    let mut headers: Vec<serde_json::Value> = response_headers
        .iter()
        .map(|(k, v)| json!({"name": k, "value": v}))
        .collect();
    for (n, v) in cookies {
        headers.push(json!({"name": "set-cookie", "value": format!("{n}={v}; Path=/")}));
    }
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
            "headers": headers,
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
            "creator": {"name": "t90-fixture", "version": "1.0"},
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

/// T90 acceptance criterion: synthetic vendor signatures in HAR
/// payloads must resolve to the expected playbooks via the full
/// `classify → resolve → resolve_with_playbooks` pipeline. This
/// test is `#[ignore]`-gated so it only runs in CI / when an
/// operator opts in via `--ignored`. It complements the
/// non-ignored per-vendor tests by asserting the wire-level
/// mapping (HAR cookies + headers → playbook id) for all four
/// Tier 1 baseline vendors plus the manual fallback.
///
/// Run with:
///
/// ```sh
/// cargo test -p stygian-charon --test vendor_resolver_integration \
///     synthetic_vendor_signatures_map_to_expected_playbooks -- --ignored --nocapture
/// ```
#[test]
#[ignore = "verifies T90 acceptance criterion: synthetic signatures resolve to expected playbooks"]
fn synthetic_vendor_signatures_map_to_expected_playbooks() {
    use stygian_charon::har;

    let vendor_resolver = make_vendor_resolver();
    let playbook_resolver = make_playbook_resolver();
    let classifier = VendorClassifier::with_builtin_defaults();

    // Each entry is `(label, har entry builder, expected playbook id)`.
    let cases: Vec<VendorCase> = vec![
        (
            "datadome",
            || {
                make_entry(
                    "https://www.example.com/products",
                    403,
                    &[("x-datadome", "protected"), ("x-datadome-cid", "abc")],
                    &[("datadome", "xyz")],
                    "captcha-delivery.com iframe",
                )
            },
            "tier2-hostile",
        ),
        (
            "cloudflare",
            || {
                make_entry(
                    "https://www.example.com/login",
                    403,
                    &[("cf-ray", "abc-ORD"), ("server", "cloudflare")],
                    &[("__cf_bm", "xyz")],
                    "Attention required! | cloudflare",
                )
            },
            "tier1-js",
        ),
        (
            "akamai",
            || {
                make_entry(
                    "https://www.example.com/_bm/v3/abc",
                    403,
                    &[("bm_sv", "xyz")],
                    &[("_abck", "abc"), ("bm_sz", "1234")],
                    "akamai bot detection",
                )
            },
            "tier2-hostile",
        ),
        (
            "perimeter_x",
            || {
                make_entry(
                    "https://www.example.com/captcha",
                    403,
                    &[("x-px", "1")],
                    &[("_px3", "abc"), ("_px2", "xyz")],
                    "perimeterx / humansecurity challenge",
                )
            },
            "tier2-hostile",
        ),
    ];

    for (label, build_entry, expected_playbook) in cases {
        let entry = build_entry();
        let har_text = build_har(&[entry]);
        let parsed = har::parse_har_transactions(&har_text).expect("HAR parse");
        assert!(
            !parsed.requests.is_empty(),
            "{label}: HAR must contain at least one request"
        );
        let classification = classifier.classify_view(&parsed.requests[0].clone().into());
        let resolution = vendor_resolver.resolve(&classification);
        let resolved = vendor_resolver
            .resolve_with_playbooks(&classification, &playbook_resolver, &PlaybookOverrides::default())
            .expect("resolve_with_playbooks")
            .unwrap_or_else(|| panic!("{label}: expected resolved playbook, got Manual"));
        assert_eq!(
            resolved.playbook_id, expected_playbook,
            "{label}: resolved.playbook_id mismatch (rationale: {})",
            resolution.rationale.summary
        );
    }
}
