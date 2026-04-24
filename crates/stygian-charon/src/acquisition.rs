//! Pure mapping from Charon policy outputs to acquisition-runner input hints.

use serde::{Deserialize, Serialize};

use crate::types::{AdapterStrategy, ExecutionMode, RuntimePolicy, SessionMode, TelemetryLevel};

/// Strategy mode hint for downstream acquisition runners.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcquisitionModeHint {
    /// Lowest-latency path, minimal escalation.
    Fast,
    /// Balanced reliability path.
    Resilient,
    /// Strong anti-bot posture first.
    Hostile,
    /// Investigation-first entry.
    Investigate,
}

/// Optional starting stage hint for investigation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcquisitionStartHint {
    /// Start from direct HTTP.
    DirectHttp,
    /// Start from TLS-profiled HTTP.
    TlsProfiledHttp,
    /// Start from browser-backed stage.
    BrowserLightStealth,
    /// Start from sticky-session browser stage.
    StickyProxyBrowser,
}

/// Deterministic runner input derived from runtime policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcquisitionPolicy {
    /// Recommended acquisition mode.
    pub mode: AcquisitionModeHint,
    /// Optional investigation entry point.
    pub investigate_start: Option<AcquisitionStartHint>,
    /// Retry budget hint for transient failures.
    pub retry_budget: u32,
    /// Base retry backoff in milliseconds.
    pub backoff_base_ms: u64,
    /// Warmup recommendation.
    pub enable_warmup: bool,
    /// Sticky-session recommendation.
    pub sticky_session: bool,
    /// Telemetry intensity carried through for logging/diagnostics.
    pub telemetry_level: TelemetryLevel,
    /// Risk score clamped to [0.0, 1.0].
    pub risk_score: f64,
}

/// Optional runtime-policy hints for partial policy inputs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RuntimePolicyHints {
    /// Optional execution mode.
    pub execution_mode: Option<ExecutionMode>,
    /// Optional session mode.
    pub session_mode: Option<SessionMode>,
    /// Optional telemetry level.
    pub telemetry_level: Option<TelemetryLevel>,
    /// Optional risk score.
    pub risk_score: Option<f64>,
    /// Optional retry count.
    pub max_retries: Option<u32>,
    /// Optional backoff base.
    pub backoff_base_ms: Option<u64>,
    /// Optional warmup flag.
    pub enable_warmup: Option<bool>,
}

/// Map a Charon strategy recommendation into an acquisition mode.
#[must_use]
pub const fn map_adapter_strategy(strategy: AdapterStrategy) -> AcquisitionModeHint {
    match strategy {
        AdapterStrategy::DirectHttp => AcquisitionModeHint::Fast,
        AdapterStrategy::BrowserStealth | AdapterStrategy::SessionWarmup => {
            AcquisitionModeHint::Resilient
        }
        AdapterStrategy::StickyProxy => AcquisitionModeHint::Hostile,
        AdapterStrategy::InvestigateOnly => AcquisitionModeHint::Investigate,
    }
}

/// Map a full runtime policy into deterministic acquisition hints.
#[must_use]
pub fn map_runtime_policy(policy: &RuntimePolicy) -> AcquisitionPolicy {
    map_policy_hints(&RuntimePolicyHints {
        execution_mode: Some(policy.execution_mode),
        session_mode: Some(policy.session_mode),
        telemetry_level: Some(policy.telemetry_level),
        risk_score: Some(policy.risk_score),
        max_retries: Some(policy.max_retries),
        backoff_base_ms: Some(policy.backoff_base_ms),
        enable_warmup: Some(policy.enable_warmup),
    })
}

