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
/// Historical HAR replay across analyzer profiles.
pub mod backtest;
/// Diagnostic bundle API with redaction policy.
pub mod bundle;
/// Investigation report cache backends and cache key helpers.
#[cfg(feature = "caching")]
pub mod cache;
/// Anti-bot change-detection feed (T88). Detects
/// canary, proxy, and extraction deltas and emits
/// actionable incident packets via the metrics
/// surface and the diagnostics payload.
#[cfg(feature = "caching")]
pub mod change_feed;
/// Challenge-aware policy feedback loop (T83).
#[cfg(feature = "caching")]
pub mod challenge_feedback;
/// Provider signature classification logic.
pub mod classifier;
/// Challenge-token lifecycle contracts (T91). Strict per-vendor
/// TTL / nonce / single-use / session-binding invariants enforced
/// before submission.
#[cfg(feature = "caching")]
pub mod token_lifecycle;
/// Vendor fingerprinting confidence classifier (T89).
pub mod vendor_classifier;
/// Vendor-to-playbook auto-resolution (T90). Bridges the vendor
/// classifier and the playbook resolver, with multi-vendor
/// precedence, merge rules, and a `Manual` fallback that keeps
/// existing manual mode selection working unchanged.
pub mod vendor_resolver;
/// Mode differential regression runner across snapshot capture modes.
pub mod differential;
/// HAR parsing and extraction utilities.
pub mod har;
/// Investigation reports and baseline/candidate diffing.
pub mod investigation;
/// Target-class playbooks as code (T85). Resolves per-target
/// acquisition / proxy / pacing / escalation knobs with
/// deterministic precedence.
pub mod playbooks;
/// Proof-of-work capability profile (T93). Quantifies
/// solve latency, success rate, retry count, and failure
/// modes into a deterministic unit-interval score, with
/// sparse-telemetry fallback and a policy mapper that
/// nudges the runtime policy toward a posture matching
/// the observed capability. Persistence reuses the same
/// `LruTtlStore` primitive T83 / T91 use (no new cache
/// store; PoW key namespace is `charon:pow:...`).
#[cfg(feature = "caching")]
pub mod pow_profile;
/// Telemetry and metrics collection (feature-gated).
#[cfg(feature = "metrics")]
pub mod metrics;
/// External observatory runner and comparison reports.
pub mod observatory;
/// Runtime policy planning based on investigation output.
pub mod policy;
/// Challenge-style probe pack for adversarial and regression testing.
pub mod probe;
/// Release risk scoring and release-candidate trend reporting.
pub mod release_risk;
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
pub use backtest::{
    BacktestCase, BacktestDisagreement, BacktestError, BacktestReport, BacktestSample,
    run_profile_backtest,
};
pub use bundle::{
    BundleCoherenceViolation, BundleError, BundleMetadata, BundleRedactionPolicy, DiagnosticBundle,
    apply_redaction, build_diagnostic_bundle, build_diagnostic_bundle_with_snapshot,
    diagnostic_bundle_from_investigation,
};
#[cfg(feature = "redis-cache")]
pub use cache::RedisInvestigationCache;
#[cfg(feature = "caching")]
pub use cache::{InvestigationReportCache, MemoryInvestigationCache, investigation_cache_key};
#[cfg(feature = "caching")]
pub use change_feed::{
    ChangeClassification, ChangeDeltaInput, ChangeDetector, ChangeEvent, ChangeEventSink,
    ChangeFeedReport, ChangeFeedThresholds, DeltaSeverity, DeltaSource, DeltaSummary,
    InMemoryChangeFeedSink, MitigationPath, record_change_event,
};
#[cfg(feature = "caching")]
pub use challenge_feedback::{
    ChallengeFeedbackPolicy, ChallengeMemory, ChallengeMemoryEntry, ChallengeOutcome,
    MAX_RISK_DELTA, adjust_runtime_policy, build_runtime_policy_with_memory,
    challenge_memory_key, memory_adjustment_for,
};
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
#[cfg(feature = "live-validation")]
pub use observatory::{LiveObservatoryProbe, run_external_observatory_live};
pub use observatory::{
    ObservatoryCase, ObservatoryComparison, ObservatoryError, ObservatoryEscalation,
    ObservatoryReport, ObservatorySample, run_external_observatory_from_hars,
};
pub use policy::{analyze_and_plan, build_runtime_policy, plan_from_report};
pub use probe::{
    ChallengeProbe, ProbeCategory, ProbeExpectation, ProbePackReport, ProbeRunResult,
    challenge_probe_pack, run_probe_pack,
};
pub use release_risk::{
    ReleaseCandidateRiskSnapshot, ReleaseRiskAssessment, ReleaseRiskBreakdown, ReleaseRiskInput,
    ReleaseRiskLevel, ReleaseRiskThresholds, ReleaseRiskWeights, ReleaseTrendDirection,
    ReleaseTrendPoint, ReleaseTrendReport, assess_release_risk, build_release_trend_report,
    release_risk_input_from_reports,
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
pub use vendor_classifier::{
    DEFAULT_HIGH_CONFIDENCE_THRESHOLD, Evidence, EvidenceBundle, EvidenceSource, VendorClassification,
    VendorClassifier, VendorDefinition, VendorError, VendorId, VendorScore, VendorSignal,
    parse_vendor_definition,
};
pub use vendor_resolver::{
    AppliedRule, MergeStrategy, PlaybookResolverExt, ResolutionRationale, ResolutionRule,
    StrategyMarker, VendorResolution, VendorResolver, VendorResolverError, VendorRuleMatch,
    parse_resolution_rule,
};
#[cfg(feature = "caching")]
pub use token_lifecycle::{
    ChallengeClass, InvalidationKind, InvalidationReason, NonceBook, NonceObservation,
    TokenContract, TokenLifecycleError, TokenPolicy, TokenPolicyTable, TokenValidator,
    ValidationOutcome, builtin_token_policies, DEFAULT_NONCE_BOOK_CAPACITY, DEFAULT_NONCE_TTL,
    nonce_book_key,
};
#[cfg(feature = "caching")]
pub use pow_profile::{
    DEFAULT_LATENCY_BUDGET_MS, DEFAULT_POW_CAPACITY, DEFAULT_POW_TTL, DEFAULT_RETRY_BUDGET,
    DEFAULT_SAMPLE_WINDOW_SECS, MAX_POW_RISK_DELTA, MIN_OBSERVATIONS_FOR_SCORING,
    PowCapabilityBand, PowCapabilityProfile, PowCapabilitySample, PowCapabilityScore,
    PowCapabilityScorer, PowCapabilityStore, PowFailureMode, PowPolicyThresholds,
    ProfileWeights, SPARSE_FALLBACK_SCORE, adjust_runtime_policy_for_pow, band_for_score,
    pow_profile_key, score_from_profile,
};
