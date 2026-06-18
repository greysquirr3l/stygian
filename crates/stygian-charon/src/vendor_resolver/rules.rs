//! Resolution rule schema (T90).
//!
//! A [`ResolutionRule`] is the **codified policy mapping** from a
//! set of detected anti-bot vendors to a target playbook. Each
//! rule carries:
//!
//! - the **playbook id** it resolves to (`tier2-hostile`,
//!   `tier1-js`, `tier1-static`, or the sentinel empty string for
//!   the `Manual` strategy marker),
//! - the **target class** the playbook maps to,
//! - a **priority** (lower wins) so multi-vendor and conflicting
//!   rule scenarios resolve deterministically,
//! - the **vendor list** that triggers the rule, with per-vendor
//!   weights so the [`MergeStrategy`] can decide what to do when
//!   more than one listed vendor matched,
//! - the **confidence/score gates** (`min_confidence`,
//!   `min_score`) the [`crate::vendor_classifier::VendorClassification`]
//!   must cross for the rule to fire, and
//! - the **merge strategy** the resolver applies when the rule
//!   fires alongside one or more other rules (see the table in
//!   [`crate::vendor_resolver`]).
//!
//! ## Multi-vendor merge strategies
//!
//! | `MergeStrategy`     | Behaviour                                                                                       |
//! |---------------------|--------------------------------------------------------------------------------------------------|
//! | `StrongestVendor`   | Pick the highest-weight vendor in the rule and resolve with its playbook.                       |
//! | `Single`            | Pick the single matched vendor (lowest `VendorId` discriminant on ties) and resolve.            |
//! | `Manual`            | Defer to manual mode — return the `StrategyMarker::Manual` marker.                              |
//!
//! The strategies are documented in the module rustdoc and shipped
//! as data in `crates/stygian-charon/data/vendor_playbook_rules/`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::types::TargetClass;
use crate::vendor_classifier::VendorId;
use crate::vendor_resolver::error::VendorResolverError;

/// How the resolver should combine multiple matched vendors into
/// a single playbook decision.
///
/// See the module-level table for the documented behaviour of each
/// variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeStrategy {
    /// Pick the highest-weight vendor in the rule and resolve with
    /// its playbook.
    StrongestVendor,
    /// Pick the single matched vendor (lowest `VendorId`
    /// discriminant on ties) and resolve with its playbook.
    Single,
    /// Defer to manual mode — return the
    /// [`StrategyMarker::Manual`][crate::vendor_resolver::StrategyMarker::Manual]
    /// marker so the caller preserves its existing manual mode
    /// selection.
    Manual,
}

impl MergeStrategy {
    /// Stable lower-case wire label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::StrongestVendor => "strongest_vendor",
            Self::Single => "single",
            Self::Manual => "manual",
        }
    }
}

/// One vendor entry inside a [`ResolutionRule::vendors`] list.
///
/// Each entry pairs a [`VendorId`] with a **rule-weight** (not to
/// be confused with the classifier's signal weight). The rule-weight
/// tells the [`MergeStrategy::StrongestVendor`] logic which vendor
/// dominates when several listed vendors match simultaneously.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorRuleMatch {
    /// Vendor that triggers the rule. The TOML wire format uses
    /// the [`label`][VendorId::label] (e.g. `"datadome"`,
    /// `"perimeter_x"`) so the format matches the existing vendor
    /// classifier TOML definitions rather than the serde
    /// `snake_case` rename of the enum.
    #[serde(deserialize_with = "deserialize_vendor_id_from_label")]
    pub vendor: VendorId,
    /// Per-rule weight used by
    /// [`MergeStrategy::StrongestVendor`] when multiple listed
    /// vendors match. Higher wins.
    pub weight: u32,
}

fn deserialize_vendor_id_from_label<'de, D>(deserializer: D) -> Result<VendorId, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let label = String::deserialize(deserializer)?;
    VendorId::from_label(&label)
        .ok_or_else(|| serde::de::Error::custom(format!("unknown vendor label '{label}'")))
}

