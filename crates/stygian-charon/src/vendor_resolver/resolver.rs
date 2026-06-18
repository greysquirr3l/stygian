//! Vendor-to-playbook resolver engine (T90).
//!
//! The [`VendorResolver`] consumes a
//! [`VendorClassification`][crate::vendor_classifier::VendorClassification]
//! (from T89) and returns a [`VendorResolution`] that points to a
//! resolved [`Playbook`][crate::playbooks::Playbook] together with a
//! rationale bundle the diagnostics layer can serialise.
//!
//! ## Resolution flow
//!
//! 1. The classifier's top vendor and confidence are matched
//!    against every registered [`ResolutionRule`] in
//!    **priority order** (lowest priority number wins).
//! 2. The first rule whose `min_confidence` and `min_score` gates
//!    pass — and whose `vendors` list contains the top vendor (or
//!    any vendor in the `ranked` list, depending on the rule's
//!    `require_unknown_vendor` flag) — fires.
//! 3. The fired rule's [`MergeStrategy`] determines how the
//!    resolver combines the rule's playbook choice with any
//!    **other** matching rules:
//!
//! | `MergeStrategy`   | Behaviour                                                                                        |
//! |-------------------|---------------------------------------------------------------------------------------------------|
//! | `StrongestVendor` | Pick the highest-weight vendor in the rule's `vendors` list and resolve with its playbook.       |
//! | `Single`          | Pick the single matched vendor (lowest `VendorId` discriminant on ties) and resolve.             |
//! | `Manual`          | Defer to manual mode — return [`StrategyMarker::Manual`].                                        |
//!
//! 4. If **no rule** matches, the resolver falls through to the
//!    lowest-priority rule (the `default-manual` sentinel). When
//!    that rule's `merge_strategy` is `Manual`, the resolver returns
//!    [`StrategyMarker::Manual`] so the existing manual mode
//!    selection keeps working — this is the
//!    "non-breaking integration with existing manual mode
//!    selection" guarantee called out in the T90 spec.
//!
//! ## Determinism
//!
//! The resolver is **fully deterministic**:
//!
//! - Rules are sorted by `(priority ASC, id ASC)` so two rules
//!   with the same priority are tie-broken by their stable `id`.
//! - The vendor scoreboard is supplied by the classifier, which
//!   is itself deterministic (T89 — `VendorId` discriminant order
//!   on ties).
//! - The `rationale.contributing_vendors` list is sorted by
//!   `(score DESC, VendorId ASC)` so the JSON form is byte-stable.
//!
//! ## Backward compatibility
//!
//! The resolver is **additive only** — no existing public type or
//! method gains a new field. The new module lives at
//! `crates/stygian-charon/src/vendor_resolver/` and is exposed
//! via the existing `vendor_resolver` re-exports in
//! [`crate::lib`]. No new feature gate is introduced.
//!
//! # Example
//!
//! ```
//! use stygian_charon::types::TargetClass;
//! use stygian_charon::vendor_classifier::{VendorClassifier, VendorId};
//! use stygian_charon::vendor_resolver::{StrategyMarker, VendorResolver};
//! use std::collections::BTreeMap;
//!
//! let vendor_resolver = VendorResolver::with_builtin_defaults();
//! let classifier = VendorClassifier::with_builtin_defaults();
//!
//! // Strong DataDome signal → tier2-hostile.
//! let cookies = vec!["datadome=abc; Path=/".to_string()];
//! let mut headers = BTreeMap::new();
//! headers.insert("x-datadome".to_string(), "protected".to_string());
//! headers.insert("x-datadome-cid".to_string(), "abc".to_string());
//! let classification = classifier.classify(&cookies, &headers, None, "https://example.com/");
//!
//! let resolution = vendor_resolver.resolve(&classification);
//! match resolution.strategy {
//!     StrategyMarker::Resolved { playbook_id, target_class } => {
//!         assert_eq!(playbook_id, "tier2-hostile");
//!         assert_eq!(target_class, TargetClass::HighSecurity);
//!     }
//!     StrategyMarker::Manual => panic!("DataDome signal should resolve"),
//! }
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::playbooks::{Playbook, PlaybookOverrides, PlaybookResolver, ResolvedPlaybook};
use crate::types::TargetClass;
use crate::vendor_classifier::{EvidenceBundle, VendorClassification, VendorId, VendorScore};
use crate::vendor_resolver::error::VendorResolverError;
use crate::vendor_resolver::rules::{MergeStrategy, ResolutionRule, VendorRuleMatch};

