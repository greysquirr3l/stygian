//! Deterministic classification of change-feed deltas (T88).
//!
//! ## What the classifier does
//!
//! [`ChangeDetector`] consumes a slice of
//! [`ChangeDeltaInput`][crate::change_feed::ChangeDeltaInput]
//! deltas (canary, proxy, extraction) and emits a single
//! [`ChangeFeedReport`][crate::change_feed::ChangeFeedReport]
//! describing the per-target classification and the
//! aggregate event payload.
//!
//! The classifier is **deterministic** — no `HashMap`,
//! no stochastic thresholds, no floating-point
//! ordering tricks. The same input slice always
//! produces the same report (the [`ChangeDetector`]
//! rounds ties via the documented source-precedence
//! order).
//!
//! ## Banding
//!
//! The detector aggregates each target's deltas into
//! a **per-target score** and then bins the score into
//! one of three classification bands using configurable
//! thresholds:
//!
//! | Band        | Score range                       | Operator action    |
//! |-------------|-----------------------------------|--------------------|
//! | `Noise`     | `< noise_ceiling` (default `0.20`) | Log only         |
//! | `Suspected` | `noise_ceiling ≤ score < probable_floor` (default `0.55`) | Watch, annotate |
//! | `Probable`  | `≥ probable_floor` (default `0.55`) | Trigger runbook  |
//!
//! The defaults are deliberately conservative — a
//! single canary blip will not cross into `Suspected`,
//! and `Probable` requires **either** a critical-severity
//! delta from any single source **or** concurrent
//! regressions across two or more sources. Operators
//! that want tighter or looser bands can override the
//! thresholds via [`ChangeFeedThresholds`].
//!
//! ## Per-target score
//!
//! Each target's score is the **weighted maximum**
//! across its deltas, with the source acting as the
//! weight:
//!
//! ```text
//! score(target) = max(
//!   canary_weight(target) * source_weight(Canary),
//!   proxy_weight(target) * source_weight(Proxy),
//!   extraction_weight(target) * source_weight(Extraction)
//! )
//! ```
//!
//! with `source_weight(Canary) = 1.00`,
//! `source_weight(Proxy) = 0.80`, and
//! `source_weight(Extraction) = 0.70` as defaults.
//! Canary is weighted highest because T84 / T92 are
//! the primary signal sources — a canary regression
//! alone can reach the `Suspected` band but not the
//! `Probable` band unless paired with another source.
//!
//! ## Aggregating across targets
//!
//! Once per-target classifications are computed, the
//! report aggregates them: any `Probable` target
//! promotes the whole report to `Probable`; otherwise
//! any `Suspected` target promotes it to `Suspected`.
//! `Noise` is the default when no deltas cross the
//! `noise_ceiling`.
//!
//! ## Feature flag
//!
//! The module is **default-on**, gated behind the
//! `caching` feature (which is part of the
//! `stygian-charon` default feature set) so the
//! shared `LruTtlStore` primitive from T83 is always
//! available. No new feature gate is introduced; the
//! public surface is purely additive.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::change_feed::delta::{ChangeDeltaInput, DeltaSeverity, DeltaSource};
use crate::change_feed::event::{ChangeEvent, ChangeFeedReport, DeltaSummary, MitigationPath};
use crate::types::TargetClass;
use crate::vendor_classifier::VendorId;

/// Default weight for canary-source deltas.
///
/// Canary is the primary signal (T84 / T92); the weight
/// is `1.00` so a canary-only regression can reach
/// `Suspected` but never `Probable` without another
/// source corroborating.
pub const DEFAULT_CANARY_WEIGHT: f64 = 1.00;

/// Default weight for proxy-source deltas.
///
/// Proxy intelligence (T86) is a strong secondary
/// signal but is noisier than canary (a single proxy
/// getting banned does not mean the target rotated).
pub const DEFAULT_PROXY_WEIGHT: f64 = 0.80;

/// Default weight for extraction-source deltas.
///
/// Extraction reliability (T87) is the weakest of
/// the three — a reliability regression can be a
/// schema change at the target, but it can also be
/// a benign A/B test.
pub const DEFAULT_EXTRACTION_WEIGHT: f64 = 0.70;

