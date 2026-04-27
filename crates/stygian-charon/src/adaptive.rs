use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{BlockedRatioSlo, RequirementLevel, TargetClass};

/// Errors returned by adaptive SLO policy operations.
#[derive(Debug, Error)]
pub enum AdaptivePolicyError {
    /// The persisted history file could not be read.
    #[error("failed to read adaptive policy store '{path}': {source}")]
    ReadStore {
        /// Path of the failing store.
        path: PathBuf,
        /// Source I/O error.
        source: std::io::Error,
    },
    /// The persisted history file could not be parsed.
    #[error("failed to parse adaptive policy store '{path}': {source}")]
    ParseStore {
        /// Path of the failing store.
        path: PathBuf,
        /// Source JSON parsing error.
        source: serde_json::Error,
    },
    /// The history store could not be written.
    #[error("failed to write adaptive policy store '{path}': {source}")]
    WriteStore {
        /// Path of the failing store.
        path: PathBuf,
        /// Source I/O error.
        source: std::io::Error,
    },
    /// In-memory history could not be serialized.
    #[error("failed to serialize adaptive policy history: {0}")]
    Serialize(serde_json::Error),
}

/// Pluggable adaptive SLO policy interface.
///
/// # Example
///
/// ```no_run
/// use stygian_charon::{AdaptiveSloPolicy, BlockedRatioSlo, RegressionHistoryPolicy, RequirementLevel, TargetClass};
///
/// # fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let policy = RegressionHistoryPolicy::new();
/// policy.record_observation(
///     "https://example.com",
///     TargetClass::ContentSite,
///     0.18,
///     RequirementLevel::Medium,
/// )?;
///
/// let adapted = policy.select_slo(
///     "https://example.com",
///     TargetClass::ContentSite,
///     BlockedRatioSlo::content_site(),
/// );
/// assert!(adapted.acceptable <= adapted.warning);
/// # Ok(())
/// # }
/// ```
pub trait AdaptiveSloPolicy: Send + Sync {
    /// Select an adjusted SLO for a target using historical observations.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_charon::{AdaptiveSloPolicy, BlockedRatioSlo, RegressionHistoryPolicy, TargetClass};
    ///
    /// let policy = RegressionHistoryPolicy::new();
    /// let slo = policy.select_slo(
    ///     "https://example.com",
    ///     TargetClass::Api,
    ///     BlockedRatioSlo::api(),
    /// );
    /// assert!(slo.acceptable <= slo.warning);
    /// ```
    fn select_slo(
        &self,
        target: &str,
        target_class: TargetClass,
        default: BlockedRatioSlo,
    ) -> BlockedRatioSlo;

    /// Record a new blocked-ratio observation for a target.
    ///
    /// # Errors
    ///
    /// Returns [`AdaptivePolicyError`] when persistence fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_charon::{AdaptiveSloPolicy, RegressionHistoryPolicy, RequirementLevel, TargetClass};
    ///
    /// # fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let policy = RegressionHistoryPolicy::new();
    /// policy.record_observation(
    ///     "https://example.com",
    ///     TargetClass::Api,
    ///     0.04,
    ///     RequirementLevel::Low,
    /// )?;
    /// # Ok(())
    /// # }
    /// ```
    fn record_observation(
        &self,
        target: &str,
        target_class: TargetClass,
        blocked_ratio: f64,
        escalation_level: RequirementLevel,
    ) -> Result<(), AdaptivePolicyError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TargetObservation {
    target_class: TargetClass,
    blocked_ratio: f64,
    escalation_level: RequirementLevel,
    observed_at_unix_secs: u64,
}

#[derive(Debug, Clone)]
struct AdaptiveBounds {
    min_acceptable: f64,
    max_acceptable: f64,
    max_shift: f64,
    min_warning_gap: f64,
    min_critical_gap: f64,
}

impl AdaptiveBounds {
    const fn for_class(target_class: TargetClass) -> Self {
        match target_class {
            TargetClass::Api | TargetClass::Unknown => Self {
                min_acceptable: 0.01,
                max_acceptable: 0.20,
                max_shift: 0.08,
                min_warning_gap: 0.03,
                min_critical_gap: 0.03,
            },
            TargetClass::ContentSite => Self {
                min_acceptable: 0.05,
                max_acceptable: 0.35,
                max_shift: 0.12,
                min_warning_gap: 0.05,
                min_critical_gap: 0.06,
            },
            TargetClass::HighSecurity => Self {
                min_acceptable: 0.15,
                max_acceptable: 0.55,
                max_shift: 0.15,
                min_warning_gap: 0.08,
                min_critical_gap: 0.08,
            },
        }
    }
}

/// Adaptive SLO policy backed by per-target blocked-ratio history.
///
/// Uses bounded threshold shifts around default class SLOs. History can be
/// persisted to JSON for process restarts and operator inspection.
#[derive(Debug)]
pub struct RegressionHistoryPolicy {
    store_path: Option<PathBuf>,
    history: Mutex<BTreeMap<String, Vec<TargetObservation>>>,
    max_observations_per_target: usize,
}

impl RegressionHistoryPolicy {
    /// Create an in-memory adaptive policy with no persistence.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::RegressionHistoryPolicy;
    ///
    /// let policy = RegressionHistoryPolicy::new();
    /// assert!(policy.tracked_target_count() == 0);
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self {
            store_path: None,
            history: Mutex::new(BTreeMap::new()),
            max_observations_per_target: 256,
        }
    }

