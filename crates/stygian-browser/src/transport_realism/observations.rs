//! Observation types for the HTTP/2 behaviour checks.
//!
//! The [`TransportObservation`] type aggregates every per-connection
//! transport fingerprint the [`score`][crate::transport_realism::score]
//! function can consume. Each field is optional so callers can
//! supply only the observations they actually captured; missing
//! observations are reflected in the
//! [`TransportCompatibility::coverage`][crate::transport_realism::TransportCompatibility::coverage]
//! marker.

use serde::{Deserialize, Serialize};

use crate::tls_validation::CHROME_136_HTTP2_SETTINGS;

/// Ordered list of HTTP/2 header names Chrome 136 sends on a
/// standard navigation (after the `:method`, `:authority`, `:scheme`,
/// `:path` pseudo-headers).
///
/// Order is observable by the server and forms part of the Akamai /
/// `DataDome` fingerprint.
pub const HEADER_ORDER_CHROME_136: &[&str] = &[
    "host",
    "connection",
    "sec-ch-ua",
    "sec-ch-ua-mobile",
    "sec-ch-ua-platform",
    "user-agent",
    "accept",
    "sec-fetch-site",
    "sec-fetch-mode",
    "sec-fetch-user",
    "sec-fetch-dest",
    "accept-encoding",
    "accept-language",
    "cookie",
];

/// Ordered list of HTTP/2 header names Firefox 130 sends on a
/// standard navigation (after the pseudo-headers).
///
/// Order differs from Chrome in two places (`user-agent` and
/// `cookie` are last in Firefox but interleaved in Chrome).
pub const HEADER_ORDER_FIREFOX_130: &[&str] = &[
    "host",
    "user-agent",
    "accept",
    "accept-language",
    "accept-encoding",
    "connection",
    "cookie",
    "sec-fetch-dest",
    "sec-fetch-mode",
    "sec-fetch-site",
    "sec-fetch-user",
];

/// Expected HTTP/2 pseudo-header order for Chrome 136.
///
/// HTTP/2 requires pseudo-headers to appear before regular headers;
/// Chrome 136 sends them in a stable, observable order.
pub const PSEUDO_HEADER_ORDER_CHROME_136: &[&str] = &[":method", ":authority", ":scheme", ":path"];

/// HTTP/2 SETTINGS frame fingerprint captured from a live connection.
///
/// The tuple mirrors the `(id, value)` layout produced by the
/// existing [`crate::tls_validation::compare_http2_settings`] helper.
pub type Http2SettingsObservation = Vec<(u32, u32)>;

/// Result of comparing an observed header order against a reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderOrderMatch {
    /// Reference header order the observation was compared against.
    pub expected: Vec<String>,
    /// Header order the caller observed.
    pub observed: Vec<String>,
    /// Number of headers in the observed order that appear in the
    /// reference order **at the same position**.
    pub matched_positions: usize,
    /// Number of headers in the reference order that appear anywhere
    /// in the observed order.
    pub matched_set: usize,
    /// Total headers in the reference order.
    pub reference_length: usize,
    /// Total headers in the observed order.
    pub observed_length: usize,
}

impl HeaderOrderMatch {
    /// Position-match ratio in `[0.0, 1.0]`. Returns `0.0` for an
    /// empty reference (avoids NaN from 0/0).
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn position_match_ratio(&self) -> f64 {
        if self.reference_length == 0 {
            return 0.0;
        }
        self.matched_positions as f64 / self.reference_length as f64
    }

    /// Set-match ratio in `[0.0, 1.0]`. Returns `0.0` for an empty
    /// reference.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn set_match_ratio(&self) -> f64 {
        if self.reference_length == 0 {
            return 0.0;
        }
        self.matched_set as f64 / self.reference_length as f64
    }
}

/// Live transport-layer observations consumed by
/// [`score`][crate::transport_realism::score].
///
/// Every field is optional so callers can supply only the
/// observations they actually captured. Missing observations are
/// surfaced in the [`TransportCompatibility::coverage`][crate::transport_realism::TransportCompatibility::coverage]
/// marker.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportObservation {
    /// Observed HTTP/2 SETTINGS frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http2_settings: Option<Http2SettingsObservation>,
    /// Observed HTTP/2 pseudo-header order (e.g. `:method`,
    /// `:authority`, `:scheme`, `:path`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http2_pseudo_header_order: Option<Vec<String>>,
    /// Observed HTTP/2 header order (after pseudo-headers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http2_header_order: Option<Vec<String>>,
    /// HTTP/3 perk text observed from the live connection (already
    /// consumed by `tls_validation::TransportDiagnostic`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http3_perk_text: Option<String>,
    /// HTTP/3 perk hash observed from the live connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http3_perk_hash: Option<String>,
    /// Observed ALPN protocol list (e.g. `["h2", "http/1.1"]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpn_protocols: Option<Vec<String>>,
}

