//! Errors returned by the vendor-to-playbook resolver (T90).
//!
//! Every variant embeds the **rule id** and, where applicable, the
//! **field path** plus the **bad value** as a string. The format
//! mirrors the existing [`crate::playbooks::ValidationError`] and
//! [`crate::vendor_classifier::VendorError`] shapes so operators
//! can read any of the three error classes with the same mental
//! model.
//!
//! # Example
//!
//! ```
//! use stygian_charon::vendor_resolver::VendorResolverError;
//!
//! let err = VendorResolverError::invalid_rule(
//!     "tier2-hostile",
//!     "min_confidence",
//!     "2.0",
//!     "min_confidence must be in [0.0, 1.0]",
//! );
//! let msg = err.to_string();
//! assert!(msg.contains("tier2-hostile"));
//! assert!(msg.contains("min_confidence"));
//! assert!(msg.contains("2.0"));
//! ```

use thiserror::Error;

/// Errors returned by vendor-resolver rule validation and loading.
#[derive(Debug, Error)]
pub enum VendorResolverError {
    /// A field on a resolution rule failed semantic validation.
    #[error("resolution rule '{rule_id}': field '{field}' has invalid value '{value}': {reason}")]
    InvalidField {
        /// Rule id containing the offending field.
        rule_id: String,
        /// Field path (dotted JSON-pointer-style).
        field: String,
        /// String form of the bad value.
        value: String,
        /// Human-readable reason the value was rejected.
        reason: String,
    },

    /// A required field is missing from the TOML payload.
    #[error("resolution rule '{rule_id}': missing required field '{field}'")]
    MissingField {
        /// Rule id missing the field.
        rule_id: String,
        /// Field path (dotted JSON-pointer-style).
        field: String,
    },

    /// The same rule id appears more than once in the input bundle.
    #[error("duplicate resolution rule id '{rule_id}' in input bundle")]
    DuplicateId {
        /// Conflicting rule id.
        rule_id: String,
    },

    /// A `[[vendors]]` entry referenced an unknown [`VendorId`].
    #[error("resolution rule '{rule_id}' references unknown vendor '{vendor_id}'")]
    UnknownVendor {
        /// Rule id that referenced the unknown vendor.
        rule_id: String,
        /// Vendor label that did not parse.
        vendor_id: String,
    },

    /// The TOML parser reported a structural error.
    #[error("resolution rule TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),
}

impl VendorResolverError {
    /// Convenience constructor for [`VendorResolverError::InvalidField`].
    #[must_use]
    pub fn invalid_rule(
        rule_id: impl Into<String>,
        field: impl Into<String>,
        value: impl std::fmt::Display,
        reason: impl Into<String>,
    ) -> Self {
        Self::InvalidField {
            rule_id: rule_id.into(),
            field: field.into(),
            value: value.to_string(),
            reason: reason.into(),
        }
    }

    /// Convenience constructor for [`VendorResolverError::MissingField`].
    #[must_use]
    pub fn missing_field(rule_id: impl Into<String>, field: impl Into<String>) -> Self {
        Self::MissingField {
            rule_id: rule_id.into(),
            field: field.into(),
        }
    }

    /// Field path (dotted JSON-pointer-style) when applicable.
    #[must_use]
    pub fn field_path(&self) -> Option<&str> {
        match self {
            Self::InvalidField { field, .. } | Self::MissingField { field, .. } => Some(field),
            _ => None,
        }
    }

    /// Bad value (string form) when applicable.
    #[must_use]
    pub fn bad_value(&self) -> Option<&str> {
        match self {
            Self::InvalidField { value, .. } => Some(value),
            _ => None,
        }
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

    #[test]
    fn invalid_rule_message_includes_rule_field_and_value() {
        let err = VendorResolverError::invalid_rule(
            "tier2-hostile",
            "min_confidence",
            "2.0",
            "must be in [0.0, 1.0]",
        );
        let msg = err.to_string();
        assert!(msg.contains("tier2-hostile"));
        assert!(msg.contains("min_confidence"));
        assert!(msg.contains("2.0"));
        assert!(msg.contains("must be in [0.0, 1.0]"));
        assert_eq!(err.field_path(), Some("min_confidence"));
        assert_eq!(err.bad_value(), Some("2.0"));
    }

    #[test]
    fn missing_field_message_includes_field() {
        let err = VendorResolverError::missing_field("tier1-js", "playbook_id");
        let msg = err.to_string();
        assert!(msg.contains("tier1-js"));
        assert!(msg.contains("playbook_id"));
        assert_eq!(err.field_path(), Some("playbook_id"));
        assert_eq!(err.bad_value(), None);
    }

    #[test]
    fn duplicate_id_does_not_report_field() {
        let err = VendorResolverError::DuplicateId {
            rule_id: "tier2-hostile".to_string(),
        };
        assert_eq!(err.field_path(), None);
        assert_eq!(err.bad_value(), None);
        assert!(err.to_string().contains("tier2-hostile"));
    }

    #[test]
    fn unknown_vendor_message_includes_label() {
        let err = VendorResolverError::UnknownVendor {
            rule_id: "tier2-hostile".to_string(),
            vendor_id: "nope".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("tier2-hostile"));
        assert!(msg.contains("nope"));
    }
}
