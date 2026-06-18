//! Fingerprint freshness contracts for browser identity reuse.
//!
//! Browser identity artifacts — fingerprints, sticky sessions, and
//! challenge contexts — must not be reused past a safe age, across
//! incompatible targets, or when their underlying signature has rotated.
//! This module provides a deterministic freshness decision function that
//! callers can plug into the [`acquisition`][crate::acquisition] runner
//! and the stealth v3 identity paths to reject stale or mismatched
//! artifacts before they are reused.
//!
//! ## Feature flag
//!
//! This module is **default-on** and is always compiled as part of the
//! `stygian-browser` crate. The [`AcquisitionRunner`][crate::acquisition::AcquisitionRunner]
//! and stealth v3 paths consult the freshness check on every reuse so
//! integration tests gated on those features exercise it.
//!
//! ## Domain-aware TTL defaults
//!
//! [`FreshnessPolicy::for_domain`] resolves a max-age using four
//! [`DomainClass`]es that callers can tune via the
//! `domain_class_overrides` map:
//!
//! - [`DomainClass::Sensitive`] (default `120 s`) — auth, payment, or
//!   challenge-issuing endpoints.
//! - [`DomainClass::Authenticated`] (default `600 s`) — logged-in
//!   user surfaces.
//! - [`DomainClass::Hostile`] (default `300 s`) — known anti-bot
//!   targets.
//! - [`DomainClass::Default`] (default `1800 s`) — generic targets.
//!
//! ## Telemetry fields
//!
//! Every non-[`FreshnessDecision::Valid`] decision carries an
//! [`InvalidationReason`] whose fields explain *why* the artifact was
//! rejected (observed vs. contract domain, observed vs. contract
//! signature, captured vs. observed timestamp, elapsed vs. max-age).
//! The runner emits these fields via `tracing::warn!` and the
//! [`FreshnessReport`] attached to the acquisition result.
//!
//! # Example
//!
//! ```
//! use stygian_browser::freshness::{
//!     DomainClass, FreshnessCheckInput, FreshnessContract, FreshnessPolicy,
//!     FreshnessPolicyKind, check,
//! };
//! use std::time::Duration;
//!
//! let policy = FreshnessPolicy::default();
//! let contract = FreshnessContract::with_signature(
//!     "example.com",
//!     "sha256:abc123",
//!     1_700_000_000_000,
//!     Duration::from_millis(policy.max_age_ms_for(DomainClass::Default)),
//!     FreshnessPolicyKind::Standard,
//! )
//! .expect("valid contract");
//! let decision = check(
//!     &contract,
//!     &FreshnessCheckInput::new("example.com", Some("sha256:abc123"), 1_700_000_060_000),
//! );
//! assert!(decision.is_valid());
//! ```

use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── Error type ───────────────────────────────────────────────────────────────

/// Errors produced by freshness contract construction.
#[derive(Debug, Error)]
pub enum FreshnessError {
    /// Contract could not be serialised or deserialised.
    #[error("failed to (de)serialise freshness contract: {0}")]
    Serialization(String),
    /// Contract carried an invalid field (empty domain, zero max-age, etc.).
    #[error("invalid freshness contract: {0}")]
    InvalidContract(String),
}

impl From<serde_json::Error> for FreshnessError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err.to_string())
    }
}

// ─── Policy ───────────────────────────────────────────────────────────────────

/// Coarse policy band for a freshness contract.
///
/// Higher bands require shorter maximum ages and stricter signature
/// matching, lowering the chance of reusing an identity that anti-bot
/// vendors may have already catalogued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreshnessPolicyKind {
    /// Shortest TTLs, signatures must match.
    Strict,
    /// Default TTLs, signatures preferred but optional.
    Standard,
    /// Longer TTLs, best-effort validation.
    Permissive,
}

/// Domain classification that controls default max-age selection.
///
/// The [`FreshnessPolicy`] resolves one of these classes per host via
/// [`FreshnessPolicy::for_domain`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainClass {
    /// Generic target — longer TTL is safe.
    Default,
    /// Hostile anti-bot target — short TTL to limit exposure.
    Hostile,
    /// Authenticated surface — moderate TTL because the user is logged in.
    Authenticated,
    /// Sensitive target (auth issuer, payment, challenge endpoint) —
    /// shortest TTL.
    Sensitive,
}

impl DomainClass {
    /// String label used in telemetry output.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Hostile => "hostile",
            Self::Authenticated => "authenticated",
            Self::Sensitive => "sensitive",
        }
    }
}

