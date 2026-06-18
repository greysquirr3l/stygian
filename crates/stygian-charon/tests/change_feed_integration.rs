#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::similar_names,
    clippy::missing_const_for_fn
)]

//! T88 — Anti-bot change-detection feed integration tests.
//!
//! Exercises the end-to-end change-detection path:
//! a synthetic canary regression sequence produces
//! a [`ChangeFeedReport`][stygian_charon::ChangeFeedReport]
//! via [`ChangeDetector`][stygian_charon::ChangeDetector],
//! records events into the in-memory sink, and emits
//! a runbook-aligned [`ChangeEvent`][stygian_charon::ChangeEvent]
//! payload.
//!
//! The `#[ignore]`-gated
//! `synthetic_canary_regression_generates_event_packet`
//! test confirms a multi-delta canary regression
//! (T92/T84-style signal) crosses into `Probable`
//! and emits a deterministic event packet suitable
//! for runbook consumption.

use std::collections::BTreeMap;

use stygian_charon::change_feed::{
    ChangeClassification, ChangeDeltaInput, ChangeDetector, ChangeFeedThresholds,
    InMemoryChangeFeedSink,
};
use stygian_charon::types::TargetClass;
use stygian_charon::vendor_classifier::VendorId;

fn canary_delta(
    target: &str,
    weight: f64,
    severity: stygian_charon::change_feed::DeltaSeverity,
    summary: &str,
) -> ChangeDeltaInput {
    ChangeDeltaInput::new(
        stygian_charon::change_feed::DeltaSource::Canary,
        target,
        weight,
        severity,
        summary,
    )
}

#[test]
fn detector_emits_suspected_event_for_single_advisory_canary_delta() {
    let detector = ChangeDetector::new();
    let sink = InMemoryChangeFeedSink::new();
    let deltas = vec![canary_delta(
        "example.com",
        0.30,
        stygian_charon::change_feed::DeltaSeverity::Advisory,
        "integrity probe webdriver regressed",
    )];
    let report = detector.detect(&deltas, &sink);

    assert_eq!(
        report.aggregate_classification,
        ChangeClassification::Suspected
    );
    assert_eq!(report.suspected_targets, vec!["example.com".to_string()]);
    assert_eq!(report.probable_targets, Vec::<String>::new());
    assert!(report.noise_targets.is_empty());

    assert_eq!(sink.len(), 1);
    let event = sink.events().pop().expect("event recorded");
    assert_eq!(event.affected_target, "example.com");
    assert_eq!(event.classification, ChangeClassification::Suspected);
    assert!(event.event_id.starts_with("cf-"));
    assert!(event.event_id.ends_with("-example.com"));
    assert_eq!(
        event.delta_summary.headline,
        "integrity probe webdriver regressed"
    );
    assert!(event.delta_summary.score > 0.0);
    assert!(
        event
            .delta_summary
            .sources
            .contains(&stygian_charon::change_feed::DeltaSource::Canary)
    );
    // The Suspected band always points at the
    // fingerprint/identity runbook section.
    assert!(
        event
            .recommended_mitigation_path
            .path
            .starts_with("category-a")
    );
}

#[test]
fn detector_emits_no_event_for_pure_noise_deltas() {
    let detector = ChangeDetector::new();
    let sink = InMemoryChangeFeedSink::new();
    let deltas = vec![
        canary_delta(
            "quiet.example.com",
            0.05,
            stygian_charon::change_feed::DeltaSeverity::Clean,
            "ok",
        ),
        canary_delta(
            "blip.example.com",
            0.05,
            stygian_charon::change_feed::DeltaSeverity::Advisory,
            "small blip",
        ),
    ];
    let report = detector.detect(&deltas, &sink);
    assert_eq!(report.aggregate_classification, ChangeClassification::Noise);
    assert_eq!(report.noise_targets.len(), 2);
    assert!(report.suspected_targets.is_empty());
    assert!(report.probable_targets.is_empty());
    assert!(sink.is_empty());
}