    /// Create an adaptive policy that persists history to a JSON file.
    ///
    /// If the file exists, it is loaded on startup.
    ///
    /// # Errors
    ///
    /// Returns [`AdaptivePolicyError`] when loading the store fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_charon::RegressionHistoryPolicy;
    ///
    /// # fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let _policy = RegressionHistoryPolicy::with_json_store("./charon-history.json")?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_json_store(path: impl AsRef<Path>) -> Result<Self, AdaptivePolicyError> {
        let path_buf = path.as_ref().to_path_buf();
        let history = if path_buf.exists() {
            let content =
                fs::read_to_string(&path_buf).map_err(|source| AdaptivePolicyError::ReadStore {
                    path: path_buf.clone(),
                    source,
                })?;
            serde_json::from_str::<BTreeMap<String, Vec<TargetObservation>>>(&content).map_err(
                |source| AdaptivePolicyError::ParseStore {
                    path: path_buf.clone(),
                    source,
                },
            )?
        } else {
            BTreeMap::new()
        };

        Ok(Self {
            store_path: Some(path_buf),
            history: Mutex::new(history),
            max_observations_per_target: 256,
        })
    }

    /// Number of unique targets tracked by history.
    #[must_use]
    pub fn tracked_target_count(&self) -> usize {
        let Ok(history) = self.history.lock() else {
            return 0;
        };
        history.len()
    }

    /// Number of observations currently retained for one target.
    #[must_use]
    pub fn observations_for_target(&self, target: &str) -> usize {
        let Ok(history) = self.history.lock() else {
            return 0;
        };
        history.get(target).map_or(0, Vec::len)
    }

    fn persist_locked(
        &self,
        history: &BTreeMap<String, Vec<TargetObservation>>,
    ) -> Result<(), AdaptivePolicyError> {
        let Some(path) = &self.store_path else {
            return Ok(());
        };

        let serialized =
            serde_json::to_string_pretty(history).map_err(AdaptivePolicyError::Serialize)?;
        fs::write(path, serialized).map_err(|source| AdaptivePolicyError::WriteStore {
            path: path.clone(),
            source,
        })
    }

    fn avg_blocked_ratio(observations: &[TargetObservation]) -> Option<f64> {
        if observations.is_empty() {
            return None;
        }

        let sum = observations.iter().map(|o| o.blocked_ratio).sum::<f64>();
        let count = observations.len();
        let Ok(count_u32) = u32::try_from(count) else {
            return None;
        };

        Some(sum / f64::from(count_u32))
    }

    const fn clamp_unit(value: f64) -> f64 {
        if value < 0.0 {
            0.0
        } else if value > 1.0 {
            1.0
        } else {
            value
        }
    }
}

