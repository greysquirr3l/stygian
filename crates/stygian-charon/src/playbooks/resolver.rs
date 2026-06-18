//! Playbook resolver with deterministic precedence (T85).
//!
//! The resolver merges three layers into a single
//! [`ResolvedPlaybook`]:
//!
//! 1. **Request override** (top priority) — per-call fields supplied
//!    by the operator when the runner is invoked.
//! 2. **Playbook default** — the codified defaults from the matched
//!    TOML playbook.
//! 3. **Global default** (bottom priority) — the resolver's own
//!    fallback when no playbook is registered for a target class.
//!
//! The precedence is **deterministic and exhaustively tested**. Each
//! field in [`ResolvedPlaybook`] carries a [`ResolutionSource`] tag so
//! observers can verify which layer contributed the value.
//!
//! # Example
//!
//! ```
//! use stygian_charon::playbooks::{
//!     AcquisitionOverrides, PlaybookOverrides, PlaybookResolver,
//! };
//! use stygian_charon::acquisition::AcquisitionModeHint;
//! use stygian_charon::types::{ExecutionMode, SessionMode, TargetClass};
//!
//! let resolver = PlaybookResolver::with_builtin_defaults();
//! let overrides = PlaybookOverrides {
//!     acquisition: AcquisitionOverrides {
//!         mode: Some(AcquisitionModeHint::Hostile),
//!         execution_mode: Some(ExecutionMode::Browser),
//!         ..AcquisitionOverrides::default()
//!     },
//!     ..PlaybookOverrides::default()
//! };
//! let resolved = resolver
//!     .resolve(TargetClass::ContentSite, "tier1-js", &overrides)
//!     .expect("resolve");
//! // Request override wins for the fields it sets.
//! assert_eq!(resolved.acquisition.mode, AcquisitionModeHint::Hostile);
//! assert_eq!(resolved.acquisition.execution_mode, ExecutionMode::Browser);
//! // Playbook default fills the rest.
//! assert_eq!(resolved.acquisition.session_mode, SessionMode::Sticky);
//! ```

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::acquisition::AcquisitionModeHint;
use crate::playbooks::error::ValidationError;
use crate::playbooks::schema::{
    AcquisitionDefaults, AcquisitionOverrides, EscalationStrategy, PacingProfile, Playbook,
    ProxyPreference, ResolutionSource,
};
use crate::types::{ExecutionMode, SessionMode, TargetClass, TelemetryLevel};

/// Per-request override bundle used by [`PlaybookResolver::resolve`].
///
/// Each field is independent: setting `acquisition.mode` does not
/// override `acquisition.session_mode`. The empty override
/// (`PlaybookOverrides::default()`) means "use the playbook default
/// for every field".
///
/// # Example
///
/// ```
/// use stygian_charon::playbooks::{AcquisitionOverrides, PlaybookOverrides};
/// use stygian_charon::acquisition::AcquisitionModeHint;
///
/// let overrides = PlaybookOverrides {
///     acquisition: AcquisitionOverrides {
///         mode: Some(AcquisitionModeHint::Hostile),
///         ..AcquisitionOverrides::default()
///     },
///     ..PlaybookOverrides::default()
/// };
/// assert_eq!(overrides.acquisition.mode, Some(AcquisitionModeHint::Hostile));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlaybookOverrides {
    /// Optional acquisition overrides.
    #[serde(default)]
    pub acquisition: AcquisitionOverrides,
    /// Optional proxy-preference override (full replacement).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_preference: Option<ProxyPreference>,
    /// Optional pacing-profile override (full replacement).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pacing: Option<PacingProfile>,
    /// Optional escalation-strategy override (full replacement).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalation: Option<EscalationStrategy>,
}

