//! Request-count rate limiter for GraphQL API targets with pluggable algorithms.
//!
//! Two strategies are supported via [`RateLimitStrategy`]:
//!
//! - **[`SlidingWindow`](RateLimitStrategy::SlidingWindow)** — limits outgoing requests to
//!   `max_requests` in any rolling `window` duration.  Before each request
//!   [`rate_limit_acquire`] is called; it records the current timestamp and sleeps
//!   until the oldest in-window request expires if the window is already full.
//!
//! - **[`TokenBucket`](RateLimitStrategy::TokenBucket)** — refills tokens at a steady rate
//!   (`max_requests / window`); short bursts are absorbed by the bucket capacity before
//!   the rate is enforced.  Computes the exact wait time required to accumulate the next
//!   token instead of sleeping speculatively.
//!
//! A server-returned `Retry-After` value can be applied via
//! [`rate_limit_retry_after`], which imposes a hard block until the indicated
//! instant irrespective of the active algorithm.
//!
//! Operates in parallel with [`graphql_throttle`](crate::adapters::graphql_throttle)
//! (leaky-bucket cost throttle).  Both can be active simultaneously.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

/// Re-export — canonical definition lives in the ports layer.
pub use crate::ports::graphql_plugin::RateLimitConfig;
pub use crate::ports::graphql_plugin::RateLimitStrategy;

// ─────────────────────────────────────────────────────────────────────────────
// WindowState — per-strategy mutable inner state
// ─────────────────────────────────────────────────────────────────────────────

/// Per-strategy mutable state held inside [`RequestWindow`].
#[derive(Debug)]
enum WindowState {
    /// A rolling `VecDeque` of request timestamps for the sliding-window algorithm.
    Sliding { timestamps: VecDeque<Instant> },
    /// Token count and last-refill timestamp for the token-bucket algorithm.
    TokenBucket { tokens: f64, last_refill: Instant },
}

// ─────────────────────────────────────────────────────────────────────────────
// RequestWindow
// ─────────────────────────────────────────────────────────────────────────────

/// Mutable inner state — algorithm-specific window data plus an optional hard
/// block set by a server-returned `Retry-After` header.
#[derive(Debug)]
struct RequestWindow {
    state: WindowState,
    config: RateLimitConfig,
    /// Hard block until this instant, set by [`record_retry_after`].
    blocked_until: Option<Instant>,
}

impl RequestWindow {
    fn new(config: &RateLimitConfig) -> Self {
        let state = match config.strategy {
            RateLimitStrategy::SlidingWindow => WindowState::Sliding {
                timestamps: VecDeque::with_capacity(config.max_requests as usize),
            },
            RateLimitStrategy::TokenBucket => WindowState::TokenBucket {
                tokens: f64::from(config.max_requests),
                last_refill: Instant::now(),
            },
        };
        Self {
            state,
            config: config.clone(),
            blocked_until: None,
        }
    }

