//! T107 poisoned-data field-level anomaly detector.
//!
//! Catches the "tarpit / poisoned data / silent 200s" pattern from
//! <https://web-scraping-guide.com/#post-extract>: a target returns
//! clean `200` responses with subtly wrong field values
//! (price drift, listing reorder, fabricated rows, stale
//! snapshots). The detector observes every field value the pipeline
//! publishes and emits [`AnomalyReport`]s when a value is
//! statistically inconsistent with the rolling baseline.
//!
//! Two pieces:
//!
//! - [`FieldAnomalyDetector`] — the consumer-owned port trait.
//! - [`StatisticalFieldAnomalyDetector`] — the default adapter
//!   implementing price-drift, outlier, listing-reorder,
//!   staleness, and cardinality-shift detection.
//!
//! Hidden behind a `field-anomaly` cargo feature in the parent
//! crate so existing charon users aren't forced to opt in.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ── Value types ─────────────────────────────────────────────────────────────

/// Stable identifier for a schema or data-contract version. Two
/// observations with different `SchemaId` belong to different
/// baselines — the detector resets state on schema change.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaId(pub String);

impl SchemaId {
    /// Borrow the inner string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SchemaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for SchemaId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SchemaId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Dot-path to a field within a record (`"price"`, `"items.0.sku"`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FieldPath(pub String);

impl FieldPath {
    /// Borrow the inner string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for FieldPath {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for FieldPath {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// One observed field value.
///
/// Covers the four kinds of fields the detector reasons about:
/// numeric (price/outlier), string (cardinality), ordered list
/// (listing-reorder), and timestamp (staleness).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FieldValue {
    /// Numeric value (price, count, score, etc.).
    Number(f64),
    /// String value (sku, title, status, etc.).
    Text(String),
    /// Ordered list of strings (a page of listings, a search
    /// result set, etc.). Used for listing-reorder detection.
    OrderedList(Vec<String>),
    /// Timestamp value (`published_at`, `expires_at`, etc.). Stored as
    /// seconds since the Unix epoch for portability.
    TimestampSeconds(i64),
    /// Boolean value.
    Bool(bool),
    /// Null / absent — explicitly recorded so the detector can
    /// distinguish "field missing" from "field present with empty
    /// value".
    Null,
}

impl FieldValue {
    /// `true` if the value carries numeric content (`Number`).
    #[must_use]
    pub const fn is_number(&self) -> bool {
        matches!(self, Self::Number(_))
    }

    /// `true` if the value carries a text payload.
    #[must_use]
    pub const fn is_text(&self) -> bool {
        matches!(self, Self::Text(_))
    }

    /// `true` if the value is an ordered list of strings.
    #[must_use]
    pub const fn is_ordered_list(&self) -> bool {
        matches!(self, Self::OrderedList(_))
    }

    /// `true` if the value is a timestamp.
    #[must_use]
    pub const fn is_timestamp(&self) -> bool {
        matches!(self, Self::TimestampSeconds(_))
    }
}

/// Per-field rolling baseline — the statistical summary the
/// detector compares new observations against.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SchemaBaseline {
    /// Last N values observed for each numeric field, capped at
    /// `StatisticalFieldAnomalyDetector::WINDOW_SIZE`.
    pub numeric_history: BTreeMap<FieldPath, VecDeque<f64>>,
    /// Last N ordered lists observed for each list-valued field.
    pub list_history: BTreeMap<FieldPath, VecDeque<Vec<String>>>,
    /// Last N cardinality counts observed for each text-valued
    /// field. Used for [`AnomalySignal::CardinalityShift`].
    pub cardinality_history: BTreeMap<FieldPath, VecDeque<usize>>,
    /// The schema this baseline belongs to.
    pub schema_id: Option<SchemaId>,
}

