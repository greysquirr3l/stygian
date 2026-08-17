//! Challenge-aware policy feedback loop (T83, T110).
//!
//! ## What this module does
//!
//! Captures the **challenge outcome** of an acquisition attempt and
//! feeds it back into the next policy planning cycle. Anti-bot vendors
//! escalate their posture as they observe more challenges (more
//! captchas, harder JS proofs, longer interstitials). A naïve
//! scraper that replays the same strategy over and over teaches the
//! vendor to escalate, eventually locking the scraper out.
//!
//! [`ChallengeMemory`] keeps a short-horizon record of the **last
//! observed outcome** per [`EngineKey`] with a TTL and a
//! max-entries cap (LRU eviction). [`adjust_runtime_policy`] (and
//! [`build_runtime_policy_with_memory`]) consume the memory to
//! nudge the risk score up (when the last outcome was adversarial)
//! or down (when the last outcome was a clean pass).
//!
//! ## Why engine-keyed memory? (T110)
//!
//! The primary key of [`ChallengeMemory`] is the **engine** —
//! `(engine, version, target_class, tls_profile)` — **not** the
//! URL. A self-healing patch recorded against one URL on one
//! engine heals every URL on that engine: a captcha workaround
//! learned on `example.com/cloudflare/page1` is immediately
//! applied to `example.com/cloudflare/page2` and to every other
//! Cloudflare-fronted URL the runner sees. The URL is kept on each
//! entry only as a secondary debugging index (see
//! [`ChallengeMemoryEntry::last_observed_url`]) — it is **not** a
//! primary key. The principle being encoded is the platform-keyed
//! memory instinct from the source guide: *"the durable asset is
//! not the record you extracted, it is the route you learned to
//! it."*
//!
//! All four fields of [`EngineKey`] participate in the
//! equivalence, hash, and ordering so re-keying a vendor version
//! (`bot-manager-v3` → `bot-manager-v4`) or changing the TLS
//! profile (`chrome136` → `firefox130`) deliberately produces a
//! fresh memory slot. The four guard tests in
//! [`ChallengeMemory`] exercise this property:
//!
//! - `same_engine_different_url_propagates_patch`
//! - `same_engine_different_target_class_keeps_separate_memory`
//! - `same_engine_different_tls_profile_keeps_separate_memory`
//! - `engine_key_round_trips_through_display_fromstr_and_serde`
//!
//! ## Why a clamp?
//!
//! Influence bounds are **critical** for this module. A feedback
//! loop that can shift the risk score arbitrarily would amplify
//! noise: a single transient captcha would cascade into a full
//! browser-stealth escalation that the site is not actually
//! demanding. To prevent runaway strategy escalation, every per-key
//! adjustment is **clamped to** [`MAX_RISK_DELTA`] (a documented,
//! conservative `0.20` ceiling) and the final risk score is
//! re-clamped to `[0.0, 1.0]` after the adjustment. Callers can
//! tighten the clamp with [`ChallengeFeedbackPolicy::with_max_delta`]
//! but cannot raise it above [`MAX_RISK_DELTA`].
//!
//! ## Backing store
//!
//! The LRU+TTL store is **shared** with the existing investigation
//! report cache
//! ([`crate::cache::MemoryInvestigationCache`]). It is exposed here
//! as the crate-private `LruTtlStore`
//! helper so the challenge memory and the investigation cache
//! share eviction + expiry semantics and we do not introduce a
//! parallel "second cache store" with its own semantics.
//!
//! ## Feature flag
//!
//! The module is **default-on** (the `caching` feature is now part
//! of `stygian-charon`'s default feature set, so the
//! `LruTtlStore` is always available). No new feature gate is
//! introduced.
//!
//! # Example
//!
//! ```
//! use stygian_charon::challenge_feedback::{
//!     ChallengeMemory, ChallengeOutcome, EngineKey, adjust_runtime_policy, MAX_RISK_DELTA,
//! };
//! use stygian_charon::types::{
//!     ExecutionMode, RuntimePolicy, SessionMode, TargetClass, TelemetryLevel,
//! };
//! use stygian_charon::vendor_classifier::VendorId;
//! use std::collections::BTreeMap;
//! use std::num::NonZeroUsize;
//!
//! let memory = ChallengeMemory::with_default_ttl(NonZeroUsize::new(64).expect("non-zero"));
//! let key = EngineKey {
//!     engine: VendorId::Cloudflare,
//!     version: None,
//!     target_class: TargetClass::ContentSite,
//!     tls_profile: None,
//! };
//! memory.record(&key, Some("https://example.com/a"), ChallengeOutcome::Captcha);
//!
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
//!     risk_score: 0.20,
//! };
//!
//! let adjusted = adjust_runtime_policy(&policy, &memory, &key);
//! assert!(adjusted.risk_score >= policy.risk_score);
//! assert!(adjusted.risk_score <= policy.risk_score + MAX_RISK_DELTA);
//! ```

mod key;
mod memory;
mod outcome;
mod policy;

pub use key::{EngineKey, EngineKeyParseError};
pub use memory::{
    ChallengeMemory, ChallengeMemoryEntry, DEFAULT_CHALLENGE_CAPACITY, DEFAULT_CHALLENGE_TTL,
    engine_memory_key,
};
pub use outcome::ChallengeOutcome;
pub use policy::{
    ChallengeFeedbackPolicy, MAX_RISK_DELTA, adjust_runtime_policy,
    build_runtime_policy_with_memory, memory_adjustment_for,
};
