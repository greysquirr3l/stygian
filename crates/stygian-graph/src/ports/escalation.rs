//! Tiered request escalation port.
//!
//! Defines the [`EscalationPolicy`] trait for deciding when and how to
//! escalate a failed request from a lightweight tier (plain HTTP) to a
//! heavier one (TLS-profiled HTTP, basic browser, advanced browser).
//!
//! This is a pure domain concept — no I/O, no adapter imports. Concrete
//! policies are implemented in adapter modules (see T19).
//!
//! # Tiers
//!
//! | Tier | Description |
//! |---|---|
//! | [`HttpPlain`](EscalationTier::HttpPlain) | Standard HTTP client, no stealth |
//! | [`HttpTlsProfiled`](EscalationTier::HttpTlsProfiled) | HTTP with TLS fingerprint matching |
//! | [`BrowserBasic`](EscalationTier::BrowserBasic) | Headless browser with basic stealth |
//! | [`BrowserAdvanced`](EscalationTier::BrowserAdvanced) | Full stealth browser (CDP fixes, JS patches) |
//!
//! # Example
//!
//! ```
//! use stygian_graph::ports::escalation::{
//!     EscalationPolicy, EscalationTier, ResponseContext,
//! };
//!
//! struct AlwaysEscalate;
//!
//! impl EscalationPolicy for AlwaysEscalate {
//!     fn initial_tier(&self) -> EscalationTier {
//!         EscalationTier::HttpPlain
//!     }
//!
//!     fn should_escalate(
//!         &self,
//!         ctx: &ResponseContext,
//!         current: EscalationTier,
//!     ) -> Option<EscalationTier> {
//!         current.next()
//!     }
//!
//!     fn max_tier(&self) -> EscalationTier {
//!         EscalationTier::BrowserAdvanced
//!     }
//! }
//!
//! let policy = AlwaysEscalate;
//! assert_eq!(policy.initial_tier(), EscalationTier::HttpPlain);
//! ```

use serde::{Deserialize, Serialize};

// ── EscalationTier ───────────────────────────────────────────────────────────

/// A request-handling tier, ordered from cheapest to most expensive.
///
/// Each tier adds complexity and resource cost but increases the chance
/// of bypassing anti-bot protections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EscalationTier {
    /// Standard HTTP client — no stealth measures, lowest resource cost.
    HttpPlain = 0,
    /// HTTP with a TLS fingerprint profile applied via rustls.
    HttpTlsProfiled = 1,
    /// Headless browser with basic CDP stealth (automation flag removed).
    BrowserBasic = 2,
    /// Full stealth browser: CDP fixes, JS patches, `WebRTC` leak prevention.
    BrowserAdvanced = 3,
}

impl EscalationTier {
    /// Return the next higher tier, or `None` if already at the maximum.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::ports::escalation::EscalationTier;
    ///
    /// assert_eq!(
    ///     EscalationTier::HttpPlain.next(),
    ///     Some(EscalationTier::HttpTlsProfiled)
    /// );
    /// assert_eq!(EscalationTier::BrowserAdvanced.next(), None);
    /// ```
    pub const fn next(self) -> Option<Self> {
        match self {
            Self::HttpPlain => Some(Self::HttpTlsProfiled),
            Self::HttpTlsProfiled => Some(Self::BrowserBasic),
            Self::BrowserBasic => Some(Self::BrowserAdvanced),
            Self::BrowserAdvanced => None,
        }
    }
}

impl std::fmt::Display for EscalationTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HttpPlain => f.write_str("http_plain"),
            Self::HttpTlsProfiled => f.write_str("http_tls_profiled"),
            Self::BrowserBasic => f.write_str("browser_basic"),
            Self::BrowserAdvanced => f.write_str("browser_advanced"),
        }
    }
}

// ── ResponseContext ──────────────────────────────────────────────────────────

/// Contextual information about an HTTP response used by
/// [`EscalationPolicy::should_escalate`] to decide whether to move to a
/// higher tier.
#[derive(Debug, Clone)]
pub struct ResponseContext {
    /// HTTP status code (e.g. 200, 403, 503).
    pub status: u16,
    /// Whether the response body is empty.
    pub body_empty: bool,
    /// Whether the response body contains a Cloudflare challenge marker
    /// (e.g. `<title>Just a moment...</title>` or a `cf-ray` header).
    pub has_cloudflare_challenge: bool,
    /// Whether a CAPTCHA marker was detected in the response
    /// (e.g. reCAPTCHA, hCaptcha script tags).
    pub has_captcha: bool,
}

// ── EscalationResult ─────────────────────────────────────────────────────────

/// The outcome of a tiered escalation run.
///
/// Records which tier ultimately succeeded and the full escalation path
/// for observability.
#[derive(Debug, Clone)]
pub struct EscalationResult<T> {
    /// The tier that produced the final response.
    pub final_tier: EscalationTier,
    /// The successful response payload.
    pub response: T,
    /// Ordered list of tiers attempted (including the successful one).
    pub escalation_path: Vec<EscalationTier>,
}

// ── EscalationPolicy ─────────────────────────────────────────────────────────

