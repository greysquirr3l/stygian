//! Change-feed delta inputs (T88).
//!
//! The change detector consumes three families of regression
//! signal — canary (T92 integrity probes + T84 trend),
//! proxy intelligence (T86), and extraction reliability
//! (T87). Each family contributes a single
//! [`ChangeDeltaInput`] describing the **delta** the
//! detector should weigh.
//!
//! ## Why abstract deltas?
//!
//! The canary probe pack lives in `stygian-browser`, the
//! proxy intelligence store lives in `stygian-proxy`, and
//! the extraction reliability scorer lives in
//! `stygian-plugin`. None of those crates may be a build
//! dependency of `stygian-charon` (the `change_feed` module
//! sits in `stygian-charon`). To keep the layering clean,
//! callers convert their upstream reports into the
//! delta types defined here at the boundary. The detector
//! never reaches back into the source crates — the input
//! is a flat, serialisable, `Copy`-friendly record.
//!
//! ## Delta fields
//!
//! Each delta carries:
//! - a **weight** in `[0.0, 1.0]` expressing how alarming
//!   the source signal thinks the regression is
//!   (`0.0` = perfectly clean, `1.0` = worst-case), and
//! - an **affected target** (a domain string) so the
//!   emitted event can be routed back to the runbook
//!   without an extra lookup.
//!
//! Optional fields let callers attach richer context
//! (vendor hint, target class, severity tag) without
//! forcing every input to populate them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::types::TargetClass;
use crate::vendor_classifier::VendorId;

/// Severity tier attached to a delta by its source.
///
/// The tier is a **coarse** label — the deterministic
/// weight field is the value the classifier actually
/// quantises against. The tier is preserved on the emitted
/// [`ChangeEvent`][crate::change_feed::ChangeEvent] so
/// downstream runbook consumers do not have to invert the
/// weight to recover the source's intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeltaSeverity {
    /// Source signal is clean / below the documented
    /// floor; nothing to act on.
    Clean,
    /// Source signal is in the advisory band; the source
    /// crate recommends watching the target.
    Advisory,
    /// Source signal is in the warning band; the source
    /// crate recommends an active response.
    Warning,
    /// Source signal is in the critical band; the source
    /// crate has triggered its own hard-gate or emergency
    /// path.
    Critical,
}

impl DeltaSeverity {
    /// Stable lower-case wire label.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::change_feed::DeltaSeverity;
    ///
    /// assert_eq!(DeltaSeverity::Advisory.label(), "advisory");
    /// assert_eq!(DeltaSeverity::Critical.label(), "critical");
    /// ```
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Advisory => "advisory",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

/// Source channel for a [`ChangeDeltaInput`].
///
/// The discriminant order is part of the deterministic
/// tie-break rule used when two deltas share the same
/// affected target. Lower discriminant wins, so
/// `Canary` is consulted before `Proxy`, which is
/// consulted before `Extraction`. This matches the
/// "canary is the primary signal" precedence implied by
/// T84 / T92.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeltaSource {
    /// Canary regression — JS integrity probe (T92)
    /// score drop, T84 trend regression, or both.
    Canary,
    /// Proxy intelligence score regression (T86).
    Proxy,
    /// Extraction reliability regression (T87).
    Extraction,
}

impl DeltaSource {
    /// Stable lower-case wire label.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::change_feed::DeltaSource;
    ///
    /// assert_eq!(DeltaSource::Canary.label(), "canary");
    /// assert_eq!(DeltaSource::Proxy.label(), "proxy");
    /// assert_eq!(DeltaSource::Extraction.label(), "extraction");
    /// ```
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Canary => "canary",
            Self::Proxy => "proxy",
            Self::Extraction => "extraction",
        }
    }
}

/// A single regression delta consumed by the
/// [`ChangeDetector`][crate::change_feed::ChangeDetector].
///
/// The delta is the **boundary type** between the
/// upstream sources (T92 / T84 / T86 / T87) and the
/// detector. The source crate converts its own report
/// into one or more `ChangeDeltaInput`s; the detector
/// does not reach into the source crates.
///
/// # Field semantics
///
/// - `source` — which upstream family produced this delta.
/// - `affected_target` — domain the delta applies to
///   (e.g. `"example.com"`).
/// - `weight` — unit-interval severity score
///   (`0.0` = clean, `1.0` = worst-case). The
///   detector quantises this through its configurable
///   thresholds to derive the classification band.
/// - `severity` — coarse tier the source attached
///   (clean / advisory / warning / critical). Preserved
///   on the emitted event for downstream consumers.
/// - `target_class` — optional `TargetClass` so the
///   detector can attach a target-class-aware
///   mitigation hint.
/// - `vendor_hint` — optional `VendorId` the source
///   recognised on this target; preserved on the event.
/// - `summary` — short human-readable description of
///   the regression (one line). Used as the
///   `delta_summary.headline` field on the emitted
///   event.
/// - `evidence` — optional structured evidence
///   (e.g. probe IDs, score deltas). Preserved verbatim
///   on the event payload.
///
/// # Example
///
/// ```
/// use stygian_charon::change_feed::{ChangeDeltaInput, DeltaSeverity, DeltaSource};
/// use stygian_charon::vendor_classifier::VendorId;
///
/// let delta = ChangeDeltaInput::new(
///     DeltaSource::Canary,
///     "example.com",
///     0.55,
///     DeltaSeverity::Warning,
///     "integrity probe webdriver regressed 0.18",
/// );
/// assert_eq!(delta.source, DeltaSource::Canary);
/// assert_eq!(delta.affected_target, "example.com");
/// assert!(delta.vendor_hint.is_none());
/// let delta = delta.with_vendor(VendorId::Cloudflare);
/// assert_eq!(delta.vendor_hint, Some(VendorId::Cloudflare));
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeDeltaInput {
    /// Source channel that produced this delta.
    pub source: DeltaSource,
    /// Affected target (domain).
    pub affected_target: String,
    /// Unit-interval severity (`0.0` = clean,
    /// `1.0` = worst-case). Used by the detector
    /// to derive the classification band.
    pub weight: f64,
    /// Coarse severity tier the source attached.
    pub severity: DeltaSeverity,
    /// Optional target class — when set, the
    /// detector uses it to choose the runbook
    /// mitigation hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_class: Option<TargetClass>,
    /// Optional vendor hint — when set, the detector
    /// surfaces it on the emitted event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_hint: Option<VendorId>,
    /// Short human-readable summary of the
    /// regression. Surfaced as the `headline`
    /// field of the delta summary on the emitted
    /// event.
    pub summary: String,
    /// Optional structured evidence (probe IDs,
    /// score deltas, etc.). Preserved verbatim on
    /// the event payload under `evidence.<key>`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub evidence: BTreeMap<String, String>,
}

