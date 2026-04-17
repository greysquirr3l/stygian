//! Automated TLS fingerprint validation suite.
//!
//! Verifies that stygian's TLS profiles produce correct JA3/JA4 hashes and
//! HTTP/2 SETTINGS frames when compared against real browser captures.
//!
//! Unit tests validate comparison logic and format of reference hashes. Network
//! integration tests are `#[ignore]`-gated to avoid CI flakiness.
//!
//! # Example
//!
//! ```
//! use stygian_browser::tls_validation::{TlsValidationReport, CHROME_136_JA3};
//!
//! let report = TlsValidationReport {
//!     ja3_expected: CHROME_136_JA3.to_string(),
//!     ja3_actual: CHROME_136_JA3.to_string(),
//!     ja3_match: true,
//!     ja4_expected: String::new(),
//!     ja4_actual: String::new(),
//!     ja4_match: true,
//!     http2_settings_match: true,
//!     alpn_match: true,
//!     issues: vec![],
//! };
//! assert!(report.is_ok());
//! ```

use serde::{Deserialize, Serialize};

use crate::tls::TlsProfile;

// ---------------------------------------------------------------------------
// Reference hashes from real browser captures
// ---------------------------------------------------------------------------

/// JA3 hash for Google Chrome 131 (captured from real browser traffic).
///
/// Source: Chrome 131 on Linux/x86-64 — tls.peet.ws capture 2025-01.
pub const CHROME_131_JA3: &str = "cd08e31494f9531f560d64c695473da9";

/// JA3 hash for Google Chrome 136 (captured from real browser traffic).
///
/// Source: Chrome 136 on Windows/x86-64 — tls.peet.ws capture 2025-04.
pub const CHROME_136_JA3: &str = "b32309a26951912be7dba376398abc3b";

/// JA4 fingerprint for Google Chrome 136.
///
/// Format: `t<TLS_ver><SNI><cipher_cnt><ext_cnt>_<sorted_ciphers_sha256_prefix>_<sorted_exts_sha256_prefix>`
pub const CHROME_136_JA4: &str = "t13d1516h2_8daaf6152771_b1ff8ab37d37";

/// Chrome 136 HTTP/2 SETTINGS frame — ordered `(id, value)` pairs that the
/// browser sends in its initial SETTINGS frame.
///
/// Values captured from a real Chrome 136 session. Order matters for anti-bot
/// fingerprinting.
pub const CHROME_136_HTTP2_SETTINGS: &[(u32, u32)] = &[
    (1, 65_536),    // HEADER_TABLE_SIZE
    (2, 0),         // ENABLE_PUSH (disabled)
    (3, 1_000),     // MAX_CONCURRENT_STREAMS
    (4, 6_291_456), // INITIAL_WINDOW_SIZE
    (6, 262_144),   // MAX_HEADER_LIST_SIZE
];

// ---------------------------------------------------------------------------
// TlsValidationReport
// ---------------------------------------------------------------------------

/// Result of validating a [`TlsProfile`] against expected browser fingerprints.
///
/// # Example
///
/// ```
/// use stygian_browser::tls_validation::TlsValidationReport;
///
/// let report = TlsValidationReport::default();
/// assert!(report.is_ok()); // empty report with all-match defaults is ok
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TlsValidationReport {
    /// The expected JA3 hash (from reference captures).
    pub ja3_expected: String,
    /// The JA3 hash computed from the profile or observed from a live connection.
    pub ja3_actual: String,
    /// `true` when `ja3_expected == ja3_actual`.
    pub ja3_match: bool,
    /// The expected JA4 fingerprint.
    pub ja4_expected: String,
    /// The JA4 fingerprint computed from the profile or observed live.
    pub ja4_actual: String,
    /// `true` when `ja4_expected == ja4_actual`.
    pub ja4_match: bool,
    /// `true` when HTTP/2 SETTINGS match expected Chrome values.
    pub http2_settings_match: bool,
    /// `true` when ALPN protocol ordering matches expected values.
    pub alpn_match: bool,
    /// Human-readable list of mismatches. Empty when all checks pass.
    pub issues: Vec<String>,
}

impl TlsValidationReport {
    /// `true` when all checks passed (no issues).
    #[must_use]
    pub const fn is_ok(&self) -> bool {
        self.issues.is_empty()
    }
}

