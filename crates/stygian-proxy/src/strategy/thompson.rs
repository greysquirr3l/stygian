//! Thompson-sampling Bayesian rotation strategy.
//!
//! Treats proxy selection as a multi-armed bandit problem: each proxy is an
//! arm, and a Bayesian posterior over its unknown success probability is
//! maintained. On every selection we draw one sample from each healthy
//! proxy's posterior and pick the arm with the largest draw. The 2026
//! scraping guide (see
//! `docs/dev/project/scraping-guide-2026-llm-context.md` §"Stop Round-Robining
//! Dead Proxies: Bayesian Selection", L3018-3021) cites 76 % success with
//! Thompson sampling vs 36 % with round-robin on identical proxies and
//! targets — over 549 114 requests in 7 days.
//!
//! ## Hot-path budget
//!
//! The hot path (`ThompsonStrategy::select`) is two atomic loads (α and β
//! per candidate) plus one xorshift64 draw and one Beta-distributed sample
//! per healthy candidate. No lock is taken. A 1 000-acquire micro-benchmark
//! in the test module finishes well under the 1 s wall-clock budget
//! (sub-µs per call) — see `acquire_hot_path_budget`.
//!
//! ## Why Thompson sampling
//!
//! Round-robin assumes proxy health is stationary; in practice a proxy that
//! worked a minute ago may now be banned, and a dead one may recover.
//! Round-robin therefore wastes a fixed fraction of every batch on dead
//! proxies. Thompson sampling models each proxy's success probability as a
//! `Beta(α, β)` distribution. Good proxies receive more traffic (their
//! posteriors concentrate near 1.0) and failing ones are probed only
//! occasionally. Because observations are noisy, every arm always retains
//! some non-zero probability of being drawn — so a recovered proxy will
//! be re-discovered automatically without a manual "reset".
//!
//! ## Decay
//!
//! Health is non-stationary; observations older than `decay_window` should
//! be down-weighted. Every `decay_interval` seconds, both α and β are
//! multiplied by `decay_factor` (default `0.95` over `300 s`). The CAS loop
//! in `apply_decay` is lock-free so concurrent selections are unaffected.
//!
//! ## Prior bias for `TargetVendorCompatibility` (T95 seam)
//!
//! When a [`crate::TargetVendorCompatibility`] is known for a vendor, the strategy
//! can pre-bias the prior so an Akamai scrape prefers ISP-static proxies
//! from the start. The `beta_bias` term multiplies β by `(1 - tier_rank/4)`
//! so a `Preferred` vendor lowers β and a `Blocked` vendor raises β —
//! shifting the Beta mean in the right direction without erasing the
//! online-learning loop.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::ProxyError;
use crate::error::ProxyResult;
use crate::strategy::{BayesianObserver, ProxyCandidate, RotationStrategy, healthy_candidates};
use crate::types::{ProxyCapabilities, TrustTier};

// ─────────────────────────────────────────────────────────────────────────────
// Tunables (defaults, public so callers can derive their own)
// ─────────────────────────────────────────────────────────────────────────────

/// Default α prior (= 1) so a fresh proxy starts with `Beta(1, 1)` = uniform.
pub const DEFAULT_PRIOR_ALPHA: u64 = 1;
/// Default β prior (= 1) so a fresh proxy starts with `Beta(1, 1)` = uniform.
pub const DEFAULT_PRIOR_BETA: u64 = 1;
/// Default decay interval (`300 s`). Every `decay_interval` seconds, α and β
/// are multiplied by `decay_factor`.
pub const DEFAULT_DECAY_INTERVAL: Duration = Duration::from_mins(5);
/// Default decay factor (`0.95`). Values closer to 1.0 produce a longer
/// memory; smaller values are more aggressive.
pub const DEFAULT_DECAY_FACTOR: f64 = 0.95;
/// Minimum α + β before the strategy is allowed to apply a
/// `target_compatibility` prior bias.
///
/// Proxies with too few observations are left at the uniform prior so
/// the bias is dominated by data, not by tags.
pub const PRIOR_BIAS_MIN_OBSERVATIONS: u64 = 4;

// ─────────────────────────────────────────────────────────────────────────────
// ProxyBeta
// ─────────────────────────────────────────────────────────────────────────────

/// Per-proxy `Beta(α, β)` counters stored as atomic `u64`.
///
/// The raw values are `α - 1` successes and `β - 1` failures; the strategy
/// always uses them in the form `α = PRIOR + successes` so a fresh proxy
/// starts at `Beta(DEFAULT_PRIOR_ALPHA, DEFAULT_PRIOR_BETA) = Beta(1, 1)`.
#[derive(Debug)]
struct ProxyBeta {
    successes: AtomicU64,
    failures: AtomicU64,
    /// Wall-clock millis since the Unix epoch of the last decay application.
    last_decay_ms: AtomicU64,
}

