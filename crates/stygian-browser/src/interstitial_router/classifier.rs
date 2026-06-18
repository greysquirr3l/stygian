//! Pure deterministic interstitial classifier.
//!
//! Consumes a [`PageSignature`] and returns the matching
//! [`InterstitialKind`]. The classifier is a finite cascade
//! of structural rules:
//!
//! 1. **Hard block** — terminal block markers in the body
//!    or status, or a URL pointing at a known block
//!    endpoint.
//! 2. **Challenge** — vendor-issued challenge markers in
//!    the body, URL, or headers (Cloudflare `cf-chl-bypass`,
//!    hCaptcha, reCAPTCHA, Akamai `_abck`, `PerimeterX`
//!    `_px`, etc.).
//! 3. **Queue** — "please wait" / waiting-room markers in
//!    the body, an explicit queue position hint, or a
//!    202/302 with queue markers.
//! 4. **Transient** — `3xx` redirect with no queue/challenge
//!    markers.
//! 5. **Default: `Transient`** — unclassified signatures
//!    fall through to the transient (generic retry) bucket
//!    so the runner can take the normal ladder without
//!    penalising unrecognised pages.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::policy::InterstitialKind;

/// Page signature consumed by the [`InterstitialClassifier`].
///
/// The signature is the **observation** that a previous
/// acquisition attempt produced. Callers attach the
/// signature to an [`AcquisitionRequest`][crate::acquisition::AcquisitionRequest]
/// via the
/// [`AcquisitionRequest::interstitial`][crate::acquisition::AcquisitionRequest::interstitial]
/// field (see `mod.rs` for the runner integration).
///
/// # Example
///
/// ```
/// use stygian_browser::interstitial_router::PageSignature;
///
/// let signature = PageSignature::new(
///     "https://example.com/cdn-cgi/challenge-platform/h/b",
///     Some(403),
/// )
/// .with_body_marker("cf-chl-bypass")
/// .with_header("cf-mitigated");
/// assert_eq!(signature.body_markers.len(), 1);
/// assert_eq!(signature.header_set.len(), 1);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageSignature {
    /// Target URL of the page.
    pub url: String,
    /// HTTP status code, when known.
    pub status_code: Option<u16>,
    /// Body substrings (case-insensitive) observed in the
    /// page. Markers are normalised to lower-case ASCII.
    pub body_markers: Vec<String>,
    /// Lower-case ASCII header names observed in the
    /// response.
    pub header_set: Vec<String>,
    /// Optional redirect target for a 3xx response.
    pub redirect_url: Option<String>,
    /// Optional queue position hint (1-based).
    pub queue_position_hint: Option<u32>,
    /// Optional vendor hint (e.g. `cloudflare`,
    /// `akamai`).
    pub vendor_hint: Option<String>,
}

impl PageSignature {
    /// Build a signature with the supplied `url` and
    /// `status_code` and no other fields set.
    #[must_use]
    pub fn new(url: impl Into<String>, status_code: Option<u16>) -> Self {
        Self {
            url: url.into(),
            status_code,
            body_markers: Vec::new(),
            header_set: Vec::new(),
            redirect_url: None,
            queue_position_hint: None,
            vendor_hint: None,
        }
    }

    /// Builder: add a body marker (case-insensitive). The
    /// marker is trimmed and lower-cased; empty markers are
    /// ignored.
    #[must_use]
    pub fn with_body_marker(mut self, marker: impl Into<String>) -> Self {
        let marker = marker.into().trim().to_ascii_lowercase();
        if !marker.is_empty() && !self.body_markers.iter().any(|m| m == &marker) {
            self.body_markers.push(marker);
        }
        self
    }

    /// Builder: add a header name (case-insensitive). The
    /// name is trimmed and lower-cased; empty names are
    /// ignored.
    #[must_use]
    pub fn with_header(mut self, header: impl Into<String>) -> Self {
        let header = header.into().trim().to_ascii_lowercase();
        if !header.is_empty() && !self.header_set.iter().any(|h| h == &header) {
            self.header_set.push(header);
        }
        self
    }

    /// Builder: set the redirect target.
    #[must_use]
    pub fn with_redirect_url(mut self, redirect_url: impl Into<String>) -> Self {
        self.redirect_url = Some(redirect_url.into());
        self
    }

    /// Builder: set the queue position hint.
    #[must_use]
    pub fn with_queue_position(mut self, position: u32) -> Self {
        self.queue_position_hint = Some(position);
        self
    }

    /// Builder: set the vendor hint.
    #[must_use]
    pub fn with_vendor_hint(mut self, vendor: impl Into<String>) -> Self {
        self.vendor_hint = Some(vendor.into());
        self
    }