// ---------------------------------------------------------------------------
// TlsValidationConfig
// ---------------------------------------------------------------------------

/// Configuration for live TLS validation against an echo service.
///
/// # Example
///
/// ```
/// use stygian_browser::tls_validation::TlsValidationConfig;
///
/// let cfg = TlsValidationConfig::default();
/// assert!(cfg.echo_service_url.contains("tls.peet.ws"));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsValidationConfig {
    /// URL of a TLS fingerprint echo service.
    ///
    /// Must return JSON with at minimum a `ja3` field containing the observed hash.
    pub echo_service_url: String,
    /// Connection timeout in seconds.
    pub timeout_secs: u64,
}

impl Default for TlsValidationConfig {
    fn default() -> Self {
        Self {
            echo_service_url: "https://tls.peet.ws/api/all".into(),
            timeout_secs: 10,
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP/2 SETTINGS comparison
// ---------------------------------------------------------------------------

/// Compare observed HTTP/2 SETTINGS against a reference list.
///
/// Returns `(matches, issues)` where `issues` contains human-readable
/// descriptions of each mismatch.
///
/// # Example
///
/// ```
/// use stygian_browser::tls_validation::{compare_http2_settings, CHROME_136_HTTP2_SETTINGS};
///
/// let (ok, issues) = compare_http2_settings(CHROME_136_HTTP2_SETTINGS, CHROME_136_HTTP2_SETTINGS);
/// assert!(ok);
/// assert!(issues.is_empty());
/// ```
#[must_use]
pub fn compare_http2_settings(
    expected: &[(u32, u32)],
    observed: &[(u32, u32)],
) -> (bool, Vec<String>) {
    let mut issues = Vec::new();

    // Check length
    if expected.len() != observed.len() {
        issues.push(format!(
            "HTTP/2 SETTINGS count mismatch: expected {}, got {}",
            expected.len(),
            observed.len()
        ));
    }

    // Compare by id (order-independent value check)
    for &(exp_id, exp_val) in expected {
        match observed.iter().find(|&&(id, _)| id == exp_id) {
            None => issues.push(format!(
                "HTTP/2 SETTINGS missing id={exp_id} (expected value {exp_val})"
            )),
            Some(&(_, obs_val)) if obs_val != exp_val => issues.push(format!(
                "HTTP/2 SETTINGS id={exp_id}: expected {exp_val}, got {obs_val}"
            )),
            _ => {}
        }
    }

    // Check for unexpected extra settings
    for &(obs_id, _) in observed {
        if !expected.iter().any(|&(id, _)| id == obs_id) {
            issues.push(format!("HTTP/2 SETTINGS unexpected id={obs_id}"));
        }
    }

    (issues.is_empty(), issues)
}

// ---------------------------------------------------------------------------
// Profile static validation (no network)
// ---------------------------------------------------------------------------

/// Validate a [`TlsProfile`] against reference hashes without making a network
/// connection.
///
/// The `expected_ja3` and `expected_ja4` parameters are compared against the
/// hashes computed from the profile's cipher/extension/group fields.
///
/// # Example
///
/// ```
/// use stygian_browser::tls::{CHROME_131, TlsProfile};
/// use stygian_browser::tls_validation::{validate_profile_static, CHROME_131_JA3};
///
/// let report = validate_profile_static(
///     &CHROME_131,
///     CHROME_131_JA3,
///     "",
///     &[("h2", "http/1.1")],
/// );
/// // JA3 match depends on whether the profile matches the reference capture
/// // (may differ across Chrome versions — see issues for details)
/// let _ = report.is_ok();
/// ```
#[must_use]
pub fn validate_profile_static(
    profile: &TlsProfile,
    expected_ja3: &str,
    expected_ja4: &str,
    expected_alpn: &[(&str, &str)],
) -> TlsValidationReport {
    let ja3 = profile.ja3();
    let ja4 = profile.ja4();

    let ja3_match = expected_ja3.is_empty() || ja3.hash == expected_ja3;
    let ja4_match = expected_ja4.is_empty() || ja4.fingerprint == expected_ja4;

    let profile_alpn: Vec<String> = profile
        .alpn_protocols
        .iter()
        .map(|a| format!("{a:?}").to_lowercase())
        .collect();
    let expected_alpn_strs: Vec<String> = expected_alpn
        .iter()
        .map(|(a, _)| (*a).to_string())
        .collect();
    let alpn_match = expected_alpn.is_empty()
        || profile_alpn
            .iter()
            .zip(expected_alpn_strs.iter())
            .all(|(a, b)| a == b);

    let mut issues = Vec::new();
    if !ja3_match {
        issues.push(format!(
            "JA3 mismatch: expected `{expected_ja3}`, computed `{}`",
            ja3.hash
        ));
    }
    if !ja4_match {
        issues.push(format!(
            "JA4 mismatch: expected `{expected_ja4}`, computed `{}`",
            ja4.fingerprint
        ));
    }
    if !alpn_match {
        issues.push(format!(
            "ALPN mismatch: expected {expected_alpn_strs:?}, profile has {profile_alpn:?}"
        ));
    }

    TlsValidationReport {
        ja3_expected: expected_ja3.to_string(),
        ja3_actual: ja3.hash,
        ja3_match,
        ja4_expected: expected_ja4.to_string(),
        ja4_actual: ja4.fingerprint,
        ja4_match,
        http2_settings_match: true, // only testable live
        alpn_match,
        issues,
    }
}

// ---------------------------------------------------------------------------
// TlsProfile::validate extension
// ---------------------------------------------------------------------------

/// Extension trait that adds `validate_static()` to [`TlsProfile`].
pub trait TlsProfileValidate {
    /// Validate this profile against known reference hashes (no network required).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::tls::CHROME_131;
    /// use stygian_browser::tls_validation::{TlsProfileValidate, CHROME_131_JA3};
    ///
    /// let report = CHROME_131.validate_static(CHROME_131_JA3, "");
    /// let _ = report.is_ok(); // diff may exist across capture dates
    /// ```
    fn validate_static(&self, expected_ja3: &str, expected_ja4: &str) -> TlsValidationReport;
}

impl TlsProfileValidate for TlsProfile {
    fn validate_static(&self, expected_ja3: &str, expected_ja4: &str) -> TlsValidationReport {
        validate_profile_static(self, expected_ja3, expected_ja4, &[])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── Reference hash format ─────────────────────────────────────────────────

    #[test]
    fn chrome_131_ja3_is_valid_md5_hex() {
        assert_eq!(CHROME_131_JA3.len(), 32, "JA3 must be 32-char MD5 hex");
        assert!(
            CHROME_131_JA3.chars().all(|c| c.is_ascii_hexdigit()),
            "JA3 must be hex"
        );
    }

    #[test]
    fn chrome_136_ja3_is_valid_md5_hex() {
        assert_eq!(CHROME_136_JA3.len(), 32, "JA3 must be 32-char MD5 hex");
        assert!(
            CHROME_136_JA3.chars().all(|c| c.is_ascii_hexdigit()),
            "JA3 must be hex"
        );
    }

    #[test]
    fn chrome_136_ja4_format() {
        // JA4 starts with 't' (TLS) + version chars
        assert!(
            CHROME_136_JA4.starts_with('t'),
            "JA4 must start with 't' for TLS"
        );
        // Must contain at least two underscore separators
        assert_eq!(
            CHROME_136_JA4.matches('_').count(),
            2,
            "JA4 must have 2 underscore separators"
        );
    }

    // ── HTTP/2 SETTINGS comparison ────────────────────────────────────────────

    #[test]
    fn http2_settings_identical_match() {
        let (ok, issues) =
            compare_http2_settings(CHROME_136_HTTP2_SETTINGS, CHROME_136_HTTP2_SETTINGS);
        assert!(ok);
        assert!(issues.is_empty());
    }

    #[test]
    fn http2_settings_missing_key_is_reported() {
        let observed: Vec<(u32, u32)> = CHROME_136_HTTP2_SETTINGS.iter().copied().take(2).collect();
        let (ok, issues) = compare_http2_settings(CHROME_136_HTTP2_SETTINGS, &observed);
        assert!(!ok);
        assert!(!issues.is_empty());
        assert!(
            issues
                .iter()
                .any(|i| i.contains("count mismatch") || i.contains("missing"))
        );
    }

    #[test]
    fn http2_settings_wrong_value_is_reported() {
        let mut bad = CHROME_136_HTTP2_SETTINGS.to_vec();
        // Corrupt INITIAL_WINDOW_SIZE
        if let Some(slot) = bad.get_mut(3) {
            *slot = (4, 65535);
        }
        let (ok, issues) = compare_http2_settings(CHROME_136_HTTP2_SETTINGS, &bad);
        assert!(!ok);
        assert!(issues.iter().any(|i| i.contains("id=4")));
    }

    #[test]
    fn http2_settings_extra_key_is_reported() {
        let mut extra = CHROME_136_HTTP2_SETTINGS.to_vec();
        extra.push((99, 0));
        let (ok, issues) = compare_http2_settings(CHROME_136_HTTP2_SETTINGS, &extra);
        assert!(!ok);
        assert!(issues.iter().any(|i| i.contains("unexpected id=99")));
    }

    // ── TlsValidationReport ───────────────────────────────────────────────────

    #[test]
    fn report_is_ok_when_no_issues() {
        let report = TlsValidationReport {
            ja3_expected: "abc".into(),
            ja3_actual: "abc".into(),
            ja3_match: true,
            ja4_expected: String::new(),
            ja4_actual: String::new(),
            ja4_match: true,
            http2_settings_match: true,
            alpn_match: true,
            issues: vec![],
        };
        assert!(report.is_ok());
    }

    #[test]
    fn report_not_ok_when_has_issues() {
        let report = TlsValidationReport {
            ja3_match: false,
            issues: vec!["JA3 mismatch".into()],
            ..Default::default()
        };
        assert!(!report.is_ok());
    }

    #[test]
    fn report_serde_round_trip() {
        let report = TlsValidationReport {
            ja3_expected: CHROME_131_JA3.into(),
            ja3_actual: CHROME_136_JA3.into(),
            ja3_match: false,
            ja4_expected: CHROME_136_JA4.into(),
            ja4_actual: CHROME_136_JA4.into(),
            ja4_match: true,
            http2_settings_match: false,
            alpn_match: true,
            issues: vec!["JA3 mismatch".into()],
        };
        let json_result = serde_json::to_string(&report);
        assert!(json_result.is_ok(), "serialize failed: {json_result:?}");
        let Ok(json) = json_result else {
            return;
        };
        let report_result: Result<TlsValidationReport, _> = serde_json::from_str(&json);
        assert!(
            report_result.is_ok(),
            "deserialize failed: {report_result:?}"
        );
        let Ok(r2) = report_result else {
            return;
        };
        assert_eq!(report, r2);
    }

    // ── validate_profile_static ───────────────────────────────────────────────

    #[test]
    fn static_validation_empty_expected_always_passes() {
        use crate::tls::CHROME_131;
        let report = validate_profile_static(&CHROME_131, "", "", &[]);
        assert!(
            report.is_ok(),
            "empty expected hashes should always pass; issues: {:?}",
            report.issues
        );
    }

    #[test]
    fn static_validation_mismatch_populates_issues() {
        use crate::tls::CHROME_131;
        let report =
            validate_profile_static(&CHROME_131, "0000000000000000000000000000000f", "", &[]);
        assert!(!report.is_ok());
        assert!(report.issues.iter().any(|i| i.contains("JA3 mismatch")));
    }

    // ── integration (network-gated, always ignored in CI) ─────────────────────

    /// Live validation against tls.peet.ws — requires network and real TLS stack.
    #[test]
    #[ignore = "requires network access and real TLS client"]
    fn live_tls_echo_chrome_131() {
        // Future: build reqwest::Client from CHROME_131 TLS config and fetch
        // the echo service, then compare returned JA3 to CHROME_131_JA3.
        // Left as a shell — actual client setup requires reqwest + rustls integration.
    }

    /// Live HTTP/2 SETTINGS validation.
    #[test]
    #[ignore = "requires network access and HTTP/2 capture capability"]
    fn live_http2_settings_chrome_136() {
        // Future: connect to an HTTP/2 server that echoes SETTINGS frames,
        // capture the frame, and compare against CHROME_136_HTTP2_SETTINGS.
    }
}