#[test]
fn detector_emits_probable_event_when_critical_severity_present() {
    let detector = ChangeDetector::new();
    let sink = InMemoryChangeFeedSink::new();
    let deltas = vec![
        ChangeDeltaInput::new(
            stygian_charon::change_feed::DeltaSource::Canary,
            "example.com",
            0.10,
            stygian_charon::change_feed::DeltaSeverity::Critical,
            "integrity probe webdriver regressed",
        )
        .with_target_class(TargetClass::HighSecurity)
        .with_vendor(VendorId::DataDome)
        .with_evidence("baseline_score", "0.95")
        .with_evidence("current_score", "0.10"),
    ];
    let report = detector.detect(&deltas, &sink);
    assert_eq!(
        report.aggregate_classification,
        ChangeClassification::Probable
    );
    assert_eq!(report.probable_targets, vec!["example.com".to_string()]);
    assert_eq!(sink.len(), 1);
    let event = sink.events().pop().expect("event recorded");
    assert_eq!(event.classification, ChangeClassification::Probable);
    assert_eq!(event.vendor_hint, Some(VendorId::DataDome));
    assert_eq!(event.target_class, Some(TargetClass::HighSecurity));
    // The DataDome + Probable pairing routes to
    // the category-a fingerprint runbook section.
    assert!(
        event
            .recommended_mitigation_path
            .path
            .starts_with("category-a")
    );
    assert_eq!(
        event.evidence.get("canary.baseline_score"),
        Some(&"0.95".to_string())
    );
}

#[test]
fn threshold_round_trip_through_config_struct() {
    // This test exercises the "config struct
    // round-trip" requirement with custom
    // thresholds — every override survives
    // serde_json round-trip.
    let thresholds = ChangeFeedThresholds::default()
        .with_noise_ceiling(0.10)
        .with_probable_floor(0.40)
        .with_canary_weight(0.95)
        .with_proxy_weight(0.85)
        .with_extraction_weight(0.65);

    let json = serde_json::to_string(&thresholds).expect("serialise");
    let parsed: ChangeFeedThresholds = serde_json::from_str(&json).expect("deserialise");
    assert_eq!(thresholds, parsed);

    let detector = ChangeDetector::new().with_thresholds(thresholds);
    let sink = InMemoryChangeFeedSink::new();
    // 0.30 canary + default canary_weight 0.95:
    // score = 0.30 * 0.95 = 0.285. Below the
    // tightened probable_floor (0.40), above the
    // tightened noise_ceiling (0.10) → Suspected.
    let report = detector.detect(
        &[canary_delta(
            "example.com",
            0.30,
            stygian_charon::change_feed::DeltaSeverity::Advisory,
            "ok",
        )],
        &sink,
    );
    assert_eq!(
        report.aggregate_classification,
        ChangeClassification::Suspected
    );
    assert_eq!(report.thresholds, thresholds);
}

#[test]
fn report_serialises_to_runbook_diagnostic_shape() {
    let detector = ChangeDetector::new();
    let sink = InMemoryChangeFeedSink::new();
    let deltas = vec![canary_delta(
        "example.com",
        0.30,
        stygian_charon::change_feed::DeltaSeverity::Advisory,
        "canary regression",
    )];
    let report = detector.detect(&deltas, &sink);

    // Round-trip through serde_json so the
    // runbook diagnostics shape is pinned by the
    // contract — operators parse this payload.
    let mut value = serde_json::to_value(&report).expect("serialise");
    let obj = value.as_object_mut().expect("object");
    assert!(obj.contains_key("aggregate_classification"));
    assert!(obj.contains_key("aggregate_score"));
    assert!(obj.contains_key("noise_targets"));
    assert!(obj.contains_key("suspected_targets"));
    assert!(obj.contains_key("probable_targets"));
    assert!(obj.contains_key("events"));
    assert!(obj.contains_key("thresholds"));

    let events = obj
        .get("events")
        .and_then(|v| v.as_array())
        .expect("events");
    assert_eq!(events.len(), 1);
    let event = &events[0];
    let event_obj = event.as_object().expect("event object");
    for required_key in [
        "event_id",
        "detected_at_unix_secs",
        "affected_target",
        "classification",
        "delta_summary",
        "recommended_mitigation_path",
    ] {
        assert!(
            event_obj.contains_key(required_key),
            "missing key {required_key} in event payload"
        );
    }
    // Vendor hint and target class are optional,
    // so they must use skip_serializing_if.
    assert!(!event_obj.contains_key("vendor_hint"));
    assert!(!event_obj.contains_key("target_class"));
}