impl Default for RegressionHistoryPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl AdaptiveSloPolicy for RegressionHistoryPolicy {
    fn select_slo(
        &self,
        target: &str,
        target_class: TargetClass,
        default: BlockedRatioSlo,
    ) -> BlockedRatioSlo {
        let Ok(history) = self.history.lock() else {
            return default;
        };

        let Some(observations) = history.get(target) else {
            return default;
        };

        let Some(avg_ratio) = Self::avg_blocked_ratio(observations) else {
            return default;
        };

        // Require at least three samples before adapting thresholds.
        if observations.len() < 3 {
            return default;
        }

        let bounds = AdaptiveBounds::for_class(target_class);
        let shift = (avg_ratio - default.acceptable).clamp(-bounds.max_shift, bounds.max_shift);

        let acceptable =
            (default.acceptable + shift).clamp(bounds.min_acceptable, bounds.max_acceptable);
        let warning = (default.warning + shift)
            .max(acceptable + bounds.min_warning_gap)
            .min(0.95);
        let critical = (default.critical + shift)
            .max(warning + bounds.min_critical_gap)
            .min(0.99);

        BlockedRatioSlo {
            target_class,
            acceptable: Self::clamp_unit(acceptable),
            warning: Self::clamp_unit(warning),
            critical: Self::clamp_unit(critical),
        }
    }

    fn record_observation(
        &self,
        target: &str,
        target_class: TargetClass,
        blocked_ratio: f64,
        escalation_level: RequirementLevel,
    ) -> Result<(), AdaptivePolicyError> {
        let Ok(mut history) = self.history.lock() else {
            return Ok(());
        };

        let entry = history.entry(target.to_string()).or_default();
        entry.push(TargetObservation {
            target_class,
            blocked_ratio: Self::clamp_unit(blocked_ratio),
            escalation_level,
            observed_at_unix_secs: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_secs()),
        });

        if entry.len() > self.max_observations_per_target {
            let overflow = entry.len() - self.max_observations_per_target;
            entry.drain(0..overflow);
        }

        self.persist_locked(&history)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_history_tracks_at_least_ten_targets() {
        let policy = RegressionHistoryPolicy::new();

        for index in 0_u32..10 {
            let target = format!("https://example{index}.com");
            let result = policy.record_observation(
                &target,
                TargetClass::ContentSite,
                0.10,
                RequirementLevel::Low,
            );
            assert!(result.is_ok(), "record_observation should succeed");
        }

        assert!(policy.tracked_target_count() >= 10);
    }

    #[test]
    fn adaptive_thresholds_preserve_zone_ordering() {
        let policy = RegressionHistoryPolicy::new();
        let target = "https://content.example";

        for ratio in [0.20, 0.22, 0.24, 0.26] {
            let result = policy.record_observation(
                target,
                TargetClass::ContentSite,
                ratio,
                RequirementLevel::Medium,
            );
            assert!(result.is_ok(), "record_observation should succeed");
        }

        let adjusted = policy.select_slo(
            target,
            TargetClass::ContentSite,
            BlockedRatioSlo::content_site(),
        );

        assert!(adjusted.acceptable <= adjusted.warning);
        assert!(adjusted.warning <= adjusted.critical);
        assert!(adjusted.acceptable >= 0.05);
        assert!(adjusted.critical <= 0.99);
    }

    #[test]
    fn json_store_round_trips_history() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let store = std::env::temp_dir().join(format!("adaptive-history-{unique}.json"));

        let create_result = RegressionHistoryPolicy::with_json_store(&store);
        assert!(create_result.is_ok(), "policy should initialize with store");
        let Ok(policy) = create_result else {
            return;
        };

        let write_result = policy.record_observation(
            "https://api.example",
            TargetClass::Api,
            0.08,
            RequirementLevel::Medium,
        );
        assert!(write_result.is_ok(), "record_observation should persist");

        let reload_result = RegressionHistoryPolicy::with_json_store(&store);
        assert!(reload_result.is_ok(), "policy should reload store");
        let Ok(reloaded) = reload_result else {
            return;
        };

        assert_eq!(reloaded.tracked_target_count(), 1);
        assert_eq!(reloaded.observations_for_target("https://api.example"), 1);

        let _ = fs::remove_file(store);
    }
}