/// Configurable TTL and signature policy for freshness contracts.
///
/// The default TTLs are tuned for typical scraping workflows; callers
/// can override them per-policy via [`FreshnessPolicy::with_overrides`]
/// or per-domain via [`FreshnessPolicy::with_domain_override`].
///
/// # Example
///
/// ```
/// use stygian_browser::freshness::{DomainClass, FreshnessPolicy};
/// use std::time::Duration;
///
/// let mut policy = FreshnessPolicy::default();
/// policy = policy.with_domain_override("accounts.example.com", Some(DomainClass::Sensitive));
/// assert_eq!(policy.class_for("accounts.example.com"), DomainClass::Sensitive);
/// assert_eq!(
///     policy.max_age_for("accounts.example.com"),
///     Duration::from_millis(policy.max_age_ms_for(DomainClass::Sensitive))
/// );
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreshnessPolicy {
    /// Policy band for short-circuit classification (logging, telemetry).
    pub kind: FreshnessPolicyKind,
    /// Per-domain class overrides keyed by lowercased host.
    pub domain_class_overrides: HashMap<String, DomainClass>,
    /// Default max-age in milliseconds.
    pub default_max_age_ms: u64,
    /// Max-age for [`DomainClass::Hostile`] in milliseconds.
    pub hostile_max_age_ms: u64,
    /// Max-age for [`DomainClass::Authenticated`] in milliseconds.
    pub authenticated_max_age_ms: u64,
    /// Max-age for [`DomainClass::Sensitive`] in milliseconds.
    pub sensitive_max_age_ms: u64,
    /// When `true`, contracts without a signature are rejected
    /// regardless of TTL.
    pub signature_required: bool,
}

impl Default for FreshnessPolicy {
    fn default() -> Self {
        Self {
            kind: FreshnessPolicyKind::Standard,
            domain_class_overrides: HashMap::new(),
            default_max_age_ms: 1_800_000,
            hostile_max_age_ms: 300_000,
            authenticated_max_age_ms: 600_000,
            sensitive_max_age_ms: 120_000,
            signature_required: false,
        }
    }
}

impl FreshnessPolicy {
    /// Build a policy with explicit `kind` and default TTLs.
    #[must_use]
    pub fn with_kind(kind: FreshnessPolicyKind) -> Self {
        Self {
            kind,
            domain_class_overrides: HashMap::new(),
            default_max_age_ms: 1_800_000,
            hostile_max_age_ms: 300_000,
            authenticated_max_age_ms: 600_000,
            sensitive_max_age_ms: 120_000,
            signature_required: false,
        }
    }

    /// Tighten all TTLs by `factor` (e.g. `0.5` for half-life).
    #[must_use]
    pub const fn tightened(mut self, factor: f64) -> Self {
        let factor = factor.clamp(0.01, 1.0);
        self.default_max_age_ms = scale_ms(self.default_max_age_ms, factor);
        self.hostile_max_age_ms = scale_ms(self.hostile_max_age_ms, factor);
        self.authenticated_max_age_ms = scale_ms(self.authenticated_max_age_ms, factor);
        self.sensitive_max_age_ms = scale_ms(self.sensitive_max_age_ms, factor);
        self
    }

    /// Override the [`DomainClass`] for `host`.
    ///
    /// `host` is normalised to lower-case ASCII before being inserted.
    /// Pass `None` to clear an existing override.
    #[must_use]
    pub fn with_domain_override(mut self, host: &str, class: Option<DomainClass>) -> Self {
        let key = host.trim().to_ascii_lowercase();
        match class {
            Some(c) => {
                self.domain_class_overrides.insert(key, c);
            }
            None => {
                self.domain_class_overrides.remove(&key);
            }
        }
        self
    }

    /// Replace the full override map at once.
    #[must_use]
    pub fn with_overrides(
        mut self,
        overrides: HashMap<String, DomainClass>,
    ) -> Self {
        self.domain_class_overrides = overrides;
        self
    }

    /// Set whether contracts without a signature are rejected.
    #[must_use]
    pub const fn with_signature_required(mut self, required: bool) -> Self {
        self.signature_required = required;
        self
    }

    /// Resolve the [`DomainClass`] for a host.
    ///
    /// Lookup walks the override map first, then falls back to
    /// heuristics that recognise well-known challenge issuers
    /// (`captcha`, `challenge`, `auth`, `login`, `accounts`,
    /// `payment`, `checkout`) as [`DomainClass::Sensitive`].
    #[must_use]
    pub fn class_for(&self, host: &str) -> DomainClass {
        let key = host.trim().to_ascii_lowercase();
        if let Some(class) = self.domain_class_overrides.get(&key).copied() {
            return class;
        }
        heuristic_class(&key)
    }

