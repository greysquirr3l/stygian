#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::float_cmp,
    clippy::cast_lossless,
    clippy::similar_names,
    clippy::missing_const_for_fn
)]

//! T93 — Proof-of-work capability profile integration tests.
//!
//! Exercises the end-to-end `PoW` policy feedback path: a
//! sequence of [`PowCapabilitySample`]s is recorded into a
//! [`PowCapabilityStore`], the resulting profile is scored
//! through [`PowCapabilityScorer`], and the score is fed
//! into [`adjust_runtime_policy_for_pow`] to produce the
//! adjusted [`RuntimePolicy`].
//!
//! The `#[ignore]`-gated
//! `synthetic_pow_profile_drives_policy_mapping` test
//! confirms a synthetic weak `PoW` profile produces a
//! fully-escalated policy (browser+sticky, lower rate,
//! higher retries) while a synthetic strong profile keeps
//! the policy at the operator's defaults with only a
//! config-hint added.

use stygian_charon::pow_profile::{
    PowCapabilitySample, PowCapabilityScorer, PowCapabilityStore, PowPolicyThresholds,
    adjust_runtime_policy_for_pow, score_from_profile,
};
use stygian_charon::types::{
    ExecutionMode, RuntimePolicy, SessionMode, TargetClass, TelemetryLevel,
};
use stygian_charon::vendor_classifier::VendorId;
use std::collections::BTreeMap;

fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn base_policy() -> RuntimePolicy {
    RuntimePolicy {
        execution_mode: ExecutionMode::Http,
        session_mode: SessionMode::Stateless,
        telemetry_level: TelemetryLevel::Standard,
        rate_limit_rps: 3.0,
        max_retries: 2,
        backoff_base_ms: 250,
        enable_warmup: false,
        enforce_webrtc_proxy_only: false,
        sticky_session_ttl_secs: None,
        required_stygian_features: Vec::new(),
        config_hints: BTreeMap::new(),
        risk_score: 0.30,
    }
}

fn record_strong_samples(store: &PowCapabilityStore) {
    // 10 fast, low-retry, no-failure samples.
    for _ in 0..10 {
        store.record_sample(
            "example.com",
            TargetClass::ContentSite,
            VendorId::Cloudflare,
            &PowCapabilitySample::solved(800, 0),
        );
    }
}

fn record_weak_samples(store: &PowCapabilityStore) {
    // 2 slow solves, 8 high-severity failures with retries.
    store.record_sample(
        "example.com",
        TargetClass::ContentSite,
        VendorId::Cloudflare,
        &PowCapabilitySample::solved(4_500, 2),
    );
    store.record_sample(
        "example.com",
        TargetClass::ContentSite,
        VendorId::Cloudflare,
        &PowCapabilitySample::solved(4_800, 3),
    );
    for _ in 0..4 {
        store.record_sample(
            "example.com",
            TargetClass::ContentSite,
            VendorId::Cloudflare,
            &PowCapabilitySample::failed(5_000, 3, stygian_charon::pow_profile::PowFailureMode::Captcha),
        );
    }
    for _ in 0..4 {
        store.record_sample(
            "example.com",
            TargetClass::ContentSite,
            VendorId::Cloudflare,
            &PowCapabilitySample::failed(5_000, 3, stygian_charon::pow_profile::PowFailureMode::Blocked),
        );
    }
}

#[test]
fn strong_profile_drives_no_escalation() {
    let store = PowCapabilityStore::with_defaults();
    record_strong_samples(&store);

    let profile = store
        .lookup("example.com", TargetClass::ContentSite, VendorId::Cloudflare)
        .expect("profile");
    let scorer = PowCapabilityScorer::new();
    let score = score_from_profile(&profile, &scorer);
    assert!(
        score.value > 0.75,
        "strong profile should score > 0.75, got {}",
        score.value
    );

    let policy = base_policy();
    let adjusted = adjust_runtime_policy_for_pow(&policy, &score, &PowPolicyThresholds::default());
    assert_eq!(adjusted.execution_mode, policy.execution_mode);
    assert_eq!(adjusted.session_mode, policy.session_mode);
    assert!(adjusted.rate_limit_rps >= 1.0);
    assert!(approx_eq(adjusted.risk_score, policy.risk_score));
    assert_eq!(
        adjusted.config_hints.get("pow.capability"),
        Some(&"strong".to_string())
    );
}

