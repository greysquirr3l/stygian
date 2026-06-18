//! Interstitial routing policy and route schema.
//!
//! Defines the [`InterstitialPolicy`] (queue / challenge /
//! hard-block / transient tunables) and the
//! [`InterstitialRoute`] enum (the dedicated strategy per
//! [`InterstitialKind`]) plus the [`InterstitialSeverity`]
//! tier (the observability discriminator that tells
//! downstream tooling whether the classified state is
//! retryable, requires solving, or is terminal).
//!
//! ## Severity tier vs classification kind
//!
//! The severity tier is a **dedicated field** on
//! [`RouterDecision`] (see
//! [`RouterDecision::severity`][super::RouterDecision::severity])
//! that groups [`InterstitialKind`]s by their
//! **operational** meaning rather than their structural
//! classification. Observability tooling can therefore
//! distinguish "queue (retryable wait)" from "hard block
//! (terminal escalation)" by reading the dedicated severity
//! field without branching on the kind enum.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::acquisition::StrategyUsed;

/// Default wait interval between queue retries.
///
/// 5 seconds is a safe "polite" default for the
/// "Please wait..." / queue-position interstitials. The
/// caller can shorten this via
/// [`InterstitialPolicy::with_queue_interval`].
pub const DEFAULT_QUEUE_INTERVAL_MS: u64 = 5_000;

/// Default maximum retries for a queue page.
///
/// Three retries matches the documented
/// "wait, retry, escalate" cadence. The caller can override
/// via [`InterstitialPolicy::with_queue_max_retries`].
pub const DEFAULT_QUEUE_MAX_RETRIES: u32 = 3;

/// Default challenge solve budget.
///
/// 30 seconds is enough to solve a captcha / turnstile
/// challenge via the [`StrategyUsed::StickyProxyBrowserSession`]
/// stage. The caller can override via
/// [`InterstitialPolicy::with_challenge_solve_budget`].
pub const DEFAULT_CHALLENGE_SOLVE_BUDGET_MS: u64 = 30_000;

/// Default strategy to escalate to on a hard block.
///
/// Browser + sticky is the most expensive strategy, so the
/// default escalation is conservative — the caller is
/// expected to use the `Escalate` route as a last-resort
/// signal rather than a routine retry path.
pub const DEFAULT_HARD_BLOCK_ESCALATION: StrategyUsed =
    StrategyUsed::StickyProxyBrowserSession;

/// Default follow-redirect flag for transient pages.
pub const DEFAULT_TRANSIENT_FOLLOW_REDIRECT: bool = true;

/// Default max redirect hops to follow on a transient page.
pub const DEFAULT_MAX_TRANSIENT_HOPS: u32 = 3;

/// Classification kind for an interstitial page.
///
/// One of four shapes the classifier emits. The
/// [`InterstitialRouter`][super::InterstitialRouter] maps
/// each kind to a dedicated [`InterstitialRoute`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterstitialKind {
    /// "Please wait..." / waiting-room page. Retryable.
    Queue,
    /// Vendor-issued challenge (captcha, turnstile, `PoW`).
    Challenge,
    /// Terminal vendor block page.
    HardBlock,
    /// Bounded 3xx redirect chain that should be followed
    /// before classifying the response.
    Transient,
}

impl InterstitialKind {
    /// Stable `snake_case` label used in telemetry output.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Queue => "queue",
            Self::Challenge => "challenge",
            Self::HardBlock => "hard_block",
            Self::Transient => "transient",
        }
    }

    /// `true` when the kind is [`Self::Queue`].
    #[must_use]
    pub const fn is_queue(self) -> bool {
        matches!(self, Self::Queue)
    }

    /// `true` when the kind is [`Self::HardBlock`].
    #[must_use]
    pub const fn is_hard_block(self) -> bool {
        matches!(self, Self::HardBlock)
    }

    /// `true` when the kind is [`Self::Challenge`].
    #[must_use]
    pub const fn is_challenge(self) -> bool {
        matches!(self, Self::Challenge)
    }

    /// `true` when the kind is [`Self::Transient`].
    #[must_use]
    pub const fn is_transient(self) -> bool {
        matches!(self, Self::Transient)
    }
}

