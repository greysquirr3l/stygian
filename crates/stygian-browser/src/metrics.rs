//! Performance metrics for stygian-browser.
//!
//! Tracks browser pool utilisation, acquisition latency, crash rates, and
//! process memory.  Metrics are exported in **Prometheus text format** via
//! [`gather`].
//!
//! ## Enabling
//!
//! ```toml
//! [dependencies]
//! stygian-browser = { version = "0.1", features = ["metrics"] }
//! ```
//!
//! ## Example
//!
//! ```no_run
//! use stygian_browser::metrics::{gather, METRICS};
//!
//! // After your scraping loop:
//! let report = gather();
//! println!("{report}");
//! ```
//!
//! ## Prometheus metrics
//!
//! | Name | Type | Description |
//! | ------ | ------ | ------------- |
//! | `browser_pool_size` | Gauge | Number of active browser instances |
//! | `browser_acquisition_duration_seconds` | Histogram | Time to acquire a browser |
//! | `browser_crashes_total` | Counter | Cumulative browser crashes |
//! | `process_rss_bytes` | Gauge | Process resident set size (Linux only) |

use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use prometheus_client::{
    encoding::text::encode,
    metrics::{
        counter::Counter,
        gauge::Gauge,
        histogram::{Histogram, exponential_buckets},
    },
    registry::Registry,
};
use tracing::{error, warn};

// ─── Thresholds ──────────────────────────────────────────────────────────────

/// Acquisition time beyond which a warning is emitted.
const WARN_ACQUISITION_SECS: f64 = 1.0;

/// Crash rate (crashes / acquisitions) beyond which an error is logged.
const ALERT_CRASH_RATE_THRESHOLD: f64 = 0.10;

// ─── Metrics container ───────────────────────────────────────────────────────

/// Global Prometheus metrics for the browser pool.
///
/// Obtain a reference via the [`METRICS`] static.
pub struct BrowserMetrics {
    /// Active browser instance count.
    pub pool_size: Gauge,
    /// Histogram of browser acquisition durations in seconds.
    pub acquisition_duration_seconds: Histogram,
    /// Total browser crashes (unexpected terminations or health-check failures).
    pub crashes_total: Counter,
    /// Process RSS in bytes (Linux only; 0 on other platforms).
    pub process_rss_bytes: Gauge,
    /// Total acquisitions — used to compute crash rate.
    acquisitions_total: Counter,
    /// Prometheus text registry (mutex-guarded for `encode`).
    registry: Mutex<Registry>,
}

impl BrowserMetrics {
    fn new() -> Self {
        // Histogram buckets: 5 ms → ~20 s (12 exponential buckets, factor 2)
        let acquisition_duration_seconds = Histogram::new(exponential_buckets(0.005, 2.0, 12));
        let pool_size = Gauge::default();
        let crashes_total: Counter = Counter::default();
        let acquisitions_total: Counter = Counter::default();
        let process_rss_bytes = Gauge::default();

        let mut registry = Registry::default();
        registry.register(
            "browser_pool_size",
            "Number of active browser instances currently in use",
            pool_size.clone(),
        );
        registry.register(
            "browser_acquisition_duration_seconds",
            "Time taken to acquire a browser instance from the pool",
            acquisition_duration_seconds.clone(),
        );
        registry.register(
            "browser_crashes_total",
            "Cumulative number of browser crashes or health-check failures",
            crashes_total.clone(),
        );
        registry.register(
            "browser_acquisitions_total",
            "Cumulative number of browser acquisition calls",
            acquisitions_total.clone(),
        );
        registry.register(
            "process_rss_bytes",
            "Resident set size of the current process in bytes",
            process_rss_bytes.clone(),
        );

        Self {
            pool_size,
            acquisition_duration_seconds,
            crashes_total,
            acquisitions_total,
            process_rss_bytes,
            registry: Mutex::new(registry),
        }
    }

    /// Record a browser acquisition that took `duration`.
    ///
    /// Emits a warning if `duration` exceeds the 1-second performance budget.
    /// Logs an error if the crash rate exceeds 10%.
    pub fn record_acquisition(&self, duration: Duration) {
        let secs = duration.as_secs_f64();
        self.acquisition_duration_seconds.observe(secs);
        self.acquisitions_total.inc();

        if secs > WARN_ACQUISITION_SECS {
            warn!(
                elapsed_ms = duration.as_millis(),
                "Browser acquisition exceeded 1s performance budget"
            );
        }

        // Crash rate check
        let crashes = self.crashes_total.get();
        let acquires = self.acquisitions_total.get();
        if acquires > 0 {
            // Cap to u32 range so f64::from() is lossless (counts this large are never realistic)
            let cap = u64::from(u32::MAX);
            let rate = f64::from(u32::try_from(crashes.min(cap)).unwrap_or(u32::MAX))
                / f64::from(u32::try_from(acquires.min(cap)).unwrap_or(u32::MAX));
            if rate > ALERT_CRASH_RATE_THRESHOLD {
                error!(
                    crash_rate = format!("{:.1}%", rate * 100.0),
                    crashes, acquires, "Browser crash rate exceeds 10% alert threshold"
                );
            }
        }
    }

