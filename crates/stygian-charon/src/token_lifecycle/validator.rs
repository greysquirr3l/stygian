//! Token validator (T91).
//!
//! The [`TokenValidator`] consumes a [`TokenContract`] and a
//! present-time clock, evaluates the four lifecycle invariants
//! (TTL, single-use, nonce match, session binding), and returns
//! a [`ValidationOutcome`] the runner can act on. The
//! validator integrates with the
//! [`ChallengeMemory`][crate::challenge_feedback::ChallengeMemory]
//! indirectly — nonce bookkeeping lives in
//! [`NonceBook`][crate::token_lifecycle::NonceBook], a separate
//! store that reuses the same LRU+TTL primitive the T83
//! challenge memory uses (so eviction + expiry semantics stay
//! consistent across both short-horizon stores).
//!
//! ## Validation flow
//!
//! 1. **TTL clamp**: if the contract's `ttl` exceeds the
//!    per-vendor `max_ttl`, the validator clamps the effective
//!    TTL to the per-vendor ceiling. This stops a contract
//!    factory from accidentally issuing overly-long-lived
//!    tokens.
//! 2. **TTL check**: the validator computes the token's age at
//!    `now_unix_secs` and rejects tokens whose age is at or
//!    beyond the effective TTL. The reason carries the
//!    observed age and the TTL the contract was issued with.
//! 3. **Nonce presence check**: if the vendor policy requires
//!    nonces (or the challenge class requires nonces), the
//!    validator rejects empty nonces with
//!    [`InvalidationKind::NonceMismatch`][crate::token_lifecycle::InvalidationKind::NonceMismatch].
//! 4. **Single-use / replay check**: if the nonce is already
//!    present in the [`NonceBook`] with an observation count
//!    `>= 1`, the validator rejects the submission with
//!    [`InvalidationKind::NonceReplayed`][crate::token_lifecycle::InvalidationKind::NonceReplayed].
//!    Multi-use tokens (`single_use = false`) bypass the
//!    replay check.
//! 5. **Session binding check**: if the vendor policy or
//!    challenge class requires session binding, the validator
//!    rejects submissions whose `session_id` does not match
//!    the contract's `bound_session`.
//! 6. **Not-applicable check**: if the challenge class is not
//!    applicable for the vendor family (e.g. Cloudflare does
//!    not issue `proof-of-work` tokens), the validator rejects
//!    with
//!    [`InvalidationKind::NotApplicable`][crate::token_lifecycle::InvalidationKind::NotApplicable].
//! 7. **Accept + mark consumed**: the validator records the
//!    nonce in the [`NonceBook`] (so the next submission is
//!    rejected with `NonceReplayed`) and returns
//!    [`ValidationOutcome::Ok`].

use std::time::Duration;

use crate::token_lifecycle::contract::{ChallengeClass, TokenContract};
use crate::token_lifecycle::error::TokenLifecycleError;
use crate::token_lifecycle::invalidation::{InvalidationKind, InvalidationReason};
use crate::token_lifecycle::nonce::NonceBook;
use crate::token_lifecycle::policy::TokenPolicyTable;
use crate::vendor_classifier::VendorId;

/// Outcome of a [`TokenValidator::validate`] call.
///
/// The validator always returns a [`ValidationOutcome`] — never
/// a `Result` — so the caller can branch on the outcome
/// without unwrapping. The error path embeds the structured
/// [`InvalidationReason`] for diagnostic routing.
///
/// # Example
///
/// ```
/// use stygian_charon::token_lifecycle::{
///     ChallengeClass, TokenContract, TokenPolicyTable, TokenValidator,
///     ValidationOutcome,
/// };
/// use stygian_charon::vendor_classifier::VendorId;
/// use std::time::Duration;
///
/// let validator = TokenValidator::with_defaults(TokenPolicyTable::with_builtin_defaults());
/// let contract = TokenContract {
///     token_id: "x".to_string(),
///     issued_at_unix_secs: 0,
///     ttl: Duration::from_mins(5),
///     nonce: "n".to_string(),
///     vendor_family: VendorId::Cloudflare,
///     challenge_class: ChallengeClass::Interstitial,
///     single_use: true,
///     bound_session: None,
///     description: String::new(),
/// };
/// let outcome = validator.validate(&contract, None, 60);
/// assert!(matches!(outcome, ValidationOutcome::Ok { .. }));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationOutcome {
    /// Validator accepted the submission.
    Ok {
        /// Contract that was accepted (with the TTL the
        /// validator actually applied — possibly clamped to
        /// the per-vendor `max_ttl`).
        contract: TokenContract,
        /// `true` when the nonce was newly observed by this
        /// submission (i.e. the validator marked it consumed).
        /// `false` for multi-use tokens whose nonce was already
        /// in the [`NonceBook`].
        consumed: bool,
        /// Effective TTL the validator applied after clamping.
        effective_ttl: Duration,
    },
    /// Validator rejected the submission. The error carries
    /// the structured [`InvalidationReason`] and a
    /// human-readable message suitable for operator logs.
    Rejected(TokenLifecycleError),
}

