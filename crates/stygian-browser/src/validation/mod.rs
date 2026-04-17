//! Anti-bot service validation suite.
//!
//! Provides an automated testing framework that exercises stygian-browser's
//! stealth posture against real anti-bot detection services and open-source
//! fingerprint observatories.
//!
//! # Tier structure
//!
//! | Tier | Services | Rate limits | CI-safe |
//! |------|----------|------------|---------|
//! | 1 | [`CreepJs`], [`BrowserScan`] | None (open) | Yes |
//! | 2 | [`Kasada`], [`Cloudflare`], [`Akamai`] | Yes | `#[ignore]` |
//! | 3 | [`FingerprintJs`], [`DataDome`], [`PerimeterX`] | Account required | Manual |
//!
//! # Example
//!
//! ```no_run
//! use stygian_browser::validation::{ValidationSuite, ValidationTarget};
//! use stygian_browser::pool::BrowserPool;
//! use stygian_browser::BrowserConfig;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let pool = BrowserPool::new(BrowserConfig::default()).await?;
//! let targets = vec![ValidationTarget::CreepJs, ValidationTarget::BrowserScan];
//! let results = ValidationSuite::run_all(&pool, &targets).await;
//! for r in &results {
//!     println!("{}: passed={} score={:?}", r.target, r.passed, r.score);
//! }
//! # Ok(())
//! # }
//! ```

pub mod validators;

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::pool::BrowserPool;

// ---------------------------------------------------------------------------
// ValidationTarget
// ---------------------------------------------------------------------------

/// The anti-bot or fingerprint-observatory services that can be probed.
///
/// # Example
///
/// ```
/// use stygian_browser::validation::ValidationTarget;
///
/// assert_eq!(ValidationTarget::CreepJs.url(), "https://abrahamjuliot.github.io/creepjs/");
/// assert_eq!(ValidationTarget::all().len(), 8);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationTarget {
    /// `CreepJS` — open-source comprehensive fingerprint observatory (Tier 1).
    CreepJs,
    /// `BrowserScan` authenticity percentage (Tier 1).
    BrowserScan,
    /// `FingerprintJS` Pro — detects canvas/audio/WebGL inconsistency (Tier 3).
    FingerprintJs,
    /// Kasada — two-phase token, iframe checks (Tier 2).
    Kasada,
    /// Cloudflare Turnstile / Bot Management (Tier 2).
    Cloudflare,
    /// Akamai sensor-data collection (Tier 2).
    Akamai,
    /// `DataDome` — e-commerce behavioral analysis (Tier 3).
    DataDome,
    /// `PerimeterX` — behavioral + fingerprint (Tier 3).
    PerimeterX,
}

impl ValidationTarget {
    /// Canonical entry-point URL for this target.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::validation::ValidationTarget;
    ///
    /// assert!(ValidationTarget::CreepJs.url().starts_with("https://"));
    /// ```
    #[must_use]
    pub const fn url(self) -> &'static str {
        match self {
            Self::CreepJs => "https://abrahamjuliot.github.io/creepjs/",
            Self::BrowserScan => "https://www.browserscan.net/",
            Self::FingerprintJs => "https://fingerprint.com/demo/",
            Self::Kasada => "https://www.wizzair.com/",
            Self::Cloudflare => "https://www.cloudflare.com/",
            Self::Akamai => "https://www.fedex.com/",
            Self::DataDome => "https://datadome.co/",
            Self::PerimeterX => "https://www.humansecurity.com/",
        }
    }

    /// Whether this target is safe to run in automated CI (Tier 1 only).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::validation::ValidationTarget;
    ///
    /// assert!(ValidationTarget::CreepJs.is_ci_safe());
    /// assert!(!ValidationTarget::Kasada.is_ci_safe());
    /// ```
    #[must_use]
    pub const fn is_ci_safe(self) -> bool {
        matches!(self, Self::CreepJs | Self::BrowserScan)
    }

    /// All 8 targets, in enum declaration order.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::validation::ValidationTarget;
    ///
    /// assert_eq!(ValidationTarget::all().len(), 8);
    /// ```
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::CreepJs,
            Self::BrowserScan,
            Self::FingerprintJs,
            Self::Kasada,
            Self::Cloudflare,
            Self::Akamai,
            Self::DataDome,
            Self::PerimeterX,
        ]
    }

    /// CI-safe Tier 1 targets only.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::validation::ValidationTarget;
    ///
    /// assert_eq!(ValidationTarget::tier1().len(), 2);
    /// assert!(ValidationTarget::tier1().iter().all(|t| t.is_ci_safe()));
    /// ```
    #[must_use]
    pub const fn tier1() -> &'static [Self] {
        &[Self::CreepJs, Self::BrowserScan]
    }
}

impl fmt::Display for ValidationTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::CreepJs => "CreepJS",
            Self::BrowserScan => "BrowserScan",
            Self::FingerprintJs => "FingerprintJS",
            Self::Kasada => "Kasada",
            Self::Cloudflare => "Cloudflare",
            Self::Akamai => "Akamai",
            Self::DataDome => "DataDome",
            Self::PerimeterX => "PerimeterX",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// ValidationResult