/// Map partial runtime-policy hints with documented defaults.
#[must_use]
pub fn map_policy_hints(hints: &RuntimePolicyHints) -> AcquisitionPolicy {
    let execution_mode = hints.execution_mode.unwrap_or(ExecutionMode::Http);
    let session_mode = hints.session_mode.unwrap_or(SessionMode::Stateless);
    let telemetry_level = hints.telemetry_level.unwrap_or(TelemetryLevel::Standard);
    let risk_score = clamp_unit(hints.risk_score.unwrap_or(0.5));
    let retry_budget = hints.max_retries.unwrap_or(2);
    let backoff_base_ms = hints.backoff_base_ms.unwrap_or(250);
    let enable_warmup = hints.enable_warmup.unwrap_or(false);

    let mode = if telemetry_level == TelemetryLevel::Deep && execution_mode == ExecutionMode::Http {
        AcquisitionModeHint::Investigate
    } else if session_mode == SessionMode::Sticky || risk_score >= 0.8 {
        AcquisitionModeHint::Hostile
    } else if execution_mode == ExecutionMode::Http && risk_score <= 0.35 && retry_budget <= 2 {
        AcquisitionModeHint::Fast
    } else {
        AcquisitionModeHint::Resilient
    };

    let investigate_start = if mode == AcquisitionModeHint::Investigate {
        Some(match (execution_mode, session_mode, risk_score) {
            (_, SessionMode::Sticky, _) => AcquisitionStartHint::StickyProxyBrowser,
            (ExecutionMode::Browser, _, _) => AcquisitionStartHint::BrowserLightStealth,
            (_, _, r) if r >= 0.7 => AcquisitionStartHint::TlsProfiledHttp,
            _ => AcquisitionStartHint::DirectHttp,
        })
    } else {
        None
    };

    AcquisitionPolicy {
        mode,
        investigate_start,
        retry_budget,
        backoff_base_ms,
        enable_warmup,
        sticky_session: session_mode == SessionMode::Sticky,
        telemetry_level,
        risk_score,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RuntimePolicy;

    #[test]
    fn adapter_strategy_maps_to_expected_mode() {
        assert_eq!(
            map_adapter_strategy(AdapterStrategy::DirectHttp),
            AcquisitionModeHint::Fast
        );
        assert_eq!(
            map_adapter_strategy(AdapterStrategy::BrowserStealth),
            AcquisitionModeHint::Resilient
        );
        assert_eq!(
            map_adapter_strategy(AdapterStrategy::StickyProxy),
            AcquisitionModeHint::Hostile
        );
        assert_eq!(
            map_adapter_strategy(AdapterStrategy::InvestigateOnly),
            AcquisitionModeHint::Investigate
        );
    }

    #[test]
    fn high_risk_biases_to_stronger_mode() {
        let mapped = map_policy_hints(&RuntimePolicyHints {
            execution_mode: Some(ExecutionMode::Http),
            session_mode: Some(SessionMode::Stateless),
            telemetry_level: Some(TelemetryLevel::Standard),
            risk_score: Some(0.92),
            max_retries: Some(2),
            backoff_base_ms: Some(250),
            enable_warmup: Some(false),
        });

        assert_eq!(mapped.mode, AcquisitionModeHint::Hostile);
        assert!(mapped.risk_score >= 0.9);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let mapped = map_policy_hints(&RuntimePolicyHints::default());

        assert_eq!(mapped.mode, AcquisitionModeHint::Resilient);
        assert_eq!(mapped.retry_budget, 2);
        assert_eq!(mapped.backoff_base_ms, 250);
        assert!(!mapped.enable_warmup);
        assert!(!mapped.sticky_session);
        assert_eq!(mapped.telemetry_level, TelemetryLevel::Standard);
        assert!((mapped.risk_score - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn runtime_policy_mapping_is_stable() {
        let policy = RuntimePolicy {
            execution_mode: ExecutionMode::Browser,
            session_mode: SessionMode::Sticky,
            telemetry_level: TelemetryLevel::Deep,
            rate_limit_rps: 1.0,
            max_retries: 5,
            backoff_base_ms: 700,
            enable_warmup: true,
            enforce_webrtc_proxy_only: true,
            sticky_session_ttl_secs: Some(300),
            required_stygian_features: vec![],
            config_hints: std::collections::BTreeMap::default(),
            risk_score: 0.81,
        };

        let mapped = map_runtime_policy(&policy);
        assert_eq!(mapped.mode, AcquisitionModeHint::Hostile);
        assert!(mapped.sticky_session);
        assert_eq!(mapped.retry_budget, 5);
        assert_eq!(mapped.backoff_base_ms, 700);
        assert!(mapped.enable_warmup);
    }
}
