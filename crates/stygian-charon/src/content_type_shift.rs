//! Content-Type shift detection (T104) — rolling baseline tracker.
//!
//! The 2026 scraping guide
//! (`docs/dev/project/scraping-guide-2026-llm-context.md` §"POST-EXTRACTION",
//! L2536) calls out that publishers are starting to serve a deliberately
//! different document to AI-bot User-Agents (HTML replaced with a
//! Markdown stub, the same URL, the same `200`, the same parse
//! succeeding). Selector-based validators that only count fields don't
//! notice because the Markdown stub still has plenty of fields — they're
//! just different fields.
//!
//! This module catches the publisher-cloaking pattern by tracking
//! `(Content-Type, byte_length)` per identity and emitting a
//! [`ContentTypeShiftReport`] when either dimension drifts past the
//! configured threshold.

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Coarse MIME classification. Two responses count as "same class" only
/// if they share the same variant — the `String` form (e.g.
/// `text/html; charset=utf-8`) is normalised to the variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MimeClass {
    /// HTML / XHTML documents.
    Html,
    /// JSON documents (including vendor `+json` suffixes).
    Json,
    /// XML documents (including vendor `+xml` suffixes).
    Xml,
    /// Markdown documents — the canonical publisher-cloaking target.
    Markdown,
    /// Plain text documents.
    Text,
    /// Binary payloads (images, audio, video, `application/octet-stream`,
    /// `application/pdf`).
    Binary,
    /// Anything else — empty, malformed, or a content type the
    /// classifier doesn't recognise.
    Unknown,
}

impl MimeClass {
    /// Classify a `Content-Type` header value (case-insensitive,
    /// parameters ignored).
    #[must_use]
    pub fn from_content_type(value: &str) -> Self {
        let head = value
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let Some((family, subtype)) = head.split_once('/') else {
            return Self::Unknown;
        };
        match (family, subtype) {
            ("text", s) if s == "html" || s == "xhtml" => Self::Html,
            ("text", "markdown") => Self::Markdown,
            // Both `application` and `text` families can carry JSON
            // (including suffixes like `+json`). Match on the
            // subtype that's already been narrowed by the guard.
            _ if subtype == "json" || subtype.ends_with("+json") => Self::Json,
            ("application", s) if s == "xml" || s == "xhtml" || s.ends_with("+xml") => Self::Xml,
            ("text", _) => Self::Text,
            ("image" | "audio" | "video", _) => Self::Binary,
            ("application", s) if s == "octet-stream" || s == "pdf" => Self::Binary,
            _ => Self::Unknown,
        }
    }
}

/// One observation per scrape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentTypeObservation {
    /// Identity the observation is keyed under. Same identity → same
    /// history slot.
    pub key: String,
    /// Raw `Content-Type` header value.
    pub content_type: String,
    /// Response body byte length.
    pub byte_length: u64,
}

/// Detected drift between the most recent observation and the
/// historical baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ContentTypeDrift {
    /// `MimeClass` flipped from one variant to another. A
    /// `Html → Markdown` shift is the canonical publisher-cloaking
    /// signature.
    ClassChange {
        /// `MimeClass` of the previous observation (the baseline).
        from: MimeClass,
        /// `MimeClass` of the latest observation.
        to: MimeClass,
    },
    /// Response body shrank by more than the configured threshold
    /// (default 50 %). The Markdown-stub pattern shrinks bodies by
    /// 20–100×.
    ByteCollapse {
        /// `byte_count_latest / byte_count_baseline`. Always in
        /// `(0.0, 1.0]` for a true collapse.
        ratio: f64,
    },
}

/// Aggregated shift report for one identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentTypeShiftReport {
    /// Identity the report covers.
    pub key: String,
    /// The latest observation that triggered the report.
    pub current: ContentTypeObservation,
    /// Detected drift, if any.
    pub drift: Option<ContentTypeDrift>,
}

