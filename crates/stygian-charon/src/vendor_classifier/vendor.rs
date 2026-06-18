//! Vendor taxonomy and TOML-loadable definitions (T89).
//!
//! The [`VendorId`] enum is the **stable, wire-level identifier**
//! for every anti-bot vendor the classifier knows about. Adding a
//! new variant is a breaking change for downstream consumers
//! (e.g. `VendorClassification` JSON payloads), so the taxonomy is
//! intentionally small and uses `#[serde(rename_all = "snake_case")]`
//! for predictable wire labels.
//!
//! ## Tier 1 (always shipped)
//!
//! The four Tier 1 vendors are documented in
//! `crates/stygian-charon/data/vendors/` and embedded into the
//! binary at compile time via `include_str!`. Their TOML payload
//! is the single source of truth for the per-vendor signal
//! catalogue; the enum below is the wire/lookup contract.
//!
//! | `VendorId`     | Display name                | TOML file                        |
//! |----------------|-----------------------------|----------------------------------|
//! | `DataDome`     | `DataDome`                  | `data/vendors/datadome.toml`     |
//! | `PerimeterX`   | `PerimeterX` / HUMAN Security | `data/vendors/perimeter_x.toml`  |
//! | `Akamai`       | `Akamai` Bot Manager        | `data/vendors/akamai.toml`       |
//! | `Cloudflare`   | `Cloudflare`                | `data/vendors/cloudflare.toml`   |
//!
//! ## Tier 2 (taxonomy-only, no baseline signals)
//!
//! `Hcaptcha`, `Recaptcha`, `Kasada`, `FingerprintCom`,
//! `ShapeSecurity`, and `Imperva` are present in the enum so
//! downstream T88/T90 layers can name them, but no baseline
//! signals ship for them — operators must register their own
//! signal catalogue via
//! [`VendorDefinition`][crate::vendor_classifier::VendorDefinition].
//!
//! ## Unknown
//!
//! `Unknown` is the catch-all variant used when no vendor matched
//! or when no classification can be produced. It must remain the
//! **last** variant so it sorts last in the
//! deterministic tie-break rule (see
//! [`crate::vendor_classifier::VendorClassification`]).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::vendor_classifier::error::VendorError;
use crate::vendor_classifier::evidence::EvidenceSource;

/// Stable identifier for an anti-bot vendor.
///
/// The discriminant order is **significant**: it is the
/// deterministic tie-break rule for the classifier. When two
/// vendors tie on the top score, the lower discriminant
/// (`Akamai` < `Cloudflare` < `DataDome` < `PerimeterX` < …)
/// wins.
///
/// # Example
///
/// ```
/// use stygian_charon::vendor_classifier::VendorId;
///
/// let v = VendorId::DataDome;
/// assert_eq!(v.label(), "datadome");
/// assert_eq!(v.tier(), 1);
/// ```
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum VendorId {
    /// `Akamai` Bot Manager (`_abck`, `bm_sz`).
    Akamai,
    /// `Cloudflare` bot management (`cf-ray`, `__cf_bm`).
    Cloudflare,
    /// `DataDome` (`datadome=`, `x-datadome`).
    DataDome,
    /// `PerimeterX` / HUMAN Security (`_px3`, `_px2`).
    PerimeterX,
    /// hCaptcha challenge provider.
    Hcaptcha,
    /// Google reCAPTCHA challenge provider.
    Recaptcha,
    /// Kasada challenge provider.
    Kasada,
    /// Fingerprint.com identification.
    FingerprintCom,
    /// Shape Security (F5).
    ShapeSecurity,
    /// Imperva (Incapsula) bot management.
    Imperva,
    /// Catch-all when no vendor matched.
    #[default]
    Unknown,
}