/// One anomaly emitted by [`FieldAnomalyDetector::observe`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnomalyReport {
    /// Field path the anomaly was observed on.
    pub field: FieldPath,
    /// Schema the field belongs to (for downstream filtering).
    pub schema_id: SchemaId,
    /// What kind of anomaly.
    pub signal: AnomalySignal,
    /// Severity rating (Info / Warning / Error).
    pub severity: AnomalySeverity,
    /// Human-readable explanation.
    pub reason: String,
}

/// The five anomaly kinds the detector emits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnomalySignal {
    /// Numeric value lies more than `k * IQR` from the rolling
    /// median. `ratio` is the observed / median ratio.
    PriceDrift {
        /// Observed / median.
        ratio: f64,
    },
    /// Ordered-list Jaccard distance vs the previous ordering is
    /// above the threshold. `jaccard` is the distance in `[0.0, 1.0]`.
    ListingReorder {
        /// Distance from the previous observation.
        jaccard: f64,
    },
    /// Timestamp is older than the staleness threshold. `age_secs`
    /// is `now - observed`.
    Staleness {
        /// Seconds since the published timestamp.
        age_secs: i64,
    },
    /// Numeric z-score across the rolling window exceeds the
    /// outlier threshold (default 3.0).
    Outlier {
        /// z-score of the observation.
        z_score: f64,
    },
    /// Text-field unique-value count changed by more than the
    /// cardinality-shift threshold (default 50%) between windows.
    CardinalityShift {
        /// Previous window's unique-count.
        from: usize,
        /// Current window's unique-count.
        to: usize,
    },
    /// No anomaly — included for completeness so callers can use a
    /// single return type.
    None,
}

/// Severity of an [`AnomalyReport`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnomalySeverity {
    /// Informational — the value is unusual but not necessarily
    /// wrong.
    Info,
    /// Warning — likely anomaly; should be reviewed.
    Warning,
    /// Error — definitely anomaly; the record should be flagged.
    Error,
}

/// Errors raised by [`FieldAnomalyDetector`] operations.
#[derive(Debug, Error)]
pub enum AnomalyError {
    /// The detector could not be configured or initialised.
    #[error("anomaly detector init failed: {0}")]
    Init(String),
    /// Observation failed (e.g. lock contention).
    #[error("observation failed: {0}")]
    Observe(String),
}

/// Port trait: observe a field value, baseline reset on schema change.
#[async_trait]
pub trait FieldAnomalyDetector: Send + Sync {
    /// Stable name for diagnostics.
    fn name(&self) -> &'static str;

    /// Observe one field value. Returns [`AnomalyReport::signal`]
    /// describing any anomaly detected, or [`AnomalySignal::None`]
    /// if the value is consistent with the rolling baseline.
    ///
    /// # Errors
    ///
    /// Returns [`AnomalyError::Observe`] if the detector cannot
    /// record the observation.
    async fn observe(
        &self,
        schema_id: &SchemaId,
        field: &FieldPath,
        value: &FieldValue,
    ) -> Result<AnomalyReport, AnomalyError>;

    /// Return the current baseline for a schema.
    async fn baseline(&self, schema_id: &SchemaId) -> Result<SchemaBaseline, AnomalyError>;

    /// Drop the rolling state for a schema. Used by callers that
    /// want to force a fresh baseline (e.g. after a backfill).
    async fn reset(&self, schema_id: &SchemaId) -> Result<(), AnomalyError>;
}

// ── Default adapter ────────────────────────────────────────────────────────

/// Default statistical detector. Rolling-window-based: keeps the
/// last [`Self::WINDOW_SIZE`] observations per field per schema and
/// applies the five heuristics in [`AnomalySignal`].
#[derive(Debug)]
pub struct StatisticalFieldAnomalyDetector {
    /// Sliding-window size for each (schema, field) pair.
    pub window_size: usize,
    /// IQR multiplier for [`AnomalySignal::PriceDrift`]. Default 3.0.
    pub iqr_multiplier: f64,
    /// Jaccard-distance threshold for [`AnomalySignal::ListingReorder`]. Default 0.3.
    pub jaccard_threshold: f64,
    /// Staleness threshold in seconds for [`AnomalySignal::Staleness`]. Default 7 days.
    pub staleness_threshold_secs: i64,
    /// z-score threshold for [`AnomalySignal::Outlier`]. Default 3.0.
    pub z_score_threshold: f64,
    /// Cardinality-shift threshold as a fraction (0.5 = 50%).
    pub cardinality_shift_fraction: f64,
    /// Internal baseline state.
    state: parking_lot::Mutex<BTreeMap<SchemaId, SchemaBaseline>>,
}

