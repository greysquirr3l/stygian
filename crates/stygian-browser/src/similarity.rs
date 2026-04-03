//! Adaptive element similarity search for [`crate::page::PageHandle`].
//!
//! Computes a structural fingerprint for a reference DOM node and finds
//! candidates on the current page that exceed a configurable similarity
//! threshold, even after class names or depth have shifted.

use crate::page::NodeHandle;

// ─── ElementFingerprint ───────────────────────────────────────────────────────

/// A structural snapshot of a DOM element for similarity comparison.
///
/// Serialisable so callers can persist fingerprints across sessions.
///
/// # Example
///
/// ```
/// use stygian_browser::similarity::ElementFingerprint;
///
/// let fp = ElementFingerprint {
///     tag: "div".to_string(),
///     classes: vec!["card".to_string(), "highlighted".to_string()],
///     attr_names: vec!["data-id".to_string()],
///     depth: 3,
/// };
/// let json = serde_json::to_string(&fp).unwrap();
/// let back: ElementFingerprint = serde_json::from_str(&json).unwrap();
/// assert_eq!(fp.tag, back.tag);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ElementFingerprint {
    /// Lower-case tag name (e.g. `"div"`, `"a"`).
    pub tag: String,
    /// Sorted CSS class list (excluding the empty string).
    pub classes: Vec<String>,
    /// Sorted attribute names present on the element, excluding `"class"` and
    /// `"id"`.
    #[serde(rename = "attrNames")]
    pub attr_names: Vec<String>,
    /// Distance from `<body>` in the DOM tree (direct children of `<body>`
    /// have depth `0`).
    pub depth: u32,
}

// ─── SimilarityConfig ─────────────────────────────────────────────────────────

/// Tuning parameters for [`crate::page::PageHandle::find_similar`].
///
/// # Example
///
/// ```
/// use stygian_browser::similarity::SimilarityConfig;
///
/// let cfg = SimilarityConfig { threshold: 0.5, max_results: 5 };
/// assert!(cfg.threshold < SimilarityConfig::DEFAULT_THRESHOLD);
/// ```
#[derive(Debug, Clone)]
pub struct SimilarityConfig {
    /// Minimum score `[0.0, 1.0]` for a candidate to be included in results.
    ///
    /// Default: [`DEFAULT_THRESHOLD`](Self::DEFAULT_THRESHOLD).
    pub threshold: f32,
    /// Maximum number of results to return.  `0` means unlimited.
    ///
    /// Default: `10`.
    pub max_results: usize,
}

impl SimilarityConfig {
    /// Default minimum similarity threshold (`0.7`).
    pub const DEFAULT_THRESHOLD: f32 = 0.7;
}

impl Default for SimilarityConfig {
    fn default() -> Self {
        Self {
            threshold: Self::DEFAULT_THRESHOLD,
            max_results: 10,
        }
    }
}

// ─── SimilarMatch ─────────────────────────────────────────────────────────────

/// A candidate element that exceeded the similarity threshold.
pub struct SimilarMatch {
    /// The matching node handle.
    pub node: NodeHandle,
    /// Similarity score in `[0.0, 1.0]`.
    pub score: f32,
}

// ─── Scoring ──────────────────────────────────────────────────────────────────

/// Compute the weighted Jaccard similarity between two element fingerprints.
///
/// Weights:
/// | Component | Weight |
/// |-----------|--------|
/// | Tag name match | 0.40 |
/// | Class list Jaccard | 0.35 |
/// | Attribute name Jaccard | 0.15 |
/// | Depth proximity | 0.10 |
///
/// Returns a score in `[0.0, 1.0]`.
///
/// # Example
///
/// ```
/// use stygian_browser::similarity::{ElementFingerprint, jaccard_weighted};
///
/// let a = ElementFingerprint { tag: "div".into(), classes: vec!["foo".into()], attr_names: vec![], depth: 2 };
/// let b = a.clone();
/// assert!((jaccard_weighted(&a, &b) - 1.0).abs() < 1e-6);
/// ```
pub fn jaccard_weighted(reference: &ElementFingerprint, candidate: &ElementFingerprint) -> f32 {
    let tag_score = if reference.tag == candidate.tag {
        1.0_f32
    } else {
        0.0_f32
    };

    let class_score = jaccard_sets(&reference.classes, &candidate.classes);
    let attr_score = jaccard_sets(&reference.attr_names, &candidate.attr_names);

    // DOM tree depth is always small (< 1000 in practice); truncate to u16
    // for a lossless f32 conversion (u16 fits exactly in f32's 23-bit mantissa).
    let ref_depth = f32::from(u16::try_from(reference.depth).unwrap_or(u16::MAX));
    let cand_depth = f32::from(u16::try_from(candidate.depth).unwrap_or(u16::MAX));
    let depth_diff = (ref_depth - cand_depth).abs();
    let depth_score = 1.0_f32 / (1.0_f32 + depth_diff);

    depth_score.mul_add(
        0.1_f32,
        attr_score.mul_add(0.15_f32, tag_score.mul_add(0.4_f32, class_score * 0.35_f32)),
    )
}

