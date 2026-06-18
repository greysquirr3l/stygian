//! Playbook schema types (T85).
//!
//! Every type is `serde::Deserialize` so the TOML files in
//! `crates/stygian-charon/data/playbooks/` round-trip through it
//! directly. Validation is performed separately by
//! [`Playbook::validate`] so that operators get **structured errors**
//! with the field path and the bad value, rather than the
//! "TOML parse failed" line/column output that serde gives by default.

use serde::{Deserialize, Serialize};

use crate::acquisition::AcquisitionModeHint;
use crate::playbooks::error::ValidationError;
use crate::types::{ExecutionMode, SessionMode, TargetClass, TelemetryLevel};

/// Acquisition-mode defaults recommended by the playbook.
///
/// These map directly to the input shape of
/// [`crate::acquisition::RuntimePolicyHints`] — a resolved playbook
/// can therefore be fed into [`crate::acquisition::map_policy_hints`]
/// to produce a downstream [`crate::acquisition::AcquisitionPolicy`].
///
/// # Example
///
/// ```
/// use stygian_charon::playbooks::AcquisitionDefaults;
/// use stygian_charon::acquisition::AcquisitionModeHint;
/// use stygian_charon::types::{ExecutionMode, SessionMode, TargetClass, TelemetryLevel};
///
/// let defaults = AcquisitionDefaults::default_for(TargetClass::ContentSite);
/// assert_eq!(defaults.mode, AcquisitionModeHint::Resilient);
/// assert_eq!(defaults.execution_mode, ExecutionMode::Http);
/// assert_eq!(defaults.session_mode, SessionMode::Stateless);
/// assert_eq!(defaults.telemetry_level, TelemetryLevel::Standard);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcquisitionDefaults {
    /// Recommended acquisition mode (see [`AcquisitionModeHint`]).
    pub mode: AcquisitionModeHint,
    /// Recommended execution mode.
    pub execution_mode: ExecutionMode,
    /// Recommended session mode.
    pub session_mode: SessionMode,
    /// Recommended telemetry level.
    pub telemetry_level: TelemetryLevel,
    /// Suggested sticky-session TTL in seconds (only meaningful when
    /// `session_mode == SessionMode::Sticky`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sticky_session_ttl_secs: Option<u64>,
    /// Whether browser warm-up navigation is recommended.
    #[serde(default)]
    pub enable_warmup: bool,
    /// Retry budget for transient failures. Must be `> 0` after
    /// validation.
    #[serde(default = "default_retry_budget")]
    pub retry_budget: u32,
    /// Base backoff in milliseconds. Must be `> 0` after validation.
    #[serde(default = "default_backoff_ms")]
    pub backoff_base_ms: u64,
}

const fn default_retry_budget() -> u32 {
    2
}

const fn default_backoff_ms() -> u64 {
    250
}

impl AcquisitionDefaults {
    /// Build acquisition defaults appropriate for a target class.
    ///
    /// The defaults match the values returned by
    /// [`crate::acquisition::map_policy_hints`] when no overrides are
    /// supplied, so a playbook's `acquisition` block can be left
    /// blank and still produce a coherent runner config.
    #[must_use]
    pub const fn default_for(target_class: TargetClass) -> Self {
        match target_class {
            TargetClass::Api => Self {
                mode: AcquisitionModeHint::Fast,
                execution_mode: ExecutionMode::Http,
                session_mode: SessionMode::Stateless,
                telemetry_level: TelemetryLevel::Standard,
                sticky_session_ttl_secs: None,
                enable_warmup: false,
                retry_budget: 2,
                backoff_base_ms: 250,
            },
            TargetClass::ContentSite | TargetClass::Unknown => Self {
                mode: AcquisitionModeHint::Resilient,
                execution_mode: ExecutionMode::Http,
                session_mode: SessionMode::Stateless,
                telemetry_level: TelemetryLevel::Standard,
                sticky_session_ttl_secs: None,
                enable_warmup: false,
                retry_budget: 2,
                backoff_base_ms: 250,
            },
            TargetClass::HighSecurity => Self {
                mode: AcquisitionModeHint::Hostile,
                execution_mode: ExecutionMode::Browser,
                session_mode: SessionMode::Sticky,
                telemetry_level: TelemetryLevel::Deep,
                sticky_session_ttl_secs: Some(900),
                enable_warmup: true,
                retry_budget: 4,
                backoff_base_ms: 500,
            },
        }
    }
}