impl VendorId {
    /// Stable, lower-case wire label.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// assert_eq!(VendorId::DataDome.label(), "datadome");
    /// assert_eq!(VendorId::PerimeterX.label(), "perimeter_x");
    /// assert_eq!(VendorId::Cloudflare.label(), "cloudflare");
    /// assert_eq!(VendorId::Akamai.label(), "akamai");
    /// ```
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Akamai => "akamai",
            Self::Cloudflare => "cloudflare",
            Self::DataDome => "datadome",
            Self::PerimeterX => "perimeter_x",
            Self::Hcaptcha => "hcaptcha",
            Self::Recaptcha => "recaptcha",
            Self::Kasada => "kasada",
            Self::FingerprintCom => "fingerprint_com",
            Self::ShapeSecurity => "shape_security",
            Self::Imperva => "imperva",
            Self::Unknown => "unknown",
        }
    }

    /// Tier number (1 = always shipped, 2 = taxonomy-only, 0 = unknown).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// assert_eq!(VendorId::DataDome.tier(), 1);
    /// assert_eq!(VendorId::Cloudflare.tier(), 1);
    /// assert_eq!(VendorId::Akamai.tier(), 1);
    /// assert_eq!(VendorId::PerimeterX.tier(), 1);
    /// assert_eq!(VendorId::Unknown.tier(), 0);
    /// ```
    #[must_use]
    pub const fn tier(self) -> u8 {
        match self {
            Self::DataDome | Self::PerimeterX | Self::Akamai | Self::Cloudflare => 1,
            Self::Hcaptcha
            | Self::Recaptcha
            | Self::Kasada
            | Self::FingerprintCom
            | Self::ShapeSecurity
            | Self::Imperva => 2,
            Self::Unknown => 0,
        }
    }

    /// Parse a [`VendorId`] from its [`label`][Self::label].
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::vendor_classifier::VendorId;
    ///
    /// assert_eq!(VendorId::from_label("datadome"), Some(VendorId::DataDome));
    /// assert_eq!(VendorId::from_label("cloudflare"), Some(VendorId::Cloudflare));
    /// assert_eq!(VendorId::from_label("nope"), None);
    /// ```
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "akamai" => Some(Self::Akamai),
            "cloudflare" => Some(Self::Cloudflare),
            "datadome" => Some(Self::DataDome),
            "perimeter_x" => Some(Self::PerimeterX),
            "hcaptcha" => Some(Self::Hcaptcha),
            "recaptcha" => Some(Self::Recaptcha),
            "kasada" => Some(Self::Kasada),
            "fingerprint_com" => Some(Self::FingerprintCom),
            "shape_security" => Some(Self::ShapeSecurity),
            "imperva" => Some(Self::Imperva),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// One signal row from a vendor definition's `[[signals]]` table.
///
/// A signal is the smallest unit the classifier matches against the
/// input strings (cookies, headers, challenge URLs, body markers,
/// scripts). Patterns are matched **case-insensitively** — the
/// loader lower-cases them at load time so the per-request
/// classification hot path never has to.
///
/// # Example
///
/// ```
/// use stygian_charon::vendor_classifier::{EvidenceSource, VendorSignal};
///
/// let s = VendorSignal {
///     pattern: "x-datadome".to_string(),
///     source: EvidenceSource::Header,
///     weight: 5,
/// };
/// assert_eq!(s.weight, 5);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorSignal {
    /// Literal pattern to search for (case-insensitive).
    pub pattern: String,
    /// Which input channel the pattern is matched against.
    pub source: EvidenceSource,
    /// Weight contributed to the vendor score on a hit.
    pub weight: u32,
}

/// One vendor's signal catalogue. Multiple vendors can ship
/// definitions; the [`crate::vendor_classifier::VendorClassifier`]
/// consumes them all and ranks the matches.
///
/// Definitions are loaded from TOML at compile time via
/// `include_str!`. The schema is
/// `serde::Deserialize` so the same TOML files double as the
/// operator-facing configuration surface.
///
/// # Example
///
/// ```
/// use stygian_charon::vendor_classifier::{VendorDefinition, VendorId, VendorSignal, EvidenceSource};
///
/// let def = VendorDefinition {
///     id: VendorId::DataDome,
///     display_name: "DataDome".to_string(),
///     description: "baseline".to_string(),
///     tier: 1,
///     signals: vec![VendorSignal {
///         pattern: "x-datadome".to_string(),
///         source: EvidenceSource::Header,
///         weight: 5,
///     }],
/// };
/// assert!(def.validate().is_ok());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorDefinition {
    /// Vendor identifier from the [`VendorId`] enum.
    pub id: VendorId,
    /// Human-readable display name (used in operator logs).
    pub display_name: String,
    /// Short description of the vendor stack.
    #[serde(default)]
    pub description: String,
    /// Tier (1 = always shipped, 2 = taxonomy-only).
    pub tier: u8,
    /// Signal catalogue.
    #[serde(default)]
    pub signals: Vec<VendorSignal>,
}

