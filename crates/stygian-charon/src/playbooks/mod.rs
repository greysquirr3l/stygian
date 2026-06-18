//! Target-class playbook schema (T85).
//!
//! A [`Playbook`] is the **codified, opinionated strategy** for one
//! anti-bot tier. It bundles four operator-facing knobs into a single
//! document:
//!
//! 1. [`AcquisitionDefaults`] — the acquisition mode / execution mode /
//!    session mode / retry budget the runner should start with.
//! 2. [`ProxyPreference`] — which proxy flavour (datacenter /
//!    residential / mobile / SOCKS5) and which sticky-session
//!    constraints the proxy manager should honour.
//! 3. [`PacingProfile`] — pacing rate, jitter, and minimum
//!    inter-request interval.
//! 4. [`EscalationStrategy`] — the deterministic ladder the runner
//!    should climb when the current stage fails.
//!
//! Playbooks live on disk as TOML data files in
//! `crates/stygian-charon/data/playbooks/`. The schema is
//! serde-deserialisable so the same TOML files double as the
//! operator-facing configuration surface.
//!
//! # Validation
//!
//! Every public mutator or loader path calls [`Playbook::validate`] to
//! ensure the four knobs are internally consistent. Validation errors
//! are reported as [`ValidationError`] variants that include the
//! **field path** (`pacing.rate_limit_rps`) and the **bad value**
//! (e.g. `"-0.5"`) so operators can locate the offending line in the
//! TOML without having to re-run the loader.
//!
//! # Example
//!
//! ```
//! use stygian_charon::playbooks::{AcquisitionDefaults, EscalationStrategy, PacingProfile, Playbook, ProxyPreference};
//! use stygian_charon::acquisition::AcquisitionModeHint;
//! use stygian_charon::types::{ExecutionMode, SessionMode, TargetClass, TelemetryLevel};
//!
//! let pb = Playbook {
//!     id: "tier1-static".to_string(),
//!     target_class: TargetClass::ContentSite,
//!     description: "Static content sites with no JavaScript challenge".to_string(),
//!     acquisition: AcquisitionDefaults {
//!         mode: AcquisitionModeHint::Fast,
//!         execution_mode: ExecutionMode::Http,
//!         session_mode: SessionMode::Stateless,
//!         telemetry_level: TelemetryLevel::Basic,
//!         sticky_session_ttl_secs: None,
//!         enable_warmup: false,
//!         retry_budget: 2,
//!         backoff_base_ms: 250,
//!     },
//!     proxy_preference: ProxyPreference {
//!         preferred_protocol: "https".to_string(),
//!         require_sticky: false,
//!         require_residential: false,
//!         max_latency_ms: None,
//!     },
//!     pacing: PacingProfile {
//!         rate_limit_rps: 3.0,
//!         jitter_pct: 0.10,
//!         min_request_interval_ms: 250,
//!     },
//!     escalation: EscalationStrategy::Capped { ceiling: AcquisitionModeHint::Resilient },
//! };
//! assert!(pb.validate().is_ok());
//! ```

mod builtin;
mod error;
mod resolver;
mod schema;

pub use error::ValidationError;
pub use resolver::{PlaybookOverrides, PlaybookResolver, ResolvedPlaybook};
pub use schema::{
    AcquisitionDefaults, AcquisitionOverrides, EscalationStrategy, PacingProfile, Playbook,
    ProxyPreference, ResolutionSource,
};