// ---------------------------------------------------------------------------

/// The outcome of running a single anti-bot validator.
///
/// # Example
///
/// ```
/// use std::collections::HashMap;
/// use std::time::Duration;
/// use stygian_browser::validation::{ValidationResult, ValidationTarget};
///
/// let r = ValidationResult {
///     target: ValidationTarget::CreepJs,
///     passed: true,
///     score: Some(0.87),
///     details: HashMap::new(),
///     screenshot: None,
///     elapsed: Duration::from_secs(5),
/// };
/// assert!(r.passed);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    /// Which anti-bot service was tested.
    pub target: ValidationTarget,
    /// Did the page pass (not blocked, score above threshold)?
    pub passed: bool,
    /// Normalised 0.0–1.0 score, where applicable.
    pub score: Option<f64>,
    /// Target-specific extracted metrics as key/value pairs.
    pub details: HashMap<String, String>,
    /// PNG screenshot captured on failure (base64-encoded when serialised).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<Vec<u8>>,
    /// Wall-clock time taken for the validation.
    #[serde(with = "duration_secs")]
    pub elapsed: Duration,
}

impl ValidationResult {
    /// Construct a failure result without a screenshot.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::validation::{ValidationResult, ValidationTarget};
    ///
    /// let r = ValidationResult::failed(ValidationTarget::CreepJs, "timeout");
    /// assert!(!r.passed);
    /// assert!(r.details.contains_key("error"));
    /// ```
    #[must_use]
    pub fn failed(target: ValidationTarget, reason: &str) -> Self {
        Self {
            target,
            passed: false,
            score: None,
            details: HashMap::from([("error".to_string(), reason.to_string())]),
            screenshot: None,
            elapsed: Duration::ZERO,
        }
    }
}

// Serde helper: Duration ↔ f64 seconds
mod duration_secs {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<S>(d: &Duration, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        d.as_secs_f64().serialize(ser)
    }

    pub(super) fn deserialize<'de, D>(de: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        f64::deserialize(de).map(Duration::from_secs_f64)
    }
}

// ---------------------------------------------------------------------------
// ValidationSuite
// ---------------------------------------------------------------------------

/// Runs one or more anti-bot validators against the given [`BrowserPool`].
///
/// # Example
///
/// ```
/// use stygian_browser::validation::{ValidationSuite, ValidationTarget};
///
/// // Empty target list returns empty results immediately.
/// ```
pub struct ValidationSuite;

