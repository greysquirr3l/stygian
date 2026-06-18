//! Adaptive session replay defense mode for browser identity.
//!
//! ## What is "replay-style" anti-bot detection?
//!
//! Several anti-bot vendors record a session (TLS handshake + a
//! sequence of navigations + identity surface values) and **replay**
//! the same artifacts later. If the same fingerprint, nonce, or
//! challenge response shows up in two different sessions (or after a
//! site-side rotation), the vendor flags the second session as a
//! replay. This is distinct from
//! [coherence drift][crate::coherence] (one session has multiple
//! inconsistent identity surfaces) — replay detection specifically
//! looks at **session-lifetime artifacts**:
//!
//! - The session **nonce** issued by a challenge / challenge-response
//!   endpoint.
//! - The browser **fingerprint** captured at session start.
//! - The **age** of the session.
//!
//! ## How this module helps
//!
//! [`ReplayDefensePolicy`] captures three orthogonal levers the
//! runner can use to defeat replay-style detection:
//!
//! - [`ReplayDefensePolicy::rotation_interval`] — maximum session
//!   age before the runner must rotate the browser.
//! - [`ReplayDefensePolicy::nonce_validity_window`] — maximum
//!   age of a session nonce. After this window the nonce is no
//!   longer trustworthy and the session must be reset.
//! - [`ReplayDefensePolicy::force_reset_on_drift`] — when `true`,
//!   signature drift (the fingerprint captured at session start
//!   no longer matches the freshly observed one) triggers a forced
//!   refresh of the sticky browser context.
//!
//! The deterministic [`check`] function evaluates a
//! [`ReplayDefenseState`] (the per-session record) against a
//! [`ReplayDefenseCheckInput`] (the observed context) and returns a
//! [`ReplayDefenseDecision`]. The decision is purely derived from
//! the inputs — no I/O, no clock reads — so unit tests can exercise
//! the full state space.
//!
//! ## Integration with `AcquisitionRunner`
//!
//! [`AcquisitionRequest::replay_defense`][crate::acquisition::AcquisitionRequest::replay_defense]
//! carries the live context (policy + state) into the runner. The
//! runner evaluates the policy before any stage executes:
//!
//! 1. The decision is logged via [`ReplayDefenseReport::log`].
//! 2. If the decision is invalid **and** `force_reset_on_drift` is
//!    `true`, the runner calls
//!    [`BrowserPool::release_context`][crate::pool::BrowserPool::release_context]
//!    to invalidate the sticky session, then short-circuits with a
//!    [`StageFailureKind::Setup`][crate::acquisition::StageFailureKind::Setup]
//!    failure tagged `replay_defense_forced_refresh`.
//! 3. The full report is attached to the
//!    [`AcquisitionResult::replay_defense`][crate::acquisition::AcquisitionResult::replay_defense]
//!    field so downstream automation can attribute the rejection.
//!
//! ## Feature flag
//!
//! This module is **default-on** and is always compiled as part of
//! the `stygian-browser` crate. No new feature gate is introduced.
//!
//! ## Default policy
//!
//! [`ReplayDefensePolicy::default`] returns a deterministic
//! baseline that is safe to ship in production:
//!
//! - `rotation_interval` = `1800 s` (30 min)
//! - `nonce_validity_window` = `300 s` (5 min)
//! - `force_reset_on_drift` = `true`
//!
//! Callers can override any field via the with-builder methods or by
//! deserialising a config from JSON / TOML.
//!
//! # Example
//!
//! ```
//! use stygian_browser::replay_defense::{
//!     ReplayDefenseCheckInput, ReplayDefenseDecision, ReplayDefensePolicy,
//!     ReplayDefenseState, check,
//! };
//! use std::time::Duration;
//!
//! let policy = ReplayDefensePolicy::default();
//! let captured = stygian_browser::freshness::unix_epoch_ms();
//! let state = ReplayDefenseState::with_fingerprint(
//!     "example.com",
//!     "sha256:abc",
//!     Some("nonce-001"),
//!     captured,
//! );
//!
//! let observed = ReplayDefenseCheckInput::new(
//!     "example.com",
//!     Some("sha256:abc"),
//!     Some("nonce-001"),
//!     captured + 1_000,
//! );
//! let decision = check(&policy, &state, &observed);
//! assert!(decision.is_valid());
//! ```

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── Error type ───────────────────────────────────────────────────────────────

/// Errors produced by replay-defense policy / state construction.
#[derive(Debug, Error)]
pub enum ReplayDefenseError {
    /// Field carried an invalid value (e.g. zero rotation interval).
    #[error("invalid replay defense field: {0}")]
    InvalidField(String),
    /// Failed to (de)serialise a replay-defense type.
    #[error("failed to (de)serialise replay defense field: {0}")]
    Serialization(String),
}

impl From<serde_json::Error> for ReplayDefenseError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err.to_string())
    }
}

// ─── Policy ───────────────────────────────────────────────────────────────────