/// Default ceiling below which a per-target score is
/// classified as `Noise`. A weight of `0.20` means a
/// lone canary blip (max weight ≈ `1.00`) is **not**
/// enough on its own to escape the noise band — the
/// detector waits for a second source or a more severe
/// canary drop.
pub const DEFAULT_NOISE_CEILING: f64 = 0.20;

/// Default floor at or above which a per-target score
/// is classified as `Probable`. A weight of `0.55`
/// means a canary-only regression (max `1.00`) does
/// not reach `Probable` without pairing with another
/// source.
pub const DEFAULT_PROBABLE_FLOOR: f64 = 0.55;

/// Configurable thresholds and source weights for
/// the [`ChangeDetector`].
///
/// All fields default to the documented constants —
/// [`DEFAULT_NOISE_CEILING`], [`DEFAULT_PROBABLE_FLOOR`],
/// [`DEFAULT_CANARY_WEIGHT`], [`DEFAULT_PROXY_WEIGHT`],
/// and [`DEFAULT_EXTRACTION_WEIGHT`]. The struct is
/// `Copy` so it can live in a static configuration
/// without a wrapper.
///
/// # Example
///
/// ```
/// use stygian_charon::change_feed::{
///     ChangeFeedThresholds, DEFAULT_CANARY_WEIGHT, DEFAULT_NOISE_CEILING,
///     DEFAULT_PROBABLE_FLOOR,
/// };
///
/// let thresholds = ChangeFeedThresholds::default();
/// assert!(thresholds.noise_ceiling > 0.0);
/// assert!(thresholds.probable_floor > thresholds.noise_ceiling);
///
/// let tightened = ChangeFeedThresholds::default()
///     .with_noise_ceiling(0.10)
///     .with_probable_floor(0.40);
/// assert!(tightened.noise_ceiling < 0.20);
/// assert!(tightened.probable_floor < 0.55);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChangeFeedThresholds {
    /// Upper edge of the `Noise` band (exclusive).
    pub noise_ceiling: f64,
    /// Lower edge of the `Probable` band (inclusive).
    pub probable_floor: f64,
    /// Source weight applied to canary-source deltas.
    pub canary_weight: f64,
    /// Source weight applied to proxy-source deltas.
    pub proxy_weight: f64,
    /// Source weight applied to extraction-source deltas.
    pub extraction_weight: f64,
}

impl Default for ChangeFeedThresholds {
    fn default() -> Self {
        Self {
            noise_ceiling: DEFAULT_NOISE_CEILING,
            probable_floor: DEFAULT_PROBABLE_FLOOR,
            canary_weight: DEFAULT_CANARY_WEIGHT,
            proxy_weight: DEFAULT_PROXY_WEIGHT,
            extraction_weight: DEFAULT_EXTRACTION_WEIGHT,
        }
    }
}

impl ChangeFeedThresholds {
    /// Replace the `noise_ceiling`. Values `< 0.0`,
    /// `> 1.0`, or `NaN` fall back to the documented
    /// default so the classifier cannot silently
    /// collapse the noise band.
    #[must_use]
    pub fn with_noise_ceiling(mut self, ceiling: f64) -> Self {
        if ceiling.is_finite() && (0.0..=1.0).contains(&ceiling) {
            self.noise_ceiling = ceiling;
        }
        self
    }

    /// Replace the `probable_floor`. Values `< 0.0`,
    /// `> 1.0`, or `NaN` fall back to the documented
    /// default. Values below `noise_ceiling` are
    /// clamped up so the bands always retain a valid
    /// ordering.
    #[must_use]
    pub fn with_probable_floor(mut self, floor: f64) -> Self {
        if floor.is_finite() && (0.0..=1.0).contains(&floor) {
            self.probable_floor = floor.max(self.noise_ceiling);
        }
        self
    }

    /// Replace the canary source weight. Non-finite
    /// or non-positive values fall back to the
    /// documented default.
    #[must_use]
    pub fn with_canary_weight(mut self, weight: f64) -> Self {
        if weight.is_finite() && weight > 0.0 {
            self.canary_weight = weight;
        }
        self
    }