/// Resolved acquisition block: the merge of overrides, playbook, and
/// global defaults. Each field carries a [`ResolutionSource`] tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedAcquisition {
    /// Resolved acquisition mode.
    pub mode: AcquisitionModeHint,
    /// Which tier contributed the mode value.
    pub mode_source: ResolutionSource,
    /// Resolved execution mode.
    pub execution_mode: ExecutionMode,
    /// Which tier contributed the execution-mode value.
    pub execution_mode_source: ResolutionSource,
    /// Resolved session mode.
    pub session_mode: SessionMode,
    /// Which tier contributed the session-mode value.
    pub session_mode_source: ResolutionSource,
    /// Resolved telemetry level.
    pub telemetry_level: TelemetryLevel,
    /// Which tier contributed the telemetry value.
    pub telemetry_level_source: ResolutionSource,
    /// Resolved sticky-session TTL (seconds).
    pub sticky_session_ttl_secs: Option<u64>,
    /// Which tier contributed the sticky-session TTL.
    pub sticky_session_ttl_source: ResolutionSource,
    /// Resolved warmup flag.
    pub enable_warmup: bool,
    /// Which tier contributed the warmup flag.
    pub enable_warmup_source: ResolutionSource,
    /// Resolved retry budget.
    pub retry_budget: u32,
    /// Which tier contributed the retry budget.
    pub retry_budget_source: ResolutionSource,
    /// Resolved backoff base (ms).
    pub backoff_base_ms: u64,
    /// Which tier contributed the backoff base.
    pub backoff_base_source: ResolutionSource,
}

/// Full resolution result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedPlaybook {
    /// Id of the matched playbook (or `"unknown"` when the resolver
    /// fell through to the global default).
    pub playbook_id: String,
    /// Target class supplied to the resolver.
    pub target_class: TargetClass,
    /// Resolved acquisition block.
    pub acquisition: ResolvedAcquisition,
    /// Resolved proxy preference.
    pub proxy_preference: ProxyPreference,
    /// Which tier contributed the proxy preference.
    pub proxy_preference_source: ResolutionSource,
    /// Resolved pacing profile.
    pub pacing: PacingProfile,
    /// Which tier contributed the pacing profile.
    pub pacing_source: ResolutionSource,
    /// Resolved escalation strategy.
    pub escalation: EscalationStrategy,
    /// Which tier contributed the escalation strategy.
    pub escalation_source: ResolutionSource,
}

impl ResolvedPlaybook {
    /// Map the resolved acquisition block to a
    /// [`crate::acquisition::RuntimePolicyHints`] ready for
    /// [`crate::acquisition::map_policy_hints`].
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::playbooks::PlaybookResolver;
    /// use stygian_charon::types::TargetClass;
    ///
    /// let resolver = PlaybookResolver::with_builtin_defaults();
    /// let resolved = resolver
    ///     .resolve(TargetClass::ContentSite, "tier1-js", &Default::default())
    ///     .expect("resolve");
    /// let _hints = resolved.to_runtime_policy_hints();
    /// ```
    #[must_use]
    pub fn to_runtime_policy_hints(&self) -> crate::acquisition::RuntimePolicyHints {
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

    /// Convenience: render the resolution result into a
    /// [`crate::acquisition::AcquisitionPolicy`] via the standard
    /// `map_policy_hints` path.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::playbooks::PlaybookResolver;
    /// use stygian_charon::types::TargetClass;
    ///
    /// let resolver = PlaybookResolver::with_builtin_defaults();
    /// let resolved = resolver
    ///     .resolve(TargetClass::ContentSite, "tier1-js", &Default::default())
    ///     .expect("resolve");
    /// let _policy = resolved.to_acquisition_policy();
    /// ```
    #[must_use]
    pub fn to_acquisition_policy(&self) -> crate::acquisition::AcquisitionPolicy {
        crate::acquisition::map_policy_hints(&self.to_runtime_policy_hints())
    }
}

/// Playbook registry + precedence resolver.
///
/// # Example
///
/// ```
/// use stygian_charon::playbooks::{PlaybookOverrides, PlaybookResolver};
/// use stygian_charon::types::TargetClass;
///
/// let resolver = PlaybookResolver::with_builtin_defaults();
/// let resolved = resolver
///     .resolve(
///         TargetClass::ContentSite,
///         "tier1-js",
///         &PlaybookOverrides::default(),
///     )
///     .expect("resolve");
/// assert_eq!(resolved.target_class, TargetClass::ContentSite);
/// ```
#[derive(Debug, Clone)]
pub struct PlaybookResolver {
    playbooks: HashMap<String, Playbook>,
    by_target_class: HashMap<TargetClass, String>,
    global_default: Playbook,
}

impl PlaybookResolver {
    /// Create a resolver seeded with the documented baseline
    /// playbooks (`tier1-static`, `tier1-js`, `tier2-hostile`,
    /// plus an `unknown` fallback).
    ///
    /// The resolver will fail-fast on startup if any embedded
    /// playbook is invalid; this guarantees the loader's
    /// `validate()`-first contract.
    ///
    /// # Panics
    ///
    /// Panics if any embedded baseline TOML fails to parse or
    /// validate. This is a **compile-time** failure guarded by
    /// the `compile_check_builtin_playbooks` test in
    /// [`crate::playbooks::builtin`]; the panic in production
    /// surfaces a regression in the embedded data as a hard
    /// startup error.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::playbooks::PlaybookResolver;
    ///
    /// let resolver = PlaybookResolver::with_builtin_defaults();
    /// assert!(resolver.contains("tier1-static"));
    /// assert!(resolver.contains("tier1-js"));
    /// assert!(resolver.contains("tier2-hostile"));
    /// ```
    #[must_use]
    pub fn with_builtin_defaults() -> Self {
        // The embedded playbooks are validated at compile time
        // (see `builtin::load_builtin_playbooks`). If the test
        // suite catches a regression in the embedded TOML, this
        // function will refuse to build, surfacing the failure at
        // task completion rather than runtime.
        let playbooks = crate::playbooks::builtin::builtin_playbooks();
        Self::from_playbooks(playbooks).expect("builtin playbooks are validated at compile time")
    }