/// Adaptive session-replay defense policy.
///
/// The three levers are independent and the runner combines them
/// to decide when a session must be rotated or reset:
///
/// - [`rotation_interval`](Self::rotation_interval) — max age of a
///   session before it is forcibly rotated.
/// - [`nonce_validity_window`](Self::nonce_validity_window) — max age
///   of the **session nonce** the challenge / challenge-response
///   endpoint issued.
/// - [`force_reset_on_drift`](Self::force_reset_on_drift) — whether
///   signature drift triggers an immediate session reset.
///
/// # Example
///
/// ```
/// use stygian_browser::replay_defense::ReplayDefensePolicy;
/// use std::time::Duration;
///
/// let policy = ReplayDefensePolicy {
///     rotation_interval: Duration::from_mins(15),
///     ..ReplayDefensePolicy::default()
/// };
/// assert_eq!(policy.rotation_interval, Duration::from_mins(15));
/// assert!(policy.force_reset_on_drift);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayDefensePolicy {
    /// Maximum age of a session before a forced rotation. The
    /// `check` function emits [`ReplayDefenseDecision::RotationDue`]
    /// once `elapsed >= rotation_interval`.
    pub rotation_interval: Duration,
    /// Maximum age of a session **nonce**. After this window the
    /// nonce is no longer trustworthy and the session must be
    /// re-bound to a fresh nonce. The `check` function emits
    /// [`ReplayDefenseDecision::NonceExpired`] once
    /// `nonce_age >= nonce_validity_window`.
    #[serde(with = "duration_ms")]
    pub nonce_validity_window: Duration,
    /// When `true`, signature drift (`observed_signature !=
    /// captured_signature`) triggers a forced refresh of the sticky
    /// browser context. When `false`, drift is reported but the
    /// runner continues.
    pub force_reset_on_drift: bool,
}

impl Default for ReplayDefensePolicy {
    fn default() -> Self {
        Self {
            rotation_interval: Duration::from_mins(30),
            nonce_validity_window: Duration::from_mins(5),
            force_reset_on_drift: true,
        }
    }
}

impl ReplayDefensePolicy {
    /// Build a policy with explicit rotation interval and defaults
    /// for the other fields.
    #[must_use]
    pub const fn with_rotation_interval(rotation_interval: Duration) -> Self {
        Self {
            rotation_interval,
            nonce_validity_window: Duration::from_mins(5),
            force_reset_on_drift: true,
        }
    }

    /// Build a policy with an explicit nonce validity window and
    /// defaults for the other fields.
    #[must_use]
    pub const fn with_nonce_validity_window(nonce_validity_window: Duration) -> Self {
        Self {
            rotation_interval: Duration::from_mins(30),
            nonce_validity_window,
            force_reset_on_drift: true,
        }
    }

    /// Replace the rotation interval.
    #[must_use]
    pub const fn with_rotation(mut self, rotation_interval: Duration) -> Self {
        self.rotation_interval = rotation_interval;
        self
    }

    /// Replace the nonce validity window.
    #[must_use]
    pub const fn with_nonce_window(mut self, nonce_validity_window: Duration) -> Self {
        self.nonce_validity_window = nonce_validity_window;
        self
    }

    /// Replace the `force_reset_on_drift` flag.
    #[must_use]
    pub const fn with_force_reset_on_drift(mut self, force: bool) -> Self {
        self.force_reset_on_drift = force;
        self
    }

    /// Validate the policy. `rotation_interval` and
    /// `nonce_validity_window` must be strictly positive.
    ///
    /// # Errors
    ///
    /// Returns [`ReplayDefenseError::InvalidField`] when either
    /// interval is zero.
    pub fn validate(&self) -> Result<(), ReplayDefenseError> {
        if self.rotation_interval.is_zero() {
            return Err(ReplayDefenseError::InvalidField(
                "rotation_interval must be > 0".to_string(),
            ));
        }
        if self.nonce_validity_window.is_zero() {
            return Err(ReplayDefenseError::InvalidField(
                "nonce_validity_window must be > 0".to_string(),
            ));
        }
        Ok(())
    }
}

// ─── State ────────────────────────────────────────────────────────────────────

/// Per-session replay-defense state captured when the session was
/// first bound.
///
/// The state is the **frozen record** the runner compares against
/// the freshly observed context on every reuse. It is fully
/// serialisable so the existing session snapshot path
/// ([`SessionSnapshot`][crate::session::SessionSnapshot] in
/// Browser T23) can persist and reload it across restarts.
///
/// # Example
///
/// ```
/// use stygian_browser::replay_defense::ReplayDefenseState;
///
/// let captured = stygian_browser::freshness::unix_epoch_ms();
/// let state = ReplayDefenseState::with_fingerprint(
///     "example.com",
///     "sha256:abc",
///     Some("nonce-001"),
///     captured,
/// );
/// assert_eq!(state.domain, "example.com");
/// assert_eq!(state.signature.as_deref(), Some("sha256:abc"));
/// assert_eq!(state.nonce.as_deref(), Some("nonce-001"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayDefenseState {
    /// Lower-cased host the state was bound to.
    pub domain: String,
    /// Fingerprint signature captured at session start.
    pub signature: Option<String>,
    /// Session nonce issued at session start (when applicable).
    pub nonce: Option<String>,
    /// Unix epoch milliseconds when the state was captured.
    pub captured_at_epoch_ms: u64,
}

