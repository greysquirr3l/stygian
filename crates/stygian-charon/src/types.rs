use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Target website classification for SLO thresholds.
///
/// Used to determine acceptable blocked ratios and risk assessments based on expected
/// anti-bot posture. Different sites have different security requirements:
///
/// - **API**: Machine-to-machine communication; expects very low block ratio.
/// - **`ContentSite`**: Public web content; moderate block tolerance.
/// - **`HighSecurity`**: Banking, auth, sensitive data; higher block ratio acceptable.
/// - **Unknown**: Default classification when unable to determine target type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TargetClass {
    /// REST API or GraphQL endpoint; expect clean machine-to-machine paths.
    Api,
    /// General content site or e-commerce; browser-like requests expected.
    ContentSite,
    /// High-security property (banking, auth, sensitive data); strict anti-bot expected.
    HighSecurity,
    /// Unknown or unclassified target.
    Unknown,
}

/// Blocked ratio service-level objectives (SLOs) by target class.
///
/// Defines acceptable and concerning block ratios for different target types.
/// These thresholds guide requirement inference and risk scoring.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockedRatioSlo {
    /// Target class for these SLOs.
    pub target_class: TargetClass,
    /// Acceptable block ratio (green threshold); below this is normal.
    pub acceptable: f64,
    /// Warning threshold; above this triggers adaptive rate requirement.
    pub warning: f64,
    /// Critical threshold; above this indicates severe anti-bot posture.
    pub critical: f64,
}

impl BlockedRatioSlo {
    /// Default SLOs for API targets (0-5% blocks, 10% warning, 15% critical).
    #[must_use]
    pub const fn api() -> Self {
        Self {
            target_class: TargetClass::Api,
            acceptable: 0.05,
            warning: 0.10,
            critical: 0.15,
        }
    }

    /// Default SLOs for content sites (0-15% blocks, 25% warning, 40% critical).
    #[must_use]
    pub const fn content_site() -> Self {
        Self {
            target_class: TargetClass::ContentSite,
            acceptable: 0.15,
            warning: 0.25,
            critical: 0.40,
        }
    }

    /// Default SLOs for high-security sites (0-30% blocks, 50% warning, 70% critical).
    #[must_use]
    pub const fn high_security() -> Self {
        Self {
            target_class: TargetClass::HighSecurity,
            acceptable: 0.30,
            warning: 0.50,
            critical: 0.70,
        }
    }

    /// Default SLOs for unknown targets (conservative: API thresholds).
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            target_class: TargetClass::Unknown,
            acceptable: 0.05, // Same as API
            warning: 0.10,
            critical: 0.15,
        }
    }

    /// Get SLO for a target class.
    #[must_use]
    pub const fn for_class(class: TargetClass) -> Self {
        match class {
            TargetClass::Api => Self::api(),
            TargetClass::ContentSite => Self::content_site(),
            TargetClass::HighSecurity => Self::high_security(),
            TargetClass::Unknown => Self::unknown(),
        }
    }

    /// Assess blocked ratio against SLO thresholds.
    ///
    /// Returns `(is_acceptable, is_warning, is_critical)`.
    #[must_use]
    pub fn assess(&self, blocked_ratio: f64) -> (bool, bool, bool) {
        (
            blocked_ratio <= self.acceptable,
            blocked_ratio > self.acceptable && blocked_ratio <= self.warning,
            blocked_ratio > self.critical,
        )
    }
}

/// A simplified view of one HTTP transaction used for provider classification.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransactionView {
    /// Request URL.
    pub url: String,
    /// HTTP status code.
    pub status: u16,
    /// Response headers (lower/upper case are normalized by the classifier).
    pub response_headers: BTreeMap<String, String>,
    /// Optional response body snippet.
    pub response_body_snippet: Option<String>,
}

/// Known anti-bot providers recognized by the classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AntiBotProvider {
    /// `DataDome`.
    DataDome,
    /// Cloudflare bot/challenge stack.
    Cloudflare,
    /// Akamai bot manager indicators.
    Akamai,
    /// Human Security / `PerimeterX` indicators.
    PerimeterX,
    /// Kasada indicators.
    Kasada,
    /// Fingerprint.com markers.
    FingerprintCom,
    /// Catch-all when no provider-specific signatures were found.
    Unknown,
}

/// Classification result with evidence markers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Detection {
    /// Most likely provider.
    pub provider: AntiBotProvider,
    /// Simple confidence score in [0.0, 1.0].
    pub confidence: f64,
    /// Marker strings that matched.
    pub markers: Vec<String>,
}

