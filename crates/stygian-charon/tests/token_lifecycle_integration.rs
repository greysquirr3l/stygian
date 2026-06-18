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

//! T91 — Challenge-token lifecycle contracts integration tests.
//!
//! Exercises the end-to-end validation path:
//!
//! - Fresh, well-formed contracts for each Tier 1 baseline vendor
//!   (`Cloudflare`, `DataDome`, `PerimeterX`, `Akamai`) accept
//!   with the expected `effective_ttl`.
//! - TTL, nonce, single-use replay, session-binding, and
//!   not-applicable rejection paths return a
//!   [`ValidationOutcome::Rejected`] whose
//!   [`InvalidationReason`][stygian_charon::token_lifecycle::InvalidationReason]
//!   carries the right vendor family + challenge class.
//! - Multi-use tokens (`single_use = false`) accept on every
//!   submission without tripping the replay reject.
//! - The vendor-policy lookup is vendor-aware: each Tier 2 vendor
//!   has its own documented default TTL.
//!
//! The `#[ignore]`-gated
//! `invalid_token_reuse_triggers_violation_path` test confirms
//! the wire-level replay path: a single-use nonce is observed
//! twice and the second observation is rejected with
//! `InvalidationKind::NonceReplayed`.
//!
//! Run with:
//!
//! ```sh
//! cargo test -p stygian-charon --test token_lifecycle_integration \
//!     invalid_token_reuse_triggers_violation_path -- --ignored --nocapture
//! ```

use std::time::Duration;

use stygian_charon::token_lifecycle::{
    ChallengeClass, InvalidationKind, TokenContract, TokenPolicyTable, TokenValidator,
    ValidationOutcome,
};
use stygian_charon::vendor_classifier::VendorId;

fn validator() -> TokenValidator {
    TokenValidator::with_defaults(TokenPolicyTable::with_builtin_defaults())
}

fn make_contract(
    vendor: VendorId,
    challenge_class: ChallengeClass,
    ttl: Duration,
    nonce: &str,
    single_use: bool,
    bound_session: Option<&str>,
    issued_at_unix_secs: u64,
) -> TokenContract {
    TokenContract {
        token_id: format!("{}-token", vendor.label()),
        issued_at_unix_secs,
        ttl,
        nonce: nonce.to_string(),
        vendor_family: vendor,
        challenge_class,
        single_use,
        bound_session: bound_session.map(str::to_string),
        description: format!("test {vendor:?} contract"),
    }
}

#[test]
fn cloudflare_interstitial_token_accepted_within_ttl() {
    let v = validator();
    let contract = make_contract(
        VendorId::Cloudflare,
        ChallengeClass::Interstitial,
        Duration::from_mins(20),
        "cf-nonce-1",
        true,
        None,
        0,
    );
    let outcome = v.validate(&contract, None, 5 * 60);
    match outcome {
        ValidationOutcome::Ok {
            effective_ttl,
            consumed,
            ..
        } => {
            assert_eq!(effective_ttl, Duration::from_mins(20));
            assert!(consumed);
        }
        ValidationOutcome::Rejected(err) => panic!("expected accept, got {err:?}"),
    }
}

#[test]
fn datadome_captcha_token_rejected_when_session_binding_misses() {
    let v = validator();
    // DataDome requires session binding per the policy table.
    let contract = make_contract(
        VendorId::DataDome,
        ChallengeClass::Captcha,
        Duration::from_mins(10),
        "dd-nonce-1",
        true,
        Some("session-A"),
        0,
    );
    let outcome = v.validate(&contract, Some("session-B"), 60);
    match outcome {
        ValidationOutcome::Rejected(err) => {
            assert_eq!(err.reason.kind(), InvalidationKind::SessionBindingMiss);
            assert_eq!(err.reason.vendor_family(), VendorId::DataDome);
            assert_eq!(err.reason.challenge_class(), ChallengeClass::Captcha);
            assert_eq!(
                err.reason.label(),
                "session_binding_miss",
                "label must be the stable wire tag"
            );
        }
        ValidationOutcome::Ok { .. } => panic!("expected binding miss"),
    }
}