impl Default for AcquisitionDefaults {
    fn default() -> Self {
        Self::default_for(TargetClass::Unknown)
    }
}

/// Per-request overrides for the acquisition block. Only fields set
/// (`Some`) participate in the precedence test; absent fields fall
/// back to the playbook default and then the global default.
///
/// # Example
///
/// ```
/// use stygian_charon::playbooks::AcquisitionOverrides;
/// use stygian_charon::acquisition::AcquisitionModeHint;
///
/// let overrides = AcquisitionOverrides {
///     mode: Some(AcquisitionModeHint::Hostile),
///     ..AcquisitionOverrides::default()
/// };
/// assert_eq!(overrides.mode, Some(AcquisitionModeHint::Hostile));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcquisitionOverrides {
    /// Optional mode override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<AcquisitionModeHint>,
    /// Optional execution-mode override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<ExecutionMode>,
    /// Optional session-mode override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_mode: Option<SessionMode>,
    /// Optional telemetry-level override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry_level: Option<TelemetryLevel>,
    /// Optional retry-budget override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_budget: Option<u32>,
    /// Optional backoff-base override (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff_base_ms: Option<u64>,
    /// Optional warmup-flag override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_warmup: Option<bool>,
}

/// Proxy flavour + sticky-session constraints for the playbook.
///
/// # Example
///
/// ```
/// use stygian_charon::playbooks::ProxyPreference;
/// use stygian_charon::types::TargetClass;
///
/// let pref = ProxyPreference::default_for(TargetClass::HighSecurity);
/// assert!(pref.require_sticky);
/// assert!(pref.max_latency_ms.is_some());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyPreference {
    /// Preferred wire protocol (`"http"`, `"https"`, or `"socks5"`).
    pub preferred_protocol: String,
    /// Whether the proxy must hold a sticky IP/identity across the
    /// session.
    #[serde(default)]
    pub require_sticky: bool,
    /// Whether the proxy must be residential (i.e. not a datacenter).
    #[serde(default)]
    pub require_residential: bool,
    /// Optional upper bound on acceptable proxy latency (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_latency_ms: Option<u64>,
}

const SUPPORTED_PROXY_PROTOCOLS: &[&str] = &["http", "https", "socks5"];

impl ProxyPreference {
    /// Build a proxy preference appropriate for a target class.
    #[must_use]
    pub fn default_for(target_class: TargetClass) -> Self {
        match target_class {
            TargetClass::Api | TargetClass::Unknown => Self {
                preferred_protocol: "https".to_string(),
                require_sticky: false,
                require_residential: false,
                max_latency_ms: Some(2_000),
            },
            TargetClass::ContentSite => Self {
                preferred_protocol: "https".to_string(),
                require_sticky: false,
                require_residential: false,
                max_latency_ms: Some(1_500),
            },
            TargetClass::HighSecurity => Self {
                preferred_protocol: "https".to_string(),
                require_sticky: true,
                require_residential: true,
                max_latency_ms: Some(800),
            },
        }
    }
}

impl Default for ProxyPreference {
    fn default() -> Self {
        Self::default_for(TargetClass::Unknown)
    }
}

/// Pacing knobs (rate, jitter, minimum inter-request interval).
///
/// # Example
///
/// ```
/// use stygian_charon::playbooks::PacingProfile;
/// use stygian_charon::types::TargetClass;
///
/// let pacing = PacingProfile::default_for(TargetClass::HighSecurity);
/// assert!(pacing.rate_limit_rps <= 1.0);
/// assert!(pacing.min_request_interval_ms >= 1_000);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PacingProfile {
    /// Sustained requests per second. Must be `> 0`.
    pub rate_limit_rps: f64,
    /// Jitter as a fraction of the inter-request interval (0.0–1.0).
    pub jitter_pct: f64,
    /// Minimum inter-request interval in milliseconds.
    pub min_request_interval_ms: u64,
}

