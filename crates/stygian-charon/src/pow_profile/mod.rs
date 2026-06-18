//! Proof-of-work capability profile (T93).
//!
//! ## What this module does
//!
//! Quantifies the scraper's **proof-of-work (PoW) handling
//! capability** for a `(domain, target_class, vendor_family)`
//! triple and feeds the resulting score into the runtime
//! policy. PoW challenges are the vendor-issued JS / WASM
//! computations a scraper must solve to pass an
//! anti-bot gate (e.g. `Akamai` `_abck` derivation,
//! `Fingerprint.com` proof-of-work, `DataDome` interstitial
//! challenge). Naïve scrapers that always try the same
//! solve strategy train the vendor to escalate the
//! challenge, eventually locking the scraper out.
//!
//! A [`PowCapabilityProfile`] aggregates solve latency,
//! success rate, retry count, and failure modes into a
//! stable, serialisable record. A
//! [`PowCapabilityScorer`] consumes the profile and
//! produces a deterministic unit-interval score plus a
//! coarse [`PowCapabilityBand`] label. The policy mapper
//! ([`adjust_runtime_policy_for_pow`]) then nudges the
//! runtime policy toward a posture that matches the
//! observed capability (faster pacing for `Strong`,
//! browser+sticky escalation for `Weak`).
//!
//! ## Schema overview
//!
//! | Field                     | Range / type                          | Source |
//! |---------------------------|---------------------------------------|--------|
//! | `solved_count`            | `u32`                                 | count of solved samples |
//! | `failed_count`            | `u32`                                 | count of failed samples |
//! | `retry_count`             | `u32` (cumulative)                    | sum of sample retries |
//! | `solve_latency_ms_p50`    | `Option<u64>`                         | running median of solved samples |
//! | `solve_latency_ms_p95`    | `Option<u64>`                         | running tail of solved samples |
//! | `failure_modes`           | `BTreeMap<PowFailureMode, u32>`       | histogram of failure modes |
//! | `observation_window_secs`  | `u64`                                 | width of the sampling window |
//! | `recorded_at_unix_secs`   | `u64`                                 | wall-clock timestamp of last merge |
//!
//! ## Sampling window defaults
//!
//! The default sampling window is
//! [`DEFAULT_SAMPLE_WINDOW_SECS`] (one hour). The default
//! store TTL ([`DEFAULT_POW_TTL`]) matches the default
//! window so a profile that was built over the default
//! window expires exactly when the window elapses.
//! Operators can override the window by calling
//! [`PowCapabilityProfile::merge`] with a sample after
//! adjusting `observation_window_secs` on the profile, or
//! by calling [`PowCapabilityStore::new`] with a custom
//! TTL.
//!
//! ## Sparse-telemetry fallback
//!
//! When the profile's `total_attempts` is below
//! [`MIN_OBSERVATIONS_FOR_SCORING`] (3) the scorer returns
//! [`SPARSE_FALLBACK_SCORE`] (`0.5`) and the band is
//! [`PowCapabilityBand::Unknown`]. The fallback is the
//! **same** value the empty profile returns, so the policy
//! mapper treats unobserved targets as the no-op baseline
//! (no escalation, no risk-score lift). This is the
//! "I have no signal" default — the operator's policy is
//! not perturbed by a profile that has not earned
//! statistical confidence.
//!
//! ## Persistence
//!
//! The persistence layer reuses the same
//! [`LruTtlStore`][crate::cache::LruTtlStore] primitive
//! the T83 [`ChallengeMemory`][crate::challenge_feedback::ChallengeMemory]
//! and the T91 [`NonceBook`][crate::token_lifecycle::NonceBook]
//! use. That keeps eviction + expiry semantics consistent
//! across all three short-horizon stores and satisfies the
//! "no new cache store" requirement. The key namespace is
//! `charon:pow:...` (see [`pow_profile_key`]) so PoW
//! entries never collide with `charon:challenge:...` (T83)
//! or `charon:token_nonce:...` (T91) on a shared backing
//! primitive.
//!
//! ## Feature flag
//!
//! The module is **default-on** (gated behind the
//! `caching` feature, which is part of the `stygian-charon`
//! default feature set). It is purely additive — no
//! existing public type gains a new field, no existing
//! behaviour changes, and no new feature gate is
//! introduced. The schema is serialised as a flat record
//! with additive `Option<T>` fields
//! (`#[serde(default, skip_serializing_if = "Option::is_none")]`
//! on `solve_latency_ms_p50` and `solve_latency_ms_p95`)
//! so older JSON payloads still deserialize and newer
//! payloads omit the optional fields when no latency has
//! been observed yet.
//!
//! # Example
//!
//! ```
//! use stygian_charon::pow_profile::{
//!     PowCapabilityProfile, PowCapabilitySample, PowCapabilityScorer,
//!     PowCapabilityStore, adjust_runtime_policy_for_pow, PowPolicyThresholds,
//!     PowCapabilityScore,
//! };
//! use stygian_charon::types::{ExecutionMode, RuntimePolicy, SessionMode, TargetClass, TelemetryLevel};
//! use stygian_charon::vendor_classifier::VendorId;
//! use std::collections::BTreeMap;
//!
//! // Record a few samples into a store.
//! let store = PowCapabilityStore::with_defaults();
//! for _ in 0..6 {
//!     store.record_sample(
//!         "example.com",
//!         TargetClass::ContentSite,
//!         VendorId::Cloudflare,
//!         &PowCapabilitySample::solved(800, 0),
//!     );
//! }
//!
//! // Look up the aggregated profile and score it.
//! let profile = store
//!     .lookup("example.com", TargetClass::ContentSite, VendorId::Cloudflare)
//!     .expect("profile");
//! let scorer = PowCapabilityScorer::new();
//! let value = scorer.score(&profile);
//! let score = PowCapabilityScore::new(value);
//!
//! // Apply the policy mapper.
//! let policy = RuntimePolicy {
//!     execution_mode: ExecutionMode::Http,
//!     session_mode: SessionMode::Stateless,
//!     telemetry_level: TelemetryLevel::Standard,
//!     rate_limit_rps: 3.0,
//!     max_retries: 2,
//!     backoff_base_ms: 250,
//!     enable_warmup: false,
//!     enforce_webrtc_proxy_only: false,
//!     sticky_session_ttl_secs: None,
//!     required_stygian_features: Vec::new(),
//!     config_hints: BTreeMap::new(),
//!     risk_score: 0.30,
//! };
//! let adjusted =
//!     adjust_runtime_policy_for_pow(&policy, &score, &PowPolicyThresholds::default());
//! assert!(adjusted.rate_limit_rps >= 1.0);
//! assert!(adjusted
//!     .config_hints
//!     .contains_key("pow.capability"));
//! ```