impl fmt::Display for InterstitialKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Operational severity tier for an interstitial decision.
///
/// The tier is a **dedicated** observability field
/// ([`RouterDecision::severity`][super::RouterDecision::severity])
/// that groups [`InterstitialKind`]s by their operational
/// meaning. It is intentionally a separate enum from
/// [`InterstitialKind`] so downstream tooling can branch on
/// "retryable vs terminal" without re-deriving it from the
/// kind.
///
/// | Kind | Severity |
/// |---|---|
/// | [`Queue`][InterstitialKind::Queue] | [`Retryable`][Self::Retryable] |
/// | [`Transient`][InterstitialKind::Transient] | [`Retryable`][Self::Retryable] |
/// | [`Challenge`][InterstitialKind::Challenge] | [`RequiresSolve`][Self::RequiresSolve] |
/// | [`HardBlock`][InterstitialKind::HardBlock] | [`Terminal`][Self::Terminal] |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterstitialSeverity {
    /// The classified page is a wait / retry path. The
    /// runner may short-circuit and let the calling layer
    /// wait + retry.
    Retryable,
    /// The classified page is a vendor challenge that
    /// requires solving before the target document can be
    /// returned.
    RequiresSolve,
    /// The classified page is a terminal vendor block. The
    /// runner should escalate (rotate session, refresh
    /// sticky context, switch to the strongest strategy).
    Terminal,
}

impl InterstitialSeverity {
    /// Stable `snake_case` label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Retryable => "retryable",
            Self::RequiresSolve => "requires_solve",
            Self::Terminal => "terminal",
        }
    }

    /// Map a classification kind to its severity tier.
    #[must_use]
    pub const fn for_kind(kind: InterstitialKind) -> Self {
        match kind {
            InterstitialKind::Queue | InterstitialKind::Transient => Self::Retryable,
            InterstitialKind::Challenge => Self::RequiresSolve,
            InterstitialKind::HardBlock => Self::Terminal,
        }
    }
}

impl fmt::Display for InterstitialSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Dedicated acquisition route per [`InterstitialKind`].
///
/// Each variant carries the per-kind tunables. The route is
/// purely declarative — the actual acquisition ladder
/// adjustment is done by the calling layer (or by the
/// runner, when
/// [`InterstitialPolicy::short_circuit_on_classified`] is
/// `true`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "route")]
pub enum InterstitialRoute {
    /// Wait `interval` for up to `max_retries` attempts.
    /// Carries the optional `queue_position` (e.g. "you are
    /// #5 in line") so the caller can scale the wait by the
    /// position.
    WaitAndRetry {
        /// Wait interval between retries.
        #[serde(with = "duration_ms")]
        interval: Duration,
        /// Maximum retry attempts.
        max_retries: u32,
        /// Optional queue position hint extracted from the
        /// page (1-based, where 1 = first in line).
        queue_position: Option<u32>,
    },
    /// Escalate to a challenge-solving strategy with the
    /// given `solve_budget`. The optional `vendor_hint`
    /// narrows the strategy (e.g. `cloudflare`,
    /// `perimeterx`).
    ChallengeSolve {
        /// Optional vendor hint extracted from the page
        /// markers (e.g. `cloudflare`, `akamai`).
        vendor_hint: Option<String>,
        /// Strategies the caller may attempt.
        allowed_strategies: Vec<StrategyUsed>,
        /// Maximum wall-clock budget for the solve.
        #[serde(with = "duration_ms")]
        solve_budget: Duration,
    },
    /// Terminal vendor block. Escalate to
    /// `escalate_to`, optionally rotate the proxy session,
    /// and optionally invalidate the sticky pool context.
    HardBlock {
        /// Strategy to escalate to.
        escalate_to: StrategyUsed,
        /// Whether to rotate the proxy session.
        rotate_session: bool,
        /// Whether to invalidate the sticky pool context.
        refresh_sticky: bool,
    },
    /// Follow up to `max_hops` redirect hops and then
    /// re-classify the result.
    Transient {
        /// Whether to follow the redirect.
        follow_redirect: bool,
        /// Maximum redirect hops to follow.
        max_hops: u32,
    },
}

impl InterstitialRoute {
    /// Stable `snake_case` route name.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::WaitAndRetry { .. } => "wait_and_retry",
            Self::ChallengeSolve { .. } => "challenge_solve",
            Self::HardBlock { .. } => "hard_block",
            Self::Transient { .. } => "transient",
        }
    }
}