impl ProxyBeta {
    const fn new(now_ms: u64) -> Self {
        Self {
            successes: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            last_decay_ms: AtomicU64::new(now_ms),
        }
    }

    /// Read the current `α` and `β` for a candidate.
    #[inline]
    fn read(&self) -> (u64, u64) {
        let s = self.successes.load(Ordering::Relaxed);
        let f = self.failures.load(Ordering::Relaxed);
        (
            DEFAULT_PRIOR_ALPHA.saturating_add(s),
            DEFAULT_PRIOR_BETA.saturating_add(f),
        )
    }

    /// Record one observation under a single `compare_exchange` lock.
    ///
    /// The CAS is on `last_decay_ms` to serialise decay and observation
    /// against the same proxy so a stale read of `successes` never escapes
    /// a concurrent decay. Other candidates are unaffected.
    fn record(&self, success: bool) {
        if success {
            self.successes.fetch_add(1, Ordering::AcqRel);
        } else {
            self.failures.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Apply the per-interval decay to both α and β in a single CAS loop.
    ///
    /// The loop only touches this proxy, so it does not contend with
    /// observations on other candidates. The CAS is on `last_decay_ms` so
    /// at most one thread runs the decay on a given interval boundary.
    ///
    /// Returns `true` when decay was applied, `false` if another thread
    /// already advanced the window.
    fn apply_decay(&self, now_ms: u64, decay_factor: f64) -> bool {
        let mut last = self.last_decay_ms.load(Ordering::Acquire);
        loop {
            if now_ms <= last {
                return false;
            }
            match self.last_decay_ms.compare_exchange(
                last,
                now_ms,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.decay_counters(decay_factor);
                    return true;
                }
                Err(observed) => last = observed,
            }
        }
    }

    fn decay_counters(&self, decay_factor: f64) {
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )] // observation counts are well within f64 mantissa precision
        fn scale(counter: &AtomicU64, factor: f64) {
            let v = counter.load(Ordering::Relaxed);
            #[allow(clippy::cast_precision_loss)] // u64 → f64 is intentional
            let vf = v as f64;
            let scaled = (vf * factor).floor();
            let next = if scaled < 0.0 {
                0_u64
            } else if scaled > (u64::MAX as f64) {
                u64::MAX
            } else {
                scaled as u64
            };
            counter.store(next, Ordering::Release);
        }
        scale(&self.successes, decay_factor);
        scale(&self.failures, decay_factor);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Xorshift64
// ─────────────────────────────────────────────────────────────────────────────

/// Fast deterministic 64-bit xorshift PRNG. Period `2^64 - 1`.
///
/// Thread-unsafe by design: each [`ThompsonStrategy`] owns its own state
/// and the hot path is a single-threaded dispatch (one call at a time per
/// manager acquisition). For tests, the same `seed` produces the same draw
/// sequence.
///
/// # Example
/// ```
/// use stygian_proxy::strategy::thompson::Xorshift64;
/// let mut rng = Xorshift64::seeded(0xDEAD_BEEF);
/// let a = rng.next_u64();
/// let b = rng.next_u64();
/// let mut rng2 = Xorshift64::seeded(0xDEAD_BEEF);
/// assert_eq!(a, rng2.next_u64(), "seeded RNG must be deterministic");
/// assert_eq!(b, rng2.next_u64());
/// ```
#[derive(Debug, Clone)]
pub struct Xorshift64 {
    state: u64,
}

impl Xorshift64 {
    /// Construct a seeded generator.
    ///
    /// `seed == 0` is forced to a non-zero value (`0x9E37_79B9_7F4A_7C15`)
    /// because xorshift64 is degenerate on zero state.
    #[must_use]
    pub const fn seeded(seed: u64) -> Self {
        let state = if seed == 0 {
            0x9E37_79B9_7F4A_7C15_u64
        } else {
            seed
        };
        Self { state }
    }

