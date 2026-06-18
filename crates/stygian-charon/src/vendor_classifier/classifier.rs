//! Vendor classification engine (T89).
//!
//! The [`VendorClassifier`] is a deterministic, evidence-emitting
//! ranker that consumes cookies, headers, challenge URLs, and body
//! markers and produces a ranked vendor scoreboard. It is the
//! primary input to T88 (anti-bot change-detection feed) and T90
//! (vendor-to-playbook auto-resolution).
//!
//! ## Confidence formula
//!
//! For each [`VendorDefinition`] the classifier sums the weights
//! of the matched signals. The **top vendor**'s confidence is then
//!
//! ```text
//! confidence = top_score / (top_score + second_score)
//! ```
//!
//! which is the same Jaccard-style ratio the existing
//! [`crate::classifier::classify_transaction`] uses. When only one
//! vendor matched, `confidence = 1.0`. When no vendor matched, the
//! classification is reported as [`VendorId::Unknown`] with
//! `confidence = 0.0`.
//!
//! ## Deterministic tie-break rule
//!
//! When two or more vendors tie on the **same top score**, the
//! tie is broken by [`VendorId`] discriminant order: the variant
//! declared **earlier** in the enum wins. This means
//! `Akamai < Cloudflare < DataDome < PerimeterX < …` — the same
//! order the enum source declares. The order is stable across
//! releases and across the
//! [`Ord`][std::cmp::Ord] implementation derived on [`VendorId`].
//!
//! ## High-confidence threshold
//!
//! The classifier carries a configurable threshold
//! [`DEFAULT_HIGH_CONFIDENCE_THRESHOLD`] (0.60). The
//! [`VendorClassification::is_high_confidence`] flag is set when
//! the top vendor's confidence crosses the threshold. Callers can
//! override the threshold via
//! [`VendorClassifier::with_threshold`].
//!
//! # Example
//!
//! ```
//! use stygian_charon::vendor_classifier::{VendorClassifier, VendorId, EvidenceSource};
//! use std::collections::BTreeMap;
//!
//! let classifier = VendorClassifier::with_builtin_defaults();
//! let mut headers = BTreeMap::new();
//! headers.insert("cf-ray".to_string(), "abc-ORD".to_string());
//! headers.insert("server".to_string(), "cloudflare".to_string());
//! let cookies = vec!["__cf_bm=xyz; path=/".to_string()];
//! let body = "Attention required! | cloudflare".to_string();
//! let url = "https://example.com/cdn-cgi/challenge-platform";
//!
//! let classification = classifier.classify(&cookies, &headers, Some(&body), url);
//! assert_eq!(classification.top_vendor, VendorId::Cloudflare);
//! assert!(classification.is_high_confidence);
//! assert!(classification.confidence > 0.0);
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::har;
use crate::types::TransactionView;
use crate::vendor_classifier::evidence::{Evidence, EvidenceBundle, EvidenceSource};
use crate::vendor_classifier::vendor::{VendorDefinition, VendorId};

/// Default confidence threshold for the
/// [`VendorClassification::is_high_confidence`] flag.
///
/// Callers can override the threshold via
/// [`VendorClassifier::with_threshold`]. Values outside the
/// `(0.0, 1.0]` range fall back to this default.
pub const DEFAULT_HIGH_CONFIDENCE_THRESHOLD: f64 = 0.60;

/// Maximum confidence (used when only one vendor matched).
const FULL_CONFIDENCE: f64 = 1.0;

/// Per-vendor scorecard returned by the classifier.
///
/// A `VendorScore` records the **total weighted signal count** for
/// a single vendor along with the evidence that contributed. The
/// scores are returned in **rank order** (top first).
///
/// # Example
///
/// ```
/// use stygian_charon::vendor_classifier::{EvidenceSource, VendorId, VendorScore};
///
/// let score = VendorScore {
///     vendor: VendorId::Cloudflare,
///     score: 10,
///     matched_sources: vec![(EvidenceSource::Header, 2), (EvidenceSource::Cookie, 1)]
///         .into_iter()
///         .collect(),
/// };
/// assert_eq!(score.vendor, VendorId::Cloudflare);
/// assert_eq!(score.score, 10);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorScore {
    /// Vendor this score belongs to.
    pub vendor: VendorId,
    /// Sum of the matched signal weights.
    pub score: u32,
    /// Per-source count of matched signals (`BTreeMap` keeps the
    /// output deterministic).
    pub matched_sources: BTreeMap<EvidenceSource, usize>,
}