#[test]
fn perimeterx_integrity_check_ttl_clamped_to_policy_max() {
    let v = validator();
    // PerimeterX default policy: max_ttl = 30 minutes. Submit a
    // contract claiming a 45-minute TTL.
    let contract = make_contract(
        VendorId::PerimeterX,
        ChallengeClass::IntegrityCheck,
        Duration::from_mins(45),
        "px-nonce-1",
        true,
        None,
        0,
    );
    let outcome = v.validate(&contract, None, 60);
    match outcome {
        ValidationOutcome::Ok { effective_ttl, .. } => {
            assert_eq!(effective_ttl, Duration::from_mins(30));
        }
        ValidationOutcome::Rejected(err) => panic!("expected accept after clamp, got {err:?}"),
    }
}

#[test]
fn akamai_proof_of_work_token_rejected_after_ttl() {
    let v = validator();
    let contract = make_contract(
        VendorId::Akamai,
        ChallengeClass::ProofOfWork,
        Duration::from_mins(15),
        "ak-nonce-1",
        true,
        None,
        0,
    );
    // 16 minutes after issuance: well past the 15-minute TTL.
    let outcome = v.validate(&contract, None, 16 * 60);
    match outcome {
        ValidationOutcome::Rejected(err) => {
            assert_eq!(err.reason.kind(), InvalidationKind::Expired);
            assert_eq!(err.reason.vendor_family(), VendorId::Akamai);
            assert_eq!(err.reason.challenge_class(), ChallengeClass::ProofOfWork);
        }
        ValidationOutcome::Ok { .. } => panic!("expected TTL reject"),
    }
}

#[test]
fn fingerprint_com_token_is_multi_use_and_does_not_replay_reject() {
    let v = validator();
    // FingerprintCom default policy: single_use = false.
    let contract = make_contract(
        VendorId::FingerprintCom,
        ChallengeClass::None,
        Duration::from_hours(1),
        "fp-nonce-1",
        false,
        None,
        0,
    );
    let first = v.validate(&contract, None, 60);
    match first {
        ValidationOutcome::Ok { consumed, .. } => assert!(!consumed),
        ValidationOutcome::Rejected(err) => panic!("expected first accept, got {err:?}"),
    }
    let second = v.validate(&contract, None, 60);
    match second {
        ValidationOutcome::Ok { consumed, .. } => assert!(!consumed),
        ValidationOutcome::Rejected(err) => panic!("expected second accept, got {err:?}"),
    }
}

#[test]
fn vendor_aware_policy_lookup_returns_documented_defaults() {
    let v = validator();
    assert_eq!(
        v.policy().policy(VendorId::Cloudflare).default_ttl(),
        Duration::from_mins(30)
    );
    assert_eq!(
        v.policy().policy(VendorId::DataDome).default_ttl(),
        Duration::from_mins(10)
    );
    assert_eq!(
        v.policy().policy(VendorId::Akamai).default_ttl(),
        Duration::from_mins(15)
    );
    assert_eq!(
        v.policy().policy(VendorId::PerimeterX).default_ttl(),
        Duration::from_mins(15)
    );
}