impl VendorDefinition {
    /// Validate the definition's internal consistency.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError`] on the first inconsistency. The
    /// error embeds the field path and the bad value so operators
    /// can locate the offending TOML line without re-running the
    /// loader.
    pub fn validate(&self) -> Result<(), VendorError> {
        if self.display_name.trim().is_empty() {
            return Err(VendorError::invalid_field(
                self.id.label(),
                "display_name",
                self.display_name.clone(),
                "display_name must be a non-empty string",
            ));
        }
        if !(0..=2).contains(&self.tier) {
            return Err(VendorError::invalid_field(
                self.id.label(),
                "tier",
                self.tier,
                "tier must be 0 (unknown), 1 (baseline), or 2 (taxonomy-only)",
            ));
        }
        for (i, sig) in self.signals.iter().enumerate() {
            if sig.pattern.trim().is_empty() {
                return Err(VendorError::invalid_field(
                    self.id.label(),
                    format!("signals[{i}].pattern"),
                    sig.pattern.clone(),
                    "pattern must be a non-empty string",
                ));
            }
            if sig.weight == 0 {
                return Err(VendorError::invalid_field(
                    self.id.label(),
                    format!("signals[{i}].weight"),
                    sig.weight,
                    "weight must be > 0",
                ));
            }
        }
        Ok(())
    }

    /// Return the signals, indexed by [`EvidenceSource`] for fast
    /// classification.
    #[must_use]
    pub fn signals_by_source(&self) -> BTreeMap<EvidenceSource, Vec<&VendorSignal>> {
        let mut grouped: BTreeMap<EvidenceSource, Vec<&VendorSignal>> = BTreeMap::new();
        for sig in &self.signals {
            grouped.entry(sig.source).or_default().push(sig);
        }
        grouped
    }
}

/// Parse a raw TOML payload into a [`VendorDefinition`].
///
/// The TOML is expected to declare the `id` field as the lower-case
/// `VendorId` label (e.g. `"datadome"`). The loader maps that label
/// into a [`VendorId`] discriminant and rejects unknown ids with
/// [`VendorError::UnknownVendorId`].
///
/// # Errors
///
/// Returns [`VendorError`] when the TOML fails to parse, the
/// declared id is not part of the supported taxonomy, or the
/// resulting [`VendorDefinition`] fails [`validate`][VendorDefinition::validate].
pub fn parse_vendor_definition(toml_text: &str) -> Result<VendorDefinition, VendorError> {
    #[derive(Deserialize)]
    struct RawDefinition {
        id: String,
        display_name: String,
        #[serde(default)]
        description: String,
        #[serde(default = "default_tier")]
        tier: u8,
        #[serde(default)]
        signals: Vec<VendorSignal>,
    }

    let raw: RawDefinition = toml::from_str(toml_text)?;
    let id = VendorId::from_label(&raw.id).ok_or_else(|| VendorError::UnknownVendorId {
        vendor_id: raw.id.clone(),
    })?;
    let def = VendorDefinition {
        id,
        display_name: raw.display_name,
        description: raw.description,
        tier: raw.tier,
        signals: raw
            .signals
            .into_iter()
            .map(|mut s| {
                s.pattern = s.pattern.to_ascii_lowercase();
                s
            })
            .collect(),
    };
    def.validate()?;
    Ok(def)
}