/// Single codified rule mapping vendor patterns to a playbook.
///
/// Rules are **ordered by priority** (lower numbers win). When two
/// rules both match a [`crate::vendor_classifier::VendorClassification`]
/// the resolver picks the lowest-priority rule, then applies its
/// [`merge_strategy`][Self::merge_strategy] to combine any
/// remaining rules into a single decision.
///
/// # Example
///
/// ```
/// use stygian_charon::vendor_resolver::{MergeStrategy, ResolutionRule, VendorRuleMatch};
/// use stygian_charon::types::TargetClass;
/// use stygian_charon::vendor_classifier::VendorId;
///
/// let rule = ResolutionRule {
///     id: "tier2-hostile".to_string(),
///     playbook_id: "tier2-hostile".to_string(),
///     target_class: TargetClass::HighSecurity,
///     priority: 0,
///     merge_strategy: MergeStrategy::StrongestVendor,
///     description: "Hostile anti-bot vendors".to_string(),
///     min_confidence: 0.60,
///     min_score: 5,
///     require_unknown_vendor: false,
///     vendors: vec![VendorRuleMatch {
///         vendor: VendorId::DataDome,
///         weight: 10,
///     }],
/// };
/// assert!(rule.validate().is_ok());
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolutionRule {
    /// Stable rule id (`"tier2-hostile"`, `"tier1-js-cloudflare"`,
    /// etc.). Required, non-empty, unique within a resolver bundle.
    pub id: String,
    /// Playbook id the rule resolves to. Empty string means the
    /// `Manual` strategy marker should be returned instead.
    pub playbook_id: String,
    /// Target class the resolved playbook maps to.
    pub target_class: TargetClass,
    /// Priority (lower wins). The baseline rules use `0`
    /// (tier2-hostile), `10` (tier1-js-cloudflare), `100`
    /// (tier1-static), and `1000` (default-manual).
    pub priority: u32,
    /// Merge strategy used when this rule fires alongside other
    /// matching rules.
    pub merge_strategy: MergeStrategy,
    /// Human-readable description for operator logs.
    #[serde(default)]
    pub description: String,
    /// Minimum classifier confidence the top vendor must cross for
    /// the rule to fire. Must be in `[0.0, 1.0]`.
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f64,
    /// Minimum classifier score the top vendor must reach for the
    /// rule to fire. Must be `> 0` (or `0` for the catch-all
    /// `default-manual` rule).
    #[serde(default)]
    pub min_score: u32,
    /// When `true`, the rule only fires when the classifier reports
    /// [`VendorId::Unknown`]. Used by the `tier1-static` rule so
    /// benign unknown classifications do not accidentally swallow
    /// single-signal low-confidence matches.
    #[serde(default)]
    pub require_unknown_vendor: bool,
    /// Vendors that trigger the rule.
    #[serde(default)]
    pub vendors: Vec<VendorRuleMatch>,
}

const fn default_min_confidence() -> f64 {
    0.0
}

impl ResolutionRule {
    /// Validate the rule's internal consistency. Reports the first
    /// failing field with a structured error that includes both the
    /// rule id and the field path.
    ///
    /// # Errors
    ///
    /// Returns [`VendorResolverError`] on the first inconsistency.
    /// The error embeds the **rule id**, the **field path**, and
    /// the **bad value** so operators can locate the offending
    /// TOML line without re-running the loader.
    pub fn validate(&self) -> Result<(), VendorResolverError> {
        if self.id.trim().is_empty() {
            return Err(VendorResolverError::invalid_rule(
                self.id.clone(),
                "id",
                self.id.clone(),
                "rule id must be a non-empty string",
            ));
        }
        if !(0.0..=1.0).contains(&self.min_confidence) {
            return Err(VendorResolverError::invalid_rule(
                self.id.clone(),
                "min_confidence",
                self.min_confidence,
                "min_confidence must be in [0.0, 1.0]",
            ));
        }
        if self.min_confidence > 0.0 && self.vendors.is_empty() {
            return Err(VendorResolverError::invalid_rule(
                self.id.clone(),
                "vendors",
                "<empty>",
                "vendors list must be non-empty when min_confidence > 0.0",
            ));
        }
        if self.merge_strategy == MergeStrategy::Manual && !self.playbook_id.is_empty() {
            return Err(VendorResolverError::invalid_rule(
                self.id.clone(),
                "playbook_id",
                self.playbook_id.clone(),
                "playbook_id must be empty when merge_strategy = manual",
            ));
        }
        if self.merge_strategy != MergeStrategy::Manual && self.playbook_id.is_empty() {
            return Err(VendorResolverError::invalid_rule(
                self.id.clone(),
                "playbook_id",
                self.playbook_id.clone(),
                "playbook_id must be a non-empty string when merge_strategy is not manual",
            ));
        }
        for (i, v) in self.vendors.iter().enumerate() {
            if v.weight == 0 {
                return Err(VendorResolverError::invalid_rule(
                    self.id.clone(),
                    format!("vendors[{i}].weight"),
                    v.weight,
                    "vendor weight must be > 0",
                ));
            }
        }
        Ok(())
    }

    /// Vendor list indexed by `VendorId` for fast lookup.
    #[must_use]
    pub fn vendors_by_id(&self) -> BTreeMap<VendorId, &VendorRuleMatch> {
        let mut map: BTreeMap<VendorId, &VendorRuleMatch> = BTreeMap::new();
        for v in &self.vendors {
            map.insert(v.vendor, v);
        }
        map
    }
}

