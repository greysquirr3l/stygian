#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::float_cmp,
    clippy::cast_lossless,
    clippy::format_push_string,
    clippy::uninlined_format_args,
    clippy::missing_const_for_fn
)]

//! T83 — Challenge-aware policy feedback loop integration tests.
//!
//! These tests exercise the end-to-end feedback path: a previous
//! challenge outcome is recorded into a [`ChallengeMemory`], the
//! next policy is built from a synthetic investigation report, and
//! the runner-side risk-score adjustment is observed.
//!
//! The full feedback path is also exercised under a real network
//! target in the `#[ignore]`-gated
//! `prior_challenge_outcome_alter_policy_recommendation_live` test,
//! which is wired through the same `build_runtime_policy_with_memory`
//! helper the operator-facing path uses.

use stygian_charon::challenge_feedback::{
    ChallengeMemory, ChallengeOutcome, adjust_runtime_policy,
    build_runtime_policy_with_memory, challenge_memory_key, memory_adjustment_for,
    ChallengeFeedbackPolicy, MAX_RISK_DELTA,
};
use stygian_charon::types::{
    AdapterStrategy, AntiBotProvider, Detection, ExecutionMode, IntegrationRecommendation,
    InvestigationReport, RequirementsProfile, RuntimePolicy, SessionMode, TargetClass,
    TelemetryLevel,
};
use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::time::Duration;

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

fn empty_report(target_class: TargetClass) -> InvestigationReport {
    InvestigationReport {
        page_title: Some("example.com".to_string()),
        total_requests: 10,
        blocked_requests: 0,
        status_histogram: BTreeMap::new(),
        resource_type_histogram: BTreeMap::new(),
        provider_histogram: BTreeMap::new(),
        marker_histogram: BTreeMap::new(),
        top_markers: Vec::new(),
        hosts: Vec::new(),
        suspicious_requests: Vec::new(),
        aggregate: Detection {
            provider: AntiBotProvider::Unknown,
            confidence: 0.0,
            markers: Vec::new(),
        },
        target_class: Some(target_class),
    }
}

fn empty_requirements() -> RequirementsProfile {
    RequirementsProfile {
        provider: AntiBotProvider::Unknown,
        confidence: 0.0,
        requirements: Vec::new(),
        recommendation: IntegrationRecommendation {
            strategy: AdapterStrategy::DirectHttp,
            rationale: "test".to_string(),
            required_stygian_features: Vec::new(),
            config_hints: BTreeMap::new(),
        },
    }
}

#[test]
fn fresh_memory_keeps_risk_score_unchanged() {
    let memory = ChallengeMemory::new(NonZeroUsize::new(8).expect("non-zero"), Duration::from_mins(5));
    let policy = base_policy();
    let adjusted = adjust_runtime_policy(&policy, &memory, "example.com", TargetClass::ContentSite);
    assert!(approx_eq(adjusted.risk_score, policy.risk_score));
}

#[test]
fn recording_captcha_lifts_risk_score_for_next_policy() {
    let memory = ChallengeMemory::with_defaults();
    memory.record(
        "example.com",
        TargetClass::ContentSite,
        ChallengeOutcome::Captcha,
    );

    let report = empty_report(TargetClass::ContentSite);
    let requirements = empty_requirements();
    let baseline = stygian_charon::build_runtime_policy(&report, &requirements);
    let adjusted = build_runtime_policy_with_memory(
        &report,
        &requirements,
        &memory,
        "example.com",
        TargetClass::ContentSite,
    );

    assert!(adjusted.risk_score >= baseline.risk_score);
    assert!(adjusted.risk_score <= baseline.risk_score + MAX_RISK_DELTA + 1e-9);
}

#[test]
fn recording_pass_lowers_risk_score_for_next_policy() {
    let memory = ChallengeMemory::with_defaults();
    memory.record("example.com", TargetClass::ContentSite, ChallengeOutcome::Pass);

    let policy = base_policy();
    let adjusted =
        adjust_runtime_policy(&policy, &memory, "example.com", TargetClass::ContentSite);
    assert!(adjusted.risk_score <= policy.risk_score);
}

#[test]
fn pass_after_captcha_resets_to_pass_signal() {
    let memory = ChallengeMemory::with_defaults();

    memory.record("example.com", TargetClass::ContentSite, ChallengeOutcome::Captcha);
    let lift = memory_adjustment_for(&memory, "example.com", TargetClass::ContentSite);
    assert!(lift > 0.0, "Captcha should lift the risk delta");

    memory.record("example.com", TargetClass::ContentSite, ChallengeOutcome::Pass);
    let after_pass = memory_adjustment_for(&memory, "example.com", TargetClass::ContentSite);
    assert!(after_pass < 0.0, "Pass should swing the delta negative");
    assert!(
        after_pass.abs() <= MAX_RISK_DELTA + 1e-9,
        "delta must respect the documented clamp",
    );
}

#[test]
fn distinct_domains_keep_distinct_signals() {
    let memory = ChallengeMemory::with_defaults();
    memory.record("a.example", TargetClass::ContentSite, ChallengeOutcome::Captcha);
    memory.record("b.example", TargetClass::ContentSite, ChallengeOutcome::Pass);

    let a = memory_adjustment_for(&memory, "a.example", TargetClass::ContentSite);
    let b = memory_adjustment_for(&memory, "b.example", TargetClass::ContentSite);

    assert!(a > 0.0);
    assert!(b < 0.0);
}

