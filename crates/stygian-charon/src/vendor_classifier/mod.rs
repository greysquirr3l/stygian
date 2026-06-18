//! Vendor fingerprinting confidence classifier (T89).
//!
//! Identifies likely anti-bot vendor(s) for a target and produces
//! a confidence-scored evidence bundle for policy routing. The
//! classifier consumes cookies, response headers, challenge URLs,
//! and body markers; each piece of evidence is labelled by
//! [`EvidenceSource`] so the diagnostic payload can be audited
//! without re-running the match.
//!
//! ## Vendor taxonomy
//!
//! The four **Tier 1** vendors ship with signal catalogues
//! embedded at compile time:
//!
//! | `VendorId`     | Display name                | TOML file                        |
//! |----------------|-----------------------------|----------------------------------|
//! | `DataDome`     | DataDome                    | `data/vendors/datadome.toml`     |
//! | `PerimeterX`   | PerimeterX / HUMAN Security | `data/vendors/perimeter_x.toml`  |
//! | `Akamai`       | Akamai Bot Manager          | `data/vendors/akamai.toml`       |
//! | `Cloudflare`   | Cloudflare                  | `data/vendors/cloudflare.toml`   |
//!
//! Tier 2 vendors ([`VendorId::Hcaptcha`], [`VendorId::Recaptcha`],
//! [`VendorId::Kasada`], [`VendorId::FingerprintCom`],
//! [`VendorId::ShapeSecurity`], [`VendorId::Imperva`]) are present
//! in the enum so downstream T88/T90 layers can name them, but no
//! baseline signals ship for them. Operators register their own
//! catalogues via [`VendorDefinition`].
//!
//! [`VendorId::Unknown`] is the catch-all when no vendor matched
//! or no classification can be produced. It must remain the
//! **last** variant so it sorts last in the deterministic
//! tie-break rule.
//!
//! ## Determinism
//!
//! The classifier is fully deterministic:
//!
//! 1. Patterns are case-folded at load time and at the match site,
//!    so a vendor's score is byte-stable across runs.
//! 2. The top-score tie-break is **VendorId discriminant order**:
//!    the lower the variant is declared in [`VendorId`], the higher
//!    its priority when scores are equal.
//! 3. Confidence is `top_score / (top_score + second_score)`, so
//!    a single matched vendor always reports `1.0`.
//! 4. The `ranked` output is a `Vec` sorted by `(score DESC,
//!    VendorId ASC)`. The `evidence` bundle is sorted by
//!    `(source, signal)` and deduplicated so the JSON form is
//!    byte-stable.
//!
//! ## High-confidence threshold
//!
//! The classifier carries a configurable threshold
//! ([`DEFAULT_HIGH_CONFIDENCE_THRESHOLD`] = 0.60). The
//! [`VendorClassification::is_high_confidence`] flag is set when
//! the top vendor's confidence crosses the threshold. Callers can
//! override the threshold via
//! [`VendorClassifier::with_threshold`].
//!
//! ## Feature flag
//!
//! The module is **default-on** and lives in
//! `crates/stygian-charon/src/vendor_classifier/`. It adds two new
//! public types ([`VendorClassification`] and the underlying
//! [`VendorScore`]) and a single additive field on
//! [`crate::bundle::DiagnosticBundle`] (gated by
//! `#[serde(default, skip_serializing_if = "Option::is_none")]`).
//! No new feature gate is introduced because the additions are
//! purely additive.
//!
//! # Example
//!
//! ```
//! use stygian_charon::vendor_classifier::{VendorClassifier, VendorId, EvidenceSource};
//! use std::collections::BTreeMap;
//!
//! let classifier = VendorClassifier::with_builtin_defaults();
//! let cookies = vec!["datadome=xyz; path=/".to_string()];
//! let mut headers = BTreeMap::new();
//! headers.insert("x-datadome".to_string(), "protected".to_string());
//! headers.insert("x-datadome-cid".to_string(), "abc".to_string());
//! let body = Some("captcha-delivery.com iframe");
//! let url = "https://www.example.com/cdn-cgi/challenge-platform";
//!
//! let classification = classifier.classify(&cookies, &headers, body, url);
//! assert_eq!(classification.top_vendor, VendorId::DataDome);
//! assert!(classification.is_high_confidence);
//! assert!(classification.evidence.source_summary.contains_key(&EvidenceSource::Cookie));
//! ```

mod builtins;
mod classifier;
mod error;
mod evidence;
mod vendor;

pub use classifier::{
    DEFAULT_HIGH_CONFIDENCE_THRESHOLD, VendorClassification, VendorClassifier, VendorScore,
};
pub use error::VendorError;
pub use evidence::{Evidence, EvidenceBundle, EvidenceSource};
pub use vendor::{VendorDefinition, VendorId, VendorSignal, parse_vendor_definition};