    /// Convenience wrapper that returns the [`DomainClass`] for `host`
    /// via [`Self::class_for`].
    #[must_use]
    pub fn for_domain(&self, host: &str) -> DomainClass {
        self.class_for(host)
    }

    /// Max-age in milliseconds for a given class.
    #[must_use]
    pub const fn max_age_ms_for(&self, class: DomainClass) -> u64 {
        match class {
            DomainClass::Default => self.default_max_age_ms,
            DomainClass::Hostile => self.hostile_max_age_ms,
            DomainClass::Authenticated => self.authenticated_max_age_ms,
            DomainClass::Sensitive => self.sensitive_max_age_ms,
        }
    }

    /// Max-age as a [`Duration`] for `host`.
    #[must_use]
    pub fn max_age_for(&self, host: &str) -> Duration {
        Duration::from_millis(self.max_age_ms_for(self.class_for(host)))
    }

    /// Build a contract for `host` capturing the current wall-clock
    /// and `signature` (when known). The max-age is resolved via
    /// [`Self::max_age_for`].
    ///
    /// # Errors
    ///
    /// Returns [`FreshnessError::InvalidContract`] when `host` is empty
    /// or `signature` is empty.
    pub fn build_contract(
        &self,
        host: &str,
        signature: Option<&str>,
    ) -> Result<FreshnessContract, FreshnessError> {
        let class = self.class_for(host);
        let max_age_ms = self.max_age_ms_for(class);
        FreshnessContract::with_signature(
            host,
            signature.unwrap_or(""),
            unix_epoch_ms(),
            Duration::from_millis(max_age_ms),
            self.kind,
        )
        .map(|mut c| {
            c.domain_class = class;
            c
        })
    }
}

fn heuristic_class(host: &str) -> DomainClass {
    const SENSITIVE_TOKENS: &[&str] = &[
        "captcha",
        "challenge",
        "auth",
        "login",
        "signin",
        "accounts",
        "payment",
        "checkout",
        "verify",
    ];
    const HOSTILE_TOKENS: &[&str] = &["cloudflare", "datadome", "perimeter", "akamai", "kasada"];

    for token in SENSITIVE_TOKENS {
        if host.contains(token) {
            return DomainClass::Sensitive;
        }
    }
    for token in HOSTILE_TOKENS {
        if host.contains(token) {
            return DomainClass::Hostile;
        }
    }
    DomainClass::Default
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)]
const fn scale_ms(value: u64, factor: f64) -> u64 {
    let scaled = (value as f64) * factor;
    if !scaled.is_finite() || scaled <= 0.0 {
        1
    } else if scaled > u64::MAX as f64 {
        u64::MAX
    } else {
        scaled as u64
    }
}

// ─── Contract ─────────────────────────────────────────────────────────────────

/// A freshness contract describing the origin and constraints of an
/// identity artifact.
///
/// Capture time, target domain, optional signature hash, and the
/// resolved max-age are all bound at the point of capture so a later
/// [`check`] can detect any of:
///
/// - TTL expiration (`now - captured_at > max_age`)
/// - Signature rotation (`signature` field mismatch)
/// - Domain rebinding (`domain` field mismatch)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshnessContract {
    /// Lower-cased host the contract was bound to (e.g. `"example.com"`).
    pub domain: String,
    /// Optional opaque signature hash (e.g. `"sha256:abc…"`) the
    /// contract was bound to. `None` when signatures are not used.
    pub signature_hash: Option<String>,
    /// Unix epoch milliseconds when the contract was captured.
    pub captured_at_epoch_ms: u64,
    /// Resolved max-age for this contract.
    #[serde(with = "duration_ms")]
    pub max_age: Duration,
    /// Policy band used to resolve `max_age`.
    pub policy_kind: FreshnessPolicyKind,
    /// Domain class used to resolve `max_age`.
    pub domain_class: DomainClass,
}

