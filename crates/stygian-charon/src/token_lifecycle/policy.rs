//! Vendor-aware token policy table (T91).
//!
//! The [`TokenPolicyTable`] is the lookup the
//! [`TokenValidator`][crate::token_lifecycle::TokenValidator]
//! consults before applying a [`TokenContract`][crate::token_lifecycle::TokenContract].
//! It carries four knobs per vendor family:
//!
//! - **Default TTL**: the TTL a freshly-issued token is
//!   expected to carry.
//! - **Max TTL**: the upper bound the validator will accept.
//!   Contracts with a longer TTL are **clamped** to `max_ttl`
//!   before the validator applies the TTL check.
//! - **`require_nonce`**: whether the validator must enforce
//!   per-issuance nonce binding. Off by default for
//!   [`ChallengeClass::None`][crate::token_lifecycle::ChallengeClass::None]
//!   tokens (cookies); on for every other challenge class.
//! - **`single_use`**: the per-vendor default for
//!   [`TokenContract::single_use`][crate::token_lifecycle::TokenContract::single_use].
//!   The validator uses this **only** when the contract's own
//!   `single_use` field is not supplied; the contract field
//!   always wins.
//! - **`require_session_binding`**: whether the validator must
//!   enforce sticky-session binding. Off by default for
//!   [`ChallengeClass::CookieRefresh`][crate::token_lifecycle::ChallengeClass::CookieRefresh]
//!   — except when the per-vendor policy overrides the default.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::vendor_classifier::VendorId;

/// Per-vendor defaults for the
/// [`TokenValidator`][crate::token_lifecycle::TokenValidator].
///
/// Every field is documented in the
/// [module docs][crate::token_lifecycle#vendor-policy-table].
/// The defaults are the values baked into
/// [`builtin_token_policies`]; operators can override per-vendor
/// with [`TokenPolicyTable::with_policy`].
///
/// # Example
///
/// ```
/// use std::time::Duration;
/// use stygian_charon::token_lifecycle::TokenPolicy;
/// use stygian_charon::vendor_classifier::VendorId;
///
/// let policy = TokenPolicy::default_for(VendorId::Cloudflare);
/// assert_eq!(policy.default_ttl(), Duration::from_mins(30));
/// assert!(policy.single_use());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenPolicy {
    /// Default TTL a freshly-issued token is expected to carry.
    default_ttl: Duration,
    /// Upper bound the validator will accept before clamping.
    max_ttl: Duration,
    /// Whether the validator must enforce per-issuance nonce
    /// binding.
    require_nonce: bool,
    /// Per-vendor default for the single-use flag.
    single_use: bool,
    /// Whether the validator must enforce sticky-session
    /// binding.
    require_session_binding: bool,
}

impl TokenPolicy {
    /// Build a [`TokenPolicy`] with explicit values. The
    /// constructor clamps `default_ttl` to `max_ttl` so a
    /// caller cannot accidentally build a policy whose default
    /// is longer than its maximum.
    #[must_use]
    pub fn new(
        default_ttl: Duration,
        max_ttl: Duration,
        require_nonce: bool,
        single_use: bool,
        require_session_binding: bool,
    ) -> Self {
        let default_ttl = if default_ttl > max_ttl {
            max_ttl
        } else {
            default_ttl
        };
        Self {
            default_ttl,
            max_ttl,
            require_nonce,
            single_use,
            require_session_binding,
        }
    }

    /// Replace the default TTL. The new value is clamped to
    /// the current `max_ttl` so the policy invariant
    /// (`max_ttl >= default_ttl`) is preserved.
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use stygian_charon::token_lifecycle::TokenPolicy;
    ///
    /// let p = TokenPolicy::default_for(stygian_charon::vendor_classifier::VendorId::Cloudflare);
    /// let tighter = p.with_default_ttl(Duration::from_mins(5));
    /// assert_eq!(tighter.default_ttl(), Duration::from_mins(5));
    /// ```
    #[must_use]
    pub fn with_default_ttl(mut self, default_ttl: Duration) -> Self {
        self.default_ttl = if default_ttl > self.max_ttl {
            self.max_ttl
        } else {
            default_ttl
        };
        self
    }

