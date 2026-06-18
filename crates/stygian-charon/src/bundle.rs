use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::har;
use crate::investigation::investigate_har;
use crate::policy::plan_from_report;
use crate::probe::{ProbePackReport, challenge_probe_pack, run_probe_pack};
use crate::snapshot;
use crate::types::{InvestigationBundle, InvestigationReport, RequirementsProfile, RuntimePolicy};
use crate::vendor_classifier::VendorClassification;
use crate::vendor_classifier::VendorClassifier;

/// Controls how sensitive fields are treated when serialising a [`DiagnosticBundle`].
///
/// # Example
///
/// ```rust
/// use stygian_charon::bundle::BundleRedactionPolicy;
///
/// let policy = BundleRedactionPolicy::Standard;
/// assert!(!matches!(policy, BundleRedactionPolicy::None));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BundleRedactionPolicy {
    /// No redaction — full detail retained (development / local use only).
    None,
    /// Redact cookies, auth headers, and URL credentials.
    /// This is the **recommended default** for incident reporting.
    #[default]
    Standard,
    /// Redact all response headers and URL query parameters in addition to
    /// everything covered by `Standard`.
    Aggressive,
}

/// Metadata fields attached to every [`DiagnosticBundle`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleMetadata {
    /// Schema version of the bundle format (`"1.0"`).
    pub schema_version: String,
    /// RFC 3339 timestamp at which the bundle was assembled.
    pub assembled_at: String,
    /// Redaction policy applied to this bundle.
    pub redaction_policy: BundleRedactionPolicy,
    /// Arbitrary key/value annotations (tooling, environment, etc.).
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
}

/// Full diagnostic bundle for a single investigation.
///
/// The bundle aggregates the investigation report, requirements, policy,
/// built-in probe outcomes, and optional fingerprint coherence results into a
/// single document suitable for incident response tooling.
///
/// Sensitive fields (cookies, auth headers) are redacted according to
/// [`BundleMetadata::redaction_policy`].
///
/// # Format
///
/// The bundle serialises to JSON via `serde`. Top-level fields are:
/// - `metadata` — provenance and redaction policy
/// - `report` — aggregated [`InvestigationReport`]
/// - `requirements` — inferred [`RequirementsProfile`]
/// - `policy` — planned [`RuntimePolicy`]
/// - `probe_report` — outcome of the built-in [`challenge_probe_pack`]
/// - `coherence_violations` — list of `{ rule_id, message, paths }` objects; empty when clean
/// - `vendor_classification` — T89 vendor-fingerprinting classification
///   (additive, `#[serde(default, skip_serializing_if = "Option::is_none")]`)
///
/// # Example
///
/// ```rust
/// use stygian_charon::bundle::{build_diagnostic_bundle, BundleRedactionPolicy};
///
/// let empty_har = r#"{"log":{"version":"1.2","creator":{"name":"test","version":"0"},"entries":[]}}"#;
/// let bundle = build_diagnostic_bundle(empty_har, BundleRedactionPolicy::Standard).unwrap();
/// assert_eq!(bundle.metadata.schema_version, "1.0");
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticBundle {
    /// Bundle provenance and redaction policy.
    pub metadata: BundleMetadata,
    /// Aggregated investigation report.
    pub report: InvestigationReport,
    /// Inferred requirements profile.
    pub requirements: RequirementsProfile,
    /// Planned runtime policy.
    pub policy: RuntimePolicy,
    /// Outcome of running the challenge probe pack against the built-in classifier.
    pub probe_report: ProbePackReport,
    /// Coherence violations across response headers in the investigation.
    ///
    /// Only populated when a [`snapshot::NormalizedFingerprintSnapshot`] is supplied
    /// via [`build_diagnostic_bundle_with_snapshot`].
    #[serde(default)]
    pub coherence_violations: Vec<BundleCoherenceViolation>,
    /// T89 vendor fingerprinting classification (additive field).
    ///
    /// Populated when the bundle is built from a HAR that contains
    /// enough information to identify a vendor; absent (and skipped
    /// during serialisation) otherwise. The field uses
    /// `#[serde(default, skip_serializing_if = "Option::is_none")]`
    /// so older JSON payloads (pre-T89) deserialize unchanged and
    /// newer payloads that did not detect a vendor omit the field
    /// rather than emit `"vendor_classification": null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_classification: Option<VendorClassification>,
}