impl FreshnessContract {
    /// Build a contract with explicit fields.
    ///
    /// # Errors
    ///
    /// Returns [`FreshnessError::InvalidContract`] when `domain` is
    /// empty after trimming, when `max_age` is zero, or when
    /// `signature` is provided but empty.
    pub fn with_signature(
        domain: &str,
        signature: &str,
        captured_at_epoch_ms: u64,
        max_age: Duration,
        policy_kind: FreshnessPolicyKind,
    ) -> Result<Self, FreshnessError> {
        let domain = domain.trim().to_ascii_lowercase();
        if domain.is_empty() {
            return Err(FreshnessError::InvalidContract(
                "domain must not be empty".to_string(),
            ));
        }
        if max_age.is_zero() {
            return Err(FreshnessError::InvalidContract(
                "max_age must be > 0".to_string(),
            ));
        }
        let signature_hash = if signature.is_empty() {
            None
        } else {
            Some(signature.to_string())
        };
        Ok(Self {
            domain,
            signature_hash,
            captured_at_epoch_ms,
            max_age,
            policy_kind,
            domain_class: DomainClass::Default,
        })
    }

    /// Build a contract without a signature.
    ///
    /// # Errors
    ///
    /// Returns [`FreshnessError::InvalidContract`] when `domain` is
    /// empty after trimming or when `max_age` is zero.
    pub fn without_signature(
        domain: &str,
        captured_at_epoch_ms: u64,
        max_age: Duration,
        policy_kind: FreshnessPolicyKind,
    ) -> Result<Self, FreshnessError> {
        Self::with_signature(domain, "", captured_at_epoch_ms, max_age, policy_kind)
    }

    /// Convenience constructor that captures the current wall-clock.
    ///
    /// # Errors
    ///
    /// See [`Self::with_signature`].
    pub fn capture_now(
        domain: &str,
        signature: Option<&str>,
        max_age: Duration,
        policy_kind: FreshnessPolicyKind,
    ) -> Result<Self, FreshnessError> {
        Self::with_signature(
            domain,
            signature.unwrap_or(""),
            unix_epoch_ms(),
            max_age,
            policy_kind,
        )
    }

    /// Resolved max-age in milliseconds.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_lossless)]
    pub const fn max_age_ms(&self) -> u64 {
        // Duration::as_millis returns u128; clamp to u64 for telemetry keys.
        let v = self.max_age.as_millis();
        if v > u64::MAX as u128 {
            u64::MAX
        } else {
            v as u64
        }
    }
}

// serde helper: serialise Duration as integer milliseconds
mod duration_ms {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    #[allow(clippy::cast_possible_truncation)]
    pub fn serialize<S: Serializer>(value: &Duration, ser: S) -> Result<S::Ok, S::Error> {
        let ms = value.as_millis();
        let n = if ms > u128::from(u64::MAX) {
            u64::MAX
        } else {
            ms as u64
        };
        ser.serialize_u64(n)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Duration, D::Error> {
        let ms = u64::deserialize(de)?;
        Ok(Duration::from_millis(ms))
    }
}

// ─── Decision ─────────────────────────────────────────────────────────────────

/// Structured reason a freshness contract was invalidated.
///
/// All fields are populated regardless of which rule fired so
/// telemetry always carries the full context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvalidationReason {
    /// Contract's bound domain (lower-case).
    pub contract_domain: String,
    /// Observed domain passed to [`check`].
    pub observed_domain: String,
    /// Contract's bound signature (when set).
    pub contract_signature: Option<String>,
    /// Observed signature passed to [`check`] (when set).
    pub observed_signature: Option<String>,
    /// Contract's captured-at timestamp.
    pub captured_at_epoch_ms: u64,
    /// Observed timestamp passed to [`check`].
    pub observed_at_epoch_ms: u64,
    /// Elapsed milliseconds between capture and observation.
    pub elapsed_ms: u64,
    /// Contract's max-age in milliseconds.
    pub max_age_ms: u64,
    /// Policy band used.
    pub policy_kind: FreshnessPolicyKind,
    /// Domain class used.
    pub domain_class: DomainClass,
    /// Stable machine-readable reason tag.
    pub kind: InvalidationKind,
}

/// Machine-readable reason tag attached to [`InvalidationReason`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidationKind {
    /// Elapsed since capture exceeded max-age.
    StaleTtl,
    /// Signature hash did not match.
    SignatureMismatch,
    /// Target domain did not match the contract's domain.
    DomainMismatch,
    /// Contract had no signature but policy requires one.
    SignatureMissing,
}

impl InvalidationKind {
    /// Stable string label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StaleTtl => "stale_ttl",
            Self::SignatureMismatch => "signature_mismatch",
            Self::DomainMismatch => "domain_mismatch",
            Self::SignatureMissing => "signature_missing",
        }
    }
}

