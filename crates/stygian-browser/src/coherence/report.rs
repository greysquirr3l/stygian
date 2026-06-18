//! Cross-context coherence drift report schema.
//!
//! Defines the per-context identity surfaces, the `Skipped` marker for
//! unavailable contexts, and the drift-diagnostic records emitted by
//! [`crate::coherence::probes::CoherenceProbe`]. All comparison logic
//! in this file is **pure Rust with no I/O** so it can be unit-tested
//! deterministically without booting Chrome.
//!
//! ## Feature flag
//!
//! The coherence module is **default-on** and is always compiled as
//! part of the `stygian-browser` crate.
//!
//! ## Separation of hard failures vs known limitations
//!
//! Drift is classified into two severity bands:
//!
//! - [`DriftSeverity::Hard`] — user-agent, platform, languages, and
//!   `navigator.webdriver` MUST be identical across all contexts.
//!   Drift here is a strong anti-bot detection signal.
//! - [`DriftSeverity::KnownLimitation`] — fields like
//!   `hardwareConcurrency` and `deviceMemory` are documented to
//!   differ between Document and Worker contexts in some browsers.
//!   Drift here is a known limitation, not a stealth regression.
//!
//! Reports always carry both bands; callers can filter on
//! [`DriftSeverity`] to surface regressions only.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::freshness::FreshnessReport;

// ─── Context kind ──────────────────────────────────────────────────────────────

/// Logical browser context in which an identity surface was observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextKind {
    /// Top-level document (`window`).
    Top,
    /// Same-origin iframe (`iframe.contentWindow`).
    Iframe,
    /// Dedicated or shared worker (`WorkerGlobalScope`).
    Worker,
}

impl ContextKind {
    /// Stable snake-case label used in telemetry.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Iframe => "iframe",
            Self::Worker => "worker",
        }
    }
}

// ─── Identity surface ──────────────────────────────────────────────────────────

/// Identity surface probed in a single browser context.
///
/// All fields are optional so partial probe results (e.g. a context
/// where `navigator.deviceMemory` is undefined) round-trip cleanly.
///
/// # Example
///
/// ```
/// use stygian_browser::coherence::IdentitySurface;
///
/// let s = IdentitySurface {
///     user_agent: Some("Mozilla/5.0 ...".to_string()),
///     platform: Some("MacIntel".to_string()),
///     languages: Some("en-US,en".to_string()),
///     hardware_concurrency: Some(8),
///     device_memory: None,
///     timezone: Some("America/Los_Angeles".to_string()),
///     screen_width: Some(1920),
///     screen_height: Some(1080),
///     color_depth: Some(24),
///     webdriver: Some(false),
/// };
/// assert_eq!(s.user_agent.as_deref(), Some("Mozilla/5.0 ..."));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentitySurface {
    /// `navigator.userAgent`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// `navigator.platform`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// `navigator.languages` joined by `","` for stable comparisons.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub languages: Option<String>,
    /// `navigator.hardwareConcurrency` (allowed to drift for workers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hardware_concurrency: Option<u32>,
    /// `navigator.deviceMemory` (often undefined in workers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_memory: Option<u32>,
    /// `Intl.DateTimeFormat().resolvedOptions().timeZone`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// `screen.width` (Document + same-origin iframe only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen_width: Option<u32>,
    /// `screen.height` (Document + same-origin iframe only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen_height: Option<u32>,
    /// `screen.colorDepth` (Document + same-origin iframe only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_depth: Option<u32>,
    /// `navigator.webdriver` (`Some(false)` on a clean browser).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webdriver: Option<bool>,
}