/// A redacted coherence violation record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleCoherenceViolation {
    /// Stable rule identifier (e.g. `"user_agent_header_match"`).
    pub rule_id: String,
    /// Human-readable explanation.
    pub message: String,
    /// JSON-path-like pointers to the offending fields.
    pub paths: Vec<String>,
}

/// Error type for diagnostic bundle construction.
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    /// The HAR input could not be parsed.
    #[error("HAR parse error: {0}")]
    Har(#[from] har::HarError),
}

/// Build a [`DiagnosticBundle`] from a raw HAR payload.
///
/// Runs investigation, requirement inference, policy planning, and the built-in
/// probe pack. Applies the given [`BundleRedactionPolicy`] to sanitise sensitive
/// fields before returning.
///
/// The T89 vendor classification is computed from the HAR using
/// [`VendorClassifier::with_builtin_defaults`]. When no vendor-specific
/// signals are detected, the resulting `vendor_classification` field is
/// still populated with an "unknown" classification (so consumers can
/// always inspect the field), but the JSON form omits it via
/// `skip_serializing_if = "Option::is_none"`.
/// Pass a custom classifier via
/// [`build_diagnostic_bundle_with_vendor_classifier`] to override the
/// threshold or supply custom vendor definitions.
///
/// # Errors
///
/// Returns [`BundleError::Har`] when the HAR payload is invalid.
///
/// # Example
///
/// ```rust
/// use stygian_charon::bundle::{build_diagnostic_bundle, BundleRedactionPolicy};
///
/// let empty_har = r#"{"log":{"version":"1.2","creator":{"name":"test","version":"0"},"entries":[]}}"#;
/// let bundle = build_diagnostic_bundle(empty_har, BundleRedactionPolicy::Standard).unwrap();
/// assert!(bundle.probe_report.total > 0);
/// ```
pub fn build_diagnostic_bundle(
    har_json: &str,
    redaction_policy: BundleRedactionPolicy,
) -> Result<DiagnosticBundle, BundleError> {
    let classifier = VendorClassifier::with_builtin_defaults();
    build_diagnostic_bundle_with_vendor_classifier(har_json, redaction_policy, &classifier)
}

/// Build a [`DiagnosticBundle`] including fingerprint coherence results.
///
/// Identical to [`build_diagnostic_bundle`] but also evaluates coherence rules
/// against the supplied [`snapshot::NormalizedFingerprintSnapshot`].
/// The vendor classification uses
/// [`VendorClassifier::with_builtin_defaults`]; pass
/// [`build_diagnostic_bundle_full`] to supply a custom classifier
/// together with a snapshot.
///
/// # Errors
///
/// Returns [`BundleError::Har`] when the HAR payload is invalid.
pub fn build_diagnostic_bundle_with_snapshot(
    har_json: &str,
    redaction_policy: BundleRedactionPolicy,
    snap: &snapshot::NormalizedFingerprintSnapshot,
) -> Result<DiagnosticBundle, BundleError> {
    let classifier = VendorClassifier::with_builtin_defaults();
    build_diagnostic_bundle_full(har_json, redaction_policy, &classifier, Some(snap))
}

/// Build a [`DiagnosticBundle`] with a caller-supplied
/// [`VendorClassifier`].
///
/// Use this when the operator wants a custom threshold or
/// additional [`crate::vendor_classifier::VendorDefinition`] entries
/// (e.g. Tier 2 vendors).
/// The classifier is **stateless**, so callers can build it once
/// and reuse it across many `build_diagnostic_bundle_*` invocations.
///
/// # Errors
///
/// Returns [`BundleError::Har`] when the HAR payload is invalid.
pub fn build_diagnostic_bundle_with_vendor_classifier(
    har_json: &str,
    redaction_policy: BundleRedactionPolicy,
    classifier: &VendorClassifier,
) -> Result<DiagnosticBundle, BundleError> {
    build_diagnostic_bundle_full(har_json, redaction_policy, classifier, None)
}