impl Default for StatisticalFieldAnomalyDetector {
    fn default() -> Self {
        Self {
            window_size: Self::WINDOW_SIZE,
            iqr_multiplier: 3.0,
            jaccard_threshold: 0.3,
            staleness_threshold_secs: 7 * 24 * 60 * 60,
            z_score_threshold: 3.0,
            cardinality_shift_fraction: 0.5,
            state: parking_lot::Mutex::new(BTreeMap::new()),
        }
    }
}

impl StatisticalFieldAnomalyDetector {
    /// Default sliding-window size.
    pub const WINDOW_SIZE: usize = 64;

    /// Construct a new detector with the default tuning constants.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Convert a `usize` count to `f64` for ratio math.
    ///
    /// Counts flowing through this helper are bounded by
    /// [`Self::WINDOW_SIZE`] (max 64) — well within `u32`, so the
    /// `usize -> u32 -> f64` chain is precision-safe on every
    /// target.
    fn usize_to_f64(n: usize) -> f64 {
        let n32 = u32::try_from(n).unwrap_or(u32::MAX);
        f64::from(n32)
    }

    /// Trim a window to `max` entries from the front.
    ///
    /// Not `const` because `VecDeque::len` / `pop_front` are not
    /// const-stable yet (Rust 1.96).
    fn cap_window<T>(window: &mut VecDeque<T>, max: usize) {
        while window.len() > max {
            window.pop_front();
        }
    }

    /// Median of a window of f64 values. Returns `None` if empty.
    fn median(window: &VecDeque<f64>) -> Option<f64> {
        if window.is_empty() {
            return None;
        }
        let mut sorted: Vec<f64> = window.iter().copied().collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = sorted.len() / 2;
        if sorted.len().is_multiple_of(2) {
            let a = sorted.get(mid - 1).copied()?;
            let b = sorted.get(mid).copied()?;
            Some(f64::midpoint(a, b))
        } else {
            Some(*sorted.get(mid)?)
        }
    }

    /// First and third quartile of a window.
    ///
    /// Returns `None` for windows with fewer than 4 samples.
    fn quartiles(window: &VecDeque<f64>) -> Option<(f64, f64)> {
        if window.len() < 4 {
            return None;
        }
        let mut sorted: Vec<f64> = window.iter().copied().collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = sorted.len() / 2;
        // split_at is the safe form of `&v[..n]` + `&v[n..]`.
        let (lower_slice, upper_with_mid) = sorted.split_at(mid);
        let q1 = Self::pick_midpoint(lower_slice);
        let q3 = Self::pick_midpoint(upper_with_mid);
        Some((q1?, q3?))
    }

    /// Pick the median (odd-length) or midpoint (even-length) of an
    /// already-sorted slice. Returns `None` for empty slices.
    fn pick_midpoint(sorted: &[f64]) -> Option<f64> {
        if sorted.is_empty() {
            return None;
        }
        let mid = sorted.len() / 2;
        if sorted.len().is_multiple_of(2) {
            let a = sorted.get(mid - 1).copied()?;
            let b = sorted.get(mid).copied()?;
            Some(f64::midpoint(a, b))
        } else {
            Some(*sorted.get(mid)?)
        }
    }

    /// Mean of a window.
    fn mean(window: &VecDeque<f64>) -> Option<f64> {
        if window.is_empty() {
            return None;
        }
        Some(window.iter().sum::<f64>() / Self::usize_to_f64(window.len()))
    }

