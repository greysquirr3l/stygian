//! Resilience adapters

use crate::domain::error::Result;
use crate::ports::{CircuitBreaker, CircuitState, RateLimitConfig, RateLimiter};
use async_trait::async_trait;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Circuit breaker implementation with configurable thresholds
///
/// Implements the circuit breaker pattern to prevent cascading failures.
/// Tracks failure rate and automatically opens the circuit when threshold is exceeded.
///
/// # State Machine
///
/// - **Closed**: Normal operation, all requests pass through
/// - **Open**: Too many failures, all requests fail fast
/// - **`HalfOpen`**: Testing recovery, limited requests allowed
///
/// # Example
///
/// ```
/// use mycelium_graph::adapters::resilience::CircuitBreakerImpl;
/// use mycelium_graph::ports::{CircuitBreaker, CircuitState};
///
/// let cb = CircuitBreakerImpl::new(5, std::time::Duration::from_secs(30));
/// // Record some failures
/// cb.record_failure();
/// cb.record_failure();
/// // Check state
/// assert!(matches!(cb.state(), CircuitState::Closed | CircuitState::Open));
/// ```
pub struct CircuitBreakerImpl {
    state: Arc<RwLock<CircuitBreakerState>>,
    failure_threshold: u32,
    timeout: Duration,
}

#[derive(Debug)]
struct CircuitBreakerState {
    current: CircuitState,
    failure_count: u32,
    last_failure_time: Option<Instant>,
}

impl CircuitBreakerImpl {
    /// Create a new circuit breaker
    ///
    /// # Arguments
    ///
    /// * `failure_threshold` - Number of failures before opening circuit
    /// * `timeout` - Duration to wait before attempting reset
    pub fn new(failure_threshold: u32, timeout: Duration) -> Self {
        Self {
            state: Arc::new(RwLock::new(CircuitBreakerState {
                current: CircuitState::Closed,
                failure_count: 0,
                last_failure_time: None,
            })),
            failure_threshold,
            timeout,
        }
    }

    /// Check if timeout has elapsed and circuit can transition to `HalfOpen`
    fn should_attempt_reset(&self, state: &CircuitBreakerState) -> bool {
        if state.current != CircuitState::Open {
            return false;
        }

        state
            .last_failure_time
            .is_some_and(|last_failure| last_failure.elapsed() >= self.timeout)
    }
}

impl CircuitBreaker for CircuitBreakerImpl {
    fn state(&self) -> CircuitState {
        let state = self.state.read();
        state.current
    }

    fn record_success(&self) {
        let mut state = self.state.write();
        // Success resets failures and closes circuit
        state.failure_count = 0;
        state.current = CircuitState::Closed;
        state.last_failure_time = None;
    }

    fn record_failure(&self) {
        let mut state = self.state.write();
        state.failure_count += 1;
        state.last_failure_time = Some(Instant::now());

        // Open circuit if threshold exceeded
        if state.failure_count >= self.failure_threshold {
            state.current = CircuitState::Open;
        }
    }

    fn attempt_reset(&self) -> bool {
        let mut state = self.state.write();

        if self.should_attempt_reset(&state) {
            state.current = CircuitState::HalfOpen;
            state.failure_count = 0;
            true
        } else {
            false
        }
    }
}

/// No-op circuit breaker for testing
///
/// Always reports Closed state and ignores all state transitions.
/// Useful for testing scenarios where circuit breaker behavior should be disabled.
///
/// # Example
///
/// ```
/// use mycelium_graph::adapters::resilience::NoopCircuitBreaker;
/// use mycelium_graph::ports::{CircuitBreaker, CircuitState};
///
/// let cb = NoopCircuitBreaker;
/// cb.record_failure();
/// assert_eq!(cb.state(), CircuitState::Closed);
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopCircuitBreaker;

impl CircuitBreaker for NoopCircuitBreaker {
    fn state(&self) -> CircuitState {
        CircuitState::Closed
    }

    fn record_success(&self) {
        // No-op
    }

    fn record_failure(&self) {
        // No-op
    }

    fn attempt_reset(&self) -> bool {
        false
    }
}