impl ReplayDefenseState {
    /// Build a state record.
    #[must_use]
    pub fn new(
        domain: &str,
        signature: Option<&str>,
        nonce: Option<&str>,
        captured_at_epoch_ms: u64,
    ) -> Self {
        Self {
            domain: domain.trim().to_ascii_lowercase(),
            signature: signature.map(str::to_string).filter(|s| !s.is_empty()),
            nonce: nonce.map(str::to_string).filter(|s| !s.is_empty()),
            captured_at_epoch_ms,
        }
    }

    /// Build a state record that captures a fingerprint + nonce for
    /// `domain` at the current wall-clock.
    #[must_use]
    pub fn with_fingerprint(
        domain: &str,
        signature: &str,
        nonce: Option<&str>,
        captured_at_epoch_ms: u64,
    ) -> Self {
        Self::new(
            domain,
            if signature.is_empty() {
                None
            } else {
                Some(signature)
            },
            nonce,
            captured_at_epoch_ms,
        )
    }

    /// Build a state record at the current wall-clock.
    #[must_use]
    pub fn capture_now(domain: &str, signature: Option<&str>, nonce: Option<&str>) -> Self {
        Self::new(domain, signature, nonce, unix_epoch_ms())
    }
}

// ─── Input ────────────────────────────────────────────────────────────────────

/// Observed context passed to [`check`] on every session reuse.
///
/// Mirrors [`crate::freshness::FreshnessCheckInput`] but adds the
/// `observed_nonce` field, which is the nonce the application
/// currently has in flight (or `None` when the host never issued
/// one).
///
/// # Example
///
/// ```
/// use stygian_browser::replay_defense::ReplayDefenseCheckInput;
///
/// let input = ReplayDefenseCheckInput::new(
///     "example.com",
///     Some("sha256:abc"),
///     Some("nonce-001"),
///     1_700_000_000_000,
/// );
/// assert_eq!(input.observed_domain, "example.com");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayDefenseCheckInput {
    /// Lower-cased target host observed at reuse time.
    pub observed_domain: String,
    /// Lower-cased observed signature, when available.
    pub observed_signature: Option<String>,
    /// Observed session nonce, when the host has issued one.
    pub observed_nonce: Option<String>,
    /// Observation timestamp (Unix epoch ms).
    pub observed_at_epoch_ms: u64,
}

impl ReplayDefenseCheckInput {
    /// Build a check input from raw fields. Empty strings are
    /// treated as `None`.
    #[must_use]
    pub fn new(
        observed_domain: &str,
        observed_signature: Option<&str>,
        observed_nonce: Option<&str>,
        observed_at_epoch_ms: u64,
    ) -> Self {
        Self {
            observed_domain: observed_domain.trim().to_ascii_lowercase(),
            observed_signature: observed_signature
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            observed_nonce: observed_nonce.filter(|s| !s.is_empty()).map(str::to_string),
            observed_at_epoch_ms,
        }
    }

    /// Build an input capturing the current wall-clock for `host`.
    #[must_use]
    pub fn capture_now(
        observed_domain: &str,
        observed_signature: Option<&str>,
        observed_nonce: Option<&str>,
    ) -> Self {
        Self::new(
            observed_domain,
            observed_signature,
            observed_nonce,
            unix_epoch_ms(),
        )
    }
}

// ─── Decision ─────────────────────────────────────────────────────────────────

/// Structured reason a replay-defense check invalidated a session.
///
/// Mirrors [`crate::freshness::InvalidationReason`] but covers the
/// replay-defense dimensions. Every field is populated regardless
/// of which rule fired so telemetry always carries the full
/// context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayDefenseReason {
    /// State's bound domain (lower-case).
    pub contract_domain: String,
    /// Observed domain passed to [`check`].
    pub observed_domain: String,
    /// State's bound signature (when set).
    pub contract_signature: Option<String>,
    /// Observed signature passed to [`check`] (when set).
    pub observed_signature: Option<String>,
    /// State's bound nonce (when set).
    pub contract_nonce: Option<String>,
    /// Observed nonce passed to [`check`] (when set).
    pub observed_nonce: Option<String>,
    /// State's captured-at timestamp.
    pub captured_at_epoch_ms: u64,
    /// Observed timestamp passed to [`check`].
    pub observed_at_epoch_ms: u64,
    /// Elapsed milliseconds between capture and observation.
    pub elapsed_ms: u64,
    /// Policy's rotation interval in milliseconds.
    pub rotation_interval_ms: u64,
    /// Policy's nonce validity window in milliseconds.
    pub nonce_validity_window_ms: u64,
    /// Whether `force_reset_on_drift` was set on the policy.
    pub force_reset_on_drift: bool,
    /// Stable machine-readable reason tag.
    pub kind: ReplayDefenseInvalidationKind,
}

/// Machine-readable reason tag attached to [`ReplayDefenseReason`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayDefenseInvalidationKind {
    /// Elapsed since capture exceeded the rotation interval.
    RotationDue,
    /// Nonce age exceeded the nonce validity window.
    NonceExpired,
    /// Nonce was rotated by the server (state nonce != observed nonce).
    NonceRotated,
    /// Observed domain did not match the state's domain.
    DomainMismatch,
    /// Signature hash did not match the state's bound signature.
    SignatureDrift,
}

