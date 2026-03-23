//! Default escalation policy adapter.
//!
//! Implements [`EscalationPolicy`] with:
//! - Automatic challenge detection (Cloudflare, DataDome, PerimeterX, CAPTCHA)
//! - Per-domain tier cache (learning cache with configurable TTL)
//! - Configurable `max_tier` and `base_tier`
//!
//! # Challenge detection
//!
//! [`DefaultEscalationPolicy::context_from_body`] inspects the response body
//! for well-known markers and populates a [`ResponseContext`] automatically.
//! Both `has_cloudflare_challenge` and DataDome/PerimeterX markers map to the
//! `has_cloudflare_challenge` field (treated as "any anti-bot challenge").
//!
//! # Per-domain learning cache
//!
//! When a request to a domain succeeds at a tier above `base_tier`, the policy
//! records that tier with [`record_tier_success`].  Future calls to
//! [`initial_tier_for_domain`] skip lower tiers automatically until the cache
//! entry expires (`cache_ttl`).
//!
//! # Example
//!
//! ```
//! use std::time::Duration;
//! use stygian_graph::adapters::escalation::{DefaultEscalationPolicy, EscalationConfig};
//! use stygian_graph::ports::escalation::{EscalationPolicy, EscalationTier, ResponseContext};
//!
//! let policy = DefaultEscalationPolicy::new(EscalationConfig::default());
//!
//! let ctx = ResponseContext {
//!     status: 403,
//!     body_empty: false,
//!     has_cloudflare_challenge: false,
//!     has_captcha: false,
//! };
//!
//! assert!(policy.should_escalate(&ctx, EscalationTier::HttpPlain).is_some());
//! ```
//!
//! [`record_tier_success`]: DefaultEscalationPolicy::record_tier_success
//! [`initial_tier_for_domain`]: DefaultEscalationPolicy::initial_tier_for_domain

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::ports::escalation::{EscalationPolicy, EscalationTier, ResponseContext};

// ── EscalationConfig ─────────────────────────────────────────────────────────

/// Configuration for [`DefaultEscalationPolicy`].
#[derive(Debug, Clone)]
pub struct EscalationConfig {
    /// Highest tier the policy is allowed to reach.
    pub max_tier: EscalationTier,
    /// Starting tier when no domain cache entry exists.
    pub base_tier: EscalationTier,
    /// How long a successful domain cache entry remains valid before eviction.
    pub cache_ttl: Duration,
}

impl Default for EscalationConfig {
    fn default() -> Self {
        Self {
            max_tier: EscalationTier::BrowserAdvanced,
            base_tier: EscalationTier::HttpPlain,
            cache_ttl: Duration::from_secs(3_600),
        }
    }
}

// ── Challenge detection helpers ───────────────────────────────────────────────

/// Returns `true` if the body contains a Cloudflare browser-check challenge.
fn is_cloudflare_challenge(body: &str) -> bool {
    body.contains("Just a moment")
        || body.contains("cf-browser-verification")
        || body.contains("__cf_bm")
        || body.contains("Checking if the site connection is secure")
}

/// Returns `true` if the body contains a DataDome interstitial marker.
fn is_datadome_interstitial(body: &str) -> bool {
    body.contains("datadome") || body.contains("dd_referrer")
}

/// Returns `true` if the body contains a PerimeterX challenge marker.
fn is_perimeterx_challenge(body: &str) -> bool {
    body.contains("_pxParam") || body.contains("_px.js") || body.contains("blockScript")
}

/// Returns `true` if the body contains a known CAPTCHA widget marker.
fn has_captcha_marker(body: &str) -> bool {
    body.contains("recaptcha") || body.contains("hcaptcha") || body.contains("turnstile")
}

// ── DefaultEscalationPolicy ───────────────────────────────────────────────────

/// Per-domain cache entry: minimum tier that was needed + expiry instant.
type CacheEntry = (EscalationTier, Instant);

/// Default escalation policy with challenge detection and per-domain learning.
///
/// Cheaply cloneable — all interior state is behind an `Arc`.
#[derive(Clone)]
pub struct DefaultEscalationPolicy {
    config: EscalationConfig,
    /// Domain → minimum successful tier, keyed by domain string.
    cache: Arc<RwLock<HashMap<String, CacheEntry>>>,
}