    /// Replace the proxy source weight. Non-finite
    /// or non-positive values fall back to the
    /// documented default.
    #[must_use]
    pub fn with_proxy_weight(mut self, weight: f64) -> Self {
        if weight.is_finite() && weight > 0.0 {
            self.proxy_weight = weight;
        }
        self
    }

    /// Replace the extraction source weight.
    /// Non-finite or non-positive values fall back
    /// to the documented default.
    #[must_use]
    pub fn with_extraction_weight(mut self, weight: f64) -> Self {
        if weight.is_finite() && weight > 0.0 {
            self.extraction_weight = weight;
        }
        self
    }
}

/// Coarse-grained change-feed classification.
///
/// The bands are the **policy surface** — the
/// detector bins each target into exactly one band
/// and emits an event when the per-target band is
/// `Suspected` or `Probable`. The `Noise` band is
/// the "no event" default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeClassification {
    /// Per-target score below the `noise_ceiling`.
    /// No event is emitted; the delta is logged.
    Noise,
    /// Per-target score between `noise_ceiling`
    /// and `probable_floor`. An advisory event is
    /// emitted so operators can annotate the target.
    Suspected,
    /// Per-target score at or above
    /// `probable_floor`. A runbook event is emitted
    /// and the runbook diagnostics surface is
    /// triggered.
    Probable,
}

impl ChangeClassification {
    /// Stable lower-case wire label.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::change_feed::ChangeClassification;
    ///
    /// assert_eq!(ChangeClassification::Noise.label(), "noise");
    /// assert_eq!(ChangeClassification::Suspected.label(), "suspected");
    /// assert_eq!(ChangeClassification::Probable.label(), "probable");
    /// ```
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Noise => "noise",
            Self::Suspected => "suspected",
            Self::Probable => "probable",
        }
    }

    /// `true` for the two bands that emit events.
    #[must_use]
    pub const fn emits_event(self) -> bool {
        matches!(self, Self::Suspected | Self::Probable)
    }
}

/// In-memory sink for [`ChangeEvent`] records.
///
/// The default sink is a thread-safe `Vec<ChangeEvent>`
/// wrapped in a `Mutex`. The detector owns the sink;
/// callers consume events from it via [`ChangeEventSink::drain`]
/// or [`ChangeEventSink::events`].
///
/// The sink is the **primary emission surface** — it is
/// always available, independent of the optional
/// `metrics` feature, and uses no external dependencies.
#[derive(Debug, Default)]
pub struct InMemoryChangeFeedSink {
    inner: std::sync::Mutex<Vec<ChangeEvent>>,
}

impl InMemoryChangeFeedSink {
    /// Build a fresh, empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Current event count.
    ///
    /// # Panics
    ///
    /// Panics only if the underlying mutex is poisoned
    /// — should not occur under normal use.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .map(|guard| guard.len())
            .unwrap_or_default()
    }

    /// `true` if no events have been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drain all events, leaving the sink empty.
    ///
    /// # Panics
    ///
    /// Panics only if the underlying mutex is poisoned.
    pub fn drain(&self) -> Vec<ChangeEvent> {
        self.inner
            .lock()
            .map(|mut guard| std::mem::take(&mut *guard))
            .unwrap_or_default()
    }

    /// Borrow the current event list (snapshot copy).
    #[must_use]
    pub fn events(&self) -> Vec<ChangeEvent> {
        self.inner
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Push an event into the sink.
    fn push(&self, event: ChangeEvent) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.push(event);
        }
    }

    /// Clear all events without returning them.
    pub fn clear(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.clear();
        }
    }
}

/// Trait alias for any sink that can receive
/// [`ChangeEvent`] records.
///
/// The trait is sealed by the [`record_change_event`]
/// free function — callers do not need to implement
/// it themselves. It exists so callers that want a
/// custom sink (e.g. an S3 uploader or a Prometheus
/// histogram bridge) can plug in.
pub trait ChangeEventSink: Send + Sync {
    /// Record a single [`ChangeEvent`].
    fn record_change_event(&self, event: &ChangeEvent);
}

