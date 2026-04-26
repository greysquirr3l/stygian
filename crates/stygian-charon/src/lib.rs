#![warn(missing_docs, rustdoc::broken_intra_doc_links)]
#![deny(unsafe_code)]

//! stygian-charon
//!
//! Defensive anti-bot diagnostics for Stygian.
//! The crate classifies likely anti-bot providers from transaction evidence
//! and from HTTP Archive (HAR) files.

/// Mapping layer from runtime policy to acquisition strategy hints.
pub mod acquisition;
/// Provider signature classification logic.
pub mod classifier;
/// HAR parsing and extraction utilities.
pub mod har;
/// Investigation reports and baseline/candidate diffing.
pub mod investigation;
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
pub use classifier::{classify_har, classify_transaction};
pub use investigation::{compare_reports, infer_requirements, investigate_har};
pub use policy::{analyze_and_plan, build_runtime_policy, plan_from_report};
pub use snapshot::{
    FingerprintSignals, NormalizedFingerprintSnapshot, ScreenFingerprint,
    SnapshotCompatibilityError, SnapshotMode, TlsFingerprint, WebGlFingerprint,
    validate_snapshot_compatibility,
};
pub use types::{
    AdapterStrategy, AntiBotProvider, AntiBotRequirement, Detection, ExecutionMode,
    HarClassificationReport, HarRequestSummary, HostSummary, IntegrationRecommendation,
    InvestigationBundle, InvestigationDiff, InvestigationReport, MarkerCount, ProviderScore,
    RequirementLevel, RequirementsProfile, RuntimePolicy, SessionMode, TelemetryLevel,
    TransactionView,
};