/// Port trait for tiered request escalation.
///
/// Implementations decide:
/// - Where to start ([`initial_tier`](Self::initial_tier))
/// - When to move up ([`should_escalate`](Self::should_escalate))
/// - Where to stop ([`max_tier`](Self::max_tier))
///
/// The trait is purely synchronous — it contains no I/O. The pipeline
/// executor (see T20) calls into the policy between tiers.
pub trait EscalationPolicy: Send + Sync {
    /// The tier to attempt first.
    fn initial_tier(&self) -> EscalationTier;

    /// Given a response context and the current tier, return the next tier
    /// to try, or `None` to accept the current response.
    ///
    /// Implementations should respect [`max_tier`](Self::max_tier): if
    /// `current >= self.max_tier()`, return `None`.
    fn should_escalate(
        &self,
        ctx: &ResponseContext,
        current: EscalationTier,
    ) -> Option<EscalationTier>;

    /// The highest tier this policy is allowed to reach.
    fn max_tier(&self) -> EscalationTier;
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A simple default policy for testing:
    /// Start at `HttpPlain`, escalate on 403 / challenge / CAPTCHA.
    struct DefaultPolicy;

    impl EscalationPolicy for DefaultPolicy {
        fn initial_tier(&self) -> EscalationTier {
            EscalationTier::HttpPlain
        }

        fn should_escalate(
            &self,
            ctx: &ResponseContext,
            current: EscalationTier,
        ) -> Option<EscalationTier> {
            if current >= self.max_tier() {
                return None;
            }

            let needs_escalation = ctx.status == 403
                || ctx.has_cloudflare_challenge
                || ctx.has_captcha
                || (ctx.body_empty && current >= EscalationTier::HttpTlsProfiled);

            if needs_escalation {
                current.next()
            } else {
                None
            }
        }

        fn max_tier(&self) -> EscalationTier {
            EscalationTier::BrowserAdvanced
        }
    }

    #[test]
    fn starts_at_http_plain() {
        let policy = DefaultPolicy;
        assert_eq!(policy.initial_tier(), EscalationTier::HttpPlain);
    }

    #[test]
    fn escalates_on_403() {
        let policy = DefaultPolicy;
        let ctx = ResponseContext {
            status: 403,
            body_empty: false,
            has_cloudflare_challenge: false,
            has_captcha: false,
        };
        assert_eq!(
            policy.should_escalate(&ctx, EscalationTier::HttpPlain),
            Some(EscalationTier::HttpTlsProfiled)
        );
    }

    #[test]
    fn escalates_on_cloudflare_challenge() {
        let policy = DefaultPolicy;
        let ctx = ResponseContext {
            status: 503,
            body_empty: false,
            has_cloudflare_challenge: true,
            has_captcha: false,
        };
        assert_eq!(
            policy.should_escalate(&ctx, EscalationTier::HttpTlsProfiled),
            Some(EscalationTier::BrowserBasic)
        );
    }

    #[test]
    fn max_tier_prevents_further_escalation() {
        let policy = DefaultPolicy;
        let ctx = ResponseContext {
            status: 403,
            body_empty: false,
            has_cloudflare_challenge: false,
            has_captcha: false,
        };
        assert_eq!(
            policy.should_escalate(&ctx, EscalationTier::BrowserAdvanced),
            None
        );
    }

    #[test]
    fn no_escalation_on_success() {
        let policy = DefaultPolicy;
        let ctx = ResponseContext {
            status: 200,
            body_empty: false,
            has_cloudflare_challenge: false,
            has_captcha: false,
        };
        assert_eq!(
            policy.should_escalate(&ctx, EscalationTier::HttpPlain),
            None
        );
    }

    #[test]
    fn no_escalation_on_redirect() {
        let policy = DefaultPolicy;
        let ctx = ResponseContext {
            status: 301,
            body_empty: false,
            has_cloudflare_challenge: false,
            has_captcha: false,
        };
        assert_eq!(
            policy.should_escalate(&ctx, EscalationTier::HttpPlain),
            None
        );
    }

    #[test]
    fn tier_ordering() {
        assert!(EscalationTier::HttpPlain < EscalationTier::HttpTlsProfiled);
        assert!(EscalationTier::HttpTlsProfiled < EscalationTier::BrowserBasic);
        assert!(EscalationTier::BrowserBasic < EscalationTier::BrowserAdvanced);
    }

    #[test]
    fn next_tier_chain() {
        assert_eq!(
            EscalationTier::HttpPlain.next(),
            Some(EscalationTier::HttpTlsProfiled)
        );
        assert_eq!(
            EscalationTier::HttpTlsProfiled.next(),
            Some(EscalationTier::BrowserBasic)
        );
        assert_eq!(
            EscalationTier::BrowserBasic.next(),
            Some(EscalationTier::BrowserAdvanced)
        );
        assert_eq!(EscalationTier::BrowserAdvanced.next(), None);
    }

    #[test]
    fn tier_display() {
        assert_eq!(EscalationTier::HttpPlain.to_string(), "http_plain");
        assert_eq!(
            EscalationTier::BrowserAdvanced.to_string(),
            "browser_advanced"
        );
    }

    #[test]
    fn tier_serde_roundtrip() {
        let tier = EscalationTier::BrowserBasic;
        let json = serde_json::to_string(&tier).unwrap();
        let back: EscalationTier = serde_json::from_str(&json).unwrap();
        assert_eq!(tier, back);
    }
}