/// Compute Jaccard similarity (|intersection| / |union|) for two **sorted**
/// string slices.
///
/// Returns `1.0` when both slices are empty (they are identical).
fn jaccard_sets(a: &[String], b: &[String]) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0_f32;
    }
    let mut intersection: usize = 0;
    let mut i = 0_usize;
    let mut j = 0_usize;
    while i < a.len() && j < b.len() {
        let (Some(ai), Some(bj)) = (a.get(i), b.get(j)) else {
            break;
        };
        match ai.cmp(bj) {
            std::cmp::Ordering::Equal => {
                intersection += 1;
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    let union = a.len() + b.len() - intersection;
    // union > 0 because at least one slice is non-empty (guarded above)
    // Class/attribute counts are tiny (< 100); u16 fits losslessly in f32.
    let i_f = f32::from(u16::try_from(intersection).unwrap_or(u16::MAX));
    let u_f = f32::from(u16::try_from(union).unwrap_or(u16::MAX));
    i_f / u_f
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::expect_used)] // serde failures in deterministic tests are programmer errors
mod tests {
    use super::*;

    fn fp(tag: &str, classes: &[&str], attrs: &[&str], depth: u32) -> ElementFingerprint {
        ElementFingerprint {
            tag: tag.to_string(),
            classes: classes.iter().map(|s| (*s).to_string()).collect(),
            attr_names: attrs.iter().map(|s| (*s).to_string()).collect(),
            depth,
        }
    }

    #[test]
    fn jaccard_identical() {
        let a = fp("div", &["card", "highlighted"], &["data-id"], 3);
        let b = a.clone();
        let score = jaccard_weighted(&a, &b);
        assert!(
            (score - 1.0_f32).abs() < 1e-5_f32,
            "identical fingerprints should score 1.0, got {score}"
        );
    }

    #[test]
    fn jaccard_disjoint() {
        let a = fp("div", &["foo", "bar"], &["data-x"], 0);
        let b = fp("span", &["baz", "qux"], &["data-y"], 20);
        let score = jaccard_weighted(&a, &b);
        // tag=0, classes=0, attrs=0; depth_prox = 1/(1+20) ≈ 0.0476; weight=0.1
        // expected ≈ 0.00476
        assert!(
            score < 0.05_f32,
            "disjoint fingerprints should score near 0, got {score}"
        );
        assert!(score >= 0.0_f32, "score must be non-negative, got {score}");
    }

    #[test]
    fn jaccard_partial() {
        // Same tag, half classes in common, no attrs, same depth
        let a = fp("div", &["a", "b"], &[], 2);
        let b = fp("div", &["a", "c"], &[], 2);
        let score = jaccard_weighted(&a, &b);
        // tag=1*0.4=0.4, classes=jaccard({a,b},{a,c})=1/3≈0.333*0.35≈0.1167
        // attrs=both empty→1.0*0.15=0.15, depth=1/(1+0)=1.0*0.1=0.1
        // total ≈ 0.4 + 0.1167 + 0.15 + 0.1 = 0.7667
        assert!(
            score > 0.5_f32,
            "partial-match fingerprint should score > 0.5, got {score}"
        );
        assert!(
            score < 0.9_f32,
            "partial-match fingerprint should score < 0.9, got {score}"
        );
    }

    #[test]
    fn similarity_config_default_threshold() {
        assert!(
            (SimilarityConfig::DEFAULT_THRESHOLD - 0.7_f32).abs() < f32::EPSILON,
            "DEFAULT_THRESHOLD should be 0.7"
        );
        let cfg = SimilarityConfig::default();
        assert!(
            (cfg.threshold - SimilarityConfig::DEFAULT_THRESHOLD).abs() < f32::EPSILON,
            "default threshold should equal DEFAULT_THRESHOLD"
        );
        assert_eq!(cfg.max_results, 10);
    }

    #[test]
    fn fingerprint_serde_roundtrip() {
        let original = ElementFingerprint {
            tag: "section".to_string(),
            classes: vec!["main".to_string(), "wrapper".to_string()],
            attr_names: vec!["aria-label".to_string(), "data-section".to_string()],
            depth: 5,
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let decoded: ElementFingerprint = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn fingerprint_serde_attr_names_key() {
        // The JSON key must be "attrNames" (camelCase) to match the JS output.
        let fp_val = ElementFingerprint {
            tag: "a".to_string(),
            classes: vec![],
            attr_names: vec!["href".to_string()],
            depth: 1,
        };
        let json = serde_json::to_string(&fp_val).expect("serialize");
        assert!(
            json.contains("\"attrNames\""),
            "JSON key must be 'attrNames', got: {json}"
        );
    }
}
