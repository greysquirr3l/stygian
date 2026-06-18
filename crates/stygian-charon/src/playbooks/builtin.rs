//! Compile-time embedding of the baseline playbooks (T85).
//!
//! The TOML files in `crates/stygian-charon/data/playbooks/` are
//! embedded into the binary with `include_str!` so the resolver can
//! load them without any runtime filesystem access. Every file is
//! parsed and validated **at compile time** by the
//! `compile_check_builtin_playbooks` test, which surfaces invalid
//! TOML as a regular `cargo test` failure.
//!
//! # Example
//!
//! ```
//! use stygian_charon::playbooks::{PlaybookResolver};
//!
//! let resolver = PlaybookResolver::with_builtin_defaults();
//! assert!(resolver.contains("tier1-static"));
//! assert!(resolver.contains("tier1-js"));
//! assert!(resolver.contains("tier2-hostile"));
//! assert!(resolver.contains("unknown"));
//! ```

#[cfg(test)]
use crate::playbooks::error::ValidationError;
use crate::playbooks::schema::Playbook;

/// Embedded TOML for the `tier1-static` baseline playbook.
///
/// Static HTML / cacheable content sites that do not require
/// JavaScript execution. Lowest-latency, lowest-cost acquisition
/// path.
pub const TIER1_STATIC_TOML: &str = include_str!("../../data/playbooks/tier1-static.toml");

/// Embedded TOML for the `tier1-js` baseline playbook.
///
/// JavaScript-rendered content sites that need a browser but do not
/// yet pose an active anti-bot challenge.
pub const TIER1_JS_TOML: &str = include_str!("../../data/playbooks/tier1-js.toml");

/// Embedded TOML for the `tier2-hostile` baseline playbook.
///
/// High-security sites with active anti-bot posture (`DataDome`,
/// Cloudflare Bot Management, Akamai Bot Manager, etc.). Requires
/// sticky-session residential proxies and a browser-stealth path.
pub const TIER2_HOSTILE_TOML: &str = include_str!("../../data/playbooks/tier2-hostile.toml");

/// Embedded TOML for the `unknown` fallback playbook.
///
/// Used when the resolver cannot match a target class to a
/// dedicated playbook. Always safe (no sticky session, no
/// residential-only proxy, no warmup).
pub const UNKNOWN_TOML: &str = include_str!("../../data/playbooks/unknown.toml");

/// Load the baseline playbooks. This function is called at runtime
/// by [`crate::playbooks::PlaybookResolver::with_builtin_defaults`].
/// Each embedded TOML is parsed and validated; the first failure is
/// returned as an error.
pub fn builtin_playbooks() -> Vec<Playbook> {
    vec![
        parse_builtin("tier1-static", TIER1_STATIC_TOML),
        parse_builtin("tier1-js", TIER1_JS_TOML),
        parse_builtin("tier2-hostile", TIER2_HOSTILE_TOML),
        parse_builtin("unknown", UNKNOWN_TOML),
    ]
}

fn parse_builtin(id: &str, toml_text: &str) -> Playbook {
    // Embedded TOML is compile-time validated by
    // `compile_check_builtin_playbooks`; runtime panic is only possible
    // if the binary was tampered with post-compilation, so this is a
    // deliberate programmer-error guard.
    #[allow(clippy::panic)]
    let pb: Playbook = toml::from_str(toml_text)
        .unwrap_or_else(|err| panic!("builtin playbook '{id}' TOML is invalid: {err}"));
    assert!(
        pb.id == id,
        "builtin playbook '{id}' has mismatched id field: '{}'",
        pb.id
    );
    #[allow(clippy::panic)]
    pb.validate()
        .unwrap_or_else(|err| panic!("builtin playbook '{id}' failed validation: {err}"));
    pb
}

/// Raw access to the embedded TOML — useful for diagnostics tests
/// that want to assert the data files are still shipped intact.
#[cfg(test)]
fn builtin_playbook_toml(id: &str) -> Option<&'static str> {
    match id {
        "tier1-static" => Some(TIER1_STATIC_TOML),
        "tier1-js" => Some(TIER1_JS_TOML),
        "tier2-hostile" => Some(TIER2_HOSTILE_TOML),
        "unknown" => Some(UNKNOWN_TOML),
        _ => None,
    }
}

/// Convenience accessor that re-parses the named TOML file on
/// demand. Test-only.
#[cfg(test)]
fn reparse(id: &str) -> Result<Playbook, ValidationError> {
    let Some(text) = builtin_playbook_toml(id) else {
        return Err(ValidationError::UnknownPlaybook {
            playbook_id: id.to_string(),
        });
    };
    let pb: Playbook = toml::from_str(text)?;
    pb.validate()?;
    Ok(pb)
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
    use crate::acquisition::AcquisitionModeHint;
    use crate::types::{ExecutionMode, TargetClass};

    #[test]
    fn tier1_static_has_expected_id_and_mode() {
        let pb = reparse("tier1-static").expect("parse");
        assert_eq!(pb.id, "tier1-static");
        assert_eq!(pb.target_class, TargetClass::ContentSite);
        assert_eq!(pb.acquisition.mode, AcquisitionModeHint::Fast);
    }

    #[test]
    fn tier1_js_uses_browser_execution() {
        let pb = reparse("tier1-js").expect("parse");
        assert_eq!(pb.id, "tier1-js");
        assert_eq!(pb.acquisition.execution_mode, ExecutionMode::Browser);
    }

    #[test]
    fn tier2_hostile_targets_high_security_class() {
        let pb = reparse("tier2-hostile").expect("parse");
        assert_eq!(pb.target_class, TargetClass::HighSecurity);
        assert_eq!(pb.acquisition.mode, AcquisitionModeHint::Hostile);
        assert!(pb.proxy_preference.require_residential);
    }

    #[test]
    fn unknown_targets_unknown_class() {
        let pb = reparse("unknown").expect("parse");
        assert_eq!(pb.target_class, TargetClass::Unknown);
    }

    /// Compile-time validation: every embedded playbook must parse
    /// and validate cleanly. If a baseline TOML is broken, this
    /// test fails the regular `cargo test` run.
    #[test]
    fn compile_check_builtin_playbooks() {
        for id in ["tier1-static", "tier1-js", "tier2-hostile", "unknown"] {
            reparse(id).unwrap_or_else(|err| panic!("builtin '{id}' invalid: {err}"));
        }
    }
}
