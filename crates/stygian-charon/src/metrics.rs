//! Telemetry and metrics collection for SLO assessment and escalation operations.
//!
//! This module provides structured metrics for monitoring SLO-driven acquisition decisions.
//! Metrics are collected optionally (feature-gated) to provide observability without overhead
//! when disabled.
//!
//! # Metrics Types
//!
//! - **slo_assessment_count**: Counter of investigations with SLO assessment
//! - **escalation_triggered_count**: Counter of escalations (warning + critical)
//! - **blocked_ratio_histogram**: Distribution of blocked ratios observed
//! - **target_class_distribution**: Breakdown by target class (API, ContentSite, HighSecurity, Unknown)
//! - **escalation_level_distribution**: Breakdown by escalation level (Acceptable, Medium, High)
//!
//! # Usage (Feature-Gated)
//!
//! When the `metrics` feature is enabled:
//!
//! ```rust,ignore
//! use stygian_charon::metrics::MetricsCollector;
//!
//! let collector = MetricsCollector::new();
//! let report = investigate_har(&har)?;
//! let requirements = infer_requirements_with_target_class(&report, TargetClass::Api);
//! collector.record_assessment(&report, &requirements);
//! ```
//!
//! Metrics can be exported to Prometheus format:
//!
//! ```rust,ignore
//! println!("{}", collector.to_prometheus());
//! ```

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Global metrics collector for SLO assessment operations.
///
/// This is a thread-safe singleton that accumulates metrics across all
/// assessment and escalation operations.
#[derive(Clone)]
pub struct MetricsCollector {
    // Counters
    assessments_total: Arc<AtomicU64>,
    escalations_warning: Arc<AtomicU64>,
    escalations_critical: Arc<AtomicU64>,

    // Histograms (simplified: min/max/sum/count)
    blocked_ratio_min: Arc<AtomicU64>, // Stored as u64 bits of f64
    blocked_ratio_max: Arc<AtomicU64>,
    blocked_ratio_sum: Arc<AtomicU64>,
    blocked_ratio_count: Arc<AtomicU64>,

    // Distributions (use Arc<Mutex> for interior mutability)
    target_class_counts: Arc<std::sync::Mutex<HashMap<String, u64>>>,
    escalation_level_counts: Arc<std::sync::Mutex<HashMap<String, u64>>>,
}

