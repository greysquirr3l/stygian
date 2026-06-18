//! Structured invalidation reasons for token lifecycle failures (T91).

use serde::{Deserialize, Serialize};

use crate::token_lifecycle::contract::ChallengeClass;
use crate::vendor_classifier::VendorId;

/// Coarse-grained kind tag for [`InvalidationReason`].
///
/// The kind is the **stable, wire-level enum** (`snake_case`)
/// that downstream consumers (alerting, dashboards, audit log
/// routers) can switch on without inspecting the full
/// [`InvalidationReason`]. The full reason carries the
/// diagnostic context (vendor family, challenge class,
/// supplied vs. expected values); the kind is the routing key.
///
/// # Example
///
/// ```
/// use stygian_charon::token_lifecycle::InvalidationKind;
///
/// let k = InvalidationKind::NonceReplayed;
/// assert_eq!(k.label(), "nonce_replayed");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidationKind {
    /// Token TTL elapsed. The
    /// [`InvalidationReason::Expired`][crate::token_lifecycle::InvalidationReason::Expired]
    /// variant carries the vendor family, challenge class,
    /// observed age, and the TTL that was crossed.
    Expired,
    /// Submission's nonce does not match the contract's nonce.
    /// The
    /// [`InvalidationReason::NonceMismatch`][crate::token_lifecycle::InvalidationReason::NonceMismatch]
    /// variant carries both nonces.
    NonceMismatch,
    /// A single-use nonce was observed more than once. The
    /// [`InvalidationReason::NonceReplayed`][crate::token_lifecycle::InvalidationReason::NonceReplayed]
    /// variant carries the observation count from the nonce
    /// book.
    NonceReplayed,
    /// Submission's session id does not match the contract's
    /// `bound_session`.
    SessionBindingMiss,
    /// No contract was supplied alongside the token. The
    /// validator returns this when the caller bypasses the
    /// contract path.
    ContractMissing,
    /// The token's challenge class is not applicable for the
    /// supplied vendor family (e.g. `Cloudflare` is asked to
    /// validate a `ProofOfWork` token — Cloudflare does not
    /// issue `PoW` tokens).
    NotApplicable,
}

impl InvalidationKind {
    /// Stable, lower-case wire label.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::InvalidationKind;
    ///
    /// assert_eq!(InvalidationKind::Expired.label(), "expired");
    /// assert_eq!(InvalidationKind::NonceMismatch.label(), "nonce_mismatch");
    /// assert_eq!(InvalidationKind::NonceReplayed.label(), "nonce_replayed");
    /// assert_eq!(InvalidationKind::SessionBindingMiss.label(), "session_binding_miss");
    /// assert_eq!(InvalidationKind::ContractMissing.label(), "contract_missing");
    /// assert_eq!(InvalidationKind::NotApplicable.label(), "not_applicable");
    /// ```
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::NonceMismatch => "nonce_mismatch",
            Self::NonceReplayed => "nonce_replayed",
            Self::SessionBindingMiss => "session_binding_miss",
            Self::ContractMissing => "contract_missing",
            Self::NotApplicable => "not_applicable",
        }
    }
}

