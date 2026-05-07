use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

// ── Input Size Limits ───────────────────────────────────────────────────────
// Bounds on snapshot field sizes to prevent resource exhaustion and DoS.

/// Maximum JSON payload size for a snapshot (10 MB).
const MAX_SNAPSHOT_JSON_BYTES: usize = 10 * 1024 * 1024;

/// Maximum length for string fields like user_agent, platform, timezone.
const MAX_STRING_FIELD_BYTES: usize = 4_096;

/// Maximum length for hash fields like ja3_hash, snapshot_id.
const MAX_HASH_FIELD_BYTES: usize = 1_024;

/// Maximum number of header entries in signals.headers.
const MAX_HEADERS_ENTRIES: usize = 256;

/// Maximum number of feature flags in signals.features.
const MAX_FEATURES_ENTRIES: usize = 256;

/// Maximum number of metadata entries.
const MAX_METADATA_ENTRIES: usize = 128;

/// Normalized capture mode for a fingerprint snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapshotMode {
    /// Snapshot captured from HTTP-oriented execution.
    Http,
    /// Snapshot captured from browser-oriented execution.
    Browser,
    /// Snapshot combines HTTP and browser surfaces.
    Hybrid,
}

/// Screen-related fingerprint surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenFingerprint {
    /// Screen width in CSS pixels.
    pub width: u32,
    /// Screen height in CSS pixels.
    pub height: u32,
    /// Device pixel ratio.
    pub device_pixel_ratio: f64,
}

/// WebGL-related fingerprint surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebGlFingerprint {
    /// WebGL vendor string.
    pub vendor: String,
    /// WebGL renderer string.
    pub renderer: String,
}

/// TLS-related fingerprint surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsFingerprint {
    /// JA3 hash for the observed TLS handshake.
    pub ja3_hash: String,
    /// Optional JA4 fingerprint.
    pub ja4: Option<String>,
}

/// Signal payload for normalized fingerprint snapshots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FingerprintSignals {
    /// User-Agent string.
    pub user_agent: String,
    /// Accept-Language header value.
    pub accept_language: String,
    /// Platform indicator.
    pub platform: String,
    /// Timezone identifier.
    pub timezone: String,
    /// Header-level snapshot values.
    pub headers: BTreeMap<String, String>,
    /// Boolean feature flags.
    pub features: BTreeMap<String, bool>,
    /// Screen metrics.
    pub screen: ScreenFingerprint,
    /// WebGL surface (required for browser mode).
    pub webgl: Option<WebGlFingerprint>,
    /// TLS surface (required for HTTP mode).
    pub tls: Option<TlsFingerprint>,
}

/// Versioned normalized fingerprint snapshot across modes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedFingerprintSnapshot {
    /// Snapshot schema version (`major.minor.patch`).
    pub schema_version: String,
    /// Stable unique snapshot id.
    pub snapshot_id: String,
    /// Capture mode.
    pub mode: SnapshotMode,
    /// RFC 3339 timestamp of capture.
    pub captured_at: String,
    /// Fingerprint signal payload.
    pub signals: FingerprintSignals,
    /// Optional metadata for provenance and notes.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// Deprecated UA mirror retained for backward compatibility.
    #[serde(default)]
    pub legacy_user_agent: Option<String>,
    /// Deprecated JA3 mirror retained for backward compatibility.
    #[serde(default)]
    pub legacy_ja3_hash: Option<String>,
}

/// Compatibility validation error for normalized snapshots.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SnapshotCompatibilityError {
    /// Schema version is not parseable as semver-like `major.minor.patch`.
    #[error("invalid schema version: {0}")]
    InvalidSchemaVersion(String),
    /// Snapshot major version is not supported by this reader.
    #[error("unsupported schema major version: {0}")]
    UnsupportedSchemaMajor(u64),
    /// A mode-required signal is missing.
    #[error("missing required signal '{signal}' for mode {mode:?}")]
    MissingModeSignal {
        /// Snapshot mode.
        mode: SnapshotMode,
        /// Required signal name.
        signal: &'static str,
    },
    /// Deprecated mirror field is inconsistent with canonical field.
    #[error("deprecated field '{field}' does not match canonical field")]
    LegacyFieldMismatch {
        /// Deprecated field name.
        field: &'static str,
    },
    /// Input validation failed due to size or structure constraints.
    #[error("input validation failed: {0}")]
    InputValidation(&'static str),
}