impl PacingProfile {
    /// Build a pacing profile appropriate for a target class.
    #[must_use]
    pub const fn default_for(target_class: TargetClass) -> Self {
        match target_class {
            TargetClass::Api => Self {
                rate_limit_rps: 5.0,
                jitter_pct: 0.05,
                min_request_interval_ms: 200,
            },
            TargetClass::ContentSite => Self {
                rate_limit_rps: 3.0,
                jitter_pct: 0.10,
                min_request_interval_ms: 300,
            },
            TargetClass::HighSecurity => Self {
                rate_limit_rps: 0.5,
                jitter_pct: 0.25,
                min_request_interval_ms: 2_000,
            },
            TargetClass::Unknown => Self {
                rate_limit_rps: 2.0,
                jitter_pct: 0.10,
                min_request_interval_ms: 500,
            },
        }
    }
}

impl Default for PacingProfile {
    fn default() -> Self {
        Self::default_for(TargetClass::Unknown)
    }
}

/// Escalation ladder the runner should climb on transient failure.
///
/// The runner walks the ladder top-to-bottom; the **first** stage
/// that returns a non-error result wins. `Capped` collapses the
/// ladder into a single-mode ceiling so the runner only retries at
/// the original mode plus the listed neighbours.
///
/// # Example
///
/// ```
/// use stygian_charon::playbooks::EscalationStrategy;
/// use stygian_charon::acquisition::AcquisitionModeHint;
///
/// let capped = EscalationStrategy::Capped {
///     ceiling: AcquisitionModeHint::Hostile,
/// };
/// let linear = EscalationStrategy::Linear {
///     steps: vec![AcquisitionModeHint::Fast, AcquisitionModeHint::Hostile],
/// };
/// assert_eq!(capped.ceiling(), AcquisitionModeHint::Hostile);
/// assert_eq!(linear.stages(), vec![AcquisitionModeHint::Fast, AcquisitionModeHint::Hostile]);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EscalationStrategy {
    /// A single ceiling mode — the runner may escalate up to (and
    /// including) the ceiling mode but not beyond.
    Capped {
        /// Upper bound on the runner's mode escalation.
        ceiling: AcquisitionModeHint,
    },
    /// An explicit ordered list of stages the runner walks.
    Linear {
        /// Ordered escalation stages (top-to-bottom).
        steps: Vec<AcquisitionModeHint>,
    },
}

impl EscalationStrategy {
    /// Upper bound the runner may escalate to.
    #[must_use]
    pub fn ceiling(&self) -> AcquisitionModeHint {
        match self {
            Self::Capped { ceiling } => *ceiling,
            Self::Linear { steps } => steps
                .last()
                .copied()
                .unwrap_or(AcquisitionModeHint::Resilient),
        }
    }

    /// First stage the runner attempts.
    #[must_use]
    pub fn first(&self) -> AcquisitionModeHint {
        match self {
            Self::Capped { ceiling } => *ceiling,
            Self::Linear { steps } => steps
                .first()
                .copied()
                .unwrap_or(AcquisitionModeHint::Resilient),
        }
    }

    /// Ordered list of stages the runner walks (deduplicated, order
    /// preserved).
    #[must_use]
    pub fn stages(&self) -> Vec<AcquisitionModeHint> {
        match self {
            Self::Capped { ceiling } => vec![*ceiling],
            Self::Linear { steps } => {
                let mut seen: Vec<AcquisitionModeHint> = Vec::new();
                for stage in steps {
                    if !seen.contains(stage) {
                        seen.push(*stage);
                    }
                }
                seen
            }
        }
    }
}

impl Default for EscalationStrategy {
    fn default() -> Self {
        Self::Capped {
            ceiling: AcquisitionModeHint::Resilient,
        }
    }
}

/// Tag describing which tier of the precedence ladder contributed
/// each field to a [`ResolvedPlaybook`](crate::playbooks::ResolvedPlaybook).
///
/// Used by `crate::playbooks::ResolvedPlaybook` source metadata fields so
/// downstream observers can verify the deterministic precedence is
/// being honoured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionSource {
    /// The value was set by the per-request override (top priority).
    RequestOverride,
    /// The value came from the playbook's own default.
    PlaybookDefault,
    /// The value fell through to the resolver's global default.
    GlobalDefault,
}