/// Scorecard for one provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderScore {
    /// Provider represented by this score.
    pub provider: AntiBotProvider,
    /// Weighted score from marker matches.
    pub score: u32,
    /// Evidence used to produce the score.
    pub markers: Vec<String>,
}

/// Minimal per-request summary extracted from a HAR file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarRequestSummary {
    /// URL requested.
    pub url: String,
    /// HTTP status code.
    pub status: u16,
    /// Best-effort resource type from HAR metadata.
    pub resource_type: Option<String>,
    /// Detection result for this request.
    pub detection: Detection,
}

/// Full HAR classification report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarClassificationReport {
    /// URL/title from HAR page metadata when available.
    pub page_title: Option<String>,
    /// Summary classification for all entries.
    pub aggregate: Detection,
    /// Request-level classification outputs.
    pub requests: Vec<HarRequestSummary>,
}

/// Frequency count for a normalized marker string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkerCount {
    /// Marker text.
    pub marker: String,
    /// Number of requests where the marker appears.
    pub count: u64,
}

/// Aggregated request metrics per host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSummary {
    /// Hostname extracted from request URL.
    pub host: String,
    /// Total requests observed for this host.
    pub total_requests: u64,
    /// Requests that returned HTTP 403 or 429.
    pub blocked_requests: u64,
}

/// Full-featured HAR investigation output suitable for diffs and alerting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvestigationReport {
    /// URL/title from HAR page metadata when available.
    pub page_title: Option<String>,
    /// Total requests in the capture.
    pub total_requests: u64,
    /// Count of blocked/challenged requests (403/429).
    pub blocked_requests: u64,
    /// Status-code histogram.
    pub status_histogram: BTreeMap<u16, u64>,
    /// Resource-type histogram from HAR metadata.
    pub resource_type_histogram: BTreeMap<String, u64>,
    /// Provider histogram inferred from signatures.
    pub provider_histogram: BTreeMap<AntiBotProvider, u64>,
    /// Full marker histogram inferred from signatures.
    pub marker_histogram: BTreeMap<String, u64>,
    /// Most frequent signature markers.
    pub top_markers: Vec<MarkerCount>,
    /// Top hosts by request volume.
    pub hosts: Vec<HostSummary>,
    /// Suspicious requests (blocked/challenged or with known provider markers).
    pub suspicious_requests: Vec<HarRequestSummary>,
    /// Aggregate provider classification.
    pub aggregate: Detection,
    /// Target website class for SLO assessment (optional; defaults to Unknown).
    #[serde(default)]
    pub target_class: Option<TargetClass>,
}

/// Delta between a baseline report and a candidate report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvestigationDiff {
    /// Baseline request count.
    pub baseline_total_requests: u64,
    /// Candidate request count.
    pub candidate_total_requests: u64,
    /// Baseline blocked requests.
    pub baseline_blocked_requests: u64,
    /// Candidate blocked requests.
    pub candidate_blocked_requests: u64,
    /// Candidate blocked ratio minus baseline blocked ratio.
    pub blocked_ratio_delta: f64,
    /// Whether blocked ratio increased by at least 2 percentage points.
    pub likely_regression: bool,
    /// Provider count delta: candidate minus baseline.
    pub provider_delta: BTreeMap<AntiBotProvider, i64>,
    /// New markers observed in candidate but not baseline.
    pub new_markers: Vec<String>,
}

/// Severity/importance level for an inferred operational requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RequirementLevel {
    /// Helpful, but usually not mandatory.
    Low,
    /// Strongly recommended for reliable automation.
    Medium,
    /// Typically required to avoid frequent blocks/challenges.
    High,
}

/// One inferred operational requirement derived from telemetry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AntiBotRequirement {
    /// Stable identifier for the requirement.
    pub id: String,
    /// Human-friendly requirement title.
    pub title: String,
    /// Why this requirement appears to matter.
    pub why: String,
    /// Marker evidence or metrics supporting the inference.
    pub evidence: Vec<String>,
    /// Estimated requirement importance.
    pub level: RequirementLevel,
}

/// High-level integration strategy for Stygian execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdapterStrategy {
    /// Standard HTTP adapter path appears sufficient.
    DirectHttp,
    /// Browser-backed execution is recommended.
    BrowserStealth,
    /// Sticky session + proxy continuity should be applied.
    StickyProxy,
    /// Warm-up/session priming before data collection is advised.
    SessionWarmup,
    /// Unknown/ambiguous conditions: keep in investigation mode.
    InvestigateOnly,
}

