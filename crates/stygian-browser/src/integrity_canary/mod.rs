//! JavaScript integrity trap canary probes.
//!
//! ## What is a "JavaScript integrity trap"?
//!
//! Modern anti-bot vendors (`Cloudflare`, `DataDome`, `PerimeterX`,
//! Akamai Bot Manager, `Kasada`) ship detection scripts that look for
//! artefacts left by **patched** browser surfaces — places where a
//! stealth framework rewrote a native prototype, getter, or accessor
//! and the patch is detectable from the JavaScript side. Classic
//! examples:
//!
//! - `Object.getOwnPropertyDescriptor(Navigator.prototype, "webdriver")`
//!   returning a **data property** instead of a native **accessor**.
//! - `Function.prototype.toString` showing patch source code (e.g.
//!   `"function webdriver() { [native code] }"`) when applied to a
//!   patched prototype method.
//! - `Performance.now` returning suspiciously round / quantized
//!   values that betray deterministic jitter injection.
//!
//! The traps come in two shapes:
//!
//! - **Suspected** — the surface shape is unusual but the signal is
//!   ambiguous (e.g. a non-`native code` `toString` on a polyfill
//!   that the user's browser already shipped).
//! - **Confirmed** — the surface shape is only achievable via a
//!   stealth framework patch on a real browser (e.g. `webdriver`
//!   is a **data** property on `Navigator.prototype`).
//!
//! ## What this module provides
//!
//! 1. A stable [`IntegrityProbe`] catalogue with weighted risk
//!    contributions and per-probe mitigation hints (see
//!    [`probes::all_probes`]).
//! 2. A pure-Rust scoring pipeline ([`report::IntegrityRiskScore`])
//!    that turns a set of [`probes::ProbeFinding`] records into an
//!    aggregate score and a documented **Suspected** vs **Confirmed**
//!    classification.
//! 3. A trend-detection seam ([`trend::CanaryTrendObservation`])
//!    that future canary infrastructure (T84) can subscribe to
//!    without modifying the probe set.
//!
//! ## Probe catalogue
//!
//! The default probe set (see [`probes::all_probes`]) covers eight
//! surfaces:
//!
//! | Probe | Default weight | What it checks |
//! |---|---|---|
//! | [`probes::IntegrityProbeId::WebDriverDescriptorNative`] | 0.20 | `Navigator.prototype.webdriver` accessor shape |
//! | [`probes::IntegrityProbeId::FunctionToStringNative`]      | 0.18 | `Function.prototype.toString` reports `[native code]` for patched natives |
//! | [`probes::IntegrityProbeId::ErrorToStringNative`]        | 0.08 | `(function(){}).toString()` reports `[native code]` |
//! | [`probes::IntegrityProbeId::IntlDateTimeFormatNative`]   | 0.10 | `Intl.DateTimeFormat.prototype.format` is native |
//! | [`probes::IntegrityProbeId::RegExpTestNative`]          | 0.08 | `RegExp.prototype.test` is native |
//! | [`probes::IntegrityProbeId::CanvasGetImageDataNative`]  | 0.10 | `CanvasRenderingContext2D.prototype.getImageData` is native |
//! | [`probes::IntegrityProbeId::PerformanceNowResolution`]   | 0.14 | `performance.now()` resolution is plausible (not quantized) |
//! | [`probes::IntegrityProbeId::ProxyTrapObservable`]       | 0.12 | `Proxy` traps on patched natives do not leak surface state |
//!
//! ## Feature flag
//!
//! This module is **default-on** and is always compiled as part of
//! the `stygian-browser` crate. The probe set and scoring pipeline
//! are pure Rust with **no I/O** so they are safely callable in
//! deterministic tests without booting Chrome.
//!
//! ## Integration with the existing diagnostic payload
//!
//! The canary report attaches additively to
//! [`crate::diagnostic::DiagnosticReport`] via
//! [`crate::diagnostic::DiagnosticReport::with_integrity_canary`]
//! (added in this task) so downstream automation can consume the
//! finding set without breaking the legacy schema.
//!
//! ## Reuse of the canary trend pipeline (T84)
//!
//! T84 will add a stealth canary hard-gate. This module exposes
//! [`trend::CanaryTrendObservation`] as the **stable seam** that
//! future canary infrastructure can consume without changing probe
//! definitions: each observation carries the normalized risk score
//! and a deterministic `signature` string so two reports with the
//! same findings produce byte-identical trend entries.
//!
//! # Example
//!
//! ```
//! use stygian_browser::integrity_canary::{
//!     IntegrityCanaryReport, IntegrityProbe, IntegrityRiskClassification,
//! };
//!
//! // Simulate a probe set where two probes fired with confirmed traps.
//! let finding_a = IntegrityProbe::confirmed_finding(
//!     "webdriver_descriptor_native",
//!     0.20,
//!     "Navigator.prototype.webdriver is a data property (should be an accessor)",
//! );
//! let finding_b = IntegrityProbe::confirmed_finding(
//!     "performance_now_resolution",
//!     0.14,
//!     "performance.now() values are quantized to 0.1 ms (timing-noise injection)",
//! );
//!
//! let report = IntegrityCanaryReport::from_findings(vec![finding_a, finding_b]);
//! assert!(report.score.value() > 0.0);
//! assert!(matches!(
//!     report.score.classification(),
//!     IntegrityRiskClassification::Confirmed | IntegrityRiskClassification::Suspected
//! ));
//! assert_eq!(report.findings.len(), 2);
//! ```

mod probes;
mod report;
mod trend;

pub use probes::{
    IntegrityProbe, IntegrityProbeId, IntegrityProbeOutcome, ProbeFinding, all_probes, probe_by_id,
};
pub use report::{
    IntegrityCanaryPolicy, IntegrityCanaryReport, IntegrityRiskClassification, IntegrityRiskScore,
    RISK_CONFIRMED_THRESHOLD_DEFAULT, RISK_SUSPECTED_THRESHOLD_DEFAULT,
};
pub use trend::{CanaryTrendObservation, TrendSeverity};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_exports_are_reachable_from_crate_root() {
        // The crate-root re-exports below would fail to compile if
        // the module structure diverges from the public API contract.
        let _report = IntegrityCanaryReport::from_findings(Vec::new());
        let _probes = all_probes();
        let _policy = IntegrityCanaryPolicy::default();
        let _score = IntegrityRiskScore::clean();
    }

    #[test]
    fn default_thresholds_are_documented_values() {
        assert!(
            (RISK_SUSPECTED_THRESHOLD_DEFAULT - 0.30).abs() < 1e-9,
            "suspected threshold must be 0.30 by default, got: {RISK_SUSPECTED_THRESHOLD_DEFAULT}"
        );
        assert!(
            (RISK_CONFIRMED_THRESHOLD_DEFAULT - 0.65).abs() < 1e-9,
            "confirmed threshold must be 0.65 by default, got: {RISK_CONFIRMED_THRESHOLD_DEFAULT}"
        );
    }
}
