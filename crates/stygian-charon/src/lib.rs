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
/// Investigation report cache backends and cache key helpers.
#[cfg(feature = "caching")]
pub mod cache;
/// Provider signature classification logic.
pub mod classifier;
/// HAR parsing and extraction utilities.
pub mod har;
/// Investigation reports and baseline/candidate diffing.
pub mod investigation;
/// Telemetry and metrics collection (feature-gated).
#[cfg(feature = "metrics")]
pub mod metrics;
/// Runtime policy planning based on investigation output.
pub mod policy;
/// Normalized fingerprint snapshot schema types and compatibility checks.
pub mod snapshot;
/// Public types for transaction and report models.
pub mod types;

pub use acquisition::{
    AcquisitionModeHint, AcquisitionPolicy, AcquisitionStartHint, RuntimePolicyHints,
    map_adapter_strategy, map_policy_hints, map_runtime_policy,
};
pub use adaptive::{AdaptivePolicyError, AdaptiveSloPolicy, RegressionHistoryPolicy};
#[cfg(feature = "redis-cache")]
pub use cache::RedisInvestigationCache;
#[cfg(feature = "caching")]
pub use cache::{InvestigationReportCache, MemoryInvestigationCache, investigation_cache_key};
pub use classifier::{classify_har, classify_transaction};
pub use investigation::{
    compare_reports, infer_requirements, infer_requirements_with_target_class, investigate_har,
};
#[cfg(feature = "caching")]
pub use investigation::{investigate_har_cached, investigate_har_cached_with_target_class};
pub use policy::{analyze_and_plan, build_runtime_policy, plan_from_report};
pub use snapshot::{
    FingerprintSignals, NormalizedFingerprintSnapshot, ScreenFingerprint, SnapshotCollectionError,
    SnapshotCompatibilityError, SnapshotDeterminismOptions, SnapshotDriftReport, SnapshotMode,
    SnapshotSignalDrift, SnapshotSignalDriftKind, TlsFingerprint, WebGlFingerprint,
    collect_deterministic_snapshot_bytes, compare_snapshot_signal_drift,
    normalize_snapshot_for_determinism, validate_snapshot_compatibility,
};
pub use types::{
    AdapterStrategy, AntiBotProvider, AntiBotRequirement, BlockedRatioSlo, Detection,
    ExecutionMode, HarClassificationReport, HarRequestSummary, HostSummary,
    IntegrationRecommendation, InvestigationBundle, InvestigationDiff, InvestigationReport,
    MarkerCount, ProviderScore, RequirementLevel, RequirementsProfile, RuntimePolicy, SessionMode,
    TargetClass, TelemetryLevel, TransactionView,
};