impl MetricsCollector {
    /// Create a new metrics collector.
    pub fn new() -> Self {
        Self {
            assessments_total: Arc::new(AtomicU64::new(0)),
            escalations_warning: Arc::new(AtomicU64::new(0)),
            escalations_critical: Arc::new(AtomicU64::new(0)),
            blocked_ratio_min: Arc::new(AtomicU64::new(u64::MAX)),
            blocked_ratio_max: Arc::new(AtomicU64::new(0)),
            blocked_ratio_sum: Arc::new(AtomicU64::new(0)),
            blocked_ratio_count: Arc::new(AtomicU64::new(0)),
            target_class_counts: Arc::new(std::sync::Mutex::new(HashMap::new())),
            escalation_level_counts: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Record an SLO assessment event.
    ///
    /// This increments assessment counters and updates blocked ratio histogram.
    /// Intended to be called after `investigate_har()` and `infer_requirements_with_target_class()`.
    pub fn record_assessment(
        &self,
        blocked_requests: u64,
        total_requests: u64,
        target_class: &str,
        escalation_level: &str,
    ) {
        // Increment assessment counter
        self.assessments_total.fetch_add(1, Ordering::Relaxed);

        // Update escalation counters
        match escalation_level {
            "Medium" => {
                self.escalations_warning.fetch_add(1, Ordering::Relaxed);
            }
            "High" => {
                self.escalations_critical.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }

        // Update blocked ratio histogram
        if total_requests > 0 {
            let blocked_ratio = to_f64(blocked_requests) / to_f64(total_requests);
            let ratio_bits = blocked_ratio.to_bits();

            // Update min
            let mut min = self.blocked_ratio_min.load(Ordering::Relaxed);
            while ratio_bits < min {
                match self.blocked_ratio_min.compare_exchange(
                    min,
                    ratio_bits,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(actual) => min = actual,
                }
            }

            // Update max
            let mut max = self.blocked_ratio_max.load(Ordering::Relaxed);
            while ratio_bits > max {
                match self.blocked_ratio_max.compare_exchange(
                    max,
                    ratio_bits,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(actual) => max = actual,
                }
            }

            // Update sum (approximate, may overflow but that's ok for this use)
            self.blocked_ratio_sum
                .fetch_add(ratio_bits, Ordering::Relaxed);
            self.blocked_ratio_count.fetch_add(1, Ordering::Relaxed);
        }

        // Update target class distribution
        if let Ok(mut counts) = self.target_class_counts.lock() {
            *counts.entry(target_class.to_string()).or_insert(0) += 1;
        }

        // Update escalation level distribution
        if let Ok(mut counts) = self.escalation_level_counts.lock() {
            *counts.entry(escalation_level.to_string()).or_insert(0) += 1;
        }
    }

    /// Export metrics in Prometheus text format.
    pub fn to_prometheus(&self) -> String {
        let mut output = String::new();

        // Assessment counter
        let total = self.assessments_total.load(Ordering::Relaxed);
        output.push_str("# HELP slo_assessment_total Total SLO assessments performed\n");
        output.push_str("# TYPE slo_assessment_total counter\n");
        let _ = writeln!(output, "slo_assessment_total {total}\n");

        // Escalation counters
        let warnings = self.escalations_warning.load(Ordering::Relaxed);
        let criticals = self.escalations_critical.load(Ordering::Relaxed);
        output.push_str("# HELP slo_escalation_warning_total SLO escalations in warning zone\n");
        output.push_str("# TYPE slo_escalation_warning_total counter\n");
        let _ = writeln!(output, "slo_escalation_warning_total {warnings}\n");

        output.push_str("# HELP slo_escalation_critical_total SLO escalations in critical zone\n");
        output.push_str("# TYPE slo_escalation_critical_total counter\n");
        let _ = writeln!(output, "slo_escalation_critical_total {criticals}\n");

        // Blocked ratio histogram
        let count = self.blocked_ratio_count.load(Ordering::Relaxed);
        if count > 0 {
            let min_bits = self.blocked_ratio_min.load(Ordering::Relaxed);
            let max_bits = self.blocked_ratio_max.load(Ordering::Relaxed);
            let sum_bits = self.blocked_ratio_sum.load(Ordering::Relaxed);

            let min = f64::from_bits(min_bits);
            let max = f64::from_bits(max_bits);
            let avg_bits = sum_bits.checked_div(count).unwrap_or_default();
            let avg = f64::from_bits(avg_bits);

            output.push_str("# HELP slo_blocked_ratio_min Minimum blocked ratio observed\n");
            output.push_str("# TYPE slo_blocked_ratio_min gauge\n");
            let _ = writeln!(output, "slo_blocked_ratio_min {min}\n");

            output.push_str("# HELP slo_blocked_ratio_max Maximum blocked ratio observed\n");
            output.push_str("# TYPE slo_blocked_ratio_max gauge\n");
            let _ = writeln!(output, "slo_blocked_ratio_max {max}\n");

            output.push_str("# HELP slo_blocked_ratio_avg Average blocked ratio\n");
            output.push_str("# TYPE slo_blocked_ratio_avg gauge\n");
            let _ = writeln!(output, "slo_blocked_ratio_avg {avg}\n");
        }

        // Target class distribution
        if let Ok(counts) = self.target_class_counts.lock()
            && !counts.is_empty()
        {
            output.push_str("# HELP slo_target_class_total Assessments by target class\n");
            output.push_str("# TYPE slo_target_class_total counter\n");
            for (class, count) in counts.iter() {
                let _ = writeln!(
                    output,
                    "slo_target_class_total{{class=\"{class}\"}} {count}"
                );
            }
            output.push('\n');
        }

        // Escalation level distribution
        if let Ok(counts) = self.escalation_level_counts.lock()
            && !counts.is_empty()
        {
            output.push_str("# HELP slo_escalation_level_total Assessments by escalation level\n");
            output.push_str("# TYPE slo_escalation_level_total counter\n");
            for (level, count) in counts.iter() {
                let _ = writeln!(
                    output,
                    "slo_escalation_level_total{{level=\"{level}\"}} {count}"
                );
            }
        }

        output
    }

    /// Clear all metrics (useful for testing).
    pub fn reset(&self) {
        self.assessments_total.store(0, Ordering::Relaxed);
        self.escalations_warning.store(0, Ordering::Relaxed);
        self.escalations_critical.store(0, Ordering::Relaxed);
        self.blocked_ratio_min.store(u64::MAX, Ordering::Relaxed);
        self.blocked_ratio_max.store(0, Ordering::Relaxed);
        self.blocked_ratio_sum.store(0, Ordering::Relaxed);
        self.blocked_ratio_count.store(0, Ordering::Relaxed);

        if let Ok(mut counts) = self.target_class_counts.lock() {
            counts.clear();
        }
        if let Ok(mut counts) = self.escalation_level_counts.lock() {
            counts.clear();
        }
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(clippy::cast_precision_loss)]
const fn to_f64(value: u64) -> f64 {
    value as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_collector_increments_assessment_count() {
        let collector = MetricsCollector::new();
        collector.record_assessment(50, 1000, "Api", "Acceptable");
        collector.record_assessment(100, 1000, "ContentSite", "Medium");
        collector.record_assessment(150, 1000, "HighSecurity", "High");

        let prometheus = collector.to_prometheus();
        assert!(prometheus.contains("slo_assessment_total 3"));
    }

    #[test]
    fn metrics_collector_tracks_escalations() {
        let collector = MetricsCollector::new();
        collector.record_assessment(50, 1000, "Api", "Acceptable");
        collector.record_assessment(100, 1000, "Api", "Medium");
        collector.record_assessment(150, 1000, "Api", "High");

        let prometheus = collector.to_prometheus();
        assert!(prometheus.contains("slo_escalation_warning_total 1"));
        assert!(prometheus.contains("slo_escalation_critical_total 1"));
    }

    #[test]
    fn metrics_collector_tracks_target_class_distribution() {
        let collector = MetricsCollector::new();
        collector.record_assessment(50, 1000, "Api", "Acceptable");
        collector.record_assessment(100, 1000, "ContentSite", "Medium");
        collector.record_assessment(150, 1000, "Api", "High");

        let prometheus = collector.to_prometheus();
        assert!(prometheus.contains("class=\"Api\""));
        assert!(prometheus.contains("class=\"ContentSite\""));
    }

    #[test]
    fn metrics_collector_reset() {
        let collector = MetricsCollector::new();
        collector.record_assessment(50, 1000, "Api", "Acceptable");

        collector.reset();
        let prometheus = collector.to_prometheus();
        // After reset, metrics should show 0 counts
        assert!(prometheus.contains("slo_assessment_total 0"));
    }
}