const fn default_tier() -> u8 {
    1
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

    #[test]
    fn vendor_id_labels_round_trip() {
        for v in [
            VendorId::Akamai,
            VendorId::Cloudflare,
            VendorId::DataDome,
            VendorId::PerimeterX,
            VendorId::Hcaptcha,
            VendorId::Recaptcha,
            VendorId::Kasada,
            VendorId::FingerprintCom,
            VendorId::ShapeSecurity,
            VendorId::Imperva,
            VendorId::Unknown,
        ] {
            assert_eq!(VendorId::from_label(v.label()), Some(v));
        }
    }

    #[test]
    fn vendor_id_unknown_label_returns_none() {
        assert_eq!(VendorId::from_label("nope"), None);
        assert_eq!(VendorId::from_label(""), None);
        assert_eq!(VendorId::from_label("DataDome"), None); // case-sensitive
    }

    #[test]
    fn vendor_id_tier_matches_taxonomy_table() {
        assert_eq!(VendorId::DataDome.tier(), 1);
        assert_eq!(VendorId::PerimeterX.tier(), 1);
        assert_eq!(VendorId::Akamai.tier(), 1);
        assert_eq!(VendorId::Cloudflare.tier(), 1);
        assert_eq!(VendorId::Hcaptcha.tier(), 2);
        assert_eq!(VendorId::Recaptcha.tier(), 2);
        assert_eq!(VendorId::Unknown.tier(), 0);
    }

    #[test]
    fn definition_rejects_empty_display_name() {
        let def = VendorDefinition {
            id: VendorId::DataDome,
            display_name: String::new(),
            description: String::new(),
            tier: 1,
            signals: Vec::new(),
        };
        let err = def.validate().expect_err("empty display_name");
        assert_eq!(err.field_path(), Some("display_name"));
    }

    #[test]
    fn definition_rejects_out_of_range_tier() {
        let def = VendorDefinition {
            id: VendorId::DataDome,
            display_name: "x".to_string(),
            description: String::new(),
            tier: 9,
            signals: Vec::new(),
        };
        let err = def.validate().expect_err("bad tier");
        assert_eq!(err.field_path(), Some("tier"));
    }

    #[test]
    fn definition_rejects_empty_pattern() {
        let def = VendorDefinition {
            id: VendorId::DataDome,
            display_name: "x".to_string(),
            description: String::new(),
            tier: 1,
            signals: vec![VendorSignal {
                pattern: String::new(),
                source: EvidenceSource::Header,
                weight: 5,
            }],
        };
        let err = def.validate().expect_err("empty pattern");
        assert!(err.field_path().is_some_and(|p| p.contains("signals[0]")));
    }

    #[test]
    fn definition_rejects_zero_weight() {
        let def = VendorDefinition {
            id: VendorId::DataDome,
            display_name: "x".to_string(),
            description: String::new(),
            tier: 1,
            signals: vec![VendorSignal {
                pattern: "x".to_string(),
                source: EvidenceSource::Header,
                weight: 0,
            }],
        };
        let err = def.validate().expect_err("zero weight");
        assert!(err.field_path().is_some_and(|p| p.contains("signals[0]")));
    }

    #[test]
    fn parse_vendor_definition_round_trips_through_toml() {
        let toml_text = r#"
id = "datadome"
display_name = "DataDome"
description = "test"
tier = 1

[[signals]]
pattern = "X-DATADOME"
source = "header"
weight = 5
"#;
        let def = parse_vendor_definition(toml_text).expect("parse");
        assert_eq!(def.id, VendorId::DataDome);
        assert_eq!(def.tier, 1);
        // Patterns are case-folded at load time.
        assert_eq!(def.signals[0].pattern, "x-datadome");
    }

    #[test]
    fn parse_vendor_definition_rejects_unknown_id() {
        let toml_text = r#"
id = "nope"
display_name = "Nope"
tier = 1
"#;
        let err = parse_vendor_definition(toml_text).expect_err("unknown id");
        assert!(matches!(err, VendorError::UnknownVendorId { .. }));
    }

    #[test]
    fn signals_by_source_groups_correctly() {
        let def = VendorDefinition {
            id: VendorId::DataDome,
            display_name: "x".to_string(),
            description: String::new(),
            tier: 1,
            signals: vec![
                VendorSignal {
                    pattern: "a".to_string(),
                    source: EvidenceSource::Header,
                    weight: 1,
                },
                VendorSignal {
                    pattern: "b".to_string(),
                    source: EvidenceSource::Header,
                    weight: 2,
                },
                VendorSignal {
                    pattern: "c".to_string(),
                    source: EvidenceSource::Cookie,
                    weight: 3,
                },
            ],
        };
        let grouped = def.signals_by_source();
        assert_eq!(grouped.get(&EvidenceSource::Header).map(Vec::len), Some(2));
        assert_eq!(grouped.get(&EvidenceSource::Cookie).map(Vec::len), Some(1));
        assert_eq!(grouped.get(&EvidenceSource::BodyMarker).map(Vec::len), None);
    }
}