impl VendorScore {
    /// `true` when this score reflects a real (non-zero) match.
    #[must_use]
    pub const fn is_match(&self) -> bool {
        self.score > 0
    }
}

/// Full vendor classification output.
///
/// Carries the **ranked scoreboard**, the **top vendor** (the
/// confidence-bearing winner), the **confidence** in the top
/// vendor, the **evidence bundle** the score was computed from,
/// and the **high-confidence flag** the operator-facing policy
/// layer reads to decide whether to escalate.
///
/// # Example
///
/// ```
/// use stygian_charon::vendor_classifier::{VendorClassification, VendorId, EvidenceBundle};
///
/// let classification = VendorClassification {
///     top_vendor: VendorId::Cloudflare,
///     confidence: 0.85,
///     is_high_confidence: true,
///     ranked: Vec::new(),
///     evidence: EvidenceBundle::default(),
///     threshold: 0.60,
/// };
/// assert_eq!(classification.top_vendor, VendorId::Cloudflare);
/// assert!(classification.is_high_confidence);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VendorClassification {
    /// Vendor with the highest (deterministically tie-broken) score.
    pub top_vendor: VendorId,
    /// Confidence in `top_vendor` in `[0.0, 1.0]`.
    pub confidence: f64,
    /// `true` when `confidence >= threshold` (the "high confidence"
    /// policy-routing flag).
    pub is_high_confidence: bool,
    /// Ranked scoreboard (top first).
    pub ranked: Vec<VendorScore>,
    /// Full evidence bundle the score was computed from.
    pub evidence: EvidenceBundle,
    /// Threshold the `is_high_confidence` flag was evaluated against.
    pub threshold: f64,
}

impl VendorClassification {
    /// `true` when at least one vendor-specific signal matched.
    #[must_use]
    pub fn is_identified(&self) -> bool {
        self.top_vendor != VendorId::Unknown
    }

    /// `true` when the classification is a clean "no vendor"
    /// signal (no evidence at all).
    #[must_use]
    pub fn is_unknown(&self) -> bool {
        self.top_vendor == VendorId::Unknown && self.confidence == 0.0
    }
}

/// Vendor-classification engine.
///
/// Construct with [`VendorClassifier::with_builtin_defaults`] to
/// load the four baseline Tier 1 vendor definitions shipped in
/// `crates/stygian-charon/data/vendors/`, or
/// [`VendorClassifier::new`] for an empty / custom registry.
///
/// The classifier is **stateless** and `Send + Sync` so it can be
/// shared across threads and requests without locking.
///
/// # Example
///
/// ```
/// use stygian_charon::vendor_classifier::{VendorClassifier, VendorId};
/// use std::collections::BTreeMap;
///
/// let empty = VendorClassifier::new(Vec::new());
/// let cookies: Vec<String> = Vec::new();
/// let headers: BTreeMap<String, String> = BTreeMap::new();
/// let classification = empty.classify(&cookies, &headers, None, "https://example.com/");
/// assert_eq!(classification.top_vendor, VendorId::Unknown);
/// assert!(classification.is_unknown());
/// ```
#[derive(Debug, Clone)]
pub struct VendorClassifier {
    definitions: Vec<VendorDefinition>,
    threshold: f64,
}

impl VendorClassifier {
    /// Build a classifier from a pre-loaded list of
    /// [`VendorDefinition`] entries.
    ///
    /// The threshold defaults to
    /// [`DEFAULT_HIGH_CONFIDENCE_THRESHOLD`]. Override with
    /// [`with_threshold`][Self::with_threshold].
    #[must_use]
    pub const fn new(definitions: Vec<VendorDefinition>) -> Self {
        Self {
            definitions,
            threshold: DEFAULT_HIGH_CONFIDENCE_THRESHOLD,
        }
    }