    /// Update the pool size gauge.
    pub fn set_pool_size(&self, active: i64) {
        self.pool_size.set(active);
    }

    /// Record a browser crash or health-check failure.
    pub fn record_crash(&self) {
        self.crashes_total.inc();
    }

    /// Refresh the process RSS gauge and return the current value in bytes.
    ///
    /// Returns `0` on platforms where `/proc/self/status` is unavailable.
    pub fn refresh_rss(&self) -> i64 {
        let rss = rss_bytes();
        self.process_rss_bytes.set(rss);
        rss
    }

    /// Encode all metrics as Prometheus text exposition format.
    ///
    /// # Errors
    ///
    /// Returns an empty string if encoding fails (registry mutex poisoned).
    pub fn gather(&self) -> String {
        self.refresh_rss();
        let guard = match self.registry.lock() {
            Ok(g) => g,
            Err(e) => {
                warn!("Metrics registry lock poisoned: {e}");
                return String::new();
            }
        };
        let mut buf = String::new();
        if let Err(e) = encode(&mut buf, &guard) {
            warn!("Failed to encode Prometheus metrics: {e}");
        }
        buf
    }
}

// ─── Global singleton ────────────────────────────────────────────────────────

/// Global metrics instance.
///
/// Use this to record acquisitions, crashes, or to produce the Prometheus
/// text output via [`BrowserMetrics::gather`].
pub static METRICS: LazyLock<BrowserMetrics> = LazyLock::new(BrowserMetrics::new);

/// Convenience alias for [`METRICS.gather()`](BrowserMetrics::gather).
///
/// # Example
///
/// ```
/// use stygian_browser::metrics::gather;
/// let text = gather();
/// assert!(text.contains("browser_pool_size"));
/// ```
pub fn gather() -> String {
    METRICS.gather()
}

// ─── Platform-specific RSS ───────────────────────────────────────────────────

/// Read process RSS from `/proc/self/status` (Linux) or return 0.
// Not `const fn`: the Linux branch reads `/proc/self/status` (file I/O).
// On other platforms clippy would suggest `const fn` because the body is just
// `0`, but that would break cross-platform compilation.
#[allow(clippy::missing_const_for_fn)]
fn rss_bytes() -> i64 {
    #[cfg(target_os = "linux")]
    {
        read_linux_rss().unwrap_or(0)
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

#[cfg(target_os = "linux")]
fn read_linux_rss() -> Option<i64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: i64 = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())?;
            return Some(kb * 1024);
        }
    }
    None
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn fresh_metrics() -> BrowserMetrics {
        BrowserMetrics::new()
    }

    #[test]
    fn pool_size_gauge_tracks_value() {
        let m = fresh_metrics();
        m.set_pool_size(3);
        assert_eq!(m.pool_size.get(), 3);
        m.set_pool_size(0);
        assert_eq!(m.pool_size.get(), 0);
    }

    #[test]
    fn crash_counter_increments() {
        let m = fresh_metrics();
        m.record_crash();
        m.record_crash();
        assert_eq!(m.crashes_total.get(), 2);
    }

    #[test]
    fn acquisition_duration_observes() {
        let m = fresh_metrics();
        m.record_acquisition(Duration::from_millis(100));
        m.record_acquisition(Duration::from_millis(500));
        // Acquisitions counter should be at 2
        assert_eq!(m.acquisitions_total.get(), 2);
    }

    #[test]
    fn gather_contains_metric_names() {
        let m = fresh_metrics();
        m.set_pool_size(2);
        m.record_crash();
        let output = m.gather();
        assert!(output.contains("browser_pool_size"), "missing pool_size");
        assert!(
            output.contains("browser_crashes_total"),
            "missing crashes_total"
        );
        assert!(
            output.contains("browser_acquisition_duration_seconds"),
            "missing acquisition histogram"
        );
    }

    #[test]
    fn global_gather_has_expected_keys() {
        let output = gather();
        assert!(output.contains("browser_pool_size"));
    }

    #[test]
    fn rss_is_non_negative() {
        // On any platform, RSS must be >= 0
        assert!(rss_bytes() >= 0);
    }
}
