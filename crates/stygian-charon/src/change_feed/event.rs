//! Change-feed event payloads (T88).
//!
//! Every [`ChangeEvent`] the detector emits carries:
//!
//! - **affected targets** — the domain(s) the event
//!   applies to (one event per target, per detection
//!   cycle).
//! - **delta summary** — the headline + score +
//!   contributing sources + severities, so the
//!   runbook consumer can render the event without
//!   re-running the classifier.
//! - **recommended mitigation path** — a stable
//!   pointer to the runbook section + a one-line
//!   hint the operator can act on.
//!
//! [`ChangeFeedReport`] aggregates the per-target
//! events into a single, serialisable structure that
//! the runbook diagnostics surface consumes. The
//! schema is **additive** — older serialisers ignore
//! fields they do not know.
//!
//! # Wire format
//!
//! ```text
//! {
//!   "aggregate_classification": "probable",
//!   "aggregate_score": 0.81,
//!   "noise_targets": ["quiet.example.com"],
//!   "suspected_targets": ["watch.example.com"],
//!   "probable_targets": ["hot.example.com"],
//!   "events": [
//!     {
//!       "event_id": "cf-<unix-secs>-hot.example.com",
//!       "detected_at_unix_secs": 1718616000,
//!       "affected_target": "hot.example.com",
//!       "classification": "probable",
//!       "delta_summary": {
//!         "headline": "integrity probe webdriver regressed",
//!         "score": 0.81,
//!         "sources": ["canary"],
//!         "severities": ["critical"],
//!         "highest_severity": "critical"
//!       },
//!       "vendor_hint": "datadome",
//!       "target_class": "high_security",
//!       "recommended_mitigation_path": {
//!         "runbook_section": "category-a-fingerprint-identity-regression",
//!         "hint": "apply browser+sticky escalation",
//!         "url": "docs/incident-runbook.md#category-a-fingerprintidentity-regression"
//!       },
//!       "evidence": { "canary.baseline_score": "0.85" }
//!     }
//!   ],
//!   "thresholds": {
//!     "noise_ceiling": 0.20,
//!     "probable_floor": 0.55,
//!     "canary_weight": 1.00,
//!     "proxy_weight": 0.80,
//!     "extraction_weight": 0.70
//!   }
//! }
//! ```
//!
//! ## Determinism
//!
//! The `event_id` is a stable composite of
//! `cf-<detected_at_unix_secs>-<affected_target>`
//! so downstream tooling can dedupe by event ID
//! without depending on the order deltas were
//! received in.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::change_feed::classification::{ChangeClassification, ChangeFeedThresholds};
use crate::change_feed::delta::{DeltaSeverity, DeltaSource};
use crate::types::TargetClass;
use crate::vendor_classifier::VendorId;

/// Stable pointer to the runbook section an
/// operator should consult when responding to a
/// [`ChangeEvent`].
///
/// The [`path`][Self::path] field is the
/// canonical, kebab-case identifier; the
/// [`hint`][Self::hint] is a short human-readable
/// action the operator can take immediately; the
/// [`url`][Self::url] is the relative path into
/// the crate's runbook docs.
///
/// # Example
///
/// ```
/// use stygian_charon::change_feed::{
///     ChangeClassification, MitigationPath,
/// };
/// use stygian_charon::vendor_classifier::VendorId;
///
/// let path = MitigationPath::for_classification(
///     ChangeClassification::Probable,
///     Some(VendorId::DataDome),
/// );
/// assert!(path.path.starts_with("category-"));
/// assert!(path.url.contains("incident-runbook.md"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MitigationPath {
    /// Stable, kebab-case runbook section identifier.
    pub path: String,
    /// One-line actionable hint for the operator.
    pub hint: String,
    /// Relative path into the runbook docs (e.g.
    /// `docs/incident-runbook.md#category-a-fingerprintidentity-regression`).
    pub url: String,
}