    /// Replace the maximum TTL.
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use stygian_charon::token_lifecycle::TokenPolicy;
    ///
    /// let p = TokenPolicy::default_for(stygian_charon::vendor_classifier::VendorId::Cloudflare);
    /// let tighter = p.with_max_ttl(Duration::from_mins(20));
    /// assert_eq!(tighter.max_ttl(), Duration::from_mins(20));
    /// ```
    #[must_use]
    pub fn with_max_ttl(mut self, max_ttl: Duration) -> Self {
        self.max_ttl = max_ttl;
        if self.default_ttl > max_ttl {
            self.default_ttl = max_ttl;
        }
        self
    }

    /// Default TTL baked into this policy.
    #[must_use]
    pub const fn default_ttl(&self) -> Duration {
        self.default_ttl
    }

    /// Maximum TTL the validator will accept.
    #[must_use]
    pub const fn max_ttl(&self) -> Duration {
        self.max_ttl
    }

    /// Whether per-issuance nonce binding is required.
    #[must_use]
    pub const fn require_nonce(&self) -> bool {
        self.require_nonce
    }

    /// Per-vendor default for the single-use flag.
    #[must_use]
    pub const fn single_use(&self) -> bool {
        self.single_use
    }

    /// Whether sticky-session binding is required.
    #[must_use]
    pub const fn require_session_binding(&self) -> bool {
        self.require_session_binding
    }

    /// Per-vendor default policy matching the
    /// [vendor policy table][crate::token_lifecycle#vendor-policy-table].
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use stygian_charon::token_lifecycle::TokenPolicy;
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// assert_eq!(TokenPolicy::default_for(VendorId::Cloudflare).default_ttl(), Duration::from_mins(30));
    /// assert_eq!(TokenPolicy::default_for(VendorId::DataDome).default_ttl(), Duration::from_mins(10));
    /// assert_eq!(TokenPolicy::default_for(VendorId::Unknown).default_ttl(), Duration::from_mins(5));
    /// ```
    #[must_use]
    pub fn default_for(vendor: VendorId) -> Self {
        match vendor {
            VendorId::Cloudflare => Self::new(
                Duration::from_mins(30),
                Duration::from_mins(45),
                true,
                true,
                false,
            ),
            VendorId::Akamai | VendorId::PerimeterX | VendorId::Imperva => Self::new(
                Duration::from_mins(15),
                Duration::from_mins(30),
                true,
                true,
                true,
            ),
            VendorId::DataDome | VendorId::ShapeSecurity => Self::new(
                Duration::from_mins(10),
                Duration::from_mins(20),
                true,
                true,
                true,
            ),
            VendorId::Hcaptcha | VendorId::Recaptcha | VendorId::Unknown => Self::new(
                Duration::from_mins(5),
                Duration::from_mins(10),
                true,
                true,
                false,
            ),
            VendorId::Kasada => Self::new(
                Duration::from_mins(5),
                Duration::from_mins(10),
                true,
                true,
                true,
            ),
            VendorId::FingerprintCom => Self::new(
                Duration::from_hours(1),
                Duration::from_hours(2),
                true,
                false,
                false,
            ),
        }
    }
}

