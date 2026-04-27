use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

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

impl SnapshotDriftReport {
    /// Return `true` when any signal drift was detected.
    #[must_use]
    pub fn has_drift(&self) -> bool {
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
    let mut parts = version.split('.');
    let Some(major) = parts.next() else {
        return Err(SnapshotCompatibilityError::InvalidSchemaVersion(
            version.to_string(),
        ));
    };
    let has_minor = parts.next().is_some();
    let has_patch = parts.next().is_some();
    if !(has_minor && has_patch) {
        return Err(SnapshotCompatibilityError::InvalidSchemaVersion(
            version.to_string(),
        ));
    }
    major
        .parse::<u64>()
        .map_err(|_| SnapshotCompatibilityError::InvalidSchemaVersion(version.to_string()))
}

/// Validate normalized snapshot compatibility rules across modes and versions.
///
/// Current compatibility contract:
/// - supports schema major version `1`
/// - requires `signals.tls` for [`SnapshotMode::Http`]
/// - requires `signals.webgl` for [`SnapshotMode::Browser`]
/// - requires deprecated mirror fields, when present, to match canonical fields
pub fn validate_snapshot_compatibility(
    snapshot: &NormalizedFingerprintSnapshot,
) -> Result<(), SnapshotCompatibilityError> {
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
mod tests {
    use super::*;

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
}