    /// Builder: replace the body marker set.
    #[must_use]
    pub fn with_body_markers(mut self, markers: Vec<String>) -> Self {
        self.body_markers = markers;
        self
    }

    /// Builder: replace the header set.
    #[must_use]
    pub fn with_header_set(mut self, headers: Vec<String>) -> Self {
        self.header_set = headers;
        self
    }

    /// Lower-case ASCII view of the URL host, when
    /// parseable. Returns `None` when the URL is empty or
    /// malformed.
    #[must_use]
    pub fn host(&self) -> Option<String> {
        let url = self.url.trim();
        if url.is_empty() {
            return None;
        }
        let without_scheme = url.split_once("://")?.1;
        let authority = without_scheme.split('/').next()?;
        let host = authority.rsplit('@').next()?.split(':').next()?;
        if host.is_empty() {
            None
        } else {
            Some(host.to_ascii_lowercase())
        }
    }

    /// `true` when the URL path (or query) contains the
    /// given lower-case substring.
    #[must_use]
    pub fn url_contains(&self, needle_lower: &str) -> bool {
        self.url.to_ascii_lowercase().contains(needle_lower)
    }

    /// `true` when any of the body markers contain the
    /// given lower-case substring.
    #[must_use]
    pub fn body_contains(&self, needle_lower: &str) -> bool {
        self.body_markers
            .iter()
            .any(|m| m.to_ascii_lowercase().contains(needle_lower))
    }

    /// `true` when the header set contains the given
    /// lower-case header name.
    #[must_use]
    pub fn has_header(&self, name_lower: &str) -> bool {
        self.header_set
            .iter()
            .any(|h| h.eq_ignore_ascii_case(name_lower))
    }

    /// Lower-case unique header set (for diagnostics).
    #[must_use]
    pub fn unique_headers(&self) -> BTreeSet<String> {
        self.header_set
            .iter()
            .map(|h| h.to_ascii_lowercase())
            .collect()
    }
}

/// Deterministic interstitial classifier.
///
/// The classifier is a pure function
/// `&PageSignature -> InterstitialKind`. It performs no I/O
/// and reads no clock — it can be unit-tested across the
/// full rule matrix without booting Chrome.
///
/// # Example
///
/// ```
/// use stygian_browser::interstitial_router::{
///     InterstitialClassifier, InterstitialKind, PageSignature,
/// };
///
/// let classifier = InterstitialClassifier::new();
/// let sig = PageSignature::new("https://example.com/queue", Some(200))
///     .with_body_marker("please wait");
/// assert_eq!(classifier.classify(&sig), InterstitialKind::Queue);
/// ```
#[derive(Debug, Clone, Default)]
pub struct InterstitialClassifier {
    _private: (),
}

impl InterstitialClassifier {
    /// Build a default classifier.
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }

    /// Classify `signature` into an [`InterstitialKind`].
    ///
    /// The function is a finite cascade: hard block →
    /// challenge → queue → transient → transient
    /// (default). The first rule that matches wins. The
    /// rules are documented in the module-level doc.
    #[must_use]
    pub fn classify(&self, signature: &PageSignature) -> InterstitialKind {
        // 1. Hard block.
        if is_hard_block(signature) {
            return InterstitialKind::HardBlock;
        }

        // 2. Challenge.
        if is_challenge(signature) {
            return InterstitialKind::Challenge;
        }

        // 3. Queue.
        if is_queue(signature) {
            return InterstitialKind::Queue;
        }

        // 4. Transient: any 3xx with no body markers, or
        //    a URL that looks like a redirect.
        if is_transient(signature) {
            return InterstitialKind::Transient;
        }

        // 5. Default: transient (most permissive — runner
        //    falls through to the normal ladder).
        InterstitialKind::Transient
    }
}

fn is_hard_block(signature: &PageSignature) -> bool {
    // Status code 403 + block markers, or 503, or a known
    // block URL pattern.
    if matches!(signature.status_code, Some(403 | 503)) {
        // 403 alone isn't a hard block — only when paired
        // with a block marker, a block URL, or a hard
        // block vendor hint.
        if HARD_BLOCK_URL_PATTERNS
            .iter()
            .any(|p| signature.url_contains(p))
        {
            return true;
        }
        if HARD_BLOCK_BODY_MARKERS.iter().any(|m| signature.body_contains(m)) {
            return true;
        }
        if signature.vendor_hint.as_deref().is_some_and(is_hard_block_vendor) {
            return true;
        }
    }

    // 429 with a block body is also a hard block.
    if matches!(signature.status_code, Some(429))
        && HARD_BLOCK_BODY_MARKERS.iter().any(|m| signature.body_contains(m))
    {
        return true;
    }

    // URL-only pattern (no status known).
    if signature.status_code.is_none()
        && HARD_BLOCK_URL_PATTERNS
            .iter()
            .any(|p| signature.url_contains(p))
    {
        return true;
    }

    false
}

