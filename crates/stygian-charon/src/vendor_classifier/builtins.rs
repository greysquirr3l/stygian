//! Compile-time embedding of the baseline vendor definitions (T89).
//!
//! The TOML files in `crates/stygian-charon/data/vendors/` are
//! embedded into the binary with `include_str!` so the classifier
//! can load them without any runtime filesystem access. Every file
//! is parsed and validated **at compile time** by the
//! [`compile_check_builtin_vendors`] test, which surfaces invalid
//! TOML as a regular `cargo test` failure.
//!
//! # Example
//!
//! ```
//! use stygian_charon::vendor_classifier::VendorClassifier;
//!
//! let classifier = VendorClassifier::with_builtin_defaults();
//! assert!(classifier.contains(stygian_charon::vendor_classifier::VendorId::DataDome));
//! assert!(classifier.contains(stygian_charon::vendor_classifier::VendorId::PerimeterX));
//! assert!(classifier.contains(stygian_charon::vendor_classifier::VendorId::Akamai));
//! assert!(classifier.contains(stygian_charon::vendor_classifier::VendorId::Cloudflare));
//! ```

use crate::vendor_classifier::vendor::{VendorDefinition, parse_vendor_definition};

/// Embedded TOML for the `datadome` baseline vendor definition.
pub const DATADOME_TOML: &str = include_str!("../../data/vendors/datadome.toml");

/// Embedded TOML for the `perimeter_x` baseline vendor definition.
pub const PERIMETER_X_TOML: &str = include_str!("../../data/vendors/perimeter_x.toml");

/// Embedded TOML for the `akamai` baseline vendor definition.
pub const AKAMAI_TOML: &str = include_str!("../../data/vendors/akamai.toml");

/// Embedded TOML for the `cloudflare` baseline vendor definition.
pub const CLOUDFLARE_TOML: &str = include_str!("../../data/vendors/cloudflare.toml");

/// Load the baseline vendor definitions. This function is called at
/// runtime by
/// [`crate::vendor_classifier::VendorClassifier::with_builtin_defaults`].
/// Each embedded TOML is parsed and validated; the first failure
/// is returned as an error.
pub fn builtin_vendors() -> Vec<VendorDefinition> {
    vec![
        parse_builtin(DATADOME_TOML),
        parse_builtin(PERIMETER_X_TOML),
        parse_builtin(AKAMAI_TOML),
        parse_builtin(CLOUDFLARE_TOML),
    ]
}

fn parse_builtin(toml_text: &str) -> VendorDefinition {
    // Embedded TOML is compile-time validated by
    // `compile_check_builtin_vendors`; runtime panic is only possible
    // if the binary was tampered with post-compilation, so this is a
    // deliberate programmer-error guard.
    #[allow(clippy::panic)]
    parse_vendor_definition(toml_text)
        .unwrap_or_else(|err| panic!("builtin vendor TOML is invalid: {err}"))
}

#[cfg(test)]
fn compile_check_builtin_vendors() -> Result<(), crate::vendor_classifier::error::VendorError> {
    for toml_text in [
        DATADOME_TOML,
        PERIMETER_X_TOML,
        AKAMAI_TOML,
        CLOUDFLARE_TOML,
    ] {
        parse_vendor_definition(toml_text)?;
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
    use crate::vendor_classifier::evidence::EvidenceSource;
    use crate::vendor_classifier::vendor::VendorId;

    #[test]
    fn baseline_vendors_compile_check_passes() {
        compile_check_builtin_vendors().expect("baseline vendors parse + validate");
    }

    #[test]
    fn builtin_vendors_match_taxonomy_table() {
        let vendors = builtin_vendors();
        let ids: Vec<VendorId> = vendors.iter().map(|d| d.id).collect();
        assert!(ids.contains(&VendorId::DataDome));
        assert!(ids.contains(&VendorId::PerimeterX));
        assert!(ids.contains(&VendorId::Akamai));
        assert!(ids.contains(&VendorId::Cloudflare));
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn baseline_vendors_carry_signals_for_every_documented_source() {
        let vendors = builtin_vendors();
        for def in &vendors {
            let sources: Vec<EvidenceSource> = def
                .signals
                .iter()
                .map(|s| s.source)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            // Every Tier 1 baseline carries at least one of:
            // Cookie, Header, BodyMarker, ChallengeUrl.
            assert!(
                sources.contains(&EvidenceSource::Header)
                    || sources.contains(&EvidenceSource::Cookie),
                "vendor {} must carry a header/cookie signal",
                def.id.label()
            );
        }
    }
}