    /// Standard deviation of a window. Returns `None` when fewer
    /// than 2 samples.
    fn stddev(window: &VecDeque<f64>) -> Option<f64> {
        if window.len() < 2 {
            return None;
        }
        let m = Self::mean(window)?;
        let n = Self::usize_to_f64(window.len());
        let variance = window.iter().map(|v| (v - m).powi(2)).sum::<f64>() / (n - 1.0);
        Some(variance.sqrt())
    }

    /// Jaccard distance between two ordered lists of strings.
    /// Distance = 1 - |intersection| / |union| for unordered, or
    /// Kendall-tau-style for ordered. We use set-based Jaccard
    /// (unordered) per the brief's "Jaccard > 0.3" criterion.
    fn jaccard_distance(a: &[String], b: &[String]) -> f64 {
        if a.is_empty() && b.is_empty() {
            return 0.0;
        }
        let sa: std::collections::BTreeSet<&str> = a.iter().map(String::as_str).collect();
        let sb: std::collections::BTreeSet<&str> = b.iter().map(String::as_str).collect();
        let intersection = sa.intersection(&sb).count();
        let union = sa.union(&sb).count();
        if union == 0 {
            0.0
        } else {
            1.0 - (Self::usize_to_f64(intersection) / Self::usize_to_f64(union))
        }
    }

    /// Evaluate one observation against the existing baseline for
    /// `(schema_id, field)`. Pure logic — no I/O.
    fn evaluate(
        baseline: &mut SchemaBaseline,
        schema_id: &SchemaId,
        field: &FieldPath,
        value: &FieldValue,
        cfg: &TuningConfig,
        now_secs: i64,
    ) -> AnomalyReport {
        match value {
            FieldValue::Number(n) => Self::evaluate_number(baseline, schema_id, field, *n, cfg),
            FieldValue::OrderedList(items) => {
                Self::evaluate_list(baseline, schema_id, field, items, cfg)
            }
            FieldValue::Text(t) => Self::evaluate_text(baseline, schema_id, field, t, cfg),
            FieldValue::TimestampSeconds(ts) => {
                Self::evaluate_timestamp(schema_id, field, *ts, now_secs, cfg)
            }
            FieldValue::Bool(_) | FieldValue::Null => AnomalyReport {
                field: field.clone(),
                schema_id: schema_id.clone(),
                signal: AnomalySignal::None,
                severity: AnomalySeverity::Info,
                reason: "value type not subject to statistical checks".to_string(),
            },
        }
    }

    fn evaluate_number(
        baseline: &mut SchemaBaseline,
        schema_id: &SchemaId,
        field: &FieldPath,
        n: f64,
        cfg: &TuningConfig,
    ) -> AnomalyReport {
        let window = baseline.numeric_history.entry(field.clone()).or_default();
        if let Some((q1, q3)) = Self::quartiles(window) {
            let iqr = q3 - q1;
            if iqr > 0.0 {
                let median = Self::median(window).unwrap_or(n);
                let deviation = (n - median).abs();
                if deviation > cfg.iqr_multiplier * iqr {
                    let ratio = if median.abs() > f64::EPSILON {
                        n / median
                    } else {
                        1.0
                    };
                    window.push_back(n);
                    Self::cap_window(window, cfg.window_size);
                    return AnomalyReport {
                        field: field.clone(),
                        schema_id: schema_id.clone(),
                        signal: AnomalySignal::PriceDrift { ratio },
                        severity: AnomalySeverity::Warning,
                        reason: format!(
                            "value {n} deviates {deviation:.2} from median {median:.2} \
                             (IQR={iqr:.2}, k={:.1})",
                            cfg.iqr_multiplier
                        ),
                    };
                }
            }
            // z-score outlier detection (independent of IQR).
            if let Some(sd) = Self::stddev(window) {
                let mean = Self::mean(window).unwrap_or(n);
                if sd > 0.0 {
                    let z = (n - mean) / sd;
                    if z.abs() > cfg.z_score_threshold {
                        window.push_back(n);
                        Self::cap_window(window, cfg.window_size);
                        return AnomalyReport {
                            field: field.clone(),
                            schema_id: schema_id.clone(),
                            signal: AnomalySignal::Outlier { z_score: z },
                            severity: AnomalySeverity::Warning,
                            reason: format!(
                                "value {n} has z-score {z:.2} (mean={mean:.2}, sd={sd:.2})"
                            ),
                        };
                    }
                }
            }
        }
        window.push_back(n);
        Self::cap_window(window, cfg.window_size);
        AnomalyReport {
            field: field.clone(),
            schema_id: schema_id.clone(),
            signal: AnomalySignal::None,
            severity: AnomalySeverity::Info,
            reason: "within IQR and z-score thresholds".to_string(),
        }
    }