/// Routing tunables for [`InterstitialRouter`][super::InterstitialRouter].
///
/// All fields have safe defaults that the production
/// acquisition path uses unchanged. Callers can override
/// any field via the `with_*` builders.
///
/// # Example
///
/// ```
/// use stygian_browser::interstitial_router::InterstitialPolicy;
/// use std::time::Duration;
///
/// let policy = InterstitialPolicy {
///     queue_interval: Duration::from_secs(10),
///     ..InterstitialPolicy::default()
/// };
/// assert_eq!(policy.queue_interval, Duration::from_secs(10));
/// assert!(policy.short_circuit_on_classified);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterstitialPolicy {
    /// Wait interval between queue retries.
    #[serde(with = "duration_ms")]
    pub queue_interval: Duration,
    /// Maximum retries for a queue page.
    pub queue_max_retries: u32,
    /// Challenge solve budget.
    #[serde(with = "duration_ms")]
    pub challenge_solve_budget: Duration,
    /// Strategy to escalate to on a hard block.
    pub hard_block_escalation: StrategyUsed,
    /// Whether to follow redirects on a transient page.
    pub transient_follow_redirect: bool,
    /// Maximum redirect hops to follow on a transient page.
    pub max_transient_hops: u32,
    /// When `true` (default), a non-`Transient`
    /// classification short-circuits the runner with a
    /// structured
    /// [`StageFailureKind::InterstitialRouted`][crate::acquisition::StageFailureKind::InterstitialRouted]
    /// failure so the calling layer can route via the
    /// dedicated strategy. When `false`, the decision is
    /// only attached to the result — the runner still
    /// executes the strategy ladder.
    pub short_circuit_on_classified: bool,
}

impl Default for InterstitialPolicy {
    fn default() -> Self {
        Self {
            queue_interval: Duration::from_millis(DEFAULT_QUEUE_INTERVAL_MS),
            queue_max_retries: DEFAULT_QUEUE_MAX_RETRIES,
            challenge_solve_budget: Duration::from_millis(DEFAULT_CHALLENGE_SOLVE_BUDGET_MS),
            hard_block_escalation: DEFAULT_HARD_BLOCK_ESCALATION,
            transient_follow_redirect: DEFAULT_TRANSIENT_FOLLOW_REDIRECT,
            max_transient_hops: DEFAULT_MAX_TRANSIENT_HOPS,
            short_circuit_on_classified: true,
        }
    }
}

impl InterstitialPolicy {
    /// Build a policy with an explicit queue interval.
    #[must_use]
    pub const fn with_queue_interval(mut self, interval: Duration) -> Self {
        self.queue_interval = interval;
        self
    }

    /// Build a policy with an explicit max retries value.
    #[must_use]
    pub const fn with_queue_max_retries(mut self, max_retries: u32) -> Self {
        self.queue_max_retries = max_retries;
        self
    }

    /// Build a policy with an explicit challenge solve
    /// budget.
    #[must_use]
    pub const fn with_challenge_solve_budget(mut self, budget: Duration) -> Self {
        self.challenge_solve_budget = budget;
        self
    }

    /// Build a policy with an explicit hard-block
    /// escalation strategy.
    #[must_use]
    pub const fn with_hard_block_escalation(mut self, strategy: StrategyUsed) -> Self {
        self.hard_block_escalation = strategy;
        self
    }

    /// Build a policy with an explicit follow-redirect flag.
    #[must_use]
    pub const fn with_transient_follow_redirect(mut self, follow: bool) -> Self {
        self.transient_follow_redirect = follow;
        self
    }

    /// Build a policy with an explicit max-hops value.
    #[must_use]
    pub const fn with_max_transient_hops(mut self, max_hops: u32) -> Self {
        self.max_transient_hops = max_hops;
        self
    }

    /// Build a policy with an explicit short-circuit flag.
    #[must_use]
    pub const fn with_short_circuit(mut self, short_circuit: bool) -> Self {
        self.short_circuit_on_classified = short_circuit;
        self
    }
}

/// serde helper: serialise [`Duration`] as integer
/// milliseconds.
mod duration_ms {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

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
