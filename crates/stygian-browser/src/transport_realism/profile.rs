//! Per-target transport profile.
//!
//! [`TransportProfile`] is the typed config field callers attach to
//! an [`AcquisitionRequest`][crate::acquisition::AcquisitionRequest]
//! to express which HTTP/2 surfaces they want the runner to compare
//! against. The profile is fully serialisable so it can travel in
//! config files / session snapshots without bespoke glue.

use serde::{Deserialize, Serialize};

use crate::tls_validation::CHROME_136_HTTP2_SETTINGS;

use super::observations::{HEADER_ORDER_CHROME_136, PSEUDO_HEADER_ORDER_CHROME_136};

/// HTTP/2 expectations as a compact bitmask.
///
/// The runner compares the supplied observation against the profile
/// only for the checks whose bit is set. Encoding expectations as a
/// `u8` bitmask keeps [`TransportProfile`] free of
/// `clippy::struct_excessive_bools` lints — three separate `bool`
/// fields would otherwise trip the lint even though each flag is
/// independently meaningful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Http2Expectations {
    /// Bitmask: bit 0 = settings, bit 1 = pseudo-header order,
    /// bit 2 = header order.
    bits: u8,
}

impl Http2Expectations {
    /// Settings-frame fingerprint comparison is enabled.
    pub const SETTINGS: u8 = 1 << 0;
    /// Pseudo-header order comparison is enabled.
    pub const PSEUDO_HEADER_ORDER: u8 = 1 << 1;
    /// Regular header order comparison is enabled.
    pub const HEADER_ORDER: u8 = 1 << 2;
    /// All three expectations enabled (default profile).
    pub const ALL: u8 = Self::SETTINGS | Self::PSEUDO_HEADER_ORDER | Self::HEADER_ORDER;

    /// Build a bitmask from raw flag bits.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self { bits }
    }

    /// `true` when no expectations are enabled.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.bits == 0
    }

    /// `true` when the supplied `flag` bit is set.
    #[must_use]
    pub const fn contains(self, flag: u8) -> bool {
        (self.bits & flag) == flag
    }

    /// Number of expectation bits enabled.
    #[must_use]
    pub const fn count(self) -> usize {
        (self.bits & Self::SETTINGS != 0) as usize
            + (self.bits & Self::PSEUDO_HEADER_ORDER != 0) as usize
            + (self.bits & Self::HEADER_ORDER != 0) as usize
    }
}

impl Default for Http2Expectations {
    fn default() -> Self {
        Self { bits: Self::ALL }
    }
}

/// Per-target HTTP/2 transport expectations.
///
/// A [`TransportProfile`] carries the reference fingerprints the
/// [`score`][crate::transport_realism::score] function compares the
/// [`TransportObservation`][crate::transport_realism::TransportObservation]
/// against. When the runner is asked to evaluate a request, the
/// profile travels with the [`AcquisitionRequest`][crate::acquisition::AcquisitionRequest]
/// and the resulting [`TransportCompatibility`][crate::transport_realism::TransportCompatibility]
/// is attached to the
/// [`AcquisitionResult::transport_realism`][crate::acquisition::AcquisitionResult::transport_realism]
/// field as a strategy hint for downstream policy mapping (T83 / T85
/// / T89 / T93).
///
/// # Example
///
/// ```
/// use stygian_browser::transport_realism::TransportProfile;
///
/// let profile = TransportProfile::chrome_136_reference();
/// assert!(profile.expectations.contains(TransportProfile::SETTINGS));
/// assert_eq!(profile.expected_http2_settings.len(), 5);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportProfile {
    /// Logical profile name (e.g. `"chrome-136"`, `"firefox-130"`).
    /// Free-form: callers can use it to label the profile in
    /// telemetry.
    #[serde(default = "default_profile_name")]
    pub name: String,
    /// Reference HTTP/2 SETTINGS frame fingerprint.
    #[serde(default = "default_http2_settings")]
    pub expected_http2_settings: Vec<(u32, u32)>,
    /// Reference HTTP/2 pseudo-header order.
    #[serde(default = "default_pseudo_header_order")]
    pub expected_pseudo_header_order: Vec<String>,
    /// Reference HTTP/2 header order (after pseudo-headers).
    #[serde(default = "default_header_order")]
    pub expected_header_order: Vec<String>,
    /// HTTP/2 expectations bitmask (settings / pseudo-header order /
    /// header order).
    #[serde(default)]
    pub expectations: Http2Expectations,
    /// When `true`, the runner rejects profiles with no HTTP/2
    /// observations as `incompatible` (used to detect partial
    /// instrumentation in hostile-target acquisition paths).
    #[serde(default)]
    pub require_http2_observations: bool,
}

impl TransportProfile {
    /// Re-export of [`Http2Expectations::SETTINGS`] so callers can
    /// write `profile.expectations.contains(TransportProfile::SETTINGS)`.
    pub const SETTINGS: u8 = Http2Expectations::SETTINGS;
    /// Re-export of [`Http2Expectations::PSEUDO_HEADER_ORDER`].
    pub const PSEUDO_HEADER_ORDER: u8 = Http2Expectations::PSEUDO_HEADER_ORDER;
    /// Re-export of [`Http2Expectations::HEADER_ORDER`].
    pub const HEADER_ORDER: u8 = Http2Expectations::HEADER_ORDER;
}