impl ChangeDeltaInput {
    /// Build a delta with the required fields. The
    /// weight is clamped to `[0.0, 1.0]` and `NaN` is
    /// treated as `0.0` so a single bad source
    /// cannot poison the detector.
    #[must_use]
    pub fn new(
        source: DeltaSource,
        affected_target: impl Into<String>,
        weight: f64,
        severity: DeltaSeverity,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            source,
            affected_target: affected_target.into(),
            weight: sanitise_weight(weight),
            severity,
            target_class: None,
            vendor_hint: None,
            summary: summary.into(),
            evidence: BTreeMap::new(),
        }
    }

    /// Attach a target class to the delta.
    #[must_use]
    pub fn with_target_class(mut self, target_class: TargetClass) -> Self {
        self.target_class = Some(target_class);
        self
    }

    /// Attach a vendor hint to the delta.
    #[must_use]
    pub fn with_vendor(mut self, vendor: VendorId) -> Self {
        self.vendor_hint = Some(vendor);
        self
    }

    /// Attach a structured evidence key/value pair.
    /// Existing keys are overwritten.
    #[must_use]
    pub fn with_evidence(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.evidence.insert(key.into(), value.into());
        self
    }

    /// Whether the delta is considered "noisy" by
    /// the source (i.e. `severity == Clean`). The
    /// detector still classifies a noisy delta as
    /// `Noise` even when the weight is non-zero —
    /// the severity tag is the source's own veto.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        matches!(self.severity, DeltaSeverity::Clean)
    }
}

fn sanitise_weight(weight: f64) -> f64 {
    if weight.is_nan() {
        0.0
    } else {
        weight.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_clamps_weight_to_unit_interval() {
        let delta = ChangeDeltaInput::new(
            DeltaSource::Canary,
            "example.com",
            2.5,
            DeltaSeverity::Warning,
            "test",
        );
        assert!((delta.weight - 1.0).abs() < 1e-9);

        let delta = ChangeDeltaInput::new(
            DeltaSource::Canary,
            "example.com",
            -0.5,
            DeltaSeverity::Warning,
            "test",
        );
        assert!(delta.weight.abs() < 1e-9);

        let delta = ChangeDeltaInput::new(
            DeltaSource::Canary,
            "example.com",
            f64::NAN,
            DeltaSeverity::Warning,
            "test",
        );
        assert!(delta.weight.abs() < 1e-9);
    }

    #[test]
    fn new_round_trips_through_serde_json() {
        let delta = ChangeDeltaInput::new(
            DeltaSource::Proxy,
            "example.com",
            0.30,
            DeltaSeverity::Advisory,
            "score dropped 0.10",
        )
        .with_target_class(TargetClass::Api)
        .with_vendor(VendorId::Cloudflare)
        .with_evidence("baseline_score", "0.85")
        .with_evidence("current_score", "0.75");
        let json = serde_json::to_string(&delta).expect("serialise");
        let parsed: ChangeDeltaInput = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(delta, parsed);
    }

    #[test]
    fn with_evidence_overwrites_existing_key() {
        let delta = ChangeDeltaInput::new(
            DeltaSource::Extraction,
            "example.com",
            0.40,
            DeltaSeverity::Advisory,
            "reliability drop",
        )
        .with_evidence("baseline", "0.95")
        .with_evidence("baseline", "0.90");
        assert_eq!(delta.evidence.get("baseline"), Some(&"0.90".to_string()));
    }

    #[test]
    fn clean_severity_short_circuits_via_is_clean() {
        let clean = ChangeDeltaInput::new(
            DeltaSource::Canary,
            "example.com",
            0.0,
            DeltaSeverity::Clean,
            "ok",
        );
        assert!(clean.is_clean());
        let noisy = ChangeDeltaInput::new(
            DeltaSource::Canary,
            "example.com",
            0.10,
            DeltaSeverity::Advisory,
            "blip",
        );
        assert!(!noisy.is_clean());
    }

    #[test]
    fn source_labels_are_stable() {
        assert_eq!(DeltaSource::Canary.label(), "canary");
        assert_eq!(DeltaSource::Proxy.label(), "proxy");
        assert_eq!(DeltaSource::Extraction.label(), "extraction");
    }

    #[test]
    fn severity_labels_are_stable() {
        assert_eq!(DeltaSeverity::Clean.label(), "clean");
        assert_eq!(DeltaSeverity::Advisory.label(), "advisory");
        assert_eq!(DeltaSeverity::Warning.label(), "warning");
        assert_eq!(DeltaSeverity::Critical.label(), "critical");
    }
}
