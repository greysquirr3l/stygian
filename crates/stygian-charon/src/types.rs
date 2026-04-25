use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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