impl ReplayDefenseInvalidationKind {
    /// Stable string label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RotationDue => "rotation_due",
            Self::NonceExpired => "nonce_expired",
            Self::NonceRotated => "nonce_rotated",
            Self::DomainMismatch => "domain_mismatch",
            Self::SignatureDrift => "signature_drift",
        }
    }
}

impl fmt::Display for ReplayDefenseInvalidationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Decision produced by [`check`].
///
/// The variants are tagged by their machine-readable
/// `outcome` field so the same JSON shape used by
/// [`crate::freshness::FreshnessDecision`] is preserved
/// for downstream automation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ReplayDefenseDecision {
    /// Session is still valid for the observed context.
    Valid,
    /// Elapsed since capture exceeded the rotation interval.
    RotationDue {
        /// Structured invalidation reason.
        reason: Box<ReplayDefenseReason>,
    },
    /// Session nonce age exceeded the validity window.
    NonceExpired {
        /// Structured invalidation reason.
        reason: Box<ReplayDefenseReason>,
    },
    /// Session nonce was rotated server-side.
    NonceRotated {
        /// Structured invalidation reason.
        reason: Box<ReplayDefenseReason>,
    },
    /// Observed domain did not match the state's bound domain.
    DomainMismatch {
        /// Structured invalidation reason.
        reason: Box<ReplayDefenseReason>,
    },
    /// Observed signature did not match the state's bound signature.
    SignatureDrift {
        /// Structured invalidation reason.
        reason: Box<ReplayDefenseReason>,
    },
}

impl ReplayDefenseDecision {
    /// `true` when the session is still valid.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }

    /// `true` when the session is invalid (any non-Valid variant).
    #[must_use]
    pub const fn is_invalid(&self) -> bool {
        !self.is_valid()
    }

    /// Invalid [`ReplayDefenseReason`] when the decision is non-Valid.
    #[must_use]
    pub fn reason(&self) -> Option<&ReplayDefenseReason> {
        match self {
            Self::Valid => None,
            Self::RotationDue { reason }
            | Self::NonceExpired { reason }
            | Self::NonceRotated { reason }
            | Self::DomainMismatch { reason }
            | Self::SignatureDrift { reason } => Some(reason),
        }
    }

    /// Stable machine-readable label.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::RotationDue { .. } => "rotation_due",
            Self::NonceExpired { .. } => "nonce_expired",
            Self::NonceRotated { .. } => "nonce_rotated",
            Self::DomainMismatch { .. } => "domain_mismatch",
            Self::SignatureDrift { .. } => "signature_drift",
        }
    }

    /// `true` when the policy mandates a forced refresh for this
    /// invalid decision. Used by the
    /// [`AcquisitionRunner`][crate::acquisition::AcquisitionRunner]
    /// to decide whether to call
    /// [`BrowserPool::release_context`][crate::pool::BrowserPool::release_context]
    /// and short-circuit the run.
    #[must_use]
    pub const fn requires_forced_refresh(&self, policy: &ReplayDefensePolicy) -> bool {
        if policy.force_reset_on_drift && matches!(self, Self::SignatureDrift { .. }) {
            return true;
        }
        // Rotation / nonce expiry always force a fresh session.
        matches!(
            self,
            Self::RotationDue { .. } | Self::NonceExpired { .. } | Self::NonceRotated { .. }
        )
    }
}

impl fmt::Display for ReplayDefenseDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Valid => f.write_str("valid"),
            Self::RotationDue { reason }
            | Self::NonceExpired { reason }
            | Self::NonceRotated { reason }
            | Self::DomainMismatch { reason }
            | Self::SignatureDrift { reason } => {
                write!(f, "{} ({})", self.label(), reason.kind)
            }
        }
    }
}

// ─── Check ────────────────────────────────────────────────────────────────────