impl fmt::Display for InvalidationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Decision produced by [`check`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum FreshnessDecision {
    /// Contract is still valid for the observed context.
    Valid,
    /// Contract expired (`elapsed > max_age`).
    StaleTtl {
        /// Structured invalidation reason.
        reason: Box<InvalidationReason>,
    },
    /// Signature hash did not match the observed value.
    SignatureMismatch {
        /// Structured invalidation reason.
        reason: Box<InvalidationReason>,
    },
    /// Target domain did not match the contract's domain.
    DomainMismatch {
        /// Structured invalidation reason.
        reason: Box<InvalidationReason>,
    },
}

impl FreshnessDecision {
    /// `true` when the contract is valid.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }

    /// `true` when the contract is invalid (any non-Valid variant).
    #[must_use]
    pub const fn is_invalid(&self) -> bool {
        !self.is_valid()
    }

    /// Invalid [`InvalidationReason`] when the decision is non-Valid.
    #[must_use]
    pub fn reason(&self) -> Option<&InvalidationReason> {
        match self {
            Self::Valid => None,
            Self::StaleTtl { reason }
            | Self::SignatureMismatch { reason }
            | Self::DomainMismatch { reason } => Some(reason),
        }
    }

    /// Stable machine-readable label for the decision.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::StaleTtl { .. } => "stale_ttl",
            Self::SignatureMismatch { .. } => "signature_mismatch",
            Self::DomainMismatch { .. } => "domain_mismatch",
        }
    }
}

impl fmt::Display for FreshnessDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Valid => f.write_str("valid"),
            Self::StaleTtl { reason }
            | Self::SignatureMismatch { reason }
            | Self::DomainMismatch { reason } => {
                write!(f, "{} ({})", self.label(), reason.kind)
            }
        }
    }
}

// ─── Input ────────────────────────────────────────────────────────────────────

/// Observed context passed to [`check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshnessCheckInput {
    /// Lower-cased target host observed at reuse time.
    pub observed_domain: String,
    /// Lower-cased observed signature hash, when available.
    pub observed_signature: Option<String>,
    /// Observation timestamp (Unix epoch ms).
    pub observed_at_epoch_ms: u64,
}

impl FreshnessCheckInput {
    /// Build a check input.
    #[must_use]
    pub fn new(
        observed_domain: &str,
        observed_signature: Option<&str>,
        observed_at_epoch_ms: u64,
    ) -> Self {
        let observed_domain = observed_domain.trim().to_ascii_lowercase();
        let observed_signature = observed_signature
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        Self {
            observed_domain,
            observed_signature,
            observed_at_epoch_ms,
        }
    }

    /// Build an input capturing the current wall-clock for `host`.
    #[must_use]
    pub fn capture_now(observed_domain: &str, observed_signature: Option<&str>) -> Self {
        Self::new(observed_domain, observed_signature, unix_epoch_ms())
    }
}

// ─── Freshness check ──────────────────────────────────────────────────────────