impl MitigationPath {
    /// Pick a mitigation path from the classification
    /// band and (optional) vendor hint.
    ///
    /// The mapping is:
    ///
    /// | Classification | Vendor hint                | Runbook section                  |
    /// |----------------|----------------------------|----------------------------------|
    /// | `Suspected`    | any / none                 | `category-a-fingerprint-identity-regression` |
    /// | `Probable`     | `DataDome`                 | `category-a-fingerprint-identity-regression` |
    /// | `Probable`     | `PerimeterX` / `Akamai`    | `category-b-rate-limit-backoff-regression` |
    /// | `Probable`     | `Cloudflare`               | `category-b-rate-limit-backoff-regression` |
    /// | `Probable`     | other / none               | `category-b-rate-limit-backoff-regression` |
    /// | `Noise`        | (event not emitted)        | n/a                              |
    ///
    /// The mapping mirrors the existing runbook
    /// categories in
    /// `crates/stygian-charon/docs/incident-runbook.md`:
    /// fingerprint/identity regressions are
    /// Category A; rate-limit / backoff / proxy
    /// regressions are Category B; the rest fall
    /// back to Category B as the safest operator
    /// action.
    #[must_use]
    pub fn for_classification(
        classification: ChangeClassification,
        vendor: Option<VendorId>,
    ) -> Self {
        match classification {
            ChangeClassification::Noise => Self {
                path: "no-action".to_string(),
                hint: "no action — delta scored below noise ceiling".to_string(),
                url: "docs/incident-runbook.md".to_string(),
            },
            ChangeClassification::Suspected => Self {
                path: "category-a-fingerprint-identity-regression".to_string(),
                hint: "annotate target, watch canary trend".to_string(),
                url: "docs/incident-runbook.md#category-afingerprintidentity-regression"
                    .to_string(),
            },
            ChangeClassification::Probable => match vendor {
                Some(VendorId::DataDome) => Self {
                    path: "category-a-fingerprint-identity-regression".to_string(),
                    hint: "apply browser+sticky escalation, refresh fingerprint profile"
                        .to_string(),
                    url: "docs/incident-runbook.md#category-afingerprintidentity-regression"
                        .to_string(),
                },
                Some(VendorId::PerimeterX | VendorId::Akamai | VendorId::Imperva) => Self {
                    path: "category-b-rate-limit-backoff-regression".to_string(),
                    hint: "rotate proxy pool, increase backoff".to_string(),
                    url: "docs/incident-runbook.md#category-b-rate-limiting-backoff-regression"
                        .to_string(),
                },
                Some(VendorId::Cloudflare) => Self {
                    path: "category-b-rate-limit-backoff-regression".to_string(),
                    hint: "verify cf-clearance flow, check UA/browser coherence".to_string(),
                    url: "docs/incident-runbook.md#category-b-rate-limiting-backoff-regression"
                        .to_string(),
                },
                _ => Self {
                    path: "category-b-rate-limit-backoff-regression".to_string(),
                    hint: "rotate proxy pool, slow pacing, escalate per runbook".to_string(),
                    url: "docs/incident-runbook.md#category-b-rate-limiting-backoff-regression"
                        .to_string(),
                },
            },
        }
    }
}

/// Per-event delta summary.
///
/// The summary is the **operator-facing view** of
/// the regression. The headline is a one-line
/// description; the score is the per-target score
/// the classifier assigned; `sources` and
/// `severities` list the contributing channels
/// (sorted for determinism); `highest_severity`
/// is the worst severity tier across the deltas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeltaSummary {
    /// One-line headline describing the regression.
    pub headline: String,
    /// Per-target score the classifier assigned,
    /// in `[0.0, 1.0]`.
    pub score: f64,
    /// Source channels that contributed deltas.
    /// Sorted for determinism.
    pub sources: Vec<DeltaSource>,
    /// Distinct severity tiers the contributing
    /// deltas attached. Sorted for determinism.
    pub severities: Vec<DeltaSeverity>,
    /// Highest severity tier across the
    /// contributing deltas.
    pub highest_severity: DeltaSeverity,
}

impl DeltaSummary {
    /// Build a summary from the per-target aggregate.
    /// `sources` and `severities` are deduplicated
    /// and sorted so the wire form is deterministic.
    #[must_use]
    pub fn new(
        headline: impl Into<String>,
        score: f64,
        sources: Vec<DeltaSource>,
        severities: Vec<DeltaSeverity>,
        highest_severity: DeltaSeverity,
    ) -> Self {
        let mut sources = sources;
        sources.sort();
        sources.dedup();
        let mut severities = severities;
        severities.sort();
        severities.dedup();
        Self {
            headline: headline.into(),
            score: sanitise_score(score),
            sources,
            severities,
            highest_severity,
        }
    }
}

const fn sanitise_score(score: f64) -> f64 {
    if score.is_nan() {
        0.0
    } else {
        score.clamp(0.0, 1.0)
    }
}