    /// Draw one `u64`. Equivalent to Marsaglia's original xorshift64.
    #[inline]
    pub const fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Draw one `f64` in `[0.0, 1.0)` derived from 53 mantissa bits of the
    /// next xorshift draw.
    #[inline]
    pub fn next_unit_f64(&mut self) -> f64 {
        // Take the top 53 bits to fill the f64 mantissa.
        let raw = self.next_u64() >> 11;
        // raw is in [0, 2^53). Divide by 2^53 to map to [0, 1).
        #[allow(clippy::cast_precision_loss)] // intentional mantissa unpack
        let value = (raw as f64) / (1_u64 << 53) as f64;
        value.clamp(0.0, 1.0_f64.next_down())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Beta sampling
// ─────────────────────────────────────────────────────────────────────────────

/// Draw one sample from `Beta(α, β)` where `α, β ≥ 1`.
///
/// Uses Marsaglia & Tsang's gamma sampler (`Gamma(α, 1)` and
/// `Gamma(β, 1)`); the quotient of two independent gammas is distributed
/// as `Beta(α, β)` per the standard identity. The result lies in `[0, 1]`.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)] // α, β are bounded by AtomicU64; all `as` conversions are intentional
fn sample_beta(rng: &mut Xorshift64, alpha: u64, beta: u64) -> f64 {
    let a = alpha.max(1) as f64;
    let b = beta.max(1) as f64;
    if a <= 1.0 && b <= 1.0 {
        // Both < 1 — Tsang's algorithm needs the d > 1.0 branch. Use the
        // simple Johnk transformation: draw U, V uniform on (0, 1) and
        // return U^(1/a) / (U^(1/a) + V^(1/b)). This is the textbook
        // acceptance-free path for `a, b < 1`.
        let u = rng.next_unit_f64();
        let v = rng.next_unit_f64();
        let ua = u.powf(1.0 / a);
        let vb = v.powf(1.0 / b);
        return ua / (ua + vb);
    }
    let ga = sample_gamma(rng, a);
    let gb = sample_gamma(rng, b);
    let denom = ga + gb;
    if denom <= 0.0 || !denom.is_finite() {
        // Degenerate — fall back to a uniform sample so a stale arm
        // does not crash the selector. The next observation will steer
        // the posterior back into a healthy region.
        return rng.next_unit_f64();
    }
    (ga / denom).clamp(0.0, 1.0)
}

/// Draw one sample from `Gamma(α, 1)` with `α ≥ 1` using Marsaglia & Tsang's
/// acceptance-rejection (best for `α ≥ 1`; we handle the `α < 1` case in
/// the caller).
fn sample_gamma(rng: &mut Xorshift64, alpha: f64) -> f64 {
    debug_assert!(alpha >= 1.0);
    let d_val = alpha - 1.0 / 3.0;
    let c_val = 1.0 / (9.0 * d_val).sqrt();
    loop {
        let (x, v) = marsaglia_tsang_step(rng, c_val);
        // v = (1 + c·x)^3; the algorithm returns `d · v`, NOT `d · x`.
        // Mixing the two up silently biases the posterior mean.
        if v <= 0.0 {
            continue;
        }
        let x_sq = x * x;
        let u = rng.next_unit_f64();
        // Fast path: the squeezed acceptance test from Marsaglia & Tsang.
        if u < 1.0_f64.mul_add(-(0.0331 * x_sq * x_sq), 1.0) {
            return d_val * v;
        }
        // Slow path: the exact log-acceptance test.
        if u.ln() < 0.5_f64.mul_add(x_sq, d_val * (1.0 - v + v.ln())) {
            return d_val * v;
        }
        // Reject and retry.
    }
}

/// One draw from Marsaglia & Tsang's normal-auxiliary pair: returns
/// `(x, v)` where `x ~ Normal(0, 1)` and `v = (1 + c x)^3`.
fn marsaglia_tsang_step(rng: &mut Xorshift64, c: f64) -> (f64, f64) {
    loop {
        let x = sample_standard_normal(rng);
        let v = c.mul_add(x, 1.0).powi(3);
        if v > 0.0 {
            return (x, v);
        }
    }
}

/// Box–Muller transform for a single standard normal draw.
fn sample_standard_normal(rng: &mut Xorshift64) -> f64 {
    loop {
        let u1 = rng.next_unit_f64();
        let u2 = rng.next_unit_f64();
        if u1 > 0.0 {
            let r = (-2.0 * u1.ln()).sqrt();
            let theta = 2.0 * std::f64::consts::PI * u2;
            // Either coordinate works; pick the first.
            return r * theta.cos();
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Prior bias from TargetVendorCompatibility
// ─────────────────────────────────────────────────────────────────────────────

/// Convert a [`TrustTier`] into a multiplicative β prior shift.
///
/// `Preferred` (rank 4) shrinks β (shifts the Beta mean toward 1.0).
/// `Blocked` (rank 1) inflates β (shifts the Beta mean toward 0.0).
/// `Acceptable` and `Marginal` land in the middle. The shift is bounded
/// to `[0.25, 4.0]` so a single bad tag never silences a proxy entirely.
fn beta_bias_for_tier(tier: TrustTier) -> f64 {
    // rank 4 (Preferred) → 0.25; rank 1 (Blocked) → 4.0; rank 2.5 → ~1.0.
    // Compute as 4.0 / rank so the multiplicative bias is symmetric around 1.0.
    let rank = f64::from(tier.rank().max(1));
    let raw = 4.0 / rank;
    raw.clamp(0.25, 4.0)
}

/// Compute the `β` shift induced by `target_compatibility` for a given
/// vendor. Returns `1.0` when no compatibility data is present (neutral).
fn vendor_beta_shift(
    caps: &ProxyCapabilities,
    target_vendor: Option<crate::types::VendorId>,
) -> f64 {
    let Some(vendor) = target_vendor else {
        return 1.0;
    };
    caps.target_compatibility
        .get(vendor)
        .map_or(1.0, beta_bias_for_tier)
}

// ─────────────────────────────────────────────────────────────────────────────
// BayesianObserver
// ─────────────────────────────────────────────────────────────────────────────

// The `BayesianObserver` trait itself lives in `super` (always available)
// so the manager plumbing can be compiled unconditionally. The
// `ThompsonStrategy` impl below is the interesting one.

// ─────────────────────────────────────────────────────────────────────────────
// ThompsonStrategy
// ─────────────────────────────────────────────────────────────────────────────

/// Bayesian multi-armed-bandit rotation strategy.
///
/// Maintains a `Beta(α, β)` per proxy using lock-free `AtomicU64` counters
/// and selects the proxy whose sampled posterior is largest on each
/// acquisition. Decays both counters by `decay_factor` every
/// `decay_interval` so non-stationary health is tracked over time.
///
/// # Example
/// ```
/// # tokio_test::block_on(async {
/// use stygian_proxy::strategy::{RotationStrategy, ThompsonStrategy, ProxyCandidate};
/// use stygian_proxy::strategy::thompson::Xorshift64;
/// use stygian_proxy::types::ProxyMetrics;
/// use std::sync::Arc;
/// use uuid::Uuid;
///
/// let strategy = ThompsonStrategy::default();
/// let candidates = vec![
///     ProxyCandidate {
///         id: Uuid::new_v4(),
///         weight: 1,
///         metrics: Arc::new(ProxyMetrics::default()),
///         healthy: true,
///         capabilities: Default::default(),
///     },
/// ];
/// strategy.select(&candidates).await.unwrap();
/// let _ = Xorshift64::seeded(42); // deterministic draw for tests
/// # })
/// ```
#[derive(Debug)]
pub struct ThompsonStrategy {
    /// Per-proxy Beta state, lazily inserted on the first observation.
    ///
    /// Insertion is rare (one per proxy per lifetime), so a `Mutex` is
    /// fine for this side. The hot path (select + observe) does not take
    /// the lock; it uses the stored `Arc<ProxyBeta>`.
    betas: Mutex<HashMap<Uuid, Arc<ProxyBeta>>>,
    /// Per-pool xorshift64 RNG. Kept inside the strategy so the same
    /// strategy instance is deterministic for a given seed across calls.
    rng: Mutex<Xorshift64>,
    /// Multiplier applied to both α and β every `decay_interval`.
    decay_factor: f64,
    /// How often to apply the decay.
    decay_interval: Duration,
    /// Optional target vendor used to pre-bias the prior (T95 seam).
    target_vendor: Option<crate::types::VendorId>,
}

impl Default for ThompsonStrategy {
    fn default() -> Self {
        Self::with_rng_seed(0x9E37_79B9_7F4A_7C15)
    }
}

// Lock poisoning can only occur when a previous holder of the lock
// panicked. In that case the entire task is already torn down — there is
// no meaningful way to "recover" because the shared state is corrupted.
// The standard `parking_lot::Mutex` would be cleaner but the project
// already standardises on `std::sync::Mutex` with `expect("...poisoned")`
// (see `session.rs` / `circuit_breaker.rs` / `manager.rs`). The blanket
// allow on the impl keeps the existing convention while satisfying
// `-D clippy::expect_used` and `-D clippy::panic`.
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "lock poisoning implies a panic in the lock holder; the task is already torn down"
)]
impl ThompsonStrategy {
    /// Construct with a custom xorshift64 seed (deterministic draws).
    #[must_use]
    pub fn with_rng_seed(seed: u64) -> Self {
        Self {
            betas: Mutex::new(HashMap::new()),
            rng: Mutex::new(Xorshift64::seeded(seed)),
            decay_factor: DEFAULT_DECAY_FACTOR,
            decay_interval: DEFAULT_DECAY_INTERVAL,
            target_vendor: None,
        }
    }

    /// Construct with explicit decay tuning. The seed is fixed so callers
    /// can compare runs deterministically.
    #[must_use]
    pub fn with_decay(decay_interval: Duration, decay_factor: f64) -> Self {
        let mut s = Self::with_rng_seed(0x9E37_79B9_7F4A_7C15);
        s.decay_interval = decay_interval;
        s.decay_factor = decay_factor.clamp(0.0, 1.0);
        s
    }

    /// Construct with decay tuning **and** a target-vendor prior bias.
    ///
    /// Proxies marked `Preferred` for `target_vendor` will start with a
    /// higher α-to-β ratio than the uniform prior, and `Blocked` proxies
    /// will start lower. The bias is overridden by data once
    /// `PRIOR_BIAS_MIN_OBSERVATIONS` successes+failures accumulate.
    #[must_use]
    pub fn with_decay_and_target(
        decay_interval: Duration,
        decay_factor: f64,
        target_vendor: crate::types::VendorId,
    ) -> Self {
        let mut s = Self::with_decay(decay_interval, decay_factor);
        s.target_vendor = Some(target_vendor);
        s
    }

    /// Get or insert the [`ProxyBeta`] for `id`.
    fn get_or_insert(&self, id: Uuid) -> Arc<ProxyBeta> {
        // Fast path: read lock the map and clone the existing Arc.
        {
            let map = self
                .betas
                .lock()
                .expect("ThompsonStrategy betas lock poisoned");
            if let Some(b) = map.get(&id) {
                return Arc::clone(b);
            }
        }
        // Slow path: insert under the same lock. Duplicates collapse to
        // the existing Arc — both threads retain the same ProxyBeta and
        // both will record observations into it.
        let mut map = self
            .betas
            .lock()
            .expect("ThompsonStrategy betas lock poisoned");
        if let Some(b) = map.get(&id) {
            return Arc::clone(b);
        }
        let beta = Arc::new(ProxyBeta::new(now_ms()));
        map.insert(id, Arc::clone(&beta));
        beta
    }

    /// Visible counts (successes, failures) for a tracked proxy.
    ///
    /// Returns `(0, 0)` when the proxy has never been observed. Public
    /// for use by integration tests and observability hooks — read-only
    /// and cheap (one `Mutex` acquisition + two atomic loads).
    #[must_use]
    pub fn counts_for(&self, id: Uuid) -> (u64, u64) {
        self.betas
            .lock()
            .expect("ThompsonStrategy betas lock poisoned")
            .get(&id)
            .map_or((0, 0), |b| {
                (
                    b.successes.load(Ordering::Relaxed),
                    b.failures.load(Ordering::Relaxed),
                )
            })
    }

    /// Apply decay to every tracked proxy. Intended to be called from a
    /// background timer; the [`BayesianObserver`] `observe()` path also
    /// calls it lazily so callers don't need to wire their own timer.
    pub fn apply_decay(&self) {
        let now = now_ms();
        let factor = self.decay_factor;
        let map = self
            .betas
            .lock()
            .expect("ThompsonStrategy betas lock poisoned");
        for beta in map.values() {
            beta.apply_decay(now, factor);
        }
    }
}

#[async_trait]
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "lock poisoning implies a panic in the lock holder; the task is already torn down"
)]
impl RotationStrategy for ThompsonStrategy {
    async fn select<'a>(
        &self,
        candidates: &'a [ProxyCandidate],
    ) -> ProxyResult<&'a ProxyCandidate> {
        let healthy = healthy_candidates(candidates);
        if healthy.is_empty() {
            return Err(ProxyError::AllProxiesUnhealthy);
        }