impl ValidationOutcome {
    /// `true` when the validator accepted the submission.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::{ValidationOutcome};
    ///
    /// let outcome = ValidationOutcome::Ok {
    ///     contract: stygian_charon::token_lifecycle::TokenContract {
    ///         token_id: "x".to_string(),
    ///         issued_at_unix_secs: 0,
    ///         ttl: std::time::Duration::from_mins(1),
    ///         nonce: "n".to_string(),
    ///         vendor_family: stygian_charon::vendor_classifier::VendorId::Unknown,
    ///         challenge_class: stygian_charon::token_lifecycle::ChallengeClass::None,
    ///         single_use: false,
    ///         bound_session: None,
    ///         description: String::new(),
    ///     },
    ///     consumed: true,
    ///     effective_ttl: std::time::Duration::from_mins(1),
    /// };
    /// assert!(outcome.is_ok());
    /// ```
    #[must_use]
    pub const fn is_ok(&self) -> bool {
        matches!(self, Self::Ok { .. })
    }

    /// `true` when the validator rejected the submission.
    #[must_use]
    pub const fn is_rejected(&self) -> bool {
        matches!(self, Self::Rejected(_))
    }

    /// Borrow the underlying error when [`Rejected`][Self::Rejected].
    #[must_use]
    pub const fn error(&self) -> Option<&TokenLifecycleError> {
        match self {
            Self::Rejected(err) => Some(err),
            Self::Ok { .. } => None,
        }
    }

    /// Invalidation kind when [`Rejected`][Self::Rejected].
    #[must_use]
    pub fn invalidation_kind(&self) -> Option<InvalidationKind> {
        self.error().map(|e| e.reason.kind())
    }
}

/// Token validator.
///
/// Holds the [`NonceBook`] (LRU+TTL nonce store) and the
/// [`TokenPolicyTable`] (per-vendor policy lookup). The
/// validator is `Send + Sync` so it can sit behind an `Arc`
/// and be shared across threads and requests without locking.
///
/// # Example
///
/// ```
/// use std::time::Duration;
/// use stygian_charon::token_lifecycle::{
///     ChallengeClass, TokenContract, TokenPolicyTable, TokenValidator,
/// };
/// use stygian_charon::vendor_classifier::VendorId;
///
/// let policy = TokenPolicyTable::with_builtin_defaults();
/// let validator = TokenValidator::new(
///     stygian_charon::token_lifecycle::NonceBook::with_defaults(),
///     policy,
/// );
/// let contract = TokenContract {
///     token_id: "x".to_string(),
///     issued_at_unix_secs: 0,
///     ttl: Duration::from_mins(5),
///     nonce: "n".to_string(),
///     vendor_family: VendorId::Unknown,
///     challenge_class: ChallengeClass::None,
///     single_use: false,
///     bound_session: None,
///     description: String::new(),
/// };
/// let outcome = validator.validate(&contract, None, 0);
/// assert!(outcome.is_ok());
/// ```
pub struct TokenValidator {
    nonce_book: NonceBook,
    policy: TokenPolicyTable,
}

impl std::fmt::Debug for TokenValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenValidator")
            .field("nonce_book", &self.nonce_book)
            .field("policy_vendors", &self.policy.len())
            .finish()
    }
}

impl TokenValidator {
    /// Build a validator with an explicit nonce book and
    /// policy table.
    ///
    /// # Example
    ///
    /// ```
    /// use std::num::NonZeroUsize;
    /// use std::time::Duration;
    /// use stygian_charon::token_lifecycle::{NonceBook, TokenPolicyTable, TokenValidator};
    ///
    /// let validator = TokenValidator::new(
    ///     NonceBook::new(NonZeroUsize::new(8).expect("non-zero"), Duration::from_mins(1)),
    ///     TokenPolicyTable::with_builtin_defaults(),
    /// );
    /// assert_eq!(validator.policy().len(), 11);
    /// ```
    #[must_use]
    pub const fn new(nonce_book: NonceBook, policy: TokenPolicyTable) -> Self {
        Self { nonce_book, policy }
    }