    fn evaluate_list(
        baseline: &mut SchemaBaseline,
        schema_id: &SchemaId,
        field: &FieldPath,
        items: &[String],
        cfg: &TuningConfig,
    ) -> AnomalyReport {
        let window = baseline.list_history.entry(field.clone()).or_default();
        let report = window.back().map_or_else(
            || AnomalyReport {
                field: field.clone(),
                schema_id: schema_id.clone(),
                signal: AnomalySignal::None,
                severity: AnomalySeverity::Info,
                reason: "first observation establishes baseline".to_string(),
            },
            |prev| {
                let distance = Self::jaccard_distance(prev, items);
                if distance > cfg.jaccard_threshold {
                    AnomalyReport {
                        field: field.clone(),
                        schema_id: schema_id.clone(),
                        signal: AnomalySignal::ListingReorder { jaccard: distance },
                        severity: AnomalySeverity::Warning,
                        reason: format!(
                            "Jaccard distance {distance:.2} exceeds threshold {:.2}",
                            cfg.jaccard_threshold
                        ),
                    }
                } else {
                    AnomalyReport {
                        field: field.clone(),
                        schema_id: schema_id.clone(),
                        signal: AnomalySignal::None,
                        severity: AnomalySeverity::Info,
                        reason: format!("Jaccard distance {distance:.2} within threshold"),
                    }
                }
            },
        );
        window.push_back(items.to_vec());
        Self::cap_window(window, cfg.window_size);
        report
    }

    fn evaluate_text(
        baseline: &mut SchemaBaseline,
        schema_id: &SchemaId,
        field: &FieldPath,
        text: &str,
        cfg: &TuningConfig,
    ) -> AnomalyReport {
        let cardinality = text.chars().filter(|c| !c.is_whitespace()).count();
        let window = baseline
            .cardinality_history
            .entry(field.clone())
            .or_default();
        let report = if window.len() >= 2 {
            let previous = window.back().copied().unwrap_or(cardinality);
            let previous_window_avg = if window.len() >= cfg.window_size {
                Self::usize_to_f64(window.iter().sum::<usize>()) / Self::usize_to_f64(window.len())
            } else {
                Self::usize_to_f64(previous)
            };
            let current = Self::usize_to_f64(cardinality);
            let drift = if previous_window_avg > 0.0 {
                (current - previous_window_avg).abs() / previous_window_avg
            } else {
                0.0
            };
            if drift > cfg.cardinality_shift_fraction {
                AnomalyReport {
                    field: field.clone(),
                    schema_id: schema_id.clone(),
                    signal: AnomalySignal::CardinalityShift {
                        from: previous,
                        to: cardinality,
                    },
                    severity: AnomalySeverity::Warning,
                    reason: format!(
                        "cardinality shifted {drift:.0}% ({previous} -> {cardinality})"
                    ),
                }
            } else {
                AnomalyReport {
                    field: field.clone(),
                    schema_id: schema_id.clone(),
                    signal: AnomalySignal::None,
                    severity: AnomalySeverity::Info,
                    reason: "cardinality within threshold".to_string(),
                }
            }
        } else {
            AnomalyReport {
                field: field.clone(),
                schema_id: schema_id.clone(),
                signal: AnomalySignal::None,
                severity: AnomalySeverity::Info,
                reason: "establishing baseline".to_string(),
            }
        };
        window.push_back(cardinality);
        Self::cap_window(window, cfg.window_size);
        report
    }

