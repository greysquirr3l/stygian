//! Compile-time embedding of the baseline resolution rules (T90).
//!
//! The TOML files in
//! `crates/stygian-charon/data/vendor_playbook_rules/` are
//! embedded into the binary with `include_str!` so the resolver
//! can load them without any runtime filesystem access. Every file
//! is parsed and validated **at compile time** by the
//! [`compile_check_builtin_resolution_rules`] test, which surfaces
//! invalid TOML as a regular `cargo test` failure.
//!
//! # Example
//!
//! ```
//! use stygian_charon::vendor_resolver::VendorResolver;
//!
//! let resolver = VendorResolver::with_builtin_defaults();
//! assert!(resolver.contains("tier2-hostile"));
//! assert!(resolver.contains("tier1-js-cloudflare"));
//! assert!(resolver.contains("tier1-static"));
//! assert!(resolver.contains("default-manual"));
//! ```

use crate::vendor_resolver::rules::{ResolutionRule, parse_resolution_rule};

/// Embedded TOML for the `tier2-hostile` baseline resolution rule.
pub const TIER2_HOSTILE_TOML: &str =
    include_str!("../../data/vendor_playbook_rules/tier2-hostile.toml");

/// Embedded TOML for the `tier1-js-cloudflare` baseline rule.
pub const TIER1_JS_CLOUDFLARE_TOML: &str =
    include_str!("../../data/vendor_playbook_rules/tier1-js-cloudflare.toml");

/// Embedded TOML for the `tier1-static` baseline rule.
pub const TIER1_STATIC_RULE_TOML: &str =
    include_str!("../../data/vendor_playbook_rules/tier1-static.toml");

/// Embedded TOML for the `default-manual` baseline rule.
pub const DEFAULT_MANUAL_TOML: &str =
    include_str!("../../data/vendor_playbook_rules/default-manual.toml");

/// Load the baseline resolution rules. This function is called at
/// runtime by
/// [`crate::vendor_resolver::VendorResolver::with_builtin_defaults`].
/// Each embedded TOML is parsed and validated; the first failure is
/// returned as an error.
pub fn builtin_resolution_rules() -> Vec<ResolutionRule> {
    vec![
        parse_builtin(TIER2_HOSTILE_TOML),
        parse_builtin(TIER1_JS_CLOUDFLARE_TOML),
        parse_builtin(TIER1_STATIC_RULE_TOML),
        parse_builtin(DEFAULT_MANUAL_TOML),
    ]
}

fn parse_builtin(toml_text: &str) -> ResolutionRule {
    // Embedded TOML is compile-time validated by
    // `compile_check_builtin_resolution_rules`; runtime panic is only
    // possible if the binary was tampered with post-compilation, so
    // this is a deliberate programmer-error guard.
    #[allow(clippy::panic)]
    parse_resolution_rule(toml_text)
        .unwrap_or_else(|err| panic!("builtin resolution rule TOML is invalid: {err}"))
}

#[cfg(test)]
fn compile_check_builtin_resolution_rules()
-> Result<(), crate::vendor_resolver::error::VendorResolverError> {
    for toml_text in [
        TIER2_HOSTILE_TOML,
        TIER1_JS_CLOUDFLARE_TOML,
        TIER1_STATIC_RULE_TOML,
        DEFAULT_MANUAL_TOML,
    ] {
        parse_resolution_rule(toml_text)?;
    }
    Ok(())
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
    use crate::vendor_classifier::VendorId;

    #[test]
    fn baseline_resolution_rules_compile_check_passes() {
        compile_check_builtin_resolution_rules().expect("baseline rules parse + validate");
    }

    #[test]
    fn builtin_rules_match_documented_baseline_set() {
        let rules = builtin_resolution_rules();
        let ids: Vec<&str> = rules.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"tier2-hostile"));
        assert!(ids.contains(&"tier1-js-cloudflare"));
        assert!(ids.contains(&"tier1-static"));
        assert!(ids.contains(&"default-manual"));
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn tier2_hostile_rule_covers_documented_vendors() {
        let rules = builtin_resolution_rules();
        let rule = rules
            .iter()
            .find(|r| r.id == "tier2-hostile")
            .expect("tier2-hostile rule");
        let vendor_ids: Vec<VendorId> = rule.vendors.iter().map(|v| v.vendor).collect();
        assert!(vendor_ids.contains(&VendorId::DataDome));
        assert!(vendor_ids.contains(&VendorId::PerimeterX));
        assert!(vendor_ids.contains(&VendorId::Akamai));
        assert_eq!(rule.priority, 0);
        assert_eq!(rule.playbook_id, "tier2-hostile");
    }

    #[test]
    fn tier1_js_cloudflare_rule_prioritises_cloudflare() {
        let rules = builtin_resolution_rules();
        let rule = rules
            .iter()
            .find(|r| r.id == "tier1-js-cloudflare")
            .expect("tier1-js-cloudflare rule");
        assert_eq!(rule.playbook_id, "tier1-js");
        assert!(
            rule.vendors
                .iter()
                .any(|v| v.vendor == VendorId::Cloudflare)
        );
    }

    #[test]
    fn default_manual_rule_uses_manual_merge_strategy() {
        let rules = builtin_resolution_rules();
        let rule = rules
            .iter()
            .find(|r| r.id == "default-manual")
            .expect("default-manual rule");
        assert!(rule.playbook_id.is_empty());
        assert_eq!(
            rule.merge_strategy,
            crate::vendor_resolver::rules::MergeStrategy::Manual
        );
    }
}