impl ValidationSuite {
    /// Run all specified targets sequentially and collect results.
    ///
    /// Returns immediately with an empty `Vec` if `targets` is empty.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::validation::{ValidationSuite, ValidationTarget};
    /// use stygian_browser::pool::BrowserPool;
    /// use stygian_browser::BrowserConfig;
    /// use std::sync::Arc;
    ///
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let pool = Arc::new(BrowserPool::new(BrowserConfig::default()).await?);
    /// let results = ValidationSuite::run_all(&pool, &[]).await;
    /// assert!(results.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn run_all(
        pool: &Arc<BrowserPool>,
        targets: &[ValidationTarget],
    ) -> Vec<ValidationResult> {
        // Run sequentially to avoid saturating the browser pool.
        let mut results = Vec::with_capacity(targets.len());
        for &target in targets {
            results.push(Self::run_one(pool, target).await);
        }
        results
    }

    /// Run a single validator and return its result.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::validation::{ValidationSuite, ValidationTarget};
    /// use stygian_browser::pool::BrowserPool;
    /// use stygian_browser::BrowserConfig;
    /// use std::sync::Arc;
    ///
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let pool = Arc::new(BrowserPool::new(BrowserConfig::default()).await?);
    /// let result = ValidationSuite::run_one(&pool, ValidationTarget::CreepJs).await;
    /// println!("passed: {}", result.passed);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn run_one(pool: &Arc<BrowserPool>, target: ValidationTarget) -> ValidationResult {
        match target {
            ValidationTarget::CreepJs => validators::run_creepjs(pool),
            ValidationTarget::BrowserScan => validators::run_browserscan(pool),
            ValidationTarget::Kasada => validators::run_kasada(pool).await,
            ValidationTarget::Cloudflare => validators::run_cloudflare(pool).await,
            ValidationTarget::Akamai => validators::run_akamai(pool).await,
            // Tier 3: not automated — return a documented stub result.
            ValidationTarget::FingerprintJs => ValidationResult::failed(
                target,
                "FingerprintJS Pro validation requires a Pro account — not automated",
            ),
            ValidationTarget::DataDome => ValidationResult::failed(
                target,
                "DataDome validation requires a Pro account — not automated",
            ),
            ValidationTarget::PerimeterX => ValidationResult::failed(
                target,
                "PerimeterX validation requires a Pro account — not automated",
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── ValidationResult serde round-trip ──────────────────────────────────────

    #[test]
    fn result_serde_round_trip() {
        let original = ValidationResult {
            target: ValidationTarget::CreepJs,
            passed: true,
            score: Some(0.92),
            details: HashMap::from([("trust_score".to_string(), "92%".to_string())]),
            screenshot: None,
            elapsed: Duration::from_millis(3800),
        };

        let json_result = serde_json::to_string(&original);
        assert!(json_result.is_ok(), "serialize failed: {json_result:?}");
        let Ok(json) = json_result else {
            return;
        };
        let decoded_result: Result<ValidationResult, _> = serde_json::from_str(&json);
        assert!(
            decoded_result.is_ok(),
            "deserialize failed: {decoded_result:?}"
        );
        let Ok(decoded) = decoded_result else {
            return;
        };

        assert_eq!(decoded.target, original.target);
        assert_eq!(decoded.passed, original.passed);
        assert!(decoded.score.is_some(), "missing score in decoded result");
        let Some(score) = decoded.score else {
            return;
        };
        assert!((score - 0.92_f64).abs() < 1e-9);
        let trust_score = decoded.details.get("trust_score");
        assert_eq!(trust_score, Some(&"92%".to_string()));
        assert!((decoded.elapsed.as_secs_f64() - 3.8_f64).abs() < 1e-6);
    }

    // ── Enum coverage ─────────────────────────────────────────────────────────

    #[test]
    fn all_targets_covered() {
        let all = ValidationTarget::all();
        assert_eq!(all.len(), 8, "all() must cover all 8 variants");

        // Spot-check URLs are non-empty HTTPS
        for t in all {
            let url = t.url();
            assert!(url.starts_with("https://"), "URL for {t} must use HTTPS");
        }
    }

    #[test]
    fn tier1_is_ci_safe() {
        let tier1 = ValidationTarget::tier1();
        assert_eq!(tier1.len(), 2);
        for t in tier1 {
            assert!(t.is_ci_safe(), "{t} must be CI-safe");
        }
    }

    #[test]
    fn tier2_not_ci_safe() {
        let tier2 = [
            ValidationTarget::Kasada,
            ValidationTarget::Cloudflare,
            ValidationTarget::Akamai,
        ];
        for t in tier2 {
            assert!(!t.is_ci_safe(), "{t} must NOT be CI-safe");
        }
    }

    // ── Display ───────────────────────────────────────────────────────────────

    #[test]
    fn display_names() {
        assert_eq!(ValidationTarget::CreepJs.to_string(), "CreepJS");
        assert_eq!(ValidationTarget::BrowserScan.to_string(), "BrowserScan");
        assert_eq!(ValidationTarget::FingerprintJs.to_string(), "FingerprintJS");
        assert_eq!(ValidationTarget::Kasada.to_string(), "Kasada");
        assert_eq!(ValidationTarget::Cloudflare.to_string(), "Cloudflare");
        assert_eq!(ValidationTarget::Akamai.to_string(), "Akamai");
        assert_eq!(ValidationTarget::DataDome.to_string(), "DataDome");
        assert_eq!(ValidationTarget::PerimeterX.to_string(), "PerimeterX");
    }

    // ── Integration (requires network + browser) ──────────────────────────────

    #[tokio::test]
    #[ignore = "requires network connectivity and a running Chrome binary"]
    async fn live_creepjs_returns_score() {
        use crate::BrowserConfig;
        use crate::pool::BrowserPool;

        let pool_result = BrowserPool::new(BrowserConfig::default()).await;
        assert!(pool_result.is_ok(), "pool init failed");
        let Ok(pool) = pool_result else {
            return;
        };
        let result = ValidationSuite::run_one(&pool, ValidationTarget::CreepJs).await;
        assert!(
            result.score.is_some(),
            "CreepJS should return a score: {:?}",
            result.details
        );
    }

    #[tokio::test]
    #[ignore = "requires network connectivity and a running Chrome binary"]
    async fn live_browserscan_returns_percentage() {
        use crate::BrowserConfig;
        use crate::pool::BrowserPool;

        let pool_result = BrowserPool::new(BrowserConfig::default()).await;
        assert!(pool_result.is_ok(), "pool init failed");
        let Ok(pool) = pool_result else {
            return;
        };
        let result = ValidationSuite::run_one(&pool, ValidationTarget::BrowserScan).await;
        assert!(
            result.score.is_some(),
            "BrowserScan should return a score: {:?}",
            result.details
        );
    }

    #[tokio::test]
    #[ignore = "requires network connectivity and a running Chrome binary"]
    async fn live_kasada_wizzair_not_blocked() {
        use crate::BrowserConfig;
        use crate::pool::BrowserPool;

        let pool_result = BrowserPool::new(BrowserConfig::default()).await;
        assert!(pool_result.is_ok(), "pool init failed");
        let Ok(pool) = pool_result else {
            return;
        };
        let result = ValidationSuite::run_one(&pool, ValidationTarget::Kasada).await;
        assert!(
            result.passed,
            "WizzAir should not block us: {:?}",
            result.details
        );
    }
}