    /// Build a resolver from a list of pre-validated playbooks. The
    /// first playbook encountered per target class becomes the
    /// default for that class; subsequent matches for the same class
    /// override the lookup (last-write-wins).
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError::DuplicateId`] when two playbooks
    /// share the same id, or [`ValidationError`] from each playbook's
    /// `validate()`.
    pub fn from_playbooks<I>(playbooks: I) -> Result<Self, ValidationError>
    where
        I: IntoIterator<Item = Playbook>,
    {
        let mut by_id: HashMap<String, Playbook> = HashMap::new();
        let mut by_target_class: HashMap<TargetClass, String> = HashMap::new();
        let mut global_default: Option<Playbook> = None;

        for pb in playbooks {
            pb.validate()?;
            if by_id.contains_key(&pb.id) {
                return Err(ValidationError::DuplicateId { playbook_id: pb.id });
            }
            if pb.id == "unknown" {
                global_default = Some(pb.clone());
            }
            by_target_class
                .entry(pb.target_class)
                .or_insert_with(|| pb.id.clone());
            by_id.insert(pb.id.clone(), pb);
        }

        let global_default = global_default.unwrap_or_else(|| {
            Playbook {
                id: "unknown".to_string(),
                target_class: TargetClass::Unknown,
                description: "Fallback when no playbook matches".to_string(),
                acquisition: AcquisitionDefaults::default_for(TargetClass::Unknown),
                proxy_preference: ProxyPreference::default_for(TargetClass::Unknown),
                pacing: PacingProfile::default_for(TargetClass::Unknown),
                escalation: EscalationStrategy::default(),
            }
        });

        Ok(Self {
            playbooks: by_id,
            by_target_class,
            global_default,
        })
    }

    /// `true` when the resolver has a playbook with the given id.
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.playbooks.contains_key(id)
    }

    /// Ids of all registered playbooks, in sorted order.
    #[must_use]
    pub fn playbook_ids(&self) -> Vec<String> {
        self.playbooks.keys().cloned().collect()
    }

    /// Resolve a playbook for a `(target_class, playbook_id)` pair
    /// with per-request overrides. Precedence:
    /// `request override > playbook default > global default`.
    ///
    /// When `playbook_id` is `Some` and registered, the playbook is
    /// used directly. When `playbook_id` is `None`, the resolver
    /// looks up the target-class default. When neither resolves,
    /// the resolver falls through to the global default.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError::UnknownPlaybook`] when an explicit
    /// `playbook_id` is supplied and is not registered.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::playbooks::{PlaybookOverrides, PlaybookResolver};
    /// use stygian_charon::types::TargetClass;
    ///
    /// let resolver = PlaybookResolver::with_builtin_defaults();
    /// let resolved = resolver
    ///     .resolve(
    ///         TargetClass::ContentSite,
    ///         "tier1-js",
    ///         &PlaybookOverrides::default(),
    ///     )
    ///     .expect("resolve");
    /// assert_eq!(resolved.playbook_id, "tier1-js");
    /// ```
    pub fn resolve(
        &self,
        target_class: TargetClass,
        playbook_id: &str,
        overrides: &PlaybookOverrides,
    ) -> Result<ResolvedPlaybook, ValidationError> {
        let playbook = self.lookup_playbook(target_class, playbook_id)?;
        Ok(self.merge(playbook, target_class, overrides))
    }