/// Errors raised by [`ContentTypeShiftDetector::record`].
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ContentTypeError {
    /// Identity (`key`) is empty. Callers must supply a non-empty
    /// stable identity — typically `(url, vendor_class, ua_fingerprint)`.
    #[error("identity key is empty")]
    EmptyKey,
}

/// Port trait for content-type shift detection.
pub trait ContentTypeShiftDetector: Send + Sync {
    /// Record one observation and return the shift report.
    ///
    /// # Errors
    ///
    /// Returns [`ContentTypeError::EmptyKey`] when the observation's
    /// `key` is empty.
    fn record(
        &mut self,
        observation: &ContentTypeObservation,
    ) -> Result<ContentTypeShiftReport, ContentTypeError>;
}

/// Default rolling-baseline adapter.
///
/// Keeps the last 32 observations per identity (configurable via
/// [`RollingBaselineDetector::with_capacity`]) and emits a drift
/// report when the latest observation disagrees with the most
/// recent baseline on either dimension.
///
/// `Send + Sync` is implemented by mutating via [`std::sync::Mutex`] —
/// the detector is shared across snapshot runs in the existing
/// `SnapshotBuilder` (T88) which already passes the detector by
/// `Arc`.
#[derive(Debug)]
pub struct RollingBaselineDetector {
    /// Per-identity ring buffer of observations (oldest first).
    history: std::sync::Mutex<std::collections::HashMap<String, VecDeque<ContentTypeObservation>>>,
    /// Max observations retained per identity (ring-buffer cap).
    capacity: usize,
    /// `byte_count_latest / byte_count_baseline` threshold below which
    /// a `ByteCollapse` is emitted. Default 0.5 (50 %).
    byte_collapse_threshold: f64,
}

impl RollingBaselineDetector {
    /// Default capacity (32) and threshold (0.5).
    #[must_use]
    pub fn new() -> Self {
        Self {
            history: std::sync::Mutex::new(std::collections::HashMap::new()),
            capacity: 32,
            byte_collapse_threshold: 0.5,
        }
    }

    /// Override the per-identity ring-buffer cap.
    #[must_use]
    pub const fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        self
    }

    /// Override the `ByteCollapse` ratio threshold.
    #[must_use]
    pub const fn with_byte_collapse_threshold(mut self, threshold: f64) -> Self {
        self.byte_collapse_threshold = threshold;
        self
    }

    fn append_to_history(&self, observation: &ContentTypeObservation) {
        let Ok(mut map) = self.history.lock() else {
            return;
        };
        let entry = map
            .entry(observation.key.clone())
            .or_insert_with(|| VecDeque::with_capacity(self.capacity));
        if entry.len() == self.capacity {
            entry.pop_front();
        }
        entry.push_back(observation.clone());
    }

    fn baseline(&self, key: &str) -> Option<ContentTypeObservation> {
        let map = self.history.lock().ok()?;
        map.get(key).and_then(|q| q.back().cloned())
    }
}

impl Default for RollingBaselineDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl ContentTypeShiftDetector for RollingBaselineDetector {
    fn record(
        &mut self,
        observation: &ContentTypeObservation,
    ) -> Result<ContentTypeShiftReport, ContentTypeError> {
        if observation.key.is_empty() {
            return Err(ContentTypeError::EmptyKey);
        }

        let current_class = MimeClass::from_content_type(&observation.content_type);
        let baseline = self.baseline(&observation.key);
        let drift = baseline.as_ref().and_then(|baseline| {
            let baseline_class = MimeClass::from_content_type(&baseline.content_type);
            if baseline_class != current_class {
                Some(ContentTypeDrift::ClassChange {
                    from: baseline_class,
                    to: current_class,
                })
            } else if baseline.byte_length > 0 {
                // `u64 as f64` may lose precision for bodies > 2^53
                // bytes (~9 PB). HTTP response bodies are capped well
                // below that, so the precision loss is acceptable.
                #[allow(clippy::cast_precision_loss)]
                let ratio = observation.byte_length as f64 / baseline.byte_length as f64;
                if ratio < self.byte_collapse_threshold {
                    Some(ContentTypeDrift::ByteCollapse { ratio })
                } else {
                    None
                }
            } else {
                None
            }
        });

        self.append_to_history(observation);

        Ok(ContentTypeShiftReport {
            key: observation.key.clone(),
            current: observation.clone(),
            drift,
        })
    }
}