mod policy;
mod profile;
mod scorer;
mod store;

pub use policy::{
    MAX_POW_RISK_DELTA, PowCapabilityScore, PowPolicyThresholds,
    adjust_runtime_policy_for_pow,
};
pub use profile::{
    DEFAULT_SAMPLE_WINDOW_SECS, PowCapabilityProfile, PowCapabilitySample, PowFailureMode,
};
pub use scorer::{
    DEFAULT_LATENCY_BUDGET_MS, DEFAULT_RETRY_BUDGET, MIN_OBSERVATIONS_FOR_SCORING,
    ProfileWeights, PowCapabilityBand, PowCapabilityScorer, SPARSE_FALLBACK_SCORE,
    band_for_score,
};
pub use store::{
    DEFAULT_POW_CAPACITY, DEFAULT_POW_TTL, PowCapabilityStore, pow_profile_key,
};

/// Convenience helper: score a profile and wrap the result
/// in a [`PowCapabilityScore`].
///
/// This is the "operator-friendly" path — most callers want
/// "give me a score I can pass to the policy mapper" and
/// do not want to assemble the [`PowCapabilityScore`]
/// manually.
#[must_use]
pub fn score_from_profile(
    profile: &PowCapabilityProfile,
    scorer: &PowCapabilityScorer,
) -> PowCapabilityScore {
    PowCapabilityScore::new(scorer.score(profile))
}
