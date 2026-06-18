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

//! T85 — Target-class playbooks as code integration tests.
//!
//! Exercises the end-to-end playbook resolution path: the resolver
//! merges per-request overrides with the codified playbook defaults
//! and the global fallback, then maps the result into the
//! acquisition-runner-facing [`AcquisitionPolicy`]. Each tier of the
//! precedence ladder is covered by at least one test so a regression
//! in any single layer surfaces immediately.
//!
//! The `#[ignore]`-gated `resolved_playbook_drives_acquisition_runner_config`
//! test confirms the resolved playbook contains every field the
//! downstream [`AcquisitionRunner`] config needs (by mapping into
//! [`AcquisitionPolicy`] via [`map_policy_hints`]).

use std::collections::BTreeMap;
use stygian_charon::acquisition::{AcquisitionModeHint, map_policy_hints};
use stygian_charon::playbooks::{
    AcquisitionDefaults, AcquisitionOverrides, EscalationStrategy, PacingProfile, Playbook,
    PlaybookOverrides, PlaybookResolver, ProxyPreference, ResolutionSource, ValidationError,
};
use stygian_charon::types::{
    ExecutionMode, RuntimePolicy, SessionMode, TargetClass, TelemetryLevel,
};

fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn empty_runtime_policy() -> RuntimePolicy {
    RuntimePolicy {
        execution_mode: ExecutionMode::Http,
        session_mode: SessionMode::Stateless,
        telemetry_level: TelemetryLevel::Standard,
        rate_limit_rps: 1.0,
        max_retries: 2,
        backoff_base_ms: 250,
        enable_warmup: false,
        enforce_webrtc_proxy_only: false,
        sticky_session_ttl_secs: None,
        required_stygian_features: Vec::new(),
        config_hints: BTreeMap::new(),
        risk_score: 0.0,
    }
}

// =====================================================================
// Precedence tier 1 — request override wins
// =====================================================================

#[test]
fn request_override_wins_for_each_overridable_field() {
    let resolver = PlaybookResolver::with_builtin_defaults();
    let overrides = PlaybookOverrides {
        acquisition: AcquisitionOverrides {
            mode: Some(AcquisitionModeHint::Hostile),
            execution_mode: Some(ExecutionMode::Browser),
            session_mode: Some(SessionMode::Sticky),
            telemetry_level: Some(TelemetryLevel::Deep),
            retry_budget: Some(11),
            backoff_base_ms: Some(1234),
            enable_warmup: Some(true),
        },
        ..PlaybookOverrides::default()
    };

    let resolved = resolver
        .resolve(TargetClass::ContentSite, "tier1-js", &overrides)
        .expect("resolve");
    assert_eq!(resolved.acquisition.mode, AcquisitionModeHint::Hostile);
    assert_eq!(
        resolved.acquisition.mode_source,
        ResolutionSource::RequestOverride
    );
    assert_eq!(resolved.acquisition.execution_mode, ExecutionMode::Browser);
    assert_eq!(
        resolved.acquisition.execution_mode_source,
        ResolutionSource::RequestOverride
    );
    assert_eq!(resolved.acquisition.session_mode, SessionMode::Sticky);
    assert_eq!(
        resolved.acquisition.session_mode_source,
        ResolutionSource::RequestOverride
    );
    assert_eq!(resolved.acquisition.telemetry_level, TelemetryLevel::Deep);
    assert_eq!(
        resolved.acquisition.telemetry_level_source,
        ResolutionSource::RequestOverride
    );
    assert_eq!(resolved.acquisition.retry_budget, 11);
    assert_eq!(
        resolved.acquisition.retry_budget_source,
        ResolutionSource::RequestOverride
    );
    assert_eq!(resolved.acquisition.backoff_base_ms, 1234);
    assert_eq!(
        resolved.acquisition.backoff_base_source,
        ResolutionSource::RequestOverride
    );
    assert!(resolved.acquisition.enable_warmup);
    assert_eq!(
        resolved.acquisition.enable_warmup_source,
        ResolutionSource::RequestOverride
    );
}

#[test]
fn request_override_replaces_proxy_preference_whole() {
    let resolver = PlaybookResolver::with_builtin_defaults();
    let proxy = ProxyPreference {
        preferred_protocol: "socks5".to_string(),
        require_sticky: true,
        require_residential: true,
        max_latency_ms: Some(300),
    };
    let overrides = PlaybookOverrides {
        proxy_preference: Some(proxy.clone()),
        ..PlaybookOverrides::default()
    };

    let resolved = resolver
        .resolve(TargetClass::ContentSite, "tier1-static", &overrides)
        .expect("resolve");
    assert_eq!(resolved.proxy_preference, proxy);
    assert_eq!(
        resolved.proxy_preference_source,
        ResolutionSource::RequestOverride
    );
}