/// Per-vendor policy lookup table.
///
/// The table is keyed by [`VendorId`] and consults the per-vendor
/// [`TokenPolicy::default_for`] when a vendor is not explicitly
/// registered. Callers can override per-vendor with
/// [`with_policy`][Self::with_policy].
///
/// The default-on path is
/// [`TokenPolicyTable::with_builtin_defaults`], which seeds the
/// table with every vendor the T89 classifier knows about
/// (Tier 1 + Tier 2 + the `Unknown` fallback).
///
/// # Example
///
/// ```
/// use std::time::Duration;
/// use stygian_charon::token_lifecycle::TokenPolicyTable;
/// use stygian_charon::vendor_classifier::VendorId;
///
/// let mut table = TokenPolicyTable::with_builtin_defaults();
/// // Override Cloudflare to a stricter 5-minute default TTL.
/// let tighter = table.policy(VendorId::Cloudflare).with_default_ttl(Duration::from_mins(5));
/// table = table.with_policy(VendorId::Cloudflare, tighter);
/// assert_eq!(table.policy(VendorId::Cloudflare).default_ttl(), Duration::from_mins(5));
/// ```
#[derive(Debug, Clone, Default)]
pub struct TokenPolicyTable {
    overrides: BTreeMap<VendorId, TokenPolicy>,
}

impl TokenPolicyTable {
    /// Build an empty table (no overrides; every lookup returns
    /// the [`TokenPolicy::default_for`] baseline).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::TokenPolicyTable;
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// let table = TokenPolicyTable::empty();
    /// assert!(table.is_empty());
    /// ```
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Build a table seeded with the per-vendor defaults for
    /// every [`VendorId`] variant. The `Unknown` vendor is
    /// always included as the catch-all fallback.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::TokenPolicyTable;
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// let table = TokenPolicyTable::with_builtin_defaults();
    /// assert!(!table.is_empty());
    /// assert!(table.contains(VendorId::Cloudflare));
    /// assert!(table.contains(VendorId::DataDome));
    /// assert!(table.contains(VendorId::Unknown));
    /// ```
    #[must_use]
    pub fn with_builtin_defaults() -> Self {
        let mut overrides = BTreeMap::new();
        for vendor in builtin_token_policies() {
            overrides.insert(vendor.0, vendor.1);
        }
        Self { overrides }
    }

    /// `true` when the table has no overrides registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.overrides.is_empty()
    }

    /// `true` when the table has an override registered for
    /// `vendor`.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::TokenPolicyTable;
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// let table = TokenPolicyTable::with_builtin_defaults();
    /// assert!(table.contains(VendorId::Cloudflare));
    /// ```
    #[must_use]
    pub fn contains(&self, vendor: VendorId) -> bool {
        self.overrides.contains_key(&vendor)
    }

    /// Number of vendors currently registered (including the
    /// `Unknown` fallback).
    #[must_use]
    pub fn len(&self) -> usize {
        self.overrides.len()
    }

    /// Per-vendor policy. Returns the override if one is
    /// registered, otherwise the [`TokenPolicy::default_for`]
    /// baseline for that vendor.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::token_lifecycle::TokenPolicyTable;
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// let table = TokenPolicyTable::with_builtin_defaults();
    /// let policy = table.policy(VendorId::Akamai);
    /// assert!(policy.require_session_binding());
    /// ```
    #[must_use]
    pub fn policy(&self, vendor: VendorId) -> TokenPolicy {
        self.overrides
            .get(&vendor)
            .copied()
            .unwrap_or_else(|| TokenPolicy::default_for(vendor))
    }

    /// Register an override for `vendor`. The override
    /// **replaces** any existing entry.
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    /// use stygian_charon::token_lifecycle::{TokenPolicy, TokenPolicyTable};
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// let mut table = TokenPolicyTable::with_builtin_defaults();
    /// let override_policy = TokenPolicy::new(
    ///     Duration::from_mins(1),
    ///     Duration::from_mins(2),
    ///     true,
    ///     true,
    ///     true,
    /// );
    /// table = table.with_policy(VendorId::PerimeterX, override_policy);
    /// assert_eq!(table.policy(VendorId::PerimeterX).default_ttl(), Duration::from_mins(1));
    /// ```
    #[must_use]
    pub fn with_policy(mut self, vendor: VendorId, policy: TokenPolicy) -> Self {
        self.overrides.insert(vendor, policy);
        self
    }

    /// Ids of every vendor currently registered (including the
    /// `Unknown` fallback when present).
    #[must_use]
    pub fn vendors(&self) -> Vec<VendorId> {
        self.overrides.keys().copied().collect()
    }
}

