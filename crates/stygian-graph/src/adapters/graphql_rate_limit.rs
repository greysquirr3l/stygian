//! Sliding-window request-count rate limiter for GraphQL API targets.
//!
//! Limits outgoing requests to `max_requests` in any rolling `window` duration.
//! Before each request [`rate_limit_acquire`] is called; it records the current
//! timestamp and sleeps until the oldest in-window request expires if the
//! window is already full.
//!
//! A server-returned `Retry-After` value can be applied via
//! [`rate_limit_retry_after`], which imposes a hard block until the indicated
//! instant irrespective of the sliding window state.
//!
//! Operates in parallel with [`graphql_throttle`](crate::adapters::graphql_throttle)
//! (leaky-bucket cost throttle).  Both can be active simultaneously.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

/// Re-export — canonical definition lives in the ports layer.
pub use crate::ports::graphql_plugin::RateLimitConfig;

// ─────────────────────────────────────────────────────────────────────────────
// RequestWindow
// ─────────────────────────────────────────────────────────────────────────────

/// Mutable inner state — a sliding window of request timestamps plus an
/// optional hard block set by a server-returned `Retry-After` header.
#[derive(Debug)]
struct RequestWindow {
    timestamps: VecDeque<Instant>,
    config: RateLimitConfig,
    /// Hard block until this instant, set by [`record_retry_after`].
    blocked_until: Option<Instant>,
}

impl RequestWindow {
    fn new(config: &RateLimitConfig) -> Self {
        Self {
            timestamps: VecDeque::with_capacity(config.max_requests as usize),
            config: config.clone(),
            blocked_until: None,
        }
    }

    /// Try to acquire a request slot.
    ///
    /// Prunes expired entries, then:
    /// - Returns `None` if a slot is available (timestamp is recorded immediately).
    /// - Returns `Some(wait)` with the duration the caller must sleep before
    ///   retrying (capped at `config.max_delay_ms`).
    fn acquire(&mut self) -> Option<Duration> {
        let now = Instant::now();
        let window = self.config.window;
        let max_delay = Duration::from_millis(self.config.max_delay_ms);

        // Check hard block imposed by a previous Retry-After response.
        if let Some(until) = self.blocked_until {
            if until > now {
                let wait = until.duration_since(now);
                return Some(wait.min(max_delay));
            }
            self.blocked_until = None;
        }

        // Drop timestamps that have rolled out of the window.
        while self
            .timestamps
            .front()
            .is_some_and(|t| now.duration_since(*t) >= window)
        {
            self.timestamps.pop_front();
        }

        if (self.timestamps.len() as u32) < self.config.max_requests {
            // Slot available — record and permit.
            self.timestamps.push_back(now);
            None
        } else {
            // Window is full; compute wait until the oldest entry rolls out.
            let oldest = *self.timestamps.front().expect("len >= 1");
            let elapsed = now.duration_since(oldest);
            let wait = window.saturating_sub(elapsed);
            Some(wait.min(max_delay))
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
        assert_eq!(w.timestamps.len(), 1);
        w.acquire();
        assert_eq!(w.timestamps.len(), 2);
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
}