/// Error returned when building deterministic snapshot bytes.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SnapshotCollectionError {
    /// Snapshot failed compatibility validation.
    #[error("compatibility validation failed: {0}")]
    Compatibility(#[from] SnapshotCompatibilityError),
    /// Snapshot serialization failed.
    #[error("serialization failed: {0}")]
    Serialization(String),
}

/// Kind of signal-level drift detected between baseline and candidate snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapshotSignalDriftKind {
    /// A signal path exists only in candidate.
    Added,
    /// A signal path exists only in baseline.
    Removed,
    /// A signal path exists in both snapshots but the value changed.
    Changed,
}

/// Focused signal-level drift entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotSignalDrift {
    /// Dot-path to the changed signal key, rooted at `signals`.
    pub path: String,
    /// Difference kind for this path.
    pub kind: SnapshotSignalDriftKind,
    /// Baseline value encoded as compact JSON when present.
    pub baseline: Option<String>,
    /// Candidate value encoded as compact JSON when present.
    pub candidate: Option<String>,
}

/// Signal-focused drift report for baseline vs candidate snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotDriftReport {
    /// Signal-level differences discovered after deterministic normalization.
    pub diffs: Vec<SnapshotSignalDrift>,
}

/// Machine-readable coherence violation produced by a snapshot rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotCoherenceViolation {
    /// Stable identifier for the violated rule.
    pub rule_id: String,
    /// Human-readable explanation of the mismatch.
    pub message: String,
    /// Dot-paths participating in the violation.
    pub paths: Vec<String>,
}

/// Result of evaluating all registered snapshot coherence rules.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotCoherenceReport {
    /// Violations returned by the active rule set.
    pub violations: Vec<SnapshotCoherenceViolation>,
}

impl SnapshotCoherenceReport {
    /// Return `true` when any coherence rule was violated.
    #[must_use]
    pub const fn has_violations(&self) -> bool {
        !self.violations.is_empty()
    }
}

impl SnapshotDriftReport {
    /// Return `true` when any signal drift was detected.
    #[must_use]
    pub const fn has_drift(&self) -> bool {
        !self.diffs.is_empty()
    }