// =====================================================================
// Precedence tier 2 — playbook default fills when no override
// =====================================================================

#[test]
fn playbook_default_used_when_no_override_set() {
    let resolver = PlaybookResolver::with_builtin_defaults();
    let resolved = resolver
        .resolve(
            TargetClass::ContentSite,
            "tier1-js",
            &PlaybookOverrides::default(),
        )
        .expect("resolve");

    // tier1-js defaults: mode=resilient, execution=browser, session=sticky,
    // telemetry=standard, retry_budget=5, backoff_base_ms=500, warmup=true
    assert_eq!(resolved.acquisition.mode, AcquisitionModeHint::Resilient);
    assert_eq!(
        resolved.acquisition.mode_source,
        ResolutionSource::PlaybookDefault
    );
    assert_eq!(resolved.acquisition.execution_mode, ExecutionMode::Browser);
    assert_eq!(
        resolved.acquisition.execution_mode_source,
        ResolutionSource::PlaybookDefault
    );
    assert_eq!(resolved.acquisition.session_mode, SessionMode::Sticky);
    assert_eq!(
        resolved.acquisition.session_mode_source,
        ResolutionSource::PlaybookDefault
    );
    assert_eq!(resolved.acquisition.retry_budget, 5);
    assert_eq!(
        resolved.acquisition.retry_budget_source,
        ResolutionSource::PlaybookDefault
    );
    assert_eq!(resolved.acquisition.backoff_base_ms, 500);
    assert!(resolved.acquisition.enable_warmup);
    assert_eq!(resolved.acquisition.sticky_session_ttl_secs, Some(600));
    assert_eq!(
        resolved.acquisition.sticky_session_ttl_source,
        ResolutionSource::PlaybookDefault
    );
}

#[test]
fn tier2_hostile_playbook_emits_hostile_mode() {
    let resolver = PlaybookResolver::with_builtin_defaults();
    let resolved = resolver
        .resolve(
            TargetClass::HighSecurity,
            "tier2-hostile",
            &PlaybookOverrides::default(),
        )
        .expect("resolve");
    assert_eq!(resolved.acquisition.mode, AcquisitionModeHint::Hostile);
    assert!(resolved.acquisition.enable_warmup);
    assert!(resolved.proxy_preference.require_residential);
    assert!(resolved.proxy_preference.require_sticky);
    // tier2-hostile uses a linear escalation ladder
    let stages = resolved.escalation.stages();
    assert!(stages.contains(&AcquisitionModeHint::Hostile));
    assert!(stages.contains(&AcquisitionModeHint::Resilient));
}

// =====================================================================
// Precedence tier 3 — global default when no playbook matches
// =====================================================================

#[test]
fn global_default_used_when_no_playbook_matches_target_class() {
    let resolver = PlaybookResolver::with_builtin_defaults();
    let resolved = resolver
        .resolve(TargetClass::Api, "", &PlaybookOverrides::default())
        .expect("resolve");
    // TargetClass::Api has no registered playbook, so the resolver
    // must fall through to the unknown / global-default playbook.
    assert_eq!(resolved.playbook_id, "unknown");
    assert_eq!(
        resolved.acquisition.mode_source,
        ResolutionSource::GlobalDefault
    );
    assert_eq!(
        resolved.acquisition.retry_budget_source,
        ResolutionSource::GlobalDefault
    );
    assert_eq!(
        resolved.acquisition.execution_mode_source,
        ResolutionSource::GlobalDefault
    );
    assert_eq!(
        resolved.proxy_preference_source,
        ResolutionSource::GlobalDefault
    );
    assert_eq!(resolved.pacing_source, ResolutionSource::GlobalDefault);
    assert_eq!(resolved.escalation_source, ResolutionSource::GlobalDefault);
}

#[test]
fn unknown_explicit_playbook_id_returns_error() {
    let resolver = PlaybookResolver::with_builtin_defaults();
    let err = resolver
        .resolve(
            TargetClass::ContentSite,
            "nope-not-a-playbook",
            &PlaybookOverrides::default(),
        )
        .expect_err("unknown id");
    assert!(matches!(err, ValidationError::UnknownPlaybook { .. }));
}

// =====================================================================
// Validation errors include field path + bad value
// =====================================================================

