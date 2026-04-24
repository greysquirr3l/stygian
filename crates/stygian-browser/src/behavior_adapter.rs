//! Polymorphic adapter for structured JSON-driven browser behavior tuning.
//!
//! This module accepts multiple JSON shapes and maps them into concrete
//! `stygian-browser` runtime behavior by mutating [`crate::BrowserConfig`] and
//! returning an [`AppliedBehaviorPlan`] for runtime orchestration.
//!
//! Supported input envelopes:
//! - Direct runtime policy object (`execution_mode`, `session_mode`, ...)
//! - Full investigation bundle object with nested `policy` field
//! - Lightweight direct override object (`headless`, `stealth_level`, ...)

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    BrowserConfig,
    cdp_protection::CdpFixMode,
    config::StealthLevel,
    error::{BrowserError, Result},
};

#[cfg(feature = "stealth")]
use crate::webrtc::WebRtcPolicy;

/// Source JSON shape selected by the polymorphic adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    /// A direct runtime policy object was provided.
    RuntimePolicy,
    /// A full investigation bundle object with nested `policy` was provided.
    InvestigationBundle,
    /// A direct override object was provided.
    DirectOverrides,
}

/// Browser execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionMode {
    /// Lightweight HTTP-oriented mode.
    Http,
    /// Full browser automation mode.
    Browser,
}

/// Session stickiness mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionMode {
    /// Stateless sessions.
    Stateless,
    /// Sticky sessions.
    Sticky,
}

/// Telemetry intensity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TelemetryLevel {
    /// Minimal telemetry.
    Basic,
    /// Balanced telemetry.
    Standard,
    /// Deep telemetry.
    Deep,
}

/// Interaction intensity recommendation for runtime page humanization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BehaviorInteractionLevel {
    /// No interaction simulation.
    None,
    /// Light interaction simulation.
    Low,
    /// Moderate interaction simulation.
    Medium,
    /// High interaction simulation.
    High,
}

impl TelemetryLevel {
    const fn to_interaction_level(self) -> BehaviorInteractionLevel {
        match self {
            Self::Basic => BehaviorInteractionLevel::Low,
            Self::Standard => BehaviorInteractionLevel::Medium,
            Self::Deep => BehaviorInteractionLevel::High,
        }
    }
}

/// Structured behavior plan produced after JSON adaptation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppliedBehaviorPlan {
    /// Which adapter shape was selected.
    pub adapter_kind: AdapterKind,
    /// Effective execution mode.
    pub execution_mode: ExecutionMode,
    /// Effective session mode.
    pub session_mode: SessionMode,
    /// Effective interaction recommendation.
    pub interaction_level: BehaviorInteractionLevel,
    /// Effective request pacing budget (requests/second).
    pub rate_limit_rps: f64,
    /// Retry budget for request orchestration.
    pub max_retries: u32,
    /// Base backoff delay in milliseconds.
    pub backoff_base_ms: u64,
    /// Whether warmup routines should run before primary navigation.
    pub enable_warmup: bool,
    /// Sticky session TTL recommendation in seconds.
    pub sticky_session_ttl_secs: Option<u64>,
    /// Policy risk score in `[0.0, 1.0]`.
    pub risk_score: f64,
    /// Required feature labels inferred from policy.
    pub required_stygian_features: Vec<String>,
    /// Config hint passthrough map.
    pub config_hints: BTreeMap<String, String>,
}

/// Trait for behavior adapters that can mutate a browser config.
pub trait BrowserBehaviorAdapter {
    /// Apply behavior to `config` and return the derived runtime plan.
    fn apply(&self, config: &mut BrowserConfig) -> AppliedBehaviorPlan;
}

/// Runtime-policy input compatible with stygian-charon output.
#[derive(Debug, Clone, Deserialize)]
struct RuntimePolicyInput {
    execution_mode: ExecutionMode,
    session_mode: SessionMode,
    telemetry_level: TelemetryLevel,
    rate_limit_rps: f64,
    max_retries: u32,
    backoff_base_ms: u64,
    enable_warmup: bool,
    enforce_webrtc_proxy_only: bool,
    sticky_session_ttl_secs: Option<u64>,
    required_stygian_features: Vec<String>,
    config_hints: BTreeMap<String, String>,
    risk_score: f64,
}

/// Investigation-bundle input with nested runtime policy.
#[derive(Debug, Clone, Deserialize)]
struct InvestigationBundleInput {
    policy: RuntimePolicyInput,
}

