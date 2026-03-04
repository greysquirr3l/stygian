//! Proactive GraphQL cost-throttle management.
//!
//! `LiveBudget` tracks the rolling point budget advertised by APIs that
//! implement the Shopify / Jobber-style cost-throttle extension envelope:
//!
//! ```json
//! { "extensions": { "cost": {
//!     "requestedQueryCost": 12,
//!     "actualQueryCost": 12,
//!     "throttleStatus": {
//!         "maximumAvailable": 10000.0,
//!         "currentlyAvailable": 9988.0,
//!         "restoreRate": 500.0
//!     }
//! }}}
//! ```
//!
//! Before each request a *proactive* pre-flight delay is computed: if the
//! projected available budget (accounting for elapsed restore time) will be
//! too low, the caller sleeps until it recovers.  This eliminates wasted
//! requests that would otherwise return `THROTTLED`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::Mutex;

/// Re-export from the ports layer — the canonical definition lives there.
pub use crate::ports::graphql_plugin::CostThrottleConfig;

// ─────────────────────────────────────────────────────────────────────────────
// LiveBudget
// ─────────────────────────────────────────────────────────────────────────────

/// Mutable runtime state tracking the current point budget.
///
/// One `LiveBudget` should be shared across all requests to the same plugin
/// endpoint, wrapped in `Arc<Mutex<LiveBudget>>` to serialise updates.
#[derive(Debug)]
pub struct LiveBudget {
    currently_available: f64,
    maximum_available: f64,
    restore_rate: f64, // points/second
    last_updated: Instant,
}

impl LiveBudget {
    /// Create a new budget initialised from `config` defaults.
    #[must_use]
    pub fn new(config: &CostThrottleConfig) -> Self {
        Self {
            currently_available: config.max_points,
            maximum_available: config.max_points,
            restore_rate: config.restore_per_sec,
            last_updated: Instant::now(),
        }
    }

    /// Update the budget from a throttle-status object.
    ///
    /// The JSON path is `extensions.cost.throttleStatus` in the GraphQL response body.
    ///
    /// # Example
    ///
    /// ```rust
    /// use serde_json::json;
    /// use stygian_graph::adapters::graphql_throttle::{CostThrottleConfig, LiveBudget};
    ///
    /// let config = CostThrottleConfig::default();
    /// let mut budget = LiveBudget::new(&config);
    ///
    /// let status = json!({
    ///     "maximumAvailable": 10000.0,
    ///     "currentlyAvailable": 4200.0,
    ///     "restoreRate": 500.0,
    /// });
    /// budget.update_from_response(&status);
    /// ```
    pub fn update_from_response(&mut self, throttle_status: &Value) {
        if let Some(max) = throttle_status["maximumAvailable"].as_f64() {
            self.maximum_available = max;
        }
        if let Some(cur) = throttle_status["currentlyAvailable"].as_f64() {
            self.currently_available = cur;
        }
        if let Some(rate) = throttle_status["restoreRate"].as_f64() {
            self.restore_rate = rate;
        }
        self.last_updated = Instant::now();
    }