fn is_challenge(signature: &PageSignature) -> bool {
    if CHALLENGE_BODY_MARKERS.iter().any(|m| signature.body_contains(m)) {
        return true;
    }
    if CHALLENGE_URL_PATTERNS
        .iter()
        .any(|p| signature.url_contains(p))
    {
        return true;
    }
    if CHALLENGE_HEADERS.iter().any(|h| signature.has_header(h)) {
        return true;
    }
    signature
        .vendor_hint
        .as_deref()
        .is_some_and(is_challenge_vendor)
}

fn is_queue(signature: &PageSignature) -> bool {
    if signature.queue_position_hint.is_some() {
        return true;
    }
    if QUEUE_BODY_MARKERS.iter().any(|m| signature.body_contains(m)) {
        return true;
    }
    if QUEUE_URL_PATTERNS
        .iter()
        .any(|p| signature.url_contains(p))
    {
        return true;
    }
    if matches!(signature.status_code, Some(202)) {
        return true;
    }
    false
}

fn is_transient(signature: &PageSignature) -> bool {
    matches!(signature.status_code, Some(301 | 302 | 303 | 307 | 308))
        || signature.redirect_url.is_some()
        || signature.url_contains("/redirect")
        || signature.url_contains("/continue")
}

pub(super) const HARD_BLOCK_BODY_MARKERS_PUBLIC: &[&str] = &[
    "access denied",
    "request blocked",
    "you have been blocked",
    "we have detected unusual traffic",
    "this site has been blocked",
    "your request has been denied",
    "forbidden",
];

const HARD_BLOCK_BODY_MARKERS: &[&str] = HARD_BLOCK_BODY_MARKERS_PUBLIC;

pub(super) const HARD_BLOCK_URL_PATTERNS_PUBLIC: &[&str] = &[
    "/blocked",
    "/forbidden",
    "/denied",
    "/err/blocked",
    "/err/forbidden",
    "/banned",
];

const HARD_BLOCK_URL_PATTERNS: &[&str] = HARD_BLOCK_URL_PATTERNS_PUBLIC;

pub(super) const HARD_BLOCK_VENDOR_HINTS_PUBLIC: &[&str] = &["blacklist", "firewall-block"];

const HARD_BLOCK_VENDOR_HINTS: &[&str] = HARD_BLOCK_VENDOR_HINTS_PUBLIC;

fn is_hard_block_vendor(vendor: &str) -> bool {
    HARD_BLOCK_VENDOR_HINTS
        .iter()
        .any(|h| vendor.eq_ignore_ascii_case(h))
}

pub(super) const CHALLENGE_BODY_MARKERS_PUBLIC: &[&str] = &[
    "cf-chl-bypass",
    "cf-challenge",
    "cf-turnstile",
    "challenge-platform",
    "checking your browser",
    "just a moment",
    "g-recaptcha",
    "h-captcha",
    "hcaptcha",
    "arkose",
    "perimeterx",
    "perimeter x",
    "press & hold",
    "press and hold",
    "akamai bot manager",
    "akamai_bm",
    "fingerprint.com",
    "shape security",
    "kasada",
    "datadome",
    "px-captcha",
    "_abck",
];

const CHALLENGE_BODY_MARKERS: &[&str] = CHALLENGE_BODY_MARKERS_PUBLIC;

pub(super) const CHALLENGE_URL_PATTERNS_PUBLIC: &[&str] = &[
    "/cdn-cgi/challenge-platform",
    "/cdn-cgi/challenge",
    "/challenge-platform",
    "/_px/",
    "/_abck",
    "/captcha",
    "/__challenge",
    "/arkose",
    "/px/validate",
    "/fingerprint",
    "/datadome",
];

const CHALLENGE_URL_PATTERNS: &[&str] = CHALLENGE_URL_PATTERNS_PUBLIC;

pub(super) const CHALLENGE_HEADERS_PUBLIC: &[&str] = &[
    "cf-mitigated",
    "cf-chl-bypass",
    "x-captcha",
    "x-akamai-bot",
    "x-datadome",
    "x-perimeterx",
];

const CHALLENGE_HEADERS: &[&str] = CHALLENGE_HEADERS_PUBLIC;

pub(super) const CHALLENGE_VENDOR_HINTS_PUBLIC: &[&str] = &[
    "cloudflare",
    "akamai",
    "akamai_bot_manager",
    "perimeterx",
    "perimeter_x",
    "datadome",
    "shape_security",
    "kasada",
    "fingerprint_com",
    "fingerprintcom",
    "hcaptcha",
    "recaptcha",
];

const CHALLENGE_VENDOR_HINTS: &[&str] = CHALLENGE_VENDOR_HINTS_PUBLIC;

