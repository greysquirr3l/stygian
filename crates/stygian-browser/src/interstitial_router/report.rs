//! Router decision and per-signature evidence types.
//!
//! The [`RouterDecision`] is the structured output of
//! [`InterstitialRouter`][super::InterstitialRouter]: a
//! classification kind, a dedicated
//! [`InterstitialRoute`][super::policy::InterstitialRoute],
//! a dedicated
//! [`severity`][super::policy::InterstitialSeverity] field
//! (the observability discriminator), the per-signature
//! evidence that drove the decision, and a wall-clock
//! timestamp for diagnostic routing.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::policy::{
    InterstitialKind, InterstitialRoute, InterstitialSeverity,
};

/// Evidence the classifier extracted from a [`PageSignature`][super::PageSignature].
///
/// Carries the URL, status, and the matched body / URL /
/// header patterns so downstream observability tooling can
/// trace the decision back to the raw observation. The
/// `evidence` is built by the router after the classifier
/// has run, so a `RouterDecision` is self-describing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageSignatureEvidence {
    /// Lower-case host extracted from the signature URL.
    pub host: Option<String>,
    /// HTTP status code.
    pub status_code: Option<u16>,
    /// URL patterns that fired (e.g. `/cdn-cgi/challenge-platform`).
    pub matched_url_patterns: Vec<String>,
    /// Body markers that fired (lower-cased substrings).
    pub matched_body_markers: Vec<String>,
    /// Header names that fired (lower-cased).
    pub matched_headers: Vec<String>,
    /// Queue position hint observed in the signature, when
    /// known.
    pub queue_position: Option<u32>,
    /// Vendor hint observed in the signature, when known.
    pub vendor_hint: Option<String>,
}

/// Result of routing a [`PageSignature`][super::PageSignature].
///
/// The decision carries:
///
/// - `kind` — the structural classification from
///   [`InterstitialClassifier`][super::InterstitialClassifier].
/// - `severity` — **dedicated** observability field
///   distinguishing retryable / requires-solve / terminal
///   tiers. Observability tooling should branch on this
///   field rather than the kind enum when the question is
///   "is the run terminal vs retryable".
/// - `route` — the dedicated acquisition route per kind.
/// - `reason` — short human-readable rationale.
/// - `evidence` — the per-signature matches that drove the
///   decision.
/// - `classified_at_unix_ms` — wall-clock timestamp the
///   decision was produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouterDecision {
    /// Structural classification.
    pub kind: InterstitialKind,
    /// Dedicated operational severity tier. Distinguishes
    /// `Queue` (retryable) from `HardBlock` (terminal) for
    /// observability tooling.
    pub severity: InterstitialSeverity,
    /// Dedicated acquisition route.
    pub route: InterstitialRoute,
    /// Compact human-readable rationale.
    pub reason: String,
    /// Per-signature evidence that drove the decision.
    pub evidence: PageSignatureEvidence,
    /// Wall-clock timestamp the decision was produced.
    pub classified_at_unix_ms: u64,
}

impl RouterDecision {
    /// Build a decision with the supplied fields and the
    /// current wall-clock timestamp.
    #[must_use]
    pub fn new(
        kind: InterstitialKind,
        route: InterstitialRoute,
        reason: impl Into<String>,
        evidence: PageSignatureEvidence,
    ) -> Self {
        let severity = InterstitialSeverity::for_kind(kind);
        let classified_at_unix_ms = unix_epoch_ms();
        Self {
            kind,
            severity,
            route,
            reason: reason.into(),
            evidence,
            classified_at_unix_ms,
        }
    }

    /// Build a decision with an explicit
    /// `classified_at_unix_ms` (useful for tests and
    /// deterministic replay).
    #[must_use]
    pub fn with_timestamp(mut self, unix_ms: u64) -> Self {
        self.classified_at_unix_ms = unix_ms;
        self
    }