fn default_profile_name() -> String {
    "chrome-136".to_string()
}

fn default_http2_settings() -> Vec<(u32, u32)> {
    CHROME_136_HTTP2_SETTINGS.to_vec()
}

fn default_pseudo_header_order() -> Vec<String> {
    PSEUDO_HEADER_ORDER_CHROME_136
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

fn default_header_order() -> Vec<String> {
    HEADER_ORDER_CHROME_136.iter().map(|s| (*s).to_string()).collect()
}

impl Default for TransportProfile {
    fn default() -> Self {
        Self {
            name: default_profile_name(),
            expected_http2_settings: default_http2_settings(),
            expected_pseudo_header_order: default_pseudo_header_order(),
            expected_header_order: default_header_order(),
            expectations: Http2Expectations::default(),
            require_http2_observations: false,
        }
    }
}

impl TransportProfile {
    /// Build a profile whose references match the Chrome 136 capture.
    ///
    /// Convenience constructor for tests and the Chrome 136 default
    /// path. The behaviour is identical to
    /// [`TransportProfile::default`].
    #[must_use]
    pub fn chrome_136_reference() -> Self {
        Self::default()
    }

    /// Replace the profile name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Replace the HTTP/2 SETTINGS reference.
    #[must_use]
    pub fn with_http2_settings(mut self, settings: Vec<(u32, u32)>) -> Self {
        self.expected_http2_settings = settings;
        self
    }

    /// Replace the HTTP/2 pseudo-header order reference.
    #[must_use]
    pub fn with_pseudo_header_order(mut self, order: Vec<String>) -> Self {
        self.expected_pseudo_header_order = order;
        self
    }

    /// Replace the HTTP/2 header order reference.
    #[must_use]
    pub fn with_header_order(mut self, order: Vec<String>) -> Self {
        self.expected_header_order = order;
        self
    }

    /// Toggle the `require_http2_observations` flag.
    #[must_use]
    pub const fn with_require_http2_observations(mut self, require: bool) -> Self {
        self.require_http2_observations = require;
        self
    }

    /// Replace the expectations bitmask wholesale.
    #[must_use]
    pub const fn with_expectations(mut self, expectations: Http2Expectations) -> Self {
        self.expectations = expectations;
        self
    }

    /// Replace the expectations bitmask from raw flag bits.
    #[must_use]
    pub const fn with_expectation_bits(mut self, bits: u8) -> Self {
        self.expectations = Http2Expectations::from_bits(bits);
        self
    }

    /// `true` when at least one HTTP/2 expectation is enabled.
    #[must_use]
    pub const fn has_any_http2_expectation(&self) -> bool {
        !self.expectations.is_empty()
    }

    /// Number of HTTP/2 expectations enabled.
    #[must_use]
    pub const fn expected_http2_check_count(&self) -> usize {
        self.expectations.count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls_validation::CHROME_136_HTTP2_SETTINGS;

    #[test]
    fn default_profile_matches_chrome_136() {
        let profile = TransportProfile::default();
        assert_eq!(profile.name, "chrome-136");
        assert_eq!(profile.expected_http2_settings, CHROME_136_HTTP2_SETTINGS);
        assert!(profile.has_any_http2_expectation());
        assert_eq!(profile.expected_http2_check_count(), 3);
        assert!(!profile.require_http2_observations);
        assert!(profile.expectations.contains(TransportProfile::SETTINGS));
        assert!(profile
            .expectations
            .contains(TransportProfile::PSEUDO_HEADER_ORDER));
        assert!(profile.expectations.contains(TransportProfile::HEADER_ORDER));
    }

    #[test]
    fn chrome_136_reference_matches_default() {
        assert_eq!(
            TransportProfile::chrome_136_reference(),
            TransportProfile::default()
        );
    }

    #[test]
    fn with_name_replaces_name_only() {
        let profile = TransportProfile::default().with_name("firefox-130");
        assert_eq!(profile.name, "firefox-130");
        assert!(profile.has_any_http2_expectation());
    }

    #[test]
    fn with_expectations_toggles_all_three_flags() {
        let profile = TransportProfile::default()
            .with_expectation_bits(0);
        assert!(!profile.has_any_http2_expectation());
        assert_eq!(profile.expected_http2_check_count(), 0);
    }

    #[test]
    fn require_http2_observations_round_trips_via_serde()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let profile = TransportProfile::default().with_require_http2_observations(true);
        let json = serde_json::to_string(&profile)?;
        let back: TransportProfile = serde_json::from_str(&json)?;
        assert_eq!(profile, back);
        Ok(())
    }

    #[test]
    fn json_round_trip_default_profile() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let p = TransportProfile::default();
        let json = serde_json::to_string(&p)?;
        let back: TransportProfile = serde_json::from_str(&json)?;
        assert_eq!(p, back);
        Ok(())
    }

    #[test]
    fn expectations_bitmask_count_matches_set_bits() {
        let empty = Http2Expectations::from_bits(0);
        assert!(empty.is_empty());
        assert_eq!(empty.count(), 0);

        let settings_only = Http2Expectations::from_bits(TransportProfile::SETTINGS);
        assert_eq!(settings_only.count(), 1);
        assert!(settings_only.contains(TransportProfile::SETTINGS));

        let all = Http2Expectations::from_bits(Http2Expectations::ALL);
        assert_eq!(all.count(), 3);
    }
}