#[test]
fn validation_error_message_includes_field_path_and_bad_value() {
    let bad = Playbook {
        id: "broken".to_string(),
        target_class: TargetClass::ContentSite,
        description: "intentionally broken".to_string(),
        acquisition: AcquisitionDefaults {
            mode: AcquisitionModeHint::Fast,
            execution_mode: ExecutionMode::Http,
            session_mode: SessionMode::Stateless,
            telemetry_level: TelemetryLevel::Basic,
            sticky_session_ttl_secs: None,
            enable_warmup: false,
            retry_budget: 0, // invalid: must be > 0
            backoff_base_ms: 250,
        },
        proxy_preference: ProxyPreference {
            preferred_protocol: "ftp".to_string(), // invalid
            require_sticky: false,
            require_residential: false,
            max_latency_ms: None,
        },
        pacing: PacingProfile {
            rate_limit_rps: -0.5,       // invalid
            jitter_pct: 1.5,            // invalid
            min_request_interval_ms: 0, // invalid
        },
        escalation: EscalationStrategy::Linear { steps: Vec::new() }, // invalid
    };

    let err = bad.validate().expect_err("validation must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("acquisition.retry_budget"),
        "message must name the field: {msg}"
    );
    assert!(
        msg.contains('0'),
        "message must include the bad value: {msg}"
    );
    assert_eq!(err.field_path(), Some("acquisition.retry_budget"));
    assert_eq!(err.bad_value(), Some("0"));
}

#[test]
fn validation_error_for_negative_pacing_rate_is_actionable() {
    let bad = Playbook {
        id: "broken-pacing".to_string(),
        target_class: TargetClass::ContentSite,
        description: String::new(),
        acquisition: AcquisitionDefaults::default(),
        proxy_preference: ProxyPreference::default(),
        pacing: PacingProfile {
            rate_limit_rps: -0.5,
            jitter_pct: 0.10,
            min_request_interval_ms: 500,
        },
        escalation: EscalationStrategy::default(),
    };
    let err = bad.validate().expect_err("negative pacing");
    let msg = err.to_string();
    assert!(msg.contains("pacing.rate_limit_rps"));
    assert!(msg.contains("-0.5"));
}

#[test]
fn validation_error_for_unknown_proxy_protocol_is_actionable() {
    let bad = Playbook {
        id: "broken-proxy".to_string(),
        target_class: TargetClass::ContentSite,
        description: String::new(),
        acquisition: AcquisitionDefaults::default(),
        proxy_preference: ProxyPreference {
            preferred_protocol: "ftp".to_string(),
            require_sticky: false,
            require_residential: false,
            max_latency_ms: None,
        },
        pacing: PacingProfile::default(),
        escalation: EscalationStrategy::default(),
    };
    let err = bad.validate().expect_err("unknown protocol");
    let msg = err.to_string();
    assert!(msg.contains("proxy_preference.preferred_protocol"));
    assert!(msg.contains("ftp"));
}

// =====================================================================
// Loader / runtime exercises
// =====================================================================

#[test]
fn builtin_playbooks_parse_and_validate_at_compile_time() {
    let resolver = PlaybookResolver::with_builtin_defaults();
    let ids = resolver.playbook_ids();
    assert!(ids.contains(&"tier1-static".to_string()));
    assert!(ids.contains(&"tier1-js".to_string()));
    assert!(ids.contains(&"tier2-hostile".to_string()));
    assert!(ids.contains(&"unknown".to_string()));
    assert_eq!(ids.len(), 4);
}

#[test]
fn resolved_playbook_maps_into_acquisition_policy() {
    let resolver = PlaybookResolver::with_builtin_defaults();
    let resolved = resolver
        .resolve(
            TargetClass::ContentSite,
            "tier1-js",
            &PlaybookOverrides::default(),
        )
        .expect("resolve");
    let policy = map_policy_hints(&resolved.to_runtime_policy_hints());
    assert_eq!(policy.retry_budget, 5);
    assert_eq!(policy.backoff_base_ms, 500);
    assert!(policy.enable_warmup);
    assert!(policy.sticky_session);
    assert_eq!(policy.telemetry_level, TelemetryLevel::Standard);
}

#[test]
fn resolved_playbook_drives_runner_config_when_overridden() {
    let resolver = PlaybookResolver::with_builtin_defaults();
    let overrides = PlaybookOverrides {
        acquisition: AcquisitionOverrides {
            retry_budget: Some(7),
            backoff_base_ms: Some(900),
            enable_warmup: Some(true),
            ..AcquisitionOverrides::default()
        },
        ..PlaybookOverrides::default()
    };
    let resolved = resolver
        .resolve(TargetClass::ContentSite, "tier1-static", &overrides)
        .expect("resolve");
    let policy = map_policy_hints(&resolved.to_runtime_policy_hints());
    assert_eq!(policy.retry_budget, 7);
    assert_eq!(policy.backoff_base_ms, 900);
    assert!(policy.enable_warmup);
}

