//! Dedicated acquisition router for classified interstitials.
//!
//! Consumes an [`InterstitialKind`] (from
//! [`InterstitialClassifier`][super::classifier::InterstitialClassifier])
//! plus a [`PageSignature`][super::PageSignature] and
//! returns a structured [`RouterDecision`] with the
//! dedicated route, the dedicated severity tier, and the
//! per-signature evidence.
//!
//! The router is a pure function — no I/O, no clock reads
//! (the timestamp is captured at the
//! [`RouterDecision::new`] boundary). The router
//! composes with the classifier via
//! [`classify_and_route`] (the most common one-shot
//! helper) and the lower-level [`route`] (when the caller
//! already classified).
//!
//! # Example
//!
//! ```
//! use stygian_browser::interstitial_router::{
//!     InterstitialKind, InterstitialRouter, InterstitialSeverity, PageSignature,
//! };
//!
//! let router = InterstitialRouter::with_defaults();
//! let sig = PageSignature::new("https://example.com/blocked", Some(403))
//!     .with_body_marker("access denied");
//! let decision = router.route(&sig, InterstitialKind::HardBlock);
//! assert_eq!(decision.severity(), InterstitialSeverity::Terminal);
//! assert!(decision.is_terminal());
//! ```

use crate::acquisition::StrategyUsed;

use super::classifier::{InterstitialClassifier, PageSignature};
use super::policy::{InterstitialKind, InterstitialPolicy, InterstitialRoute};
use super::report::{PageSignatureEvidence, RouterDecision};

/// Dedicated acquisition router for classified interstitials.
///
/// `InterstitialRouter` is constructed once with an
/// [`InterstitialPolicy`] and is safe to share across
/// threads (the policy is immutable; the classifier is
/// stateless).
///
/// # Example
///
/// ```
/// use stygian_browser::interstitial_router::{
///     InterstitialKind, InterstitialRouter, PageSignature,
/// };
///
/// let router = InterstitialRouter::with_defaults();
/// let sig = PageSignature::new("https://example.com/redirect", Some(302));
/// let decision = router.route(&sig, InterstitialKind::Transient);
/// assert!(!decision.is_classified());
/// assert!(decision.is_retryable());
/// ```
#[derive(Debug, Clone)]
pub struct InterstitialRouter {
    classifier: InterstitialClassifier,
    policy: InterstitialPolicy,
}

impl Default for InterstitialRouter {
    fn default() -> Self {
        Self::with_defaults()
    }
}

impl InterstitialRouter {
    /// Build a router with the supplied policy.
    #[must_use]
    pub const fn new(policy: InterstitialPolicy) -> Self {
        Self {
            classifier: InterstitialClassifier::new(),
            policy,
        }
    }