impl TransportObservation {
    /// Build an observation seeded with the supplied HTTP/2 SETTINGS
    /// frame. Used by the unit tests and the integration tests that
    /// compare against the `tls_validation` reference captures.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::tls_validation::CHROME_136_HTTP2_SETTINGS;
    /// use stygian_browser::transport_realism::TransportObservation;
    ///
    /// let obs = TransportObservation::from_settings(CHROME_136_HTTP2_SETTINGS);
    /// assert!(obs.http2_settings.is_some());
    /// ```
    #[must_use]
    pub fn from_settings(settings: &[(u32, u32)]) -> Self {
        Self {
            http2_settings: Some(settings.to_vec()),
            ..Self::default()
        }
    }

    /// Attach a pseudo-header order observation.
    #[must_use]
    pub fn with_pseudo_header_order<I, S>(mut self, order: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.http2_pseudo_header_order = Some(order.into_iter().map(Into::into).collect());
        self
    }

    /// Attach a header order observation.
    #[must_use]
    pub fn with_header_order<I, S>(mut self, order: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.http2_header_order = Some(order.into_iter().map(Into::into).collect());
        self
    }

    /// Attach a single HTTP/3 perk text observation.
    #[must_use]
    pub fn with_http3_perk_text(mut self, text: impl Into<String>) -> Self {
        self.http3_perk_text = Some(text.into());
        self
    }

    /// Attach a single HTTP/3 perk hash observation.
    #[must_use]
    pub fn with_http3_perk_hash(mut self, hash: impl Into<String>) -> Self {
        self.http3_perk_hash = Some(hash.into());
        self
    }

    /// Attach an ALPN protocol list observation.
    #[must_use]
    pub fn with_alpn<I, S>(mut self, protocols: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.alpn_protocols = Some(protocols.into_iter().map(Into::into).collect());
        self
    }

    /// Convenience: build an observation that exactly matches the
    /// Chrome 136 reference captures.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::transport_realism::TransportObservation;
    ///
    /// let obs = TransportObservation::chrome_136_reference();
    /// assert!(obs.http2_settings.is_some());
    /// assert!(obs.http2_header_order.is_some());
    /// assert!(obs.http2_pseudo_header_order.is_some());
    /// assert!(obs.alpn_protocols.is_some());
    /// ```
    #[must_use]
    pub fn chrome_136_reference() -> Self {
        Self {
            http2_settings: Some(CHROME_136_HTTP2_SETTINGS.to_vec()),
            http2_pseudo_header_order: Some(
                PSEUDO_HEADER_ORDER_CHROME_136
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
            ),
            http2_header_order: Some(
                HEADER_ORDER_CHROME_136
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
            ),
            http3_perk_text: None,
            http3_perk_hash: None,
            alpn_protocols: Some(vec!["h2".to_string(), "http/1.1".to_string()]),
        }
    }

    /// `true` when the observation carries any HTTP/2 surface
    /// (`http2_settings`, `http2_pseudo_header_order`, or
    /// `http2_header_order`).
    #[must_use]
    pub const fn has_http2(&self) -> bool {
        self.http2_settings.is_some()
            || self.http2_pseudo_header_order.is_some()
            || self.http2_header_order.is_some()
    }

    /// Number of HTTP/2 observations that were supplied.
    #[must_use]
    pub const fn http2_observation_count(&self) -> usize {
        let mut n = 0;
        if self.http2_settings.is_some() {
            n += 1;
        }
        if self.http2_pseudo_header_order.is_some() {
            n += 1;
        }
        if self.http2_header_order.is_some() {
            n += 1;
        }
        n
    }
}

/// Compare an observed header order against a reference header
/// order, returning a structured [`HeaderOrderMatch`].
///
/// Both lists are lower-cased before comparison so casing mismatches
/// don't inflate the position-count mismatch list.
#[must_use]
pub fn compare_header_order(expected: &[&str], observed: &[String]) -> HeaderOrderMatch {
    let expected_lc: Vec<String> = expected.iter().map(|s| s.to_ascii_lowercase()).collect();
    let observed_lc: Vec<String> = observed.iter().map(|s| s.to_ascii_lowercase()).collect();

    let matched_positions = expected_lc
        .iter()
        .zip(observed_lc.iter())
        .filter(|(a, b)| a == b)
        .count();
    let matched_set = expected_lc
        .iter()
        .filter(|header| observed_lc.iter().any(|o| o == *header))
        .count();

    HeaderOrderMatch {
        expected: expected_lc,
        observed: observed_lc,
        matched_positions,
        matched_set,
        reference_length: expected.len(),
        observed_length: observed.len(),
    }
}

