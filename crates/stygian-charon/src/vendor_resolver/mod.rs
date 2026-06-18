//! Vendor-to-playbook auto-resolution (T90).
//!
//! This module bridges the [`vendor_classifier`][crate::vendor_classifier]
//! (T89) and the [`playbooks`][crate::playbooks] resolver (T85):
//! given a [`VendorClassification`], it picks the right codified
//! [`Playbook`][crate::playbooks::Playbook] for the runner and
//! ships a rationale bundle the diagnostic payload can render.
//!
//! ## Resolution rule table
//!
//! The baseline rule bundle lives in
//! `crates/stygian-charon/data/vendor_playbook_rules/` and is
//! embedded into the binary at compile time via `include_str!`.
//! The four baseline rules and their precedence are:
//!
//! | Priority | Rule id                | Vendors                                                      | Resolves to            | Merge strategy     |
//! |----------|------------------------|--------------------------------------------------------------|------------------------|--------------------|
//! | `0`      | `tier2-hostile`        | `DataDome`, `PerimeterX`, `Akamai`, `Kasada`, `Imperva`, `ShapeSecurity` | `tier2-hostile` / `high_security` | `StrongestVendor` |
//! | `10`     | `tier1-js-cloudflare`  | `Cloudflare`, `Hcaptcha`, `Recaptcha`, `FingerprintCom`      | `tier1-js` / `content_site`       | `StrongestVendor` |
//! | `100`    | `tier1-static`         | `Unknown` (require_unknown_vendor = `true`)                  | `tier1-static` / `content_site`   | `Single`          |
//! | `1000`   | `default-manual`       | *(catch-all)*                                                | `Manual` strategy marker          | `Manual`          |
//!
//! Lower priority numbers win. When a rule's
//! [`min_confidence`][crate::vendor_resolver::rules::ResolutionRule::min_confidence]
//! gate passes **and** at least one of its listed vendors is in the
//! classifier's ranked scoreboard, the rule fires.
//!
//! ## Multi-vendor precedence + merge
//!
//! The classifier emits a ranked scoreboard (top vendor first,
//! ties broken by [`VendorId`][crate::vendor_classifier::VendorId]
//! discriminant order). When the scoreboard lists multiple
//! vendors that match the fired rule, the
//! [`MergeStrategy`][crate::vendor_resolver::rules::MergeStrategy]
//! determines how the rule consolidates them into a single
//! decision:
//!
//! | `MergeStrategy`   | Behaviour                                                                                              |
//! |-------------------|---------------------------------------------------------------------------------------------------------|
//! | `StrongestVendor` | Pick the listed vendor with the highest per-rule weight. Used by the two high-priority rules.          |
//! | `Single`          | Pick the single listed vendor (ties broken by `VendorId` discriminant order). Used by `tier1-static`.   |
//! | `Manual`          | Defer to manual mode — return [`StrategyMarker::Manual`]. Used by the `default-manual` sentinel.       |
//!
//! ## Low-confidence fallback
//!
//! When no specific rule fires, the resolver falls through to the
//! `default-manual` sentinel and returns
//! [`StrategyMarker::Manual`]. The existing manual mode
//! selection is **not** modified by the resolver — the caller keeps
//! whatever mode it had in effect. This is the
//! "non-breaking integration with existing manual mode selection"
//! guarantee from the T90 spec.
//!
//! ## Determinism
//!
//! The resolver is **fully deterministic**:
//!
//! - Rules are sorted by `(priority ASC, id ASC)` on construction,
//!   so two rules with the same priority are tie-broken by their
//!   stable `id`.
//! - The vendor scoreboard is supplied by the classifier, which
//!   is itself deterministic (T89 — `VendorId` discriminant
//!   order on ties).
//! - The `rationale.contributing_vendors` list is sorted by
//!   `(score DESC, VendorId ASC)` so the JSON form is byte-stable.
//!
//! ## Backward compatibility
//!
//! The resolver is **additive only** — no existing public type or
//! method gains a new field. The new module lives at
//! `crates/stygian-charon/src/vendor_resolver/` and is exposed via
//! the `vendor_resolver` re-exports below. No new feature gate is
//! introduced (per the T90 spec — `## Feature flag`).
//!
//! ## Feature flag
//!
//! The module is **default-on**. It is compiled into every build
//! of `stygian-charon` (which already defaults to the `caching`
//! feature). No new feature gate is introduced because the new
//! surface is purely additive — no existing public type gains a
//! new field, no existing behaviour changes, and the manual
//! fallback is non-breaking with the pre-T90 acquisition runner.
//!
//! # Example
//!
//! ```
//! use stygian_charon::types::TargetClass;
//! use stygian_charon::vendor_classifier::{VendorClassifier, VendorId};
//! use stygian_charon::vendor_resolver::{StrategyMarker, VendorResolver};
//! use std::collections::BTreeMap;
//!
//! let resolver = VendorResolver::with_builtin_defaults();
//! let classifier = VendorClassifier::with_builtin_defaults();
//! let cookies = vec!["datadome=abc; Path=/".to_string()];
//! let mut headers = BTreeMap::new();
//! headers.insert("x-datadome".to_string(), "protected".to_string());
//! headers.insert("x-datadome-cid".to_string(), "abc".to_string());
//! let classification =
//!     classifier.classify(&cookies, &headers, None, "https://example.com/");
//!
//! let resolution = resolver.resolve(&classification);
//! assert!(resolution.is_resolved());
//! match resolution.strategy {
//!     StrategyMarker::Resolved { playbook_id, target_class } => {
//!         assert_eq!(playbook_id, "tier2-hostile");
//!         assert_eq!(target_class, TargetClass::HighSecurity);
//!     }
//!     StrategyMarker::Manual => panic!("DataDome should resolve, not defer"),
//! }
//! ```

mod builtins;
mod error;
mod resolver;
mod rules;

pub use error::VendorResolverError;
pub use resolver::{
    AppliedRule, PlaybookResolverExt, ResolutionRationale, StrategyMarker, VendorResolution,
    VendorResolver,
};
pub use rules::{MergeStrategy, ResolutionRule, VendorRuleMatch, parse_resolution_rule};