impl IdentitySurface {
    /// `true` when **no** field is populated — useful for asserting a
    /// context was probed but produced no observations.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.user_agent.is_none()
            && self.platform.is_none()
            && self.languages.is_none()
            && self.hardware_concurrency.is_none()
            && self.device_memory.is_none()
            && self.timezone.is_none()
            && self.screen_width.is_none()
            && self.screen_height.is_none()
            && self.color_depth.is_none()
            && self.webdriver.is_none()
    }

    /// Build a deterministic signature for [`crate::freshness::signature_hash`].
    ///
    /// Concatenates the fields most likely to be anti-bot detection
    /// targets: `user_agent`, `platform`, `languages`, `timezone`,
    /// `screen_width`, `screen_height`, `color_depth`. Missing
    /// fields emit `"-"` so signatures are stable across partial
    /// observations.
    #[must_use]
    pub fn signature_parts(&self) -> Vec<String> {
        vec![
            self.user_agent.clone().unwrap_or_else(|| "-".to_string()),
            self.platform.clone().unwrap_or_else(|| "-".to_string()),
            self.languages.clone().unwrap_or_else(|| "-".to_string()),
            self.timezone.clone().unwrap_or_else(|| "-".to_string()),
            self.screen_width.map_or_else(|| "-".to_string(), |v| v.to_string()),
            self.screen_height.map_or_else(|| "-".to_string(), |v| v.to_string()),
            self.color_depth.map_or_else(|| "-".to_string(), |v| v.to_string()),
        ]
    }
}

// ─── Context observation ───────────────────────────────────────────────────────

/// Result of probing a single context: either an observed
/// [`IdentitySurface`] or a `Skipped` marker describing why the
/// probe could not run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ContextObservation {
    /// The context was probed and produced a (possibly partial)
    /// identity surface.
    Observed {
        /// Captured identity surface.
        surface: IdentitySurface,
    },
    /// The context could not be probed (worker unsupported, iframe
    /// creation blocked, etc.) — no panic, just a structured marker.
    Skipped {
        /// Human-readable reason for the skip.
        reason: String,
    },
}

impl ContextObservation {
    /// Convenience constructor for an [`Observed`][Self::Observed] surface.
    #[must_use]
    pub fn observed(surface: IdentitySurface) -> Self {
        Self::Observed { surface }
    }

    /// Convenience constructor for a [`Skipped`][Self::Skipped] marker.
    #[must_use]
    pub fn skipped(reason: impl Into<String>) -> Self {
        Self::Skipped {
            reason: reason.into(),
        }
    }

    /// `true` when the observation is an [`Observed`][Self::Observed]
    /// variant (regardless of how many fields were populated).
    #[must_use]
    pub const fn is_observed(&self) -> bool {
        matches!(self, Self::Observed { .. })
    }

    /// `true` when the observation is a [`Skipped`][Self::Skipped]
    /// marker.
    #[must_use]
    pub const fn is_skipped(&self) -> bool {
        matches!(self, Self::Skipped { .. })
    }

    /// Borrow the captured surface, when observed.
    #[must_use]
    pub fn surface(&self) -> Option<&IdentitySurface> {
        match self {
            Self::Observed { surface } => Some(surface),
            Self::Skipped { .. } => None,
        }
    }
}

// ─── Drift diagnostic ─────────────────────────────────────────────────────────

/// Drift severity classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftSeverity {
    /// Hard stealth regression — UA, platform, languages, webdriver
    /// should be identical across contexts.
    Hard,
    /// Known limitation — the field is documented to differ between
    /// Document and Worker contexts in some browser engines.
    KnownLimitation,
}

impl DriftSeverity {
    /// Stable snake-case label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hard => "hard",
            Self::KnownLimitation => "known_limitation",
        }
    }
}

/// Single drift record describing one field that disagreed across
/// two contexts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriftDiagnostic {
    /// First context in the comparison.
    pub context_a: ContextKind,
    /// Second context in the comparison.
    pub context_b: ContextKind,
    /// Field name (`snake_case`, e.g. `"user_agent"`).
    pub field: String,
    /// Observed value in `context_a` (rendered via `Display`).
    pub observed_a: String,
    /// Observed value in `context_b` (rendered via `Display`).
    pub observed_b: String,
    /// Severity classification.
    pub severity: DriftSeverity,
}