    /// Compute the projected available budget accounting for elapsed restore time.
    fn projected_available(&self) -> f64 {
        let elapsed = self.last_updated.elapsed().as_secs_f64();
        let restored = elapsed * self.restore_rate;
        (self.currently_available + restored).min(self.maximum_available)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-plugin budget store
// ─────────────────────────────────────────────────────────────────────────────

/// A shareable, cheaply-cloneable handle to a per-plugin `LiveBudget`.
///
/// Create one per registered plugin and pass it to [`pre_flight_delay`] before
/// each request.
///
/// # Example
///
/// ```rust
/// use stygian_graph::adapters::graphql_throttle::{CostThrottleConfig, PluginBudget};
///
/// let budget = PluginBudget::new(CostThrottleConfig::default());
/// let budget2 = budget.clone(); // cheap Arc clone
/// ```
#[derive(Clone, Debug)]
pub struct PluginBudget {
    inner: Arc<Mutex<LiveBudget>>,
    config: CostThrottleConfig,
}

impl PluginBudget {
    /// Create a new `PluginBudget` initialised from `config`.
    #[must_use]
    pub fn new(config: CostThrottleConfig) -> Self {
        let budget = LiveBudget::new(&config);
        Self {
            inner: Arc::new(Mutex::new(budget)),
            config,
        }
    }

    /// Return the `CostThrottleConfig` this budget was initialised from.
    #[must_use]
    pub fn config(&self) -> &CostThrottleConfig {
        &self.config
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Compute a pre-flight delay and sleep if the budget is projected to be too low.
///
/// Must be called **before** sending a request.  The `Mutex` guard is released
/// before the `.await` to satisfy `Send` bounds.
///
/// # Example
///
/// ```rust
/// use stygian_graph::adapters::graphql_throttle::{CostThrottleConfig, PluginBudget, pre_flight_delay};
///
/// # async fn example() {
/// let budget = PluginBudget::new(CostThrottleConfig::default());
/// pre_flight_delay(&budget).await;
/// // safe to send the request now
/// # }
/// ```
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub async fn pre_flight_delay(budget: &PluginBudget) {
    let delay = {
        let guard = budget.inner.lock().await;
        let projected = guard.projected_available();
        let rate = guard.restore_rate.max(1.0);
        drop(guard);
        let min = budget.config.min_available;
        if projected < min {
            let deficit = min - projected;
            let secs = (deficit / rate) * 1.1;
            let ms = (secs * 1_000.0) as u64;
            Some(Duration::from_millis(ms.min(budget.config.max_delay_ms)))
        } else {
            None
        }
    };

    if let Some(d) = delay {
        tracing::debug!(
            delay_ms = d.as_millis(),
            "graphql throttle: pre-flight delay"
        );
        tokio::time::sleep(d).await;
    }
}

/// Update the `PluginBudget` from a completed response body.
///
/// Extracts `extensions.cost.throttleStatus` if present and forwards to
/// [`LiveBudget::update_from_response`].
///
/// # Example
///
/// ```rust
/// use serde_json::json;
/// use stygian_graph::adapters::graphql_throttle::{CostThrottleConfig, PluginBudget, update_budget};
///
/// # async fn example() {
/// let budget = PluginBudget::new(CostThrottleConfig::default());
/// let response = json!({
///     "data": {},
///     "extensions": { "cost": { "throttleStatus": {
///         "maximumAvailable": 10000.0,
///         "currentlyAvailable": 8000.0,
///         "restoreRate": 500.0,
///     }}}
/// });
/// update_budget(&budget, &response).await;
/// # }
/// ```
pub async fn update_budget(budget: &PluginBudget, response_body: &Value) {
    let Some(status) = response_body.pointer("/extensions/cost/throttleStatus") else {
        return;
    };
    if status.is_object() {
        let mut guard = budget.inner.lock().await;
        guard.update_from_response(status);
    }
}

/// Compute the reactive back-off delay from a throttle response body.
///
/// Use this when `extensions.cost.throttleStatus` signals `THROTTLED` rather
/// than projecting from the `LiveBudget`.
///
/// ```text
/// deficit = max_available − currently_available
/// base_ms = deficit / restore_rate * 1100
/// ms      = (base_ms * 1.5^attempt).clamp(500, max_delay_ms)
/// ```
///
/// # Example
///
/// ```rust
/// use serde_json::json;
/// use stygian_graph::adapters::graphql_throttle::{CostThrottleConfig, reactive_backoff_ms};
///
/// let config = CostThrottleConfig::default();
/// let body = json!({ "extensions": { "cost": { "throttleStatus": {
///     "maximumAvailable": 10000.0,
///     "currentlyAvailable": 0.0,
///     "restoreRate": 500.0,
/// }}}});
/// let ms = reactive_backoff_ms(&config, &body, 0);
/// assert!(ms >= 500);
/// ```
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
pub fn reactive_backoff_ms(config: &CostThrottleConfig, body: &Value, attempt: u32) -> u64 {
    let status = body.pointer("/extensions/cost/throttleStatus");
    let max_avail = status
        .and_then(|s| s.get("maximumAvailable"))
        .and_then(Value::as_f64)
        .unwrap_or(config.max_points);
    let cur_avail = status
        .and_then(|s| s.get("currentlyAvailable"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let restore_rate = status
        .and_then(|s| s.get("restoreRate"))
        .and_then(Value::as_f64)
        .unwrap_or(config.restore_per_sec)
        .max(1.0);
    let deficit = (max_avail - cur_avail).max(0.0);
    let base_secs = if deficit > 0.0 {
        (deficit / restore_rate) * 1.1
    } else {
        0.5
    };
    let backoff = base_secs * 1.5_f64.powi(attempt as i32);
    let ms = (backoff * 1_000.0) as u64;
    ms.clamp(500, config.max_delay_ms)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    clippy::unwrap_used,
    clippy::significant_drop_tightening
)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn live_budget_initialises_from_config() {
        let config = CostThrottleConfig {
            max_points: 5_000.0,
            restore_per_sec: 250.0,
            min_available: 50.0,
            max_delay_ms: 10_000,
        };
        let budget = LiveBudget::new(&config);
        assert_eq!(budget.currently_available, 5_000.0);
        assert_eq!(budget.maximum_available, 5_000.0);
        assert_eq!(budget.restore_rate, 250.0);
    }

    #[test]
    fn live_budget_updates_from_response() {
        let config = CostThrottleConfig::default();
        let mut budget = LiveBudget::new(&config);

        let status = json!({
            "maximumAvailable": 10_000.0,
            "currentlyAvailable": 3_000.0,
            "restoreRate": 500.0,
        });
        budget.update_from_response(&status);

        assert_eq!(budget.currently_available, 3_000.0);
        assert_eq!(budget.maximum_available, 10_000.0);
    }

    #[test]
    fn projected_available_accounts_for_restore() {
        let config = CostThrottleConfig {
            max_points: 10_000.0,
            restore_per_sec: 1_000.0, // fast restore for test
            ..Default::default()
        };
        let mut budget = LiveBudget::new(&config);
        // Simulate a low budget
        budget.currently_available = 0.0;
        // Immediately after update, projected = 0 + small_elapsed * 1000
        // which is ~ 0 (sub-millisecond). Just confirm it doesn't panic.
        let p = budget.projected_available();
        assert!(p >= 0.0);
        assert!(p <= 10_000.0);
    }

    #[test]
    fn projected_available_caps_at_maximum() {
        let config = CostThrottleConfig::default();
        let budget = LiveBudget::new(&config);
        // Fresh budget is already at maximum
        assert!(budget.projected_available() <= budget.maximum_available);
    }

    #[tokio::test]
    async fn pre_flight_delay_does_not_sleep_when_budget_healthy() {
        let budget = PluginBudget::new(CostThrottleConfig::default());
        // Budget starts full — no delay expected.
        let before = Instant::now();
        pre_flight_delay(&budget).await;
        assert!(before.elapsed().as_millis() < 100, "unexpected delay");
    }

    #[tokio::test]
    async fn update_budget_parses_throttle_status() {
        let budget = PluginBudget::new(CostThrottleConfig::default());
        let response = json!({
            "data": {},
            "extensions": { "cost": { "throttleStatus": {
                "maximumAvailable": 10_000.0,
                "currentlyAvailable": 2_500.0,
                "restoreRate": 500.0,
            }}}
        });
        update_budget(&budget, &response).await;
        let guard = budget.inner.lock().await;
        assert_eq!(guard.currently_available, 2_500.0);
    }

    #[test]
    fn reactive_backoff_ms_clamps_to_500ms_floor() {
        let config = CostThrottleConfig::default();
        let body = json!({ "extensions": { "cost": { "throttleStatus": {
            "maximumAvailable": 10_000.0,
            "currentlyAvailable": 9_999.0,
            "restoreRate": 500.0,
        }}}});
        let ms = reactive_backoff_ms(&config, &body, 0);
        assert_eq!(ms, 500); // Very small deficit rounds up to floor
    }

    #[test]
    fn reactive_backoff_ms_increases_with_attempt() {
        let config = CostThrottleConfig::default();
        let body = json!({ "extensions": { "cost": { "throttleStatus": {
            "maximumAvailable": 10_000.0,
            "currentlyAvailable": 5_000.0,
            "restoreRate": 500.0,
        }}}});
        let ms0 = reactive_backoff_ms(&config, &body, 0);
        let ms1 = reactive_backoff_ms(&config, &body, 1);
        let ms2 = reactive_backoff_ms(&config, &body, 2);
        assert!(ms1 > ms0);
        assert!(ms2 > ms1);
    }

    #[test]
    fn reactive_backoff_ms_caps_at_max_delay() {
        let config = CostThrottleConfig {
            max_delay_ms: 1_000,
            ..Default::default()
        };
        let body = json!({ "extensions": { "cost": { "throttleStatus": {
            "maximumAvailable": 10_000.0,
            "currentlyAvailable": 0.0,
            "restoreRate": 1.0, // very slow restore → huge deficit
        }}}});
        let ms = reactive_backoff_ms(&config, &body, 10);
        assert_eq!(ms, 1_000);
    }
}
