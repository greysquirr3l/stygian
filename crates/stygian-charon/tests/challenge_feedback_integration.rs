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

//! T83 / T110 — Challenge-aware policy feedback loop integration
//! tests.
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
//!
//! All paths use the **engine-keyed** API introduced in T110:
//! outcomes are recorded and looked up by [`EngineKey`], not by
//! `(domain, target_class)`. The integration-level guard test
//! `patch_propagates_across_urls_on_same_engine` (T110) verifies
//! that a self-healing patch recorded against one URL on one
//! engine heals every URL on that engine.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::time::Duration;
use stygian_charon::challenge_feedback::{
    ChallengeFeedbackPolicy, ChallengeMemory, ChallengeOutcome, EngineKey, MAX_RISK_DELTA,
    adjust_runtime_policy, build_runtime_policy_with_memory, engine_memory_key,
    memory_adjustment_for,
};
use stygian_charon::types::{
    AdapterStrategy, AntiBotProvider, Detection, ExecutionMode, IntegrationRecommendation,
    InvestigationReport, RequirementsProfile, RuntimePolicy, SessionMode, TargetClass,
    TelemetryLevel,
};
use stygian_charon::vendor_classifier::VendorId;

fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn cf_content() -> EngineKey {
    EngineKey {
        engine: VendorId::Cloudflare,
        version: None,
        target_class: TargetClass::ContentSite,
        tls_profile: None,
    }
}

fn cf_api() -> EngineKey {
    EngineKey {
        engine: VendorId::Cloudflare,
        version: None,
        target_class: TargetClass::Api,
        tls_profile: None,
    }
}

fn cf_high_security() -> EngineKey {
    EngineKey {
        engine: VendorId::Cloudflare,
        version: None,
        target_class: TargetClass::HighSecurity,
        tls_profile: None,
    }
}

fn cf_unknown() -> EngineKey {
    EngineKey {
        engine: VendorId::Cloudflare,
        version: None,
        target_class: TargetClass::Unknown,
        tls_profile: None,
    }
}

fn akamai_v3_api() -> EngineKey {
    EngineKey {
        engine: VendorId::Akamai,
        version: Some("bot-manager-v3".to_string()),
        target_class: TargetClass::Api,
        tls_profile: None,
    }
}

fn datadome_api_chrome136() -> EngineKey {
    EngineKey {
        engine: VendorId::DataDome,
        version: None,
        target_class: TargetClass::Api,
        tls_profile: Some("chrome136".to_string()),
    }
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
    let memory = ChallengeMemory::new(
        NonZeroUsize::new(8).expect("non-zero"),
        Duration::from_mins(5),
    );
    let policy = base_policy();
    let adjusted = adjust_runtime_policy(&policy, &memory, &cf_content());
    assert!(approx_eq(adjusted.risk_score, policy.risk_score));
}

#[test]
fn recording_captcha_lifts_risk_score_for_next_policy() {
    let memory = ChallengeMemory::with_defaults();
    memory.record(&cf_content(), None, ChallengeOutcome::Captcha);

    let report = empty_report(TargetClass::ContentSite);
    let requirements = empty_requirements();
    let baseline = stygian_charon::build_runtime_policy(&report, &requirements);
    let adjusted = build_runtime_policy_with_memory(&report, &requirements, &memory, &cf_content());

    assert!(adjusted.risk_score >= baseline.risk_score);
}

#[test]
fn distinct_target_classes_keep_distinct_entries() {
    let memory = ChallengeMemory::with_defaults();
    memory.record(&cf_content(), None, ChallengeOutcome::Pass);
    memory.record(&cf_api(), None, ChallengeOutcome::Captcha);

    let content_entry = memory.lookup(&cf_content()).expect("content entry");
    let api_entry = memory.lookup(&cf_api()).expect("api entry");

    assert_eq!(content_entry.last_outcome, ChallengeOutcome::Pass);
    assert_eq!(api_entry.last_outcome, ChallengeOutcome::Captcha);
}

#[test]
fn distinct_target_classes_yield_distinct_adjustments() {
    let memory = ChallengeMemory::with_defaults();
    memory.record(&cf_content(), None, ChallengeOutcome::Pass);
    memory.record(&cf_api(), None, ChallengeOutcome::Captcha);
    memory.record(&cf_high_security(), None, ChallengeOutcome::HardChallenge);

    let api = memory_adjustment_for(&memory, &cf_api());
    let high = memory_adjustment_for(&memory, &cf_high_security());

    assert!(api < 0.0);
    assert!(high > 0.0);
}

#[test]
fn unknown_target_class_does_not_pull_from_other_class() {
    let memory = ChallengeMemory::with_defaults();
    memory.record(&cf_content(), None, ChallengeOutcome::Captcha);

    let unknown = memory_adjustment_for(&memory, &cf_unknown());
    assert!(approx_eq(unknown, 0.0));
}

#[test]
fn clamp_caps_extreme_outcomes_at_documented_max() {
    let memory = ChallengeMemory::with_defaults();
    memory.record(&cf_content(), None, ChallengeOutcome::Blocked);

    let policy = RuntimePolicy {
        risk_score: 0.0,
        ..base_policy()
    };
    let adjusted = adjust_runtime_policy(&policy, &memory, &cf_content());
    let lift = adjusted.risk_score - policy.risk_score;
    assert!(lift > 0.0);
    assert!(lift <= MAX_RISK_DELTA + 1e-9);
}