    /// Build a router with the default policy.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(InterstitialPolicy::default())
    }

    /// Borrow the configured policy.
    #[must_use]
    pub const fn policy(&self) -> &InterstitialPolicy {
        &self.policy
    }

    /// Replace the policy.
    #[must_use]
    pub const fn with_policy(mut self, policy: InterstitialPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Classify `signature` via the router's classifier
    /// and route the result. This is the one-shot helper
    /// most callers want.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::interstitial_router::{
    ///     InterstitialKind, InterstitialRouter, PageSignature,
    /// };
    ///
    /// let router = InterstitialRouter::with_defaults();
    /// let sig = PageSignature::new("https://example.com", Some(302));
    /// let decision = router.classify_and_route(&sig);
    /// assert_eq!(decision.kind(), InterstitialKind::Transient);
    /// ```
    #[must_use]
    pub fn classify_and_route(&self, signature: &PageSignature) -> RouterDecision {
        let kind = self.classifier.classify(signature);
        self.route(signature, kind)
    }

    /// Route a pre-classified signature. The decision is
    /// built from the supplied `kind` plus the
    /// signature's evidence.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::interstitial_router::{
    ///     InterstitialKind, InterstitialRouter, InterstitialSeverity, PageSignature,
    /// };
    ///
    /// let router = InterstitialRouter::with_defaults();
    /// let sig = PageSignature::new("https://example.com", Some(202))
    ///     .with_body_marker("please wait");
    /// let decision = router.route(&sig, InterstitialKind::Queue);
    /// assert_eq!(decision.severity(), InterstitialSeverity::Retryable);
    /// ```
    #[must_use]
    pub fn route(
        &self,
        signature: &PageSignature,
        kind: InterstitialKind,
    ) -> RouterDecision {
        let evidence = build_evidence(signature, kind);
        let route = self.build_route(signature, kind);
        let reason = build_reason(signature, kind);
        RouterDecision::new(kind, route, reason, evidence)
    }

    fn build_route(
        &self,
        signature: &PageSignature,
        kind: InterstitialKind,
    ) -> InterstitialRoute {
        match kind {
            InterstitialKind::Queue => InterstitialRoute::WaitAndRetry {
                interval: self.policy.queue_interval,
                max_retries: self.policy.queue_max_retries,
                queue_position: signature.queue_position_hint,
            },
            InterstitialKind::Challenge => InterstitialRoute::ChallengeSolve {
                vendor_hint: signature.vendor_hint.clone(),
                allowed_strategies: allowed_strategies_for_challenge(),
                solve_budget: self.policy.challenge_solve_budget,
            },
            InterstitialKind::HardBlock => InterstitialRoute::HardBlock {
                escalate_to: self.policy.hard_block_escalation,
                rotate_session: true,
                refresh_sticky: true,
            },
            InterstitialKind::Transient => InterstitialRoute::Transient {
                follow_redirect: self.policy.transient_follow_redirect,
                max_hops: self.policy.max_transient_hops,
            },
        }
    }

    /// `true` when the router's policy
    /// (`short_circuit_on_classified`) is set and the
    /// `kind` is a classified (non-`Transient`) decision.
    /// The runner calls this helper to decide whether to
    /// short-circuit on the decision.
    #[must_use]
    pub const fn should_short_circuit(&self, kind: InterstitialKind) -> bool {
        self.policy.short_circuit_on_classified && !matches!(kind, InterstitialKind::Transient)
    }
}

/// One-shot helper: classify + route via a default
/// router. Convenience for tests and call sites that
/// don't need to customise the policy.
///
/// # Example
///
/// ```
/// use stygian_browser::interstitial_router::{
///     classify_and_route, InterstitialKind, PageSignature,
/// };
///
/// let sig = PageSignature::new("https://example.com/cdn-cgi/challenge-platform/h/b", Some(403))
///     .with_body_marker("cf-chl-bypass");
/// let decision = classify_and_route(&sig);
/// assert_eq!(decision.kind(), InterstitialKind::Challenge);
/// ```
#[must_use]
pub fn classify_and_route(signature: &PageSignature) -> RouterDecision {
    InterstitialRouter::with_defaults().classify_and_route(signature)
}

/// One-shot helper: route a pre-classified signature via
/// a default router.
#[must_use]
pub fn route(signature: &PageSignature, kind: InterstitialKind) -> RouterDecision {
    InterstitialRouter::with_defaults().route(signature, kind)
}

fn allowed_strategies_for_challenge() -> Vec<StrategyUsed> {
    vec![
        StrategyUsed::BrowserLightStealth,
        StrategyUsed::StickyProxyBrowserSession,
    ]
}

fn build_evidence(
    signature: &PageSignature,
    kind: InterstitialKind,
) -> PageSignatureEvidence {
    let host = signature.host();
    let matched_url_patterns = match kind {
        InterstitialKind::HardBlock => {
            url_pattern_matches(signature, super::classifier::HARD_BLOCK_URL_PATTERNS_PUBLIC)
        }
        InterstitialKind::Challenge => {
            url_pattern_matches(signature, super::classifier::CHALLENGE_URL_PATTERNS_PUBLIC)
        }
        InterstitialKind::Queue => {
            url_pattern_matches(signature, super::classifier::QUEUE_URL_PATTERNS_PUBLIC)
        }
        InterstitialKind::Transient => Vec::new(),
    };
    let matched_body_markers = body_marker_matches(signature, kind);
    let matched_headers = match kind {
        InterstitialKind::Challenge => {
            header_matches(signature, super::classifier::CHALLENGE_HEADERS_PUBLIC)
        }
        _ => Vec::new(),
    };
    PageSignatureEvidence {
        host,
        status_code: signature.status_code,
        matched_url_patterns,
        matched_body_markers,
        matched_headers,
        queue_position: signature.queue_position_hint,
        vendor_hint: signature.vendor_hint.clone(),
    }
}

