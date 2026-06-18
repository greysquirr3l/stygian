//! Anti-bot change-detection feed (T88).
//!
//! ## What this module does
//!
//! Anti-bot vendors rotate wall-logic, escalate
//! challenges, and rewrite challenge JS without
//! notice. The change feed is the **early-warning**
//! surface: it consumes regression signals from the
//! canary (T92 + T84), proxy intelligence (T86), and
//! extraction reliability (T87) pipelines and emits
//! actionable incident packets when the signals
//! agree.
//!
//! The feed is **deterministic** — no `HashMap`,
//! no stochastic thresholds, no ML. The same input
//! always produces the same classification, so
//! downstream runbook tooling can dedupe and audit
//! without trusting the call site.
//!
//! ## Detection flow
//!
//! ```text
//! ChangeDeltaInput[]                       (boundary type)
//!      │
//!      ▼
//! ChangeDetector::detect()                 (this module)
//!      │
//!      ├── per-target aggregate
//!      │     - max(weight * source_weight)
//!      │     - banded via ChangeFeedThresholds
//!      │
//!      ├── ChangeFeedReport                (always returned)
//!      │
//!      └── ChangeEventSink::record_change_event
//!            - InMemoryChangeFeedSink       (always available)
//!            - MetricsCollector            (with `metrics` feature)
//! ```
//!
//! ## Classification
//!
//! | Band        | Default score range       | What the operator sees |
//! |-------------|---------------------------|------------------------|
//! | `Noise`     | `< 0.20`                  | Log only, no event     |
//! | `Suspected` | `[0.20, 0.55)`            | Advisory event         |
//! | `Probable`  | `≥ 0.55` or critical tier | Runbook event          |
//!
//! A single canary delta cannot reach `Probable`
//! on its own — the default `canary_weight` is
//! `1.00`, so a delta with weight `0.55` reaches the
//! floor only when paired with another source or
//! marked `Critical` by the upstream signal.
//!
//! ## Schema
//!
//! The [`ChangeEvent`] payload is the **operator-facing**
//! view of the regression. It carries the affected
//! target, the delta summary (headline + score +
//! sources + severities), the vendor hint, the
//! target class, the runbook mitigation pointer, and
//! the structured evidence preserved from the
//! upstream deltas. [`ChangeFeedReport`] is the
//! per-cycle aggregate that the runbook diagnostics
//! surface consumes.
//!
//! ## Metrics surface
//!
//! When the `metrics` feature is enabled, the
//! `MetricsCollector` (see `crate::metrics::MetricsCollector`; only compiled
//! with `--features metrics`)
//! implements [`ChangeEventSink`] so the detector
//! records events directly into the existing
//! Prometheus exporter. The `change_feed_*` series
//! are only emitted when at least one counter is
//! non-zero, so dashboards that have not wired the
//! change feed in keep their existing layout
//! unchanged.
//!
//! ## Feature flag
//!
//! The module is **default-on** (gated behind the
//! `caching` feature, which is part of the
//! `stygian-charon` default feature set). No new
//! feature gate is introduced — the public surface
//! is purely additive and existing serialisers see
//! no change unless they explicitly opt in via the
//! new fields on [`ChangeFeedReport`] /
//! [`ChangeEvent`].
//!
//! # Example
//!
//! ```
//! use stygian_charon::change_feed::{
//!     ChangeDeltaInput, ChangeDetector, InMemoryChangeFeedSink, DeltaSeverity, DeltaSource,
//! };
//!
//! let detector = ChangeDetector::new();
//! let sink = InMemoryChangeFeedSink::new();
//!
//! // Two deltas on the same target — a canary
//! // warning plus a proxy advisory. Neither
//! // alone is enough for `Probable`, but the
//! // canary marks itself `Critical`.
//! let deltas = vec![
//!     ChangeDeltaInput::new(
//!         DeltaSource::Canary,
//!         "example.com",
//!         0.40,
//!         DeltaSeverity::Critical,
//!         "integrity probe webdriver regressed",
//!     ),
//!     ChangeDeltaInput::new(
//!         DeltaSource::Proxy,
//!         "example.com",
//!         0.50,
//!         DeltaSeverity::Warning,
//!         "proxy score dropped",
//!     ),
//! ];
//!
//! let report = detector.detect(&deltas, &sink);
//! assert_eq!(report.aggregate_classification, stygian_charon::change_feed::ChangeClassification::Probable);
//! assert_eq!(report.probable_targets, vec!["example.com".to_string()]);
//! assert_eq!(sink.len(), 1);
//! ```

mod classification;
mod delta;
mod event;

pub use classification::{
    ChangeClassification, ChangeDetector, ChangeEventSink, ChangeFeedThresholds,
    DEFAULT_CANARY_WEIGHT, DEFAULT_EXTRACTION_WEIGHT, DEFAULT_NOISE_CEILING,
    DEFAULT_PROBABLE_FLOOR, DEFAULT_PROXY_WEIGHT, InMemoryChangeFeedSink, record_change_event,
};
pub use delta::{ChangeDeltaInput, DeltaSeverity, DeltaSource};
pub use event::{ChangeEvent, ChangeFeedReport, DeltaSummary, MitigationPath};

// Wire the metrics surface into the change-feed
// sink when the optional `metrics` feature is
// enabled. The MetricsCollector implements
// `ChangeEventSink` so the detector can record
// events into the existing Prometheus exporter
// without any extra glue.
#[cfg(feature = "metrics")]
impl ChangeEventSink for crate::metrics::MetricsCollector {
    fn record_change_event(&self, event: &ChangeEvent) {
        Self::record_change_event(self, event);
    }
}