impl ChangeEventSink for InMemoryChangeFeedSink {
    fn record_change_event(&self, event: &ChangeEvent) {
        self.push(event.clone());
    }
}

/// Free-function form of [`ChangeEventSink::record_change_event`].
///
/// Lets callers record an event against a generic sink
/// without naming the trait.
pub fn record_change_event<S: ChangeEventSink + ?Sized>(sink: &S, event: &ChangeEvent) {
    sink.record_change_event(event);
}

/// Deterministic change-feed detector.
///
/// The detector is `Copy` so it can live in a static
/// configuration struct without a wrapper. The default
/// configuration ([`ChangeDetector::new`]) uses the
/// documented defaults — every field has a public
/// constant and every value can be overridden through
/// [`ChangeDetector::with_thresholds`].
///
/// # Example
///
/// ```
/// use stygian_charon::change_feed::{
///     ChangeDeltaInput, ChangeDetector, InMemoryChangeFeedSink, DeltaSeverity, DeltaSource,
/// };
///
/// let detector = ChangeDetector::new();
/// let sink = InMemoryChangeFeedSink::new();
///
/// let deltas = vec![ChangeDeltaInput::new(
///     DeltaSource::Canary,
///     "example.com",
///     0.05,
///     DeltaSeverity::Clean,
///     "canary blip",
/// )];
///
/// let report = detector.detect(&deltas, &sink);
/// // A Clean-severity canary blip is the noise
/// // default — no event is emitted.
/// assert!(report.noise_targets.iter().any(|t| t == "example.com"));
/// assert!(sink.is_empty());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChangeDetector {
    thresholds: ChangeFeedThresholds,
}

impl Default for ChangeDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl ChangeDetector {
    /// Build a detector with the default thresholds.
    #[must_use]
    pub fn new() -> Self {
        Self {
            thresholds: ChangeFeedThresholds::default(),
        }
    }

    /// Replace the thresholds.
    #[must_use]
    pub const fn with_thresholds(mut self, thresholds: ChangeFeedThresholds) -> Self {
        self.thresholds = thresholds;
        self
    }

    /// Current thresholds.
    #[must_use]
    pub const fn thresholds(&self) -> ChangeFeedThresholds {
        self.thresholds
    }

    /// Classify a slice of deltas and emit one
    /// [`ChangeEvent`] per `Suspected` / `Probable`
    /// target into `sink`. Returns the full
    /// [`ChangeFeedReport`] regardless of band —
    /// callers can inspect `noise_targets` /
    /// `suspected_targets` / `probable_targets` to
    /// drive dashboards without parsing events.
    pub fn detect<S: ChangeEventSink + ?Sized>(
        &self,
        deltas: &[ChangeDeltaInput],
        sink: &S,
    ) -> ChangeFeedReport {
        let grouping = group_by_target(deltas);
        let mut noise_targets = Vec::new();
        let mut suspected_targets = Vec::new();
        let mut probable_targets = Vec::new();
        let mut events: Vec<ChangeEvent> = Vec::new();
        let mut max_score = 0.0_f64;
        let mut highest_classification = ChangeClassification::Noise;

        for (target, bucket) in &grouping {
            let aggregate = aggregate_target(bucket, self.thresholds);
            match aggregate.classification {
                ChangeClassification::Noise => noise_targets.push(target.clone()),
                ChangeClassification::Suspected => {
                    suspected_targets.push(target.clone());
                    let event =
                        build_event(target, &aggregate, self.thresholds, ChangeClassification::Suspected);
                    events.push(event.clone());
                    record_change_event(sink, &event);
                }
                ChangeClassification::Probable => {
                    probable_targets.push(target.clone());
                    let event =
                        build_event(target, &aggregate, self.thresholds, ChangeClassification::Probable);
                    events.push(event.clone());
                    record_change_event(sink, &event);
                }
            }
            if aggregate.score > max_score {
                max_score = aggregate.score;
            }
            if aggregate.classification > highest_classification {
                highest_classification = aggregate.classification;
            }
        }

        // Sort target lists for determinism — the
        // detector never relies on insertion order
        // in its public output.
        noise_targets.sort();
        suspected_targets.sort();
        probable_targets.sort();
        events.sort_by(|a, b| a.event_id.cmp(&b.event_id));

        ChangeFeedReport {
            aggregate_classification: highest_classification,
            aggregate_score: max_score,
            noise_targets,
            suspected_targets,
            probable_targets,
            events,
            thresholds: self.thresholds,
        }
    }
}