/// Token bucket rate limiter implementation
///
/// Implements rate limiting using the token bucket algorithm.
/// Supports per-key rate limiting for multi-tenant scenarios.
///
/// # Algorithm
///
/// - Each key has a bucket with a maximum number of tokens
/// - Tokens are consumed on each request
/// - Tokens regenerate over time based on the configured window
/// - Requests are rejected when bucket is empty
///
/// # Example
///
/// ```
/// use mycelium_graph::adapters::resilience::TokenBucketRateLimiter;
/// use mycelium_graph::ports::{RateLimiter, RateLimitConfig};
/// use std::time::Duration;
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let config = RateLimitConfig {
///     max_requests: 10,
///     window: Duration::from_secs(60),
/// };
/// let limiter = TokenBucketRateLimiter::new(config);
/// assert!(limiter.check_rate_limit("api:test").await.unwrap());
/// # });
/// ```
pub struct TokenBucketRateLimiter {
    config: RateLimitConfig,
    buckets: Arc<RwLock<HashMap<String, TokenBucket>>>,
}

#[derive(Debug)]
struct TokenBucket {
    tokens: u32,
    last_refill: Instant,
}

impl TokenBucketRateLimiter {
    /// Create a new token bucket rate limiter
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            buckets: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Refill tokens based on elapsed time
    fn refill_tokens(&self, bucket: &mut TokenBucket) {
        let elapsed = bucket.last_refill.elapsed();
        let refill_rate = f64::from(self.config.max_requests) / self.config.window.as_secs_f64();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let tokens_to_add = (elapsed.as_secs_f64() * refill_rate) as u32;

        if tokens_to_add > 0 {
            bucket.tokens = (bucket.tokens + tokens_to_add).min(self.config.max_requests);
            bucket.last_refill = Instant::now();
        }
    }
}

#[async_trait]
impl RateLimiter for TokenBucketRateLimiter {
    #[allow(clippy::significant_drop_tightening)]
    async fn check_rate_limit(&self, key: &str) -> Result<bool> {
        let has_tokens = {
            let mut buckets = self.buckets.write();
            let bucket = buckets
                .entry(key.to_string())
                .or_insert_with(|| TokenBucket {
                    tokens: self.config.max_requests,
                    last_refill: Instant::now(),
                });
            self.refill_tokens(bucket);
            bucket.tokens > 0
        };
        Ok(has_tokens)
    }

    async fn record_request(&self, key: &str) -> Result<()> {
        {
            let mut buckets = self.buckets.write();
            if let Some(bucket) = buckets.get_mut(key)
                && bucket.tokens > 0
            {
                bucket.tokens -= 1;
            }
        }
        Ok(())
    }
}

/// No-op rate limiter for testing
///
/// Always allows requests and ignores all rate limit tracking.
/// Useful for testing scenarios where rate limiting should be disabled.
///
/// # Example
///
/// ```
/// use mycelium_graph::adapters::resilience::NoopRateLimiter;
/// use mycelium_graph::ports::RateLimiter;
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let limiter = NoopRateLimiter;
/// assert!(limiter.check_rate_limit("any_key").await.unwrap());
/// # });
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopRateLimiter;

#[async_trait]
impl RateLimiter for NoopRateLimiter {
    async fn check_rate_limit(&self, _key: &str) -> Result<bool> {
        Ok(true)
    }

    async fn record_request(&self, _key: &str) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_closes_on_success() {
        let cb = CircuitBreakerImpl::new(3, Duration::from_secs(5));
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_opens_on_threshold() {
        let cb = CircuitBreakerImpl::new(3, Duration::from_secs(5));
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_noop_circuit_breaker_always_closed() {
        let cb = NoopCircuitBreaker;
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[tokio::test]
    async fn test_rate_limiter_allows_within_limit() -> Result<()> {
        let config = RateLimitConfig {
            max_requests: 10,
            window: Duration::from_secs(60),
        };
        let limiter = TokenBucketRateLimiter::new(config);

        assert!(limiter.check_rate_limit("test").await?);
        limiter.record_request("test").await?;
        assert!(limiter.check_rate_limit("test").await?);
        Ok(())
    }

    #[tokio::test]
    async fn test_noop_rate_limiter_always_allows() -> Result<()> {
        let limiter = NoopRateLimiter;
        assert!(limiter.check_rate_limit("any").await?);
        limiter.record_request("any").await?;
        assert!(limiter.check_rate_limit("any").await?);
        Ok(())
    }
}

// ─── Exponential Backoff Retry ────────────────────────────────────────────────

/// Policy controlling exponential backoff retry behaviour.
///
/// Delays follow the formula: `base_delay * 2^attempt + rand(0..jitter_ms)`.
/// The computed delay is capped at `max_delay`.
///
/// # Example
///
/// ```
/// use mycelium_graph::adapters::resilience::RetryPolicy;
/// use std::time::Duration;
///
/// let policy = RetryPolicy::new(3, Duration::from_millis(100), Duration::from_secs(10));
/// ```
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts (not counting the initial call)
    pub max_attempts: u32,
    /// Base delay for the first retry
    pub base_delay: Duration,
    /// Maximum delay cap
    pub max_delay: Duration,
    /// Additional random jitter ceiling (milliseconds)
    pub jitter_ms: u64,
}

impl RetryPolicy {
    /// Create a new retry policy.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::adapters::resilience::RetryPolicy;
    /// use std::time::Duration;
    ///
    /// let p = RetryPolicy::new(5, Duration::from_millis(200), Duration::from_secs(30));
    /// assert_eq!(p.max_attempts, 5);
    /// ```
    pub const fn new(max_attempts: u32, base_delay: Duration, max_delay: Duration) -> Self {
        Self {
            max_attempts,
            base_delay,
            max_delay,
            jitter_ms: 50,
        }
    }

    /// Override the jitter ceiling in milliseconds.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::adapters::resilience::RetryPolicy;
    /// use std::time::Duration;
    ///
    /// let p = RetryPolicy::new(3, Duration::from_millis(100), Duration::from_secs(5))
    ///     .with_jitter_ms(100);
    /// assert_eq!(p.jitter_ms, 100);
    /// ```
    #[must_use]
    pub const fn with_jitter_ms(mut self, jitter_ms: u64) -> Self {
        self.jitter_ms = jitter_ms;
        self
    }

    /// Compute the sleep duration for a given attempt index (0-based).
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::adapters::resilience::RetryPolicy;
    /// use std::time::Duration;
    ///
    /// let p = RetryPolicy::new(3, Duration::from_millis(100), Duration::from_secs(10))
    ///     .with_jitter_ms(0);
    /// // attempt 0 → 100 ms, attempt 1 → 200 ms, attempt 2 → 400 ms
    /// assert_eq!(p.delay_for(0), Duration::from_millis(100));
    /// assert_eq!(p.delay_for(1), Duration::from_millis(200));
    /// ```
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let factor = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
        #[allow(clippy::cast_possible_truncation)]
        let base_ms = self.base_delay.as_millis() as u64;
        let jitter = if self.jitter_ms > 0 {
            // Deterministic-enough without pulling in `rand`: use mix of attempt
            // and current nanos as a low-cost entropy source.
            let seed = u64::from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .subsec_nanos(),
            );
            (seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407)
                >> 33)
                % self.jitter_ms
        } else {
            0
        };
        let ms = base_ms.saturating_mul(factor).saturating_add(jitter);
        let delay = Duration::from_millis(ms);
        delay.min(self.max_delay)
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new(3, Duration::from_millis(200), Duration::from_secs(30))
    }
}