/// A single change-feed event.
///
/// One [`ChangeEvent`] is emitted per `Suspected`
/// / `Probable` target per detection cycle. The
/// `event_id` is stable so downstream tooling can
/// dedupe by ID without depending on order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeEvent {
    /// Stable event identifier —
    /// `cf-<detected_at_unix_secs>-<affected_target>`.
    pub event_id: String,
    /// Wall-clock timestamp the event was assembled.
    pub detected_at_unix_secs: u64,
    /// Target the event applies to (domain).
    pub affected_target: String,
    /// Classification band the detector assigned.
    pub classification: ChangeClassification,
    /// Per-target delta summary.
    pub delta_summary: DeltaSummary,
    /// Optional vendor hint preserved from the
    /// upstream deltas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_hint: Option<VendorId>,
    /// Optional target class preserved from the
    /// upstream deltas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_class: Option<TargetClass>,
    /// Runbook mitigation pointer.
    pub recommended_mitigation_path: MitigationPath,
    /// Structured evidence preserved verbatim from
    /// the upstream deltas. Keys are namespaced
    /// by source (e.g. `canary.baseline_score`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub evidence: BTreeMap<String, String>,
}

impl ChangeEvent {
    /// Build a new event. The `event_id` is generated
    /// deterministically from
    /// `cf-<detected_at_unix_secs>-<affected_target>`
    /// so downstream consumers can dedupe without
    /// trusting insertion order.
    #[must_use]
    pub fn new(
        affected_target: impl Into<String>,
        classification: ChangeClassification,
        delta_summary: DeltaSummary,
        vendor_hint: Option<VendorId>,
        target_class: Option<TargetClass>,
        recommended_mitigation_path: MitigationPath,
        evidence: BTreeMap<String, String>,
    ) -> Self {
        let target = affected_target.into();
        let event_id = format!(
            "cf-{}-{}",
            unix_timestamp_secs(),
            sanitize_segment(&target)
        );
        Self {
            event_id,
            detected_at_unix_secs: unix_timestamp_secs(),
            affected_target: target,
            classification,
            delta_summary,
            vendor_hint,
            target_class,
            recommended_mitigation_path,
            evidence,
        }
    }

    /// Build a new event with an explicit wall-clock
    /// timestamp. Useful for deterministic tests
    /// and for callers that hold their own clock.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new_at(
        detected_at_unix_secs: u64,
        affected_target: impl Into<String>,
        classification: ChangeClassification,
        delta_summary: DeltaSummary,
        vendor_hint: Option<VendorId>,
        target_class: Option<TargetClass>,
        recommended_mitigation_path: MitigationPath,
        evidence: BTreeMap<String, String>,
    ) -> Self {
        let target = affected_target.into();
        let event_id = format!(
            "cf-{}-{}",
            detected_at_unix_secs,
            sanitize_segment(&target)
        );
        Self {
            event_id,
            detected_at_unix_secs,
            affected_target: target,
            classification,
            delta_summary,
            vendor_hint,
            target_class,
            recommended_mitigation_path,
            evidence,
        }
    }
}

fn unix_timestamp_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn sanitize_segment(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Aggregated change-feed report for a single
/// detection cycle.
///
/// The report carries:
/// - the aggregate classification (the worst
///   per-target band);
/// - the per-target lists grouped by band;
/// - the emitted [`ChangeEvent`] records;
/// - the threshold configuration the detector
///   used (so downstream consumers can audit the
///   banding decision without consulting the
///   detector config separately).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeFeedReport {
    /// Worst per-target band across the cycle.
    pub aggregate_classification: ChangeClassification,
    /// Highest per-target score across the cycle.
    pub aggregate_score: f64,
    /// Targets scored below `noise_ceiling`.
    pub noise_targets: Vec<String>,
    /// Targets scored between `noise_ceiling` and
    /// `probable_floor`.
    pub suspected_targets: Vec<String>,
    /// Targets scored at or above `probable_floor`.
    pub probable_targets: Vec<String>,
    /// Emitted events, one per `Suspected` /
    /// `Probable` target.
    pub events: Vec<ChangeEvent>,
    /// Thresholds the detector used for this cycle.
    pub thresholds: ChangeFeedThresholds,
}

impl ChangeFeedReport {
    /// Whether the report contains any events that
    /// should be surfaced to operators.
    #[must_use]
    pub const fn has_actionable_events(&self) -> bool {
        !self.events.is_empty()
    }