impl DriftDiagnostic {
    /// Stable, machine-readable reason tag (`"top:iframe:user_agent:hard"`).
    #[must_use]
    pub fn reason_tag(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.context_a.label(),
            self.context_b.label(),
            self.field,
            self.severity.label()
        )
    }
}

// ─── CoherenceDriftReport ──────────────────────────────────────────────────────

/// Aggregate coherence drift report covering top-level, iframe, and
/// (best-effort) worker contexts.
///
/// Skipped contexts never panic — they are emitted as
/// [`ContextObservation::Skipped`] markers so callers can attribute
/// missing coverage to the runtime, not a probe failure.
///
/// # Example
///
/// ```
/// use stygian_browser::coherence::{
///     CoherenceDriftReport, ContextObservation, ContextKind, IdentitySurface,
/// };
///
/// let s = IdentitySurface {
///     user_agent: Some("Mozilla/5.0 ...".to_string()),
///     platform: Some("MacIntel".to_string()),
///     languages: Some("en-US".to_string()),
///     ..IdentitySurface::default()
/// };
/// let report = CoherenceDriftReport {
///     top: ContextObservation::observed(s.clone()),
///     iframe: ContextObservation::observed(s.clone()),
///     worker: ContextObservation::skipped("Worker unsupported"),
///     drifts: Vec::new(),
///     freshness: None,
/// };
/// assert!(report.is_coherent());
/// assert!(!report.has_hard_drift());
/// assert_eq!(report.observed_context_count(), 2);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoherenceDriftReport {
    /// Top-level document observation.
    pub top: ContextObservation,
    /// Same-origin iframe observation.
    pub iframe: ContextObservation,
    /// Dedicated/shared worker observation (best-effort, may be `Skipped`).
    pub worker: ContextObservation,
    /// Drift diagnostics comparing all available context pairs.
    pub drifts: Vec<DriftDiagnostic>,
    /// Optional freshness report attached when the probe was
    /// supplied with a [`crate::freshness::FreshnessContract`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freshness: Option<FreshnessReport>,
}

impl CoherenceDriftReport {
    /// `true` when no drift diagnostics were emitted (i.e. all
    /// observed contexts are coherent). Skipped contexts are not
    /// counted as drift.
    #[must_use]
    pub fn is_coherent(&self) -> bool {
        self.drifts.is_empty()
    }

    /// `true` when at least one [`DriftSeverity::Hard`] drift is
    /// present. Callers should treat this as a stealth regression.
    #[must_use]
    pub fn has_hard_drift(&self) -> bool {
        self.drifts
            .iter()
            .any(|d| d.severity == DriftSeverity::Hard)
    }

    /// Number of contexts that were actually observed (not skipped).
    #[must_use]
    pub fn observed_context_count(&self) -> usize {
        [&self.top, &self.iframe, &self.worker]
            .iter()
            .filter(|o| o.is_observed())
            .count()
    }

    /// Number of contexts skipped.
    #[must_use]
    pub fn skipped_context_count(&self) -> usize {
        [&self.top, &self.iframe, &self.worker]
            .iter()
            .filter(|o| o.is_skipped())
            .count()
    }

    /// Iterate over all [`DriftSeverity::Hard`] diagnostics.
    pub fn hard_drifts(&self) -> impl Iterator<Item = &DriftDiagnostic> {
        self.drifts
            .iter()
            .filter(|d| d.severity == DriftSeverity::Hard)
    }

    /// Iterate over all [`DriftSeverity::KnownLimitation`] diagnostics.
    pub fn known_limitations(&self) -> impl Iterator<Item = &DriftDiagnostic> {
        self.drifts
            .iter()
            .filter(|d| d.severity == DriftSeverity::KnownLimitation)
    }
}

// ─── Comparison helpers (pure Rust, no I/O) ────────────────────────────────────

/// Pair of contexts to compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContextPair {
    /// Top vs Iframe.
    TopIframe,
    /// Top vs Worker.
    TopWorker,
    /// Iframe vs Worker.
    IframeWorker,
}