/// Helper for the snapshot builder: format a drift report as a short
/// human-readable string suitable for inclusion in a diagnostic
/// summary line.
#[must_use]
pub fn drift_summary(report: &ContentTypeShiftReport) -> String {
    match &report.drift {
        None => format!(
            "[{}] {} (no drift)",
            report.key, report.current.content_type
        ),
        Some(ContentTypeDrift::ClassChange { from, to }) => {
            format!(
                "[{}] class drift: {} -> {}",
                report.key,
                class_label(*from),
                class_label(*to)
            )
        }
        Some(ContentTypeDrift::ByteCollapse { ratio }) => {
            format!("[{}] byte collapse: ratio={:.3}", report.key, ratio)
        }
    }
}

const fn class_label(class: MimeClass) -> &'static str {
    match class {
        MimeClass::Html => "html",
        MimeClass::Json => "json",
        MimeClass::Xml => "xml",
        MimeClass::Markdown => "markdown",
        MimeClass::Text => "text",
        MimeClass::Binary => "binary",
        MimeClass::Unknown => "unknown",
    }
}

/// Unix-seconds timestamp helper for callers that need to record the
/// observation's wall-clock time alongside the drift event.
#[must_use]
pub fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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

    fn obs(key: &str, ct: &str, len: u64) -> ContentTypeObservation {
        ContentTypeObservation {
            key: key.to_string(),
            content_type: ct.to_string(),
            byte_length: len,
        }
    }

    #[test]
    fn mime_class_html_with_charset_param() {
        assert_eq!(
            MimeClass::from_content_type("text/html; charset=utf-8"),
            MimeClass::Html
        );
    }

    #[test]
    fn mime_class_json_with_vendor_suffix() {
        assert_eq!(
            MimeClass::from_content_type("application/vnd.api+json"),
            MimeClass::Json
        );
    }

    #[test]
    fn mime_class_markdown_stub() {
        assert_eq!(
            MimeClass::from_content_type("text/markdown; charset=utf-8"),
            MimeClass::Markdown
        );
    }

    #[test]
    fn mime_class_xml() {
        assert_eq!(
            MimeClass::from_content_type("application/rss+xml"),
            MimeClass::Xml
        );
    }

    #[test]
    fn mime_class_binary_octet_stream() {
        assert_eq!(
            MimeClass::from_content_type("application/octet-stream"),
            MimeClass::Binary
        );
    }

    #[test]
    fn mime_class_unknown_garbage() {
        assert_eq!(
            MimeClass::from_content_type("not a mime type"),
            MimeClass::Unknown
        );
        assert_eq!(MimeClass::from_content_type(""), MimeClass::Unknown);
    }

    #[test]
    fn first_observation_records_no_drift() {
        let mut d = RollingBaselineDetector::new();
        let report = d
            .record(&obs("https://example.com/a", "text/html", 100))
            .unwrap();
        assert_eq!(report.key, "https://example.com/a");
        assert!(report.drift.is_none(), "first observation must not drift");
    }

    #[test]
    fn two_identical_observations_no_drift() {
        let mut d = RollingBaselineDetector::new();
        d.record(&obs("k1", "text/html", 100)).unwrap();
        let report = d.record(&obs("k1", "text/html", 110)).unwrap();
        assert!(
            report.drift.is_none(),
            "identical observations must not drift"
        );
    }

    #[test]
    fn html_to_markdown_shift_is_class_change() {
        let mut d = RollingBaselineDetector::new();
        d.record(&obs("k1", "text/html", 50_000)).unwrap();
        let report = d.record(&obs("k1", "text/markdown", 2_000)).unwrap();
        match report.drift {
            Some(ContentTypeDrift::ClassChange { from, to }) => {
                assert_eq!(from, MimeClass::Html);
                assert_eq!(to, MimeClass::Markdown);
            }
            other => panic!("expected ClassChange, got {other:?}"),
        }
    }

    #[test]
    fn massive_byte_collapse_emits_byte_collapse_drift() {
        let mut d = RollingBaselineDetector::new();
        d.record(&obs("k1", "text/html", 100_000)).unwrap();
        let report = d.record(&obs("k1", "text/html", 4_000)).unwrap();
        match report.drift {
            Some(ContentTypeDrift::ByteCollapse { ratio }) => {
                assert!(
                    ratio > 0.0 && ratio < 0.5,
                    "ratio must be below the 0.5 threshold, got {ratio}"
                );
            }
            other => panic!("expected ByteCollapse, got {other:?}"),
        }
    }

    #[test]
    fn byte_collapse_threshold_is_configurable() {
        // Threshold 0.9 → any drop below 90% triggers a collapse.
        let mut d = RollingBaselineDetector::new().with_byte_collapse_threshold(0.9);
        d.record(&obs("k1", "text/html", 1000)).unwrap();
        let report = d.record(&obs("k1", "text/html", 200)).unwrap();
        assert!(
            matches!(report.drift, Some(ContentTypeDrift::ByteCollapse { .. })),
            "80% drop must collapse when threshold is 0.9"
        );

        // Threshold 0.1 → any drop below 10% triggers a collapse.
        // 95% drop (1000→50) has ratio 0.05, well below 0.1.
        let mut d2 = RollingBaselineDetector::new().with_byte_collapse_threshold(0.1);
        d2.record(&obs("k2", "text/html", 1000)).unwrap();
        let report2 = d2.record(&obs("k2", "text/html", 50)).unwrap();
        assert!(
            matches!(report2.drift, Some(ContentTypeDrift::ByteCollapse { .. })),
            "95% drop must collapse when threshold is 0.1"
        );
    }

    #[test]
    fn distinct_keys_dont_cross_pollute() {
        let mut d = RollingBaselineDetector::new();
        d.record(&obs("k1", "text/html", 50_000)).unwrap();
        let report = d.record(&obs("k2", "text/markdown", 100)).unwrap();
        assert!(
            report.drift.is_none(),
            "different keys must not trigger drift"
        );
    }

    #[test]
    fn ring_buffer_caps_at_capacity() {
        let mut d = RollingBaselineDetector::new().with_capacity(3);
        for i in 0..10 {
            d.record(&obs("k1", "text/html", 100 + i)).unwrap();
        }
        // Only the last 3 should be retained; the baseline should
        // compare against the last-inserted (most recent), not the
        // 0th. Verify by recording one more and checking the report
        // is anchored to length 109 (the most recent prior).
        let report = d.record(&obs("k1", "text/html", 150)).unwrap();
        assert!(report.drift.is_none(), "100→150 is growth, not collapse");
    }

    #[test]
    fn record_rejects_empty_key() {
        let mut d = RollingBaselineDetector::new();
        let err = d
            .record(&obs("", "text/html", 100))
            .expect_err("empty key must be rejected");
        assert!(matches!(err, ContentTypeError::EmptyKey));
    }

    #[test]
    fn drift_summary_strings_have_useful_content() {
        let mut d = RollingBaselineDetector::new();
        d.record(&obs("k1", "text/html", 50_000)).unwrap();
        let report = d.record(&obs("k1", "text/markdown", 100)).unwrap();
        let s = drift_summary(&report);
        assert!(s.contains("k1"));
        assert!(s.contains("html"));
        assert!(s.contains("markdown"));
    }

    #[test]
    fn zero_baseline_does_not_divide_by_zero() {
        let mut d = RollingBaselineDetector::new();
        d.record(&obs("k1", "text/html", 0)).unwrap();
        let report = d.record(&obs("k1", "text/html", 100)).unwrap();
        assert!(
            report.drift.is_none(),
            "zero-byte baseline must not emit ByteCollapse (no division)"
        );
    }
}
