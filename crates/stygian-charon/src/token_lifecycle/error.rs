//! Errors returned by the token lifecycle module (T91).

use thiserror::Error;

use crate::token_lifecycle::invalidation::InvalidationReason;
use crate::vendor_classifier::VendorId;

/// Errors returned by
/// [`TokenValidator`][crate::token_lifecycle::TokenValidator]
/// when it rejects a token submission.
///
/// Carries the structured [`InvalidationReason`] (which itself
/// embeds the vendor family + challenge class) and a short
/// human-readable message suitable for operator logs. The
/// diagnostic payload the runner exposes can switch on the
/// [`InvalidationReason::kind`][crate::token_lifecycle::InvalidationReason::kind]
/// tag to route the failure into the correct per-family audit
/// log.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct TokenLifecycleError {
    /// Structured reason the validator rejected the token.
    pub reason: InvalidationReason,
    /// Human-readable message suitable for operator logs.
    pub message: String,
}

impl TokenLifecycleError {
    /// Build a [`TokenLifecycleError`] from a reason + a
    /// human-readable message.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::{
    ///     ChallengeClass, InvalidationReason, TokenLifecycleError,
    /// };
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// let err = TokenLifecycleError::new(
    ///     InvalidationReason::Expired {
    ///         vendor: VendorId::Cloudflare,
    ///         challenge_class: ChallengeClass::Interstitial,
    ///         age_secs: 1900,
    ///         ttl_secs: 1800,
    ///     },
    ///     "Cloudflare token expired",
    /// );
    /// assert_eq!(err.message, "Cloudflare token expired");
    /// assert_eq!(err.reason.vendor_family(), VendorId::Cloudflare);
    /// ```
    #[must_use]
    pub fn new(reason: InvalidationReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }

    /// Vendor family the error is attributed to.
    #[must_use]
    pub const fn vendor_family(&self) -> VendorId {
        self.reason.vendor_family()
    }
}