    /// Like [`resolve`](Self::resolve) but takes an optional
    /// `playbook_id` and falls through to the target-class default
    /// (or the global default) when `None`.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError::UnknownPlaybook`] when an explicit
    /// `playbook_id` is supplied and is not registered.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::playbooks::{PlaybookOverrides, PlaybookResolver};
    /// use stygian_charon::types::TargetClass;
    ///
    /// let resolver = PlaybookResolver::with_builtin_defaults();
    /// let resolved = resolver
    ///     .resolve_optional(
    ///         TargetClass::ContentSite,
    ///         None,
    ///         &PlaybookOverrides::default(),
    ///     )
    ///     .expect("resolve");
    /// assert!(resolver.contains(&resolved.playbook_id));
    /// ```
    pub fn resolve_optional(
        &self,
        target_class: TargetClass,
        playbook_id: Option<&str>,
        overrides: &PlaybookOverrides,
    ) -> Result<ResolvedPlaybook, ValidationError> {
        let playbook = match playbook_id {
            Some(id) => self.lookup_by_id(id)?,
            None => self.lookup_by_target_class(target_class),
        };
        Ok(self.merge(playbook, target_class, overrides))
    }

    fn lookup_playbook(
        &self,
        target_class: TargetClass,
        playbook_id: &str,
    ) -> Result<&Playbook, ValidationError> {
        if !playbook_id.is_empty() {
            return self.lookup_by_id(playbook_id);
        }
        Ok(self.lookup_by_target_class(target_class))
    }

    fn lookup_by_id(&self, id: &str) -> Result<&Playbook, ValidationError> {
        self.playbooks.get(id).ok_or_else(|| ValidationError::UnknownPlaybook {
            playbook_id: id.to_string(),
        })
    }

    fn lookup_by_target_class(&self, target_class: TargetClass) -> &Playbook {
        if let Some(id) = self.by_target_class.get(&target_class)
            && let Some(pb) = self.playbooks.get(id)
        {
            return pb;
        }
        if let Some(id) = self.by_target_class.get(&TargetClass::Unknown)
            && let Some(pb) = self.playbooks.get(id)
        {
            return pb;
        }
        &self.global_default
    }

