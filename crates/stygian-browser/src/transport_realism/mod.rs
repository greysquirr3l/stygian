//! Transport-layer realism expansion (HTTP/2 + HTTP/3 surfaces).
//!
//! This module sits on top of the existing
//! [`tls_validation`][crate::tls_validation] layer (T46) and adds the
//! HTTP/2 behaviour checks anti-bot vendors observe in the wild:
//!
//! - HTTP/2 SETTINGS frame fingerprint (initial values Chrome, Firefox,
//!   Safari send on every connection).
//! - HTTP/2 header order (the order browsers send `:method`,
//!   `:authority`, `:scheme`, `:path`, then standard headers is part of
//!   the Akamai / `DataDome` fingerprint).
//! - HTTP/2 pseudo-header order (`:method` before `:authority` before
//!   `:scheme` before `:path` is the Chrome 136 ordering).
//! - HTTP/3 perk fingerprint (already produced by the existing
//!   `tls_validation` module — this module consumes it).
//!
//! All checks share the same scoring interface:
//! [`TransportCompatibility::score`] produces a normalised
//! compatibility score in `[0.0, 1.0]`, with confidence/coverage
//! markers that tell the caller how much of the surface was actually
//! observable. When HTTP/2 observations are unavailable (e.g. the
//! caller is running a non-HTTP/2 path or a live capture failed),
//! the score collapses to a deterministic neutral value and the
//! coverage marker reflects that no HTTP/2 observations were
//! supplied — see
//! [`DEFAULT_COVERAGE_WHEN_HTTP2_UNAVAILABLE`] and
//! [`DEFAULT_CONFIDENCE_WHEN_HTTP2_UNAVAILABLE`].
//!
//! ## Feature flag
//!
//! This module is **default-on** and is always compiled as part of
//! the `stygian-browser` crate. The runner surface
//! ([`AcquisitionRequest::transport_realism`][crate::acquisition::AcquisitionRequest::transport_realism])
//! is additive — callers that do not pass a context see the same
//! runner behaviour they saw before this task landed.
//!
//! ## Integration with the `AcquisitionRunner`
//!
//! [`TransportProfile`] is a typed config field that callers can attach
//! to an [`AcquisitionRequest`][crate::acquisition::AcquisitionRequest].
//! When present, the runner records the resulting
//! [`TransportCompatibility`] on the
//! [`AcquisitionResult::transport_realism`][crate::acquisition::AcquisitionResult::transport_realism]
//! field as a [`TransportRealismReport`] so downstream policy mapping
//! (T83 / T85 / T89 / T93) can consume it as a strategy hint.
//!
//! ## Default behaviour
//!
//! - `TransportProfile::default()` enables every observable check and
//!   marks HTTP/2 as required.
//! - When no HTTP/2 observations are supplied, the score is `0.0` and
//!   coverage is `0.0` (clearly marked as such). Callers that always
//!   have HTTP/2 captures should override `require_http2_observations`
//!   to detect partial instrumentation and short-circuit the run.
//!
//! # Example
//!
//! ```
//! use stygian_browser::tls_validation::CHROME_136_HTTP2_SETTINGS;
//! use stygian_browser::transport_realism::{
//!     score, TransportObservation, TransportProfile, HEADER_ORDER_CHROME_136,
//!     PSEUDO_HEADER_ORDER_CHROME_136,
//! };
//!
//! // Live capture that exactly matches Chrome 136 → score 1.0
//! let obs = TransportObservation::from_settings(CHROME_136_HTTP2_SETTINGS)
//!     .with_header_order(HEADER_ORDER_CHROME_136.iter().copied())
//!     .with_pseudo_header_order(PSEUDO_HEADER_ORDER_CHROME_136.iter().copied());
//! let report = score(&TransportProfile::default(), &obs);
//! assert!(report.compatibility.score > 0.95);
//! assert!(report.compatibility.is_high_confidence());
//! ```

mod observations;
mod profile;
mod report;
mod scoring;

pub use observations::{
    HEADER_ORDER_CHROME_136, HEADER_ORDER_FIREFOX_130, HeaderOrderMatch, Http2SettingsObservation,
    PSEUDO_HEADER_ORDER_CHROME_136, TransportObservation, compare_header_order,
    compare_pseudo_header_order,
};
pub use profile::{Http2Expectations, TransportProfile};
pub use report::{TransportCompatibility, TransportRealismReport};
pub use scoring::{HTTP2_CHECK_KIND_COUNT, Http2CheckKind, Http2CheckResult, score};

use std::fmt;

/// Default confidence score returned when no HTTP/2 observations are
/// available.
///
/// The score is `0.0` because no comparison could be made — the
/// caller has no signal that the observed transport is realistic for
/// the target profile.
pub const DEFAULT_CONFIDENCE_WHEN_HTTP2_UNAVAILABLE: f64 = 0.0;

/// Default coverage score returned when no HTTP/2 observations are
/// available.
///
/// Coverage is `0.0` because none of the HTTP/2 checks could be
/// executed. Callers can compare against this constant to detect
/// "missing instrumentation" rather than "low but real coverage".
pub const DEFAULT_COVERAGE_WHEN_HTTP2_UNAVAILABLE: f64 = 0.0;

/// Default compatibility score returned when no HTTP/2 observations
/// are available.
///
/// The score is `0.0` because no observation could be compared — the
/// runner treats the surface as "no signal" rather than "matches".
pub const DEFAULT_SCORE_WHEN_HTTP2_UNAVAILABLE: f64 = 0.0;

/// Errors produced by transport-realism helpers.
#[derive(Debug)]
pub enum TransportRealismError {
    /// A supplied header order or pseudo-header order is empty.
    EmptyHeaderOrder,
    /// A header order observation contained a duplicate header name.
    DuplicateHeader,
}

impl fmt::Display for TransportRealismError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyHeaderOrder => f.write_str("transport realism: empty header order"),
            Self::DuplicateHeader => f.write_str("transport realism: duplicate header in order"),
        }
    }
}

impl std::error::Error for TransportRealismError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls_validation::CHROME_136_HTTP2_SETTINGS;

    #[test]
    fn module_exports_are_reachable_from_crate_root() {
        // The crate-root re-exports below would fail to compile if the
        // module structure diverged from the public API contract.
        let _profile = TransportProfile::default();
        let obs = TransportObservation::from_settings(CHROME_136_HTTP2_SETTINGS);
        let _ = score(&TransportProfile::default(), &obs);
    }

    #[test]
    fn default_constants_are_stable() {
        assert!(DEFAULT_CONFIDENCE_WHEN_HTTP2_UNAVAILABLE.abs() < 1e-9);
        assert!(DEFAULT_COVERAGE_WHEN_HTTP2_UNAVAILABLE.abs() < 1e-9);
        assert!(DEFAULT_SCORE_WHEN_HTTP2_UNAVAILABLE.abs() < 1e-9);
    }
}
