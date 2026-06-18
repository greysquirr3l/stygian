//! Challenge-token contract model (T91).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::vendor_classifier::VendorId;

/// Stable label for the kind of challenge a token is bound to.
///
/// The taxonomy mirrors the surfaces a challenge token can be
/// issued for. It is intentionally **smaller** than the
/// [`ChallengeOutcome`][crate::challenge_feedback::ChallengeOutcome]
/// enum (T83) because outcomes describe what the runner
/// observed on the wire, while `ChallengeClass` describes what
/// the token is **for**. A `Captcha` outcome and a
/// `Captcha` token challenge class are paired by construction —
/// but a `Captcha` token can also be observed against an
/// `IntegrityCheck` outcome when the vendor re-checks the
/// captcha solution during a follow-up request.
///
/// # Example
///
/// ```
/// use stygian_charon::token_lifecycle::ChallengeClass;
///
/// let c = ChallengeClass::Captcha;
/// assert_eq!(c.label(), "captcha");
/// assert!(c.requires_nonce());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChallengeClass {
    /// No challenge — token is a session cookie or
    /// bearer-style bearer that does not gate a specific
    /// challenge artefact.
    None,
    /// Soft / interstitial challenge (e.g. `cf-chl-bypass` for
    /// Cloudflare's "Just a moment…" page).
    Interstitial,
    /// Captcha challenge (reCAPTCHA, hCaptcha, `DataDome`
    /// captcha-delivery, etc.).
    Captcha,
    /// Proof-of-work challenge (e.g. `Akamai` `_abck`
    /// derivation).
    ProofOfWork,
    /// JS integrity check (e.g. `PerimeterX` `_px3` payload).
    IntegrityCheck,
    /// Cookie refresh / sticky session roll-over.
    CookieRefresh,
    /// Catch-all when the challenge class cannot be classified.
    Unknown,
}

impl ChallengeClass {
    /// Stable, lower-case wire label.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::ChallengeClass;
    ///
    /// assert_eq!(ChallengeClass::Interstitial.label(), "interstitial");
    /// assert_eq!(ChallengeClass::Captcha.label(), "captcha");
    /// assert_eq!(ChallengeClass::ProofOfWork.label(), "proof_of_work");
    /// assert_eq!(ChallengeClass::IntegrityCheck.label(), "integrity_check");
    /// assert_eq!(ChallengeClass::CookieRefresh.label(), "cookie_refresh");
    /// assert_eq!(ChallengeClass::None.label(), "none");
    /// assert_eq!(ChallengeClass::Unknown.label(), "unknown");
    /// ```
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Interstitial => "interstitial",
            Self::Captcha => "captcha",
            Self::ProofOfWork => "proof_of_work",
            Self::IntegrityCheck => "integrity_check",
            Self::CookieRefresh => "cookie_refresh",
            Self::Unknown => "unknown",
        }
    }

    /// Whether the validator must enforce nonce binding for
    /// tokens of this challenge class.
    ///
    /// Every class except [`None`][Self::None] requires a
    /// nonce. `None` is the "session cookie" path — the cookie
    /// itself is the contract and nonce binding is meaningless.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::ChallengeClass;
    ///
    /// assert!(!ChallengeClass::None.requires_nonce());
    /// assert!(ChallengeClass::Interstitial.requires_nonce());
    /// assert!(ChallengeClass::Captcha.requires_nonce());
    /// ```
    #[must_use]
    pub const fn requires_nonce(self) -> bool {
        !matches!(self, Self::None)
    }

    /// Whether the validator must enforce session binding for
    /// tokens of this challenge class.
    ///
    /// Only the cookie-refresh / sticky-session class is
    /// sensitive to session binding by default; all other
    /// classes are session-agnostic. Per-vendor policy overrides
    /// can still require session binding for any class.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::ChallengeClass;
    ///
    /// assert!(ChallengeClass::CookieRefresh.requires_session_binding());
    /// assert!(!ChallengeClass::Interstitial.requires_session_binding());
    /// ```
    #[must_use]
    pub const fn requires_session_binding(self) -> bool {
        matches!(self, Self::CookieRefresh)
    }
}