/// Direct JSON overrides for behavior tuning.
#[derive(Debug, Clone, Default, Deserialize)]
struct DirectOverridesInput {
    execution_mode: Option<ExecutionMode>,
    session_mode: Option<SessionMode>,
    telemetry_level: Option<TelemetryLevel>,
    interaction_level: Option<BehaviorInteractionLevel>,
    stealth_level: Option<StealthLevel>,
    headless: Option<bool>,
    rate_limit_rps: Option<f64>,
    max_retries: Option<u32>,
    backoff_base_ms: Option<u64>,
    enable_warmup: Option<bool>,
    enforce_webrtc_proxy_only: Option<bool>,
    sticky_session_ttl_secs: Option<u64>,
    required_stygian_features: Option<Vec<String>>,
    config_hints: Option<BTreeMap<String, String>>,
    risk_score: Option<f64>,
}

/// Polymorphic adapter selected from structured JSON input.
pub struct PolymorphicBehaviorAdapter {
    kind: AdapterKind,
    inner: Box<dyn BrowserBehaviorAdapter + Send + Sync>,
}

impl PolymorphicBehaviorAdapter {
    /// Build an adapter from a JSON string.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::ConfigError`] when JSON parsing fails or the
    /// structure is invalid.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::behavior_adapter::PolymorphicBehaviorAdapter;
    /// use stygian_browser::BrowserConfig;
    ///
    /// let json = r#"{"execution_mode":"Browser","session_mode":"Sticky","telemetry_level":"Deep","rate_limit_rps":0.8,"max_retries":4,"backoff_base_ms":1200,"enable_warmup":true,"enforce_webrtc_proxy_only":true,"sticky_session_ttl_secs":1800,"required_stygian_features":["browser"],"config_hints":{},"risk_score":0.9}"#;
    /// let adapter = PolymorphicBehaviorAdapter::from_json_str(json).expect("valid adapter");
    /// let mut cfg = BrowserConfig::default();
    /// let plan = adapter.apply(&mut cfg);
    /// assert!(plan.enable_warmup);
    /// ```
    pub fn from_json_str(json: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(json)
            .map_err(|e| BrowserError::ConfigError(format!("Invalid behavior JSON: {e}")))?;
        Self::from_json_value(value)
    }

    /// Build an adapter from a JSON value.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::ConfigError`] when the value does not match any
    /// supported input envelope.
    pub fn from_json_value(value: Value) -> Result<Self> {
        let obj = value.as_object().ok_or_else(|| {
            BrowserError::ConfigError("Behavior input must be a JSON object".to_string())
        })?;

        if obj.contains_key("policy") {
            let parsed: InvestigationBundleInput = serde_json::from_value(value).map_err(|e| {
                BrowserError::ConfigError(format!(
                    "Invalid investigation bundle behavior input: {e}"
                ))
            })?;
            return Ok(Self {
                kind: AdapterKind::InvestigationBundle,
                inner: Box::new(RuntimePolicyAdapter {
                    kind: AdapterKind::InvestigationBundle,
                    policy: parsed.policy,
                }),
            });
        }

        if obj.contains_key("execution_mode")
            && obj.contains_key("session_mode")
            && obj.contains_key("telemetry_level")
        {
            let parsed: RuntimePolicyInput = serde_json::from_value(value).map_err(|e| {
                BrowserError::ConfigError(format!("Invalid runtime policy behavior input: {e}"))
            })?;
            return Ok(Self {
                kind: AdapterKind::RuntimePolicy,
                inner: Box::new(RuntimePolicyAdapter {
                    kind: AdapterKind::RuntimePolicy,
                    policy: parsed,
                }),
            });
        }

        let parsed: DirectOverridesInput = serde_json::from_value(value).map_err(|e| {
            BrowserError::ConfigError(format!("Invalid direct override behavior input: {e}"))
        })?;

        Ok(Self {
            kind: AdapterKind::DirectOverrides,
            inner: Box::new(DirectOverridesAdapter { overrides: parsed }),
        })
    }

    /// Return the selected adapter kind.
    pub const fn kind(&self) -> AdapterKind {
        self.kind
    }

    /// Apply behavior mutations to `config` and return the resulting plan.
    pub fn apply(&self, config: &mut BrowserConfig) -> AppliedBehaviorPlan {
        self.inner.apply(config)
    }
}

struct RuntimePolicyAdapter {
    kind: AdapterKind,
    policy: RuntimePolicyInput,
}