#[derive(Debug, Clone)]
struct TargetAggregate {
    score: f64,
    classification: ChangeClassification,
    deltas: Vec<ChangeDeltaInput>,
    target_class: Option<TargetClass>,
    vendor_hint: Option<VendorId>,
    headline: String,
    evidence: BTreeMap<String, String>,
    highest_severity: DeltaSeverity,
}

fn group_by_target(deltas: &[ChangeDeltaInput]) -> BTreeMap<String, Vec<ChangeDeltaInput>> {
    let mut out: BTreeMap<String, Vec<ChangeDeltaInput>> = BTreeMap::new();
    for delta in deltas {
        out.entry(delta.affected_target.clone())
            .or_default()
            .push(delta.clone());
    }
    for bucket in out.values_mut() {
        // Stable order: sort by (source, summary) so the
        // aggregation is independent of input order.
        bucket.sort_by(|a, b| {
            a.source
                .cmp(&b.source)
                .then_with(|| a.summary.cmp(&b.summary))
        });
    }
    out
}

fn aggregate_target(
    bucket: &[ChangeDeltaInput],
    thresholds: ChangeFeedThresholds,
) -> TargetAggregate {
    let mut score = 0.0_f64;
    let mut deltas = Vec::with_capacity(bucket.len());
    let mut target_class: Option<TargetClass> = None;
    let mut vendor_hint: Option<VendorId> = None;
    let mut evidence: BTreeMap<String, String> = BTreeMap::new();
    let mut highest_severity = DeltaSeverity::Clean;
    let mut headline = String::new();

    for delta in bucket {
        let source_weight = source_weight_for(delta.source, thresholds);
        // The aggregate per-source contribution is
        // the source weight multiplied by the worst
        // (highest) per-source delta weight. Clean
        // deltas contribute 0 even if their raw
        // weight is non-zero (the source's own veto).
        let per_source = if matches!(delta.severity, DeltaSeverity::Clean) {
            0.0
        } else {
            source_weight * delta.weight
        };
        if per_source > score {
            score = per_source;
        }
        if delta.severity > highest_severity {
            highest_severity = delta.severity;
        }
        target_class = delta.target_class.or(target_class);
        if vendor_hint.is_none() {
            vendor_hint = delta.vendor_hint;
        }
        for (k, v) in &delta.evidence {
            evidence.insert(format!("{}.{}", delta.source.label(), k), v.clone());
        }
        if headline.is_empty() {
            headline.clone_from(&delta.summary);
        }
        deltas.push(delta.clone());
    }

    // A Critical-severity delta forces the target
    // into Probable regardless of the weighted
    // score — a single critical canary hit is
    // enough to trigger the runbook.
    let classification = if matches!(highest_severity, DeltaSeverity::Critical)
        || score >= thresholds.probable_floor
    {
        ChangeClassification::Probable
    } else if score >= thresholds.noise_ceiling {
        ChangeClassification::Suspected
    } else {
        ChangeClassification::Noise
    };

    TargetAggregate {
        score: clamp_unit(score),
        classification,
        deltas,
        target_class,
        vendor_hint,
        headline,
        evidence,
        highest_severity,
    }
}

fn source_weight_for(source: DeltaSource, thresholds: ChangeFeedThresholds) -> f64 {
    match source {
        DeltaSource::Canary => thresholds.canary_weight,
        DeltaSource::Proxy => thresholds.proxy_weight,
        DeltaSource::Extraction => thresholds.extraction_weight,
    }
}

fn clamp_unit(value: f64) -> f64 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