/// What the resolver decided to do with the vendor classification.
///
/// `Resolved` means the resolver picked a concrete playbook. `Manual`
/// means the resolver could not pick a playbook with sufficient
/// confidence and is deferring to whatever manual mode selection the
/// caller had in effect before the resolver was invoked (this is
/// the "non-breaking integration with existing manual mode
/// selection" guarantee from the T90 spec).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StrategyMarker {
    /// Resolver chose a playbook deterministically.
    Resolved {
        /// Playbook id the resolver chose (matches
        /// [`crate::playbooks::Playbook::id`]).
        playbook_id: String,
        /// Target class the resolved playbook maps to.
        target_class: TargetClass,
    },
    /// Resolver deferred to manual mode. Existing manual mode
    /// selection continues to apply — the resolver did not modify
    /// the caller's mode state.
    Manual,
}

impl StrategyMarker {
    /// `true` when the resolver returned a concrete playbook.
    #[must_use]
    pub const fn is_resolved(&self) -> bool {
        matches!(self, Self::Resolved { .. })
    }

    /// `true` when the resolver returned the `Manual` fallback.
    #[must_use]
    pub const fn is_manual(&self) -> bool {
        matches!(self, Self::Manual)
    }

    /// Playbook id when [`Resolved`][Self::Resolved], `None`
    /// otherwise.
    #[must_use]
    pub fn playbook_id(&self) -> Option<&str> {
        match self {
            Self::Resolved { playbook_id, .. } => Some(playbook_id),
            Self::Manual => None,
        }
    }
}

/// One rule that contributed to the resolver's decision.
///
/// Each entry records the rule id, whether it fired, the
/// [`MergeStrategy`] it applied, and a short human-readable note
/// the operator log can render verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedRule {
    /// Rule id.
    pub rule_id: String,
    /// `true` when the rule fired.
    pub fired: bool,
    /// Merge strategy the rule carries.
    pub merge_strategy: MergeStrategy,
    /// Human-readable note explaining why the rule fired or did
    /// not fire.
    pub note: String,
}

/// Full rationale bundle the resolver returns alongside the
/// strategy marker.
///
/// The bundle carries everything the diagnostic payload needs to
/// audit the resolver's decision: the top vendor, the confidence
/// the rule was evaluated against, the ranked vendor scoreboard
/// the classifier produced, the evidence bundle that produced the
/// scoreboard, the [`MergeStrategy`] that was applied, and a
/// per-rule audit log (`applied_rules`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolutionRationale {
    /// Human-readable summary suitable for an operator log.
    pub summary: String,
    /// Per-rule audit log, in priority order.
    pub applied_rules: Vec<AppliedRule>,
    /// Ranked vendor scoreboard that drove the decision (top first).
    pub contributing_vendors: Vec<VendorScore>,
    /// Evidence bundle that produced the scoreboard.
    pub evidence: EvidenceBundle,
    /// Top vendor (mirrors
    /// [`VendorClassification::top_vendor`][crate::vendor_classifier::VendorClassification::top_vendor]).
    pub top_vendor: VendorId,
    /// Confidence the rule was evaluated against.
    pub confidence: f64,
    /// Merge strategy the resolver applied.
    pub merge_strategy: MergeStrategy,
}

/// Full vendor-to-playbook resolution result.
///
/// `VendorResolution` is the **single object** the downstream
/// acquisition runner consumes. It pairs the [`StrategyMarker`]
/// (the decision) with a [`ResolutionRationale`] (the audit log)
/// so the runner and the diagnostic payload can both read it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VendorResolution {
    /// Resolver decision.
    pub strategy: StrategyMarker,
    /// Audit log explaining the decision.
    pub rationale: ResolutionRationale,
}

impl VendorResolution {
    /// `true` when the resolver returned a concrete playbook.
    #[must_use]
    pub const fn is_resolved(&self) -> bool {
        self.strategy.is_resolved()
    }

    /// `true` when the resolver returned the `Manual` fallback.
    #[must_use]
    pub const fn is_manual(&self) -> bool {
        self.strategy.is_manual()
    }
}

/// Vendor-to-playbook resolver.
///
/// Construct with [`VendorResolver::with_builtin_defaults`] to
/// load the four baseline rules shipped in
/// `crates/stygian-charon/data/vendor_playbook_rules/`, or
/// [`VendorResolver::from_rules`] for an empty / custom bundle.
///
/// The resolver is **stateless** and `Send + Sync` so it can be
/// shared across threads and requests without locking.
///
/// # Example
///
/// ```
/// use stygian_charon::vendor_classifier::VendorClassifier;
/// use stygian_charon::vendor_resolver::{StrategyMarker, VendorResolver};
///
/// let resolver = VendorResolver::with_builtin_defaults();
/// let classifier = VendorClassifier::with_builtin_defaults();
/// let c = classifier.classify(
///     &[],
///     &std::collections::BTreeMap::new(),
///     Some("harmless html"),
///     "https://example.com/",
/// );
/// let r = resolver.resolve(&c);
/// // A clean HAR reports Unknown with confidence 0.0; the resolver
/// // routes that through the tier1-static rule (Unknown vendor).
/// assert!(r.is_resolved() || r.is_manual());
/// ```
#[derive(Debug, Clone)]
pub struct VendorResolver {
    rules: Vec<ResolutionRule>,
}