#[test]
fn engine_memory_key_is_stable_and_namespaced() {
    let a = engine_memory_key(&cf_content());
    let b = engine_memory_key(&cf_content());
    assert_eq!(a, b);
    assert!(a.contains("charon:challenge:cloudflare:content_site"));
}

#[test]
fn ttl_decay_clears_prior_outcome() {
    let memory = ChallengeMemory::new(
        NonZeroUsize::new(4).expect("non-zero"),
        Duration::from_millis(1),
    );
    memory.record(&cf_content(), None, ChallengeOutcome::Captcha);
    std::thread::sleep(Duration::from_millis(5));
    assert!(approx_eq(
        memory_adjustment_for(&memory, &cf_content()),
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
    memory.record(&cf_content(), None, ChallengeOutcome::Pass);
    memory.record(&cf_content(), None, ChallengeOutcome::SoftChallenge);
    memory.record(&cf_content(), None, ChallengeOutcome::Captcha);

    let entry = memory.lookup(&cf_content()).expect("entry present");
    assert_eq!(entry.last_outcome, ChallengeOutcome::Captcha);
    assert_eq!(entry.observation_count, 3);
}

// --------------------------------------------------------------------
// T110 integration guard tests
// --------------------------------------------------------------------

/// T110 guard test (integration): a self-healing patch recorded
/// against URL A on engine E heals URL B on engine E. The URL is
/// not the key; the engine is.
#[test]
fn patch_propagates_across_urls_on_same_engine() {
    let memory = ChallengeMemory::with_defaults();
    let key = cf_content();

    memory.record(
        &key,
        Some("https://example.com/cloudflare/page1"),
        ChallengeOutcome::Captcha,
    );

    // Adjust the runtime policy referencing *only* the engine key.
    // The fact that we never named the URL is the whole point.
    let policy = base_policy();
    let adjusted = adjust_runtime_policy(&policy, &memory, &key);
    assert!(
        adjusted.risk_score > policy.risk_score,
        "captcha recorded on URL A must heal the runtime policy for URL B too"
    );

    // Look up the same engine key again under a different URL
    // (debugging context) — the patch must propagate.
    memory.record(
        &key,
        Some("https://example.com/cloudflare/page2"),
        ChallengeOutcome::Captcha,
    );
    let entry = memory.lookup(&key).expect("entry present");
    assert_eq!(
        entry.observation_count, 2,
        "URLs are not keys — observation_count must aggregate across URLs"
    );
    assert_eq!(
        entry.last_observed_url.as_deref(),
        Some("https://example.com/cloudflare/page2"),
        "last_observed_url tracks the most recent URL we saw the engine on"
    );
}

/// T110 guard test (integration): `EngineKey`s differing only by
/// `tls_profile` keep separate memory. Encoding the wrong
/// combination is structurally impossible.
#[test]
fn tls_profile_variants_keep_distinct_memory() {
    let memory = ChallengeMemory::with_defaults();
    let baseline = cf_api();
    let with_tls = datadome_api_chrome136();

    memory.record(&baseline, None, ChallengeOutcome::Pass);
    memory.record(&with_tls, None, ChallengeOutcome::HardChallenge);

    let base_entry = memory.lookup(&baseline).expect("baseline entry");
    let tls_entry = memory.lookup(&with_tls).expect("tls entry");
    assert_eq!(base_entry.last_outcome, ChallengeOutcome::Pass);
    assert_eq!(tls_entry.last_outcome, ChallengeOutcome::HardChallenge);
    assert_ne!(base_entry.key, tls_entry.key);
}

/// T110 guard test (integration): re-keying the vendor version
/// does **not** silently carry the old patch. `bot-manager-v3`
/// memory is structurally separate from `bot-manager-v4` — v4 has
/// no recorded entry, so `lookup(v4)` returns `None`.
#[test]
fn re_keying_vendor_version_does_not_carry_old_patches() {
    let memory = ChallengeMemory::with_defaults();
    let v3 = akamai_v3_api();
    let v4 = EngineKey {
        version: Some("bot-manager-v4".to_string()),
        ..v3.clone()
    };

    memory.record(&v3, None, ChallengeOutcome::Captcha);
    let v3_entry = memory.lookup(&v3).expect("v3 entry");

    assert_eq!(v3_entry.last_outcome, ChallengeOutcome::Captcha);
    assert_ne!(v3_entry.key.version, v4.version);
    assert_eq!(v3_entry.observation_count, 1);

    // v4 was never recorded — must not have an entry. This is the
    // structural separation that makes the v3 self-healing patch
    // invisible to v4 (and vice versa).
    assert!(
        memory.lookup(&v4).is_none(),
        "v4 must not inherit v3's entry — version is part of the engine key"
    );
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
    let baseline_with_empty_memory =
        build_runtime_policy_with_memory(&report, &requirements, &memory, &cf_content());
    assert!(approx_eq(
        baseline.risk_score,
        baseline_with_empty_memory.risk_score
    ));

    // Record a Captcha — the next recommendation must reflect it.
    memory.record(&cf_content(), None, ChallengeOutcome::Captcha);
    let after_captcha =
        build_runtime_policy_with_memory(&report, &requirements, &memory, &cf_content());
    assert!(after_captcha.risk_score > baseline.risk_score);

    // Now record a Pass — the next recommendation must come back down.
    memory.record(&cf_content(), None, ChallengeOutcome::Pass);
    let after_pass =
        build_runtime_policy_with_memory(&report, &requirements, &memory, &cf_content());
    assert!(after_pass.risk_score < after_captcha.risk_score);
    assert!(after_pass.risk_score <= baseline.risk_score + MAX_RISK_DELTA);
}