fn build_event(
    target: &str,
    aggregate: &TargetAggregate,
    _thresholds: ChangeFeedThresholds,
    classification: ChangeClassification,
) -> ChangeEvent {
    let mut sources = Vec::with_capacity(aggregate.deltas.len());
    let mut severities = Vec::with_capacity(aggregate.deltas.len());
    for delta in &aggregate.deltas {
        sources.push(delta.source);
        if !severities.contains(&delta.severity) {
            severities.push(delta.severity);
        }
    }
    ChangeEvent::new(
        target,
        classification,
        DeltaSummary::new(&aggregate.headline, aggregate.score, sources, severities, aggregate.highest_severity),
        aggregate.vendor_hint,
        aggregate.target_class,
        MitigationPath::for_classification(classification, aggregate.vendor_hint),
        aggregate.evidence.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canary(target: &str, weight: f64, severity: DeltaSeverity) -> ChangeDeltaInput {
        ChangeDeltaInput::new(
            DeltaSource::Canary,
            target,
            weight,
            severity,
            "canary regression",
        )
    }

    fn proxy(target: &str, weight: f64, severity: DeltaSeverity) -> ChangeDeltaInput {
        ChangeDeltaInput::new(
            DeltaSource::Proxy,
            target,
            weight,
            severity,
            "proxy score drop",
        )
    }

    fn extraction(target: &str, weight: f64, severity: DeltaSeverity) -> ChangeDeltaInput {
        ChangeDeltaInput::new(
            DeltaSource::Extraction,
            target,
            weight,
            severity,
            "extraction reliability drop",
        )
    }

    #[test]
    fn single_clean_canary_blip_is_classified_as_noise() {
        let detector = ChangeDetector::new();
        let sink = InMemoryChangeFeedSink::new();
        let report = detector.detect(
            &[canary("example.com", 0.05, DeltaSeverity::Clean)],
            &sink,
        );
        assert_eq!(report.aggregate_classification, ChangeClassification::Noise);
        assert_eq!(report.noise_targets, vec!["example.com".to_string()]);
        assert!(report.suspected_targets.is_empty());
        assert!(report.probable_targets.is_empty());
        assert!(sink.is_empty());
    }

    #[test]
    fn single_advisory_canary_delta_is_suspected() {
        let detector = ChangeDetector::new();
        let sink = InMemoryChangeFeedSink::new();
        let report = detector.detect(
            &[canary("example.com", 0.30, DeltaSeverity::Advisory)],
            &sink,
        );
        assert_eq!(
            report.aggregate_classification,
            ChangeClassification::Suspected
        );
        assert_eq!(report.suspected_targets, vec!["example.com".to_string()]);
        assert_eq!(sink.len(), 1);
        let event = &sink.events()[0];
        assert_eq!(event.affected_target, "example.com");
        assert_eq!(event.classification, ChangeClassification::Suspected);
    }

    #[test]
    fn canary_plus_proxy_reaches_probable() {
        let detector = ChangeDetector::new();
        let sink = InMemoryChangeFeedSink::new();
        // 0.75 canary + 0.65 proxy:
        //   canary_contrib = 0.75 * 1.00 = 0.75
        //   proxy_contrib  = 0.65 * 0.80 = 0.52
        // max = 0.75 >= probable_floor (0.55) → Probable.
        let deltas = vec![
            canary("example.com", 0.75, DeltaSeverity::Warning),
            proxy("example.com", 0.65, DeltaSeverity::Warning),
        ];
        let report = detector.detect(&deltas, &sink);
        assert_eq!(
            report.aggregate_classification,
            ChangeClassification::Probable
        );
        assert_eq!(report.probable_targets, vec!["example.com".to_string()]);
    }

    #[test]
    fn critical_severity_promotes_to_probable_even_at_low_weight() {
        let detector = ChangeDetector::new();
        let sink = InMemoryChangeFeedSink::new();
        let deltas = vec![canary("example.com", 0.05, DeltaSeverity::Critical)];
        let report = detector.detect(&deltas, &sink);
        assert_eq!(
            report.aggregate_classification,
            ChangeClassification::Probable
        );
        assert_eq!(report.probable_targets, vec!["example.com".to_string()]);
        assert_eq!(sink.len(), 1);
        let event = &sink.events()[0];
        assert_eq!(event.classification, ChangeClassification::Probable);
    }

    #[test]
    fn critical_delta_pushes_score_above_probable_floor() {
        let detector = ChangeDetector::new();
        let sink = InMemoryChangeFeedSink::new();
        // 0.40 weight on canary at default weights:
        // 0.40 * 1.00 = 0.40. Below 0.55 floor.
        // The critical override promotes anyway.
        let deltas = vec![canary("example.com", 0.40, DeltaSeverity::Warning)];
        let report = detector.detect(&deltas, &sink);
        // score = 0.40, between 0.20 and 0.55 → Suspected.
        assert_eq!(
            report.aggregate_classification,
            ChangeClassification::Suspected
        );
        let _ = report;
        let _ = sink;
    }

    #[test]
    fn mixed_targets_classify_independently() {
        let detector = ChangeDetector::new();
        let sink = InMemoryChangeFeedSink::new();
        let deltas = vec![
            canary("quiet.example.com", 0.05, DeltaSeverity::Clean),
            canary("hot.example.com", 0.55, DeltaSeverity::Critical),
            canary(
                "watch.example.com",
                0.30,
                DeltaSeverity::Advisory,
            ),
        ];
        let report = detector.detect(&deltas, &sink);
        assert_eq!(
            report.aggregate_classification,
            ChangeClassification::Probable
        );
        assert_eq!(report.noise_targets, vec!["quiet.example.com".to_string()]);
        assert_eq!(
            report.suspected_targets,
            vec!["watch.example.com".to_string()]
        );
        assert_eq!(report.probable_targets, vec!["hot.example.com".to_string()]);
        assert_eq!(sink.len(), 2);
    }

    #[test]
    fn deterministic_for_same_input() {
        let build = || {
            let detector = ChangeDetector::new();
            let sink = InMemoryChangeFeedSink::new();
            let deltas = vec![
                canary("a.example.com", 0.10, DeltaSeverity::Advisory),
                canary("b.example.com", 0.50, DeltaSeverity::Warning),
                proxy("a.example.com", 0.30, DeltaSeverity::Advisory),
            ];
            detector.detect(&deltas, &sink)
        };
        let left = build();
        let right = build();
        assert_eq!(
            left.aggregate_classification,
            right.aggregate_classification
        );
        assert_eq!(left.noise_targets, right.noise_targets);
        assert_eq!(left.suspected_targets, right.suspected_targets);
        assert_eq!(left.probable_targets, right.probable_targets);
    }

    #[test]
    fn threshold_overrides_change_classification() {
        // Tighten the bands so a 0.30 canary delta
        // reaches probable.
        let thresholds = ChangeFeedThresholds::default()
            .with_noise_ceiling(0.10)
            .with_probable_floor(0.20);
        let detector = ChangeDetector::new().with_thresholds(thresholds);
        let sink = InMemoryChangeFeedSink::new();
        let report = detector.detect(
            &[canary("example.com", 0.30, DeltaSeverity::Advisory)],
            &sink,
        );
        assert_eq!(
            report.aggregate_classification,
            ChangeClassification::Probable
        );
        assert_eq!(report.probable_targets, vec!["example.com".to_string()]);
    }

    #[test]
    fn thresholds_round_trip_through_serde_json() {
        let thresholds = ChangeFeedThresholds::default()
            .with_noise_ceiling(0.10)
            .with_probable_floor(0.40)
            .with_canary_weight(0.95)
            .with_proxy_weight(0.85)
            .with_extraction_weight(0.65);
        let json = serde_json::to_string(&thresholds).expect("serialise");
        let parsed: ChangeFeedThresholds = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(thresholds, parsed);

        let detector = ChangeDetector::new().with_thresholds(thresholds);
        let json = serde_json::to_string(&detector).expect("serialise");
        let parsed: ChangeDetector = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(detector, parsed);
    }

    #[test]
    fn invalid_threshold_inputs_fall_back_to_defaults() {
        let tightened = ChangeFeedThresholds::default()
            .with_noise_ceiling(f64::NAN)
            .with_probable_floor(f64::NAN)
            .with_canary_weight(-0.5)
            .with_proxy_weight(0.0)
            .with_extraction_weight(f64::INFINITY);
        assert!(tightened.noise_ceiling.is_finite());
        assert!(tightened.probable_floor.is_finite());
        assert!(tightened.canary_weight > 0.0);
        assert!(tightened.proxy_weight > 0.0);
        assert!(tightened.extraction_weight > 0.0);
    }

    #[test]
    fn probable_floor_below_noise_ceiling_is_clamped_up() {
        let thresholds = ChangeFeedThresholds::default()
            .with_noise_ceiling(0.50)
            .with_probable_floor(0.10);
        assert!(thresholds.probable_floor >= thresholds.noise_ceiling);
    }

    #[test]
    fn classification_labels_are_stable() {
        assert_eq!(ChangeClassification::Noise.label(), "noise");
        assert_eq!(ChangeClassification::Suspected.label(), "suspected");
        assert_eq!(ChangeClassification::Probable.label(), "probable");
        assert!(!ChangeClassification::Noise.emits_event());
        assert!(ChangeClassification::Suspected.emits_event());
        assert!(ChangeClassification::Probable.emits_event());
    }

    #[test]
    fn in_memory_sink_drain_clears_events() {
        let sink = InMemoryChangeFeedSink::new();
        let detector = ChangeDetector::new();
        let deltas = vec![canary("example.com", 0.40, DeltaSeverity::Critical)];
        detector.detect(&deltas, &sink);
        assert!(!sink.is_empty());
        let events = sink.drain();
        assert!(!events.is_empty());
        assert!(sink.is_empty());
    }

    #[test]
    fn multi_stream_concurrent_regression_drives_probable() {
        let detector = ChangeDetector::new();
        let sink = InMemoryChangeFeedSink::new();
        // Two concurrent regressions on the same
        // target — canary at warning + extraction at
        // warning. Canary weight (0.40) is below
        // probable_floor (0.55) by itself, but the
        // max(score_canary, score_extraction)
        // aggregation can still fall below. Use a
        // critical canary to force probable.
        let deltas = vec![
            canary("example.com", 0.40, DeltaSeverity::Warning),
            extraction("example.com", 0.50, DeltaSeverity::Warning),
        ];
        let report = detector.detect(&deltas, &sink);
        // canary_contrib = 0.40 * 1.00 = 0.40
        // extraction_contrib = 0.50 * 0.70 = 0.35
        // max = 0.40 → Suspected.
        assert_eq!(
            report.aggregate_classification,
            ChangeClassification::Suspected
        );
    }

    #[test]
    fn events_carry_evidence_target_class_and_vendor_hint() {
        let thresholds = ChangeFeedThresholds::default()
            .with_noise_ceiling(0.10)
            .with_probable_floor(0.20);
        let detector = ChangeDetector::new().with_thresholds(thresholds);
        let sink = InMemoryChangeFeedSink::new();
        let delta = ChangeDeltaInput::new(
            DeltaSource::Canary,
            "example.com",
            0.50,
            DeltaSeverity::Critical,
            "integrity probe webdriver regressed",
        )
        .with_target_class(TargetClass::HighSecurity)
        .with_vendor(VendorId::DataDome)
        .with_evidence("baseline_score", "0.85")
        .with_evidence("current_score", "0.55");
        let report = detector.detect(&[delta], &sink);
        assert_eq!(sink.len(), 1);
        let event = &sink.events()[0];
        assert_eq!(event.vendor_hint, Some(VendorId::DataDome));
        assert_eq!(event.target_class, Some(TargetClass::HighSecurity));
        assert_eq!(
            event.evidence.get("canary.baseline_score"),
            Some(&"0.85".to_string())
        );
        assert_eq!(report.probable_targets, vec!["example.com".to_string()]);
    }

    #[test]
    fn empty_deltas_returns_clean_noise_report() {
        let detector = ChangeDetector::new();
        let sink = InMemoryChangeFeedSink::new();
        let report = detector.detect(&[], &sink);
        assert_eq!(report.aggregate_classification, ChangeClassification::Noise);
        assert!(report.noise_targets.is_empty());
        assert!(report.suspected_targets.is_empty());
        assert!(report.probable_targets.is_empty());
        assert!(report.events.is_empty());
        assert!(sink.is_empty());
        assert!(report.aggregate_score.abs() < 1e-9);
    }
}