/// Evaluate `contract` against `input`, returning a deterministic
/// [`FreshnessDecision`].
///
/// Precedence:
///
/// 1. Domain mismatch is checked first (cheap, structural).
/// 2. Signature missing-when-required is checked next.
/// 3. Signature mismatch is checked before TTL so a rotated signature
///    never silently slips through on an unexpired contract.
/// 4. TTL (elapsed > max-age) is the last gate.
///
/// The decision is fully determined by `(contract, input)` — no I/O,
/// no clock reads.
#[must_use]
pub fn check(contract: &FreshnessContract, input: &FreshnessCheckInput) -> FreshnessDecision {
    let elapsed_ms = input
        .observed_at_epoch_ms
        .saturating_sub(contract.captured_at_epoch_ms);

    // 1. Domain mismatch
    if contract.domain != input.observed_domain {
        return FreshnessDecision::DomainMismatch {
            reason: Box::new(InvalidationReason {
                contract_domain: contract.domain.clone(),
                observed_domain: input.observed_domain.clone(),
                contract_signature: contract.signature_hash.clone(),
                observed_signature: input.observed_signature.clone(),
                captured_at_epoch_ms: contract.captured_at_epoch_ms,
                observed_at_epoch_ms: input.observed_at_epoch_ms,
                elapsed_ms,
                max_age_ms: contract.max_age_ms(),
                policy_kind: contract.policy_kind,
                domain_class: contract.domain_class,
                kind: InvalidationKind::DomainMismatch,
            }),
        };
    }

    // 2. Signature required but missing
    if contract.signature_hash.is_none() && input.observed_signature.is_some() {
        return FreshnessDecision::SignatureMismatch {
            reason: Box::new(InvalidationReason {
                contract_domain: contract.domain.clone(),
                observed_domain: input.observed_domain.clone(),
                contract_signature: contract.signature_hash.clone(),
                observed_signature: input.observed_signature.clone(),
                captured_at_epoch_ms: contract.captured_at_epoch_ms,
                observed_at_epoch_ms: input.observed_at_epoch_ms,
                elapsed_ms,
                max_age_ms: contract.max_age_ms(),
                policy_kind: contract.policy_kind,
                domain_class: contract.domain_class,
                kind: InvalidationKind::SignatureMissing,
            }),
        };
    }

    // 3. Signature mismatch
    if let (Some(expected), Some(observed)) =
        (&contract.signature_hash, &input.observed_signature)
        && expected != observed
    {
        return FreshnessDecision::SignatureMismatch {
            reason: Box::new(InvalidationReason {
                contract_domain: contract.domain.clone(),
                observed_domain: input.observed_domain.clone(),
                contract_signature: Some(expected.clone()),
                observed_signature: Some(observed.clone()),
                captured_at_epoch_ms: contract.captured_at_epoch_ms,
                observed_at_epoch_ms: input.observed_at_epoch_ms,
                elapsed_ms,
                max_age_ms: contract.max_age_ms(),
                policy_kind: contract.policy_kind,
                domain_class: contract.domain_class,
                kind: InvalidationKind::SignatureMismatch,
            }),
        };
    }

    // 4. TTL
    if elapsed_ms > contract.max_age_ms() {
        return FreshnessDecision::StaleTtl {
            reason: Box::new(InvalidationReason {
                contract_domain: contract.domain.clone(),
                observed_domain: input.observed_domain.clone(),
                contract_signature: contract.signature_hash.clone(),
                observed_signature: input.observed_signature.clone(),
                captured_at_epoch_ms: contract.captured_at_epoch_ms,
                observed_at_epoch_ms: input.observed_at_epoch_ms,
                elapsed_ms,
                max_age_ms: contract.max_age_ms(),
                policy_kind: contract.policy_kind,
                domain_class: contract.domain_class,
                kind: InvalidationKind::StaleTtl,
            }),
        };
    }

    FreshnessDecision::Valid
}

// ─── Telemetry helper ─────────────────────────────────────────────────────────

/// Compact freshness report attached to acquisition results and
/// emitted via `tracing`.
///
/// Includes both the decision and (when invalidated) the structured
/// reason fields, so downstream automation can attribute rejections
/// without re-parsing log strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshnessReport {
    /// Resolved decision for this run.
    pub decision: FreshnessDecision,
    /// Resolved [`DomainClass`] for the target host.
    pub domain_class: DomainClass,
    /// Policy band used.
    pub policy_kind: FreshnessPolicyKind,
    /// Whether the contract was considered (vs. no contract supplied).
    pub contract_evaluated: bool,
}

impl FreshnessReport {
    /// A no-contract report (`Valid`, no evaluation performed).
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn skipped(policy_kind: FreshnessPolicyKind, domain_class: DomainClass) -> Self {
        Self {
            decision: FreshnessDecision::Valid,
            domain_class,
            policy_kind,
            contract_evaluated: false,
        }
    }

    /// Build a report from a contract + input pair.
    #[must_use]
    pub fn evaluate(
        contract: &FreshnessContract,
        input: &FreshnessCheckInput,
    ) -> Self {
        Self {
            decision: check(contract, input),
            domain_class: contract.domain_class,
            policy_kind: contract.policy_kind,
            contract_evaluated: true,
        }
    }

    /// Emit a structured `tracing` event for this report.
    pub fn log(&self) {
        match &self.decision {
            FreshnessDecision::Valid => {
                if self.contract_evaluated {
                    tracing::debug!(
                        target: "stygian::freshness",
                        decision = self.decision.label(),
                        domain_class = self.domain_class.label(),
                        policy = policy_label(self.policy_kind),
                        "freshness contract is valid",
                    );
                }
            }
            FreshnessDecision::StaleTtl { reason }
            | FreshnessDecision::SignatureMismatch { reason }
            | FreshnessDecision::DomainMismatch { reason } => {
                tracing::warn!(
                    target: "stygian::freshness",
                    decision = self.decision.label(),
                    invalidation_reason = reason.kind.as_str(),
                    contract_domain = %reason.contract_domain,
                    observed_domain = %reason.observed_domain,
                    contract_signature = reason.contract_signature.as_deref().unwrap_or(""),
                    observed_signature = reason.observed_signature.as_deref().unwrap_or(""),
                    captured_at_epoch_ms = reason.captured_at_epoch_ms,
                    observed_at_epoch_ms = reason.observed_at_epoch_ms,
                    elapsed_ms = reason.elapsed_ms,
                    max_age_ms = reason.max_age_ms,
                    domain_class = self.domain_class.label(),
                    policy = policy_label(self.policy_kind),
                    "freshness contract invalidated",
                );
            }
        }
    }
}