    fn merge(
        &self,
        playbook: &Playbook,
        target_class: TargetClass,
        overrides: &PlaybookOverrides,
    ) -> ResolvedPlaybook {
        let default_acq = AcquisitionDefaults::default_for(target_class);
        let playbook_acq = &playbook.acquisition;
        let is_global_default = playbook.id == self.global_default.id;

        let pick_mode = overrides
            .acquisition
            .mode
            .unwrap_or(playbook_acq.mode);
        let pick_execution = overrides
            .acquisition
            .execution_mode
            .unwrap_or(playbook_acq.execution_mode);
        let pick_session = overrides
            .acquisition
            .session_mode
            .unwrap_or(playbook_acq.session_mode);
        let pick_telemetry = overrides
            .acquisition
            .telemetry_level
            .unwrap_or(playbook_acq.telemetry_level);
        let pick_sticky = playbook_acq.sticky_session_ttl_secs;
        let pick_warmup = overrides
            .acquisition
            .enable_warmup
            .unwrap_or(playbook_acq.enable_warmup);
        let pick_retry = overrides
            .acquisition
            .retry_budget
            .unwrap_or(playbook_acq.retry_budget);
        let pick_backoff = overrides
            .acquisition
            .backoff_base_ms
            .unwrap_or(playbook_acq.backoff_base_ms);

        let acquisition = ResolvedAcquisition {
            mode: pick_mode,
            mode_source: source_for_mode(overrides, playbook_acq, default_acq.mode, is_global_default),
            execution_mode: pick_execution,
            execution_mode_source: source_for_scalar(overrides.acquisition.execution_mode.is_some(), is_global_default),
            session_mode: pick_session,
            session_mode_source: source_for_scalar(overrides.acquisition.session_mode.is_some(), is_global_default),
            telemetry_level: pick_telemetry,
            telemetry_level_source: source_for_scalar(overrides.acquisition.telemetry_level.is_some(), is_global_default),
            sticky_session_ttl_secs: pick_sticky,
            sticky_session_ttl_source: source_for_scalar(false, is_global_default || playbook_acq.sticky_session_ttl_secs.is_none()),
            enable_warmup: pick_warmup,
            enable_warmup_source: source_for_scalar(overrides.acquisition.enable_warmup.is_some(), is_global_default),
            retry_budget: pick_retry,
            retry_budget_source: source_for_scalar(overrides.acquisition.retry_budget.is_some(), is_global_default),
            backoff_base_ms: pick_backoff,
            backoff_base_source: source_for_scalar(overrides.acquisition.backoff_base_ms.is_some(), is_global_default),
        };

        ResolvedPlaybook {
            playbook_id: playbook.id.clone(),
            target_class,
            acquisition,
            proxy_preference: overrides
                .proxy_preference
                .clone()
                .unwrap_or_else(|| playbook.proxy_preference.clone()),
            proxy_preference_source: if overrides.proxy_preference.is_some() {
                ResolutionSource::RequestOverride
            } else if is_global_default {
                ResolutionSource::GlobalDefault
            } else {
                ResolutionSource::PlaybookDefault
            },
            pacing: overrides
                .pacing
                .clone()
                .unwrap_or_else(|| playbook.pacing.clone()),
            pacing_source: tier_source(overrides.pacing.is_some(), is_global_default),
            escalation: overrides
                .escalation
                .clone()
                .unwrap_or_else(|| playbook.escalation.clone()),
            escalation_source: tier_source(overrides.escalation.is_some(), is_global_default),
        }
    }
}

fn source_for_mode(
    overrides: &PlaybookOverrides,
    _playbook: &AcquisitionDefaults,
    _default: AcquisitionModeHint,
    is_global_default: bool,
) -> ResolutionSource {
    if overrides.acquisition.mode.is_some() {
        ResolutionSource::RequestOverride
    } else if is_global_default {
        ResolutionSource::GlobalDefault
    } else {
        ResolutionSource::PlaybookDefault
    }
}

fn source_for_scalar(override_set: bool, is_global_default: bool) -> ResolutionSource {
    if override_set {
        ResolutionSource::RequestOverride
    } else if is_global_default {
        ResolutionSource::GlobalDefault
    } else {
        ResolutionSource::PlaybookDefault
    }
}

fn tier_source(override_set: bool, is_global_default: bool) -> ResolutionSource {
    source_for_scalar(override_set, is_global_default)
}

#[cfg(test)]
#[allow(clippy::similar_names)]
mod tests {
    use super::*;