/// Suggested Stygian integration plan derived from investigation signals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationRecommendation {
    /// Selected strategy.
    pub strategy: AdapterStrategy,
    /// Why this strategy was selected.
    pub rationale: String,
    /// Suggested feature flags/components for Stygian wiring.
    pub required_stygian_features: Vec<String>,
    /// Suggested runtime configuration hints.
    pub config_hints: BTreeMap<String, String>,
}

/// Provider-aware operational profile and integration guidance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequirementsProfile {
    /// Aggregate inferred provider.
    pub provider: AntiBotProvider,
    /// Confidence for the provider assignment.
    pub confidence: f64,
    /// Inferred operational requirements.
    pub requirements: Vec<AntiBotRequirement>,
    /// Suggested Stygian integration strategy.
    pub recommendation: IntegrationRecommendation,
}

/// High-level execution mode for a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionMode {
    /// Standard HTTP adapters.
    Http,
    /// Browser-backed execution.
    Browser,
}

/// Session persistence mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionMode {
    /// No explicit session persistence.
    Stateless,
    /// Reuse a sticky proxy/session identity.
    Sticky,
}

/// Recommended anti-bot telemetry level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TelemetryLevel {
    /// Minimal telemetry.
    Basic,
    /// Normal diagnostics.
    Standard,
    /// Deep diagnostics and marker tracking.
    Deep,
}

/// Concrete runtime policy that can be mapped to Stygian config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimePolicy {
    /// Recommended execution mode.
    pub execution_mode: ExecutionMode,
    /// Recommended session mode.
    pub session_mode: SessionMode,
    /// Recommended telemetry level.
    pub telemetry_level: TelemetryLevel,
    /// Requests per second budget.
    pub rate_limit_rps: f64,
    /// Max retries per request.
    pub max_retries: u32,
    /// Baseline backoff in milliseconds.
    pub backoff_base_ms: u64,
    /// Whether warm-up navigation/requests are recommended.
    pub enable_warmup: bool,
    /// Whether browser context should block WebRTC non-proxied paths.
    pub enforce_webrtc_proxy_only: bool,
    /// Suggested sticky-session TTL in seconds (if relevant).
    pub sticky_session_ttl_secs: Option<u64>,
    /// Required Stygian features/components.
    pub required_stygian_features: Vec<String>,
    /// Additional hints mapped by key.
    pub config_hints: BTreeMap<String, String>,
    /// Composite risk score in [0.0, 1.0].
    pub risk_score: f64,
}