const fn policy_label(kind: FreshnessPolicyKind) -> &'static str {
    match kind {
        FreshnessPolicyKind::Strict => "strict",
        FreshnessPolicyKind::Standard => "standard",
        FreshnessPolicyKind::Permissive => "permissive",
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Current Unix epoch in milliseconds, clamped to `u64`.
///
/// Saturates to `0` if the clock is before the epoch (theoretical).
#[must_use]
pub fn unix_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(Duration::ZERO, |d| d)
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// Produce a stable, low-cost signature hash for an arbitrary list of
/// string fields.
///
/// Returns a `"fnv64:<hex>"` string suitable for use as a
/// [`FreshnessContract::signature_hash`]. The function is pure and
/// deterministic — equal inputs always produce the same output.
///
/// # Example
///
/// ```
/// use stygian_browser::freshness::signature_hash;
///
/// let h = signature_hash(&["example.com", "MacIntel", "1920x1080"]);
/// assert!(h.starts_with("fnv64:"));
/// assert_eq!(h, signature_hash(&["example.com", "MacIntel", "1920x1080"]));
/// ```
#[must_use]
pub fn signature_hash(parts: &[&str]) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
        // separator: 0x1f (unit separator)
        hash ^= 0x1f;
        hash = hash.wrapping_mul(PRIME);
    }
    format!("fnv64:{hash:016x}")
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const CAPTURED_AT: u64 = 1_700_000_000_000;

    fn contract(max_age_ms: u64, sig: Option<&str>) -> FreshnessContract {
        FreshnessContract::with_signature(
            "example.com",
            sig.unwrap_or(""),
            CAPTURED_AT,
            Duration::from_millis(max_age_ms),
            FreshnessPolicyKind::Standard,
        )
        .expect("valid contract")
    }

    fn input(observed_at_ms: u64, sig: Option<&str>) -> FreshnessCheckInput {
        FreshnessCheckInput::new("example.com", sig, observed_at_ms)
    }

    #[test]
    fn ttl_invalidates_past_max_age() {
        let c = contract(1_000, Some("sha256:abc"));
        // 2_000ms after capture -> 1_000ms past max_age -> stale
        let decision = check(&c, &input(CAPTURED_AT + 2_000, Some("sha256:abc")));
        assert!(matches!(
            decision,
            FreshnessDecision::StaleTtl { ref reason } if reason.kind == InvalidationKind::StaleTtl
        ));
    }

    #[test]
    fn ttl_holds_within_max_age() {
        let c = contract(60_000, Some("sha256:abc"));
        let decision = check(&c, &input(CAPTURED_AT + 30_000, Some("sha256:abc")));
        assert!(decision.is_valid());
    }

    #[test]
    fn signature_mismatch_invalidates_even_when_within_ttl() {
        let c = contract(60_000, Some("sha256:abc"));
        let decision = check(&c, &input(CAPTURED_AT + 1_000, Some("sha256:xyz")));
        match decision {
            FreshnessDecision::SignatureMismatch { reason } => {
                assert_eq!(reason.kind, InvalidationKind::SignatureMismatch);
                assert_eq!(reason.contract_signature.as_deref(), Some("sha256:abc"));
                assert_eq!(reason.observed_signature.as_deref(), Some("sha256:xyz"));
            }
            other => panic!("expected SignatureMismatch, got {other:?}"),
        }
    }

    #[test]
    fn domain_mismatch_takes_precedence_over_ttl() {
        let c = contract(60_000, Some("sha256:abc"));
        let input = FreshnessCheckInput::new("other.example", Some("sha256:abc"), CAPTURED_AT);
        let decision = check(&c, &input);
        match decision {
            FreshnessDecision::DomainMismatch { reason } => {
                assert_eq!(reason.kind, InvalidationKind::DomainMismatch);
                assert_eq!(reason.contract_domain, "example.com");
                assert_eq!(reason.observed_domain, "other.example");
            }
            other => panic!("expected DomainMismatch, got {other:?}"),
        }
    }

    #[test]
    fn missing_signature_when_required_rejects() {
        let policy = FreshnessPolicy {
            signature_required: true,
            ..FreshnessPolicy::default()
        };
        // Re-classify as sensitive to also test class plumbing
        let policy = policy.with_domain_override("example.com", Some(DomainClass::Sensitive));
        assert!(policy.signature_required);
        assert_eq!(
            policy.class_for("example.com"),
            DomainClass::Sensitive
        );
        // Build a contract without signature
        let c = FreshnessContract::without_signature(
            "example.com",
            CAPTURED_AT,
            policy.max_age_for("example.com"),
            policy.kind,
        )
        .expect("contract");
        let observed_with_sig = input(CAPTURED_AT + 1_000, Some("sha256:abc"));
        let decision = check(&c, &observed_with_sig);
        match decision {
            FreshnessDecision::SignatureMismatch { reason } => {
                assert_eq!(reason.kind, InvalidationKind::SignatureMissing);
            }
            other => panic!("expected SignatureMismatch (missing), got {other:?}"),
        }
    }

    #[test]
    fn determinism_same_inputs_same_decision() {
        let c = contract(60_000, Some("sha256:abc"));
        let i = input(CAPTURED_AT + 30_000, Some("sha256:abc"));
        let a = check(&c, &i);
        let b = check(&c, &i);
        assert_eq!(a, b);

        // Deterministic for invalid cases too
        let c2 = contract(1_000, Some("sha256:abc"));
        let i2 = input(CAPTURED_AT + 5_000, Some("sha256:abc"));
        let a = check(&c2, &i2);
        let b = check(&c2, &i2);
        assert_eq!(a, b);
    }

    #[test]
    fn signature_hash_is_deterministic_and_stable() {
        let h1 = signature_hash(&["a", "b", "c"]);
        let h2 = signature_hash(&["a", "b", "c"]);
        assert_eq!(h1, h2);
        assert!(h1.starts_with("fnv64:"));
        // Different inputs -> different hash
        assert_ne!(h1, signature_hash(&["a", "b", "d"]));
    }

    #[test]
    fn policy_tightening_reduces_max_age() {
        let p = FreshnessPolicy::default();
        let tightened = p.clone().tightened(0.5);
        assert!(tightened.default_max_age_ms < p.default_max_age_ms);
        assert!(tightened.sensitive_max_age_ms < p.sensitive_max_age_ms);
    }

    #[test]
    fn policy_class_overrides_win_over_heuristic() {
        let p = FreshnessPolicy::default()
            .with_domain_override("captcha.example", Some(DomainClass::Default))
            .with_domain_override("Friendly", Some(DomainClass::Hostile));
        // captcha would normally be Sensitive, but we overrode it to Default
        assert_eq!(p.class_for("captcha.example"), DomainClass::Default);
        // 'Friendly' would normally be Default but overridden to Hostile
        assert_eq!(p.class_for("friendly"), DomainClass::Hostile);
    }

    #[test]
    fn contract_rejects_empty_domain() {
        let err = FreshnessContract::with_signature(
            "",
            "sha256:abc",
            CAPTURED_AT,
            Duration::from_secs(1),
            FreshnessPolicyKind::Standard,
        )
        .unwrap_err();
        assert!(matches!(err, FreshnessError::InvalidContract(_)));
    }

    #[test]
    fn contract_rejects_zero_max_age() {
        let err = FreshnessContract::with_signature(
            "example.com",
            "sha256:abc",
            CAPTURED_AT,
            Duration::ZERO,
            FreshnessPolicyKind::Standard,
        )
        .unwrap_err();
        assert!(matches!(err, FreshnessError::InvalidContract(_)));
    }

    #[test]
    fn report_logs_skip_when_no_contract() {
        let report = FreshnessReport::skipped(FreshnessPolicyKind::Standard, DomainClass::Default);
        assert!(report.decision.is_valid());
        assert!(!report.contract_evaluated);
    }

    #[test]
    fn domain_class_label_is_stable() {
        assert_eq!(DomainClass::Default.label(), "default");
        assert_eq!(DomainClass::Hostile.label(), "hostile");
        assert_eq!(DomainClass::Authenticated.label(), "authenticated");
        assert_eq!(DomainClass::Sensitive.label(), "sensitive");
    }

    #[test]
    fn json_roundtrip_preserves_contract() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let c = contract(60_000, Some("sha256:abc"));
        let json = serde_json::to_string(&c)?;
        let back: FreshnessContract = serde_json::from_str(&json)?;
        assert_eq!(c, back);
        Ok(())
    }
}
