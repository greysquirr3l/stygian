//! Vendor-classifier evidence types (T89).
//!
//! Every signal the [`VendorClassifier`][crate::vendor_classifier::VendorClassifier]
//! observes is recorded as an [`Evidence`] item, labelled by its
//! [`EvidenceSource`]. The bundle of matched evidence is returned
//! alongside the ranked scores so diagnostics consumers can audit
//! *why* the classifier picked a given vendor without re-running it.
//!
//! ## Determinism
//!
//! Signals are sorted by `(source, signal)` in lexicographic order
//! before the score is computed. This keeps the confidence output
//! stable across runs even when the input vectors are produced in
//! different orders (a common pitfall when assembling
//! cookie/header/body strings from independent matchers).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Where a single classifier signal came from.
///
/// The five variants are the documented input channels for
/// [`crate::vendor_classifier::VendorClassifier`]:
///
/// | Source         | Where it was found                                  |
/// |----------------|-----------------------------------------------------|
/// | `Cookie`       | A `Set-Cookie` response header or `Cookie` header. |
/// | `Header`       | Any other response header.                          |
/// | `ChallengeUrl` | A challenge/redirect URL (request URL or `Location`).|
/// | `BodyMarker`   | A literal string in the response body snippet.      |
/// | `Script`       | A literal in a `<script>` snippet (inline JS).      |
///
/// The taxonomy is `#[serde(rename_all = "snake_case")]` so the
/// wire form is stable across releases.
///
/// # Example
///
/// ```
/// use stygian_charon::vendor_classifier::EvidenceSource;
///
/// let src = EvidenceSource::Cookie;
/// assert_eq!(src.label(), "cookie");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    /// `Set-Cookie` or `Cookie` header.
    Cookie,
    /// Any non-cookie response header.
    Header,
    /// Challenge/redirect URL (request URL or `Location` header).
    ChallengeUrl,
    /// Literal string in the response body.
    BodyMarker,
    /// Literal in a `<script>` block (inline JS challenge).
    Script,
}

impl EvidenceSource {
    /// Stable, human-readable label.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::vendor_classifier::EvidenceSource;
    ///
    /// assert_eq!(EvidenceSource::Cookie.label(), "cookie");
    /// assert_eq!(EvidenceSource::Header.label(), "header");
    /// assert_eq!(EvidenceSource::ChallengeUrl.label(), "challenge_url");
    /// assert_eq!(EvidenceSource::BodyMarker.label(), "body_marker");
    /// assert_eq!(EvidenceSource::Script.label(), "script");
    /// ```
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cookie => "cookie",
            Self::Header => "header",
            Self::ChallengeUrl => "challenge_url",
            Self::BodyMarker => "body_marker",
            Self::Script => "script",
        }
    }
}

/// One matched signal in the evidence bundle.
///
/// An `Evidence` row is the **smallest auditable unit** the
/// classifier emits. Each row carries:
///
/// - the literal `signal` text that matched,
/// - the [`EvidenceSource`] it came from,
/// - and the `weight` (sourced from the vendor definition) that
///   the classifier added to the vendor's score for this match.
///
/// # Example
///
/// ```
/// use stygian_charon::vendor_classifier::{Evidence, EvidenceSource};
///
/// let ev = Evidence {
///     signal: "_abck=".to_string(),
///     source: EvidenceSource::Cookie,
///     weight: 5,
/// };
/// assert_eq!(ev.source, EvidenceSource::Cookie);
/// assert_eq!(ev.weight, 5);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// The literal text that matched (case-folded, lower-cased).
    pub signal: String,
    /// Which input channel produced the match.
    pub source: EvidenceSource,
    /// Weight contributed to the vendor score (from the vendor
    /// definition's `signals[*].weight`).
    pub weight: u32,
}

/// Bundle of every [`Evidence`] item the classifier observed,
/// plus a per-source count summary.
///
/// The bundle is **append-only**: the classifier never drops or
/// re-orders evidence after the match phase.
///
/// The [`source_summary`][Self::source_summary] is a precomputed
/// `BTreeMap` so consumers can render a compact "matched
/// `n_cookies` cookie + `n_headers` header" summary without
/// walking the evidence vector.
///
/// # Example
///
/// ```
/// use stygian_charon::vendor_classifier::{Evidence, EvidenceBundle, EvidenceSource};
///
/// let bundle = EvidenceBundle {
///     items: vec![Evidence {
///         signal: "x-datadome".to_string(),
///         source: EvidenceSource::Header,
///         weight: 5,
///     }],
///     source_summary: vec![(EvidenceSource::Header, 1)].into_iter().collect(),
/// };
/// assert_eq!(bundle.items.len(), 1);
/// assert_eq!(bundle.source_summary.get(&EvidenceSource::Header), Some(&1));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EvidenceBundle {
    /// Every evidence row the classifier observed, in match order.
    pub items: Vec<Evidence>,
    /// Precomputed per-source count summary.
    pub source_summary: BTreeMap<EvidenceSource, usize>,
}

impl EvidenceBundle {
    /// Total number of evidence items in the bundle.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.items.len()
    }

    /// `true` when the bundle is empty (no signals matched).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// All evidence rows that came from a single source.
    pub fn for_source(&self, source: EvidenceSource) -> impl Iterator<Item = &Evidence> {
        self.items.iter().filter(move |e| e.source == source)
    }
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

    #[test]
    fn evidence_source_labels_are_stable() {
        assert_eq!(EvidenceSource::Cookie.label(), "cookie");
        assert_eq!(EvidenceSource::Header.label(), "header");
        assert_eq!(EvidenceSource::ChallengeUrl.label(), "challenge_url");
        assert_eq!(EvidenceSource::BodyMarker.label(), "body_marker");
        assert_eq!(EvidenceSource::Script.label(), "script");
    }

    #[test]
    fn evidence_source_serde_round_trip_is_stable() {
        for src in [
            EvidenceSource::Cookie,
            EvidenceSource::Header,
            EvidenceSource::ChallengeUrl,
            EvidenceSource::BodyMarker,
            EvidenceSource::Script,
        ] {
            let json = serde_json::to_string(&src).expect("serialize");
            let back: EvidenceSource = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(src, back);
            assert_eq!(json, format!("\"{}\"", src.label()));
        }
    }

    #[test]
    fn evidence_bundle_filter_by_source_is_correct() {
        let bundle = EvidenceBundle {
            items: vec![
                Evidence {
                    signal: "x-datadome".to_string(),
                    source: EvidenceSource::Header,
                    weight: 5,
                },
                Evidence {
                    signal: "datadome=".to_string(),
                    source: EvidenceSource::Cookie,
                    weight: 4,
                },
                Evidence {
                    signal: "cf-ray".to_string(),
                    source: EvidenceSource::Header,
                    weight: 5,
                },
            ],
            source_summary: vec![(EvidenceSource::Header, 2), (EvidenceSource::Cookie, 1)]
                .into_iter()
                .collect(),
        };
        let headers = bundle.for_source(EvidenceSource::Header).count();
        assert_eq!(headers, 2);
        assert_eq!(bundle.len(), 3);
        assert!(!bundle.is_empty());
    }

    #[test]
    fn empty_bundle_is_empty() {
        let bundle = EvidenceBundle::default();
        assert_eq!(bundle.len(), 0);
        assert!(bundle.is_empty());
    }
}