/// Lifecycle contract for a single challenge token.
///
/// The contract is the **wire-level schema** for "what the
/// scraper is allowed to do with this token". It is a pure
/// data structure: validation logic lives in
/// [`TokenValidator`][crate::token_lifecycle::TokenValidator],
/// nonce bookkeeping lives in
/// [`NonceBook`][crate::token_lifecycle::NonceBook].
///
/// # Example
///
/// ```
/// use std::time::Duration;
/// use stygian_charon::token_lifecycle::{ChallengeClass, TokenContract};
/// use stygian_charon::vendor_classifier::VendorId;
///
/// let contract = TokenContract {
///     token_id: "cf-bypass-xyz".to_string(),
///     issued_at_unix_secs: 1_700_000_000,
///     ttl: Duration::from_mins(30),
///     nonce: "n-001".to_string(),
///     vendor_family: VendorId::Cloudflare,
///     challenge_class: ChallengeClass::Interstitial,
///     single_use: true,
///     bound_session: Some("session-abc".to_string()),
///     description: "Cloudflare interstitial bypass token".to_string(),
/// };
/// assert_eq!(contract.vendor_family, VendorId::Cloudflare);
/// assert!(contract.single_use);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenContract {
    /// Stable token identifier (vendor-issued or
    /// scraper-derived). Used as a stable key in audit logs.
    pub token_id: String,
    /// Unix epoch seconds when the token was issued. The
    /// validator uses this with the supplied `now_unix_secs`
    /// to compute the token's age.
    pub issued_at_unix_secs: u64,
    /// Time-to-live the token was issued with. The validator
    /// clamps this against the per-vendor
    /// [`TokenPolicy::max_ttl`][crate::token_lifecycle::TokenPolicy::max_ttl]
    /// ceiling before applying it.
    pub ttl: Duration,
    /// Per-issuance nonce. The validator enforces that every
    /// submission carries the same nonce and that a single-use
    /// nonce cannot be submitted twice.
    pub nonce: String,
    /// Vendor family the token was issued for. Used for both
    /// the per-vendor policy lookup and the diagnostic
    /// invalidation routing.
    pub vendor_family: VendorId,
    /// Challenge class the token is bound to. Used for both
    /// the default per-class policy and the diagnostic
    /// invalidation routing.
    pub challenge_class: ChallengeClass,
    /// `true` when the token may only be submitted once. The
    /// validator marks the nonce as consumed on first
    /// successful validation.
    pub single_use: bool,
    /// Optional sticky-session identifier the token is bound
    /// to. When `Some`, the validator rejects submissions
    /// whose `session_id` does not match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_session: Option<String>,
    /// Short human-readable description (operator log / audit).
    #[serde(default)]
    pub description: String,
}

impl TokenContract {
    /// Effective age in seconds at the supplied `now_unix_secs`.
    ///
    /// Returns `0` when `now < issued_at_unix_secs` (clock skew
    /// or test fixtures where the supplied clock is before the
    /// issuance timestamp). The validator still rejects the
    /// token when the TTL check fires — clock skew is a
    /// different invalidation path that the policy planner
    /// surfaces via
    /// [`InvalidationKind::Expired`][crate::token_lifecycle::InvalidationKind::Expired].
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use stygian_charon::token_lifecycle::{ChallengeClass, TokenContract};
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// let contract = TokenContract {
    ///     token_id: "x".to_string(),
    ///     issued_at_unix_secs: 100,
    ///     ttl: Duration::from_mins(5),
    ///     nonce: "n".to_string(),
    ///     vendor_family: VendorId::Unknown,
    ///     challenge_class: ChallengeClass::None,
    ///     single_use: false,
    ///     bound_session: None,
    ///     description: String::new(),
    /// };
    /// assert_eq!(contract.age_secs(160), 60);
    /// assert_eq!(contract.age_secs(50), 0);
    /// ```
    #[must_use]
    pub const fn age_secs(&self, now_unix_secs: u64) -> u64 {
        now_unix_secs.saturating_sub(self.issued_at_unix_secs)
    }