/// Compare an observed pseudo-header order against the Chrome 136
/// reference.
///
/// The reference is the only stable observation we have
/// for pseudo-headers; mismatches fall into "wrong order"
/// rather than "wrong set" because the set is fixed by the
/// HTTP/2 spec.
///
/// Exposed for callers that want to reuse the same matcher the
/// scoring logic uses.
#[must_use]
pub fn compare_pseudo_header_order(observed: &[String]) -> HeaderOrderMatch {
    compare_header_order(PSEUDO_HEADER_ORDER_CHROME_136, observed)
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
    use crate::tls_validation::{CHROME_131_JA3, CHROME_136_HTTP2_SETTINGS};

    #[test]
    fn chrome_136_reference_seed_is_complete() {
        let obs = TransportObservation::chrome_136_reference();
        assert_eq!(
            obs.http2_settings.as_deref(),
            Some(CHROME_136_HTTP2_SETTINGS)
        );
        assert!(obs.has_http2());
        assert_eq!(obs.http2_observation_count(), 3);
        assert_eq!(
            obs.http2_pseudo_header_order.as_deref(),
            Some(PSEUDO_HEADER_ORDER_CHROME_136)
                .map(|s| s.iter().map(|x| (*x).to_string()).collect::<Vec<_>>())
            .as_ref()
            .map(|v| &v[..])
        );
    }

    #[test]
    fn empty_observation_carries_no_http2_signal() {
        let obs = TransportObservation::default();
        assert!(!obs.has_http2());
        assert_eq!(obs.http2_observation_count(), 0);
    }

    #[test]
    fn header_order_position_match_counts_in_order_only() {
        // Swapped order should drop position matches but keep set matches.
        let expected = HEADER_ORDER_CHROME_136;
        let observed: Vec<String> = vec![
            "cookie".into(),
            "accept-language".into(),
            "host".into(),
            "connection".into(),
        ];
        let m = compare_header_order(expected, &observed);
        assert_eq!(m.matched_set, 4);
        assert_eq!(m.matched_positions, 0);
        assert!(m.position_match_ratio() < m.set_match_ratio());
    }

    #[test]
    fn header_order_position_match_perfect_for_chrome_136() {
        let expected = HEADER_ORDER_CHROME_136;
        let observed: Vec<String> = expected.iter().map(|s| (*s).to_string()).collect();
        let m = compare_header_order(expected, &observed);
        assert_eq!(m.matched_positions, expected.len());
        assert_eq!(m.matched_set, expected.len());
        assert!((m.position_match_ratio() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn header_order_position_match_does_not_panic_on_empty_inputs() {
        let m = compare_header_order(&[], &[]);
        assert_eq!(m.matched_positions, 0);
        assert_eq!(m.matched_set, 0);
        assert_eq!(m.reference_length, 0);
        let pos_ratio = m.position_match_ratio();
        assert!(pos_ratio.abs() < 1e-9, "pos_ratio={pos_ratio}");
        let set_ratio = m.set_match_ratio();
        assert!(set_ratio.abs() < 1e-9, "set_ratio={set_ratio}");
    }

    #[test]
    fn pseudo_header_order_matches_chrome_136() {
        let observed: Vec<String> = PSEUDO_HEADER_ORDER_CHROME_136
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let m = compare_pseudo_header_order(&observed);
        assert_eq!(m.matched_positions, PSEUDO_HEADER_ORDER_CHROME_136.len());
    }

    #[test]
    fn from_settings_preserves_order_and_values() {
        let obs = TransportObservation::from_settings(CHROME_136_HTTP2_SETTINGS);
        let settings = obs.http2_settings.expect("settings");
        assert_eq!(settings, CHROME_136_HTTP2_SETTINGS);
    }

    #[test]
    fn builders_chain_and_preserve_previous_fields() {
        let obs = TransportObservation::from_settings(CHROME_136_HTTP2_SETTINGS)
            .with_header_order(HEADER_ORDER_CHROME_136.iter().copied())
            .with_alpn(["h2", "http/1.1"]);
        assert!(obs.http2_settings.is_some());
        assert!(obs.http2_header_order.is_some());
        assert_eq!(
            obs.alpn_protocols.as_deref(),
            Some(&["h2".to_string(), "http/1.1".to_string()][..])
        );
    }

    #[test]
    fn unused_constants_are_reachable() {
        // Reference capture constants must remain reachable so other
        // modules (diagnostic.rs, tls_validation.rs) can keep using them.
        assert!(CHROME_131_JA3.len() == 32);
    }
}