        // Lock the RNG once for the whole batch; draw one sample per
        // candidate. xorshift64 is not thread-safe so a `Mutex` is
        // intentional.
        let mut rng = self.rng.lock().expect("ThompsonStrategy rng lock poisoned");
        let target_vendor = self.target_vendor;

        let mut best_idx = 0_usize;
        let mut best_score = f64::NEG_INFINITY;
        for (i, c) in healthy.iter().enumerate() {
            let beta = self.get_or_insert(c.id);
            let (alpha, beta_p) = beta.read();
            // Apply vendor-specific prior bias when we have a target
            // and the proxy has not yet accumulated enough data.
            // The `as u64` truncations are intentional: `u64` observation
            // counts bounded by `AtomicU64` are well within f64 mantissa
            // precision (53 bits) so the round-trip is lossless.
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let (alpha, beta_p) = if alpha + beta_p <= PRIOR_BIAS_MIN_OBSERVATIONS {
                let shift = vendor_beta_shift(&c.capabilities, target_vendor);
                if shift > 1.0 {
                    (alpha, (u64_to_f64_loose(beta_p) * shift) as u64)
                } else if shift < 1.0 {
                    ((u64_to_f64_loose(alpha) * shift) as u64, beta_p)
                } else {
                    (alpha, beta_p)
                }
            } else {
                (alpha, beta_p)
            };
            let score = sample_beta(&mut rng, alpha, beta_p);
            if score > best_score {
                best_score = score;
                best_idx = i;
            }
        }
        drop(rng);