/// Evaluate `policy` + `state` against `input` and return a
/// deterministic [`ReplayDefenseDecision`].
///
/// Precedence:
///
/// 1. Domain mismatch is checked first (cheap, structural).
/// 2. Signature drift is checked next so a rotated signature
///    never silently slips through on an unexpired session.
/// 3. Nonce rotation (state nonce != observed nonce) is checked
///    before the nonce age so a nonce that was explicitly rotated
///    is reported distinctly from a nonce that simply aged out.
/// 4. Nonce age (`nonce_age >= nonce_validity_window`).
/// 5. Rotation age (`elapsed >= rotation_interval`).
///
/// The decision is fully determined by `(policy, state, input)` —
/// no I/O, no clock reads.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn check(
    policy: &ReplayDefensePolicy,
    state: &ReplayDefenseState,
    input: &ReplayDefenseCheckInput,
) -> ReplayDefenseDecision {
    let elapsed_ms = input
        .observed_at_epoch_ms
        .saturating_sub(state.captured_at_epoch_ms);
    let rotation_interval_ms = duration_to_ms_u64(policy.rotation_interval);
    let nonce_validity_window_ms = duration_to_ms_u64(policy.nonce_validity_window);
    let nonce_age_ms = match (state.nonce.as_deref(), input.observed_nonce.as_deref()) {
        // Nonce present on both sides — the only meaningful "age"
        // is the elapsed time between capture and observation.
        (Some(_), Some(_)) => elapsed_ms,
        // No nonce tracked → nonce age is also elapsed time, but the
        // age check below will only fire if a nonce is in flight.
        _ => 0,
    };

    // 1. Domain mismatch
    if state.domain != input.observed_domain {
        return ReplayDefenseDecision::DomainMismatch {
            reason: Box::new(ReplayDefenseReason {
                contract_domain: state.domain.clone(),
                observed_domain: input.observed_domain.clone(),
                contract_signature: state.signature.clone(),
                observed_signature: input.observed_signature.clone(),
                contract_nonce: state.nonce.clone(),
                observed_nonce: input.observed_nonce.clone(),
                captured_at_epoch_ms: state.captured_at_epoch_ms,
                observed_at_epoch_ms: input.observed_at_epoch_ms,
                elapsed_ms,
                rotation_interval_ms,
                nonce_validity_window_ms,
                force_reset_on_drift: policy.force_reset_on_drift,
                kind: ReplayDefenseInvalidationKind::DomainMismatch,
            }),
        };
    }

    // 2. Signature drift
    if let (Some(expected), Some(observed)) = (&state.signature, &input.observed_signature)
        && expected != observed
    {
        return ReplayDefenseDecision::SignatureDrift {
            reason: Box::new(ReplayDefenseReason {
                contract_domain: state.domain.clone(),
                observed_domain: input.observed_domain.clone(),
                contract_signature: Some(expected.clone()),
                observed_signature: Some(observed.clone()),
                contract_nonce: state.nonce.clone(),
                observed_nonce: input.observed_nonce.clone(),
                captured_at_epoch_ms: state.captured_at_epoch_ms,
                observed_at_epoch_ms: input.observed_at_epoch_ms,
                elapsed_ms,
                rotation_interval_ms,
                nonce_validity_window_ms,
                force_reset_on_drift: policy.force_reset_on_drift,
                kind: ReplayDefenseInvalidationKind::SignatureDrift,
            }),
        };
    }

    // 3. Nonce rotation
    if let (Some(contract_nonce), Some(observed_nonce)) = (&state.nonce, &input.observed_nonce)
        && contract_nonce != observed_nonce
    {
        return ReplayDefenseDecision::NonceRotated {
            reason: Box::new(ReplayDefenseReason {
                contract_domain: state.domain.clone(),
                observed_domain: input.observed_domain.clone(),
                contract_signature: state.signature.clone(),
                observed_signature: input.observed_signature.clone(),
                contract_nonce: Some(contract_nonce.clone()),
                observed_nonce: Some(observed_nonce.clone()),
                captured_at_epoch_ms: state.captured_at_epoch_ms,
                observed_at_epoch_ms: input.observed_at_epoch_ms,
                elapsed_ms,
                rotation_interval_ms,
                nonce_validity_window_ms,
                force_reset_on_drift: policy.force_reset_on_drift,
                kind: ReplayDefenseInvalidationKind::NonceRotated,
            }),
        };
    }

    // 4. Nonce age (only meaningful when a nonce is in flight)
    if state.nonce.is_some() && nonce_age_ms > nonce_validity_window_ms {
        return ReplayDefenseDecision::NonceExpired {
            reason: Box::new(ReplayDefenseReason {
                contract_domain: state.domain.clone(),
                observed_domain: input.observed_domain.clone(),
                contract_signature: state.signature.clone(),
                observed_signature: input.observed_signature.clone(),
                contract_nonce: state.nonce.clone(),
                observed_nonce: input.observed_nonce.clone(),
                captured_at_epoch_ms: state.captured_at_epoch_ms,
                observed_at_epoch_ms: input.observed_at_epoch_ms,
                elapsed_ms,
                rotation_interval_ms,
                nonce_validity_window_ms,
                force_reset_on_drift: policy.force_reset_on_drift,
                kind: ReplayDefenseInvalidationKind::NonceExpired,
            }),
        };
    }

    // 5. Rotation age
    if elapsed_ms > rotation_interval_ms {
        return ReplayDefenseDecision::RotationDue {
            reason: Box::new(ReplayDefenseReason {
                contract_domain: state.domain.clone(),
                observed_domain: input.observed_domain.clone(),
                contract_signature: state.signature.clone(),
                observed_signature: input.observed_signature.clone(),
                contract_nonce: state.nonce.clone(),
                observed_nonce: input.observed_nonce.clone(),
                captured_at_epoch_ms: state.captured_at_epoch_ms,
                observed_at_epoch_ms: input.observed_at_epoch_ms,
                elapsed_ms,
                rotation_interval_ms,
                nonce_validity_window_ms,
                force_reset_on_drift: policy.force_reset_on_drift,
                kind: ReplayDefenseInvalidationKind::RotationDue,
            }),
        };
    }

    ReplayDefenseDecision::Valid
}

// ─── Report ───────────────────────────────────────────────────────────────────

/// Compact replay-defense report attached to acquisition results
/// and emitted via `tracing`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayDefenseReport {
    /// Resolved decision for this run.
    pub decision: ReplayDefenseDecision,
    /// Whether the state was considered (vs. no state supplied).
    pub state_evaluated: bool,
    /// Whether the runner was instructed to force a refresh.
    pub forced_refresh: bool,
}