    /// Render a focused, line-oriented diff for changed signal paths.
    #[must_use]
    pub fn render_focused_diff(&self) -> String {
        if self.diffs.is_empty() {
            return "no signal drift detected".to_string();
        }

        self.diffs
            .iter()
            .map(|entry| match entry.kind {
                SnapshotSignalDriftKind::Added => format!(
                    "{} added: {}",
                    entry.path,
                    entry.candidate.as_deref().unwrap_or("null")
                ),
                SnapshotSignalDriftKind::Removed => format!(
                    "{} removed: {}",
                    entry.path,
                    entry.baseline.as_deref().unwrap_or("null")
                ),
                SnapshotSignalDriftKind::Changed => format!(
                    "{} changed: {} -> {}",
                    entry.path,
                    entry.baseline.as_deref().unwrap_or("null"),
                    entry.candidate.as_deref().unwrap_or("null")
                ),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Options controlling deterministic snapshot collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotDeterminismOptions {
    /// Normalize `captured_at` to a stable placeholder timestamp.
    pub normalize_captured_at: bool,
    /// Remove volatile metadata keys before serialization.
    pub strip_volatile_metadata: bool,
}

impl Default for SnapshotDeterminismOptions {
    fn default() -> Self {
        Self {
            normalize_captured_at: true,
            strip_volatile_metadata: true,
        }
    }
}

const DETERMINISTIC_CAPTURED_AT: &str = "1970-01-01T00:00:00Z";
const VOLATILE_METADATA_KEYS: &[&str] = &[
    "capture_nonce",
    "generated_at",
    "request_id",
    "run_id",
    "session_id",
    "trace_id",
];

type SnapshotCoherenceRule =
    fn(&NormalizedFingerprintSnapshot) -> Option<SnapshotCoherenceViolation>;

const SNAPSHOT_COHERENCE_RULES: &[SnapshotCoherenceRule] = &[
    rule_user_agent_header_matches,
    rule_accept_language_header_matches,
    rule_browser_webdriver_disabled,
    rule_webgl_fields_populated,
    rule_tls_fields_populated,
];

/// Normalize a snapshot in-place for deterministic collection.
pub fn normalize_snapshot_for_determinism(
    snapshot: &mut NormalizedFingerprintSnapshot,
    options: &SnapshotDeterminismOptions,
) {
    if options.normalize_captured_at {
        snapshot.captured_at = DETERMINISTIC_CAPTURED_AT.to_string();
    }

    if options.strip_volatile_metadata {
        for key in VOLATILE_METADATA_KEYS {
            snapshot.metadata.remove(*key);
        }
    }
}

/// Serialize a snapshot into deterministic JSON bytes.
///
/// The function first validates snapshot compatibility, then applies
/// deterministic normalization rules, and finally serializes with a stable
/// field order (provided by struct declaration order + `BTreeMap` keys).
///
/// # Errors
///
/// Returns [`SnapshotCollectionError`] when snapshot compatibility validation
/// fails or when JSON serialization fails.
pub fn collect_deterministic_snapshot_bytes(
    snapshot: &NormalizedFingerprintSnapshot,
    options: &SnapshotDeterminismOptions,
) -> Result<Vec<u8>, SnapshotCollectionError> {
    validate_snapshot_compatibility(snapshot)?;

    let mut normalized = snapshot.clone();
    normalize_snapshot_for_determinism(&mut normalized, options);

    serde_json::to_vec(&normalized)
        .map_err(|error| SnapshotCollectionError::Serialization(error.to_string()))
}

/// Compare baseline and candidate snapshots for deterministic, signal-focused drift.
///
/// The comparison validates both snapshots, applies deterministic normalization,
/// and reports only differences under the `signals` subtree.
///
/// # Errors
///
/// Returns [`SnapshotCollectionError`] when either snapshot fails compatibility
/// validation or deterministic serialization.
pub fn compare_snapshot_signal_drift(
    baseline: &NormalizedFingerprintSnapshot,
    candidate: &NormalizedFingerprintSnapshot,
    options: &SnapshotDeterminismOptions,
) -> Result<SnapshotDriftReport, SnapshotCollectionError> {
    let baseline_bytes = collect_deterministic_snapshot_bytes(baseline, options)?;
    let candidate_bytes = collect_deterministic_snapshot_bytes(candidate, options)?;

    let baseline_normalized: NormalizedFingerprintSnapshot =
        serde_json::from_slice(&baseline_bytes)
            .map_err(|error| SnapshotCollectionError::Serialization(error.to_string()))?;
    let candidate_normalized: NormalizedFingerprintSnapshot =
        serde_json::from_slice(&candidate_bytes)
            .map_err(|error| SnapshotCollectionError::Serialization(error.to_string()))?;

    let baseline_signals = serde_json::to_value(&baseline_normalized.signals)
        .map_err(|error| SnapshotCollectionError::Serialization(error.to_string()))?;
    let candidate_signals = serde_json::to_value(&candidate_normalized.signals)
        .map_err(|error| SnapshotCollectionError::Serialization(error.to_string()))?;

    let mut diffs = Vec::new();
    collect_signal_diffs("signals", &baseline_signals, &candidate_signals, &mut diffs);

    Ok(SnapshotDriftReport { diffs })
}

/// Evaluate registered coherence rules across normalized snapshot fields.
#[must_use]
pub fn evaluate_snapshot_coherence(
    snapshot: &NormalizedFingerprintSnapshot,
) -> SnapshotCoherenceReport {
    let violations = SNAPSHOT_COHERENCE_RULES
        .iter()
        .filter_map(|rule| rule(snapshot))
        .collect();

    SnapshotCoherenceReport { violations }
}

fn collect_signal_diffs(
    path: &str,
    baseline: &serde_json::Value,
    candidate: &serde_json::Value,
    diffs: &mut Vec<SnapshotSignalDrift>,
) {
    match (baseline, candidate) {
        (serde_json::Value::Object(left), serde_json::Value::Object(right)) => {
            let keys: BTreeSet<&String> = left.keys().chain(right.keys()).collect();
            for key in keys {
                let next_path = format!("{path}.{key}");
                match (left.get(key), right.get(key)) {
                    (Some(left_value), Some(right_value)) => {
                        collect_signal_diffs(&next_path, left_value, right_value, diffs);
                    }
                    (Some(left_value), None) => {
                        diffs.push(SnapshotSignalDrift {
                            path: next_path,
                            kind: SnapshotSignalDriftKind::Removed,
                            baseline: Some(left_value.to_string()),
                            candidate: None,
                        });
                    }
                    (None, Some(right_value)) => {
                        diffs.push(SnapshotSignalDrift {
                            path: next_path,
                            kind: SnapshotSignalDriftKind::Added,
                            baseline: None,
                            candidate: Some(right_value.to_string()),
                        });
                    }
                    (None, None) => {}
                }
            }
        }
        _ => {
            if baseline != candidate {
                diffs.push(SnapshotSignalDrift {
                    path: path.to_string(),
                    kind: SnapshotSignalDriftKind::Changed,
                    baseline: Some(baseline.to_string()),
                    candidate: Some(candidate.to_string()),
                });
            }
        }
    }
}

fn parse_schema_major(version: &str) -> Result<u64, SnapshotCompatibilityError> {
    let parts = version.split('.').collect::<Vec<_>>();
    let [major, minor, patch] = parts.as_slice() else {
        return Err(SnapshotCompatibilityError::InvalidSchemaVersion(
            version.to_string(),
        ));
    };

    if major.parse::<u64>().is_err()
        || minor.parse::<u64>().is_err()
        || patch.parse::<u64>().is_err()
    {
        return Err(SnapshotCompatibilityError::InvalidSchemaVersion(
            version.to_string(),
        ));
    }

    major
        .parse::<u64>()
        .map_err(|_| SnapshotCompatibilityError::InvalidSchemaVersion(version.to_string()))
}

/// Validate input sizes for a snapshot to prevent resource exhaustion.
///
/// # Errors
///
/// Returns [`SnapshotCompatibilityError::InputValidation`] when any size limit is exceeded.
fn validate_snapshot_input_sizes(
    snapshot: &NormalizedFingerprintSnapshot,
) -> Result<(), SnapshotCompatibilityError> {
    // Validate string field sizes
    if snapshot.schema_version.len() > MAX_STRING_FIELD_BYTES {
        return Err(SnapshotCompatibilityError::InputValidation(
            "schema_version exceeds maximum length",
        ));
    }
    if snapshot.snapshot_id.len() > MAX_HASH_FIELD_BYTES {
        return Err(SnapshotCompatibilityError::InputValidation(
            "snapshot_id exceeds maximum length",
        ));
    }
    if snapshot.captured_at.len() > MAX_STRING_FIELD_BYTES {
        return Err(SnapshotCompatibilityError::InputValidation(
            "captured_at exceeds maximum length",
        ));
    }

    // Validate signals string fields
    let signals = &snapshot.signals;
    if signals.user_agent.len() > MAX_STRING_FIELD_BYTES {
        return Err(SnapshotCompatibilityError::InputValidation(
            "signals.user_agent exceeds maximum length",
        ));
    }
    if signals.accept_language.len() > MAX_STRING_FIELD_BYTES {
        return Err(SnapshotCompatibilityError::InputValidation(
            "signals.accept_language exceeds maximum length",
        ));
    }
    if signals.platform.len() > MAX_STRING_FIELD_BYTES {
        return Err(SnapshotCompatibilityError::InputValidation(
            "signals.platform exceeds maximum length",
        ));
    }
    if signals.timezone.len() > MAX_STRING_FIELD_BYTES {
        return Err(SnapshotCompatibilityError::InputValidation(
            "signals.timezone exceeds maximum length",
        ));
    }

    // Validate headers and features collections
    if signals.headers.len() > MAX_HEADERS_ENTRIES {
        return Err(SnapshotCompatibilityError::InputValidation(
            "signals.headers exceeds maximum entries",
        ));
    }
    if signals.features.len() > MAX_FEATURES_ENTRIES {
        return Err(SnapshotCompatibilityError::InputValidation(
            "signals.features exceeds maximum entries",
        ));
    }

    // Validate individual header key/value sizes
    for (key, value) in &signals.headers {
        if key.len() > MAX_STRING_FIELD_BYTES {
            return Err(SnapshotCompatibilityError::InputValidation(
                "header key exceeds maximum length",
            ));
        }
        if value.len() > MAX_STRING_FIELD_BYTES {
            return Err(SnapshotCompatibilityError::InputValidation(
                "header value exceeds maximum length",
            ));
        }
    }

    // Validate WebGL fields if present
    if let Some(webgl) = &signals.webgl {
        if webgl.vendor.len() > MAX_STRING_FIELD_BYTES {
            return Err(SnapshotCompatibilityError::InputValidation(
                "signals.webgl.vendor exceeds maximum length",
            ));
        }
        if webgl.renderer.len() > MAX_STRING_FIELD_BYTES {
            return Err(SnapshotCompatibilityError::InputValidation(
                "signals.webgl.renderer exceeds maximum length",
            ));
        }
    }

    // Validate TLS fields if present
    if let Some(tls) = &signals.tls {
        if tls.ja3_hash.len() > MAX_HASH_FIELD_BYTES {
            return Err(SnapshotCompatibilityError::InputValidation(
                "signals.tls.ja3_hash exceeds maximum length",
            ));
        }
        if let Some(ja4) = &tls.ja4 {
            if ja4.len() > MAX_HASH_FIELD_BYTES {
                return Err(SnapshotCompatibilityError::InputValidation(
                    "signals.tls.ja4 exceeds maximum length",
                ));
            }
        }
    }

    // Validate metadata
    if snapshot.metadata.len() > MAX_METADATA_ENTRIES {
        return Err(SnapshotCompatibilityError::InputValidation(
            "metadata exceeds maximum entries",
        ));
    }
    for (key, value) in &snapshot.metadata {
        if key.len() > MAX_HASH_FIELD_BYTES || value.len() > MAX_STRING_FIELD_BYTES {
            return Err(SnapshotCompatibilityError::InputValidation(
                "metadata entry exceeds maximum size",
            ));
        }
    }

    // Validate legacy fields if present
    if let Some(legacy_ua) = &snapshot.legacy_user_agent {
        if legacy_ua.len() > MAX_STRING_FIELD_BYTES {
            return Err(SnapshotCompatibilityError::InputValidation(
                "legacy_user_agent exceeds maximum length",
            ));
        }
    }
    if let Some(legacy_ja3) = &snapshot.legacy_ja3_hash {
        if legacy_ja3.len() > MAX_HASH_FIELD_BYTES {
            return Err(SnapshotCompatibilityError::InputValidation(
                "legacy_ja3_hash exceeds maximum length",
            ));
        }
    }

    Ok(())
}

fn signal_header<'a>(snapshot: &'a NormalizedFingerprintSnapshot, name: &str) -> Option<&'a str> {
    snapshot
        .signals
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn mismatch_violation(
    rule_id: &'static str,
    message: impl Into<String>,
    paths: &[&str],
) -> SnapshotCoherenceViolation {
    SnapshotCoherenceViolation {
        rule_id: rule_id.to_string(),
        message: message.into(),
        paths: paths.iter().map(|path| (*path).to_string()).collect(),
    }
}

fn rule_user_agent_header_matches(
    snapshot: &NormalizedFingerprintSnapshot,
) -> Option<SnapshotCoherenceViolation> {
    let header = signal_header(snapshot, "user-agent")?;
    if header == snapshot.signals.user_agent {
        return None;
    }

    Some(mismatch_violation(
        "user_agent_header_match",
        "signals.user_agent does not match signals.headers.user-agent",
        &["signals.user_agent", "signals.headers.user-agent"],
    ))
}

fn rule_accept_language_header_matches(
    snapshot: &NormalizedFingerprintSnapshot,
) -> Option<SnapshotCoherenceViolation> {
    let header = signal_header(snapshot, "accept-language")?;
    if header == snapshot.signals.accept_language {
        return None;
    }

    Some(mismatch_violation(
        "accept_language_header_match",
        "signals.accept_language does not match signals.headers.accept-language",
        &["signals.accept_language", "signals.headers.accept-language"],
    ))
}

fn rule_browser_webdriver_disabled(
    snapshot: &NormalizedFingerprintSnapshot,
) -> Option<SnapshotCoherenceViolation> {
    if snapshot.mode == SnapshotMode::Http {
        return None;
    }

    if snapshot
        .signals
        .features
        .get("navigator.webdriver")
        .copied()
        != Some(true)
    {
        return None;
    }

    Some(mismatch_violation(
        "navigator_webdriver_disabled",
        "browser-oriented snapshots should not report navigator.webdriver=true",
        &["mode", "signals.features.navigator.webdriver"],
    ))
}

fn rule_webgl_fields_populated(
    snapshot: &NormalizedFingerprintSnapshot,
) -> Option<SnapshotCoherenceViolation> {
    let webgl = snapshot.signals.webgl.as_ref()?;
    if !webgl.vendor.trim().is_empty() && !webgl.renderer.trim().is_empty() {
        return None;
    }

    Some(mismatch_violation(
        "webgl_fields_populated",
        "signals.webgl vendor and renderer must both be populated when webgl is present",
        &["signals.webgl.vendor", "signals.webgl.renderer"],
    ))
}

fn rule_tls_fields_populated(
    snapshot: &NormalizedFingerprintSnapshot,
) -> Option<SnapshotCoherenceViolation> {
    let tls = snapshot.signals.tls.as_ref()?;
    if !tls.ja3_hash.trim().is_empty() {
        return None;
    }

    Some(mismatch_violation(
        "tls_ja3_populated",
        "signals.tls.ja3_hash must be populated when tls is present",
        &["signals.tls.ja3_hash"],
    ))
}

/// Validate normalized snapshot compatibility rules across modes and versions.
///
/// Current compatibility contract:
/// - supports schema major version `1`
/// - requires `signals.tls` for [`SnapshotMode::Http`]
/// - requires `signals.webgl` for [`SnapshotMode::Browser`]
/// - requires deprecated mirror fields, when present, to match canonical fields
///
/// # Errors
///
/// Returns [`SnapshotCompatibilityError`] when schema version is invalid or
/// unsupported, required mode-specific signals are missing, or legacy mirror
/// fields do not match canonical signal values.
pub fn validate_snapshot_compatibility(
    snapshot: &NormalizedFingerprintSnapshot,
) -> Result<(), SnapshotCompatibilityError> {
    // Validate input sizes first to prevent resource exhaustion
    validate_snapshot_input_sizes(snapshot)?;

    let major = parse_schema_major(&snapshot.schema_version)?;
    if major != 1 {
        return Err(SnapshotCompatibilityError::UnsupportedSchemaMajor(major));
    }

    match snapshot.mode {
        SnapshotMode::Http => {
            if snapshot.signals.tls.is_none() {
                return Err(SnapshotCompatibilityError::MissingModeSignal {
                    mode: snapshot.mode,
                    signal: "tls",
                });
            }
        }
        SnapshotMode::Browser => {
            if snapshot.signals.webgl.is_none() {
                return Err(SnapshotCompatibilityError::MissingModeSignal {
                    mode: snapshot.mode,
                    signal: "webgl",
                });
            }
        }
        SnapshotMode::Hybrid => {}
    }

    if let Some(legacy_ua) = snapshot.legacy_user_agent.as_deref()
        && legacy_ua != snapshot.signals.user_agent
    {
        return Err(SnapshotCompatibilityError::LegacyFieldMismatch {
            field: "legacy_user_agent",
        });
    }

    if let Some(legacy_ja3) = snapshot.legacy_ja3_hash.as_deref() {
        let Some(tls) = snapshot.signals.tls.as_ref() else {
            return Err(SnapshotCompatibilityError::LegacyFieldMismatch {
                field: "legacy_ja3_hash",
            });
        };
        if legacy_ja3 != tls.ja3_hash {
            return Err(SnapshotCompatibilityError::LegacyFieldMismatch {
                field: "legacy_ja3_hash",
            });
        }
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[allow(clippy::missing_const_for_fn)]
    fn parse_snapshot(path: &str) -> NormalizedFingerprintSnapshot {
        serde_json::from_str::<NormalizedFingerprintSnapshot>(path)
            .expect("example snapshot should deserialize")
    }

    #[test]
    fn example_http_snapshot_is_compatible() {
        let snap = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        assert!(validate_snapshot_compatibility(&snap).is_ok());
    }

    #[test]
    fn example_browser_snapshot_is_compatible() {
        let snap = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-browser.json"
        ));
        assert!(validate_snapshot_compatibility(&snap).is_ok());
    }

    #[test]
    fn coherence_report_is_clean_for_example_browser_snapshot() {
        let snap = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-browser.json"
        ));

        let report = evaluate_snapshot_coherence(&snap);

        assert!(!report.has_violations());
    }