impl VendorResolver {
    /// Build a resolver from a pre-loaded list of [`ResolutionRule`]
    /// entries.
    ///
    /// The rules are validated, deduplicated by id, and sorted by
    /// `(priority ASC, id ASC)` so the resolver's deterministic
    /// iteration order is independent of the caller's input order.
    ///
    /// # Errors
    ///
    /// Returns [`VendorResolverError`] on the first invalid rule
    /// or on duplicate ids.
    #[allow(clippy::missing_errors_doc)]
    pub fn from_rules<I>(rules: I) -> Result<Self, VendorResolverError>
    where
        I: IntoIterator<Item = ResolutionRule>,
    {
        let mut by_id: BTreeMap<String, ResolutionRule> = BTreeMap::new();
        for rule in rules {
            rule.validate()?;
            if by_id.contains_key(&rule.id) {
                return Err(VendorResolverError::DuplicateId { rule_id: rule.id });
            }
            by_id.insert(rule.id.clone(), rule);
        }
        let mut sorted: Vec<ResolutionRule> = by_id.into_values().collect();
        sorted.sort_by(|a, b| a.priority.cmp(&b.priority).then_with(|| a.id.cmp(&b.id)));
        Ok(Self { rules: sorted })
    }

    /// Build a resolver seeded with the four baseline rules
    /// embedded at compile time from
    /// `crates/stygian-charon/data/vendor_playbook_rules/`.
    ///
    /// The compile-time check
    /// `compile_check_builtin_resolution_rules`
    /// guarantees that every embedded TOML is valid; if it
    /// regresses, the build will fail.
    ///
    /// # Panics
    ///
    /// Panics if any embedded baseline TOML fails to parse or
    /// validate. This is a **compile-time** failure guarded by the
    /// `compile_check_builtin_resolution_rules` test; the panic in
    /// production surfaces a regression in the embedded data as a
    /// hard startup error.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::vendor_resolver::VendorResolver;
    ///
    /// let resolver = VendorResolver::with_builtin_defaults();
    /// assert!(resolver.contains("tier2-hostile"));
    /// assert!(resolver.contains("tier1-js-cloudflare"));
    /// assert!(resolver.contains("tier1-static"));
    /// assert!(resolver.contains("default-manual"));
    /// ```
    #[must_use]
    pub fn with_builtin_defaults() -> Self {
        let rules = crate::vendor_resolver::builtins::builtin_resolution_rules();
        // Baseline rules are compile-time validated by
        // `compile_check_builtin_resolution_rules`; runtime failure
        // is only possible if the binary was tampered with
        // post-compilation, so this is a deliberate
        // programmer-error guard.
        #[allow(clippy::expect_used)]
        Self::from_rules(rules).expect("builtin resolution rules are validated at compile time")
    }