/// Structured reason a [`TokenValidator`][crate::token_lifecycle::TokenValidator]
/// rejected a token submission.
///
/// Every variant carries the **vendor family** and
/// **challenge class** so the diagnostic payload can route the
/// invalidation to the per-family audit log without inspecting
/// the rest of the report. The kind tag is the routing key
/// (see [`InvalidationKind`]); the variant embeds the
/// diagnostic context (expected vs. observed values, age,
/// observation count, etc.).
///
/// # Example
///
/// ```
/// use stygian_charon::token_lifecycle::{ChallengeClass, InvalidationKind, InvalidationReason};
/// use stygian_charon::vendor_classifier::VendorId;
///
/// let reason = InvalidationReason::Expired {
///     vendor: VendorId::Cloudflare,
///     challenge_class: ChallengeClass::Interstitial,
///     age_secs: 1900,
///     ttl_secs: 1800,
/// };
/// assert_eq!(reason.kind(), InvalidationKind::Expired);
/// assert_eq!(reason.vendor_family(), VendorId::Cloudflare);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InvalidationReason {
    /// Token TTL elapsed before submission.
    Expired {
        /// Vendor family that issued the token.
        vendor: VendorId,
        /// Challenge class the token is bound to.
        challenge_class: ChallengeClass,
        /// Observed age at submission time, in seconds.
        age_secs: u64,
        /// TTL the contract was issued with, in seconds.
        ttl_secs: u64,
    },
    /// Submission's nonce does not match the contract's nonce.
    NonceMismatch {
        /// Vendor family the contract was issued for.
        vendor: VendorId,
        /// Challenge class the contract is bound to.
        challenge_class: ChallengeClass,
        /// Nonce the contract was issued with.
        expected: String,
        /// Nonce the submission carried.
        observed: String,
    },
    /// A single-use nonce was observed more than once.
    NonceReplayed {
        /// Vendor family the contract was issued for.
        vendor: VendorId,
        /// Challenge class the contract is bound to.
        challenge_class: ChallengeClass,
        /// Number of times the nonce has now been observed
        /// (capped at `u32::MAX` for monotonic counters).
        observation_count: u32,
    },
    /// Submission's session id does not match the contract's
    /// `bound_session`.
    SessionBindingMiss {
        /// Vendor family the contract was issued for.
        vendor: VendorId,
        /// Challenge class the contract is bound to.
        challenge_class: ChallengeClass,
        /// Session id the contract was bound to (`None` when
        /// the contract carried no binding).
        expected: Option<String>,
        /// Session id the submission carried.
        observed: Option<String>,
    },
    /// No contract was supplied alongside the token.
    ContractMissing {
        /// Vendor family the token was nominally issued for
        /// (when the caller had partial evidence).
        vendor: VendorId,
        /// Challenge class the token was nominally bound to.
        challenge_class: ChallengeClass,
    },
    /// The token's challenge class is not applicable for the
    /// supplied vendor family.
    NotApplicable {
        /// Vendor family the contract was issued for.
        vendor: VendorId,
        /// Challenge class the contract claimed to cover.
        challenge_class: ChallengeClass,
    },
}

impl InvalidationReason {
    /// Routing kind for this invalidation.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::{ChallengeClass, InvalidationKind, InvalidationReason};
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// let reason = InvalidationReason::NonceReplayed {
    ///     vendor: VendorId::Akamai,
    ///     challenge_class: ChallengeClass::ProofOfWork,
    ///     observation_count: 2,
    /// };
    /// assert_eq!(reason.kind(), InvalidationKind::NonceReplayed);
    /// ```
    #[must_use]
    pub const fn kind(&self) -> InvalidationKind {
        match self {
            Self::Expired { .. } => InvalidationKind::Expired,
            Self::NonceMismatch { .. } => InvalidationKind::NonceMismatch,
            Self::NonceReplayed { .. } => InvalidationKind::NonceReplayed,
            Self::SessionBindingMiss { .. } => InvalidationKind::SessionBindingMiss,
            Self::ContractMissing { .. } => InvalidationKind::ContractMissing,
            Self::NotApplicable { .. } => InvalidationKind::NotApplicable,
        }
    }

    /// Vendor family the invalidation is attributed to.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::{ChallengeClass, InvalidationReason};
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// let reason = InvalidationReason::NonceMismatch {
    ///     vendor: VendorId::PerimeterX,
    ///     challenge_class: ChallengeClass::IntegrityCheck,
    ///     expected: "n".to_string(),
    ///     observed: "m".to_string(),
    /// };
    /// assert_eq!(reason.vendor_family(), VendorId::PerimeterX);
    /// ```
    #[must_use]
    pub const fn vendor_family(&self) -> VendorId {
        match self {
            Self::Expired { vendor, .. }
            | Self::NonceMismatch { vendor, .. }
            | Self::NonceReplayed { vendor, .. }
            | Self::SessionBindingMiss { vendor, .. }
            | Self::ContractMissing { vendor, .. }
            | Self::NotApplicable { vendor, .. } => *vendor,
        }
    }

    /// Challenge class the invalidation is attributed to.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::{ChallengeClass, InvalidationReason};
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// let reason = InvalidationReason::SessionBindingMiss {
    ///     vendor: VendorId::DataDome,
    ///     challenge_class: ChallengeClass::Captcha,
    ///     expected: Some("s1".to_string()),
    ///     observed: Some("s2".to_string()),
    /// };
    /// assert_eq!(reason.challenge_class(), ChallengeClass::Captcha);
    /// ```
    #[must_use]
    pub const fn challenge_class(&self) -> ChallengeClass {
        match self {
            Self::Expired { challenge_class, .. }
            | Self::NonceMismatch { challenge_class, .. }
            | Self::NonceReplayed { challenge_class, .. }
            | Self::SessionBindingMiss { challenge_class, .. }
            | Self::ContractMissing { challenge_class, .. }
            | Self::NotApplicable { challenge_class, .. } => *challenge_class,
        }
    }