/// A single codified playbook for one anti-bot tier.
///
/// # Example
///
/// ```
/// use stygian_charon::playbooks::{
///     AcquisitionDefaults, EscalationStrategy, PacingProfile, Playbook, ProxyPreference,
/// };
/// use stygian_charon::acquisition::AcquisitionModeHint;
/// use stygian_charon::types::TargetClass;
///
/// let pb = Playbook {
///     id: "tier1-static".to_string(),
///     target_class: TargetClass::ContentSite,
///     description: "Static content sites".to_string(),
///     acquisition: AcquisitionDefaults::default_for(TargetClass::ContentSite),
///     proxy_preference: ProxyPreference::default(),
///     pacing: PacingProfile::default(),
///     escalation: EscalationStrategy::Capped { ceiling: AcquisitionModeHint::Resilient },
/// };
/// assert!(pb.validate().is_ok());
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Playbook {
    /// Stable identifier (`"tier1-static"`, `"tier1-js"`,
    /// `"tier2-hostile"`, etc.). Required, non-empty, unique within a
    /// resolver bundle.
    pub id: String,
    /// Target class this playbook belongs to.
    pub target_class: TargetClass,
    /// Human-readable description for operator logs.
    #[serde(default)]
    pub description: String,
    /// Acquisition-mode defaults.
    pub acquisition: AcquisitionDefaults,
    /// Proxy preference.
    #[serde(default)]
    pub proxy_preference: ProxyPreference,
    /// Pacing profile.
    #[serde(default)]
    pub pacing: PacingProfile,
    /// Escalation strategy.
    #[serde(default)]
    pub escalation: EscalationStrategy,
}

impl Playbook {
    /// Validate the playbook's internal consistency. Reports the
    /// first failing field with a structured error that includes
    /// both the field path and the bad value.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError`] on the first inconsistency. The
    /// error embeds the field path and the bad value so operators
    /// can locate the offending TOML line without re-running the
    /// loader.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::playbooks::{
    ///     AcquisitionDefaults, EscalationStrategy, PacingProfile, Playbook, ProxyPreference,
    /// };
    /// use stygian_charon::acquisition::AcquisitionModeHint;
    /// use stygian_charon::types::{ExecutionMode, SessionMode, TargetClass, TelemetryLevel};
    ///
    /// let bad = Playbook {
    ///     id: String::new(),
    ///     target_class: TargetClass::ContentSite,
    ///     description: String::new(),
    ///     acquisition: AcquisitionDefaults {
    ///         mode: AcquisitionModeHint::Fast,
    ///         execution_mode: ExecutionMode::Http,
    ///         session_mode: SessionMode::Stateless,
    ///         telemetry_level: TelemetryLevel::Basic,
    ///         sticky_session_ttl_secs: None,
    ///         enable_warmup: false,
    ///         retry_budget: 0,
    ///         backoff_base_ms: 250,
    ///     },
    ///     proxy_preference: ProxyPreference::default(),
    ///     pacing: PacingProfile::default(),
    ///     escalation: EscalationStrategy::Capped { ceiling: AcquisitionModeHint::Fast },
    /// };
    /// let err = bad.validate().expect_err("id is empty");
    /// assert!(err.to_string().contains("id"));
    /// ```
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.id.trim().is_empty() {
            return Err(ValidationError::invalid_field(
                self.id.clone(),
                "id",
                self.id.clone(),
                "playbook id must be a non-empty string",
            ));
        }
        validate_acquisition(self)?;
        validate_proxy_preference(self)?;
        validate_pacing(self)?;
        validate_escalation(self)?;
        Ok(())
    }

    /// Convenience helper that converts the resolved playbook into a
    /// [`crate::acquisition::RuntimePolicyHints`] block ready to feed
    /// into [`crate::acquisition::map_policy_hints`]. The mapping is
    /// pure and deterministic.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::playbooks::{Playbook, AcquisitionDefaults};
    /// use stygian_charon::types::TargetClass;
    ///
    /// let pb = Playbook {
    ///     id: "tier1-static".to_string(),
    ///     target_class: TargetClass::ContentSite,
    ///     description: String::new(),
    ///     acquisition: AcquisitionDefaults::default_for(TargetClass::ContentSite),
    ///     proxy_preference: Default::default(),
    ///     pacing: Default::default(),
    ///     escalation: Default::default(),
    /// };
    /// let _hints = pb.to_runtime_policy_hints();
    /// ```
    #[must_use]
    pub const fn to_runtime_policy_hints(&self) -> crate::acquisition::RuntimePolicyHints {
        crate::acquisition::RuntimePolicyHints {
            execution_mode: Some(self.acquisition.execution_mode),
            session_mode: Some(self.acquisition.session_mode),
            telemetry_level: Some(self.acquisition.telemetry_level),
            risk_score: None,
            max_retries: Some(self.acquisition.retry_budget),
            backoff_base_ms: Some(self.acquisition.backoff_base_ms),
            enable_warmup: Some(self.acquisition.enable_warmup),
        }
    }
}