    /// Build a validator with the default
    /// [`NonceBook::with_defaults()`][crate::token_lifecycle::NonceBook::with_defaults]
    /// nonce book and the supplied policy table.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::{TokenPolicyTable, TokenValidator};
    ///
    /// let validator = TokenValidator::with_defaults(TokenPolicyTable::with_builtin_defaults());
    /// assert!(validator.nonce_book().is_empty());
    /// ```
    #[must_use]
    pub fn with_defaults(policy: TokenPolicyTable) -> Self {
        Self::new(NonceBook::with_defaults(), policy)
    }

    /// Borrow the nonce book.
    #[must_use]
    pub const fn nonce_book(&self) -> &NonceBook {
        &self.nonce_book
    }

    /// Borrow the policy table.
    #[must_use]
    pub const fn policy(&self) -> &TokenPolicyTable {
        &self.policy
    }

    /// Validate a [`TokenContract`] against the supplied
    /// `session_id` (when the submission carries one) and the
    /// supplied `now_unix_secs` clock.
    ///
    /// On accept, the nonce is recorded in the [`NonceBook`]
    /// (so the next submission is rejected as a replay) and the
    /// outcome's `consumed` flag is `true`. On reject, the
    /// outcome is [`ValidationOutcome::Rejected`] with the
    /// structured [`InvalidationReason`] the runner can route
    /// into the per-family audit log.
    #[allow(clippy::too_many_lines)]
    pub fn validate(
        &self,
        contract: &TokenContract,
        session_id: Option<&str>,
        now_unix_secs: u64,
    ) -> ValidationOutcome {
        let vendor_policy = self.policy.policy(contract.vendor_family);

        // 1. Not-applicable check.
        if let Some(reason) = Self::check_not_applicable(contract) {
            return ValidationOutcome::Rejected(reason);
        }

        // 2. TTL clamp.
        let effective_ttl = if contract.ttl > vendor_policy.max_ttl() {
            vendor_policy.max_ttl()
        } else {
            contract.ttl
        };
        let age = contract.age_secs(now_unix_secs);

        // 3. TTL check.
        if age >= effective_ttl.as_secs() {
            return ValidationOutcome::Rejected(TokenLifecycleError::new(
                InvalidationReason::Expired {
                    vendor: contract.vendor_family,
                    challenge_class: contract.challenge_class,
                    age_secs: age,
                    ttl_secs: effective_ttl.as_secs(),
                },
                format!(
                    "{:?} token '{}' expired (age {}s >= ttl {}s)",
                    contract.vendor_family,
                    contract.token_id,
                    age,
                    effective_ttl.as_secs()
                ),
            ));
        }

        // 4. Nonce presence check.
        let nonce_required =
            vendor_policy.require_nonce() || contract.challenge_class.requires_nonce();
        if nonce_required && contract.nonce.is_empty() {
            return ValidationOutcome::Rejected(TokenLifecycleError::new(
                InvalidationReason::NonceMismatch {
                    vendor: contract.vendor_family,
                    challenge_class: contract.challenge_class,
                    expected: "<required>".to_string(),
                    observed: String::new(),
                },
                format!(
                    "{:?} token '{}' missing nonce",
                    contract.vendor_family, contract.token_id
                ),
            ));
        }

        // 5. Single-use / replay check.
        let effective_single_use = contract.single_use || vendor_policy.single_use();
        if effective_single_use
            && !contract.nonce.is_empty()
            && let Some(observation) = self
                .nonce_book
                .lookup(contract.vendor_family, &contract.nonce)
        {
            return ValidationOutcome::Rejected(TokenLifecycleError::new(
                InvalidationReason::NonceReplayed {
                    vendor: contract.vendor_family,
                    challenge_class: contract.challenge_class,
                    observation_count: observation.observation_count,
                },
                format!(
                    "{:?} token nonce '{}' replayed ({} observations)",
                    contract.vendor_family, contract.nonce, observation.observation_count
                ),
            ));
        }

        // 6. Session binding check.
        let binding_required = vendor_policy.require_session_binding()
            || contract.challenge_class.requires_session_binding();
        if binding_required {
            let expected = contract.bound_session.as_deref();
            let observed = session_id;
            let miss = match (expected, observed) {
                (Some(e), Some(o)) => e != o,
                (Some(_), None) => true,
                (None, _) => false,
            };
            if miss {
                return ValidationOutcome::Rejected(TokenLifecycleError::new(
                    InvalidationReason::SessionBindingMiss {
                        vendor: contract.vendor_family,
                        challenge_class: contract.challenge_class,
                        expected: contract.bound_session.clone(),
                        observed: session_id.map(str::to_string),
                    },
                    format!(
                        "{:?} token '{}' session binding miss (expected {:?}, observed {:?})",
                        contract.vendor_family,
                        contract.token_id,
                        contract.bound_session,
                        session_id
                    ),
                ));
            }
        }

        // 7. Accept + mark consumed.
        if !contract.nonce.is_empty() {
            self.nonce_book.record(
                contract.vendor_family,
                contract.challenge_class,
                &contract.nonce,
            );
        }

        let mut accepted = contract.clone();
        accepted.ttl = effective_ttl;
        ValidationOutcome::Ok {
            contract: accepted,
            consumed: effective_single_use && !contract.nonce.is_empty(),
            effective_ttl,
        }
    }