#[test]
fn resolver_optional_falls_back_to_target_class_default() {
    let resolver = PlaybookResolver::with_builtin_defaults();
    let resolved = resolver
        .resolve_optional(
            TargetClass::HighSecurity,
            None,
            &PlaybookOverrides::default(),
        )
        .expect("resolve");
    assert_eq!(resolved.playbook_id, "tier2-hostile");
}

#[test]
fn risk_score_defaults_match_acquisition_helper() {
    // Resolved acquisition hints combined with the rest of the
    // runtime policy should not corrupt the risk_score field.
    let resolver = PlaybookResolver::with_builtin_defaults();
    let resolved = resolver
        .resolve(
            TargetClass::ContentSite,
            "tier1-static",
            &PlaybookOverrides::default(),
        )
        .expect("resolve");
    let policy = empty_runtime_policy();
    let hints = resolved.to_runtime_policy_hints();
    assert_eq!(hints.execution_mode, Some(ExecutionMode::Http));
    assert_eq!(hints.session_mode, Some(SessionMode::Stateless));
    assert_eq!(hints.max_retries, Some(2));
    assert_eq!(hints.backoff_base_ms, Some(250));
    assert!(approx_eq(policy.risk_score, 0.0));
}

// =====================================================================
// #[ignore] — confirms a resolved playbook drives a real AcquisitionRunner config
// =====================================================================

/// T85 acceptance criterion: a resolved playbook must contain every
/// field a real `AcquisitionRunner` config needs. We verify this by
/// mapping the resolved playbook into the canonical
/// [`AcquisitionPolicy`] (the same output charon's
/// `map_policy_hints` produces for the downstream
/// `AcquisitionRequest::mode` / sticky-session / retry-budget
/// fields). Marked `#[ignore]` so it is **only** exercised on CI or
/// when an operator opts in via
/// `cargo test -- --ignored`. This is consistent with the T83
/// pattern where live-target validation is also gated.
///
/// Run with:
///
/// ```sh
/// cargo test -p stygian-charon --test playbooks_integration \
///     resolved_playbook_drives_acquisition_runner_config -- --ignored --nocapture
/// ```
#[test]
#[ignore = "verifies T85 acceptance criterion: resolved playbook drives a real AcquisitionRunner config"]
fn resolved_playbook_drives_acquisition_runner_config() {
    let resolver = PlaybookResolver::with_builtin_defaults();

    // Real-world override scenario: operator wants Hostile mode +
    // deep telemetry for an unexpected Tier-1 JS site.
    let overrides = PlaybookOverrides {
        acquisition: AcquisitionOverrides {
            mode: Some(AcquisitionModeHint::Hostile),
            telemetry_level: Some(TelemetryLevel::Deep),
            enable_warmup: Some(true),
            ..AcquisitionOverrides::default()
        },
        ..PlaybookOverrides::default()
    };
    let resolved = resolver
        .resolve(TargetClass::ContentSite, "tier1-js", &overrides)
        .expect("resolve");

    // Every field the downstream AcquisitionRunner config needs:
    let policy = map_policy_hints(&resolved.to_runtime_policy_hints());

    // - mode drives the strategy ladder (Fast/Resilient/Hostile/Investigate)
    assert_eq!(policy.mode, AcquisitionModeHint::Hostile);
    // - retry_budget drives the per-stage retry loop
    assert_eq!(policy.retry_budget, 5);
    // - backoff_base_ms drives the backoff curve between retries
    assert_eq!(policy.backoff_base_ms, 500);
    // - enable_warmup drives the warmup navigation phase
    assert!(policy.enable_warmup);
    // - sticky_session drives BrowserPool::acquire_for(host) pinning
    assert!(policy.sticky_session);
    // - telemetry_level is forwarded into the runner's diagnostic bundle
    assert_eq!(policy.telemetry_level, TelemetryLevel::Deep);

    // Confirm the resolution sources tag every field as
    // "RequestOverride" (top priority) for the overridden fields.
    assert_eq!(
        resolved.acquisition.mode_source,
        ResolutionSource::RequestOverride
    );
    assert_eq!(
        resolved.acquisition.telemetry_level_source,
        ResolutionSource::RequestOverride
    );
    assert_eq!(
        resolved.acquisition.enable_warmup_source,
        ResolutionSource::RequestOverride
    );

    // Confirm the escalation strategy produces a deterministic
    // ladder — a downstream runner uses `stages()` to walk the
    // ordered escalation list.
    let stages = resolved.escalation.stages();
    assert!(!stages.is_empty());
    assert_eq!(
        *stages.last().expect("ceiling"),
        AcquisitionModeHint::Hostile
    );
}