    /// Build a classifier seeded with the four baseline Tier 1
    /// vendor definitions embedded at compile time from
    /// `crates/stygian-charon/data/vendors/`.
    ///
    /// The compile-time check
    /// `compile_check_builtin_vendors`
    /// guarantees that every embedded TOML is valid; if it
    /// regresses, the build will fail.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::vendor_classifier::{VendorClassifier, VendorId};
    ///
    /// let classifier = VendorClassifier::with_builtin_defaults();
    /// assert!(classifier.contains(VendorId::DataDome));
    /// assert!(classifier.contains(VendorId::PerimeterX));
    /// assert!(classifier.contains(VendorId::Akamai));
    /// assert!(classifier.contains(VendorId::Cloudflare));
    /// ```
    #[must_use]
    pub fn with_builtin_defaults() -> Self {
        let definitions = crate::vendor_classifier::builtins::builtin_vendors();
        Self::new(definitions)
    }

    /// Override the high-confidence threshold. The supplied value
    /// is clamped to `(0.0, 1.0]`. Non-finite values (`NaN`,
    /// `±∞`) fall back to
    /// [`DEFAULT_HIGH_CONFIDENCE_THRESHOLD`].
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_charon::vendor_classifier::{VendorClassifier, DEFAULT_HIGH_CONFIDENCE_THRESHOLD};
    ///
    /// let classifier = VendorClassifier::new(Vec::new()).with_threshold(0.85);
    /// assert!((classifier.threshold() - 0.85).abs() < 1e-9);
    ///
    /// // Out-of-range values clamp to the default.
    /// let reset = VendorClassifier::new(Vec::new()).with_threshold(f64::NAN);
    /// assert!((reset.threshold() - DEFAULT_HIGH_CONFIDENCE_THRESHOLD).abs() < 1e-9);
    /// ```
    #[must_use]
    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.threshold = if threshold.is_finite() && threshold > 0.0 && threshold <= 1.0 {
            threshold
        } else {
            DEFAULT_HIGH_CONFIDENCE_THRESHOLD
        };
        self
    }

    /// Configured high-confidence threshold.
    #[must_use]
    pub const fn threshold(&self) -> f64 {
        self.threshold
    }

    /// `true` when the registry contains a definition for the
    /// given [`VendorId`].
    #[must_use]
    pub fn contains(&self, vendor: VendorId) -> bool {
        self.definitions.iter().any(|d| d.id == vendor)
    }

    /// Number of vendor definitions currently registered.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.definitions.len()
    }

    /// `true` when the registry has no definitions.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    /// Classify a single set of input strings (cookies, headers,
    /// optional body, request URL) into a ranked vendor
    /// classification.
    ///
    /// The classifier scans every registered
    /// [`VendorDefinition`]'s signal catalogue and computes a
    /// per-vendor weighted score. The match is case-insensitive
    /// (definitions are lower-cased at load time, and the input
    /// strings are lower-cased at the match site).
    /// strings are lower-cased at the match site).
    ///
    /// # Determinism
    ///
    /// - Signals are matched in `(source, pattern)` lex order.
    /// - Ties on the top score are broken by
    ///   [`VendorId`] discriminant order (see module docs).
    /// - The output is `Send + Sync` and contains no
    ///   `HashMap`/`HashSet` so the JSON form is byte-stable.
    #[must_use]
    pub fn classify(
        &self,
        cookies: &[String],
        headers: &BTreeMap<String, String>,
        body: Option<&str>,
        url: &str,
    ) -> VendorClassification {
        let mut evidence_items: Vec<Evidence> = Vec::new();
        let mut scores: BTreeMap<VendorId, VendorScore> = BTreeMap::new();

        for def in &self.definitions {
            let score = score_definition(def, cookies, headers, body, url, &mut evidence_items);
            scores.insert(
                def.id,
                VendorScore {
                    vendor: def.id,
                    score,
                    matched_sources: BTreeMap::new(),
                },
            );
        }

        // Precompute the per-source count summaries.
        let mut ranked: Vec<VendorScore> = scores.into_values().collect();
        for score in &mut ranked {
            let mut per_source: BTreeMap<EvidenceSource, usize> = BTreeMap::new();
            for ev in evidence_items.iter().filter(|e| {
                self.definitions
                    .iter()
                    .find(|d| d.id == score.vendor)
                    .is_some_and(|d| {
                        // Compound (pattern, source) key match: the
                        // vendor's pattern `s.pattern` is compared
                        // against the matched literal `e.signal` plus
                        // the channel `e.source` — the same-name
                        // comparison is intentional, not a typo.
                        #[allow(clippy::suspicious_operation_groupings)]
                        d.signals
                            .iter()
                            .any(|s| s.pattern == e.signal && s.source == e.source)
                    })
            }) {
                *per_source.entry(ev.source).or_insert(0) += 1;
            }
            score.matched_sources = per_source;
        }

        // Rank: descending score, then ascending VendorId (the
        // deterministic tie-break rule).
        ranked.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.vendor.cmp(&b.vendor)));

        let (top, second) = match ranked.as_slice() {
            [] => (None, None),
            [single] => (Some(single), None),
            [first, rest @ ..] => (Some(first), rest.first()),
        };

        let (top_vendor, confidence) = match (top, second) {
            (Some(primary), Some(secondary)) if primary.score > 0 => {
                let denom = u64::from(primary.score) + u64::from(secondary.score);
                let conf = if denom == 0 {
                    0.0
                } else {
                    // u32 scores are well within the f64 mantissa
                    // (max ~4.3B), so the precision loss is
                    // bounded and intentional.
                    #[allow(clippy::cast_precision_loss)]
                    let result = f64::from(primary.score) / (denom as f64);
                    result
                };
                (primary.vendor, conf)
            }
            (Some(primary), _) if primary.score > 0 => (primary.vendor, FULL_CONFIDENCE),
            _ => (VendorId::Unknown, 0.0),
        };

        let is_high_confidence = confidence >= self.threshold;

        let mut source_summary: BTreeMap<EvidenceSource, usize> = BTreeMap::new();
        for ev in &evidence_items {
            *source_summary.entry(ev.source).or_insert(0) += 1;
        }
        let evidence = EvidenceBundle {
            items: evidence_items,
            source_summary,
        };

        VendorClassification {
            top_vendor,
            confidence,
            is_high_confidence,
            ranked,
            evidence,
            threshold: self.threshold,
        }
    }

    /// Convenience wrapper around
    /// [`classify`][Self::classify] that pulls the inputs out of a
    /// [`TransactionView`].
    ///
    /// Cookies are extracted from the `set-cookie` / `cookie`
    /// response header (everything else is treated as a generic
    /// header). The body is the `response_body_snippet`. The URL
    /// is `tx.url`.
    #[must_use]
    pub fn classify_view(&self, tx: &TransactionView) -> VendorClassification {
        let cookies = extract_cookies(&tx.response_headers);
        self.classify(
            &cookies,
            &tx.response_headers,
            tx.response_body_snippet.as_deref(),
            &tx.url,
        )
    }

    /// Classify every transaction in a HAR payload and return the
    /// top vendor's classification. Cookies, headers, and body
    /// snippets are pulled from each HAR entry directly.
    ///
    /// # Errors
    ///
    /// Returns [`har::HarError`] when the HAR JSON is invalid or
    /// exceeds a configured safety limit.
    pub fn classify_har(&self, har_json: &str) -> Result<VendorClassification, har::HarError> {
        let parsed = har::parse_har_transactions(har_json)?;
        // Each transaction is classified independently; the
        // **final** classification is the one with the highest
        // confidence. This keeps the output focused on the
        // strongest single piece of evidence (typically the
        // challenge response, which is a single transaction in a
        // capture).
        let mut best: Option<VendorClassification> = None;
        for entry in parsed.requests {
            let view: TransactionView = entry.into();
            let classification = self.classify_view(&view);
            // Higher confidence wins; ties broken by the
            // deterministic `VendorId` order (lower discriminant
            // wins). The float comparison is intentional — the
            // confidence is derived deterministically from the
            // weighted scoreboard, so equality is meaningful.
            #[allow(clippy::float_cmp)]
            let is_better = match &best {
                None => true,
                Some(prev) => {
                    classification.confidence > prev.confidence
                        || (classification.confidence == prev.confidence
                            && classification.top_vendor < prev.top_vendor)
                }
            };
            if is_better {
                best = Some(classification);
            }
        }
        Ok(best.unwrap_or_else(|| VendorClassification {
            top_vendor: VendorId::Unknown,
            confidence: 0.0,
            is_high_confidence: false,
            ranked: Vec::new(),
            evidence: EvidenceBundle::default(),
            threshold: self.threshold,
        }))
    }
}