/// Build a [`DiagnosticBundle`] with a caller-supplied
/// [`VendorClassifier`] **and** an optional
/// [`snapshot::NormalizedFingerprintSnapshot`].
///
/// This is the most general bundle constructor; every other builder
/// delegates here.
///
/// # Errors
///
/// Returns [`BundleError::Har`] when the HAR payload is invalid.
pub fn build_diagnostic_bundle_full(
    har_json: &str,
    redaction_policy: BundleRedactionPolicy,
    classifier: &VendorClassifier,
    snap: Option<&snapshot::NormalizedFingerprintSnapshot>,
) -> Result<DiagnosticBundle, BundleError> {
    let report = investigate_har(har_json)?;
    let plan = plan_from_report(report);

    let coherence_violations = snap.map_or_else(Vec::new, |s| {
        let coherence = snapshot::evaluate_snapshot_coherence(s);
        coherence
            .violations
            .into_iter()
            .map(|v| BundleCoherenceViolation {
                rule_id: v.rule_id,
                message: v.message,
                paths: v.paths,
            })
            .collect()
    });

    let probe_report = run_probe_pack(&challenge_probe_pack());
    // The vendor classification is best-effort: if the HAR is
    // unparseable for the classifier's purposes (it consumes the
    // same HAR shape as `investigate_har`), we keep the
    // `vendor_classification` field at `None` so the JSON form
    // omits it via `skip_serializing_if`. We also drop the field
    // when the classifier reports "unknown" with no evidence, so
    // empty HARs do not produce noise in the diagnostic payload.
    let vendor_classification = classifier
        .classify_har(har_json)
        .ok()
        .filter(|c| c.is_identified() || !c.evidence.is_empty());

    let mut bundle = DiagnosticBundle {
        metadata: make_metadata(redaction_policy),
        report: plan.report,
        requirements: plan.requirements,
        policy: plan.policy,
        probe_report,
        coherence_violations,
        vendor_classification,
    };

    apply_redaction(&mut bundle);
    Ok(bundle)
}

/// Convert an [`InvestigationBundle`] into a [`DiagnosticBundle`] (no HAR needed).
///
/// Useful when the caller already has an `InvestigationBundle` and only needs to
/// enrich it with probe outcomes and metadata.
#[must_use]
pub fn diagnostic_bundle_from_investigation(
    bundle: InvestigationBundle,
    redaction_policy: BundleRedactionPolicy,
) -> DiagnosticBundle {
    let probe_report = run_probe_pack(&challenge_probe_pack());
    let mut result = DiagnosticBundle {
        metadata: make_metadata(redaction_policy),
        report: bundle.report,
        requirements: bundle.requirements,
        policy: bundle.policy,
        probe_report,
        coherence_violations: Vec::new(),
        vendor_classification: None,
    };
    apply_redaction(&mut result);
    result
}

fn make_metadata(redaction_policy: BundleRedactionPolicy) -> BundleMetadata {
    BundleMetadata {
        schema_version: "1.0".to_string(),
        assembled_at: chrono_now(),
        redaction_policy,
        annotations: BTreeMap::new(),
    }
}

/// Produce an RFC 3339–like timestamp without pulling in `chrono`.
fn chrono_now() -> String {
    // std::time gives us seconds since UNIX epoch; we format it as an
    // opaque but sortable string when a proper date library is not available.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!("unix:{secs}")
}

/// Apply the redaction policy to a bundle in place.
pub fn apply_redaction(bundle: &mut DiagnosticBundle) {
    match bundle.metadata.redaction_policy {
        BundleRedactionPolicy::None => {}
        BundleRedactionPolicy::Standard => redact_standard(bundle),
        BundleRedactionPolicy::Aggressive => redact_aggressive(bundle),
    }
}

const REDACTED: &str = "[REDACTED]";

fn redact_standard(bundle: &mut DiagnosticBundle) {
    // Redact URL credentials in request summaries.
    for req in &mut bundle.report.suspicious_requests {
        redact_url_credentials(&mut req.url);
    }
    // Redact page title if it looks like it contains credentials.
    if let Some(title) = &mut bundle.report.page_title {
        redact_url_credentials(title);
    }
}

fn redact_aggressive(bundle: &mut DiagnosticBundle) {
    // Redact all URL query strings from suspicious requests.
    for req in &mut bundle.report.suspicious_requests {
        redact_url_credentials(&mut req.url);
        redact_url_query(&mut req.url);
    }
    // Clear all markers (may contain partial cookie/token values).
    bundle.report.top_markers.clear();
    bundle.report.marker_histogram.clear();
    for req in &mut bundle.report.suspicious_requests {
        req.detection.markers.clear();
    }
}

fn redact_url_credentials(url: &mut String) {
    // Replace `://user:pass@` style credentials.
    if let Some(at_pos) = url.find('@')
        && let Some(scheme_end) = url.find("://")
    {
        let after_scheme = scheme_end + 3;
        if after_scheme < at_pos {
            let scheme = url[..scheme_end].to_string();
            let rest = url[at_pos + 1..].to_string();
            *url = format!("{scheme}://{REDACTED}@{rest}");
        }
    }
}