fn validate_acquisition(pb: &Playbook) -> Result<(), ValidationError> {
    let acq = &pb.acquisition;
    if acq.retry_budget == 0 {
        return Err(ValidationError::invalid_field(
            pb.id.clone(),
            "acquisition.retry_budget",
            acq.retry_budget,
            "retry_budget must be > 0",
        ));
    }
    if acq.retry_budget > 32 {
        return Err(ValidationError::invalid_field(
            pb.id.clone(),
            "acquisition.retry_budget",
            acq.retry_budget,
            "retry_budget must be <= 32",
        ));
    }
    if acq.backoff_base_ms == 0 {
        return Err(ValidationError::invalid_field(
            pb.id.clone(),
            "acquisition.backoff_base_ms",
            acq.backoff_base_ms,
            "backoff_base_ms must be > 0",
        ));
    }
    if acq.backoff_base_ms > 60_000 {
        return Err(ValidationError::invalid_field(
            pb.id.clone(),
            "acquisition.backoff_base_ms",
            acq.backoff_base_ms,
            "backoff_base_ms must be <= 60_000",
        ));
    }
    if let Some(ttl) = acq.sticky_session_ttl_secs
        && ttl == 0
    {
        return Err(ValidationError::invalid_field(
            pb.id.clone(),
            "acquisition.sticky_session_ttl_secs",
            ttl,
            "sticky_session_ttl_secs must be > 0 when set",
        ));
    }
    Ok(())
}

fn validate_proxy_preference(pb: &Playbook) -> Result<(), ValidationError> {
    let proxy = &pb.proxy_preference;
    let proto = proxy.preferred_protocol.as_str();
    if !SUPPORTED_PROXY_PROTOCOLS.contains(&proto) {
        return Err(ValidationError::invalid_field(
            pb.id.clone(),
            "proxy_preference.preferred_protocol",
            proto,
            format!("preferred_protocol must be one of {SUPPORTED_PROXY_PROTOCOLS:?}"),
        ));
    }
    if let Some(max_latency) = proxy.max_latency_ms
        && max_latency == 0
    {
        return Err(ValidationError::invalid_field(
            pb.id.clone(),
            "proxy_preference.max_latency_ms",
            max_latency,
            "max_latency_ms must be > 0 when set",
        ));
    }
    Ok(())
}

fn validate_pacing(pb: &Playbook) -> Result<(), ValidationError> {
    let pacing = &pb.pacing;
    if pacing.rate_limit_rps <= 0.0 {
        return Err(ValidationError::invalid_field(
            pb.id.clone(),
            "pacing.rate_limit_rps",
            pacing.rate_limit_rps,
            "rate_limit_rps must be > 0",
        ));
    }
    if pacing.rate_limit_rps > 1000.0 {
        return Err(ValidationError::invalid_field(
            pb.id.clone(),
            "pacing.rate_limit_rps",
            pacing.rate_limit_rps,
            "rate_limit_rps must be <= 1000",
        ));
    }
    if !(0.0..=1.0).contains(&pacing.jitter_pct) {
        return Err(ValidationError::invalid_field(
            pb.id.clone(),
            "pacing.jitter_pct",
            pacing.jitter_pct,
            "jitter_pct must be in [0.0, 1.0]",
        ));
    }
    if pacing.min_request_interval_ms == 0 {
        return Err(ValidationError::invalid_field(
            pb.id.clone(),
            "pacing.min_request_interval_ms",
            pacing.min_request_interval_ms,
            "min_request_interval_ms must be > 0",
        ));
    }
    Ok(())
}

