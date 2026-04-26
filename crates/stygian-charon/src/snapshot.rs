use std::collections::BTreeMap;

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
        let snap = parse_snapshot(include_str!("../docs/examples/fingerprint-snapshot-v1-http.json"));
        assert!(validate_snapshot_compatibility(&snap).is_ok());
    }

    #[test]
    fn example_browser_snapshot_is_compatible() {
        let snap =
            parse_snapshot(include_str!("../docs/examples/fingerprint-snapshot-v1-browser.json"));
        assert!(validate_snapshot_compatibility(&snap).is_ok());
    }

    #[test]
    fn http_mode_requires_tls_signal() {
        let mut snap =
            parse_snapshot(include_str!("../docs/examples/fingerprint-snapshot-v1-http.json"));
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
        let mut snap =
            parse_snapshot(include_str!("../docs/examples/fingerprint-snapshot-v1-browser.json"));
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
        let mut snap = parse_snapshot(include_str!("../docs/examples/fingerprint-snapshot-v1-http.json"));
        snap.schema_version = "2.0.0".to_string();
        let err =
            validate_snapshot_compatibility(&snap).expect_err("must fail unsupported major");
        assert_eq!(err, SnapshotCompatibilityError::UnsupportedSchemaMajor(2));
    }
}