    /// `true` when the token's age at `now_unix_secs` exceeds
    /// `ttl`.
    ///
    /// This is the **raw TTL check** — the validator applies
    /// the per-vendor `max_ttl` clamp **before** calling this
    /// helper. Callers that want to validate on their own
    /// (e.g. doctests) should respect the policy table the same
    /// way the validator does.
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use stygian_charon::token_lifecycle::{ChallengeClass, TokenContract};
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// let contract = TokenContract {
    ///     token_id: "x".to_string(),
    ///     issued_at_unix_secs: 0,
    ///     ttl: Duration::from_mins(1),
    ///     nonce: "n".to_string(),
    ///     vendor_family: VendorId::Unknown,
    ///     challenge_class: ChallengeClass::None,
    ///     single_use: false,
    ///     bound_session: None,
    ///     description: String::new(),
    /// };
    /// assert!(!contract.is_expired(30));
    /// assert!(contract.is_expired(120));
    /// ```
    #[must_use]
    pub const fn is_expired(&self, now_unix_secs: u64) -> bool {
        self.age_secs(now_unix_secs) >= self.ttl.as_secs()
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

    #[test]
    fn challenge_class_labels_are_stable() {
        for (variant, label) in [
            (ChallengeClass::None, "none"),
            (ChallengeClass::Interstitial, "interstitial"),
            (ChallengeClass::Captcha, "captcha"),
            (ChallengeClass::ProofOfWork, "proof_of_work"),
            (ChallengeClass::IntegrityCheck, "integrity_check"),
            (ChallengeClass::CookieRefresh, "cookie_refresh"),
            (ChallengeClass::Unknown, "unknown"),
        ] {
            assert_eq!(variant.label(), label);
        }
    }

    #[test]
    fn challenge_class_requires_nonce_except_none() {
        assert!(!ChallengeClass::None.requires_nonce());
        assert!(ChallengeClass::Interstitial.requires_nonce());
        assert!(ChallengeClass::Captcha.requires_nonce());
        assert!(ChallengeClass::ProofOfWork.requires_nonce());
        assert!(ChallengeClass::IntegrityCheck.requires_nonce());
        assert!(ChallengeClass::CookieRefresh.requires_nonce());
        assert!(ChallengeClass::Unknown.requires_nonce());
    }

    #[test]
    fn challenge_class_requires_session_binding_only_for_cookie_refresh() {
        assert!(ChallengeClass::CookieRefresh.requires_session_binding());
        for variant in [
            ChallengeClass::None,
            ChallengeClass::Interstitial,
            ChallengeClass::Captcha,
            ChallengeClass::ProofOfWork,
            ChallengeClass::IntegrityCheck,
            ChallengeClass::Unknown,
        ] {
            assert!(
                !variant.requires_session_binding(),
                "{variant:?} should not require session binding"
            );
        }
    }

    #[test]
    fn token_contract_age_secs_clamps_clock_skew_to_zero() {
        let contract = TokenContract {
            token_id: "x".to_string(),
            issued_at_unix_secs: 100,
            ttl: Duration::from_mins(1),
            nonce: "n".to_string(),
            vendor_family: VendorId::Unknown,
            challenge_class: ChallengeClass::None,
            single_use: false,
            bound_session: None,
            description: String::new(),
        };
        assert_eq!(contract.age_secs(50), 0);
        assert_eq!(contract.age_secs(100), 0);
        assert_eq!(contract.age_secs(160), 60);
    }

    #[test]
    fn token_contract_is_expired_returns_true_after_ttl() {
        let contract = TokenContract {
            token_id: "x".to_string(),
            issued_at_unix_secs: 0,
            ttl: Duration::from_mins(1),
            nonce: "n".to_string(),
            vendor_family: VendorId::Unknown,
            challenge_class: ChallengeClass::None,
            single_use: false,
            bound_session: None,
            description: String::new(),
        };
        assert!(!contract.is_expired(0));
        assert!(!contract.is_expired(30));
        assert!(!contract.is_expired(59));
        assert!(contract.is_expired(60));
        assert!(contract.is_expired(120));
    }

    #[test]
    fn token_contract_serializes_round_trip() {
        let contract = TokenContract {
            token_id: "x".to_string(),
            issued_at_unix_secs: 100,
            ttl: Duration::from_mins(1),
            nonce: "n".to_string(),
            vendor_family: VendorId::DataDome,
            challenge_class: ChallengeClass::Captcha,
            single_use: true,
            bound_session: Some("session-1".to_string()),
            description: "x".to_string(),
        };
        let json = serde_json::to_string(&contract).expect("serialize");
        let back: TokenContract = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(contract, back);
    }
}