fn url_pattern_matches(signature: &PageSignature, patterns: &[&str]) -> Vec<String> {
    patterns
        .iter()
        .filter(|p| signature.url_contains(p))
        .map(|p| (*p).to_string())
        .collect()
}

fn body_marker_matches(signature: &PageSignature, kind: InterstitialKind) -> Vec<String> {
    let catalog: &[&str] = match kind {
        InterstitialKind::HardBlock => super::classifier::HARD_BLOCK_BODY_MARKERS_PUBLIC,
        InterstitialKind::Challenge => super::classifier::CHALLENGE_BODY_MARKERS_PUBLIC,
        InterstitialKind::Queue => super::classifier::QUEUE_BODY_MARKERS_PUBLIC,
        InterstitialKind::Transient => &[],
    };
    catalog
        .iter()
        .filter(|m| signature.body_contains(m))
        .map(|m| (*m).to_string())
        .collect()
}

fn header_matches(signature: &PageSignature, names: &[&str]) -> Vec<String> {
    names
        .iter()
        .filter(|n| signature.has_header(n))
        .map(|n| (*n).to_string())
        .collect()
}

fn build_reason(signature: &PageSignature, kind: InterstitialKind) -> String {
    let host = signature.host().unwrap_or_else(|| "<unknown>".to_string());
    match kind {
        InterstitialKind::Queue => format!(
            "queue page observed on {host} (url={})",
            truncate_url(&signature.url)
        ),
        InterstitialKind::Challenge => format!(
            "challenge interstitial observed on {host} (url={})",
            truncate_url(&signature.url)
        ),
        InterstitialKind::HardBlock => format!(
            "hard block observed on {host} (url={})",
            truncate_url(&signature.url)
        ),
        InterstitialKind::Transient => format!(
            "transient redirect observed on {host} (url={})",
            truncate_url(&signature.url)
        ),
    }
}