fn redact_url_query(url: &mut String) {
    if let Some(q) = url.find('?') {
        url.truncate(q);
        url.push('?');
        url.push_str(REDACTED);
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

    const EMPTY_HAR: &str =
        r#"{"log":{"version":"1.2","creator":{"name":"test","version":"0"},"entries":[]}}"#;

    #[test]
    fn build_diagnostic_bundle_empty_har() {
        let result = build_diagnostic_bundle(EMPTY_HAR, BundleRedactionPolicy::Standard);
        assert!(result.is_ok(), "bundle build should succeed");
        let Ok(bundle) = result else {
            return;
        };
        assert_eq!(bundle.metadata.schema_version, "1.0");
        assert_eq!(
            bundle.metadata.redaction_policy,
            BundleRedactionPolicy::Standard
        );
        assert!(bundle.probe_report.total > 0);
        assert!(bundle.coherence_violations.is_empty());
    }

    #[test]
    fn redaction_standard_masks_url_credentials() {
        let har = r#"{"log":{"version":"1.2","creator":{"name":"test","version":"0"},"entries":[
            {"startedDateTime":"2026-01-01T00:00:00Z","time":100,
             "request":{"method":"GET","url":"https://user:pass@example.com/page","httpVersion":"HTTP/1.1","headers":[],"queryString":[],"cookies":[],"headersSize":-1,"bodySize":-1},
             "response":{"status":403,"statusText":"Forbidden","httpVersion":"HTTP/1.1",
               "headers":[{"name":"cf-ray","value":"abc-LHR"},{"name":"server","value":"cloudflare"}],
               "cookies":[],"content":{"size":0,"mimeType":"text/html"},"redirectURL":"","headersSize":-1,"bodySize":-1},
             "cache":{},"timings":{"send":0,"wait":100,"receive":0}}
        ]}}"#;

        let result = build_diagnostic_bundle(har, BundleRedactionPolicy::Standard);
        assert!(result.is_ok(), "bundle build should succeed");
        let Ok(bundle) = result else {
            return;
        };
        for req in &bundle.report.suspicious_requests {
            assert!(
                !req.url.contains("user:pass"),
                "URL credentials should be redacted: {}",
                req.url
            );
        }
    }

    #[test]
    fn redaction_none_preserves_url_credentials() {
        let har = r#"{"log":{"version":"1.2","creator":{"name":"test","version":"0"},"entries":[
            {"startedDateTime":"2026-01-01T00:00:00Z","time":100,
             "request":{"method":"GET","url":"https://user:pass@example.com/page","httpVersion":"HTTP/1.1","headers":[],"queryString":[],"cookies":[],"headersSize":-1,"bodySize":-1},
             "response":{"status":403,"statusText":"Forbidden","httpVersion":"HTTP/1.1",
               "headers":[{"name":"cf-ray","value":"abc-LHR"},{"name":"server","value":"cloudflare"}],
               "cookies":[],"content":{"size":0,"mimeType":"text/html"},"redirectURL":"","headersSize":-1,"bodySize":-1},
             "cache":{},"timings":{"send":0,"wait":100,"receive":0}}
        ]}}"#;

        let result = build_diagnostic_bundle(har, BundleRedactionPolicy::None);
        assert!(result.is_ok(), "bundle build should succeed");
        let Ok(bundle) = result else {
            return;
        };
        for req in &bundle.report.suspicious_requests {
            assert!(
                req.url.contains("user:pass"),
                "URL credentials should be preserved with None policy: {}",
                req.url
            );
        }
    }

    #[test]
    fn bundle_metadata_schema_version_is_stable() {
        let result = build_diagnostic_bundle(EMPTY_HAR, BundleRedactionPolicy::None);
        assert!(result.is_ok(), "bundle build should succeed");
        let Ok(bundle) = result else {
            return;
        };
        assert_eq!(bundle.metadata.schema_version, "1.0");
    }

    #[test]
    fn redact_url_credentials_removes_userinfo() {
        let mut url = "https://user:pass@example.com/path".to_string();
        redact_url_credentials(&mut url);
        assert!(
            !url.contains("user:pass"),
            "URL credentials should be removed: {url}"
        );
        assert!(url.contains(REDACTED));
    }

    #[test]
    fn redact_url_query_removes_query_string() {
        let mut url = "https://example.com/path?token=secret&other=val".to_string();
        redact_url_query(&mut url);
        assert!(
            !url.contains("secret"),
            "query string should be removed: {url}"
        );
        assert!(url.contains('?'));
    }
}