    /// Try to acquire a request slot.
    ///
    /// - Returns `None` if a slot is available (the slot is claimed immediately).
    /// - Returns `Some(wait)` with the duration the caller must sleep before
    ///   retrying (capped at `config.max_delay_ms`).
    fn acquire(&mut self) -> Option<Duration> {
        let now = Instant::now();
        let max_delay = Duration::from_millis(self.config.max_delay_ms);

        // Check hard block imposed by a previous Retry-After response.
        if let Some(until) = self.blocked_until {
            if until > now {
                let wait = until.duration_since(now);
                return Some(wait.min(max_delay));
            }
            self.blocked_until = None;
        }

        match &mut self.state {
            WindowState::Sliding { timestamps } => {
                let window = self.config.window;
                // Drop timestamps that have rolled out of the window.
                while timestamps
                    .front()
                    .is_some_and(|t| now.duration_since(*t) >= window)
                {
                    timestamps.pop_front();
                }

                if timestamps.len() < self.config.max_requests as usize {
                    // Slot available — record and permit.
                    timestamps.push_back(now);
                    None
                } else {
                    // Window is full; compute wait until the oldest entry rolls out.
                    let &oldest = timestamps.front()?;
                    let elapsed = now.duration_since(oldest);
                    let wait = window.saturating_sub(elapsed);
                    Some(wait.min(max_delay))
                }
            }

            WindowState::TokenBucket {
                tokens,
                last_refill,
            } => {
                // Refill tokens proportional to elapsed time.
                let elapsed = now.duration_since(*last_refill);
                let rate = f64::from(self.config.max_requests) / self.config.window.as_secs_f64();
                let refill = elapsed.as_secs_f64() * rate;
                *tokens = (*tokens + refill).min(f64::from(self.config.max_requests));
                *last_refill = now;

                if *tokens >= 1.0 {
                    *tokens -= 1.0;
                    None
                } else {
                    // Compute exact wait until 1 token has accumulated.
                    let wait_secs = (1.0 - *tokens) / rate;
                    let wait = Duration::from_secs_f64(wait_secs);
                    Some(wait.min(max_delay))
                }
            }
        }
    }