fn truncate_url(url: &str) -> String {
    const MAX: usize = 128;
    if url.len() <= MAX {
        url.to_string()
    } else {
        format!("{}…", &url[..MAX])
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
    use super::super::policy::{
        DEFAULT_HARD_BLOCK_ESCALATION, DEFAULT_MAX_TRANSIENT_HOPS, DEFAULT_QUEUE_MAX_RETRIES,
        DEFAULT_TRANSIENT_FOLLOW_REDIRECT, InterstitialSeverity,
    };
    use super::*;

    #[test]
    fn route_returns_wait_and_retry_for_queue() {
        let router = InterstitialRouter::with_defaults();
        let sig = PageSignature::new("https://example.com/queue", Some(200))
            .with_body_marker("please wait")
            .with_queue_position(5);
        let decision = router.route(&sig, InterstitialKind::Queue);
        assert_eq!(decision.kind(), InterstitialKind::Queue);
        assert_eq!(decision.severity(), InterstitialSeverity::Retryable);
        match decision.route() {
            InterstitialRoute::WaitAndRetry {
                max_retries,
                queue_position,
                ..
            } => {
                assert_eq!(*max_retries, DEFAULT_QUEUE_MAX_RETRIES);
                assert_eq!(*queue_position, Some(5));
            }
            other => panic!("expected WaitAndRetry, got {other:?}"),
        }
    }

    #[test]
    fn route_returns_challenge_solve_for_challenge() {
        let router = InterstitialRouter::with_defaults();
        let sig = PageSignature::new(
            "https://example.com/cdn-cgi/challenge-platform/h/b",
            Some(403),
        )
        .with_body_marker("cf-chl-bypass")
        .with_vendor_hint("cloudflare");
        let decision = router.route(&sig, InterstitialKind::Challenge);
        assert_eq!(decision.kind(), InterstitialKind::Challenge);
        assert_eq!(decision.severity(), InterstitialSeverity::RequiresSolve);
        match decision.route() {
            InterstitialRoute::ChallengeSolve {
                vendor_hint,
                allowed_strategies,
                ..
            } => {
                assert_eq!(vendor_hint.as_deref(), Some("cloudflare"));
                assert!(allowed_strategies.contains(&StrategyUsed::StickyProxyBrowserSession));
            }
            other => panic!("expected ChallengeSolve, got {other:?}"),
        }
    }

    #[test]
    fn route_returns_hard_block_strategy_for_hardblock() {
        let router = InterstitialRouter::with_defaults();
        let sig = PageSignature::new("https://example.com/blocked", Some(403))
            .with_body_marker("access denied");
        let decision = router.route(&sig, InterstitialKind::HardBlock);
        assert_eq!(decision.kind(), InterstitialKind::HardBlock);
        assert_eq!(decision.severity(), InterstitialSeverity::Terminal);
        assert!(decision.is_terminal());
        match decision.route() {
            InterstitialRoute::HardBlock {
                escalate_to,
                rotate_session,
                refresh_sticky,
            } => {
                assert_eq!(*escalate_to, DEFAULT_HARD_BLOCK_ESCALATION);
                assert!(*rotate_session);
                assert!(*refresh_sticky);
            }
            other => panic!("expected HardBlock, got {other:?}"),
        }
    }

    #[test]
    fn route_returns_transient_strategy_for_transient() {
        let router = InterstitialRouter::with_defaults();
        let sig = PageSignature::new("https://example.com/redirect", Some(302));
        let decision = router.route(&sig, InterstitialKind::Transient);
        assert_eq!(decision.kind(), InterstitialKind::Transient);
        assert_eq!(decision.severity(), InterstitialSeverity::Retryable);
        match decision.route() {
            InterstitialRoute::Transient {
                follow_redirect,
                max_hops,
            } => {
                assert_eq!(*follow_redirect, DEFAULT_TRANSIENT_FOLLOW_REDIRECT);
                assert_eq!(*max_hops, DEFAULT_MAX_TRANSIENT_HOPS);
            }
            other => panic!("expected Transient, got {other:?}"),
        }
    }

    #[test]
    fn should_short_circuit_skips_transient() {
        let router = InterstitialRouter::with_defaults();
        assert!(router.should_short_circuit(InterstitialKind::Queue));
        assert!(router.should_short_circuit(InterstitialKind::Challenge));
        assert!(router.should_short_circuit(InterstitialKind::HardBlock));
        assert!(!router.should_short_circuit(InterstitialKind::Transient));

        let lenient = InterstitialRouter::with_defaults()
            .with_policy(InterstitialPolicy {
                short_circuit_on_classified: false,
                ..InterstitialPolicy::default()
            });
        assert!(!lenient.should_short_circuit(InterstitialKind::HardBlock));
    }

    #[test]
    fn determinism_identical_signatures_yield_identical_decisions() {
        let router = InterstitialRouter::with_defaults();
        let sig_a = PageSignature::new("https://example.com/blocked", Some(403))
            .with_body_marker("access denied")
            .with_vendor_hint("cloudflare");
        let sig_b = PageSignature::new("https://example.com/blocked", Some(403))
            .with_body_marker("access denied")
            .with_vendor_hint("cloudflare");
        let decision_a = router.classify_and_route(&sig_a);
        let decision_b = router.classify_and_route(&sig_b);
        assert_eq!(decision_a, decision_b);
        // Same kind, severity, route, and reason.
        assert_eq!(decision_a.kind(), decision_b.kind());
        assert_eq!(decision_a.severity(), decision_b.severity());
        assert_eq!(decision_a.route(), decision_b.route());
        assert_eq!(decision_a.reason(), decision_b.reason());
    }

    #[test]
    fn observability_distinguishes_queue_from_hard_block() {
        let router = InterstitialRouter::with_defaults();
        let queue_sig = PageSignature::new("https://example.com/queue", Some(200))
            .with_body_marker("please wait");
        let hard_block_sig = PageSignature::new("https://example.com/blocked", Some(403))
            .with_body_marker("access denied");
        let queue_decision = router.classify_and_route(&queue_sig);
        let hard_block_decision = router.classify_and_route(&hard_block_sig);
        // The dedicated severity field must distinguish them.
        assert_eq!(queue_decision.severity(), InterstitialSeverity::Retryable);
        assert_eq!(hard_block_decision.severity(), InterstitialSeverity::Terminal);
        assert!(queue_decision.is_retryable());
        assert!(hard_block_decision.is_terminal());
        assert!(!queue_decision.is_terminal());
        assert!(!hard_block_decision.is_retryable());
    }
}