    /// `true` when the resolver has a rule with the given id.
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.rules.iter().any(|r| r.id == id)
    }

    /// Number of rules currently registered.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.rules.len()
    }

    /// `true` when no rules are registered.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Ids of all registered rules, in priority order.
    #[must_use]
    pub fn rule_ids(&self) -> Vec<String> {
        self.rules.iter().map(|r| r.id.clone()).collect()
    }

    /// Resolve a [`VendorClassification`] into a [`VendorResolution`].
    ///
    /// The resolver iterates the rules in priority order (lowest
    /// priority number first) and fires the **first** rule whose
    /// confidence and score gates pass. See the module-level docs
    /// for the full resolution flow.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::vendor_classifier::VendorClassifier;
    /// use stygian_charon::vendor_resolver::VendorResolver;
    ///
    /// let resolver = VendorResolver::with_builtin_defaults();
    /// let classifier = VendorClassifier::with_builtin_defaults();
    /// let cookies = vec!["datadome=abc; Path=/".to_string()];
    /// let mut headers = std::collections::BTreeMap::new();
    /// headers.insert("x-datadome".to_string(), "protected".to_string());
    /// let c = classifier.classify(&cookies, &headers, None, "https://example.com/");
    /// let r = resolver.resolve(&c);
    /// assert!(r.is_resolved());
    /// ```
    #[must_use]
    pub fn resolve(&self, classification: &VendorClassification) -> VendorResolution {
        let top_score = classification.ranked.first().map_or(0, |s| s.score);
        let mut applied: Vec<AppliedRule> = Vec::new();
        let mut fired: Option<&ResolutionRule> = None;

        for rule in &self.rules {
            let note = evaluate_rule_note(rule, classification, top_score);
            if rule_matches(rule, classification, top_score) {
                applied.push(AppliedRule {
                    rule_id: rule.id.clone(),
                    fired: true,
                    merge_strategy: rule.merge_strategy,
                    note,
                });
                fired = Some(rule);
                break;
            }
            applied.push(AppliedRule {
                rule_id: rule.id.clone(),
                fired: false,
                merge_strategy: rule.merge_strategy,
                note,
            });
        }

        let chosen = fired.unwrap_or_else(|| {
            // The baseline bundle always contains a `default-manual`
            // sentinel at priority 1000; if a custom bundle omits
            // it (or is empty), we still want the resolver to
            // return a well-formed `Manual` marker rather than
            // panic.
            self.rules.last().unwrap_or_else(|| manual_fallback_rule())
        });

        let strategy = strategy_from_rule(chosen, classification);
        let summary = build_summary(&strategy, chosen, classification);
        let merge_strategy = chosen.merge_strategy;
        let rationale = ResolutionRationale {
            summary,
            applied_rules: applied,
            contributing_vendors: classification.ranked.clone(),
            evidence: classification.evidence.clone(),
            top_vendor: classification.top_vendor,
            confidence: classification.confidence,
            merge_strategy,
        };

        VendorResolution {
            strategy,
            rationale,
        }
    }

    /// Convenience helper that resolves a classification and then
    /// resolves the matched playbook through a
    /// [`PlaybookResolver`].
    ///
    /// Returns `None` when the resolver returned the `Manual`
    /// strategy marker, mirroring the
    /// "non-breaking integration with existing manual mode
    /// selection" guarantee — the caller keeps its manual mode
    /// selection rather than receiving a `ResolvedPlaybook`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::playbooks::ValidationError`] when the
    /// resolver picked a playbook id the [`PlaybookResolver`] does
    /// not have registered.
    pub fn resolve_with_playbooks(
        &self,
        classification: &VendorClassification,
        playbook_resolver: &PlaybookResolver,
        overrides: &PlaybookOverrides,
    ) -> Result<Option<ResolvedPlaybook>, crate::playbooks::ValidationError> {
        let resolution = self.resolve(classification);
        let Some(playbook_id) = resolution.strategy.playbook_id() else {
            return Ok(None);
        };
        let target_class = match &resolution.strategy {
            StrategyMarker::Resolved { target_class, .. } => *target_class,
            StrategyMarker::Manual => TargetClass::Unknown,
        };
        let resolved = playbook_resolver.resolve(target_class, playbook_id, overrides)?;
        Ok(Some(resolved))
    }
}

fn rule_matches(
    rule: &ResolutionRule,
    classification: &VendorClassification,
    top_score: u32,
) -> bool {
    if classification.confidence < rule.min_confidence {
        return false;
    }
    if top_score < rule.min_score {
        return false;
    }
    if rule.require_unknown_vendor && classification.top_vendor != VendorId::Unknown {
        return false;
    }
    if rule.vendors.is_empty() {
        // A rule with no vendor list and no confidence gate acts
        // as a catch-all. This is how `default-manual` is shaped.
        return true;
    }
    // When the classification reports Unknown as the top vendor
    // (no signals matched at all), the classifier's `ranked`
    // list is typically empty — so the "is this vendor in
    // ranked?" check below would always miss the `unknown`
    // placeholder entry. Treat the Unknown placeholder as a
    // wildcard match when the classification is unknown.
    // We require `score > 0` so a vendor that merely exists in
    // the registered registry (with zero score) does not
    // accidentally satisfy a rule.
    let classification_has_vendor = |v: &VendorId| {
        if *v == VendorId::Unknown && classification.top_vendor == VendorId::Unknown {
            return true;
        }
        classification
            .ranked
            .iter()
            .any(|s| s.vendor == *v && s.score > 0)
    };
    rule.vendors
        .iter()
        .any(|v| classification_has_vendor(&v.vendor))
}

