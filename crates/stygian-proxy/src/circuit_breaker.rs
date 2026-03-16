//! Per-proxy circuit breaker.
//!
//! State machine:
//!
//! ```text
//! CLOSED ──(failures ≥ threshold)──► OPEN
//!   ▲                                  │
//!   │                     (elapsed > half_open_after)
//!   │                                  ▼
//! (success)                        HALF_OPEN
//!   └──────────────────────────────────┘
//!                    or
//! HALF_OPEN ──(failure)──► OPEN  (timer reset)
//! ```

use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const STATE_CLOSED: u8 = 0;
pub const STATE_OPEN: u8 = 1;
pub const STATE_HALF_OPEN: u8 = 2;

/// Lightweight, lock-free per-proxy circuit breaker.
///
/// All fields are atomics so many tasks can call `record_failure` /
/// `record_success` / `is_available` concurrently without a mutex.
pub struct CircuitBreaker {
    state: AtomicU8,
    failure_count: AtomicU32,
    /// Milliseconds since UNIX_EPOCH of the last recorded failure.
    last_failure: AtomicU64,
    threshold: u32,
    half_open_after_ms: u64,
}

impl CircuitBreaker {
    /// Create a new breaker from config parameters.
    pub fn new(threshold: u32, half_open_after_ms: u64) -> Self {
        Self {
            state: AtomicU8::new(STATE_CLOSED),
            failure_count: AtomicU32::new(0),
            last_failure: AtomicU64::new(0),
            threshold,
            half_open_after_ms,
        }
    }

    /// Current state as a u8 constant.
    #[inline]
    pub fn state(&self) -> u8 {
        self.state.load(Ordering::Acquire)
    }

    /// Returns `true` when the proxy may be used (Closed or HalfOpen).
    ///
    /// When the circuit is Open and enough time has elapsed since the last
    /// failure the breaker transitions to HalfOpen and returns `true`.
    pub fn is_available(&self) -> bool {
        match self.state.load(Ordering::Acquire) {
            STATE_CLOSED => true,
            STATE_HALF_OPEN => true,
            STATE_OPEN => {
                let elapsed_ms = now_ms().saturating_sub(self.last_failure.load(Ordering::Acquire));
                if elapsed_ms >= self.half_open_after_ms {
                    // Try to transition Open → HalfOpen.  Another thread may
                    // get there first — both outcomes are fine: the proxy is
                    // available in HalfOpen regardless of which thread won.
                    let _ = self.state.compare_exchange(
                        STATE_OPEN,
                        STATE_HALF_OPEN,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Record a successful request.
    ///
    /// When the circuit is in HalfOpen this resets the failure count and
    /// transitions back to Closed.
    pub fn record_success(&self) {
        if self
            .state
            .compare_exchange(
                STATE_HALF_OPEN,
                STATE_CLOSED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.failure_count.store(0, Ordering::Release);
        }
    }

    /// Record a failed request.
    ///
    /// In Closed: if the incremented count reaches the threshold the circuit
    /// trips to Open.  In HalfOpen: immediately trips back to Open and resets
    /// the timer.
    pub fn record_failure(&self) {
        let count = self.failure_count.fetch_add(1, Ordering::AcqRel) + 1;
        self.last_failure.store(now_ms(), Ordering::Release);

        let current_state = self.state.load(Ordering::Acquire);
        if current_state == STATE_CLOSED && count >= self.threshold {
            // Closed → Open
            let _ = self.state.compare_exchange(
                STATE_CLOSED,
                STATE_OPEN,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        } else if current_state == STATE_HALF_OPEN {
            // HalfOpen → Open (probe failed)
            let _ = self.state.compare_exchange(
                STATE_HALF_OPEN,
                STATE_OPEN,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }
}

#[inline]
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn breaker(threshold: u32, half_open_after_ms: u64) -> CircuitBreaker {
        CircuitBreaker::new(threshold, half_open_after_ms)
    }

    #[test]
    fn failures_open_circuit() {
        let cb = breaker(3, 30_000);
        assert_eq!(cb.state(), STATE_CLOSED);
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), STATE_CLOSED, "not tripped yet");
        cb.record_failure();
        assert_eq!(cb.state(), STATE_OPEN, "should be open after threshold");
        assert!(!cb.is_available());
    }

    #[test]
    fn half_open_after_elapsed() {
        let cb = breaker(1, 0); // half_open_after = 0 ms → immediate
        cb.record_failure();
        assert_eq!(cb.state(), STATE_OPEN);
        // With half_open_after_ms = 0, any call to is_available should
        // transition to HalfOpen because elapsed ≥ 0 is always true.
        assert!(cb.is_available(), "should transition to half-open");
        assert_eq!(cb.state(), STATE_HALF_OPEN);
    }

    #[test]
    fn success_in_half_open_closes_circuit() {
        let cb = breaker(1, 0);
        cb.record_failure();
        assert!(cb.is_available()); // → HalfOpen
        cb.record_success();
        assert_eq!(cb.state(), STATE_CLOSED);
        assert!(cb.is_available());
    }

    #[test]
    fn failure_in_half_open_reopens() {
        let cb = breaker(1, 0);
        cb.record_failure();
        assert!(cb.is_available()); // → HalfOpen
        cb.record_failure(); // probe failed → back to Open
        assert_eq!(cb.state(), STATE_OPEN);
    }

    #[test]
    fn concurrent_failures_open_circuit() {
        use std::thread;
        let cb = Arc::new(breaker(5, 30_000));
        let handles: Vec<_> = (0..100)
            .map(|_| {
                let cb = Arc::clone(&cb);
                thread::spawn(move || cb.record_failure())
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(cb.state(), STATE_OPEN);
        assert!(cb.failure_count.load(Ordering::Relaxed) >= 5);
    }
}