    /// Structural classification kind.
    #[must_use]
    pub const fn kind(&self) -> InterstitialKind {
        self.kind
    }

    /// Operational severity tier (dedicated observability
    /// field).
    #[must_use]
    pub const fn severity(&self) -> InterstitialSeverity {
        self.severity
    }

    /// Dedicated acquisition route.
    #[must_use]
    pub fn route(&self) -> &InterstitialRoute {
        &self.route
    }

    /// Per-signature evidence.
    #[must_use]
    pub fn evidence(&self) -> &PageSignatureEvidence {
        &self.evidence
    }

    /// Compact human-readable rationale.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// `true` when the decision is for a classified
    /// interstitial (kind != transient). Used by the runner
    /// to decide whether to short-circuit.
    #[must_use]
    pub const fn is_classified(&self) -> bool {
        !matches!(self.kind, InterstitialKind::Transient)
    }

    /// `true` when the severity is
    /// [`InterstitialSeverity::Terminal`].
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self.severity, InterstitialSeverity::Terminal)
    }

    /// `true` when the severity is
    /// [`InterstitialSeverity::RequiresSolve`].
    #[must_use]
    pub const fn requires_solve(&self) -> bool {
        matches!(self.severity, InterstitialSeverity::RequiresSolve)
    }

    /// `true` when the severity is
    /// [`InterstitialSeverity::Retryable`].
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self.severity, InterstitialSeverity::Retryable)
    }

    /// Emit a structured `tracing` event for the decision.
    /// Always emits at `info` for classified decisions and
    /// at `debug` for transient ones so a single
    /// `tracing-subscriber` filter can be used to suppress
    /// the noisy transient path while keeping the
    /// classified path visible.
    pub fn log(&self) {
        if self.is_classified() {
            tracing::info!(
                target: "stygian::interstitial_router",
                kind = self.kind.label(),
                severity = self.severity.label(),
                route = self.route.label(),
                host = self.evidence.host.as_deref().unwrap_or(""),
                status_code = self.evidence.status_code.unwrap_or(0),
                queue_position = self.evidence.queue_position.unwrap_or(0),
                vendor_hint = self.evidence.vendor_hint.as_deref().unwrap_or(""),
                matched_url_patterns = self.evidence.matched_url_patterns.len(),
                matched_body_markers = self.evidence.matched_body_markers.len(),
                matched_headers = self.evidence.matched_headers.len(),
                classified_at_unix_ms = self.classified_at_unix_ms,
                "interstitial routing decision",
            );
        } else {
            tracing::debug!(
                target: "stygian::interstitial_router",
                kind = self.kind.label(),
                severity = self.severity.label(),
                route = self.route.label(),
                host = self.evidence.host.as_deref().unwrap_or(""),
                status_code = self.evidence.status_code.unwrap_or(0),
                "interstitial routing decision (transient)",
            );
        }
    }
}

impl fmt::Display for RouterDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "interstitial(kind={}, severity={}, route={}, reason={})",
            self.kind.label(),
            self.severity.label(),
            self.route.label(),
            self.reason,
        )
    }
}

/// Wrapper struct that records the router decision
/// alongside the original [`PageSignature`][super::PageSignature]
/// for audit / replay. Not currently part of the
/// acquisition result schema, but the type is exposed so
/// downstream tooling that wants to log the full
/// decision-derivation can build one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouterDecisionLog {
    /// The original signature.
    pub signature: super::PageSignature,
    /// The decision.
    pub decision: RouterDecision,
}

impl RouterDecisionLog {
    /// Build a log record from a signature + decision pair.
    #[must_use]
    pub fn new(signature: super::PageSignature, decision: RouterDecision) -> Self {
        Self { signature, decision }
    }
}

/// Current Unix epoch in milliseconds, clamped to `u64`.
#[must_use]
pub fn unix_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(std::time::Duration::ZERO, |d| d)
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