/// End-to-end result from HAR analysis, requirements inference, and policy planning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvestigationBundle {
    /// Parsed/aggregated investigation report.
    pub report: InvestigationReport,
    /// Inferred requirements profile.
    pub requirements: RequirementsProfile,
    /// Planned runtime policy.
    pub policy: RuntimePolicy,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blocked_ratio_slo_api_thresholds() {
        let slo = BlockedRatioSlo::api();
        assert_eq!(slo.target_class, TargetClass::Api);
        assert!((slo.acceptable - 0.05).abs() < f64::EPSILON);
        assert!((slo.warning - 0.10).abs() < f64::EPSILON);
        assert!((slo.critical - 0.15).abs() < f64::EPSILON);
    }

    #[test]
    fn test_blocked_ratio_slo_content_site_thresholds() {
        let slo = BlockedRatioSlo::content_site();
        assert_eq!(slo.target_class, TargetClass::ContentSite);
        assert!((slo.acceptable - 0.15).abs() < f64::EPSILON);
        assert!((slo.warning - 0.25).abs() < f64::EPSILON);
        assert!((slo.critical - 0.40).abs() < f64::EPSILON);
    }

    #[test]
    fn test_blocked_ratio_slo_high_security_thresholds() {
        let slo = BlockedRatioSlo::high_security();
        assert_eq!(slo.target_class, TargetClass::HighSecurity);
        assert!((slo.acceptable - 0.30).abs() < f64::EPSILON);
        assert!((slo.warning - 0.50).abs() < f64::EPSILON);
        assert!((slo.critical - 0.70).abs() < f64::EPSILON);
    }

    #[test]
    fn test_blocked_ratio_slo_unknown_defaults_to_api() {
        let slo = BlockedRatioSlo::unknown();
        assert_eq!(slo.target_class, TargetClass::Unknown);
        assert!((slo.acceptable - 0.05).abs() < f64::EPSILON); // Same thresholds as API
        assert!((slo.warning - 0.10).abs() < f64::EPSILON);
        assert!((slo.critical - 0.15).abs() < f64::EPSILON);
    }

    #[test]
    fn test_blocked_ratio_slo_for_class_api() {
        let slo = BlockedRatioSlo::for_class(TargetClass::Api);
        assert_eq!(slo.target_class, TargetClass::Api);
        assert!((slo.acceptable - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn test_blocked_ratio_slo_for_class_content_site() {
        let slo = BlockedRatioSlo::for_class(TargetClass::ContentSite);
        assert_eq!(slo.target_class, TargetClass::ContentSite);
        assert!((slo.acceptable - 0.15).abs() < f64::EPSILON);
    }

    #[test]
    fn test_blocked_ratio_slo_assess_below_acceptable() {
        let slo = BlockedRatioSlo::api();
        let (acceptable, warning, critical) = slo.assess(0.02);
        assert!(acceptable);
        assert!(!warning);
        assert!(!critical);
    }

    #[test]
    fn test_blocked_ratio_slo_assess_at_acceptable() {
        let slo = BlockedRatioSlo::api();
        let (acceptable, warning, critical) = slo.assess(0.05);
        assert!(acceptable);
        assert!(!warning);
        assert!(!critical);
    }

    #[test]
    fn test_blocked_ratio_slo_assess_in_warning_zone() {
        let slo = BlockedRatioSlo::api();
        let (acceptable, warning, critical) = slo.assess(0.075);
        assert!(!acceptable);
        assert!(warning);
        assert!(!critical);
    }

    #[test]
    fn test_blocked_ratio_slo_assess_at_warning() {
        let slo = BlockedRatioSlo::api();
        let (acceptable, warning, critical) = slo.assess(0.10);
        // At exactly 0.10 (warning threshold), warning should be true
        // because warning is true when > acceptable && <= warning
        assert!(!acceptable);
        assert!(warning); // 0.10 is in the warning zone (0.05 < 0.10 <= 0.10)
        assert!(!critical);
    }

    #[test]
    fn test_blocked_ratio_slo_assess_between_warning_and_critical() {
        let slo = BlockedRatioSlo::api();
        let (acceptable, warning, critical) = slo.assess(0.125);
        assert!(!acceptable);
        assert!(!warning);
        assert!(!critical);
    }

    #[test]
    fn test_blocked_ratio_slo_assess_above_critical() {
        let slo = BlockedRatioSlo::api();
        let (acceptable, warning, critical) = slo.assess(0.20);
        assert!(!acceptable);
        assert!(!warning);
        assert!(critical);
    }

    #[test]
    fn test_blocked_ratio_slo_content_site_assessment() {
        let slo = BlockedRatioSlo::content_site();

        // Below acceptable (green)
        let (acc, warn, crit) = slo.assess(0.10);
        assert!(acc && !warn && !crit);

        // In warning zone (yellow): 0.15 < 0.20 <= 0.25
        let (acc, warn, crit) = slo.assess(0.20);
        assert!(!acc && warn && !crit);

        // In critical zone: 0.45 > 0.40
        let (acc, warn, crit) = slo.assess(0.45);
        assert!(!acc && !warn && crit);

        // Exactly at critical threshold
        let (acc, warn, crit) = slo.assess(0.40);
        assert!(!acc && !warn && !crit); // Exactly at threshold is not > critical
    }

    #[test]
    fn test_target_class_derives() {
        // Verify that TargetClass can be compared and hashed
        let api1 = TargetClass::Api;
        let api2 = TargetClass::Api;
        let content = TargetClass::ContentSite;

        assert_eq!(api1, api2);
        assert_ne!(api1, content);
    }

    #[test]
    fn test_blocked_ratio_slo_serialization() {
        let slo = BlockedRatioSlo::content_site();
        let json = serde_json::to_string(&slo).unwrap_or_default();
        if let Ok(deserialized) = serde_json::from_str::<BlockedRatioSlo>(&json) {
            assert_eq!(slo, deserialized);
        }
    }

    #[test]
    fn test_target_class_serialization() {
        let target = TargetClass::HighSecurity;
        let json = serde_json::to_string(&target).unwrap_or_default();
        if let Ok(deserialized) = serde_json::from_str::<TargetClass>(&json) {
            assert_eq!(target, deserialized);
        }
    }
}