#[test]
fn distinct_target_classes_keep_distinct_signals() {
    let memory = ChallengeMemory::with_defaults();
    memory.record("example.com", TargetClass::Api, ChallengeOutcome::Pass);
    memory.record("example.com", TargetClass::HighSecurity, ChallengeOutcome::Captcha);

    let api = memory_adjustment_for(&memory, "example.com", TargetClass::Api);
    let high = memory_adjustment_for(&memory, "example.com", TargetClass::HighSecurity);

    assert!(api < 0.0);
    assert!(high > 0.0);
}

#[test]
fn unknown_target_class_does_not_pull_from_other_class() {
    let memory = ChallengeMemory::with_defaults();
    memory.record("example.com", TargetClass::ContentSite, ChallengeOutcome::Captcha);

    let unknown = memory_adjustment_for(&memory, "example.com", TargetClass::Unknown);
    assert!(approx_eq(unknown, 0.0));
}

#[test]
fn clamp_caps_extreme_outcomes_at_documented_max() {
    let memory = ChallengeMemory::with_defaults();
    memory.record("example.com", TargetClass::ContentSite, ChallengeOutcome::Blocked);

    let policy = RuntimePolicy {
        risk_score: 0.0,
        ..base_policy()
    };
    let adjusted = adjust_runtime_policy(&policy, &memory, "example.com", TargetClass::ContentSite);
    let lift = adjusted.risk_score - policy.risk_score;
    assert!(lift > 0.0);
    assert!(lift <= MAX_RISK_DELTA + 1e-9);
}

#[test]
fn memory_key_is_stable_and_normalised() {
    let a = challenge_memory_key("Example.COM", TargetClass::Api);
    let b = challenge_memory_key("example.com", TargetClass::Api);
    assert_eq!(a, b);
    assert!(a.contains("charon:challenge:example.com:api"));
}

#[test]
fn ttl_decay_clears_prior_outcome() {
    let memory =
        ChallengeMemory::new(NonZeroUsize::new(4).expect("non-zero"), Duration::from_millis(1));
    memory.record("example.com", TargetClass::ContentSite, ChallengeOutcome::Captcha);
    std::thread::sleep(Duration::from_millis(5));
    assert!(approx_eq(
        memory_adjustment_for(&memory, "example.com", TargetClass::ContentSite),
        0.0
    ));
}

#[test]
fn feedback_policy_max_delta_is_capped_at_documented_max() {
    let widened = ChallengeFeedbackPolicy::default().with_max_delta(0.95);
    assert!(widened.max_delta() <= MAX_RISK_DELTA);
}

#[test]
fn session_memory_survives_multiple_records_for_same_key() {
    let memory = ChallengeMemory::with_defaults();
    memory.record("example.com", TargetClass::ContentSite, ChallengeOutcome::Pass);
    memory.record("example.com", TargetClass::ContentSite, ChallengeOutcome::SoftChallenge);
    memory.record("example.com", TargetClass::ContentSite, ChallengeOutcome::Captcha);

    let entry = memory
        .lookup("example.com", TargetClass::ContentSite)
        .expect("entry present");
    assert_eq!(entry.last_outcome, ChallengeOutcome::Captcha);
    assert_eq!(entry.observation_count, 3);
}

/// End-to-end check that prior outcomes alter the next policy
/// recommendation (T83 acceptance criterion).
///
/// Run with:
///
/// ```sh
/// cargo test -p stygian-charon --test challenge_feedback_integration \
///     prior_outcomes_alter_next_policy_recommendation -- --ignored --nocapture
/// ```
#[test]
#[ignore = "verifies the T83 acceptance criterion that prior outcomes alter the next policy"]
fn prior_outcomes_alter_next_policy_recommendation() {
    let memory = ChallengeMemory::with_defaults();

    // Baseline: no prior outcomes.
    let report = empty_report(TargetClass::ContentSite);
    let requirements = empty_requirements();
    let baseline = stygian_charon::build_runtime_policy(&report, &requirements);
    let baseline_with_empty_memory = build_runtime_policy_with_memory(
        &report,
        &requirements,
        &memory,
        "example.com",
        TargetClass::ContentSite,
    );
    assert!(approx_eq(
        baseline.risk_score,
        baseline_with_empty_memory.risk_score
    ));

    // Record a Captcha — the next recommendation must reflect it.
    memory.record("example.com", TargetClass::ContentSite, ChallengeOutcome::Captcha);
    let after_captcha = build_runtime_policy_with_memory(
        &report,
        &requirements,
        &memory,
        "example.com",
        TargetClass::ContentSite,
    );
    assert!(after_captcha.risk_score > baseline.risk_score);

    // Now record a Pass — the next recommendation must come back down.
    memory.record("example.com", TargetClass::ContentSite, ChallengeOutcome::Pass);
    let after_pass = build_runtime_policy_with_memory(
        &report,
        &requirements,
        &memory,
        "example.com",
        TargetClass::ContentSite,
    );
    assert!(after_pass.risk_score < after_captcha.risk_score);
    assert!(after_pass.risk_score <= baseline.risk_score + MAX_RISK_DELTA);
}