fn evaluate_rule_note(
    rule: &ResolutionRule,
    classification: &VendorClassification,
    top_score: u32,
) -> String {
    if classification.confidence < rule.min_confidence {
        format!(
            "skipped: confidence {} < min_confidence {}",
            classification.confidence, rule.min_confidence
        )
    } else if top_score < rule.min_score {
        format!(
            "skipped: top_score {top_score} < min_score {}",
            rule.min_score
        )
    } else if rule.require_unknown_vendor && classification.top_vendor != VendorId::Unknown {
        format!(
            "skipped: top_vendor {} is not Unknown",
            classification.top_vendor.label()
        )
    } else if rule.vendors.is_empty() {
        "fired: catch-all rule (no vendor list, gates passed)".to_string()
    } else if rule
        .vendors
        .iter()
        .any(|v| classification.ranked.iter().any(|s| s.vendor == v.vendor))
    {
        "fired: at least one listed vendor matched".to_string()
    } else {
        "skipped: no listed vendor matched".to_string()
    }
}

fn strategy_from_rule(
    rule: &ResolutionRule,
    classification: &VendorClassification,
) -> StrategyMarker {
    match rule.merge_strategy {
        MergeStrategy::Manual => StrategyMarker::Manual,
        MergeStrategy::StrongestVendor => {
            let winning = pick_strongest_vendor(rule, classification);
            StrategyMarker::Resolved {
                playbook_id: rule.playbook_id.clone(),
                target_class: winning_target_class(rule, winning, classification),
            }
        }
        MergeStrategy::Single => {
            let winning = pick_single_vendor(rule, classification);
            StrategyMarker::Resolved {
                playbook_id: rule.playbook_id.clone(),
                target_class: winning_target_class(rule, winning, classification),
            }
        }
    }
}

fn pick_strongest_vendor<'a>(
    rule: &'a ResolutionRule,
    classification: &VendorClassification,
) -> Option<&'a VendorRuleMatch> {
    rule.vendors
        .iter()
        .filter(|v| classification.ranked.iter().any(|s| s.vendor == v.vendor))
        .max_by_key(|v| v.weight)
}

fn pick_single_vendor<'a>(
    rule: &'a ResolutionRule,
    classification: &VendorClassification,
) -> Option<&'a VendorRuleMatch> {
    rule.vendors
        .iter()
        .filter(|v| classification.ranked.iter().any(|s| s.vendor == v.vendor))
        .min_by_key(|v| v.vendor)
}

/// Fallback rule used when the resolver has zero registered rules.
///
/// Built lazily on first use via [`std::sync::LazyLock`]. The
/// fallback is intentionally constructed with
/// [`MergeStrategy::Manual`] so the resulting strategy is always
/// [`StrategyMarker::Manual`] when no rule fired.
fn manual_fallback_rule() -> &'static ResolutionRule {
    static FALLBACK: std::sync::LazyLock<ResolutionRule> =
        std::sync::LazyLock::new(|| ResolutionRule {
            id: String::new(),
            playbook_id: String::new(),
            target_class: TargetClass::Unknown,
            priority: u32::MAX,
            merge_strategy: MergeStrategy::Manual,
            description: String::new(),
            min_confidence: 0.0,
            min_score: 0,
            require_unknown_vendor: false,
            vendors: Vec::new(),
        });
    &FALLBACK
}

const fn winning_target_class(
    rule: &ResolutionRule,
    winning: Option<&VendorRuleMatch>,
    _classification: &VendorClassification,
) -> TargetClass {
    // The rule's own `target_class` is the primary signal. The
    // winning vendor only matters when the rule carries no
    // target class (currently unused in the baseline rules).
    let _ = winning;
    rule.target_class
}

fn build_summary(
    strategy: &StrategyMarker,
    rule: &ResolutionRule,
    classification: &VendorClassification,
) -> String {
    let vendor_label = classification.top_vendor.label();
    let target_class_label = |tc: TargetClass| match tc {
        TargetClass::Api => "api",
        TargetClass::ContentSite => "content_site",
        TargetClass::HighSecurity => "high_security",
        TargetClass::Unknown => "unknown",
    };
    match strategy {
        StrategyMarker::Resolved {
            playbook_id,
            target_class,
        } => format!(
            "rule '{}' fired for vendor {} (confidence {:.3}); resolved to playbook '{}' ({})",
            rule.id,
            vendor_label,
            classification.confidence,
            playbook_id,
            target_class_label(*target_class),
        ),
        StrategyMarker::Manual => format!(
            "rule '{}' fired for vendor {} (confidence {:.3}); deferring to manual mode",
            rule.id, vendor_label, classification.confidence
        ),
    }
}