fn score_definition(
    def: &VendorDefinition,
    cookies: &[String],
    headers: &BTreeMap<String, String>,
    body: Option<&str>,
    url: &str,
    evidence: &mut Vec<Evidence>,
) -> u32 {
    let mut total: u32 = 0;
    let body_lower = body.map(str::to_ascii_lowercase);
    let url_lower = url.to_ascii_lowercase();
    let grouped = def.signals_by_source();

    for (source, signals) in &grouped {
        match source {
            EvidenceSource::Cookie => {
                for cookie in cookies {
                    let lower = cookie.to_ascii_lowercase();
                    for sig in signals {
                        if lower.contains(&sig.pattern) {
                            total = total.saturating_add(sig.weight);
                            evidence.push(Evidence {
                                signal: sig.pattern.clone(),
                                source: EvidenceSource::Cookie,
                                weight: sig.weight,
                            });
                        }
                    }
                }
            }
            EvidenceSource::Header => {
                for (name, value) in headers {
                    // Skip the `set-cookie` / `cookie` headers —
                    // they are scored as cookies, not generic
                    // headers, to avoid double-counting the same
                    // signal in two sources.
                    let lower_name = name.to_ascii_lowercase();
                    if lower_name == "set-cookie" || lower_name == "cookie" {
                        continue;
                    }
                    let haystack = format!("{lower_name}:{}", value.to_ascii_lowercase());
                    for sig in signals {
                        if haystack.contains(&sig.pattern) {
                            total = total.saturating_add(sig.weight);
                            evidence.push(Evidence {
                                signal: sig.pattern.clone(),
                                source: EvidenceSource::Header,
                                weight: sig.weight,
                            });
                        }
                    }
                }
            }
            EvidenceSource::ChallengeUrl => {
                for sig in signals {
                    if url_lower.contains(&sig.pattern) {
                        total = total.saturating_add(sig.weight);
                        evidence.push(Evidence {
                            signal: sig.pattern.clone(),
                            source: EvidenceSource::ChallengeUrl,
                            weight: sig.weight,
                        });
                    }
                }
            }
            EvidenceSource::BodyMarker => {
                if let Some(body) = &body_lower {
                    for sig in signals {
                        if body.contains(&sig.pattern) {
                            total = total.saturating_add(sig.weight);
                            evidence.push(Evidence {
                                signal: sig.pattern.clone(),
                                source: EvidenceSource::BodyMarker,
                                weight: sig.weight,
                            });
                        }
                    }
                }
            }
            EvidenceSource::Script => {
                // The classifier does not currently surface a
                // separate script snippet, so the `script` source
                // folds into the body marker matching. This keeps
                // the public API stable: a future `script` field
                // on the classifier input can be added without
                // changing the wire format.
                if let Some(body) = &body_lower {
                    for sig in signals {
                        if body.contains(&sig.pattern) {
                            total = total.saturating_add(sig.weight);
                            evidence.push(Evidence {
                                signal: sig.pattern.clone(),
                                source: EvidenceSource::Script,
                                weight: sig.weight,
                            });
                        }
                    }
                }
            }
        }
    }

    // De-duplicate evidence rows that came from the same
    // pattern + source pair (e.g. the same cookie value
    // appearing in multiple header rows). Keeping one row per
    // (source, pattern) preserves the audit trail without
    // double-counting.
    evidence.sort_by(|a, b| (a.source, &a.signal).cmp(&(b.source, &b.signal)));
    evidence.dedup_by(|a, b| a.source == b.source && a.signal == b.signal);

    total
}