fn validate_escalation(pb: &Playbook) -> Result<(), ValidationError> {
    match &pb.escalation {
        EscalationStrategy::Capped { .. } => Ok(()),
        EscalationStrategy::Linear { steps } => {
            if steps.is_empty() {
                return Err(ValidationError::invalid_field(
                    pb.id.clone(),
                    "escalation.steps",
                    "<empty>",
                    "linear escalation must contain at least one stage",
                ));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn ok_playbook() -> Playbook {
        Playbook {
            id: "tier1-static".to_string(),
            target_class: TargetClass::ContentSite,
            description: "static content".to_string(),
            acquisition: AcquisitionDefaults::default_for(TargetClass::ContentSite),
            proxy_preference: ProxyPreference::default_for(TargetClass::ContentSite),
            pacing: PacingProfile::default_for(TargetClass::ContentSite),
            escalation: EscalationStrategy::Capped {
                ceiling: AcquisitionModeHint::Resilient,
            },
        }
    }

    #[test]
    fn defaults_match_target_class_taxonomy() {
        let api = AcquisitionDefaults::default_for(TargetClass::Api);
        assert_eq!(api.mode, AcquisitionModeHint::Fast);

        let high = AcquisitionDefaults::default_for(TargetClass::HighSecurity);
        assert_eq!(high.mode, AcquisitionModeHint::Hostile);
        assert_eq!(high.session_mode, SessionMode::Sticky);
        assert!(high.enable_warmup);
    }

    #[test]
    fn valid_playbook_passes_validation() {
        assert!(ok_playbook().validate().is_ok());
    }

    #[test]
    fn empty_id_is_rejected_with_field_path() {
        let mut pb = ok_playbook();
        pb.id.clear();
        let err = pb.validate().expect_err("empty id");
        assert_eq!(err.field_path(), Some("id"));
        assert_eq!(err.bad_value(), Some(""));
        assert!(err.to_string().contains("id"));
    }

    #[test]
    fn zero_retry_budget_is_rejected() {
        let mut pb = ok_playbook();
        pb.acquisition.retry_budget = 0;
        let err = pb.validate().expect_err("zero retry budget");
        assert_eq!(err.field_path(), Some("acquisition.retry_budget"));
        assert!(err.bad_value().is_some());
    }

    #[test]
    fn negative_pacing_rate_is_rejected() {
        let mut pb = ok_playbook();
        pb.pacing.rate_limit_rps = -0.5;
        let err = pb.validate().expect_err("negative pacing");
        assert_eq!(err.field_path(), Some("pacing.rate_limit_rps"));
        assert_eq!(err.bad_value(), Some("-0.5"));
    }

    #[test]
    fn jitter_out_of_range_is_rejected() {
        let mut pb = ok_playbook();
        pb.pacing.jitter_pct = 1.5;
        let err = pb.validate().expect_err("jitter out of range");
        assert_eq!(err.field_path(), Some("pacing.jitter_pct"));
    }

    #[test]
    fn unknown_proxy_protocol_is_rejected() {
        let mut pb = ok_playbook();
        pb.proxy_preference.preferred_protocol = "ftp".to_string();
        let err = pb.validate().expect_err("unknown protocol");
        assert_eq!(
            err.field_path(),
            Some("proxy_preference.preferred_protocol")
        );
        assert_eq!(err.bad_value(), Some("ftp"));
    }

    #[test]
    fn empty_linear_escalation_is_rejected() {
        let mut pb = ok_playbook();
        pb.escalation = EscalationStrategy::Linear { steps: Vec::new() };
        let err = pb.validate().expect_err("empty linear");
        assert_eq!(err.field_path(), Some("escalation.steps"));
    }

    #[test]
    fn to_runtime_policy_hints_carries_acquisition_fields() {
        let pb = ok_playbook();
        let hints = pb.to_runtime_policy_hints();
        assert_eq!(hints.execution_mode, Some(pb.acquisition.execution_mode));
        assert_eq!(hints.session_mode, Some(pb.acquisition.session_mode));
        assert_eq!(hints.telemetry_level, Some(pb.acquisition.telemetry_level));
        assert_eq!(hints.max_retries, Some(pb.acquisition.retry_budget));
        assert_eq!(hints.backoff_base_ms, Some(pb.acquisition.backoff_base_ms));
        assert_eq!(hints.enable_warmup, Some(pb.acquisition.enable_warmup));
    }

    #[test]
    fn escalation_stages_dedup() {
        let dup = EscalationStrategy::Linear {
            steps: vec![
                AcquisitionModeHint::Fast,
                AcquisitionModeHint::Fast,
                AcquisitionModeHint::Resilient,
            ],
        };
        assert_eq!(
            dup.stages(),
            vec![AcquisitionModeHint::Fast, AcquisitionModeHint::Resilient]
        );
    }
}
