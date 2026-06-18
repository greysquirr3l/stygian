//! Challenge-token lifecycle contracts (T91).
//!
//! ## What this module does
//!
//! Defines strict lifecycle contracts for **challenge tokens**
//! — short-lived, vendor-issued artefacts (e.g. `cf-chl-bypass`,
//! `_px3`, `_abck`, `datadome=…`) that the scraper must present
//! alongside its request to convince the vendor the challenge has
//! been solved.
//!
//! Each [`TokenContract`] captures four invariants:
//!
//! 1. **Time-to-live**: tokens older than their TTL are rejected
//!    as stale. The TTL is **vendor-aware** (see the
//!    [vendor policy table](#vendor-policy-table) below).
//! 2. **Nonce binding**: the contract carries a per-issuance
//!    nonce. The validator enforces that any subsequent
//!    submission carries the same nonce — a mismatched nonce is
//!    always rejected.
//! 3. **Single-use**: contracts with `single_use = true` may only
//!    be submitted once. Subsequent submissions trip the
//!    [replay-defense][crate::challenge_feedback] path with
//!    [`InvalidationReason::NonceReplayed`].
//! 4. **Vendor family + challenge class**: every contract is
//!    stamped with a [`VendorId`] family (e.g. `Cloudflare`,
//!    `PerimeterX`, `Akamai`, `DataDome`) and a
//!    [`ChallengeClass`] (interstitial, captcha, proof-of-work,
//!    integrity check, cookie refresh, none). The two together
//!    drive **diagnostic routing** — operators can wire
//!    [`InvalidationReason`] events into the per-family audit
//!    log without re-running the classifier.
//!
//! ## Why a contract?
//!
//! A naïve scraper that caches a token for hours and replays it
//! across sessions trains the vendor to escalate its posture
//! (rotating nonces more aggressively, shortening TTLs,
//! eventually locking the scraper out entirely). The contract
//! pins **when** a token may be used, **how often**, and **which**
//! vendor issued it so the policy planner can refresh the token
//! before the vendor invalidates it server-side.
//!
//! ## Nonce bookkeeping (T83 integration)
//!
//! Per-issuance nonces are tracked by a [`NonceBook`] — a
//! capacity-bounded LRU+TTL store that reuses the same
//! [`LruTtlStore`][crate::cache::LruTtlStore] primitive the
//! [`ChallengeMemory`][crate::challenge_feedback::ChallengeMemory]
//! uses (T83). That keeps eviction + expiry semantics consistent
//! across both short-horizon stores and satisfies the
//! "no new cache store" constraint.
//!
//! A [`TokenValidator`] consumes a [`TokenContract`] and a
//! present-time clock, looks up the nonce in the [`NonceBook`],
//! evaluates the four invariants, and returns a
//! [`ValidationOutcome`] the runner can act on. The validator
//! also integrates with the T83 feedback loop by emitting a
//! structured [`InvalidationReason`] (with vendor family +
//! challenge class) so the diagnostic payload can route
//! invalidations to the correct per-family audit log.
//!
//! ## Vendor policy table
//!
//! The defaults are tuned for the Tier 1 vendor catalogue
//! shipped with T89 and the Tier 1 / Tier 2 playbooks shipped
//! with T85:
//!
//! | Vendor family          | Default TTL | Max TTL | Nonce required | Single-use | Session binding |
//! |------------------------|-------------|---------|----------------|------------|-----------------|
//! | [`VendorId::Cloudflare`] | 30 minutes  | 45 minutes | yes       | yes        | optional        |
//! | [`VendorId::Akamai`]     | 15 minutes  | 30 minutes | yes       | yes        | required        |
//! | [`VendorId::DataDome`]   | 10 minutes  | 20 minutes | yes       | yes        | required        |
//! | [`VendorId::PerimeterX`] | 15 minutes  | 30 minutes | yes       | yes        | required        |
//! | [`VendorId::Hcaptcha`]   | 5 minutes   | 10 minutes | yes       | yes        | optional        |
//! | [`VendorId::Recaptcha`]  | 5 minutes   | 10 minutes | yes       | yes        | optional        |
//! | [`VendorId::Kasada`]     | 5 minutes   | 10 minutes | yes       | yes        | required        |
//! | [`VendorId::FingerprintCom`] | 1 hour | 2 hours  | yes           | no         | optional        |
//! | [`VendorId::ShapeSecurity`] | 10 minutes | 20 minutes | yes       | yes        | required        |
//! | [`VendorId::Imperva`]    | 15 minutes  | 30 minutes | yes       | yes        | required        |
//! | [`VendorId::Unknown`]    | 5 minutes   | 10 minutes | yes       | yes        | optional        |
//!
//! Operators can override per-family defaults via
//! [`TokenPolicyTable::with_policy`]; the validator consults the
//! table before applying the contract's own `ttl` field, so an
//! over-long contract is **clamped to `policy.max_ttl`** at
//! validation time.
//!
//! ## Feature flag
//!
//! The module is **default-on** (gated behind the
//! `caching` feature, which is part of the `stygian-charon`
//! default feature set, so the module is always compiled).
//! It is purely additive — no existing public type gains a new
//! field, no existing behaviour changes, and no new feature gate
//! is introduced. Operators who want the strict lifecycle
//! validation call [`TokenValidator::validate`] on every
//! submission; callers that ignore it see no behaviour change.
//!
//! # Example
//!
//! ```
//! use std::time::Duration;
//! use stygian_charon::token_lifecycle::{
//!     ChallengeClass, TokenContract, TokenPolicyTable, TokenValidator,
//!     ValidationOutcome,
//! };
//! use stygian_charon::vendor_classifier::VendorId;
//!
//! // Build a policy table seeded with the per-vendor defaults
//! // and a 256-entry nonce book with a 10-minute TTL.
//! let policy = TokenPolicyTable::with_builtin_defaults();
//! let validator = TokenValidator::with_defaults(policy);
//!
//! // A Cloudflare interstitial token issued 1 minute ago.
//! let contract = TokenContract {
//!     token_id: "cf-chl-bypass-abc".to_string(),
//!     issued_at_unix_secs: 1_700_000_000,
//!     ttl: Duration::from_mins(30),
//!     nonce: "nonce-xyz".to_string(),
//!     vendor_family: VendorId::Cloudflare,
//!     challenge_class: ChallengeClass::Interstitial,
//!     single_use: true,
//!     bound_session: None,
//!     description: "Cloudflare turnstile bypass token".to_string(),
//! };
//!
//! // First submission passes: the nonce has not been seen.
//! let outcome = validator.validate(&contract, Some("session-1"), 1_700_000_060);
//! assert!(matches!(outcome, ValidationOutcome::Ok { .. }));
//! ```

mod contract;
mod error;
mod invalidation;
mod nonce;
mod policy;
mod validator;

pub use contract::{ChallengeClass, TokenContract};
pub use error::TokenLifecycleError;
pub use invalidation::{InvalidationKind, InvalidationReason};
pub use nonce::{
    DEFAULT_NONCE_BOOK_CAPACITY, DEFAULT_NONCE_TTL, NonceBook, NonceObservation, nonce_book_key,
};
pub use policy::{TokenPolicy, TokenPolicyTable, builtin_token_policies};
pub use validator::{TokenValidator, ValidationOutcome};