impl BrowserBehaviorAdapter for RuntimePolicyAdapter {
    fn apply(&self, config: &mut BrowserConfig) -> AppliedBehaviorPlan {
        let interaction = self.policy.telemetry_level.to_interaction_level();
        let stealth_level = stealth_level_for_policy(&self.policy);

        config.stealth_level = stealth_level;
        config.cdp_fix_mode = if matches!(self.policy.execution_mode, ExecutionMode::Browser) {
            CdpFixMode::AddBinding
        } else {
            CdpFixMode::None
        };

        #[cfg(feature = "stealth")]
        {
            if self.policy.enforce_webrtc_proxy_only {
                config.webrtc.policy = WebRtcPolicy::DisableNonProxied;
            }
        }

        apply_config_hints(config, &self.policy.config_hints);

        AppliedBehaviorPlan {
            adapter_kind: self.kind,
            execution_mode: self.policy.execution_mode,
            session_mode: self.policy.session_mode,
            interaction_level: interaction,
            rate_limit_rps: self.policy.rate_limit_rps,
            max_retries: self.policy.max_retries,
            backoff_base_ms: self.policy.backoff_base_ms,
            enable_warmup: self.policy.enable_warmup,
            sticky_session_ttl_secs: self.policy.sticky_session_ttl_secs,
            risk_score: clamp_unit(self.policy.risk_score),
            required_stygian_features: self.policy.required_stygian_features.clone(),
            config_hints: self.policy.config_hints.clone(),
        }
    }
}

struct DirectOverridesAdapter {
    overrides: DirectOverridesInput,
}

impl BrowserBehaviorAdapter for DirectOverridesAdapter {
    fn apply(&self, config: &mut BrowserConfig) -> AppliedBehaviorPlan {
        if let Some(headless) = self.overrides.headless {
            config.headless = headless;
        }
        if let Some(stealth) = self.overrides.stealth_level {
            config.stealth_level = stealth;
        }

        #[cfg(feature = "stealth")]
        {
            if self.overrides.enforce_webrtc_proxy_only == Some(true) {
                config.webrtc.policy = WebRtcPolicy::DisableNonProxied;
            }
        }

        let hints = self.overrides.config_hints.clone().unwrap_or_default();
        apply_config_hints(config, &hints);

        let execution_mode = self
            .overrides
            .execution_mode
            .unwrap_or(ExecutionMode::Browser);
        let session_mode = self
            .overrides
            .session_mode
            .unwrap_or(SessionMode::Stateless);
        let telemetry = self
            .overrides
            .telemetry_level
            .unwrap_or(TelemetryLevel::Standard);
        let interaction = self
            .overrides
            .interaction_level
            .unwrap_or_else(|| telemetry.to_interaction_level());

        AppliedBehaviorPlan {
            adapter_kind: AdapterKind::DirectOverrides,
            execution_mode,
            session_mode,
            interaction_level: interaction,
            rate_limit_rps: self.overrides.rate_limit_rps.unwrap_or(1.0),
            max_retries: self.overrides.max_retries.unwrap_or(2),
            backoff_base_ms: self.overrides.backoff_base_ms.unwrap_or(500),
            enable_warmup: self.overrides.enable_warmup.unwrap_or(false),
            sticky_session_ttl_secs: self.overrides.sticky_session_ttl_secs,
            risk_score: clamp_unit(self.overrides.risk_score.unwrap_or(0.5)),
            required_stygian_features: self
                .overrides
                .required_stygian_features
                .clone()
                .unwrap_or_default(),
            config_hints: hints,
        }
    }
}

fn stealth_level_for_policy(policy: &RuntimePolicyInput) -> StealthLevel {
    if matches!(policy.execution_mode, ExecutionMode::Http) {
        return StealthLevel::None;
    }

    if policy
        .required_stygian_features
        .iter()
        .any(|f| f.contains("stealth") || f.contains("browser"))
        || policy.risk_score >= 0.65
    {
        StealthLevel::Advanced
    } else if policy.risk_score >= 0.25 {
        StealthLevel::Basic
    } else {
        StealthLevel::None
    }
}