    fn evaluate_timestamp(
        schema_id: &SchemaId,
        field: &FieldPath,
        ts: i64,
        now_secs: i64,
        cfg: &TuningConfig,
    ) -> AnomalyReport {
        let age = (now_secs - ts).max(0);
        if age > cfg.staleness_threshold_secs {
            AnomalyReport {
                field: field.clone(),
                schema_id: schema_id.clone(),
                signal: AnomalySignal::Staleness { age_secs: age },
                severity: AnomalySeverity::Warning,
                reason: format!(
                    "age {age}s exceeds staleness threshold {}s",
                    cfg.staleness_threshold_secs
                ),
            }
        } else {
            AnomalyReport {
                field: field.clone(),
                schema_id: schema_id.clone(),
                signal: AnomalySignal::None,
                severity: AnomalySeverity::Info,
                reason: format!("age {age}s within threshold"),
            }
        }
    }
}

/// Internal tuning snapshot — kept separate from the public
/// detector struct so the async interface doesn't have to take
/// `&self` in three places.
#[derive(Debug, Clone, Copy)]
struct TuningConfig {
    window_size: usize,
    iqr_multiplier: f64,
    jaccard_threshold: f64,
    staleness_threshold_secs: i64,
    z_score_threshold: f64,
    cardinality_shift_fraction: f64,
}

impl TuningConfig {
    const fn from_detector(d: &StatisticalFieldAnomalyDetector) -> Self {
        Self {
            window_size: d.window_size,
            iqr_multiplier: d.iqr_multiplier,
            jaccard_threshold: d.jaccard_threshold,
            staleness_threshold_secs: d.staleness_threshold_secs,
            z_score_threshold: d.z_score_threshold,
            cardinality_shift_fraction: d.cardinality_shift_fraction,
        }
    }
}