impl ContextPair {
    /// All three pairs in deterministic order.
    pub const ALL: [ContextPair; 3] = [
        ContextPair::TopIframe,
        ContextPair::TopWorker,
        ContextPair::IframeWorker,
    ];

    /// Resolve the [`ContextKind`] for side `a` / `b`.
    #[must_use]
    pub const fn sides(self) -> (ContextKind, ContextKind) {
        match self {
            Self::TopIframe => (ContextKind::Top, ContextKind::Iframe),
            Self::TopWorker => (ContextKind::Top, ContextKind::Worker),
            Self::IframeWorker => (ContextKind::Iframe, ContextKind::Worker),
        }
    }
}

/// Fields that MUST be identical across all contexts.
const HARD_FIELDS: &[&str] = &["user_agent", "platform", "languages", "webdriver"];

/// Fields where drift is a known browser-engine limitation, not a
/// stealth regression.
#[allow(dead_code)]
const KNOWN_LIMITATION_FIELDS: &[&str] = &[
    "hardware_concurrency",
    "device_memory",
    "screen_width",
    "screen_height",
    "color_depth",
    "timezone",
];

/// Severity for a given field name.
#[must_use]
pub fn field_severity(field: &str) -> DriftSeverity {
    if HARD_FIELDS.contains(&field) {
        DriftSeverity::Hard
    } else {
        DriftSeverity::KnownLimitation
    }
}

/// Compare `a` and `b` (both `Observed`) and emit a
/// [`Vec<DriftDiagnostic>`] covering every disagreed field. Returns
/// an empty vec when `a == b`.
///
/// `pair` is recorded into each diagnostic so the report can
/// attribute drift to the right context pair.
#[must_use]
pub fn diff_surfaces(
    pair: ContextPair,
    a: &IdentitySurface,
    b: &IdentitySurface,
) -> Vec<DriftDiagnostic> {
    let (kind_a, kind_b) = pair.sides();
    let mut drifts = Vec::new();

    let pairs: [(&str, Option<String>, Option<String>); 10] = [
        (
            "user_agent",
            a.user_agent.clone(),
            b.user_agent.clone(),
        ),
        ("platform", a.platform.clone(), b.platform.clone()),
        ("languages", a.languages.clone(), b.languages.clone()),
        (
            "hardware_concurrency",
            a.hardware_concurrency.map(|v| v.to_string()),
            b.hardware_concurrency.map(|v| v.to_string()),
        ),
        (
            "device_memory",
            a.device_memory.map(|v| v.to_string()),
            b.device_memory.map(|v| v.to_string()),
        ),
        ("timezone", a.timezone.clone(), b.timezone.clone()),
        (
            "screen_width",
            a.screen_width.map(|v| v.to_string()),
            b.screen_width.map(|v| v.to_string()),
        ),
        (
            "screen_height",
            a.screen_height.map(|v| v.to_string()),
            b.screen_height.map(|v| v.to_string()),
        ),
        (
            "color_depth",
            a.color_depth.map(|v| v.to_string()),
            b.color_depth.map(|v| v.to_string()),
        ),
        (
            "webdriver",
            a.webdriver.map(|v| v.to_string()),
            b.webdriver.map(|v| v.to_string()),
        ),
    ];

    for (field, va, vb) in pairs {
        if va == vb {
            continue;
        }
        // Treat (Some, None) and (None, Some) as drift too — a
        // context that cannot see a field is a meaningful signal
        // when the other context can.
        let observed_a = va.unwrap_or_else(|| "<absent>".to_string());
        let observed_b = vb.unwrap_or_else(|| "<absent>".to_string());
        drifts.push(DriftDiagnostic {
            context_a: kind_a,
            context_b: kind_b,
            field: field.to_string(),
            observed_a,
            observed_b,
            severity: field_severity(field),
        });
    }

    drifts
}

