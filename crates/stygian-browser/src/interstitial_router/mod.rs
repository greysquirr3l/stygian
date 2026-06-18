//! Queue and interstitial detection routing.
//!
//! ## What is an "interstitial"?
//!
//! Anti-bot vendors (`Cloudflare`, `DataDome`, `PerimeterX`,
//! `Akamai` Bot Manager, `Kasada`, `Fingerprint.com`) often
//! respond to a high-risk navigation with a **non-success page**
//! that is not a hard `4xx`/`5xx` and not the target
//! document. These intermediate pages come in four shapes:
//!
//! - **Queue / waiting room** — the user is told to wait
//!   ("Please wait...", "You are #5 in line", "Estimated wait
//!   time 2 minutes"). The page often returns a `2xx` or `3xx`
//!   with `queue` / `wait` body markers.
//! - **Challenge interstitial** — a vendor-issued
//!   captcha / turnstile / proof-of-work challenge that the
//!   client must solve before being allowed to the target
//!   document. Common markers: `cf-chl-bypass`, `g-recaptcha`,
//!   `h-captcha`, `cf-turnstile`, `akamai`, `perimeterx`,
//!   `_abck`.
//! - **Hard block** — a terminal vendor block page
//!   ("Access denied", "Request blocked", `Just a moment...`
//!   that does not auto-resolve, vendor-specific
//!   `/blocked` / `/forbidden` URLs).
//! - **Transient redirect** — a `3xx` redirect chain that
//!   should be followed before classifying the response
//!   (often the case for region / cookie-consent
//!   redirections that vendors insert before the
//!   challenge).
//!
//! ## What this module provides
//!
//! 1. [`InterstitialClassifier`] — pure deterministic
//!    classifier that consumes a [`PageSignature`] and
//!    returns an [`InterstitialKind`].
//! 2. [`InterstitialRouter`] — maps the classification to a
//!    dedicated [`InterstitialRoute`] (the dedicated
//!    acquisition strategy per kind) with explicit
//!    diagnostics in a [`RouterDecision`].
//! 3. A stable [`severity`][InterstitialSeverity] field on
//!    [`RouterDecision`] that observability tooling can use
//!    to distinguish [`InterstitialKind::Queue`] (retryable
//!    wait) from [`InterstitialKind::HardBlock`] (terminal
//!    escalation) without branching on the kind itself.
//!
//! ## Routing behavior table
//!
//! | [`InterstitialKind`] | Default route | Default severity | Strategy hint |
//! |---|---|---|---|
//! | [`Queue`][InterstitialKind::Queue]        | [`InterstitialRoute::WaitAndRetry`]    | [`Retryable`][InterstitialSeverity::Retryable]       | Wait the configured interval, then retry. Honors the optional queue position hint. |
//! | [`Challenge`][InterstitialKind::Challenge] | [`InterstitialRoute::ChallengeSolve`]  | [`RequiresSolve`][InterstitialSeverity::RequiresSolve] | Escalate to a browser with sticky session + solve budget. Optional vendor hint narrows the strategy. |
//! | [`HardBlock`][InterstitialKind::HardBlock]  | [`InterstitialRoute::HardBlock`]       | [`Terminal`][InterstitialSeverity::Terminal]         | Rotate session + invalidate sticky context + escalate to the strongest available strategy. |
//! | [`Transient`][InterstitialKind::Transient]  | [`InterstitialRoute::Transient`]       | [`Retryable`][InterstitialSeverity::Retryable]       | Follow redirect chain (bounded hops), then re-classify. |
//!
//! ## Integration with `AcquisitionRunner`
//!
//! [`AcquisitionRequest::interstitial`][crate::acquisition::AcquisitionRequest::interstitial]
//! carries a previously-observed [`PageSignature`] plus an
//! [`InterstitialPolicy`] into the runner. The runner
//! evaluates the signature via [`InterstitialClassifier`]
//! before any stage executes:
//!
//! 1. The resulting [`RouterDecision`] is attached to
//!    [`AcquisitionResult::interstitial`][crate::acquisition::AcquisitionResult::interstitial]
//!    so downstream policy mapping (T83 / T85 / T89 / T93)
//!    can consume the decision as a strategy hint.
//! 2. When the decision is non-[`Transient`][InterstitialKind::Transient]
//!    **and** [`InterstitialPolicy::short_circuit_on_classified`]
//!    is `true` (the default), the runner short-circuits
//!    with a structured
//!    [`StageFailureKind::InterstitialRouted`][crate::acquisition::StageFailureKind::InterstitialRouted]
//!    failure tagged with the decision so the calling
//!    layer can route via the dedicated strategy without
//!    burning through the generic ladder.
//!
//! Transient redirects do not short-circuit by default —
//! they flow through the ladder so the redirect can be
//! followed normally.
//!
//! ## Feature flag
//!
//! This module is **default-on** and is always compiled as
//! part of the `stygian-browser` crate. No new feature gate
//! is introduced; the integration is purely additive on
//! [`crate::acquisition::AcquisitionRequest`] and
//! [`crate::acquisition::AcquisitionResult`].
//!
//! # Example
//!
//! ```
//! use stygian_browser::interstitial_router::{
//!     InterstitialClassifier, InterstitialKind, InterstitialRouter, PageSignature,
//! };
//!
//! // A Cloudflare challenge interstitial observed on a previous attempt.
//! let signature = PageSignature::new(
//!     "https://example.com/cdn-cgi/challenge-platform/h/b",
//!     Some(403),
//! )
//! .with_body_marker("cf-chl-bypass")
//! .with_header("cf-mitigated");
//!
//! let classifier = InterstitialClassifier::new();
//! let kind = classifier.classify(&signature);
//! assert_eq!(kind, InterstitialKind::Challenge);
//!
//! let router = InterstitialRouter::with_defaults();
//! let decision = router.route(&signature, kind);
//! assert!(decision.is_classified());
//! assert_eq!(decision.kind(), InterstitialKind::Challenge);
//! ```

mod classifier;
mod policy;
mod report;
mod router;

pub use classifier::{InterstitialClassifier, PageSignature};
pub use policy::{
    DEFAULT_CHALLENGE_SOLVE_BUDGET_MS, DEFAULT_HARD_BLOCK_ESCALATION, DEFAULT_MAX_TRANSIENT_HOPS,
    DEFAULT_QUEUE_INTERVAL_MS, DEFAULT_QUEUE_MAX_RETRIES, DEFAULT_TRANSIENT_FOLLOW_REDIRECT,
    InterstitialKind, InterstitialPolicy, InterstitialRoute, InterstitialSeverity,
};
pub use report::{PageSignatureEvidence, RouterDecision, RouterDecisionLog};
pub use router::{InterstitialRouter, classify_and_route, route};