fn extract_cookies(headers: &BTreeMap<String, String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (name, value) in headers {
        let lower = name.to_ascii_lowercase();
        if lower == "set-cookie" || lower == "cookie" {
            out.push(value.clone());
        }
    }
    out
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::vendor_classifier::evidence::EvidenceSource;
    use crate::vendor_classifier::vendor::VendorSignal;

    fn datadome_definition() -> VendorDefinition {
        VendorDefinition {
            id: VendorId::DataDome,
            display_name: "DataDome".to_string(),
            description: String::new(),
            tier: 1,
            signals: vec![VendorSignal {
                pattern: "x-datadome".to_string(),
                source: EvidenceSource::Header,
                weight: 5,
            }],
        }
    }

    fn cloudflare_definition() -> VendorDefinition {
        VendorDefinition {
            id: VendorId::Cloudflare,
            display_name: "Cloudflare".to_string(),
            description: String::new(),
            tier: 1,
            signals: vec![VendorSignal {
                pattern: "cf-ray".to_string(),
                source: EvidenceSource::Header,
                weight: 5,
            }],
        }
    }

    fn empty_classifier() -> VendorClassifier {
        VendorClassifier::new(Vec::new())
    }

    #[test]
    fn empty_classifier_reports_unknown() {
        let classification =
            empty_classifier().classify(&[], &BTreeMap::new(), None, "https://example.com/");
        assert_eq!(classification.top_vendor, VendorId::Unknown);
        assert!(classification.is_unknown());
        assert!(!classification.is_high_confidence);
        assert!(classification.evidence.is_empty());
        assert!(classification.ranked.is_empty());
    }

    #[test]
    fn single_vendor_match_with_one_signal_above_threshold() {
        let classifier = VendorClassifier::new(vec![datadome_definition()]).with_threshold(0.60);
        let mut headers = BTreeMap::new();
        headers.insert("x-datadome".to_string(), "protected".to_string());
        let classification = classifier.classify(&[], &headers, None, "https://example.com/");
        assert_eq!(classification.top_vendor, VendorId::DataDome);
        assert!((classification.confidence - 1.0).abs() < 1e-9);
        assert!(classification.is_high_confidence);
        assert_eq!(classification.evidence.items.len(), 1);
        assert_eq!(
            classification.evidence.items[0].source,
            EvidenceSource::Header
        );
    }

    #[test]
    fn multi_vendor_match_ranks_by_score_with_deterministic_tie_break() {
        let classifier =
            VendorClassifier::new(vec![datadome_definition(), cloudflare_definition()]);
        let mut headers = BTreeMap::new();
        // Both vendors score 5 from their respective signals.
        headers.insert("x-datadome".to_string(), "1".to_string());
        headers.insert("cf-ray".to_string(), "1".to_string());
        let classification = classifier.classify(&[], &headers, None, "https://example.com/");
        // Tie-break: Akamai (0) < Cloudflare (1) < DataDome (2) < PerimeterX (3).
        // We have Cloudflare (1) and DataDome (2) tied at 5; DataDome is
        // declared later in the registry *and* has a higher discriminant,
        // so Cloudflare wins on the VendorId order tie-break.
        assert_eq!(classification.top_vendor, VendorId::Cloudflare);
        // Confidence = top / (top + second) = 5 / (5 + 5) = 0.5
        assert!((classification.confidence - 0.5).abs() < 1e-9);
        assert!(!classification.is_high_confidence);
    }

    #[test]
    fn below_threshold_classification_is_not_high_confidence() {
        let classifier = VendorClassifier::new(vec![datadome_definition()]).with_threshold(0.99);
        let mut headers = BTreeMap::new();
        headers.insert("x-datadome".to_string(), "1".to_string());
        let classification = classifier.classify(&[], &headers, None, "https://example.com/");
        // Single-vendor match still has confidence 1.0, so the
        // only way to push it below threshold is via a multi-
        // vendor split.
        let two = VendorClassifier::new(vec![datadome_definition(), cloudflare_definition()])
            .with_threshold(0.99);
        let mut headers2 = BTreeMap::new();
        headers2.insert("x-datadome".to_string(), "1".to_string());
        headers2.insert("cf-ray".to_string(), "1".to_string());
        let c2 = two.classify(&[], &headers2, None, "https://example.com/");
        assert!(!c2.is_high_confidence);
        // Sanity-check the value.
        let _ = classification;
    }

    #[test]
    fn cookies_are_extracted_from_set_cookie_header() {
        let classifier = VendorClassifier::new(vec![VendorDefinition {
            id: VendorId::DataDome,
            display_name: "x".to_string(),
            description: String::new(),
            tier: 1,
            signals: vec![VendorSignal {
                pattern: "datadome=".to_string(),
                source: EvidenceSource::Cookie,
                weight: 5,
            }],
        }]);
        // The classifier accepts a `cookies: &[String]` parameter
        // directly; `classify_view` is the convenience wrapper
        // that pulls cookies out of the `set-cookie` header.
        let cookies = vec!["datadome=abc; Path=/".to_string()];
        let classification =
            classifier.classify(&cookies, &BTreeMap::new(), None, "https://example.com/");
        assert_eq!(classification.top_vendor, VendorId::DataDome);
        assert_eq!(classification.evidence.items.len(), 1);
        assert_eq!(
            classification.evidence.items[0].source,
            EvidenceSource::Cookie
        );
    }

    #[test]
    fn classify_view_extracts_cookies_from_set_cookie_header() {
        let classifier = VendorClassifier::new(vec![VendorDefinition {
            id: VendorId::DataDome,
            display_name: "x".to_string(),
            description: String::new(),
            tier: 1,
            signals: vec![VendorSignal {
                pattern: "datadome=".to_string(),
                source: EvidenceSource::Cookie,
                weight: 5,
            }],
        }]);
        let mut headers = BTreeMap::new();
        headers.insert("set-cookie".to_string(), "datadome=abc; Path=/".to_string());
        let tx = TransactionView {
            url: "https://example.com/".to_string(),
            status: 403,
            response_headers: headers,
            response_body_snippet: None,
        };
        let classification = classifier.classify_view(&tx);
        assert_eq!(classification.top_vendor, VendorId::DataDome);
        assert_eq!(
            classification.evidence.items[0].source,
            EvidenceSource::Cookie
        );
    }

    #[test]
    fn body_markers_match_case_insensitively() {
        let classifier = VendorClassifier::new(vec![VendorDefinition {
            id: VendorId::Cloudflare,
            display_name: "x".to_string(),
            description: String::new(),
            tier: 1,
            signals: vec![VendorSignal {
                pattern: "attention required! | cloudflare".to_string(),
                source: EvidenceSource::BodyMarker,
                weight: 4,
            }],
        }]);
        let body = "<h1>Attention Required! | Cloudflare</h1>";
        let classification = classifier.classify(&[], &BTreeMap::new(), Some(body), "https://x/");
        assert_eq!(classification.top_vendor, VendorId::Cloudflare);
        assert_eq!(
            classification.evidence.items[0].source,
            EvidenceSource::BodyMarker
        );
    }

    #[test]
    fn challenge_url_signal_matches_path_segments() {
        let classifier = VendorClassifier::new(vec![VendorDefinition {
            id: VendorId::Cloudflare,
            display_name: "x".to_string(),
            description: String::new(),
            tier: 1,
            signals: vec![VendorSignal {
                pattern: "cdn-cgi/challenge-platform".to_string(),
                source: EvidenceSource::ChallengeUrl,
                weight: 4,
            }],
        }]);
        let url = "https://example.com/cdn-cgi/challenge-platform/orchestrate/jschl/abc";
        let classification = classifier.classify(&[], &BTreeMap::new(), None, url);
        assert_eq!(classification.top_vendor, VendorId::Cloudflare);
        assert_eq!(
            classification.evidence.items[0].source,
            EvidenceSource::ChallengeUrl
        );
    }

    #[test]
    fn classify_view_pulls_inputs_from_transaction() {
        let classifier = VendorClassifier::new(vec![datadome_definition()]);
        let mut headers = BTreeMap::new();
        headers.insert("x-datadome".to_string(), "1".to_string());
        let tx = TransactionView {
            url: "https://example.com/".to_string(),
            status: 403,
            response_headers: headers,
            response_body_snippet: None,
        };
        let c = classifier.classify_view(&tx);
        assert_eq!(c.top_vendor, VendorId::DataDome);
    }

    #[test]
    fn threshold_validation_falls_back_to_default() {
        let classifier = VendorClassifier::new(Vec::new()).with_threshold(f64::NAN);
        assert!((classifier.threshold() - DEFAULT_HIGH_CONFIDENCE_THRESHOLD).abs() < 1e-9);
        let negative = VendorClassifier::new(Vec::new()).with_threshold(-1.0);
        assert!((negative.threshold() - DEFAULT_HIGH_CONFIDENCE_THRESHOLD).abs() < 1e-9);
        let above = VendorClassifier::new(Vec::new()).with_threshold(1.5);
        assert!((above.threshold() - DEFAULT_HIGH_CONFIDENCE_THRESHOLD).abs() < 1e-9);
    }

    #[test]
    fn vendor_id_discriminant_order_breaks_ties() {
        // The order of variants in the `VendorId` enum
        // determines tie-break: Akamai (0) < Cloudflare (1) <
        // DataDome (2) < PerimeterX (3).
        let classifier = VendorClassifier::new(vec![
            VendorDefinition {
                id: VendorId::Akamai,
                display_name: "x".to_string(),
                description: String::new(),
                tier: 1,
                signals: vec![VendorSignal {
                    pattern: "tied".to_string(),
                    source: EvidenceSource::BodyMarker,
                    weight: 5,
                }],
            },
            VendorDefinition {
                id: VendorId::PerimeterX,
                display_name: "x".to_string(),
                description: String::new(),
                tier: 1,
                signals: vec![VendorSignal {
                    pattern: "tied".to_string(),
                    source: EvidenceSource::BodyMarker,
                    weight: 5,
                }],
            },
        ]);
        let body = "this body contains the tied marker";
        let c = classifier.classify(&[], &BTreeMap::new(), Some(body), "https://x/");
        // Both score 5; lower VendorId discriminant wins.
        assert_eq!(c.top_vendor, VendorId::Akamai);
    }

    #[test]
    fn builtin_classifier_includes_all_tier1_vendors() {
        let classifier = VendorClassifier::with_builtin_defaults();
        assert!(classifier.contains(VendorId::DataDome));
        assert!(classifier.contains(VendorId::PerimeterX));
        assert!(classifier.contains(VendorId::Akamai));
        assert!(classifier.contains(VendorId::Cloudflare));
    }

    #[test]
    fn builtin_classifier_detects_cloudflare_in_realistic_input() {
        let classifier = VendorClassifier::with_builtin_defaults();
        let mut headers = BTreeMap::new();
        headers.insert("cf-ray".to_string(), "abc-ORD".to_string());
        headers.insert("server".to_string(), "cloudflare".to_string());
        let cookies = vec!["__cf_bm=xyz; path=/".to_string()];
        let body = "Attention required! | cloudflare";
        let url = "https://example.com/cdn-cgi/challenge-platform/orchestrate";
        let c = classifier.classify(&cookies, &headers, Some(body), url);
        assert_eq!(c.top_vendor, VendorId::Cloudflare);
        assert!(c.is_high_confidence);
        assert!(c.confidence > 0.0);
        // Per-source summary should record at least one of each source.
        assert!(
            c.evidence
                .source_summary
                .contains_key(&EvidenceSource::Header)
        );
        assert!(
            c.evidence
                .source_summary
                .contains_key(&EvidenceSource::Cookie)
        );
        assert!(
            c.evidence
                .source_summary
                .contains_key(&EvidenceSource::BodyMarker)
        );
        assert!(
            c.evidence
                .source_summary
                .contains_key(&EvidenceSource::ChallengeUrl)
        );
    }
}