/// Snapshot of the built-in per-vendor policy table.
///
/// Returns `(vendor, policy)` pairs in [`VendorId`] discriminant
/// order so the JSON form is byte-stable. Used by
/// [`TokenPolicyTable::with_builtin_defaults`] and by the
/// compile-time validation in
/// [`compile_check_builtin_token_policies`].
///
/// # Example
///
/// ```
/// use stygian_charon::token_lifecycle::builtin_token_policies;
///
/// let rows = builtin_token_policies();
/// assert!(rows.iter().any(|(v, _)| *v == stygian_charon::vendor_classifier::VendorId::Cloudflare));
/// ```
#[must_use]
pub fn builtin_token_policies() -> Vec<(VendorId, TokenPolicy)> {
    [
        VendorId::Akamai,
        VendorId::Cloudflare,
        VendorId::DataDome,
        VendorId::PerimeterX,
        VendorId::Hcaptcha,
        VendorId::Recaptcha,
        VendorId::Kasada,
        VendorId::FingerprintCom,
        VendorId::ShapeSecurity,
        VendorId::Imperva,
        VendorId::Unknown,
    ]
    .iter()
    .map(|v| (*v, TokenPolicy::default_for(*v)))
    .collect()
}

/// Compile-time guarantee that every baseline policy the
/// built-in table seeds is well-formed.
///
/// Used by the
/// `compile_check_builtin_token_policies` test in the module
/// tests block below.
#[doc(hidden)]
#[allow(dead_code)]
pub fn compile_check_builtin_token_policies() {
    for (vendor, policy) in builtin_token_policies() {
        assert!(policy.max_ttl() >= policy.default_ttl());
        let _ = vendor;
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
    fn token_policy_clamps_default_ttl_to_max_ttl() {
        let policy = TokenPolicy::new(
            Duration::from_hours(1),
            Duration::from_mins(10),
            true,
            true,
            false,
        );
        assert_eq!(policy.default_ttl(), Duration::from_mins(10));
        assert_eq!(policy.max_ttl(), Duration::from_mins(10));
    }

    #[test]
    fn vendor_default_policies_match_module_table() {
        assert_eq!(
            TokenPolicy::default_for(VendorId::Cloudflare).default_ttl(),
            Duration::from_mins(30)
        );
        assert_eq!(
            TokenPolicy::default_for(VendorId::Cloudflare).max_ttl(),
            Duration::from_mins(45)
        );
        assert_eq!(
            TokenPolicy::default_for(VendorId::Akamai).default_ttl(),
            Duration::from_mins(15)
        );
        assert_eq!(
            TokenPolicy::default_for(VendorId::DataDome).default_ttl(),
            Duration::from_mins(10)
        );
        assert_eq!(
            TokenPolicy::default_for(VendorId::PerimeterX).default_ttl(),
            Duration::from_mins(15)
        );
        assert_eq!(
            TokenPolicy::default_for(VendorId::Hcaptcha).default_ttl(),
            Duration::from_mins(5)
        );
        assert_eq!(
            TokenPolicy::default_for(VendorId::Recaptcha).default_ttl(),
            Duration::from_mins(5)
        );
        assert_eq!(
            TokenPolicy::default_for(VendorId::Kasada).default_ttl(),
            Duration::from_mins(5)
        );
        assert_eq!(
            TokenPolicy::default_for(VendorId::FingerprintCom).default_ttl(),
            Duration::from_hours(1)
        );
        assert_eq!(
            TokenPolicy::default_for(VendorId::ShapeSecurity).default_ttl(),
            Duration::from_mins(10)
        );
        assert_eq!(
            TokenPolicy::default_for(VendorId::Imperva).default_ttl(),
            Duration::from_mins(15)
        );
        assert_eq!(
            TokenPolicy::default_for(VendorId::Unknown).default_ttl(),
            Duration::from_mins(5)
        );
    }

    #[test]
    fn default_for_includes_required_session_binding_for_tier2() {
        // Tier 2 vendors require session binding by default.
        assert!(TokenPolicy::default_for(VendorId::DataDome).require_session_binding());
        assert!(TokenPolicy::default_for(VendorId::PerimeterX).require_session_binding());
        assert!(TokenPolicy::default_for(VendorId::Akamai).require_session_binding());
        // Tier 1 / fingerprint vendors do not.
        assert!(!TokenPolicy::default_for(VendorId::Cloudflare).require_session_binding());
        assert!(!TokenPolicy::default_for(VendorId::Hcaptcha).require_session_binding());
        assert!(!TokenPolicy::default_for(VendorId::FingerprintCom).require_session_binding());
    }

    #[test]
    fn builtin_policies_cover_every_vendor_in_taxonomy() {
        let rows = builtin_token_policies();
        assert!(rows.iter().any(|(v, _)| *v == VendorId::Unknown));
        assert_eq!(rows.len(), 11);
    }

    #[test]
    fn compile_check_builtin_token_policies_passes_for_builtins() {
        // Re-run the runtime compile-check helper to make sure
        // it executes end-to-end against the built-in table.
        compile_check_builtin_token_policies();
    }

    #[test]
    fn policy_table_lookup_returns_override_or_default() {
        let mut table = TokenPolicyTable::empty();
        // Empty table: lookups return TokenPolicy::default_for().
        assert_eq!(
            table.policy(VendorId::Cloudflare).default_ttl(),
            TokenPolicy::default_for(VendorId::Cloudflare).default_ttl()
        );

        // Register an override.
        let override_policy = TokenPolicy::new(
            Duration::from_mins(1),
            Duration::from_mins(2),
            true,
            true,
            true,
        );
        table = table.with_policy(VendorId::Cloudflare, override_policy);
        assert_eq!(
            table.policy(VendorId::Cloudflare).default_ttl(),
            Duration::from_mins(1)
        );
        // Non-overridden vendor still returns the baseline.
        assert_eq!(
            table.policy(VendorId::DataDome).default_ttl(),
            TokenPolicy::default_for(VendorId::DataDome).default_ttl()
        );
    }

    #[test]
    fn with_builtin_defaults_seeds_every_vendor() {
        let table = TokenPolicyTable::with_builtin_defaults();
        for vendor in [
            VendorId::Akamai,
            VendorId::Cloudflare,
            VendorId::DataDome,
            VendorId::PerimeterX,
            VendorId::Hcaptcha,
            VendorId::Recaptcha,
            VendorId::Kasada,
            VendorId::FingerprintCom,
            VendorId::ShapeSecurity,
            VendorId::Imperva,
            VendorId::Unknown,
        ] {
            assert!(
                table.contains(vendor),
                "missing builtin policy for {vendor:?}"
            );
        }
        assert!(!table.is_empty());
    }

    #[test]
    fn policy_table_is_additive_after_override() {
        let table = TokenPolicyTable::with_builtin_defaults().with_policy(
            VendorId::Cloudflare,
            TokenPolicy::new(
                Duration::from_mins(1),
                Duration::from_mins(2),
                true,
                true,
                true,
            ),
        );
        // Cloudflare override applied.
        assert_eq!(
            table.policy(VendorId::Cloudflare).default_ttl(),
            Duration::from_mins(1)
        );
        // Akamai untouched.
        assert_eq!(
            table.policy(VendorId::Akamai).default_ttl(),
            Duration::from_mins(15)
        );
    }
}
