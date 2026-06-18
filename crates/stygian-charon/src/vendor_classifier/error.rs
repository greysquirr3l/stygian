//! Error type for vendor classifier loading and validation (T89).
//!
//! Mirrors the [`crate::playbooks::ValidationError`] shape so
//! operators can read either error class with the same mental model:
//! every variant reports the offending **vendor id**, the **field
//! path**, and the **bad value** (where applicable).

use thiserror::Error;

/// Errors returned by vendor-definition validation and loading.
///
/// Every variant embeds the **vendor id** and, where applicable, the
/// **field path** (a dotted path such as `signals[2].weight`) plus
/// the **bad value** as a string. The Display impl formats all three
/// so the operator-facing message is actionable without any
/// auxiliary lookup.
#[derive(Debug, Error)]
pub enum VendorError {
    /// A field failed semantic validation (empty pattern, out-of-range
    /// weight, unknown source, etc.).
    #[error("vendor '{vendor_id}': field '{field}' has invalid value '{value}': {reason}")]
    InvalidField {
        /// Vendor containing the offending field.
        vendor_id: String,
        /// Field path (dotted JSON-pointer-style).
        field: String,
        /// String form of the bad value.
        value: String,
        /// Human-readable reason the value was rejected.
        reason: String,
    },

    /// A required field is missing from the TOML payload.
    #[error("vendor '{vendor_id}': missing required field '{field}'")]
    MissingField {
        /// Vendor missing the field.
        vendor_id: String,
        /// Field path (dotted JSON-pointer-style).
        field: String,
    },

    /// The same vendor id appears more than once in the input bundle.
    #[error("duplicate vendor id '{vendor_id}' in input bundle")]
    DuplicateId {
        /// Conflicting vendor id.
        vendor_id: String,
    },

    /// The vendor id from the TOML does not match the
    /// [`crate::vendor_classifier::VendorId`]
    /// taxonomy.
    #[error("vendor '{vendor_id}' is not part of the supported taxonomy")]
    UnknownVendorId {
        /// Vendor id the loader did not recognise.
        vendor_id: String,
    },

    /// The TOML parser reported a structural error.
    #[error("vendor TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),
}

impl VendorError {
    /// Convenience constructor for [`VendorError::InvalidField`].
    #[must_use]
    pub fn invalid_field(
        vendor_id: impl Into<String>,
        field: impl Into<String>,
        value: impl std::fmt::Display,
        reason: impl Into<String>,
    ) -> Self {
        Self::InvalidField {
            vendor_id: vendor_id.into(),
            field: field.into(),
            value: value.to_string(),
            reason: reason.into(),
        }
    }

    /// Convenience constructor for [`VendorError::MissingField`].
    #[must_use]
    pub fn missing_field(vendor_id: impl Into<String>, field: impl Into<String>) -> Self {
        Self::MissingField {
            vendor_id: vendor_id.into(),
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
    fn invalid_field_message_includes_field_and_value() {
        let err =
            VendorError::invalid_field("datadome", "signals[0].weight", "-1", "weight must be > 0");
        let msg = err.to_string();
        assert!(msg.contains("datadome"));
        assert!(msg.contains("signals[0].weight"));
        assert!(msg.contains("-1"));
        assert!(msg.contains("weight must be > 0"));
        assert_eq!(err.field_path(), Some("signals[0].weight"));
        assert_eq!(err.bad_value(), Some("-1"));
    }

    #[test]
    fn missing_field_message_includes_field() {
        let err = VendorError::missing_field("cloudflare", "display_name");
        let msg = err.to_string();
        assert!(msg.contains("cloudflare"));
        assert!(msg.contains("display_name"));
        assert_eq!(err.field_path(), Some("display_name"));
        assert_eq!(err.bad_value(), None);
    }

    #[test]
    fn duplicate_id_does_not_report_field() {
        let err = VendorError::DuplicateId {
            vendor_id: "akamai".to_string(),
        };
        assert_eq!(err.field_path(), None);
        assert_eq!(err.bad_value(), None);
        assert!(err.to_string().contains("akamai"));
    }
}