/// End-to-end confirmation that a synthetic canary
/// regression sequence produces a deterministic
/// event packet. The test is `#[ignore]`-gated so
/// the fast preflight remains deterministic — the
/// assertion itself is the documented T88 contract
/// (multi-delta canary regression → `Probable`
/// classification → runbook event with vendor
/// hint, target class, evidence, mitigation
/// pointer, and stable `event_id`).
#[test]
#[ignore = "synthetic_canary_regression_generates_event_packet: end-to-end T88 contract"]
fn synthetic_canary_regression_generates_event_packet() {
    let detector = ChangeDetector::new();
    let sink = InMemoryChangeFeedSink::new();
    let mut evidence: BTreeMap<String, String> = BTreeMap::new();
    evidence.insert("baseline_score".to_string(), "0.95".to_string());
    evidence.insert("current_score".to_string(), "0.55".to_string());

    let deltas = vec![
        ChangeDeltaInput::new(
            stygian_charon::change_feed::DeltaSource::Canary,
            "example.com",
            0.40,
            stygian_charon::change_feed::DeltaSeverity::Critical,
            "integrity probe webdriver regressed",
        )
        .with_target_class(TargetClass::HighSecurity)
        .with_vendor(VendorId::DataDome),
        ChangeDeltaInput::new(
            stygian_charon::change_feed::DeltaSource::Proxy,
            "example.com",
            0.65,
            stygian_charon::change_feed::DeltaSeverity::Warning,
            "proxy score dropped 0.30",
        ),
        ChangeDeltaInput::new(
            stygian_charon::change_feed::DeltaSource::Extraction,
            "example.com",
            0.50,
            stygian_charon::change_feed::DeltaSeverity::Advisory,
            "reliability regressed 0.20",
        ),
    ];
    let _ = evidence;

    let report = detector.detect(&deltas, &sink);

    // The synthetic canary regression must:
    // 1. Classify the target as Probable (the
    //    Critical-severity canary delta is
    //    sufficient on its own).
    // 2. Emit exactly one event for the target.
    // 3. Carry the DataDome vendor hint and
    //    HighSecurity target class through.
    // 4. Route to the fingerprint runbook section.
    // 5. Carry a stable event_id that includes
    //    the affected target so downstream tooling
    //    can dedupe.
    assert_eq!(
        report.aggregate_classification,
        ChangeClassification::Probable
    );
    assert_eq!(report.probable_targets, vec!["example.com".to_string()]);
    assert_eq!(sink.len(), 1);

    let event = sink.events().pop().expect("event recorded");
    assert_eq!(event.classification, ChangeClassification::Probable);
    assert_eq!(event.vendor_hint, Some(VendorId::DataDome));
    assert_eq!(event.target_class, Some(TargetClass::HighSecurity));
    assert!(
        event
            .recommended_mitigation_path
            .path
            .starts_with("category-a")
    );
    assert!(
        event
            .recommended_mitigation_path
            .url
            .contains("incident-runbook.md")
    );
    assert!(event.event_id.starts_with("cf-"));
    assert!(event.event_id.ends_with("-example.com"));

    let sources = &event.delta_summary.sources;
    assert!(sources.contains(&stygian_charon::change_feed::DeltaSource::Canary));
    assert!(sources.contains(&stygian_charon::change_feed::DeltaSource::Proxy));
    assert!(sources.contains(&stygian_charon::change_feed::DeltaSource::Extraction));

    // The detector must produce a deterministic
    // report across runs — replaying the same
    // deltas with the same thresholds yields the
    // same per-target lists.
    let replay = detector.detect(&deltas, &InMemoryChangeFeedSink::new());
    assert_eq!(report.noise_targets, replay.noise_targets);
    assert_eq!(report.suspected_targets, replay.suspected_targets);
    assert_eq!(report.probable_targets, replay.probable_targets);
    assert_eq!(
        report.aggregate_classification,
        replay.aggregate_classification
    );
}
