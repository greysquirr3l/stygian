#![warn(missing_docs, rustdoc::broken_intra_doc_links)]
#![deny(unsafe_code)]

//! stygian-charon
//!
//! Defensive anti-bot diagnostics for Stygian.
//! The crate classifies likely anti-bot providers from transaction evidence
//! and from HTTP Archive (HAR) files.

/// Mapping layer from runtime policy to acquisition strategy hints.
pub mod acquisition;
/// Adaptive SLO policy interfaces and regression-history implementation.
pub mod adaptive;
/// Versioned analyzer interfaces and profile selection.
pub mod analyzer;
/// Investigation report cache backends and cache key helpers.
#[cfg(feature = "caching")]
pub mod cache;
/// Provider signature classification logic.
pub mod classifier;
/// Mode differential regression runner across snapshot capture modes.
pub mod differential;
/// HAR parsing and extraction utilities.
pub mod har;
/// Investigation reports and baseline/candidate diffing.
pub mod investigation;
/// Telemetry and metrics collection (feature-gated).
#[cfg(feature = "metrics")]
pub mod metrics;
/// Runtime policy planning based on investigation output.
pub mod policy;
/// Challenge-style probe pack for adversarial and regression testing.
pub mod probe;
/// Normalized fingerprint snapshot schema types and compatibility checks.
pub mod snapshot;
/// Public types for transaction and report models.
pub mod types;

pub use acquisition::{
    AcquisitionModeHint, AcquisitionPolicy, AcquisitionStartHint, RuntimePolicyHints,
    map_adapter_strategy, map_policy_hints, map_runtime_policy,
};
pub use adaptive::{AdaptivePolicyError, AdaptiveSloPolicy, RegressionHistoryPolicy};
pub use analyzer::{AnalyzerProfile, AnalyzerVersion, ProviderAnalyzer};
#[cfg(feature = "redis-cache")]
pub use cache::RedisInvestigationCache;
#[cfg(feature = "caching")]
pub use cache::{InvestigationReportCache, MemoryInvestigationCache, investigation_cache_key};
pub use classifier::{
    classify_har, classify_har_with_profile, classify_transaction,
    classify_transaction_with_profile,
};
pub use differential::{
    ModeComparison, ModeDifferentialCorpus, ModeDifferentialError, ModeDifferentialPairResult,
    ModeDifferentialRunReport, ModeDifferentialThresholds, run_mode_differential_regression,
};
pub use investigation::{
    compare_reports, infer_requirements, infer_requirements_with_target_class, investigate_har,
    investigate_har_with_profile,
};
#[cfg(feature = "caching")]
pub use investigation::{investigate_har_cached, investigate_har_cached_with_target_class};
pub use policy::{analyze_and_plan, build_runtime_policy, plan_from_report};
pub use probe::{
    ChallengeProbe, ProbeCategory, ProbeExpectation, ProbePackReport, ProbeRunResult,
    challenge_probe_pack, run_probe_pack,
};
pub use snapshot::{
    FingerprintSignals, NormalizedFingerprintSnapshot, ScreenFingerprint, SnapshotCoherenceReport,
    SnapshotCoherenceViolation, SnapshotCollectionError, SnapshotCompatibilityError,
    SnapshotDeterminismOptions, SnapshotDriftReport, SnapshotMode, SnapshotSignalDrift,
    SnapshotSignalDriftKind, TlsFingerprint, WebGlFingerprint,
    collect_deterministic_snapshot_bytes, compare_snapshot_signal_drift,
    evaluate_snapshot_coherence, normalize_snapshot_for_determinism,
    validate_snapshot_compatibility,
};
pub use types::{
    AdapterStrategy, AntiBotProvider, AntiBotRequirement, BlockedRatioSlo, Detection,
    ExecutionMode, HarClassificationReport, HarRequestSummary, HostSummary,
    IntegrationRecommendation, InvestigationBundle, InvestigationDiff, InvestigationReport,
    MarkerCount, ProviderScore, RequirementLevel, RequirementsProfile, RuntimePolicy, SessionMode,
    TargetClass, TelemetryLevel, TransactionView,
};