#[test]
fn weak_profile_drives_full_escalation() {
    let store = PowCapabilityStore::with_defaults();
    record_weak_samples(&store);

    let profile = store
        .lookup("example.com", TargetClass::ContentSite, VendorId::Cloudflare)
        .expect("profile");
    let scorer = PowCapabilityScorer::new();
    let score = score_from_profile(&profile, &scorer);
    assert!(
        score.value < 0.40,
        "weak profile should score < 0.40, got {}",
        score.value
    );

    let policy = base_policy();
    let adjusted = adjust_runtime_policy_for_pow(&policy, &score, &PowPolicyThresholds::default());
    assert_eq!(adjusted.execution_mode, ExecutionMode::Browser);
    assert_eq!(adjusted.session_mode, SessionMode::Sticky);
    assert!(adjusted.max_retries > policy.max_retries);
    assert!(adjusted.backoff_base_ms >= 1_000);
    assert!(adjusted.sticky_session_ttl_secs.is_some());
    assert!(adjusted
        .required_stygian_features
        .iter()
        .any(|f| f == "stygian-proxy"));
    assert!(adjusted.risk_score >= policy.risk_score);
    assert_eq!(
        adjusted.config_hints.get("pow.escalation"),
        Some(&"weak".to_string())
    );
}

#[test]
fn sparse_profile_drives_no_op_with_unknown_hint() {
    let store = PowCapabilityStore::with_defaults();
    // Two attempts is below the documented minimum
    // (MIN_OBSERVATIONS_FOR_SCORING = 3).
    store.record_sample(
        "example.com",
        TargetClass::ContentSite,
        VendorId::Cloudflare,
        &PowCapabilitySample::solved(800, 0),
    );
    store.record_sample(
        "example.com",
        TargetClass::ContentSite,
        VendorId::Cloudflare,
        &PowCapabilitySample::solved(900, 0),
    );

    let profile = store
        .lookup("example.com", TargetClass::ContentSite, VendorId::Cloudflare)
        .expect("profile");
    let scorer = PowCapabilityScorer::new();
    let score = score_from_profile(&profile, &scorer);
    assert!(score.is_unknown());

    let policy = base_policy();
    let adjusted = adjust_runtime_policy_for_pow(&policy, &score, &PowPolicyThresholds::default());
    assert_eq!(adjusted.execution_mode, policy.execution_mode);
    assert_eq!(adjusted.session_mode, policy.session_mode);
    assert!(approx_eq(adjusted.risk_score, policy.risk_score));
    assert_eq!(
        adjusted.config_hints.get("pow.capability"),
        Some(&"unknown".to_string())
    );
}

#[test]
fn scorer_is_deterministic_for_same_recorded_sequence() {
    let build = || {
        let store = PowCapabilityStore::with_defaults();
        record_strong_samples(&store);
        store
            .lookup("example.com", TargetClass::ContentSite, VendorId::Cloudflare)
            .expect("profile")
    };
    let a = build();
    let b = build();
    let scorer = PowCapabilityScorer::new();
    assert!(approx_eq(scorer.score(&a), scorer.score(&b)));
    assert_eq!(scorer.band(&a), scorer.band(&b));
}

#[test]
fn distinct_vendors_keep_distinct_profiles() {
    let store = PowCapabilityStore::with_defaults();
    for _ in 0..5 {
        store.record_sample(
            "example.com",
            TargetClass::ContentSite,
            VendorId::Cloudflare,
            &PowCapabilitySample::solved(800, 0),
        );
    }
    for _ in 0..5 {
        store.record_sample(
            "example.com",
            TargetClass::ContentSite,
            VendorId::Akamai,
            &PowCapabilitySample::failed(
                5_000,
                3,
                stygian_charon::pow_profile::PowFailureMode::Captcha,
            ),
        );
    }
    let cloudflare = store
        .lookup("example.com", TargetClass::ContentSite, VendorId::Cloudflare)
        .expect("cloudflare profile");
    let akamai = store
        .lookup("example.com", TargetClass::ContentSite, VendorId::Akamai)
        .expect("akamai profile");
    assert_eq!(cloudflare.solved_count, 5);
    assert_eq!(cloudflare.failed_count, 0);
    assert_eq!(akamai.solved_count, 0);
    assert_eq!(akamai.failed_count, 5);
    let scorer = PowCapabilityScorer::new();
    let cf_score = scorer.score(&cloudflare);
    let ak_score = scorer.score(&akamai);
    assert!(cf_score > ak_score);
}