impl DefaultEscalationPolicy {
    /// Create a new policy with the given configuration.
    pub fn new(config: EscalationConfig) -> Self {
        Self {
            config,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Build a [`ResponseContext`] from an HTTP status code and response body.
    ///
    /// Inspects the body for Cloudflare, DataDome, PerimeterX, and CAPTCHA
    /// markers.  All anti-bot challenge types map to `has_cloudflare_challenge`
    /// (the field name reflects its original purpose but covers all vendors).
    pub fn context_from_body(status: u16, body: &str) -> ResponseContext {
        ResponseContext {
            status,
            body_empty: body.trim().is_empty(),
            has_cloudflare_challenge: is_cloudflare_challenge(body)
                || is_datadome_interstitial(body)
                || is_perimeterx_challenge(body),
            has_captcha: has_captcha_marker(body),
        }
    }

    /// Return the starting tier for `domain`, consulting the learning cache.
    ///
    /// If the domain has a valid (non-expired) cache entry, returns that tier
    /// instead of [`EscalationConfig::base_tier`], skipping unnecessary tiers.
    pub fn initial_tier_for_domain(&self, domain: &str) -> EscalationTier {
        let cache = self.cache.read().expect("escalation cache poisoned");
        if let Some((tier, expires_at)) = cache.get(domain)
            && Instant::now() < *expires_at
        {
            tracing::debug!(domain, tier = %tier, "using cached initial escalation tier");
            return *tier;
        }
        self.config.base_tier
    }

    /// Record a successful response for `domain` at `tier`.
    ///
    /// If `tier` is above `base_tier`, caches it so future requests to this
    /// domain can skip lower tiers.  The cache never regresses — a lower tier
    /// will not overwrite a higher cached value.
    pub fn record_tier_success(&self, domain: &str, tier: EscalationTier) {
        if tier <= self.config.base_tier {
            return; // nothing meaningful to cache
        }
        let expires_at = Instant::now() + self.config.cache_ttl;
        let mut cache = self.cache.write().expect("escalation cache poisoned");
        let should_insert = cache.get(domain).is_none_or(|(cached, _)| tier >= *cached);
        if should_insert {
            tracing::info!(domain, tier = %tier, "caching successful escalation tier");
            cache.insert(domain.to_string(), (tier, expires_at));
        }
    }

    /// Purge expired domain cache entries.
    ///
    /// Returns the number of entries removed.  Safe to call on any schedule;
    /// the T20 pipeline executor calls this periodically.
    pub fn purge_expired_cache(&self) -> usize {
        let mut cache = self.cache.write().expect("escalation cache poisoned");
        let now = Instant::now();
        let before = cache.len();
        cache.retain(|_, (_, expires_at)| now < *expires_at);
        before - cache.len()
    }
}

impl EscalationPolicy for DefaultEscalationPolicy {
    fn initial_tier(&self) -> EscalationTier {
        self.config.base_tier
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
            || ctx.status == 429
            || ctx.has_cloudflare_challenge
            || ctx.has_captcha
            || (ctx.body_empty && current >= EscalationTier::HttpTlsProfiled);

        if needs_escalation {
            let next = current.next()?;
            tracing::info!(
                status = ctx.status,
                current_tier = %current,
                next_tier = %next,
                "escalating request to higher tier"
            );
            Some(next)
        } else {
            None
        }
    }

    fn max_tier(&self) -> EscalationTier {
        self.config.max_tier
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn default_policy() -> DefaultEscalationPolicy {
        DefaultEscalationPolicy::new(EscalationConfig::default())
    }

    fn ok_ctx(status: u16) -> ResponseContext {
        ResponseContext {
            status,
            body_empty: false,
            has_cloudflare_challenge: false,
            has_captcha: false,
        }
    }

    // ── EscalationPolicy trait ────────────────────────────────────────────────

    #[test]
    fn initial_tier_returns_base() {
        assert_eq!(default_policy().initial_tier(), EscalationTier::HttpPlain);
    }

    #[test]
    fn status_200_no_markers_does_not_escalate() {
        let policy = default_policy();
        assert!(
            policy
                .should_escalate(&ok_ctx(200), EscalationTier::HttpPlain)
                .is_none()
        );
    }

    #[test]
    fn status_403_triggers_escalation() {
        let policy = default_policy();
        assert_eq!(
            policy.should_escalate(&ok_ctx(403), EscalationTier::HttpPlain),
            Some(EscalationTier::HttpTlsProfiled),
        );
    }

    #[test]
    fn status_429_triggers_escalation() {
        let policy = default_policy();
        assert_eq!(
            policy.should_escalate(&ok_ctx(429), EscalationTier::HttpPlain),
            Some(EscalationTier::HttpTlsProfiled),
        );
    }

    #[test]
    fn cloudflare_challenge_escalates_from_tls_profiled() {
        let policy = default_policy();
        let ctx = ResponseContext {
            status: 200,
            body_empty: false,
            has_cloudflare_challenge: true,
            has_captcha: false,
        };
        assert_eq!(
            policy.should_escalate(&ctx, EscalationTier::HttpTlsProfiled),
            Some(EscalationTier::BrowserBasic),
        );
    }

    #[test]
    fn captcha_escalates_from_browser_basic() {
        let policy = default_policy();
        let ctx = ResponseContext {
            status: 200,
            body_empty: false,
            has_cloudflare_challenge: false,
            has_captcha: true,
        };
        assert_eq!(
            policy.should_escalate(&ctx, EscalationTier::BrowserBasic),
            Some(EscalationTier::BrowserAdvanced),
        );
    }

    #[test]
    fn max_tier_cap_prevents_further_escalation() {
        let policy = DefaultEscalationPolicy::new(EscalationConfig {
            max_tier: EscalationTier::BrowserBasic,
            ..EscalationConfig::default()
        });
        // At max_tier, must not escalate even on 403
        assert!(
            policy
                .should_escalate(&ok_ctx(403), EscalationTier::BrowserBasic)
                .is_none()
        );
    }

    #[test]
    fn empty_body_at_http_plain_does_not_escalate() {
        let policy = default_policy();
        let ctx = ResponseContext {
            status: 200,
            body_empty: true,
            has_cloudflare_challenge: false,
            has_captcha: false,
        };
        // Empty body only triggers escalation at HttpTlsProfiled+
        assert!(
            policy
                .should_escalate(&ctx, EscalationTier::HttpPlain)
                .is_none()
        );
    }

    #[test]
    fn empty_body_at_tls_profiled_triggers_escalation() {
        let policy = default_policy();
        let ctx = ResponseContext {
            status: 200,
            body_empty: true,
            has_cloudflare_challenge: false,
            has_captcha: false,
        };
        assert_eq!(
            policy.should_escalate(&ctx, EscalationTier::HttpTlsProfiled),
            Some(EscalationTier::BrowserBasic),
        );
    }

    // ── Domain cache ──────────────────────────────────────────────────────────

    #[test]
    fn domain_cache_starts_at_base_tier() {
        let policy = default_policy();
        assert_eq!(
            policy.initial_tier_for_domain("example.com"),
            EscalationTier::HttpPlain
        );
    }

    #[test]
    fn domain_cache_returns_recorded_tier() {
        let policy = default_policy();
        policy.record_tier_success("guarded.io", EscalationTier::BrowserBasic);
        assert_eq!(
            policy.initial_tier_for_domain("guarded.io"),
            EscalationTier::BrowserBasic
        );
    }

    #[test]
    fn domain_cache_does_not_regress() {
        let policy = default_policy();
        policy.record_tier_success("strict.io", EscalationTier::BrowserAdvanced);
        policy.record_tier_success("strict.io", EscalationTier::BrowserBasic); // lower — ignore
        assert_eq!(
            policy.initial_tier_for_domain("strict.io"),
            EscalationTier::BrowserAdvanced
        );
    }

    #[test]
    fn record_base_tier_does_not_pollute_cache() {
        let policy = default_policy();
        policy.record_tier_success("plain.io", EscalationTier::HttpPlain);
        // base tier should not be cached (no meaningful skip)
        assert_eq!(
            policy.initial_tier_for_domain("plain.io"),
            EscalationTier::HttpPlain
        );
    }

    #[test]
    fn purge_expired_removes_entries() {
        let policy = DefaultEscalationPolicy::new(EscalationConfig {
            cache_ttl: Duration::from_millis(1),
            ..EscalationConfig::default()
        });
        policy.record_tier_success("fast-expiry.com", EscalationTier::BrowserBasic);
        std::thread::sleep(Duration::from_millis(10));
        let removed = policy.purge_expired_cache();
        assert_eq!(removed, 1);
        // After purge, domain reverts to base tier
        assert_eq!(
            policy.initial_tier_for_domain("fast-expiry.com"),
            EscalationTier::HttpPlain
        );
    }

    // ── context_from_body ─────────────────────────────────────────────────────

    #[test]
    fn context_from_body_detects_cloudflare() {
        let body = "<html><title>Just a moment...</title></html>";
        let ctx = DefaultEscalationPolicy::context_from_body(403, body);
        assert!(ctx.has_cloudflare_challenge);
        assert_eq!(ctx.status, 403);
        assert!(!ctx.body_empty);
    }

    #[test]
    fn context_from_body_detects_perimeterx() {
        let body = r#"<script src="/_px.js"></script>"#;
        let ctx = DefaultEscalationPolicy::context_from_body(200, body);
        assert!(ctx.has_cloudflare_challenge);
    }

    #[test]
    fn context_from_body_detects_datadome() {
        let body = r#"<meta name="datadome" content="protected">"#;
        let ctx = DefaultEscalationPolicy::context_from_body(200, body);
        assert!(ctx.has_cloudflare_challenge);
    }

    #[test]
    fn context_from_body_detects_captcha() {
        let body = r#"<script src="hcaptcha.com/1/api.js"></script>"#;
        let ctx = DefaultEscalationPolicy::context_from_body(200, body);
        assert!(ctx.has_captcha);
        assert!(!ctx.has_cloudflare_challenge);
    }

    #[test]
    fn context_from_body_empty_whitespace() {
        let ctx = DefaultEscalationPolicy::context_from_body(200, "   \n  ");
        assert!(ctx.body_empty);
    }

    // ── Detection helper coverage ─────────────────────────────────────────────

    #[test]
    fn detection_helpers_match_markers() {
        assert!(is_cloudflare_challenge("Just a moment..."));
        assert!(is_cloudflare_challenge("cf-browser-verification token"));
        assert!(is_datadome_interstitial("window.datadome = {}"));
        assert!(is_perimeterx_challenge("var _pxParam1 = 'abc'"));
        assert!(has_captcha_marker("www.google.com/recaptcha/api.js"));
        assert!(has_captcha_marker("turnstile.cloudflare.com"));
    }
}
