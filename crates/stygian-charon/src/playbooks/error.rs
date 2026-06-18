//! Actionable validation errors for playbooks (T85).
//!
//! Every [`ValidationError`] variant **carries the field path and the
//! bad value** so operators can locate the offending knob without
//! re-running the loader. The format is stable:
//!
//! ```text
//! playbook '<playbook_id>': field '<field>' has invalid value '<value>': <reason>
//! ```
//!
//! # Example
//!
//! ```
//! use stygian_charon::playbooks::{AcquisitionDefaults, EscalationStrategy, PacingProfile, Playbook, ProxyPreference};
//! use stygian_charon::acquisition::AcquisitionModeHint;
//! use stygian_charon::types::{ExecutionMode, SessionMode, TargetClass, TelemetryLevel};
//!
//! let bad = Playbook {
//!     id: "broken".to_string(),
//!     target_class: TargetClass::ContentSite,
//!     description: "intentionally broken".to_string(),
//!     acquisition: AcquisitionDefaults {
//!         retry_budget: 0,
//!         ..AcquisitionDefaults::default_for(TargetClass::ContentSite)
//!     },
//!     proxy_preference: ProxyPreference::default(),
//!     pacing: PacingProfile::default(),
//!     escalation: EscalationStrategy::Capped { ceiling: AcquisitionModeHint::Fast },
//! };
//! let err = bad.validate().expect_err("retry_budget must be > 0");
//! let message = err.to_string();
//! assert!(message.contains("acquisition.retry_budget"), "message must name the field: {message}");
//! assert!(message.contains("0"), "message must include the bad value: {message}");
//! ```

use thiserror::Error;

/// Errors returned by playbook validation and loading.
///
/// Every variant embeds the **playbook id**, the **field path** (a
/// dotted JSON-pointer-style path such as `acquisition.retry_budget`),
/// and the **bad value** as a string. The Display impl formats all
/// three so the operator-facing message is actionable without any
/// auxiliary lookup.
#[derive(Debug, Error)]
pub enum ValidationError {
    /// A field failed semantic validation (out of range, wrong
    /// multiplicity, inconsistent state).
    #[error("playbook '{playbook_id}': field '{field}' has invalid value '{value}': {reason}")]
    InvalidField {
        /// Playbook containing the offending field.
        playbook_id: String,
        /// Field path (dotted JSON-pointer-style).
        field: String,
        /// String form of the bad value.
        value: String,
        /// Human-readable reason the value was rejected.
        reason: String,
    },

    /// A required field is missing from the TOML payload.
    #[error("playbook '{playbook_id}': missing required field '{field}'")]
    MissingField {
        /// Playbook missing the field.
        playbook_id: String,
        /// Field path (dotted JSON-pointer-style).
        field: String,
    },

    /// The same playbook id appears more than once in the input
    /// bundle.
    #[error("duplicate playbook id '{playbook_id}' in input bundle")]
    DuplicateId {
        /// Conflicting playbook id.
        playbook_id: String,
    },

    /// The TOML parser reported a structural error.
    #[error("playbook TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    /// The resolver was asked for a playbook id that is not loaded.
    #[error("playbook '{playbook_id}' not registered in resolver")]
    UnknownPlaybook {
        /// Playbook id the resolver could not find.
        playbook_id: String,
    },

    /// A reference to a sibling playbook id (e.g. an
    /// `extends = "tier1-static"` declaration) could not be resolved.
    #[error("playbook '{playbook_id}' extends unknown playbook '{parent_id}'")]
    UnknownParent {
        /// Child playbook id.
        playbook_id: String,
        /// Parent playbook id it tried to extend.
        parent_id: String,
    },
}

impl ValidationError {
    /// Convenience constructor for [`ValidationError::InvalidField`]
    /// that builds the field path and bad-value string from caller
    /// inputs.
    #[must_use]
    pub fn invalid_field(
        playbook_id: impl Into<String>,
        field: impl Into<String>,
        value: impl std::fmt::Display,
        reason: impl Into<String>,
    ) -> Self {
        Self::InvalidField {
            playbook_id: playbook_id.into(),
            field: field.into(),
            value: value.to_string(),
            reason: reason.into(),
        }
    }

    /// Convenience constructor for [`ValidationError::MissingField`].
    #[must_use]
    pub fn missing_field(playbook_id: impl Into<String>, field: impl Into<String>) -> Self {
        Self::MissingField {
            playbook_id: playbook_id.into(),
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
mod tests {
    use super::*;

    #[test]
    fn invalid_field_message_includes_field_and_value() {
        let err = ValidationError::invalid_field("tier1-js", "pacing.rate_limit_rps", "-0.5", "must be > 0");
        let msg = err.to_string();
        assert!(msg.contains("tier1-js"));
        assert!(msg.contains("pacing.rate_limit_rps"));
        assert!(msg.contains("-0.5"));
        assert!(msg.contains("must be > 0"));
        assert_eq!(err.field_path(), Some("pacing.rate_limit_rps"));
        assert_eq!(err.bad_value(), Some("-0.5"));
    }

    #[test]
    fn missing_field_message_includes_field() {
        let err = ValidationError::missing_field("tier2-hostile", "acquisition.mode");
        let msg = err.to_string();
        assert!(msg.contains("tier2-hostile"));
        assert!(msg.contains("acquisition.mode"));
        assert_eq!(err.field_path(), Some("acquisition.mode"));
        assert_eq!(err.bad_value(), None);
    }

    #[test]
    fn duplicate_id_does_not_report_field() {
        let err = ValidationError::DuplicateId {
            playbook_id: "tier1-static".to_string(),
        };
        assert_eq!(err.field_path(), None);
        assert_eq!(err.bad_value(), None);
        assert!(err.to_string().contains("tier1-static"));
    }
}