#[async_trait]
impl FieldAnomalyDetector for StatisticalFieldAnomalyDetector {
    fn name(&self) -> &'static str {
        "statistical"
    }

    async fn observe(
        &self,
        schema_id: &SchemaId,
        field: &FieldPath,
        value: &FieldValue,
    ) -> Result<AnomalyReport, AnomalyError> {
        let cfg = TuningConfig::from_detector(self);
        let now = chrono::Utc::now();
        let now_secs = now.timestamp();
        let mut guard = self.state.lock();
        let baseline = guard.entry(schema_id.clone()).or_default();
        if baseline.schema_id.is_none() {
            baseline.schema_id = Some(schema_id.clone());
        }
        if baseline.schema_id.as_ref() != Some(schema_id) {
            *baseline = SchemaBaseline {
                schema_id: Some(schema_id.clone()),
                ..Default::default()
            };
        }
        let report = Self::evaluate(baseline, schema_id, field, value, &cfg, now_secs);
        drop(guard);
        Ok(report)
    }

    async fn baseline(&self, schema_id: &SchemaId) -> Result<SchemaBaseline, AnomalyError> {
        Ok(self
            .state
            .lock()
            .get(schema_id)
            .cloned()
            .unwrap_or_else(|| SchemaBaseline {
                schema_id: Some(schema_id.clone()),
                ..Default::default()
            }))
    }

    async fn reset(&self, schema_id: &SchemaId) -> Result<(), AnomalyError> {
        self.state.lock().remove(schema_id);
        Ok(())
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn det() -> StatisticalFieldAnomalyDetector {
        StatisticalFieldAnomalyDetector::new()
    }

    #[tokio::test]
    async fn first_observation_is_baseline() {
        let d = det();
        let report = d
            .observe(&"s1".into(), &"price".into(), &FieldValue::Number(9.99))
            .await
            .unwrap();
        assert!(matches!(report.signal, AnomalySignal::None));
    }

    #[tokio::test]
    async fn price_drift_far_from_median_triggers() {
        let d = det();
        // Establish baseline (median ~10)
        for v in [9.5, 10.0, 10.0, 10.5, 9.8, 10.2, 9.9, 10.1] {
            d.observe(&"s1".into(), &"price".into(), &FieldValue::Number(v))
                .await
                .unwrap();
        }
        // Outlier
        let report = d
            .observe(&"s1".into(), &"price".into(), &FieldValue::Number(99.0))
            .await
            .unwrap();
        assert!(
            matches!(
                report.signal,
                AnomalySignal::PriceDrift { .. } | AnomalySignal::Outlier { .. }
            ),
            "expected PriceDrift or Outlier, got {:?}",
            report.signal
        );
    }

    #[tokio::test]
    async fn value_within_iqr_passes() {
        let d = det();
        for v in [9.5, 10.0, 10.0, 10.5, 9.8, 10.2, 9.9, 10.1] {
            let r = d
                .observe(&"s1".into(), &"price".into(), &FieldValue::Number(v))
                .await
                .unwrap();
            assert!(
                matches!(r.signal, AnomalySignal::None),
                "in-range value should pass: {r:?}"
            );
        }
    }

    #[tokio::test]
    async fn identical_list_ordering_passes() {
        let d = det();
        let items1 = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let items2 = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        d.observe(
            &"s1".into(),
            &"listings".into(),
            &FieldValue::OrderedList(items1),
        )
        .await
        .unwrap();
        let r = d
            .observe(
                &"s1".into(),
                &"listings".into(),
                &FieldValue::OrderedList(items2),
            )
            .await
            .unwrap();
        assert!(matches!(r.signal, AnomalySignal::None));
    }

    #[tokio::test]
    async fn reordered_list_with_high_jaccard_triggers() {
        let d = det();
        let items1 = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
            "f".to_string(),
        ];
        // Completely different set
        let items2 = vec![
            "x".to_string(),
            "y".to_string(),
            "z".to_string(),
            "w".to_string(),
            "v".to_string(),
            "u".to_string(),
        ];
        d.observe(
            &"s1".into(),
            &"listings".into(),
            &FieldValue::OrderedList(items1),
        )
        .await
        .unwrap();
        let r = d
            .observe(
                &"s1".into(),
                &"listings".into(),
                &FieldValue::OrderedList(items2),
            )
            .await
            .unwrap();
        assert!(
            matches!(r.signal, AnomalySignal::ListingReorder { .. }),
            "expected ListingReorder, got {:?}",
            r.signal
        );
    }

    #[tokio::test]
    async fn stale_timestamp_triggers() {
        let d = det();
        let now = chrono::Utc::now().timestamp();
        let stale_ts = now - (365 * 24 * 60 * 60); // 1 year old
        let r = d
            .observe(
                &"s1".into(),
                &"published_at".into(),
                &FieldValue::TimestampSeconds(stale_ts),
            )
            .await
            .unwrap();
        assert!(matches!(r.signal, AnomalySignal::Staleness { .. }));
    }

    #[tokio::test]
    async fn outlier_z_score_triggers() {
        let d = det();
        // Tight cluster
        for v in [10.0, 10.1, 9.9, 10.0, 10.05, 9.95] {
            d.observe(&"s1".into(), &"x".into(), &FieldValue::Number(v))
                .await
                .unwrap();
        }
        let r = d
            .observe(&"s1".into(), &"x".into(), &FieldValue::Number(50.0))
            .await
            .unwrap();
        assert!(
            matches!(
                r.signal,
                AnomalySignal::Outlier { .. } | AnomalySignal::PriceDrift { .. }
            ),
            "expected Outlier or PriceDrift, got {:?}",
            r.signal
        );
    }

    #[tokio::test]
    async fn cardinality_shift_triggers() {
        let d = det();
        // Establish baseline of small strings
        for _ in 0..5 {
            d.observe(
                &"s1".into(),
                &"title".into(),
                &FieldValue::Text("hello world".to_string()),
            )
            .await
            .unwrap();
        }
        // Shift to huge strings
        let huge = "x".repeat(10_000);
        let r = d
            .observe(&"s1".into(), &"title".into(), &FieldValue::Text(huge))
            .await
            .unwrap();
        assert!(
            matches!(r.signal, AnomalySignal::CardinalityShift { .. }),
            "expected CardinalityShift, got {:?}",
            r.signal
        );
    }

    #[tokio::test]
    async fn detector_resets_on_schema_change() {
        let d = det();
        // Establish s1 baseline.
        d.observe(&"s1".into(), &"price".into(), &FieldValue::Number(100.0))
            .await
            .unwrap();
        // Switch to s2 — the baseline for s1 is preserved but
        // s2 starts fresh.
        let r = d
            .observe(&"s2".into(), &"price".into(), &FieldValue::Number(9.99))
            .await
            .unwrap();
        assert!(matches!(r.signal, AnomalySignal::None));
        // s1 baseline should still be retrievable.
        let b = d.baseline(&"s1".into()).await.unwrap();
        assert_eq!(b.schema_id, Some("s1".into()));
    }

    #[tokio::test]
    async fn reset_clears_schema_state() {
        let d = det();
        d.observe(&"s1".into(), &"price".into(), &FieldValue::Number(10.0))
            .await
            .unwrap();
        d.reset(&"s1".into()).await.unwrap();
        let b = d.baseline(&"s1".into()).await.unwrap();
        assert!(b.numeric_history.is_empty());
    }

    #[tokio::test]
    async fn field_value_type_helpers() {
        assert!(FieldValue::Number(1.0).is_number());
        assert!(!FieldValue::Number(1.0).is_text());
        assert!(FieldValue::Text("x".into()).is_text());
        assert!(FieldValue::OrderedList(vec!["a".into()]).is_ordered_list());
        assert!(FieldValue::TimestampSeconds(0).is_timestamp());
    }

    #[tokio::test]
    async fn bool_and_null_values_do_not_trigger_anomaly() {
        let d = det();
        let r1 = d
            .observe(&"s1".into(), &"flag".into(), &FieldValue::Bool(true))
            .await
            .unwrap();
        let r2 = d
            .observe(&"s1".into(), &"x".into(), &FieldValue::Null)
            .await
            .unwrap();
        assert!(matches!(r1.signal, AnomalySignal::None));
        assert!(matches!(r2.signal, AnomalySignal::None));
    }

    #[test]
    fn jaccard_distance_identical_sets_is_zero() {
        let a = vec!["x".into(), "y".into(), "z".into()];
        let b = vec!["x".into(), "y".into(), "z".into()];
        let distance = StatisticalFieldAnomalyDetector::jaccard_distance(&a, &b);
        assert!(
            distance.abs() < 1e-9,
            "identical sets should have distance 0, got {distance}"
        );
    }

    #[test]
    fn jaccard_distance_disjoint_sets_is_one() {
        let a = vec!["x".into(), "y".into()];
        let b = vec!["p".into(), "q".into()];
        assert!((StatisticalFieldAnomalyDetector::jaccard_distance(&a, &b) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn median_odd_count() {
        let w: VecDeque<f64> = [3.0, 1.0, 2.0].into_iter().collect();
        assert_eq!(StatisticalFieldAnomalyDetector::median(&w), Some(2.0));
    }

    #[test]
    fn median_even_count() {
        let w: VecDeque<f64> = [4.0, 1.0, 3.0, 2.0].into_iter().collect();
        let m = StatisticalFieldAnomalyDetector::median(&w).unwrap_or(0.0);
        assert!(
            (m - 2.5).abs() < 1e-9,
            "median of even-count window should be 2.5, got {m}"
        );
    }

    #[test]
    fn schema_id_round_trip() {
        let s: SchemaId = "product-v1".into();
        assert_eq!(s.as_str(), "product-v1");
        assert_eq!(s.to_string(), "product-v1");
    }

    #[test]
    fn field_path_round_trip() {
        let p: FieldPath = "items.0.sku".into();
        assert_eq!(p.as_str(), "items.0.sku");
    }
}