/// Parse a raw TOML payload into a [`ResolutionRule`].
///
/// # Errors
///
/// Returns [`VendorResolverError`] when the TOML fails to parse,
/// the declared `[[vendors]]` entries reference an unknown
/// [`VendorId`], or the resulting [`ResolutionRule`] fails
/// [`validate`][ResolutionRule::validate].
pub fn parse_resolution_rule(toml_text: &str) -> Result<ResolutionRule, VendorResolverError> {
    let rule: ResolutionRule = toml::from_str(toml_text)?;
    rule.validate()?;
    Ok(rule)
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

    fn ok_rule() -> ResolutionRule {
        ResolutionRule {
            id: "tier2-hostile".to_string(),
            playbook_id: "tier2-hostile".to_string(),
            target_class: TargetClass::HighSecurity,
            priority: 0,
            merge_strategy: MergeStrategy::StrongestVendor,
            description: "Hostile anti-bot vendors".to_string(),
            min_confidence: 0.60,
            min_score: 5,
            require_unknown_vendor: false,
            vendors: vec![VendorRuleMatch {
                vendor: VendorId::DataDome,
                weight: 10,
            }],
        }
    }

    #[test]
    fn valid_rule_passes_validation() {
        assert!(ok_rule().validate().is_ok());
    }

    #[test]
    fn empty_rule_id_is_rejected() {
        let mut r = ok_rule();
        r.id.clear();
        let err = r.validate().expect_err("empty rule id");
        assert_eq!(err.field_path(), Some("id"));
    }

    #[test]
    fn out_of_range_confidence_is_rejected() {
        let mut r = ok_rule();
        r.min_confidence = 1.5;
        let err = r.validate().expect_err("bad confidence");
        assert_eq!(err.field_path(), Some("min_confidence"));
    }

    #[test]
    fn vendors_required_when_confidence_above_zero() {
        let mut r = ok_rule();
        r.vendors.clear();
        let err = r.validate().expect_err("empty vendors");
        assert_eq!(err.field_path(), Some("vendors"));
    }

    #[test]
    fn manual_strategy_requires_empty_playbook_id() {
        let mut r = ok_rule();
        r.merge_strategy = MergeStrategy::Manual;
        r.playbook_id = "tier2-hostile".to_string();
        let err = r.validate().expect_err("manual w/ playbook_id");
        assert_eq!(err.field_path(), Some("playbook_id"));
    }

    #[test]
    fn non_manual_strategy_requires_non_empty_playbook_id() {
        let mut r = ok_rule();
        r.playbook_id.clear();
        let err = r.validate().expect_err("non-manual w/ empty playbook_id");
        assert_eq!(err.field_path(), Some("playbook_id"));
    }

    #[test]
    fn zero_weight_vendor_is_rejected() {
        let mut r = ok_rule();
        r.vendors[0].weight = 0;
        let err = r.validate().expect_err("zero weight");
        let path = err.field_path().unwrap_or("");
        assert!(path.contains("vendors[0]"), "got {path}");
    }

    #[test]
    fn merge_strategy_labels_are_stable() {
        assert_eq!(MergeStrategy::StrongestVendor.label(), "strongest_vendor");
        assert_eq!(MergeStrategy::Single.label(), "single");
        assert_eq!(MergeStrategy::Manual.label(), "manual");
    }

    #[test]
    fn parse_round_trip_through_toml() {
        let toml_text = r#"
id = "tier2-hostile"
playbook_id = "tier2-hostile"
target_class = "high_security"
priority = 0
merge_strategy = "strongest_vendor"
description = "Hostile anti-bot vendors"
min_confidence = 0.60
min_score = 5

[[vendors]]
vendor = "datadome"
weight = 10
"#;
        let rule = parse_resolution_rule(toml_text).expect("parse");
        assert_eq!(rule.id, "tier2-hostile");
        assert_eq!(rule.target_class, TargetClass::HighSecurity);
        assert_eq!(rule.vendors.len(), 1);
        assert_eq!(rule.vendors[0].vendor, VendorId::DataDome);
    }

    #[test]
    fn vendors_by_id_groups_correctly() {
        let rule = ResolutionRule {
            id: "x".to_string(),
            playbook_id: "x".to_string(),
            target_class: TargetClass::Unknown,
            priority: 0,
            merge_strategy: MergeStrategy::Single,
            description: String::new(),
            min_confidence: 0.0,
            min_score: 0,
            require_unknown_vendor: false,
            vendors: vec![
                VendorRuleMatch {
                    vendor: VendorId::DataDome,
                    weight: 5,
                },
                VendorRuleMatch {
                    vendor: VendorId::Cloudflare,
                    weight: 7,
                },
            ],
        };
        let map = rule.vendors_by_id();
        assert_eq!(map.get(&VendorId::DataDome).map(|v| v.weight), Some(5));
        assert_eq!(map.get(&VendorId::Cloudflare).map(|v| v.weight), Some(7));
    }
}