    /// Total target count (noise + suspected +
    /// probable).
    #[must_use]
    pub const fn target_count(&self) -> usize {
        self.noise_targets.len() + self.suspected_targets.len() + self.probable_targets.len()
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn summary() -> DeltaSummary {
        DeltaSummary::new(
            "integrity probe webdriver regressed",
            0.40,
            vec![DeltaSource::Canary],
            vec![DeltaSeverity::Advisory],
            DeltaSeverity::Advisory,
        )
    }

    fn path() -> MitigationPath {
        MitigationPath::for_classification(ChangeClassification::Suspected, None)
    }

    #[test]
    fn event_id_is_stable_composite() {
        let event = ChangeEvent::new_at(
            1_718_616_000,
            "example.com",
            ChangeClassification::Suspected,
            summary(),
            None,
            None,
            path(),
            BTreeMap::new(),
        );
        assert_eq!(event.event_id, "cf-1718616000-example.com");
    }

    #[test]
    fn event_id_sanitises_non_alphanumeric_target() {
        let event = ChangeEvent::new_at(
            1_718_616_000,
            "weird host.example.com/path",
            ChangeClassification::Suspected,
            summary(),
            None,
            None,
            path(),
            BTreeMap::new(),
        );
        assert!(event.event_id.starts_with("cf-1718616000-"));
        // Spaces and slashes get replaced with
        // underscores; alphanumerics survive.
        assert!(!event.event_id.contains(' '));
        assert!(!event.event_id.contains('/'));
    }

    #[test]
    fn event_round_trips_through_serde_json() {
        let event = ChangeEvent::new_at(
            1_718_616_000,
            "example.com",
            ChangeClassification::Probable,
            summary(),
            Some(VendorId::DataDome),
            Some(TargetClass::HighSecurity),
            path(),
            BTreeMap::new(),
        );
        let json = serde_json::to_string(&event).expect("serialise");
        let parsed: ChangeEvent = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(event, parsed);
    }

    #[test]
    fn delta_summary_dedupes_sources_and_severities() {
        let summary = DeltaSummary::new(
            "multi-source regression",
            0.50,
            vec![DeltaSource::Canary, DeltaSource::Canary, DeltaSource::Proxy],
            vec![
                DeltaSeverity::Advisory,
                DeltaSeverity::Advisory,
                DeltaSeverity::Warning,
            ],
            DeltaSeverity::Warning,
        );
        assert_eq!(summary.sources, vec![DeltaSource::Canary, DeltaSource::Proxy]);
        assert_eq!(
            summary.severities,
            vec![DeltaSeverity::Advisory, DeltaSeverity::Warning]
        );
    }

    #[test]
    fn delta_summary_clamps_score_and_nan() {
        let summary = DeltaSummary::new(
            "score bounds",
            f64::NAN,
            vec![DeltaSource::Canary],
            vec![DeltaSeverity::Advisory],
            DeltaSeverity::Advisory,
        );
        assert!(summary.score.abs() < 1e-9);
        let summary = DeltaSummary::new(
            "score bounds",
            1.5,
            vec![DeltaSource::Canary],
            vec![DeltaSeverity::Advisory],
            DeltaSeverity::Advisory,
        );
        assert!((summary.score - 1.0).abs() < 1e-9);
    }

    #[test]
    fn mitigation_path_picks_category_a_for_datadome() {
        let path = MitigationPath::for_classification(
            ChangeClassification::Probable,
            Some(VendorId::DataDome),
        );
        assert!(path.path.starts_with("category-a"));
        assert!(path.url.contains("incident-runbook.md"));
    }

    #[test]
    fn mitigation_path_picks_category_b_for_akamai() {
        let path = MitigationPath::for_classification(
            ChangeClassification::Probable,
            Some(VendorId::Akamai),
        );
        assert!(path.path.starts_with("category-b"));
    }

    #[test]
    fn mitigation_path_picks_category_b_for_unknown() {
        let path = MitigationPath::for_classification(ChangeClassification::Probable, None);
        assert!(path.path.starts_with("category-b"));
    }

    #[test]
    fn mitigation_path_suspected_always_uses_category_a() {
        for vendor in [
            None,
            Some(VendorId::DataDome),
            Some(VendorId::Cloudflare),
            Some(VendorId::Akamai),
        ] {
            let path =
                MitigationPath::for_classification(ChangeClassification::Suspected, vendor);
            assert!(
                path.path.starts_with("category-a"),
                "suspected band should pick category-a regardless of vendor"
            );
        }
    }

    #[test]
    fn report_target_count_sums_bands() {
        let report = ChangeFeedReport {
            aggregate_classification: ChangeClassification::Suspected,
            aggregate_score: 0.40,
            noise_targets: vec!["a".to_string(), "b".to_string()],
            suspected_targets: vec!["c".to_string()],
            probable_targets: vec!["d".to_string()],
            events: Vec::new(),
            thresholds: ChangeFeedThresholds::default(),
        };
        assert_eq!(report.target_count(), 4);
        assert!(!report.has_actionable_events());
    }
}