/// Build a [`CoherenceDriftReport`] from three [`ContextObservation`]s
/// by running [`diff_surfaces`] for every applicable pair.
///
/// Skipped contexts are excluded from the comparison without
/// raising an error or panic.
#[must_use]
pub fn build_report(
    top: ContextObservation,
    iframe: ContextObservation,
    worker: ContextObservation,
    freshness: Option<FreshnessReport>,
) -> CoherenceDriftReport {
    let mut drifts = Vec::new();
    let observed = [
        (ContextKind::Top, &top),
        (ContextKind::Iframe, &iframe),
        (ContextKind::Worker, &worker),
    ];

    for pair in ContextPair::ALL {
        let (ka, kb) = pair.sides();
        let surface_a = observed
            .iter()
            .find(|(k, _)| *k == ka)
            .and_then(|(_, o)| o.surface());
        let surface_b = observed
            .iter()
            .find(|(k, _)| *k == kb)
            .and_then(|(_, o)| o.surface());
        if let (Some(sa), Some(sb)) = (surface_a, surface_b) {
            drifts.extend(diff_surfaces(pair, sa, sb));
        }
    }

    CoherenceDriftReport {
        top,
        iframe,
        worker,
        drifts,
        freshness,
    }
}

// ─── Signature helper ─────────────────────────────────────────────────────────

/// Compute a deterministic signature hash from an [`IdentitySurface`].
///
/// Wraps [`crate::freshness::signature_hash`] so the same input always
/// produces the same `"fnv64:<hex>"` string. Useful for cross-context
/// signatures that can then be compared against a
/// [`crate::freshness::FreshnessContract`].
///
/// # Example
///
/// ```
/// use stygian_browser::coherence::{IdentitySurface, surface_signature};
///
/// let s = IdentitySurface {
///     user_agent: Some("Mozilla/5.0 ...".to_string()),
///     platform: Some("MacIntel".to_string()),
///     ..IdentitySurface::default()
/// };
/// let h1 = surface_signature(&s);
/// let h2 = surface_signature(&s);
/// assert_eq!(h1, h2);
/// assert!(h1.starts_with("fnv64:"));
/// ```
#[must_use]
pub fn surface_signature(surface: &IdentitySurface) -> String {
    let parts = surface.signature_parts();
    let borrowed: Vec<&str> = parts.iter().map(String::as_str).collect();
    crate::freshness::signature_hash(&borrowed)
}