/// Execute an async operation with automatic retry according to a [`RetryPolicy`].
///
/// Returns the first `Ok` value, or the last `Err` after all attempts are exhausted.
/// Each retry sleeps for an exponentially increasing delay with jitter.
///
/// # Example
///
/// ```
/// use mycelium_graph::adapters::resilience::{RetryPolicy, retry};
/// use std::sync::atomic::{AtomicU32, Ordering};
/// use std::sync::Arc;
/// use std::time::Duration;
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let attempts = Arc::new(AtomicU32::new(0));
/// let policy = RetryPolicy::new(3, Duration::from_millis(1), Duration::from_millis(10))
///     .with_jitter_ms(0);
///
/// let result = retry(&policy, || {
///     let counter = Arc::clone(&attempts);
///     async move {
///         let n = counter.fetch_add(1, Ordering::SeqCst);
///         if n < 2 { Err("not yet".to_string()) } else { Ok(n) }
///     }
/// }).await;
///
/// assert!(result.is_ok());
/// assert_eq!(attempts.load(Ordering::SeqCst), 3);
/// # });
/// ```
pub async fn retry<F, Fut, T, E>(policy: &RetryPolicy, mut f: F) -> std::result::Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, E>>,
{
    let mut result = f().await;
    for attempt in 1..=policy.max_attempts {
        if result.is_ok() {
            return result;
        }
        tokio::time::sleep(policy.delay_for(attempt - 1)).await;
        result = f().await;
    }
    result
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn delay_for_doubles() {
        let p = RetryPolicy::new(4, Duration::from_millis(100), Duration::from_secs(60))
            .with_jitter_ms(0);
        assert_eq!(p.delay_for(0), Duration::from_millis(100));
        assert_eq!(p.delay_for(1), Duration::from_millis(200));
        assert_eq!(p.delay_for(2), Duration::from_millis(400));
        assert_eq!(p.delay_for(3), Duration::from_millis(800));
    }

    #[test]
    fn delay_capped_at_max() {
        let p = RetryPolicy::new(10, Duration::from_millis(1000), Duration::from_secs(3))
            .with_jitter_ms(0);
        // 1000 * 2^4 = 16_000 ms, capped at 3_000 ms
        assert_eq!(p.delay_for(4), Duration::from_secs(3));
    }

    #[tokio::test]
    async fn retry_succeeds_on_first_try() {
        let policy = RetryPolicy::new(3, Duration::from_millis(1), Duration::from_millis(10))
            .with_jitter_ms(0);
        let result: std::result::Result<i32, &str> = retry(&policy, || async { Ok(42) }).await;
        assert_eq!(result.ok(), Some(42));
    }

    #[tokio::test]
    async fn retry_retries_until_success() {
        let counter = Arc::new(AtomicU32::new(0));
        let policy = RetryPolicy::new(4, Duration::from_millis(1), Duration::from_millis(50))
            .with_jitter_ms(0);

        let result: std::result::Result<u32, String> = retry(&policy, || {
            let c = Arc::clone(&counter);
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n < 3 {
                    Err(format!("fail {n}"))
                } else {
                    Ok(n)
                }
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 4); // 3 failures + 1 success
    }

    #[tokio::test]
    async fn retry_exhausts_and_returns_last_error() {
        let policy = RetryPolicy::new(2, Duration::from_millis(1), Duration::from_millis(10))
            .with_jitter_ms(0);
        let counter = Arc::new(AtomicU32::new(0));

        let result: std::result::Result<(), String> = retry(&policy, || {
            let c = Arc::clone(&counter);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err("always fails".to_string())
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 3); // initial + 2 retries
    }
}