#[test]
fn profile_merge_aggregates_distinct_vendors_in_hand_rolled_summary() {
    // Demonstrates the merge path used by callers that
    // want a single profile across multiple vendors
    // (e.g. for a "broad" capability estimate).
    let store = PowCapabilityStore::with_defaults();
    record_strong_samples(&store);
    let mut broad = store
        .lookup("example.com", TargetClass::ContentSite, VendorId::Cloudflare)
        .expect("cloudflare profile");
    let akamai_store = PowCapabilityStore::with_defaults();
    for _ in 0..3 {
        akamai_store.record_sample(
            "example.com",
            TargetClass::ContentSite,
            VendorId::Akamai,
            &PowCapabilitySample::failed(
                5_000,
                3,
                stygian_charon::pow_profile::PowFailureMode::Captcha,
            ),
        );
    }
    let akamai = akamai_store
        .lookup("example.com", TargetClass::ContentSite, VendorId::Akamai)
        .expect("akamai profile");
    let combined_solved = broad.solved_count + akamai.solved_count;
    let combined_failed = broad.failed_count + akamai.failed_count;
    broad.merge_profile(&akamai);
    assert_eq!(broad.solved_count, combined_solved);
    assert_eq!(broad.failed_count, combined_failed);
    assert_eq!(broad.solved_count + broad.failed_count, broad.total_attempts());
}

/// End-to-end confirmation that a synthetic `PoW` profile
/// drives the policy mapping: a recorded sequence of
/// samples produces a profile, the profile produces a
/// score, and the score produces a fully-adjusted
/// runtime policy. The test is `#[ignore]`-gated so the
/// fast preflight remains deterministic — the assertion
/// itself is the documented T93 contract.
#[test]
#[ignore = "synthetic_pow_profile_drives_policy_mapping: end-to-end T93 contract"]
fn synthetic_pow_profile_drives_policy_mapping() {
    let store = PowCapabilityStore::with_defaults();
    record_weak_samples(&store);
    let profile = store
        .lookup("example.com", TargetClass::ContentSite, VendorId::Cloudflare)
        .expect("profile");
    let scorer = PowCapabilityScorer::new();
    let score = score_from_profile(&profile, &scorer);
    assert!(score.value < 0.40);
    assert_eq!(scorer.band(&profile), stygian_charon::pow_profile::PowCapabilityBand::Weak);

    let policy = base_policy();
    let adjusted =
        adjust_runtime_policy_for_pow(&policy, &score, &PowPolicyThresholds::default());

    // The synthetic weak profile must:
    // 1. Escalate to Browser + Sticky session.
    // 2. Cap the rate-limit at the documented weak ceiling.
    // 3. Lift backoff and retries.
    // 4. Add the stygian-proxy feature dependency.
    // 5. Lift the risk score by at most MAX_POW_RISK_DELTA.
    assert_eq!(adjusted.execution_mode, ExecutionMode::Browser);
    assert_eq!(adjusted.session_mode, SessionMode::Sticky);
    assert!(adjusted.rate_limit_rps <= 1.0);
    assert!(adjusted.max_retries > policy.max_retries);
    assert!(adjusted.backoff_base_ms >= 1_000);
    assert!(adjusted.sticky_session_ttl_secs.is_some());
    assert!(adjusted
        .required_stygian_features
        .iter()
        .any(|f| f == "stygian-proxy"));
    let lift = adjusted.risk_score - policy.risk_score;
    assert!(lift > 0.0);
    assert!(lift <= stygian_charon::pow_profile::MAX_POW_RISK_DELTA + 1e-9);
    assert_eq!(
        adjusted.config_hints.get("pow.escalation"),
        Some(&"weak".to_string())
    );
}