    #[test]
    fn coherence_report_flags_cross_field_mismatches() {
        let mut snap = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-browser.json"
        ));
        snap.signals
            .headers
            .insert("user-agent".to_string(), "different-user-agent".to_string());
        snap.signals
            .headers
            .insert("accept-language".to_string(), "fr-FR,fr;q=0.9".to_string());
        snap.signals
            .features
            .insert("navigator.webdriver".to_string(), true);

        let report = evaluate_snapshot_coherence(&snap);

        assert!(report.has_violations());
        assert_eq!(report.violations.len(), 3);
        let ids = report
            .violations
            .iter()
            .map(|violation| violation.rule_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![
                "user_agent_header_match",
                "accept_language_header_match",
                "navigator_webdriver_disabled"
            ]
        );
    }

    #[test]
    fn http_mode_requires_tls_signal() {
        let mut snap = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        snap.signals.tls = None;
        let err = validate_snapshot_compatibility(&snap).expect_err("must fail without tls");
        assert_eq!(
            err,
            SnapshotCompatibilityError::MissingModeSignal {
                mode: SnapshotMode::Http,
                signal: "tls"
            }
        );
    }

    #[test]
    fn browser_mode_requires_webgl_signal() {
        let mut snap = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-browser.json"
        ));
        snap.signals.webgl = None;
        let err = validate_snapshot_compatibility(&snap).expect_err("must fail without webgl");
        assert_eq!(
            err,
            SnapshotCompatibilityError::MissingModeSignal {
                mode: SnapshotMode::Browser,
                signal: "webgl"
            }
        );
    }

    #[test]
    fn unsupported_schema_major_fails_compatibility() {
        let mut snap = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        snap.schema_version = "2.0.0".to_string();
        let err = validate_snapshot_compatibility(&snap).expect_err("must fail unsupported major");
        assert_eq!(err, SnapshotCompatibilityError::UnsupportedSchemaMajor(2));
    }

    #[test]
    fn schema_version_requires_exact_semver_triplet() {
        let mut snap = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        snap.schema_version = "1.0.0.1".to_string();

        let err = validate_snapshot_compatibility(&snap).expect_err("must reject extra segments");
        assert_eq!(
            err,
            SnapshotCompatibilityError::InvalidSchemaVersion("1.0.0.1".to_string())
        );
    }

    #[test]
    fn deterministic_collector_produces_identical_bytes_for_volatile_differences() {
        let mut first = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        first.captured_at = "2026-04-26T23:11:11Z".to_string();
        first
            .metadata
            .insert("trace_id".to_string(), "trace-a".to_string());
        first
            .metadata
            .insert("request_id".to_string(), "request-a".to_string());

        let mut second = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        second.captured_at = "2026-04-27T01:22:33Z".to_string();
        second
            .metadata
            .insert("request_id".to_string(), "request-b".to_string());
        second
            .metadata
            .insert("trace_id".to_string(), "trace-b".to_string());

        let options = SnapshotDeterminismOptions::default();
        let left = collect_deterministic_snapshot_bytes(&first, &options).expect("must serialize");
        let right =
            collect_deterministic_snapshot_bytes(&second, &options).expect("must serialize");
        assert_eq!(left, right);
    }

    #[test]
    fn deterministic_collector_keeps_nonvolatile_metadata() {
        let mut snap = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        snap.metadata
            .insert("collector".to_string(), "charon-v2".to_string());
        snap.metadata
            .insert("trace_id".to_string(), "volatile".to_string());

        let options = SnapshotDeterminismOptions::default();
        let bytes = collect_deterministic_snapshot_bytes(&snap, &options).expect("must collect");
        let collected: NormalizedFingerprintSnapshot =
            serde_json::from_slice(&bytes).expect("bytes should deserialize");

        assert_eq!(collected.captured_at, DETERMINISTIC_CAPTURED_AT);
        assert_eq!(
            collected.metadata.get("collector"),
            Some(&"charon-v2".to_string())
        );
        assert!(!collected.metadata.contains_key("trace_id"));
    }

    #[test]
    fn deterministic_collector_rejects_incompatible_snapshot() {
        let mut snap = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-browser.json"
        ));
        snap.signals.webgl = None;

        let options = SnapshotDeterminismOptions::default();
        let err = collect_deterministic_snapshot_bytes(&snap, &options)
            .expect_err("incompatible snapshot must fail");

        assert_eq!(
            err,
            SnapshotCollectionError::Compatibility(SnapshotCompatibilityError::MissingModeSignal {
                mode: SnapshotMode::Browser,
                signal: "webgl"
            })
        );
    }

    #[test]
    fn compare_snapshot_signal_drift_reports_focused_paths() {
        let baseline = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        let mut candidate = baseline.clone();
        candidate.signals.user_agent = "Mozilla/5.0 (X11; Linux x86_64)".to_string();
        candidate.legacy_user_agent = Some(candidate.signals.user_agent.clone());
        candidate
            .signals
            .features
            .insert("new_flag".to_string(), true);

        let report = compare_snapshot_signal_drift(
            &baseline,
            &candidate,
            &SnapshotDeterminismOptions::default(),
        )
        .expect("drift comparison must succeed");

        assert!(report.has_drift());
        assert!(
            report
                .diffs
                .iter()
                .any(|d| d.path == "signals.user_agent"
                    && d.kind == SnapshotSignalDriftKind::Changed)
        );
        assert!(
            report
                .diffs
                .iter()
                .any(|d| d.path == "signals.features.new_flag"
                    && d.kind == SnapshotSignalDriftKind::Added)
        );
    }

    #[test]
    fn compare_snapshot_signal_drift_ignores_volatile_only_changes() {
        let mut baseline = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        baseline.captured_at = "2026-04-26T00:00:00Z".to_string();
        baseline
            .metadata
            .insert("trace_id".to_string(), "trace-a".to_string());

        let mut candidate = baseline.clone();
        candidate.captured_at = "2026-04-27T00:00:00Z".to_string();
        candidate
            .metadata
            .insert("trace_id".to_string(), "trace-b".to_string());

        let report = compare_snapshot_signal_drift(
            &baseline,
            &candidate,
            &SnapshotDeterminismOptions::default(),
        )
        .expect("drift comparison must succeed");

        assert!(!report.has_drift());
    }

    #[test]
    fn reject_excessively_long_user_agent() {
        let mut snapshot = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        snapshot.signals.user_agent = "A".repeat(MAX_STRING_FIELD_BYTES + 1);

        let result = validate_snapshot_compatibility(&snapshot);
        assert!(result.is_err(), "should reject excessively long user_agent");
        assert!(matches!(
            result,
            Err(SnapshotCompatibilityError::InputValidation(_))
        ));
    }

    #[test]
    fn reject_excessively_long_platform() {
        let mut snapshot = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        snapshot.signals.platform = "B".repeat(MAX_STRING_FIELD_BYTES + 1);

        let result = validate_snapshot_compatibility(&snapshot);
        assert!(result.is_err(), "should reject excessively long platform");
    }

    #[test]
    fn reject_excessive_headers_count() {
        let mut snapshot = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        for i in 0..=MAX_HEADERS_ENTRIES {
            snapshot
                .signals
                .headers
                .insert(format!("X-Custom-{}", i), "value".to_string());
        }

        let result = validate_snapshot_compatibility(&snapshot);
        assert!(result.is_err(), "should reject excessive headers count");
    }

    #[test]
    fn reject_excessive_metadata_count() {
        let mut snapshot = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        for i in 0..=MAX_METADATA_ENTRIES {
            snapshot
                .metadata
                .insert(format!("key_{}", i), "value".to_string());
        }

        let result = validate_snapshot_compatibility(&snapshot);
        assert!(result.is_err(), "should reject excessive metadata count");
    }

    #[test]
    fn reject_oversized_webgl_vendor() {
        let mut snapshot = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        if let Some(webgl) = snapshot.signals.webgl.as_mut() {
            webgl.vendor = "X".repeat(MAX_STRING_FIELD_BYTES + 1);
        }

        let result = validate_snapshot_compatibility(&snapshot);
        assert!(result.is_err(), "should reject oversized webgl vendor");
    }

    #[test]
    fn reject_oversized_ja3_hash() {
        let mut snapshot = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        if let Some(tls) = snapshot.signals.tls.as_mut() {
            tls.ja3_hash = "X".repeat(MAX_HASH_FIELD_BYTES + 1);
        }

        let result = validate_snapshot_compatibility(&snapshot);
        assert!(result.is_err(), "should reject oversized ja3_hash");
    }

    #[test]
    fn reject_oversized_schema_version() {
        let mut snapshot = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        snapshot.schema_version = "1".repeat(MAX_STRING_FIELD_BYTES + 1);

        let result = validate_snapshot_compatibility(&snapshot);
        assert!(result.is_err(), "should reject oversized schema_version");
    }

    #[test]
    fn accept_valid_snapshot_with_max_sizes() {
        let mut snapshot = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        // Fill fields to max allowed sizes
        snapshot.signals.user_agent = "A".repeat(MAX_STRING_FIELD_BYTES);
        snapshot.signals.platform = "B".repeat(MAX_STRING_FIELD_BYTES - 1);

        let result = validate_snapshot_compatibility(&snapshot);
        assert!(result.is_ok(), "should accept snapshot at max limits");
    }

    #[test]
    fn reject_excessive_features_count() {
        let mut snapshot = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        for i in 0..=MAX_FEATURES_ENTRIES {
            snapshot
                .signals
                .features
                .insert(format!("feature_{}", i), false);
        }

        let result = validate_snapshot_compatibility(&snapshot);
        assert!(result.is_err(), "should reject excessive features count");
    }

    #[test]
    fn reject_large_header_value() {
        let mut snapshot = parse_snapshot(include_str!(
            "../docs/examples/fingerprint-snapshot-v1-http.json"
        ));
        snapshot.signals.headers.insert(
            "X-Large".to_string(),
            "V".repeat(MAX_STRING_FIELD_BYTES + 1),
        );

        let result = validate_snapshot_compatibility(&snapshot);
        assert!(result.is_err(), "should reject large header value");
    }

    #[test]
    fn deserialize_malformed_json_fails_gracefully() {
        let malformed = "{invalid json}";
        let result: Result<NormalizedFingerprintSnapshot, _> = serde_json::from_str(malformed);
        assert!(result.is_err(), "should fail on malformed JSON");
        // Error should not expose sensitive information
        let err_msg = result.unwrap_err().to_string();
        assert!(!err_msg.contains("secret"), "error should not leak secrets");
    }
}