    fn make_resolver() -> PlaybookResolver {
        PlaybookResolver::from_playbooks(vec![
            Playbook {
                id: "tier1-static".to_string(),
                target_class: TargetClass::ContentSite,
                description: String::new(),
                acquisition: AcquisitionDefaults {
                    mode: AcquisitionModeHint::Fast,
                    execution_mode: ExecutionMode::Http,
                    session_mode: SessionMode::Stateless,
                    telemetry_level: TelemetryLevel::Basic,
                    sticky_session_ttl_secs: None,
                    enable_warmup: false,
                    retry_budget: 3,
                    backoff_base_ms: 250,
                },
                proxy_preference: ProxyPreference::default_for(TargetClass::ContentSite),
                pacing: PacingProfile::default_for(TargetClass::ContentSite),
                escalation: EscalationStrategy::Capped {
                    ceiling: AcquisitionModeHint::Resilient,
                },
            },
            Playbook {
                id: "tier1-js".to_string(),
                target_class: TargetClass::ContentSite,
                description: String::new(),
                acquisition: AcquisitionDefaults {
                    mode: AcquisitionModeHint::Resilient,
                    execution_mode: ExecutionMode::Browser,
                    session_mode: SessionMode::Sticky,
                    telemetry_level: TelemetryLevel::Standard,
                    sticky_session_ttl_secs: Some(600),
                    enable_warmup: true,
                    retry_budget: 5,
                    backoff_base_ms: 500,
                },
                proxy_preference: ProxyPreference::default_for(TargetClass::ContentSite),
                pacing: PacingProfile::default_for(TargetClass::ContentSite),
                escalation: EscalationStrategy::Capped {
                    ceiling: AcquisitionModeHint::Hostile,
                },
            },
            Playbook {
                id: "tier2-hostile".to_string(),
                target_class: TargetClass::HighSecurity,
                description: String::new(),
                acquisition: AcquisitionDefaults::default_for(TargetClass::HighSecurity),
                proxy_preference: ProxyPreference::default_for(TargetClass::HighSecurity),
                pacing: PacingProfile::default_for(TargetClass::HighSecurity),
                escalation: EscalationStrategy::Capped {
                    ceiling: AcquisitionModeHint::Hostile,
                },
            },
            Playbook {
                id: "unknown".to_string(),
                target_class: TargetClass::Unknown,
                description: String::new(),
                acquisition: AcquisitionDefaults::default_for(TargetClass::Unknown),
                proxy_preference: ProxyPreference::default_for(TargetClass::Unknown),
                pacing: PacingProfile::default_for(TargetClass::Unknown),
                escalation: EscalationStrategy::default(),
            },
        ])
        .expect("resolver fixture is valid")
    }

    #[test]
    fn duplicate_ids_rejected() {
        let result = PlaybookResolver::from_playbooks(vec![
            Playbook {
                id: "dup".to_string(),
                target_class: TargetClass::ContentSite,
                description: String::new(),
                acquisition: AcquisitionDefaults::default(),
                proxy_preference: ProxyPreference::default(),
                pacing: PacingProfile::default(),
                escalation: EscalationStrategy::default(),
            },
            Playbook {
                id: "dup".to_string(),
                target_class: TargetClass::Api,
                description: String::new(),
                acquisition: AcquisitionDefaults::default(),
                proxy_preference: ProxyPreference::default(),
                pacing: PacingProfile::default(),
                escalation: EscalationStrategy::default(),
            },
        ]);
        assert!(matches!(
            result,
            Err(ValidationError::DuplicateId { .. })
        ));
    }

    #[test]
    fn invalid_playbook_rejected() {
        let result = PlaybookResolver::from_playbooks(vec![Playbook {
            id: "broken".to_string(),
            target_class: TargetClass::ContentSite,
            description: String::new(),
            acquisition: AcquisitionDefaults {
                retry_budget: 0,
                ..AcquisitionDefaults::default()
            },
            proxy_preference: ProxyPreference::default(),
            pacing: PacingProfile::default(),
            escalation: EscalationStrategy::default(),
        }]);
        let err = result.expect_err("retry_budget 0 is invalid");
        assert_eq!(err.field_path(), Some("acquisition.retry_budget"));
    }