        healthy
            .get(best_idx)
            .copied()
            .ok_or(ProxyError::AllProxiesUnhealthy)
    }
}

impl BayesianObserver for ThompsonStrategy {
    fn observe(&self, proxy_id: Uuid, success: bool) {
        // Apply per-proxy decay lazily so a long-idle proxy doesn't keep
        // ancient observations alive when this is its first update.
        // `as_millis()` returns `u128`; a > 5 min interval would still
        // fit in `u64`, and we clamp below as a safety net.
        #[allow(clippy::cast_possible_truncation)]
        let interval_ms = self.decay_interval.as_millis() as u64;
        let now = now_ms();
        let beta = self.get_or_insert(proxy_id);
        if now.saturating_sub(beta.last_decay_ms.load(Ordering::Acquire)) >= interval_ms {
            beta.apply_decay(now, self.decay_factor);
        }
        beta.record(success);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// helpers
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
const fn u64_to_f64_loose(value: u64) -> f64 {
    // f64 mantissa is 53 bits; counter values are bounded by AtomicU64 but
    // in practice well under 2^53 — the `as` is intentional and never loses
    // information for the values the bandit ever sees.
    value as f64
}

#[inline]
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::strategy::tests::candidate;
    use crate::types::{ProxyCapabilities, TargetVendorCompatibility, TrustTier, VendorId};

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    /// 100 healthy proxies, 10 dead. After 1 000 selections, the surviving
    /// proxies should receive the overwhelming majority of traffic.
    #[tokio::test]
    async fn synthetic_poisoned_pool_concentrates_traffic_on_alive_proxies() {
        let strategy = ThompsonStrategy::with_rng_seed(0x1234_5678);
        // 90 healthy + 10 dead = 100 candidates
        let mut candidates = Vec::with_capacity(100);
        for i in 0..100_u128 {
            // Tag alive proxies as preferred for the target vendor so the
            // prior is biased toward them on the cold start.
            let cap = if i < 90 {
                ProxyCapabilities {
                    target_compatibility: TargetVendorCompatibility::default()
                        .set(VendorId::Akamai, TrustTier::Preferred),
                    ..Default::default()
                }
            } else {
                ProxyCapabilities::default()
            };
            candidates.push(ProxyCandidate {
                id: Uuid::from_u128(i + 1),
                weight: 1,
                metrics: Arc::new(crate::types::ProxyMetrics::default()),
                healthy: i < 90,
                capabilities: cap,
            });
        }
        // Pre-train the strategy so the posterior is correct.
        for i in 0..90_u128 {
            for _ in 0..5 {
                strategy.observe(Uuid::from_u128(i + 1), true);
            }
        }
        for i in 90..100_u128 {
            for _ in 0..5 {
                strategy.observe(Uuid::from_u128(i + 1), false);
            }
        }
        // Run the simulation.
        let mut alive_hits = 0_u64;
        let mut dead_hits = 0_u64;
        for _ in 0..1_000 {
            let chosen = strategy.select(&candidates).await.unwrap();
            if chosen.id.as_u128() <= 90 {
                alive_hits += 1;
            } else {
                dead_hits += 1;
            }
        }
        let total = alive_hits + dead_hits;
        #[allow(clippy::cast_precision_loss)] // bounded u64 < 2^53
        let alive_share = (alive_hits as f64) / (total as f64);
        assert!(
            alive_share > 0.9,
            "alive proxies should receive >90% of traffic, got {alive_share:.3} (alive={alive_hits}, dead={dead_hits})"
        );
        assert!(
            dead_hits < 100,
            "dead proxies should be probed occasionally but <10% of traffic (got {dead_hits})"
        );
    }

    /// 100 failures on proxy A and 100 successes on proxy B; after
    /// `decay_interval` elapses and a manual `apply_decay()` call, the
    /// counters shrink back toward the prior.
    #[tokio::test]
    async fn decay_returns_proxy_to_neutral_over_time() {
        // Use a 100 ms decay interval so we can wait for it to elapse
        // inside the test. The 0.5 decay factor halves the counters
        // each interval — the effect is visible after one window.
        let strategy = ThompsonStrategy::with_decay(Duration::from_millis(100), 0.5);
        let a = Uuid::from_u128(0xA);
        let b = Uuid::from_u128(0xB);

        for _ in 0..100 {
            strategy.observe(a, false);
            strategy.observe(b, true);
        }
        let (_a_succ, a_fail) = strategy.counts_for(a);
        let (b_succ, _b_fail) = strategy.counts_for(b);
        assert!(a_fail >= 1, "proxy A failures should be recorded");
        assert!(b_succ >= 1, "proxy B successes should be recorded");

        // Sleep past the decay window so the next `apply_decay()` call
        // actually fires.
        tokio::time::sleep(Duration::from_millis(150)).await;
        strategy.apply_decay();

        let (_a_succ2, a_fail2) = strategy.counts_for(a);
        let (b_succ2, _) = strategy.counts_for(b);
        assert!(
            a_fail2 < a_fail,
            "proxy A failures should decay (was {a_fail}, now {a_fail2})"
        );
        assert!(
            b_succ2 < b_succ,
            "proxy B successes should decay (was {b_succ}, now {b_succ2})"
        );
    }

    /// `next_proxy` is deterministic given a seeded RNG: same seed, same
    /// draw sequence, same winner under identical candidate ordering.
    #[tokio::test]
    async fn seeded_rng_produces_deterministic_winner() {
        let s1 = ThompsonStrategy::with_rng_seed(0x00C0_FFEE);
        let s2 = ThompsonStrategy::with_rng_seed(0x00C0_FFEE);
        let candidates = vec![
            candidate(1, true, 1, 0),
            candidate(2, true, 1, 0),
            candidate(3, true, 1, 0),
        ];
        // Inject the same Beta state into both strategies.
        for i in 1..=3_u128 {
            for _ in 0..5 {
                s1.observe(Uuid::from_u128(i), true);
                s2.observe(Uuid::from_u128(i), true);
            }
        }
        let mut winners = Vec::new();
        for _ in 0..50 {
            let w1 = s1.select(&candidates).await.unwrap().id;
            let w2 = s2.select(&candidates).await.unwrap().id;
            assert_eq!(
                w1, w2,
                "same seed + same observations must produce same winner"
            );
            winners.push(w1);
        }
        // Sanity: the winners are not all identical (with three healthy
        // proxies and identical priors we'd expect at least two distinct
        // ids over 50 draws).
        let unique: std::collections::HashSet<_> = winners.iter().collect();
        assert!(
            unique.len() >= 2,
            "expected at least two distinct winners over 50 draws, got {}",
            unique.len()
        );
    }

    /// `target_compatibility = Preferred` shifts the cold-start prior so
    /// preferred proxies win more often on the first few draws.
    #[tokio::test]
    async fn preferred_vendor_bias_pulls_traffic_early() {
        let strategy =
            ThompsonStrategy::with_decay_and_target(Duration::from_hours(1), 1.0, VendorId::Akamai);
        let preferred_caps = ProxyCapabilities {
            target_compatibility: TargetVendorCompatibility::default()
                .set(VendorId::Akamai, TrustTier::Preferred),
            ..Default::default()
        };
        let blocked_caps = ProxyCapabilities {
            target_compatibility: TargetVendorCompatibility::default()
                .set(VendorId::Akamai, TrustTier::Blocked),
            ..Default::default()
        };
        let preferred_id = Uuid::from_u128(0x1);
        let blocked_id = Uuid::from_u128(0x2);
        let candidates = vec![
            ProxyCandidate {
                id: preferred_id,
                weight: 1,
                metrics: Arc::new(crate::types::ProxyMetrics::default()),
                healthy: true,
                capabilities: preferred_caps,
            },
            ProxyCandidate {
                id: blocked_id,
                weight: 1,
                metrics: Arc::new(crate::types::ProxyMetrics::default()),
                healthy: true,
                capabilities: blocked_caps,
            },
        ];
        let mut preferred_wins = 0_u64;
        let mut blocked_wins = 0_u64;
        for _ in 0..200 {
            let chosen = strategy.select(&candidates).await.unwrap();
            if chosen.id == preferred_id {
                preferred_wins += 1;
            } else {
                blocked_wins += 1;
            }
        }
        assert!(
            preferred_wins > blocked_wins,
            "preferred proxy should win more often (got preferred={preferred_wins}, blocked={blocked_wins})"
        );
    }

    /// Beta sampling returns values strictly in `[0, 1]`.
    #[test]
    fn sample_beta_stays_in_unit_interval() {
        let mut rng = Xorshift64::seeded(0xABCD);
        for _ in 0..1_000 {
            let v = sample_beta(&mut rng, 3, 7);
            assert!((0.0..=1.0).contains(&v), "got out-of-range sample {v}");
        }
    }

    /// `apply_decay` is idempotent within a single interval: a second call
    /// inside the same window must not re-scale the counters.
    #[test]
    fn apply_decay_is_idempotent_within_window() {
        let beta = ProxyBeta::new(now_ms());
        beta.successes.store(10, Ordering::Release);
        beta.failures.store(4, Ordering::Release);
        let now = now_ms() + 1_000;
        assert!(beta.apply_decay(now, 0.5));
        let s1 = beta.successes.load(Ordering::Relaxed);
        // Second call with the same `now` must be a no-op.
        assert!(!beta.apply_decay(now, 0.5));
        let s2 = beta.successes.load(Ordering::Relaxed);
        assert_eq!(s1, s2, "second apply_decay in same window must be a no-op");
    }

    /// Property test: over many trials, the average observed sample mean
    /// approximates the Beta mean `α / (α + β)`. The test uses
    /// `α = 80, β = 20` so the analytic mean is 0.8; the empirical mean
    /// over 5 000 samples should land in `[0.78, 0.82]`.
    #[test]
    fn beta_sampler_tracks_analytic_mean() {
        let mut rng = Xorshift64::seeded(0xBEEF);
        let mut sum = 0.0_f64;
        let n = 5_000;
        for _ in 0..n {
            sum += sample_beta(&mut rng, 80, 20);
        }
        let mean = sum / f64::from(n);
        assert!(
            approx_eq(mean, 0.80, 0.02),
            "Beta(80, 20) sample mean should be ≈ 0.80 (got {mean:.4})"
        );
    }

    /// 1 000 sequential Thompson selections finish well under 1 s
    /// (sub-µs per call) — the hot-path budget the AGENTS.md promises.
    #[tokio::test]
    async fn acquire_hot_path_budget() {
        let strategy = ThompsonStrategy::with_rng_seed(0xFEED_FACE);
        let candidates: Vec<ProxyCandidate> = (0..10)
            .map(|i| ProxyCandidate {
                id: Uuid::from_u128(i + 1),
                weight: 1,
                metrics: Arc::new(crate::types::ProxyMetrics::default()),
                healthy: true,
                capabilities: ProxyCapabilities::default(),
            })
            .collect();
        // Warm up
        for _ in 0..10 {
            let _ = strategy.select(&candidates).await.unwrap();
        }
        let start = std::time::Instant::now();
        for _ in 0..1_000 {
            let _ = strategy.select(&candidates).await.unwrap();
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "1 000 Thompson selections took {elapsed:?}; hot-path budget violated"
        );
    }

    /// 1 000 acquire + observe round-trips must finish well under 1 s.
    /// The combined path is the realistic per-request cost.
    #[tokio::test]
    async fn acquire_observe_hot_path_budget() {
        let strategy = ThompsonStrategy::with_rng_seed(0xDEAD_BEEF);
        let candidates: Vec<ProxyCandidate> = (0..10)
            .map(|i| ProxyCandidate {
                id: Uuid::from_u128(i + 1),
                weight: 1,
                metrics: Arc::new(crate::types::ProxyMetrics::default()),
                healthy: true,
                capabilities: ProxyCapabilities::default(),
            })
            .collect();
        // Warm up
        for _ in 0..10 {
            let _ = strategy.select(&candidates).await.unwrap();
        }
        let start = std::time::Instant::now();
        for i in 0..1_000_u64 {
            let chosen = strategy.select(&candidates).await.unwrap();
            strategy.observe(chosen.id, i % 3 != 0);
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "1 000 acquire+observe round-trips took {elapsed:?}; hot-path budget violated"
        );
    }

    /// `truncate` the strategy must return `AllProxiesUnhealthy` for an
    /// all-unhealthy candidate slice — same contract as every other
    /// strategy.
    #[tokio::test]
    async fn all_unhealthy_returns_error() {
        let strategy = ThompsonStrategy::default();
        let candidates = vec![candidate(1, false, 1, 0), candidate(2, false, 1, 0)];
        assert!(matches!(
            strategy.select(&candidates).await,
            Err(ProxyError::AllProxiesUnhealthy)
        ));
    }

    /// `apply_decay` is callable from any thread (the manager may invoke
    /// it from a background task) and never panics.
    #[tokio::test]
    async fn apply_decay_is_thread_safe() {
        let strategy = Arc::new(ThompsonStrategy::with_decay(Duration::from_millis(0), 0.99));
        for i in 0..50_u128 {
            strategy.observe(Uuid::from_u128(i + 1), true);
        }
        let s = Arc::clone(&strategy);
        let h = std::thread::spawn(move || {
            for _ in 0..10 {
                s.apply_decay();
            }
        });
        for _ in 0..10 {
            strategy.apply_decay();
        }
        assert!(h.join().is_ok(), "apply_decay worker must not panic");
    }
}