impl ReplayDefenseReport {
    /// A no-state report (`Valid`, no evaluation performed, no forced refresh).
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn skipped() -> Self {
        Self {
            decision: ReplayDefenseDecision::Valid,
            state_evaluated: false,
            forced_refresh: false,
        }
    }

    /// Build a report from a policy / state / input triple.
    #[must_use]
    pub fn evaluate(
        policy: &ReplayDefensePolicy,
        state: &ReplayDefenseState,
        input: &ReplayDefenseCheckInput,
    ) -> Self {
        let decision = check(policy, state, input);
        let forced_refresh = decision.requires_forced_refresh(policy);
        Self {
            decision,
            state_evaluated: true,
            forced_refresh,
        }
    }

    /// Emit a structured `tracing` event for this report.
    pub fn log(&self) {
        match &self.decision {
            ReplayDefenseDecision::Valid => {
                if self.state_evaluated {
                    tracing::debug!(
                        target: "stygian::replay_defense",
                        decision = self.decision.label(),
                        forced_refresh = self.forced_refresh,
                        "replay defense state is valid",
                    );
                }
            }
            ReplayDefenseDecision::RotationDue { reason }
            | ReplayDefenseDecision::NonceExpired { reason }
            | ReplayDefenseDecision::NonceRotated { reason }
            | ReplayDefenseDecision::DomainMismatch { reason }
            | ReplayDefenseDecision::SignatureDrift { reason } => {
                tracing::warn!(
                    target: "stygian::replay_defense",
                    decision = self.decision.label(),
                    invalidation_reason = reason.kind.as_str(),
                    contract_domain = %reason.contract_domain,
                    observed_domain = %reason.observed_domain,
                    contract_signature = reason.contract_signature.as_deref().unwrap_or(""),
                    observed_signature = reason.observed_signature.as_deref().unwrap_or(""),
                    contract_nonce = reason.contract_nonce.as_deref().unwrap_or(""),
                    observed_nonce = reason.observed_nonce.as_deref().unwrap_or(""),
                    captured_at_epoch_ms = reason.captured_at_epoch_ms,
                    observed_at_epoch_ms = reason.observed_at_epoch_ms,
                    elapsed_ms = reason.elapsed_ms,
                    rotation_interval_ms = reason.rotation_interval_ms,
                    nonce_validity_window_ms = reason.nonce_validity_window_ms,
                    forced_refresh = self.forced_refresh,
                    "replay defense state invalidated",
                );
            }
        }
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

#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_lossless,
    clippy::cast_sign_loss
)]
const fn duration_to_ms_u64(d: Duration) -> u64 {
    let v = d.as_millis();
    if v > u64::MAX as u128 {
        u64::MAX
    } else {
        v as u64
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

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    const CAPTURED_AT: u64 = 1_700_000_000_000;

    /// Convert a [`Duration`] to a `u64` millisecond count. Saturates
    /// to `u64::MAX` on overflow (theoretical for ms-scale inputs).
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_lossless,
        clippy::cast_sign_loss
    )]
    fn duration_ms(d: Duration) -> u64 {
        let v = d.as_millis();
        if v > u64::MAX as u128 {
            u64::MAX
        } else {
            v as u64
        }
    }

    fn policy() -> ReplayDefensePolicy {
        ReplayDefensePolicy {
            rotation_interval: Duration::from_secs(1),
            nonce_validity_window: Duration::from_secs(1),
            force_reset_on_drift: true,
        }
    }

    #[test]
    fn default_policy_is_deterministic() {
        let a = ReplayDefensePolicy::default();
        let b = ReplayDefensePolicy::default();
        assert_eq!(a.rotation_interval, b.rotation_interval);
        assert_eq!(a.nonce_validity_window, b.nonce_validity_window);
        assert_eq!(a.force_reset_on_drift, b.force_reset_on_drift);
        assert!(a.force_reset_on_drift);
    }

    #[test]
    fn default_policy_is_serializable() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let p = ReplayDefensePolicy::default();
        let json = serde_json::to_string(&p)?;
        let back: ReplayDefensePolicy = serde_json::from_str(&json)?;
        assert_eq!(p.rotation_interval, back.rotation_interval);
        assert_eq!(p.nonce_validity_window, back.nonce_validity_window);
        assert_eq!(p.force_reset_on_drift, back.force_reset_on_drift);
        Ok(())
    }

    #[test]
    fn rotation_interval_triggers_rotation_due() {
        let policy = ReplayDefensePolicy {
            rotation_interval: Duration::from_mins(1),
            ..policy()
        };
        let state = ReplayDefenseState::new("example.com", None, None, CAPTURED_AT);
        // 2 minutes later — past the rotation interval
        let input = ReplayDefenseCheckInput::new(
            "example.com",
            None,
            None,
            CAPTURED_AT + duration_ms(Duration::from_mins(2)),
        );
        let decision = check(&policy, &state, &input);
        assert!(matches!(
            decision,
            ReplayDefenseDecision::RotationDue { ref reason } if reason.kind == ReplayDefenseInvalidationKind::RotationDue
        ));
        assert!(decision.requires_forced_refresh(&policy));
    }

    #[test]
    fn rotation_holds_within_window() {
        let policy = ReplayDefensePolicy {
            rotation_interval: Duration::from_mins(1),
            ..policy()
        };
        let state = ReplayDefenseState::new("example.com", None, None, CAPTURED_AT);
        let input = ReplayDefenseCheckInput::new(
            "example.com",
            None,
            None,
            CAPTURED_AT + duration_ms(Duration::from_secs(30)),
        );
        assert!(check(&policy, &state, &input).is_valid());
    }

    #[test]
    fn nonce_window_expires_nonce() {
        let policy = ReplayDefensePolicy {
            nonce_validity_window: Duration::from_secs(1),
            ..policy()
        };
        let state = ReplayDefenseState::new("example.com", None, Some("nonce-001"), CAPTURED_AT);
        // 5 seconds later — past the nonce validity window
        let input = ReplayDefenseCheckInput::new(
            "example.com",
            None,
            Some("nonce-001"),
            CAPTURED_AT + duration_ms(Duration::from_secs(5)),
        );
        let decision = check(&policy, &state, &input);
        match &decision {
            ReplayDefenseDecision::NonceExpired { reason } => {
                assert_eq!(reason.kind, ReplayDefenseInvalidationKind::NonceExpired);
                assert_eq!(reason.contract_nonce.as_deref(), Some("nonce-001"));
            }
            other => panic!("expected NonceExpired, got {other:?}"),
        }
        assert!(decision.requires_forced_refresh(&policy));
    }

    #[test]
    fn nonce_rotation_emits_nonce_rotated() {
        let policy = policy();
        let state = ReplayDefenseState::new("example.com", None, Some("nonce-001"), CAPTURED_AT);
        let input = ReplayDefenseCheckInput::new(
            "example.com",
            None,
            Some("nonce-002"),
            CAPTURED_AT + duration_ms(Duration::from_secs(1)),
        );
        let decision = check(&policy, &state, &input);
        match decision {
            ReplayDefenseDecision::NonceRotated { reason } => {
                assert_eq!(reason.kind, ReplayDefenseInvalidationKind::NonceRotated);
                assert_eq!(reason.contract_nonce.as_deref(), Some("nonce-001"));
                assert_eq!(reason.observed_nonce.as_deref(), Some("nonce-002"));
            }
            other => panic!("expected NonceRotated, got {other:?}"),
        }
    }

    #[test]
    fn signature_drift_with_force_reset_requires_refresh() {
        let policy = ReplayDefensePolicy {
            force_reset_on_drift: true,
            ..policy()
        };
        let state =
            ReplayDefenseState::with_fingerprint("example.com", "sha256:abc", None, CAPTURED_AT);
        let input = ReplayDefenseCheckInput::new(
            "example.com",
            Some("sha256:xyz"),
            None,
            CAPTURED_AT + 1_000,
        );
        let decision = check(&policy, &state, &input);
        match &decision {
            ReplayDefenseDecision::SignatureDrift { reason } => {
                assert_eq!(reason.kind, ReplayDefenseInvalidationKind::SignatureDrift);
                assert_eq!(reason.contract_signature.as_deref(), Some("sha256:abc"));
                assert_eq!(reason.observed_signature.as_deref(), Some("sha256:xyz"));
                assert!(reason.force_reset_on_drift);
            }
            other => panic!("expected SignatureDrift, got {other:?}"),
        }
        assert!(decision.requires_forced_refresh(&policy));
    }

    #[test]
    fn signature_drift_without_force_reset_does_not_require_refresh() {
        let policy = ReplayDefensePolicy {
            force_reset_on_drift: false,
            ..policy()
        };
        let state =
            ReplayDefenseState::with_fingerprint("example.com", "sha256:abc", None, CAPTURED_AT);
        let input = ReplayDefenseCheckInput::new(
            "example.com",
            Some("sha256:xyz"),
            None,
            CAPTURED_AT + 1_000,
        );
        let decision = check(&policy, &state, &input);
        assert!(matches!(
            decision,
            ReplayDefenseDecision::SignatureDrift { .. }
        ));
        // Without force_reset_on_drift, drift alone does not force
        // a refresh — the runner continues.
        assert!(!decision.requires_forced_refresh(&policy));
    }

    #[test]
    fn domain_mismatch_takes_precedence_over_other_checks() {
        let policy = policy();
        let state = ReplayDefenseState::with_fingerprint(
            "example.com",
            "sha256:abc",
            Some("nonce-001"),
            CAPTURED_AT,
        );
        let input = ReplayDefenseCheckInput::new(
            "other.example",
            Some("sha256:abc"),
            Some("nonce-001"),
            CAPTURED_AT + 1_000,
        );
        let decision = check(&policy, &state, &input);
        match decision {
            ReplayDefenseDecision::DomainMismatch { reason } => {
                assert_eq!(reason.kind, ReplayDefenseInvalidationKind::DomainMismatch);
                assert_eq!(reason.contract_domain, "example.com");
                assert_eq!(reason.observed_domain, "other.example");
            }
            other => panic!("expected DomainMismatch, got {other:?}"),
        }
    }

    #[test]
    fn determinism_same_inputs_same_decision() {
        let policy = policy();
        let state = ReplayDefenseState::with_fingerprint(
            "example.com",
            "sha256:abc",
            Some("nonce-001"),
            CAPTURED_AT,
        );
        let input = ReplayDefenseCheckInput::new(
            "example.com",
            Some("sha256:abc"),
            Some("nonce-001"),
            CAPTURED_AT + 30_000,
        );
        assert_eq!(
            check(&policy, &state, &input),
            check(&policy, &state, &input)
        );
    }

    #[test]
    fn empty_signature_and_nonce_stays_valid() {
        let policy = policy();
        let state = ReplayDefenseState::new("example.com", None, None, CAPTURED_AT);
        let input = ReplayDefenseCheckInput::new("example.com", None, None, CAPTURED_AT + 1_000);
        assert!(check(&policy, &state, &input).is_valid());
    }

    #[test]
    fn decision_labels_are_stable() {
        assert_eq!(ReplayDefenseDecision::Valid.label(), "valid");
        assert_eq!(
            ReplayDefenseInvalidationKind::RotationDue.as_str(),
            "rotation_due"
        );
        assert_eq!(
            ReplayDefenseInvalidationKind::NonceExpired.as_str(),
            "nonce_expired"
        );
        assert_eq!(
            ReplayDefenseInvalidationKind::NonceRotated.as_str(),
            "nonce_rotated"
        );
        assert_eq!(
            ReplayDefenseInvalidationKind::DomainMismatch.as_str(),
            "domain_mismatch"
        );
        assert_eq!(
            ReplayDefenseInvalidationKind::SignatureDrift.as_str(),
            "signature_drift"
        );
    }

    #[test]
    fn skipped_report_is_valid_and_does_not_force_refresh() {
        let report = ReplayDefenseReport::skipped();
        assert!(report.decision.is_valid());
        assert!(!report.state_evaluated);
        assert!(!report.forced_refresh);
    }

    #[test]
    fn evaluate_report_attaches_forced_refresh_flag() {
        let policy = policy();
        let state = ReplayDefenseState::new("example.com", None, None, CAPTURED_AT);
        let input = ReplayDefenseCheckInput::new(
            "example.com",
            None,
            None,
            CAPTURED_AT + duration_ms(Duration::from_secs(2)),
        );
        let report = ReplayDefenseReport::evaluate(&policy, &state, &input);
        assert!(report.state_evaluated);
        assert!(report.decision.is_invalid());
        assert!(report.forced_refresh);
    }

    #[test]
    fn validate_rejects_zero_intervals() {
        let zero_rotation = ReplayDefensePolicy {
            rotation_interval: Duration::ZERO,
            ..ReplayDefensePolicy::default()
        };
        assert!(zero_rotation.validate().is_err());
        let zero_nonce = ReplayDefensePolicy {
            nonce_validity_window: Duration::ZERO,
            ..ReplayDefensePolicy::default()
        };
        assert!(zero_nonce.validate().is_err());
        assert!(ReplayDefensePolicy::default().validate().is_ok());
    }

    #[test]
    fn state_trims_and_lowercases_domain() {
        let s = ReplayDefenseState::new("  EXAMPLE.com  ", Some("sha256:a"), None, 0);
        assert_eq!(s.domain, "example.com");
        assert_eq!(s.signature.as_deref(), Some("sha256:a"));
    }

    #[test]
    fn state_drops_empty_signature_and_nonce() {
        let s = ReplayDefenseState::new("example.com", Some(""), Some(""), 0);
        assert!(s.signature.is_none());
        assert!(s.nonce.is_none());
    }

    #[test]
    fn input_trims_and_lowercases_domain() {
        let i = ReplayDefenseCheckInput::new("  Example.COM  ", Some("sha256:a"), Some("n1"), 0);
        assert_eq!(i.observed_domain, "example.com");
        assert_eq!(i.observed_signature.as_deref(), Some("sha256:a"));
        assert_eq!(i.observed_nonce.as_deref(), Some("n1"));
    }

    #[test]
    fn json_roundtrip_preserves_policy() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let p = ReplayDefensePolicy::default();
        let json = serde_json::to_string(&p)?;
        let back: ReplayDefensePolicy = serde_json::from_str(&json)?;
        assert_eq!(p.rotation_interval, back.rotation_interval);
        assert_eq!(p.nonce_validity_window, back.nonce_validity_window);
        assert_eq!(p.force_reset_on_drift, back.force_reset_on_drift);
        Ok(())
    }

    #[test]
    fn json_roundtrip_preserves_decision() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let policy = policy();
        let state = ReplayDefenseState::new("example.com", None, None, CAPTURED_AT);
        let input = ReplayDefenseCheckInput::new(
            "example.com",
            None,
            None,
            CAPTURED_AT + duration_ms(Duration::from_secs(5)),
        );
        let decision = check(&policy, &state, &input);
        let json = serde_json::to_string(&decision)?;
        let back: ReplayDefenseDecision = serde_json::from_str(&json)?;
        assert_eq!(decision, back);
        Ok(())
    }
}