    #[test]
    fn request_override_wins_over_playbook_default() {
        let resolver = make_resolver();
        let overrides = PlaybookOverrides {
            acquisition: AcquisitionOverrides {
                retry_budget: Some(99),
                ..AcquisitionOverrides::default()
            },
            ..PlaybookOverrides::default()
        };
        let resolved = resolver
            .resolve(TargetClass::ContentSite, "tier1-static", &overrides)
            .expect("resolve");
        assert_eq!(resolved.acquisition.retry_budget, 99);
        assert_eq!(
            resolved.acquisition.retry_budget_source,
            ResolutionSource::RequestOverride
        );
    }

    #[test]
    fn playbook_default_used_when_no_override() {
        let resolver = make_resolver();
        let resolved = resolver
            .resolve(
                TargetClass::ContentSite,
                "tier1-js",
                &PlaybookOverrides::default(),
            )
            .expect("resolve");
        assert_eq!(resolved.acquisition.retry_budget, 5);
        assert_eq!(
            resolved.acquisition.retry_budget_source,
            ResolutionSource::PlaybookDefault
        );
        assert!(resolved.acquisition.enable_warmup);
        assert_eq!(
            resolved.acquisition.enable_warmup_source,
            ResolutionSource::PlaybookDefault
        );
    }

    #[test]
    fn global_default_used_when_no_playbook_matches() {
        let resolver = make_resolver();
        let resolved = resolver
            .resolve(TargetClass::Api, "", &PlaybookOverrides::default())
            .expect("resolve");
        assert_eq!(resolved.playbook_id, "unknown");
        assert_eq!(
            resolved.acquisition.retry_budget_source,
            ResolutionSource::GlobalDefault
        );
    }

    #[test]
    fn unknown_explicit_id_returns_error() {
        let resolver = make_resolver();
        let err = resolver
            .resolve(
                TargetClass::ContentSite,
                "nope",
                &PlaybookOverrides::default(),
            )
            .expect_err("unknown id");
        assert!(matches!(err, ValidationError::UnknownPlaybook { .. }));
    }

    #[test]
    fn override_replaces_proxy_preference_whole() {
        let resolver = make_resolver();
        let proxy = ProxyPreference {
            preferred_protocol: "socks5".to_string(),
            require_sticky: true,
            require_residential: true,
            max_latency_ms: Some(300),
        };
        let overrides = PlaybookOverrides {
            proxy_preference: Some(proxy.clone()),
            ..PlaybookOverrides::default()
        };
        let resolved = resolver
            .resolve(TargetClass::ContentSite, "tier1-static", &overrides)
            .expect("resolve");
        assert_eq!(resolved.proxy_preference, proxy);
        assert_eq!(
            resolved.proxy_preference_source,
            ResolutionSource::RequestOverride
        );
    }

    #[test]
    fn override_replaces_pacing_whole() {
        let resolver = make_resolver();
        let pacing = PacingProfile {
            rate_limit_rps: 7.5,
            jitter_pct: 0.30,
            min_request_interval_ms: 150,
        };
        let overrides = PlaybookOverrides {
            pacing: Some(pacing.clone()),
            ..PlaybookOverrides::default()
        };
        let resolved = resolver
            .resolve(TargetClass::ContentSite, "tier1-static", &overrides)
            .expect("resolve");
        assert_eq!(resolved.pacing, pacing);
        assert_eq!(resolved.pacing_source, ResolutionSource::RequestOverride);
    }

    #[test]
    fn resolve_optional_falls_through_to_target_class_default() {
        let resolver = make_resolver();
        let resolved = resolver
            .resolve_optional(
                TargetClass::HighSecurity,
                None,
                &PlaybookOverrides::default(),
            )
            .expect("resolve");
        assert_eq!(resolved.playbook_id, "tier2-hostile");
    }

    #[test]
    fn to_acquisition_policy_propagates_fields() {
        let resolver = make_resolver();
        let resolved = resolver
            .resolve(
                TargetClass::ContentSite,
                "tier1-js",
                &PlaybookOverrides::default(),
            )
            .expect("resolve");
        let policy = resolved.to_acquisition_policy();
        assert_eq!(policy.retry_budget, 5);
        assert_eq!(policy.backoff_base_ms, 500);
        assert!(policy.enable_warmup);
        assert!(policy.sticky_session);
    }
}