fn apply_config_hints(config: &mut BrowserConfig, hints: &BTreeMap<String, String>) {
    if let Some(proxy) = hints.get("proxy_url").or_else(|| hints.get("proxy")) {
        config.proxy = Some(proxy.clone());
    }

    if let Some(headless_raw) = hints.get("headless")
        && let Ok(headless) = headless_raw.parse::<bool>()
    {
        config.headless = headless;
    }

    if let (Some(width_raw), Some(height_raw)) =
        (hints.get("viewport_width"), hints.get("viewport_height"))
        && let (Ok(width), Ok(height)) = (width_raw.parse::<u32>(), height_raw.parse::<u32>())
    {
        config.window_size = Some((width, height));
    }

    if let Some(mode_raw) = hints.get("cdp_fix_mode") {
        config.cdp_fix_mode = parse_cdp_fix_mode(mode_raw);
    }

    if let Some(user_agent) = hints.get("user_agent") {
        let arg = format!("--user-agent={user_agent}");
        if !config.args.iter().any(|existing| existing == &arg) {
            config.args.push(arg);
        }
    }
}

fn parse_cdp_fix_mode(raw: &str) -> CdpFixMode {
    match raw.to_ascii_lowercase().as_str() {
        "none" | "0" => CdpFixMode::None,
        "isolatedworld" | "isolated_world" | "isolated" => CdpFixMode::IsolatedWorld,
        "enabledisable" | "enable_disable" => CdpFixMode::EnableDisable,
        _ => CdpFixMode::AddBinding,
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
    use serde_json::json;

    #[test]
    fn selects_runtime_policy_shape() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let value = json!({
            "execution_mode": "Browser",
            "session_mode": "Sticky",
            "telemetry_level": "Deep",
            "rate_limit_rps": 0.5,
            "max_retries": 3,
            "backoff_base_ms": 1200,
            "enable_warmup": true,
            "enforce_webrtc_proxy_only": true,
            "sticky_session_ttl_secs": 1800,
            "required_stygian_features": ["browser", "stealth"],
            "config_hints": {"proxy_url": "http://127.0.0.1:8080"},
            "risk_score": 0.9
        });

        let adapter = PolymorphicBehaviorAdapter::from_json_value(value)
            .map_err(|e| format!("adapter parse failed: {e}"))?;
        assert_eq!(adapter.kind(), AdapterKind::RuntimePolicy);

        let mut cfg = BrowserConfig::default();
        let plan = adapter.apply(&mut cfg);
        assert_eq!(plan.interaction_level, BehaviorInteractionLevel::High);
        assert_eq!(cfg.proxy.as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(cfg.stealth_level, StealthLevel::Advanced);
        Ok(())
    }

    #[test]
    fn selects_investigation_bundle_shape() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let value = json!({
            "report": {},
            "requirements": {},
            "policy": {
                "execution_mode": "Browser",
                "session_mode": "Stateless",
                "telemetry_level": "Standard",
                "rate_limit_rps": 1.2,
                "max_retries": 2,
                "backoff_base_ms": 400,
                "enable_warmup": false,
                "enforce_webrtc_proxy_only": false,
                "sticky_session_ttl_secs": null,
                "required_stygian_features": [],
                "config_hints": {},
                "risk_score": 0.2
            }
        });

        let adapter = PolymorphicBehaviorAdapter::from_json_value(value)
            .map_err(|e| format!("adapter parse failed: {e}"))?;
        assert_eq!(adapter.kind(), AdapterKind::InvestigationBundle);

        let mut cfg = BrowserConfig::default();
        let plan = adapter.apply(&mut cfg);
        assert_eq!(plan.execution_mode, ExecutionMode::Browser);
        assert_eq!(plan.session_mode, SessionMode::Stateless);
        Ok(())
    }

    #[test]
    fn direct_overrides_apply_to_config() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let value = json!({
            "headless": false,
            "stealth_level": "basic",
            "interaction_level": "medium",
            "config_hints": {
                "viewport_width": "1366",
                "viewport_height": "768",
                "user_agent": "Mozilla/5.0 test"
            }
        });

        let adapter = PolymorphicBehaviorAdapter::from_json_value(value)
            .map_err(|e| format!("adapter parse failed: {e}"))?;
        assert_eq!(adapter.kind(), AdapterKind::DirectOverrides);

        let mut cfg = BrowserConfig::default();
        let plan = adapter.apply(&mut cfg);

        assert!(!cfg.headless);
        assert_eq!(cfg.stealth_level, StealthLevel::Basic);
        assert_eq!(plan.interaction_level, BehaviorInteractionLevel::Medium);
        assert_eq!(cfg.window_size, Some((1366, 768)));
        assert!(
            cfg.args
                .iter()
                .any(|arg| arg.contains("--user-agent=Mozilla/5.0 test"))
        );
        Ok(())
    }

    #[test]
    fn invalid_non_object_input_is_rejected() {
        let err = PolymorphicBehaviorAdapter::from_json_value(json!("not-object"))
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(err.contains("must be a JSON object"));
    }
}