    /// Set a hard block for `secs` seconds to honour a server-returned
    /// `Retry-After` interval.
    ///
    /// A shorter `secs` value will never override a longer existing block.
    fn record_retry_after(&mut self, secs: u64) {
        let until = Instant::now() + Duration::from_secs(secs);
        match self.blocked_until {
            // Keep the later of the two blocks.
            Some(existing) if existing >= until => {}
            _ => self.blocked_until = Some(until),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RequestRateLimit
// ─────────────────────────────────────────────────────────────────────────────

/// Shareable, cheaply-cloneable handle to a per-plugin sliding-window limiter.
///
/// # Example
///
/// ```rust
/// use stygian_graph::adapters::graphql_rate_limit::{
///     RateLimitConfig, RequestRateLimit, rate_limit_acquire,
/// };
///
/// # async fn example() {
/// let rl = RequestRateLimit::new(RateLimitConfig::default());
/// rate_limit_acquire(&rl).await;
/// // … send the request …
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct RequestRateLimit {
    inner: Arc<Mutex<RequestWindow>>,
    config: RateLimitConfig,
}

impl RequestRateLimit {
    /// Create a new `RequestRateLimit` from a [`RateLimitConfig`].
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::adapters::graphql_rate_limit::{RateLimitConfig, RequestRateLimit};
    ///
    /// let rl = RequestRateLimit::new(RateLimitConfig::default());
    /// ```
    #[must_use]
    pub fn new(config: RateLimitConfig) -> Self {
        let window = RequestWindow::new(&config);
        Self {
            inner: Arc::new(Mutex::new(window)),
            config,
        }
    }

    /// Return the [`RateLimitConfig`] this limiter was initialised from.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::time::Duration;
    /// use stygian_graph::adapters::graphql_rate_limit::{RateLimitConfig, RequestRateLimit};
    ///
    /// let cfg = RateLimitConfig { max_requests: 50, ..Default::default() };
    /// let rl = RequestRateLimit::new(cfg.clone());
    /// assert_eq!(rl.config().max_requests, 50);
    /// ```
    #[must_use]
    pub const fn config(&self) -> &RateLimitConfig {
        &self.config
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Sleep until a request slot is available within the rolling window, then
/// record the slot.
///
/// If the window is full the function sleeps until the oldest in-window entry
/// expires (capped at `config.max_delay_ms`).  The `Mutex` guard is dropped
/// before every `.await` call to preserve `Send` bounds.
///
/// # Example
///
/// ```rust
/// use stygian_graph::adapters::graphql_rate_limit::{
///     RateLimitConfig, RequestRateLimit, rate_limit_acquire,
/// };
/// use std::time::Duration;
///
/// # async fn example() {
/// let rl = RequestRateLimit::new(RateLimitConfig {
///     max_requests: 2,
///     ..Default::default()
/// });
/// rate_limit_acquire(&rl).await;
/// rate_limit_acquire(&rl).await;
/// // Third call blocks until the window rolls forward.
/// # }
/// ```
pub async fn rate_limit_acquire(rl: &RequestRateLimit) {
    loop {
        let delay = {
            let mut guard = rl.inner.lock().await;
            guard.acquire()
        };
        match delay {
            None => return,
            Some(d) => {
                tracing::debug!(
                    delay_ms = d.as_millis(),
                    "rate limiter: window full, sleeping"
                );
                tokio::time::sleep(d).await;
            }
        }
    }
}

/// Record a server-returned `Retry-After` delay.
///
/// Call this when the upstream API responds with HTTP 429 and a `Retry-After`
/// header.  Subsequent calls to [`rate_limit_acquire`] will block for at least
/// `secs` seconds.  A shorter `secs` value will never shorten an existing block.
///
/// # Example
///
/// ```rust
/// use stygian_graph::adapters::graphql_rate_limit::{
///     RateLimitConfig, RequestRateLimit, rate_limit_retry_after,
/// };
///
/// # async fn example() {
/// let rl = RequestRateLimit::new(RateLimitConfig::default());
/// rate_limit_retry_after(&rl, 30).await;
/// # }
/// ```
pub async fn rate_limit_retry_after(rl: &RequestRateLimit, retry_after_secs: u64) {
    let mut guard = rl.inner.lock().await;
    guard.record_retry_after(retry_after_secs);
}

/// Parse an integer `Retry-After` value from a header string.
///
/// Returns `None` if the value cannot be parsed as a non-negative integer.
/// HTTP-date format is intentionally not supported.
///
/// # Example
///
/// ```rust
/// use stygian_graph::adapters::graphql_rate_limit::parse_retry_after;
///
/// assert_eq!(parse_retry_after("30"), Some(30));
/// assert_eq!(parse_retry_after("not-a-number"), None);
/// ```
#[must_use]
pub fn parse_retry_after(value: &str) -> Option<u64> {
    value.trim().parse::<u64>().ok()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn cfg(max_requests: u32, window_secs: u64) -> RateLimitConfig {
        RateLimitConfig {
            max_requests,
            window: Duration::from_secs(window_secs),
            max_delay_ms: 60_000,
            strategy: RateLimitStrategy::SlidingWindow,
        }
    }

    fn cfg_bucket(max_requests: u32, window_secs: u64) -> RateLimitConfig {
        RateLimitConfig {
            max_requests,
            window: Duration::from_secs(window_secs),
            max_delay_ms: 60_000,
            strategy: RateLimitStrategy::TokenBucket,
        }
    }

    // 1. Window allows exactly max_requests without blocking.
    #[test]
    fn window_allows_up_to_max() {
        let mut w = RequestWindow::new(&cfg(3, 60));
        assert!(w.acquire().is_none(), "slot 1");
        assert!(w.acquire().is_none(), "slot 2");
        assert!(w.acquire().is_none(), "slot 3");
        assert!(w.acquire().is_some(), "4th request must be blocked");
    }

    // 2. After the window expires the slot becomes available again.
    #[test]
    fn window_resets_after_expiry() {
        let mut w = RequestWindow::new(&RateLimitConfig {
            max_requests: 1,
            window: Duration::from_millis(10),
            max_delay_ms: 60_000,
            strategy: RateLimitStrategy::SlidingWindow,
        });
        assert!(w.acquire().is_none(), "first request");
        std::thread::sleep(Duration::from_millis(25));
        assert!(w.acquire().is_none(), "window should have expired");
    }

    // 3. Timestamps are recorded immediately so concurrent callers see a
    //    reduced slot count without waiting for the request to complete.
    #[test]
    fn timestamps_recorded_immediately() {
        let mut w = RequestWindow::new(&cfg(2, 60));
        w.acquire();
        w.acquire();
        // After two acquisitions the window is full.
        assert!(w.acquire().is_some(), "third request must be blocked");
    }

    // 4. record_retry_after blocks subsequent acquire calls.
    #[test]
    fn retry_after_blocks_further_requests() {
        let mut w = RequestWindow::new(&cfg(100, 60));
        w.record_retry_after(30);
        assert!(
            w.acquire().is_some(),
            "Retry-After must block the next request"
        );
    }

    // 5. A shorter Retry-After must not reduce an existing longer block.
    #[test]
    fn retry_after_does_not_shorten_existing_block() {
        let mut w = RequestWindow::new(&cfg(100, 60));
        w.record_retry_after(60);
        let until_before = w.blocked_until.unwrap();
        w.record_retry_after(1);
        let until_after = w.blocked_until.unwrap();
        assert!(
            until_after >= until_before,
            "shorter retry-after must not override the longer block"
        );
    }

    // 6. parse_retry_after handles valid integers and rejects garbage.
    #[test]
    fn parse_retry_after_parses_integers() {
        assert_eq!(parse_retry_after("42"), Some(42));
        assert_eq!(parse_retry_after("0"), Some(0));
        assert_eq!(parse_retry_after("not-a-number"), None);
        assert_eq!(parse_retry_after(""), None);
        assert_eq!(parse_retry_after("  30  "), Some(30));
    }

    // ── Token-bucket strategy tests ──────────────────────────────────────────

    // 7. Token bucket allows up to max_requests immediately (full bucket).
    #[test]
    fn token_bucket_allows_up_to_max() {
        let mut w = RequestWindow::new(&cfg_bucket(3, 60));
        assert!(w.acquire().is_none(), "token 1");
        assert!(w.acquire().is_none(), "token 2");
        assert!(w.acquire().is_none(), "token 3");
        assert!(
            w.acquire().is_some(),
            "4th request must be blocked — bucket empty"
        );
    }

    // 8. Token bucket refills over time.
    #[test]
    fn token_bucket_refills_after_delay() {
        let mut w = RequestWindow::new(&RateLimitConfig {
            // 1 token per 10 ms
            max_requests: 1,
            window: Duration::from_millis(10),
            max_delay_ms: 60_000,
            strategy: RateLimitStrategy::TokenBucket,
        });
        assert!(w.acquire().is_none(), "first request consumes the token");
        assert!(w.acquire().is_some(), "bucket empty — must block");
        std::thread::sleep(Duration::from_millis(20));
        assert!(w.acquire().is_none(), "bucket should have refilled");
    }

    // 9. Token bucket also respects Retry-After hard blocks.
    #[test]
    fn token_bucket_respects_retry_after() {
        let mut w = RequestWindow::new(&cfg_bucket(100, 60));
        w.record_retry_after(30);
        assert!(
            w.acquire().is_some(),
            "Retry-After must block even with tokens available"
        );
    }

    // 10. Token bucket wait duration is proportional to the deficit.
    #[test]
    fn token_bucket_wait_is_proportional() {
        // 60 requests / 60 s = 1 token/s.  After draining the bucket the wait
        // for a single token should be ≈ 1 s.
        let mut w = RequestWindow::new(&RateLimitConfig {
            max_requests: 1,
            window: Duration::from_secs(1),
            max_delay_ms: 60_000,
            strategy: RateLimitStrategy::TokenBucket,
        });
        w.acquire(); // consume the only token
        let wait = w.acquire().unwrap();
        // Wait should be close to 1 s; allow generous tolerance for slow CI.
        assert!(
            wait <= Duration::from_secs(1),
            "wait {wait:?} should not exceed 1 s"
        );
        assert!(
            wait >= Duration::from_millis(800),
            "wait {wait:?} should be close to 1 s"
        );
    }
}