fn is_challenge_vendor(vendor: &str) -> bool {
    CHALLENGE_VENDOR_HINTS
        .iter()
        .any(|h| vendor.eq_ignore_ascii_case(h))
}

pub(super) const QUEUE_BODY_MARKERS_PUBLIC: &[&str] = &[
    "please wait",
    "you are in line",
    "queue position",
    "your place in line",
    "estimated wait",
    "waiting room",
    "one moment please",
    "almost done",
];

const QUEUE_BODY_MARKERS: &[&str] = QUEUE_BODY_MARKERS_PUBLIC;

pub(super) const QUEUE_URL_PATTERNS_PUBLIC: &[&str] = &["/queue", "/waiting", "/wait-room", "/waitroom"];

const QUEUE_URL_PATTERNS: &[&str] = QUEUE_URL_PATTERNS_PUBLIC;

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifier_identifies_queue_via_body_marker() {
        let classifier = InterstitialClassifier::new();
        let sig = PageSignature::new("https://example.com/queue", Some(200))
            .with_body_marker("please wait")
            .with_queue_position(3);
        assert_eq!(classifier.classify(&sig), InterstitialKind::Queue);
    }

    #[test]
    fn classifier_identifies_challenge_via_captcha_marker() {
        let classifier = InterstitialClassifier::new();
        let sig = PageSignature::new(
            "https://example.com/cdn-cgi/challenge-platform/h/b",
            Some(403),
        )
        .with_body_marker("cf-chl-bypass")
        .with_header("cf-mitigated")
        .with_vendor_hint("cloudflare");
        assert_eq!(classifier.classify(&sig), InterstitialKind::Challenge);
    }

    #[test]
    fn classifier_identifies_hard_block_via_status_and_marker() {
        let classifier = InterstitialClassifier::new();
        let sig = PageSignature::new("https://example.com/blocked", Some(403))
            .with_body_marker("access denied");
        assert_eq!(classifier.classify(&sig), InterstitialKind::HardBlock);
    }

    #[test]
    fn classifier_identifies_transient_via_3xx_redirect() {
        let classifier = InterstitialClassifier::new();
        let sig = PageSignature::new("https://example.com/redirect", Some(302));
        assert_eq!(classifier.classify(&sig), InterstitialKind::Transient);
    }

    #[test]
    fn classifier_default_unclassified_is_transient() {
        let classifier = InterstitialClassifier::new();
        let sig = PageSignature::new("https://example.com/some-page", Some(200));
        assert_eq!(classifier.classify(&sig), InterstitialKind::Transient);
    }

    #[test]
    fn classifier_is_deterministic_for_identical_signatures() {
        let classifier = InterstitialClassifier::new();
        let sig = PageSignature::new("https://example.com/blocked", Some(403))
            .with_body_marker("access denied");
        let a = classifier.classify(&sig);
        let b = classifier.classify(&sig);
        assert_eq!(a, b);
        assert_eq!(a, InterstitialKind::HardBlock);
    }

    #[test]
    fn classifier_precedence_hard_block_wins_over_challenge() {
        let classifier = InterstitialClassifier::new();
        // A signature that has BOTH hard-block and challenge markers
        // must classify as hard block (higher precedence).
        let sig = PageSignature::new("https://example.com/blocked", Some(403))
            .with_body_marker("access denied")
            .with_body_marker("cf-chl-bypass");
        assert_eq!(classifier.classify(&sig), InterstitialKind::HardBlock);
    }

    #[test]
    fn classifier_precedence_challenge_wins_over_queue() {
        let classifier = InterstitialClassifier::new();
        // Both challenge AND queue markers: challenge wins.
        let sig = PageSignature::new(
            "https://example.com/cdn-cgi/challenge-platform/h/b",
            Some(403),
        )
        .with_body_marker("cf-chl-bypass")
        .with_body_marker("please wait");
        assert_eq!(classifier.classify(&sig), InterstitialKind::Challenge);
    }

    #[test]
    fn page_signature_builder_dedupes_markers() {
        let sig = PageSignature::new("https://example.com", None)
            .with_body_marker("please wait")
            .with_body_marker("please wait")
            .with_body_marker("Please Wait")
            .with_header("x-foo")
            .with_header("X-Foo");
        assert_eq!(sig.body_markers.len(), 1);
        assert_eq!(sig.header_set.len(), 1);
    }

    #[test]
    fn page_signature_host_extracts_lowercase_authority() {
        let sig = PageSignature::new("https://User:Pass@Example.COM:8443/path", None);
        assert_eq!(sig.host().as_deref(), Some("example.com"));
    }

    #[test]
    fn page_signature_host_returns_none_for_empty() {
        let sig = PageSignature::new("", None);
        assert!(sig.host().is_none());
    }
}