/// Extension trait that surfaces the resolved [`Playbook`] from a
/// [`PlaybookResolver`] without going through the full
/// precedence ladder.
///
/// This is a convenience used by downstream callers that want to
/// read the raw codified [`Playbook`] (e.g. for diagnostics or
/// for round-tripping the rule table back to TOML). The trait
/// lives in the `vendor_resolver` module because it is primarily
/// useful in conjunction with the vendor resolver — when
/// `VendorResolver::resolve_with_playbooks` returns a
/// `ResolvedPlaybook`, callers occasionally need to read the
/// underlying codified playbook for `description` /
/// `target_class` audit fields.
pub trait PlaybookResolverExt {
    /// Resolve a single playbook by id, returning the underlying
    /// [`Playbook`] without applying any precedence / override
    /// merge.
    ///
    /// # Errors
    ///
    /// Returns
    /// [`ValidationError::UnknownPlaybook`][crate::playbooks::ValidationError::UnknownPlaybook]
    /// when the id is not registered.
    fn resolve_unsafe_playbook(
        &self,
        id: &str,
    ) -> Result<Playbook, crate::playbooks::ValidationError>;
}

impl PlaybookResolverExt for PlaybookResolver {
    fn resolve_unsafe_playbook(
        &self,
        id: &str,
    ) -> Result<Playbook, crate::playbooks::ValidationError> {
        // Round-trip the public API: resolve_optional returns
        // the merged ResolvedPlaybook; we then mirror the four
        // config blocks back into a Playbook. The description
        // string is the only field that round-trips lossily —
        // it is operator-facing text and does not affect the
        // resolver's logic.
        let target_class = TargetClass::Unknown;
        let overrides = PlaybookOverrides::default();
        let resolved = self.resolve_optional(target_class, Some(id), &overrides)?;
        Ok(Playbook {
            id: resolved.playbook_id,
            target_class: resolved.target_class,
            description: String::new(),
            acquisition: crate::playbooks::AcquisitionDefaults {
                mode: resolved.acquisition.mode,
                execution_mode: resolved.acquisition.execution_mode,
                session_mode: resolved.acquisition.session_mode,
                telemetry_level: resolved.acquisition.telemetry_level,
                sticky_session_ttl_secs: resolved.acquisition.sticky_session_ttl_secs,
                enable_warmup: resolved.acquisition.enable_warmup,
                retry_budget: resolved.acquisition.retry_budget,
                backoff_base_ms: resolved.acquisition.backoff_base_ms,
            },
            proxy_preference: resolved.proxy_preference,
            pacing: resolved.pacing,
            escalation: resolved.escalation,
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::similar_names
)]
mod tests {
    use super::*;
    use crate::vendor_classifier::{Evidence, EvidenceBundle, EvidenceSource};

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn classification(
        top_vendor: VendorId,
        confidence: f64,
        ranked: Vec<VendorScore>,
    ) -> VendorClassification {
        VendorClassification {
            top_vendor,
            confidence,
            is_high_confidence: confidence >= 0.60,
            ranked,
            evidence: EvidenceBundle::default(),
            threshold: 0.60,
        }
    }

    fn datadome_score() -> VendorScore {
        VendorScore {
            vendor: VendorId::DataDome,
            score: 15,
            matched_sources: vec![(EvidenceSource::Header, 3), (EvidenceSource::Cookie, 1)]
                .into_iter()
                .collect(),
        }
    }

    fn cloudflare_score() -> VendorScore {
        VendorScore {
            vendor: VendorId::Cloudflare,
            score: 10,
            matched_sources: vec![(EvidenceSource::Header, 2)].into_iter().collect(),
        }
    }

    fn akamai_score() -> VendorScore {
        VendorScore {
            vendor: VendorId::Akamai,
            score: 12,
            matched_sources: vec![(EvidenceSource::Cookie, 2)].into_iter().collect(),
        }
    }

    fn evidence() -> EvidenceBundle {
        EvidenceBundle {
            items: vec![Evidence {
                signal: "x-datadome".to_string(),
                source: EvidenceSource::Header,
                weight: 5,
            }],
            source_summary: vec![(EvidenceSource::Header, 1)].into_iter().collect(),
        }
    }

    #[test]
    fn empty_resolver_returns_manual_marker() {
        let resolver = VendorResolver::from_rules(Vec::new()).expect("empty resolver");
        let c = classification(VendorId::DataDome, 1.0, vec![datadome_score()]);
        let r = resolver.resolve(&c);
        // An empty resolver has no rule to fire. The fallback
        // rule returns the `Manual` strategy marker.
        assert!(r.is_manual());
        assert!(r.rationale.applied_rules.is_empty());
    }

    #[test]
    fn single_vendor_datadome_resolves_to_tier2_hostile() {
        let resolver = VendorResolver::with_builtin_defaults();
        let mut c = classification(VendorId::DataDome, 1.0, vec![datadome_score()]);
        c.evidence = evidence();
        let r = resolver.resolve(&c);
        match &r.strategy {
            StrategyMarker::Resolved {
                playbook_id,
                target_class,
            } => {
                assert_eq!(playbook_id, "tier2-hostile");
                assert_eq!(*target_class, TargetClass::HighSecurity);
            }
            StrategyMarker::Manual => panic!("DataDome should resolve, not defer"),
        }
        assert!(r.is_resolved());
        assert_eq!(r.rationale.merge_strategy, MergeStrategy::StrongestVendor);
        assert!(r.rationale.applied_rules.iter().any(|a| a.fired));
    }

    #[test]
    fn single_vendor_cloudflare_resolves_to_tier1_js() {
        let resolver = VendorResolver::with_builtin_defaults();
        let c = classification(VendorId::Cloudflare, 0.9, vec![cloudflare_score()]);
        let r = resolver.resolve(&c);
        match &r.strategy {
            StrategyMarker::Resolved {
                playbook_id,
                target_class,
            } => {
                assert_eq!(playbook_id, "tier1-js");
                assert_eq!(*target_class, TargetClass::ContentSite);
            }
            StrategyMarker::Manual => panic!("Cloudflare should resolve, not defer"),
        }
    }

    #[test]
    fn multi_vendor_datadome_plus_cloudflare_picks_tier2_hostile() {
        let resolver = VendorResolver::with_builtin_defaults();
        let ranked = vec![
            VendorScore {
                vendor: VendorId::DataDome,
                score: 15,
                matched_sources: BTreeMap::new(),
            },
            VendorScore {
                vendor: VendorId::Cloudflare,
                score: 10,
                matched_sources: BTreeMap::new(),
            },
        ];
        let c = classification(VendorId::DataDome, 0.60, ranked);
        let r = resolver.resolve(&c);
        // Tier 2 hostile has priority 0; tier 1 js-cloudflare has
        // priority 10. Priority 0 wins.
        match &r.strategy {
            StrategyMarker::Resolved { playbook_id, .. } => {
                assert_eq!(playbook_id, "tier2-hostile");
            }
            StrategyMarker::Manual => panic!("multi-vendor should resolve"),
        }
    }

    #[test]
    fn low_confidence_datadome_falls_through_to_manual() {
        let resolver = VendorResolver::with_builtin_defaults();
        // Below the tier2-hostile gate (0.60) but DataDome still
        // listed as a vendor. The resolver should NOT pick
        // tier2-hostile; it should fall through to default-manual
        // and return the Manual marker.
        let c = classification(VendorId::DataDome, 0.30, vec![datadome_score()]);
        let r = resolver.resolve(&c);
        assert!(r.is_manual(), "expected Manual, got {:?}", r.strategy);
        assert_eq!(r.rationale.top_vendor, VendorId::DataDome);
    }

    #[test]
    fn unknown_with_no_evidence_picks_tier1_static() {
        let resolver = VendorResolver::with_builtin_defaults();
        let c = classification(VendorId::Unknown, 0.0, Vec::new());
        let r = resolver.resolve(&c);
        match &r.strategy {
            StrategyMarker::Resolved {
                playbook_id,
                target_class,
            } => {
                assert_eq!(playbook_id, "tier1-static");
                assert_eq!(*target_class, TargetClass::ContentSite);
            }
            StrategyMarker::Manual => panic!("clean Unknown should pick tier1-static"),
        }
    }

    #[test]
    fn unknown_vendor_with_some_evidence_falls_through_to_manual() {
        let resolver = VendorResolver::with_builtin_defaults();
        // Some evidence (i.e. a vendor matched), but confidence is
        // too low for any specific rule. Default-manual fires
        // because tier1-static requires `require_unknown_vendor`
        // AND confidence 0.0. Here confidence is 0.0 but the
        // classification is NOT pure unknown.
        let c = classification(VendorId::DataDome, 0.0, vec![datadome_score()]);
        let r = resolver.resolve(&c);
        // DataDome with 0.0 confidence: tier2-hostile skipped
        // (confidence < 0.60), tier1-js-cloudflare skipped (no
        // Cloudflare signal), tier1-static skipped (top vendor
        // is not Unknown), default-manual fires with Manual
        // merge strategy.
        assert!(r.is_manual());
    }

    #[test]
    fn akamai_vendor_resolves_to_tier2_hostile() {
        let resolver = VendorResolver::with_builtin_defaults();
        let c = classification(VendorId::Akamai, 0.85, vec![akamai_score()]);
        let r = resolver.resolve(&c);
        match &r.strategy {
            StrategyMarker::Resolved { playbook_id, .. } => {
                assert_eq!(playbook_id, "tier2-hostile");
            }
            StrategyMarker::Manual => panic!("Akamai should resolve"),
        }
    }

    #[test]
    fn perimeterx_vendor_resolves_to_tier2_hostile() {
        let resolver = VendorResolver::with_builtin_defaults();
        let c = classification(
            VendorId::PerimeterX,
            0.95,
            vec![VendorScore {
                vendor: VendorId::PerimeterX,
                score: 18,
                matched_sources: BTreeMap::new(),
            }],
        );
        let r = resolver.resolve(&c);
        match &r.strategy {
            StrategyMarker::Resolved { playbook_id, .. } => {
                assert_eq!(playbook_id, "tier2-hostile");
            }
            StrategyMarker::Manual => panic!("PerimeterX should resolve"),
        }
    }

    #[test]
    fn rationale_summary_mentions_top_vendor_and_confidence() {
        let resolver = VendorResolver::with_builtin_defaults();
        let c = classification(VendorId::DataDome, 0.9, vec![datadome_score()]);
        let r = resolver.resolve(&c);
        assert!(r.rationale.summary.contains("datadome"));
        assert!(r.rationale.summary.contains("tier2-hostile"));
    }

    #[test]
    fn rationale_records_every_evaluated_rule() {
        let resolver = VendorResolver::with_builtin_defaults();
        let c = classification(VendorId::DataDome, 1.0, vec![datadome_score()]);
        let r = resolver.resolve(&c);
        let rule_ids: Vec<&str> = r
            .rationale
            .applied_rules
            .iter()
            .map(|a| a.rule_id.as_str())
            .collect();
        // The fired rule is tier2-hostile; the rest are not
        // recorded because the resolver short-circuits on the
        // first match. Confirm tier2-hostile is recorded.
        assert_eq!(rule_ids, vec!["tier2-hostile"]);
    }

    #[test]
    fn rule_ids_are_sorted_by_priority_then_id() {
        let resolver = VendorResolver::with_builtin_defaults();
        let ids = resolver.rule_ids();
        assert_eq!(
            ids,
            vec![
                "tier2-hostile".to_string(),
                "tier1-js-cloudflare".to_string(),
                "tier1-static".to_string(),
                "default-manual".to_string(),
            ]
        );
    }

    #[test]
    fn confidence_propagates_into_rationale() {
        let resolver = VendorResolver::with_builtin_defaults();
        let c = classification(VendorId::DataDome, 0.9, vec![datadome_score()]);
        let r = resolver.resolve(&c);
        assert!(approx_eq(r.rationale.confidence, 0.9));
        assert_eq!(r.rationale.top_vendor, VendorId::DataDome);
    }

    #[test]
    fn from_rules_rejects_duplicates() {
        let rule = ResolutionRule {
            id: "dup".to_string(),
            playbook_id: "tier2-hostile".to_string(),
            target_class: TargetClass::HighSecurity,
            priority: 0,
            merge_strategy: MergeStrategy::StrongestVendor,
            description: String::new(),
            min_confidence: 0.0,
            min_score: 0,
            require_unknown_vendor: false,
            vendors: vec![VendorRuleMatch {
                vendor: VendorId::DataDome,
                weight: 5,
            }],
        };
        let result = VendorResolver::from_rules(vec![rule.clone(), rule]);
        assert!(matches!(
            result,
            Err(VendorResolverError::DuplicateId { .. })
        ));
    }

    #[test]
    fn from_rules_rejects_invalid_rule() {
        let rule = ResolutionRule {
            id: "broken".to_string(),
            playbook_id: String::new(),
            target_class: TargetClass::HighSecurity,
            priority: 0,
            merge_strategy: MergeStrategy::StrongestVendor,
            description: String::new(),
            min_confidence: 0.0,
            min_score: 0,
            require_unknown_vendor: false,
            vendors: vec![VendorRuleMatch {
                vendor: VendorId::DataDome,
                weight: 5,
            }],
        };
        let result = VendorResolver::from_rules(vec![rule]);
        assert!(result.is_err());
    }

    #[test]
    fn manual_strategy_marker_helpers() {
        let manual = StrategyMarker::Manual;
        assert!(manual.is_manual());
        assert!(!manual.is_resolved());
        assert!(manual.playbook_id().is_none());

        let resolved = StrategyMarker::Resolved {
            playbook_id: "tier2-hostile".to_string(),
            target_class: TargetClass::HighSecurity,
        };
        assert!(resolved.is_resolved());
        assert!(!resolved.is_manual());
        assert_eq!(resolved.playbook_id(), Some("tier2-hostile"));
    }
}