    /// `true` when the supplied `(vendor, challenge_class)`
    /// pair is not a valid combination (e.g. Cloudflare does
    /// not issue `PoW` tokens; `FingerprintCom` does not issue
    /// captcha tokens).
    fn check_not_applicable(contract: &TokenContract) -> Option<TokenLifecycleError> {
        let applicable = is_applicable(contract.vendor_family, contract.challenge_class);
        if applicable {
            None
        } else {
            Some(TokenLifecycleError::new(
                InvalidationReason::NotApplicable {
                    vendor: contract.vendor_family,
                    challenge_class: contract.challenge_class,
                },
                format!(
                    "{:?} does not issue {:?} tokens",
                    contract.vendor_family, contract.challenge_class
                ),
            ))
        }
    }
}

/// `true` when the supplied `(vendor, challenge_class)` pair is
/// a valid combination. The matrix mirrors the
/// [vendor policy table][crate::token_lifecycle#vendor-policy-table]:
///
/// - **Tier 2 vendors** (`DataDome`, `PerimeterX`, `Akamai`,
///   `Imperva`, `ShapeSecurity`, `Kasada`) issue **every**
///   challenge class except `None` (the cookie class).
/// - **Tier 1 captcha providers** (`Hcaptcha`, `Recaptcha`)
///   only issue [`ChallengeClass::Captcha`] and
///   [`ChallengeClass::CookieRefresh`].
/// - **`FingerprintCom`** only issues
///   [`ChallengeClass::None`] (identification cookies) and
///   [`ChallengeClass::CookieRefresh`].
/// - **`Cloudflare`** issues [`ChallengeClass::Interstitial`]
///   and [`ChallengeClass::Captcha`] plus
///   [`ChallengeClass::None`] / `CookieRefresh`.
/// - **`Unknown`** only issues the safe defaults
///   (`None`, `CookieRefresh`, `Unknown`).
const fn is_applicable(vendor: VendorId, challenge_class: ChallengeClass) -> bool {
    use ChallengeClass as C;
    use VendorId as V;
    match vendor {
        V::DataDome | V::PerimeterX | V::Akamai | V::Imperva | V::ShapeSecurity | V::Kasada => {
            matches!(
                challenge_class,
                C::Interstitial
                    | C::Captcha
                    | C::ProofOfWork
                    | C::IntegrityCheck
                    | C::CookieRefresh
                    | C::Unknown
            )
        }
        V::Hcaptcha | V::Recaptcha => {
            matches!(challenge_class, C::Captcha | C::CookieRefresh | C::Unknown)
        }
        V::FingerprintCom => matches!(challenge_class, C::None | C::CookieRefresh | C::Unknown),
        V::Cloudflare => matches!(
            challenge_class,
            C::None | C::Interstitial | C::CookieRefresh | C::Unknown
        ),
        V::Unknown => matches!(challenge_class, C::None | C::CookieRefresh | C::Unknown),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn approx_eq(a: Duration, b: Duration) -> bool {
        a == b
    }

    fn validator() -> TokenValidator {
        TokenValidator::with_defaults(TokenPolicyTable::with_builtin_defaults())
    }

    fn contract(
        vendor: VendorId,
        class: ChallengeClass,
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
            challenge_class: class,
            single_use,
            bound_session: bound_session.map(str::to_string),
            description: String::new(),
        }
    }

    #[test]
    fn accepts_fresh_cloudflare_interstitial() {
        let v = validator();
        // codeql[rust/hard-coded-cryptographic-value] false-positive: deterministic test label
        let c = contract(
            VendorId::Cloudflare,
            ChallengeClass::Interstitial,
            Duration::from_mins(30),
            "nonce-1",
            true,
            None,
            0,
        );
        let outcome = v.validate(&c, None, 60);
        match outcome {
            ValidationOutcome::Ok { consumed, .. } => assert!(consumed),
            ValidationOutcome::Rejected(err) => panic!("unexpected reject: {err:?}"),
        }
    }

    #[test]
    fn rejects_expired_cloudflare_interstitial() {
        let v = validator();
        // codeql[rust/hard-coded-cryptographic-value] false-positive: deterministic test label
        let c = contract(
            VendorId::Cloudflare,
            ChallengeClass::Interstitial,
            Duration::from_mins(30),
            "nonce-2",
            true,
            None,
            0,
        );
        // Submitted 31 minutes after issuance (Cloudflare max ttl = 45 minutes).
        let outcome = v.validate(&c, None, 31 * 60);
        match outcome {
            ValidationOutcome::Rejected(err) => {
                assert_eq!(
                    err.reason.kind(),
                    InvalidationKind::Expired,
                    "expected Expired, got {:?}",
                    err.reason
                );
                assert_eq!(err.reason.vendor_family(), VendorId::Cloudflare);
            }
            ValidationOutcome::Ok { .. } => panic!("expected reject"),
        }
    }

    #[test]
    fn rejects_single_use_replay() {
        let v = validator();
        // codeql[rust/hard-coded-cryptographic-value] false-positive: deterministic test label
        let c = contract(
            VendorId::DataDome,
            ChallengeClass::Captcha,
            Duration::from_mins(10),
            "replay-n",
            true,
            None,
            0,
        );
        let first = v.validate(&c, None, 60);
        assert!(matches!(first, ValidationOutcome::Ok { .. }));
        let second = v.validate(&c, None, 60);
        match second {
            ValidationOutcome::Rejected(err) => {
                assert_eq!(err.reason.kind(), InvalidationKind::NonceReplayed);
                if let InvalidationReason::NonceReplayed {
                    observation_count, ..
                } = &err.reason
                {
                    assert!(*observation_count >= 1);
                } else {
                    panic!("unexpected variant");
                }
            }
            ValidationOutcome::Ok { .. } => panic!("expected replay reject"),
        }
    }

    #[test]
    fn rejects_nonce_mismatch() {
        // A multi-use token that *was* issued with nonce A but
        // is being submitted with nonce B. The validator
        // records nonces per-vendor, so nonce B is fresh
        // (first observation) — but the contract's `nonce`
        // field says A. We do not currently reject a fresh
        // nonce against an older contract's nonce (the runner
        // is responsible for surfacing that the contract itself
        // has changed). However, we *do* reject a missing nonce
        // when the policy requires one.
        let v = validator();
        // codeql[rust/hard-coded-cryptographic-value] false-positive: deterministic test label
        let mut c = contract(
            VendorId::Cloudflare,
            ChallengeClass::Interstitial,
            Duration::from_mins(30),
            "nonce-A",
            true,
            None,
            0,
        );
        c.nonce = String::new();
        let outcome = v.validate(&c, None, 60);
        match outcome {
            ValidationOutcome::Rejected(err) => {
                assert_eq!(err.reason.kind(), InvalidationKind::NonceMismatch);
                assert_eq!(err.reason.vendor_family(), VendorId::Cloudflare);
                assert_eq!(err.reason.challenge_class(), ChallengeClass::Interstitial);
            }
            ValidationOutcome::Ok { .. } => panic!("expected missing-nonce reject"),
        }
    }

    #[test]
    fn rejects_session_binding_miss_when_required() {
        // Akamai requires session binding.
        let v = validator();
        // codeql[rust/hard-coded-cryptographic-value] false-positive: deterministic test label
        let c = contract(
            VendorId::Akamai,
            ChallengeClass::ProofOfWork,
            Duration::from_mins(15),
            "akamai-n",
            true,
            Some("session-A"),
            0,
        );
        let miss = v.validate(&c, Some("session-B"), 60);
        assert!(matches!(
            miss,
            ValidationOutcome::Rejected(ref err) if err.reason.kind() == InvalidationKind::SessionBindingMiss
        ));
        let hit = v.validate(&c, Some("session-A"), 60);
        // Wait, session-A nonce is already consumed by the
        // first attempt? No — the first attempt was rejected
        // (session binding miss), so the nonce was NOT
        // recorded. The second attempt with matching session
        // should accept.
        assert!(matches!(hit, ValidationOutcome::Ok { .. }));
    }

    #[test]
    fn rejects_not_applicable_combination() {
        let v = validator();
        // Cloudflare does not issue ProofOfWork tokens.
        // codeql[rust/hard-coded-cryptographic-value] false-positive: deterministic test label
        let c = contract(
            VendorId::Cloudflare,
            ChallengeClass::ProofOfWork,
            Duration::from_mins(30),
            "cf-pow",
            true,
            None,
            0,
        );
        let outcome = v.validate(&c, None, 60);
        match outcome {
            ValidationOutcome::Rejected(err) => {
                assert_eq!(err.reason.kind(), InvalidationKind::NotApplicable);
            }
            ValidationOutcome::Ok { .. } => panic!("expected not-applicable reject"),
        }
    }

    #[test]
    fn vendor_policy_lookup_returns_tier2_defaults() {
        let v = validator();
        // codeql[rust/hard-coded-cryptographic-value] false-positive: deterministic test label
        let c = contract(
            VendorId::DataDome,
            ChallengeClass::Captcha,
            Duration::from_mins(10),
            "dd-1",
            true,
            None,
            0,
        );
        let policy = v.policy().policy(c.vendor_family);
        assert_eq!(policy.default_ttl(), Duration::from_mins(10));
        assert!(policy.single_use());
        assert!(policy.require_session_binding());
        assert!(policy.require_nonce());
    }

    #[test]
    fn ttl_is_clamped_to_vendor_max() {
        let v = validator();
        // Cloudflare's max ttl is 45 minutes. Submit a contract
        // claiming a 60-minute TTL.
        // codeql[rust/hard-coded-cryptographic-value] false-positive: deterministic test label
        let c = contract(
            VendorId::Cloudflare,
            ChallengeClass::Interstitial,
            Duration::from_hours(1),
            "cf-long",
            true,
            None,
            0,
        );
        let outcome = v.validate(&c, None, 40 * 60);
        match outcome {
            ValidationOutcome::Ok { effective_ttl, .. } => {
                assert!(approx_eq(effective_ttl, Duration::from_mins(45)));
            }
            ValidationOutcome::Rejected(err) => panic!("unexpected reject: {err:?}"),
        }
    }

    #[test]
    fn multi_use_token_does_not_replay_reject() {
        let v = validator();
        // FingerprintCom default policy: single_use = false.
        // Re-submitting the same nonce must not trip replay.
        // codeql[rust/hard-coded-cryptographic-value] false-positive: deterministic test label
        let c = contract(
            VendorId::FingerprintCom,
            ChallengeClass::None,
            Duration::from_hours(1),
            "fp-n",
            false,
            None,
            0,
        );
        let first = v.validate(&c, None, 60);
        assert!(matches!(first, ValidationOutcome::Ok { .. }));
        let second = v.validate(&c, None, 60);
        match second {
            ValidationOutcome::Ok { consumed, .. } => assert!(!consumed),
            ValidationOutcome::Rejected(err) => panic!("unexpected reject: {err:?}"),
        }
    }

    #[test]
    fn validation_outcome_helpers() {
        let ok = ValidationOutcome::Ok {
            // codeql[rust/hard-coded-cryptographic-value] false-positive: deterministic test label
            contract: contract(
                VendorId::Unknown,
                ChallengeClass::None,
                Duration::from_mins(1),
                "x",
                false,
                None,
                0,
            ),
            consumed: false,
            effective_ttl: Duration::from_mins(1),
        };
        assert!(ok.is_ok());
        assert!(!ok.is_rejected());
        assert!(ok.error().is_none());

        let rejected = ValidationOutcome::Rejected(TokenLifecycleError::new(
            InvalidationReason::ContractMissing {
                vendor: VendorId::Unknown,
                challenge_class: ChallengeClass::Unknown,
            },
            "no contract",
        ));
        assert!(!rejected.is_ok());
        assert!(rejected.is_rejected());
        assert_eq!(
            rejected.invalidation_kind(),
            Some(InvalidationKind::ContractMissing)
        );
    }
}