    /// Stable wire label — equivalent to
    /// [`kind().label()`][InvalidationKind::label]. Useful for
    /// the JSON field that the diagnostic payload exposes.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::{ChallengeClass, InvalidationReason};
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// let reason = InvalidationReason::ContractMissing {
    ///     vendor: VendorId::Unknown,
    ///     challenge_class: ChallengeClass::Unknown,
    /// };
    /// assert_eq!(reason.label(), "contract_missing");
    /// ```
    #[must_use]
    pub const fn label(&self) -> &'static str {
        self.kind().label()
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
    fn invalidation_kind_labels_are_stable() {
        for (kind, label) in [
            (InvalidationKind::Expired, "expired"),
            (InvalidationKind::NonceMismatch, "nonce_mismatch"),
            (InvalidationKind::NonceReplayed, "nonce_replayed"),
            (InvalidationKind::SessionBindingMiss, "session_binding_miss"),
            (InvalidationKind::ContractMissing, "contract_missing"),
            (InvalidationKind::NotApplicable, "not_applicable"),
        ] {
            assert_eq!(kind.label(), label);
        }
    }

    #[test]
    fn invalidation_reason_kind_vendor_and_class_match_variant() {
        let expired = InvalidationReason::Expired {
            vendor: VendorId::Cloudflare,
            challenge_class: ChallengeClass::Interstitial,
            age_secs: 100,
            ttl_secs: 60,
        };
        assert_eq!(expired.kind(), InvalidationKind::Expired);
        assert_eq!(expired.vendor_family(), VendorId::Cloudflare);
        assert_eq!(expired.challenge_class(), ChallengeClass::Interstitial);
        assert_eq!(expired.label(), "expired");

        let mismatch = InvalidationReason::NonceMismatch {
            vendor: VendorId::PerimeterX,
            challenge_class: ChallengeClass::IntegrityCheck,
            expected: "a".to_string(),
            observed: "b".to_string(),
        };
        assert_eq!(mismatch.kind(), InvalidationKind::NonceMismatch);
        assert_eq!(mismatch.vendor_family(), VendorId::PerimeterX);
        assert_eq!(mismatch.label(), "nonce_mismatch");

        let replayed = InvalidationReason::NonceReplayed {
            vendor: VendorId::Akamai,
            challenge_class: ChallengeClass::ProofOfWork,
            observation_count: 3,
        };
        assert_eq!(replayed.kind(), InvalidationKind::NonceReplayed);
        assert_eq!(replayed.vendor_family(), VendorId::Akamai);
        assert_eq!(replayed.label(), "nonce_replayed");

        let binding = InvalidationReason::SessionBindingMiss {
            vendor: VendorId::DataDome,
            challenge_class: ChallengeClass::Captcha,
            expected: Some("s1".to_string()),
            observed: Some("s2".to_string()),
        };
        assert_eq!(binding.kind(), InvalidationKind::SessionBindingMiss);
        assert_eq!(binding.vendor_family(), VendorId::DataDome);
        assert_eq!(binding.label(), "session_binding_miss");

        let missing = InvalidationReason::ContractMissing {
            vendor: VendorId::Unknown,
            challenge_class: ChallengeClass::Unknown,
        };
        assert_eq!(missing.kind(), InvalidationKind::ContractMissing);
        assert_eq!(missing.vendor_family(), VendorId::Unknown);
        assert_eq!(missing.label(), "contract_missing");

        let not_applicable = InvalidationReason::NotApplicable {
            vendor: VendorId::Cloudflare,
            challenge_class: ChallengeClass::ProofOfWork,
        };
        assert_eq!(not_applicable.kind(), InvalidationKind::NotApplicable);
        assert_eq!(not_applicable.vendor_family(), VendorId::Cloudflare);
        assert_eq!(not_applicable.label(), "not_applicable");
    }

    #[test]
    fn invalidation_reason_serializes_with_tag() {
        let reason = InvalidationReason::NonceReplayed {
            vendor: VendorId::Akamai,
            challenge_class: ChallengeClass::ProofOfWork,
            observation_count: 2,
        };
        let json = serde_json::to_string(&reason).expect("serialize");
        assert!(json.contains("\"kind\":\"nonce_replayed\""));
        assert!(json.contains("\"vendor\":\"akamai\""));
        assert!(json.contains("\"observation_count\":2"));
    }
}