/// Fields that contribute to [`surface_signature`]. Exposed for
/// callers that need to extend the contract.
#[must_use]
pub fn signature_field_names() -> &'static BTreeSet<&'static str> {
    // Lazily-initialised static keeps the helper callable from
    // `const` contexts in callers without a const-friendly BTreeMap
    // literal in the source.
    static NAMES: std::sync::OnceLock<BTreeSet<&'static str>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        [
            "user_agent",
            "platform",
            "languages",
            "timezone",
            "screen_width",
            "screen_height",
            "color_depth",
        ]
        .into_iter()
        .collect()
    })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::freshness::{DomainClass, FreshnessCheckInput, FreshnessContract, FreshnessPolicyKind};
    use std::time::Duration;

    fn surface_a() -> IdentitySurface {
        IdentitySurface {
            user_agent: Some("Mozilla/5.0".to_string()),
            platform: Some("MacIntel".to_string()),
            languages: Some("en-US,en".to_string()),
            hardware_concurrency: Some(8),
            device_memory: Some(8),
            timezone: Some("America/Los_Angeles".to_string()),
            screen_width: Some(1920),
            screen_height: Some(1080),
            color_depth: Some(24),
            webdriver: Some(false),
        }
    }

    fn surface_b_drift_ua_platform() -> IdentitySurface {
        IdentitySurface {
            user_agent: Some("Mozilla/4.0".to_string()),
            platform: Some("Win32".to_string()),
            languages: Some("en-US,en".to_string()),
            hardware_concurrency: Some(8),
            device_memory: Some(8),
            timezone: Some("America/Los_Angeles".to_string()),
            screen_width: Some(1920),
            screen_height: Some(1080),
            color_depth: Some(24),
            webdriver: Some(false),
        }
    }

    fn surface_worker_differs_in_hardware() -> IdentitySurface {
        IdentitySurface {
            user_agent: Some("Mozilla/5.0".to_string()),
            platform: Some("MacIntel".to_string()),
            languages: Some("en-US,en".to_string()),
            hardware_concurrency: Some(2), // documented worker limitation
            device_memory: None,           // workers lack deviceMemory
            timezone: Some("America/Los_Angeles".to_string()),
            screen_width: None, // workers have no screen
            screen_height: None,
            color_depth: None,
            webdriver: Some(false),
        }
    }

    #[test]
    fn diff_surfaces_empty_when_identical() {
        let a = surface_a();
        let b = a.clone();
        let drifts = diff_surfaces(ContextPair::TopIframe, &a, &b);
        assert!(drifts.is_empty());
    }

    #[test]
    fn diff_surfaces_emits_hard_drift_for_ua_and_platform() {
        let a = surface_a();
        let b = surface_b_drift_ua_platform();
        let drifts = diff_surfaces(ContextPair::TopIframe, &a, &b);
        let fields: Vec<&str> = drifts.iter().map(|d| d.field.as_str()).collect();
        assert!(fields.contains(&"user_agent"));
        assert!(fields.contains(&"platform"));
        assert!(!fields.contains(&"languages"));
        // All emitted drifts are hard
        for d in &drifts {
            assert_eq!(d.severity, DriftSeverity::Hard);
            assert_eq!(d.context_a, ContextKind::Top);
            assert_eq!(d.context_b, ContextKind::Iframe);
        }
    }

    #[test]
    fn diff_surfaces_classifies_worker_hardware_drift_as_known_limitation() {
        let a = surface_a();
        let b = surface_worker_differs_in_hardware();
        let drifts = diff_surfaces(ContextPair::TopWorker, &a, &b);
        let hardware: Vec<&DriftDiagnostic> = drifts
            .iter()
            .filter(|d| d.field == "hardware_concurrency")
            .collect();
        assert_eq!(hardware.len(), 1);
        assert_eq!(hardware[0].severity, DriftSeverity::KnownLimitation);

        let device_memory: Vec<&DriftDiagnostic> = drifts
            .iter()
            .filter(|d| d.field == "device_memory")
            .collect();
        assert_eq!(device_memory.len(), 1);
        assert_eq!(device_memory[0].severity, DriftSeverity::KnownLimitation);
        // "<absent>" rendered for the missing field
        assert_eq!(device_memory[0].observed_b, "<absent>");
    }

    #[test]
    fn build_report_skips_unavailable_contexts_without_panic() {
        let top = ContextObservation::observed(surface_a());
        let iframe = ContextObservation::skipped("iframe blocked by CSP");
        let worker = ContextObservation::skipped("Worker unsupported");
        let report = build_report(top, iframe, worker, None);
        // No top↔iframe, top↔worker, iframe↔worker comparisons possible
        assert!(report.drifts.is_empty());
        assert_eq!(report.observed_context_count(), 1);
        assert_eq!(report.skipped_context_count(), 2);
        assert!(report.is_coherent());
        assert!(!report.has_hard_drift());
    }

    #[test]
    fn build_report_flags_hard_drift_between_top_and_iframe() {
        let top = ContextObservation::observed(surface_a());
        let iframe = ContextObservation::observed(surface_b_drift_ua_platform());
        let worker = ContextObservation::skipped("Worker unsupported");
        let report = build_report(top, iframe, worker, None);
        assert!(!report.is_coherent());
        assert!(report.has_hard_drift());
        let hard_count = report.hard_drifts().count();
        assert!(hard_count >= 2); // UA + platform
    }

    #[test]
    fn build_report_flags_only_known_limitations_for_worker_drift() {
        let top = ContextObservation::observed(surface_a());
        let iframe = ContextObservation::observed(surface_a());
        let worker = ContextObservation::observed(surface_worker_differs_in_hardware());
        let report = build_report(top, iframe, worker, None);
        // Top↔iframe must be clean
        let top_iframe_drift: Vec<&DriftDiagnostic> = report
            .drifts
            .iter()
            .filter(|d| {
                d.context_a == ContextKind::Top && d.context_b == ContextKind::Iframe
            })
            .collect();
        assert!(top_iframe_drift.is_empty());
        // Top↔worker + Iframe↔worker must contain only known-limitation
        // diagnostics
        for d in &report.drifts {
            assert_eq!(d.severity, DriftSeverity::KnownLimitation);
        }
        assert!(!report.has_hard_drift());
        assert!(report.known_limitations().count() > 0);
    }

    #[test]
    fn surface_signature_is_deterministic_and_starts_with_fnv64() {
        let a = surface_a();
        let h1 = surface_signature(&a);
        let h2 = surface_signature(&a);
        assert_eq!(h1, h2);
        assert!(h1.starts_with("fnv64:"));
    }

    #[test]
    fn surface_signature_changes_with_user_agent() {
        let a = surface_a();
        let mut b = a.clone();
        b.user_agent = Some("Mozilla/4.0".to_string());
        assert_ne!(surface_signature(&a), surface_signature(&b));
    }

    #[test]
    fn report_carries_freshness_when_supplied() {
        let contract = FreshnessContract::with_signature(
            "example.com",
            surface_signature(&surface_a()).as_str(),
            1_700_000_000_000,
            Duration::from_mins(1),
            FreshnessPolicyKind::Standard,
        )
        .expect("contract");
        let input = FreshnessCheckInput::new(
            "example.com",
            Some(surface_signature(&surface_a()).as_str()),
            1_700_000_030_000,
        );
        let report = build_report(
            ContextObservation::observed(surface_a()),
            ContextObservation::observed(surface_a()),
            ContextObservation::skipped("Worker unsupported"),
            Some(FreshnessReport::evaluate(&contract, &input)),
        );
        let fr = report
            .freshness
            .as_ref()
            .expect("freshness report attached");
        assert!(fr.decision.is_valid());
        assert_eq!(fr.domain_class, DomainClass::Default);
    }

    #[test]
    fn drift_reason_tag_is_stable() {
        let d = DriftDiagnostic {
            context_a: ContextKind::Top,
            context_b: ContextKind::Worker,
            field: "user_agent".to_string(),
            observed_a: "a".to_string(),
            observed_b: "b".to_string(),
            severity: DriftSeverity::Hard,
        };
        assert_eq!(d.reason_tag(), "top:worker:user_agent:hard");
    }

    #[test]
    fn context_kind_label_is_stable() {
        assert_eq!(ContextKind::Top.label(), "top");
        assert_eq!(ContextKind::Iframe.label(), "iframe");
        assert_eq!(ContextKind::Worker.label(), "worker");
    }

    #[test]
    fn context_observation_accessors() {
        let o = ContextObservation::observed(IdentitySurface::default());
        assert!(o.is_observed());
        assert!(!o.is_skipped());
        assert!(o.surface().is_some());

        let s = ContextObservation::skipped("nope");
        assert!(s.is_skipped());
        assert!(!s.is_observed());
        assert!(s.surface().is_none());
    }

    #[test]
    fn empty_surface_reports_empty() {
        let s = IdentitySurface::default();
        assert!(s.is_empty());
        let full = surface_a();
        assert!(!full.is_empty());
    }

    #[test]
    fn json_roundtrip_preserves_report() {
        let report = build_report(
            ContextObservation::observed(surface_a()),
            ContextObservation::observed(surface_a()),
            ContextObservation::skipped("Worker unsupported"),
            None,
        );
        let json = serde_json::to_string(&report).expect("serialize");
        let back: CoherenceDriftReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(report, back);
    }
}