#[test]
fn invalidation_reason_label_is_stable_across_kinds() {
    let v = validator();
    // Force each kind and assert its label.
    let expired = make_contract(
        VendorId::Cloudflare,
        ChallengeClass::Interstitial,
        Duration::from_mins(1),
        "x",
        true,
        None,
        0,
    );
    let expired_outcome = v.validate(&expired, None, 5 * 60);
    assert_eq!(
        expired_outcome.invalidation_kind(),
        Some(InvalidationKind::Expired)
    );

    let mut nonce_missing = make_contract(
        VendorId::Cloudflare,
        ChallengeClass::Interstitial,
        Duration::from_mins(5),
        "placeholder",
        true,
        None,
        0,
    );
    nonce_missing.nonce = String::new();
    let nonce_outcome = v.validate(&nonce_missing, None, 0);
    assert_eq!(
        nonce_outcome.invalidation_kind(),
        Some(InvalidationKind::NonceMismatch)
    );

    let replayed = make_contract(
        VendorId::DataDome,
        ChallengeClass::Captcha,
        Duration::from_mins(10),
        "replay",
        true,
        Some("session"),
        0,
    );
    // First call: session binding pass, accept + consume.
    let _ = v.validate(&replayed, Some("session"), 0);
    let replay_outcome = v.validate(&replayed, Some("session"), 0);
    assert_eq!(
        replay_outcome.invalidation_kind(),
        Some(InvalidationKind::NonceReplayed)
    );

    let not_applicable = make_contract(
        VendorId::Cloudflare,
        ChallengeClass::ProofOfWork,
        Duration::from_mins(5),
        "cf-pow",
        true,
        None,
        0,
    );
    let not_app_outcome = v.validate(&not_applicable, None, 0);
    assert_eq!(
        not_app_outcome.invalidation_kind(),
        Some(InvalidationKind::NotApplicable)
    );
}

#[test]
fn policy_table_override_is_respected() {
    let mut policy = TokenPolicyTable::with_builtin_defaults();
    // Tighten Cloudflare's max TTL to 2 minutes so any contract
    // longer than 2 minutes gets clamped at validation time.
    let tighter = policy
        .policy(VendorId::Cloudflare)
        .with_max_ttl(Duration::from_mins(2));
    policy = policy.with_policy(VendorId::Cloudflare, tighter);
    let v = TokenValidator::with_defaults(policy);

    let contract = make_contract(
        VendorId::Cloudflare,
        ChallengeClass::Interstitial,
        Duration::from_mins(20),
        "cf-tight",
        true,
        None,
        0,
    );
    let outcome = v.validate(&contract, None, 3 * 60);
    // 3 minutes after issuance: well past the 2-minute override.
    match outcome {
        ValidationOutcome::Rejected(err) => {
            assert_eq!(err.reason.kind(), InvalidationKind::Expired);
        }
        ValidationOutcome::Ok { .. } => panic!("expected TTL reject after override"),
    }
}

/// T91 acceptance criterion: invalid token reuse must trigger
/// the violation path. A single-use nonce is observed once,
/// the second observation trips `InvalidationKind::NonceReplayed`
/// with a non-zero observation count.
#[test]
#[ignore = "verifies T91 acceptance criterion: invalid token reuse triggers violation path"]
fn invalid_token_reuse_triggers_violation_path() {
    let v = validator();
    let contract = make_contract(
        VendorId::Cloudflare,
        ChallengeClass::Interstitial,
        Duration::from_mins(30),
        "shared-nonce",
        true,
        None,
        0,
    );

    let first = v.validate(&contract, None, 60);
    match first {
        ValidationOutcome::Ok {
            consumed,
            effective_ttl,
            ..
        } => {
            assert!(consumed, "first observation must mark the nonce consumed");
            assert_eq!(effective_ttl, Duration::from_mins(30));
        }
        ValidationOutcome::Rejected(err) => panic!("first observation must accept: {err:?}"),
    }

    let second = v.validate(&contract, None, 60);
    match second {
        ValidationOutcome::Rejected(err) => {
            assert_eq!(err.reason.kind(), InvalidationKind::NonceReplayed);
            assert_eq!(err.reason.vendor_family(), VendorId::Cloudflare);
            assert_eq!(err.reason.challenge_class(), ChallengeClass::Interstitial);
            match &err.reason {
                stygian_charon::token_lifecycle::InvalidationReason::NonceReplayed {
                    observation_count,
                    ..
                } => {
                    assert!(
                        *observation_count >= 1,
                        "observation_count must reflect the first submission, got {observation_count}"
                    );
                }
                _ => unreachable!("kind matched, variant must be NonceReplayed"),
            }
            assert!(err.message.contains("replayed"));
        }
        ValidationOutcome::Ok { .. } => panic!("second observation must be rejected as replay"),
    }
}
