//! Network-identity coherence port trait and supporting types.
//!
//! The 2026 scraping guide
//! (`docs/dev/project/scraping-guide-2026-llm-context.md` §"WebRTC
//! coherence rule", L2839) requires five orthogonal vectors to agree
//! before a request is sent:
//!
//! 1. Proxy exit IP country
//! 2. DNS resolver country
//! 3. WebRTC public IP (must be in the same `/16` as the proxy exit)
//! 4. Browser timezone (IANA TZ database, e.g. `America/New_York`)
//! 5. Browser `Accept-Language`
//!
//! A mismatch on any of these vectors is the "WebRTC Trap" (L3135-3138) —
//! one of the highest-signal anti-bot tells in the field. The
//! [`CoherencePort`] trait captures this check as a pure, stateless
//! function that consumes a [`CoherenceContext`] and returns a
//! [`CoherenceVerdict`]; the default implementation in
//! `adapters::coherence::DefaultCoherenceValidator` (behind the
//! `coherence-validation` cargo feature) applies the rule above.
//!
//! The trait lives in the always-compiled `ports::coherence` module so the
//! [`crate::manager::ProxyManager`] plumbing can reference it uniformly
//! with or without the feature; only the default validator is
//! feature-gated, mirroring the T96 `BayesianObserver` / `ThompsonStrategy`
//! pattern.
//!
//! ## Module-level example
//!
//! ```
//! use std::net::IpAddr;
//! use std::str::FromStr;
//! use stygian_proxy::ports::coherence::{
//!     AcceptLanguage, CoherenceContext, CoherencePolicy, CoherencePort,
//!     CoherenceVerdict, IsoCountry, Locale, MismatchField, MismatchSeverity, Tz,
//! };
//!
//! // A clean US context: every vector agrees.
//! let ctx = CoherenceContext {
//!     proxy_geo_country: Some(IsoCountry::new("US").unwrap()),
//!     dns_resolver_country: Some(IsoCountry::new("US").unwrap()),
//!     browser_locale: Locale::new("en-US").unwrap(),
//!     browser_timezone: Tz::new("America/New_York").unwrap(),
//!     accept_language: AcceptLanguage::new("en-US,en;q=0.9").unwrap(),
//!     webrtc_local_ip: None,
//!     webrtc_public_ip: Some(IpAddr::from_str("192.0.2.42").unwrap()),
//!     proxy_ip: Some(IpAddr::from_str("192.0.2.7").unwrap()),
//! };
//!
//! // Without a validator the verdict is "Unknown"; the trait itself is
//! // always-compiled but the default implementation lives behind the
//! // `coherence-validation` feature.
//! let verdict = ctx.evaluate();
//! assert!(verdict.is_unknown());
//! assert_eq!(verdict.unknown_reason(), Some("no_coherence_validator"));
//!
//! // Policies are independent of the validator: the default is
//! // advisory-only.
//! let policy = CoherencePolicy::advisory();
//! assert!(!policy.is_hard_fail(MismatchField::ProxyGeoVsDns));
//! let policy = CoherencePolicy::hard_fail_on(MismatchField::ProxyGeoVsDns);
//! assert!(policy.is_hard_fail(MismatchField::ProxyGeoVsDns));
//! assert_eq!(policy.severity(MismatchField::ProxyGeoVsDns), MismatchSeverity::Hard);
//! let _ = ctx; // suppress unused warning under no-features build
//! ```

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// ISO-3166-1 alpha-2 country code (e.g. `US`, `GB`, `PK`).
///
/// Validated on construction so a downstream validator never has to
/// reject malformed strings. Stored upper-case for stable hashing and
/// serde output; comparisons are case-insensitive at the boundary via
/// [`IsoCountry::eq_ignore_ascii_case`].
///
/// # Example
/// ```
/// use stygian_proxy::ports::coherence::IsoCountry;
/// let us = IsoCountry::new("us").unwrap();
/// assert_eq!(us.as_str(), "US");
/// assert!(IsoCountry::new("USA").is_none());
/// assert!(IsoCountry::new("u").is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IsoCountry(String);

impl IsoCountry {
    /// Parse and uppercase a two-letter country code.
    ///
    /// Returns `None` when `raw` is not exactly two ASCII alpha
    /// characters; the validator treats unknown / malformed codes as
    /// [`CoherenceVerdict::Unknown`] rather than emitting false
    /// mismatches.
    #[must_use]
    pub fn new(raw: &str) -> Option<Self> {
        let upper = raw.trim().to_ascii_uppercase();
        if upper.len() == 2 && upper.chars().all(|c| c.is_ascii_alphabetic()) {
            Some(Self(upper))
        } else {
            None
        }
    }

    /// Returns the upper-case two-letter code.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Case-insensitive equality check for inbound config that may not
    /// have been normalised.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::ports::coherence::IsoCountry;
    /// let us = IsoCountry::new("US").unwrap();
    /// assert!(us.eq_ignore_ascii_case("us"));
    /// assert!(us.eq_ignore_ascii_case("Us"));
    /// assert!(!us.eq_ignore_ascii_case("GB"));
    /// ```
    #[must_use]
    pub fn eq_ignore_ascii_case(&self, other: &str) -> bool {
        self.0.eq_ignore_ascii_case(other)
    }
}

impl std::fmt::Display for IsoCountry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// IANA timezone identifier (e.g. `America/New_York`, `Europe/London`).
///
/// The validator only inspects the leading path segment (`America`,
/// `Europe`, `Asia`, …) plus the city when relevant; the wrapper
/// itself stores the canonical IANA string verbatim so the existing
/// `Intl.DateTimeFormat().resolvedOptions().timeZone` output round-trips
/// without translation.
///
/// # Example
/// ```
/// use stygian_proxy::ports::coherence::Tz;
/// let tz = Tz::new("America/New_York").unwrap();
/// assert_eq!(tz.as_str(), "America/New_York");
/// assert_eq!(tz.region(), Some("America"));
/// assert_eq!(tz.city(), Some("New_York"));
/// assert!(Tz::new("").is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Tz(String);

impl Tz {
    /// Parse an IANA timezone string. Accepts the canonical
    /// `Region/City` form and a leading `Etc/UTC` shortcut. Returns
    /// `None` when the string is empty.
    #[must_use]
    pub fn new(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(Self(trimmed.to_owned()))
    }

    /// Returns the raw IANA string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Leading path segment (`America`, `Europe`, …). Always present for
    /// valid IANA ids; `None` for the synthetic `UTC` shortcut.
    #[must_use]
    pub fn region(&self) -> Option<&str> {
        self.0.split('/').next()
    }

    /// City segment after the first `/`.
    #[must_use]
    pub fn city(&self) -> Option<&str> {
        self.0.split_once('/').map(|(_, city)| city)
    }
}

impl std::fmt::Display for Tz {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// BCP-47 locale tag (e.g. `en-US`, `fr-FR`).
///
/// Stores the lower-cased language and upper-cased region so locale
/// vectors from different OS surfaces (`navigator.language` versus
/// `Accept-Language` versus a manual `setlocale` call) collapse to the
/// same canonical form.
///
/// # Example
/// ```
/// use stygian_proxy::ports::coherence::Locale;
/// let l = Locale::new("en-us").unwrap();
/// assert_eq!(l.as_str(), "en-US");
/// assert_eq!(l.language(), "en");
/// assert_eq!(l.region().unwrap(), "US");
/// assert!(Locale::new("en").is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Locale(String);

impl Locale {
    /// Parse a BCP-47 locale with both a language and a region
    /// subtag (e.g. `en-US`, `fr-FR`). Returns `None` for bare
    /// language tags (`en`) since the country↔locale check requires
    /// a region.
    #[must_use]
    pub fn new(raw: &str) -> Option<Self> {
        let normalized = raw.trim().replace('_', "-");
        let (lang, region) = normalized.split_once('-')?;
        let lang = lang.to_ascii_lowercase();
        let region = region.to_ascii_uppercase();
        if lang.len() < 2
            || !lang.chars().all(|c| c.is_ascii_alphabetic())
            || region.len() != 2
            || !region.chars().all(|c| c.is_ascii_alphabetic())
        {
            return None;
        }
        Some(Self(format!("{lang}-{region}")))
    }

    /// Returns the canonical `lang-REGION` form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Lower-cased language subtag (`en`, `fr`, …).
    #[must_use]
    pub fn language(&self) -> &str {
        self.0.split_once('-').map_or(&self.0, |(l, _)| l)
    }

    /// Upper-cased region subtag (`US`, `FR`, …).
    #[must_use]
    pub fn region(&self) -> Option<&str> {
        self.0.split_once('-').map(|(_, r)| r)
    }
}

impl std::fmt::Display for Locale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// `Accept-Language` header value (RFC 7231 §5.3.5).
///
/// Captures the full header — `en-US,en;q=0.9,fr;q=0.8` — so the
/// validator can inspect the region of the highest-q entry without
/// parsing on every call. The first / highest-priority entry is the one
/// that drives the country agreement check.
///
/// # Example
/// ```
/// use stygian_proxy::ports::coherence::AcceptLanguage;
/// let al = AcceptLanguage::new("en-US,en;q=0.9").unwrap();
/// assert_eq!(al.as_str(), "en-US,en;q=0.9");
/// let primary = al.primary_region().unwrap();
/// assert_eq!(primary.as_str(), "en-US");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AcceptLanguage(String);

impl AcceptLanguage {
    /// Capture the raw header. Empty strings are rejected so the
    /// validator never sees a phantom "missing language" mismatch.
    #[must_use]
    pub fn new(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(Self(trimmed.to_owned()))
    }

    /// Returns the raw header value verbatim.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Highest-priority region parsed as a [`Locale`] (best-effort).
    ///
    /// Returns `None` when the primary entry has no region subtag
    /// (e.g. `en;q=1.0`).
    #[must_use]
    pub fn primary_region(&self) -> Option<Locale> {
        let primary = self.0.split(',').next()?;
        // Strip any `;q=...` quality value before locale parsing.
        let tag = primary.split(';').next()?.trim();
        Locale::new(tag)
    }
}

impl std::fmt::Display for AcceptLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Field on which two coherence vectors disagreed.
///
/// Every variant has a fixed [`MismatchSeverity`] (advisory vs hard)
/// embedded in the validator — the port just enumerates the possible
/// mismatch sites so [`CoherencePolicy::hard_fail_on`] can target a
/// specific vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MismatchField {
    /// `proxy_geo_country` vs `dns_resolver_country`.
    ProxyGeoVsDns,
    /// `webrtc_public_ip` not in the same `/16` as the proxy exit.
    WebRtcPublicIp,
    /// `browser_timezone` disagrees with the proxy / DNS country.
    Timezone,
    /// `browser_locale` region disagrees with the proxy / DNS country.
    Locale,
    /// `accept_language` primary region disagrees with the proxy / DNS country.
    AcceptLanguage,
}

impl MismatchField {
    /// Stable `snake_case` wire label.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::ports::coherence::MismatchField;
    /// assert_eq!(MismatchField::ProxyGeoVsDns.label(), "proxy_geo_vs_dns");
    /// assert_eq!(MismatchField::WebRtcPublicIp.label(), "web_rtc_public_ip");
    /// ```
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ProxyGeoVsDns => "proxy_geo_vs_dns",
            Self::WebRtcPublicIp => "web_rtc_public_ip",
            Self::Timezone => "timezone",
            Self::Locale => "locale",
            Self::AcceptLanguage => "accept_language",
        }
    }
}

impl std::fmt::Display for MismatchField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// How serious a given mismatch is for downstream routing.
///
/// - `Advisory` — recoverable drift (locale / timezone); logged but the
///   request still goes through.
/// - `Hard` — geo / WebRTC divergence that the major anti-bot vendors
///   treat as an immediate block signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MismatchSeverity {
    /// Mismatch is logged; the request proceeds.
    Advisory,
    /// Mismatch triggers an immediate block under `CoherencePolicy::hard_fail_on`.
    Hard,
}

impl MismatchSeverity {
    /// Stable `snake_case` wire label.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::ports::coherence::MismatchSeverity;
    /// assert_eq!(MismatchSeverity::Advisory.label(), "advisory");
    /// assert_eq!(MismatchSeverity::Hard.label(), "hard");
    /// ```
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Advisory => "advisory",
            Self::Hard => "hard",
        }
    }

    /// `true` when the variant is [`MismatchSeverity::Hard`].
    #[must_use]
    pub const fn is_hard(self) -> bool {
        matches!(self, Self::Hard)
    }
}

impl std::fmt::Display for MismatchSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Outcome of a single coherence evaluation.
///
/// Constructed by [`CoherencePort::evaluate`]. The `Mismatch` variant
/// carries enough information for the caller to map the verdict to a
/// [`CoherencePolicy`] decision (`Advisory` → log + proceed;
/// `Hard` + `policy.hard_fail_on(field)` → return error).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum CoherenceVerdict {
    /// Every checked vector agreed (or no vectors were available).
    Coherent,
    /// A specific vector disagreed.
    Mismatch {
        /// Which vector disagreed.
        field: MismatchField,
        /// How serious the disagreement is.
        severity: MismatchSeverity,
    },
    /// Verdict could not be reached (missing data, disabled feature, etc.).
    ///
    /// The `String` reason is intended for structured logs and test
    /// assertions, never user-facing copy. Use
    /// [`CoherenceVerdict::unknown`] to build a verdict from a
    /// `&'static str` reason without spelling out the `String`
    /// constructor at every call site.
    Unknown(String),
}

impl CoherenceVerdict {
    /// Convenience constructor for [`CoherenceVerdict::Unknown`] from a
    /// static reason. Keeps the hot-path call site concise while still
    /// allowing external callers to wrap an arbitrary string.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::ports::coherence::CoherenceVerdict;
    /// let v = CoherenceVerdict::unknown("missing_dns");
    /// assert!(v.is_unknown());
    /// assert_eq!(v.unknown_reason(), Some("missing_dns"));
    /// ```
    #[must_use]
    pub fn unknown(reason: &'static str) -> Self {
        Self::Unknown(reason.to_owned())
    }

    /// Returns the [`Unknown`](Self::Unknown) reason as a string slice,
    /// or `None` for `Coherent` / `Mismatch`.
    #[must_use]
    pub const fn unknown_reason(&self) -> Option<&str> {
        match self {
            Self::Unknown(reason) => Some(reason.as_str()),
            _ => None,
        }
    }

    /// Returns `true` when the verdict is [`CoherenceVerdict::Coherent`].
    #[must_use]
    pub const fn is_coherent(&self) -> bool {
        matches!(self, Self::Coherent)
    }

    /// Returns `true` when the verdict is [`CoherenceVerdict::Unknown`].
    #[must_use]
    pub const fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown(_))
    }
}

impl std::fmt::Display for CoherenceVerdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Coherent => f.write_str("coherent"),
            Self::Mismatch { field, severity } => {
                write!(f, "mismatch:{severity}:{field}")
            }
            Self::Unknown(reason) => write!(f, "unknown:{reason}"),
        }
    }
}

/// Policy configuring how [`crate::manager::ProxyManager::acquire_proxy_with_coherence`]
/// reacts to a [`CoherenceVerdict::Mismatch`].
///
/// The default [`CoherencePolicy::advisory`] policy never blocks; every
/// mismatch is logged and the proxy is returned. Operators opt into
/// blocking via [`CoherencePolicy::hard_fail_on`] or the
/// [`CoherencePolicy::with_hard_fail`] builder step.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CoherencePolicy {
    /// Fields whose `Hard` mismatch should fail the acquisition.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    hard_fail_on: std::collections::BTreeSet<MismatchField>,
}

impl CoherencePolicy {
    /// Advisory-only policy: every mismatch is logged, none block.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::ports::coherence::{CoherencePolicy, MismatchField};
    /// let policy = CoherencePolicy::advisory();
    /// assert!(!policy.is_hard_fail(MismatchField::ProxyGeoVsDns));
    /// assert!(policy.is_advisory_only());
    /// ```
    #[must_use]
    pub const fn advisory() -> Self {
        Self {
            hard_fail_on: std::collections::BTreeSet::new(),
        }
    }

    /// Build a policy that fails on a single hard-mismatch field.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::ports::coherence::{CoherencePolicy, MismatchField, MismatchSeverity};
    /// let policy = CoherencePolicy::hard_fail_on(MismatchField::WebRtcPublicIp);
    /// assert!(policy.is_hard_fail(MismatchField::WebRtcPublicIp));
    /// assert_eq!(policy.severity(MismatchField::WebRtcPublicIp), MismatchSeverity::Hard);
    /// assert!(!policy.is_hard_fail(MismatchField::Timezone));
    /// ```
    #[must_use]
    pub fn hard_fail_on(field: MismatchField) -> Self {
        let mut hard_fail_on = std::collections::BTreeSet::new();
        hard_fail_on.insert(field);
        Self { hard_fail_on }
    }

    /// Builder step: register an additional hard-fail field.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::ports::coherence::{CoherencePolicy, MismatchField};
    /// let policy = CoherencePolicy::advisory()
    ///     .with_hard_fail(MismatchField::ProxyGeoVsDns)
    ///     .with_hard_fail(MismatchField::WebRtcPublicIp);
    /// assert!(policy.is_hard_fail(MismatchField::ProxyGeoVsDns));
    /// assert!(policy.is_hard_fail(MismatchField::WebRtcPublicIp));
    /// assert!(!policy.is_hard_fail(MismatchField::Timezone));
    /// ```
    #[must_use]
    pub fn with_hard_fail(mut self, field: MismatchField) -> Self {
        self.hard_fail_on.insert(field);
        self
    }

    /// Returns `true` when `field` is registered as a hard-fail vector.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::ports::coherence::{CoherencePolicy, MismatchField};
    /// let policy = CoherencePolicy::hard_fail_on(MismatchField::Timezone);
    /// assert!(policy.is_hard_fail(MismatchField::Timezone));
    /// assert!(!policy.is_hard_fail(MismatchField::Locale));
    /// ```
    #[must_use]
    pub fn contains(&self, field: MismatchField) -> bool {
        self.hard_fail_on.contains(&field)
    }

    /// Alias for [`CoherencePolicy::contains`] matching the
    /// `policy.is_hard_fail(field)` shape used in the rustdoc examples
    /// above.
    #[must_use]
    pub fn is_hard_fail(&self, field: MismatchField) -> bool {
        self.contains(field)
    }

    /// Returns `true` when no field is registered for hard-fail.
    #[must_use]
    pub fn is_advisory_only(&self) -> bool {
        self.hard_fail_on.is_empty()
    }

    /// Severity under which the policy blocks a request on `field`.
    ///
    /// Always [`MismatchSeverity::Hard`] for registered fields and
    /// [`MismatchSeverity::Advisory`] otherwise. Used by the manager
    /// to decide whether a given [`CoherenceVerdict::Mismatch`] should
    /// fail the acquisition or just be logged.
    #[must_use]
    pub fn severity(&self, field: MismatchField) -> MismatchSeverity {
        if self.hard_fail_on.contains(&field) {
            MismatchSeverity::Hard
        } else {
            MismatchSeverity::Advisory
        }
    }

    /// Number of registered hard-fail fields.
    #[must_use]
    pub fn hard_fail_count(&self) -> usize {
        self.hard_fail_on.len()
    }
}

/// Snapshot of the network-identity vectors that the [`CoherencePort`]
/// validates.
///
/// The browser (or test harness) builds a `CoherenceContext` from the
/// live page + the proxy that is about to be used; the port then
/// decides whether the request is safe to send.
///
/// `proxy_ip` is optional because the manager can build the context
/// from the [`crate::types::Proxy`] record (which carries `url` but not
/// the resolved IP) before the proxy is actually contacted; the
/// validator treats a missing `proxy_ip` as a soft `Unknown` rather
/// than a hard mismatch so the integration does not require an extra
/// DNS lookup on the hot path.
///
/// # Example
/// ```
/// use std::net::IpAddr;
/// use std::str::FromStr;
/// use stygian_proxy::ports::coherence::{AcceptLanguage, CoherenceContext, IsoCountry, Locale, Tz};
///
/// let ctx = CoherenceContext {
///     proxy_geo_country: Some(IsoCountry::new("US").unwrap()),
///     dns_resolver_country: Some(IsoCountry::new("US").unwrap()),
///     browser_locale: Locale::new("en-US").unwrap(),
///     browser_timezone: Tz::new("America/New_York").unwrap(),
///     accept_language: AcceptLanguage::new("en-US,en;q=0.9").unwrap(),
///     webrtc_local_ip: None,
///     webrtc_public_ip: Some(IpAddr::from_str("192.0.2.42").unwrap()),
///     proxy_ip: Some(IpAddr::from_str("192.0.2.7").unwrap()),
/// };
/// assert_eq!(ctx.proxy_geo_country.as_ref().map(IsoCountry::as_str), Some("US"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CoherenceContext {
    /// ISO-3166-1 alpha-2 country of the proxy exit (provider-supplied).
    pub proxy_geo_country: Option<IsoCountry>,
    /// ISO-3166-1 alpha-2 country of the recursive DNS resolver.
    pub dns_resolver_country: Option<IsoCountry>,
    /// Browser locale reported by `navigator.language`.
    pub browser_locale: Locale,
    /// Browser timezone reported by `Intl.DateTimeFormat().resolvedOptions().timeZone`.
    pub browser_timezone: Tz,
    /// `Accept-Language` header value.
    pub accept_language: AcceptLanguage,
    /// WebRTC local (LAN) candidate IP, if the browser exposed one.
    pub webrtc_local_ip: Option<IpAddr>,
    /// WebRTC public (server-reflexive) candidate IP, if the browser
    /// exposed one.
    pub webrtc_public_ip: Option<IpAddr>,
    /// Resolved IP of the proxy exit. `None` skips the WebRTC /16 check.
    pub proxy_ip: Option<IpAddr>,
}

impl CoherenceContext {
    /// Compute the canonical `/16` prefix of an `IpAddr`.
    ///
    /// Returns `None` for IPv6 addresses (the heuristic is IPv4-only) and
    /// for addresses that fall outside the routable unicast space. Used
    /// by [`crate::adapters::coherence::DefaultCoherenceValidator`] for
    /// the WebRTC public-IP agreement check.
    ///
    /// # Example
    /// ```
    /// use std::net::IpAddr;
    /// use std::str::FromStr;
    /// use stygian_proxy::ports::coherence::CoherenceContext;
    /// let ip = IpAddr::from_str("192.0.2.42").unwrap();
    /// assert_eq!(CoherenceContext::same_slash_16(ip, IpAddr::from_str("192.0.2.7").unwrap()), Some(true));
    /// assert_eq!(CoherenceContext::same_slash_16(ip, IpAddr::from_str("203.0.113.5").unwrap()), Some(false));
    /// ```
    #[must_use]
    pub fn same_slash_16(a: IpAddr, b: IpAddr) -> Option<bool> {
        let (IpAddr::V4(a), IpAddr::V4(b)) = (a, b) else {
            return None;
        };
        let a_prefix = u32::from(a) >> 16;
        let b_prefix = u32::from(b) >> 16;
        Some(a_prefix == b_prefix)
    }

    /// Convenience: evaluate this context with no validator wired in.
    ///
    /// Without the `coherence-validation` cargo feature enabled, the
    /// [`ProxyManager`](crate::manager::ProxyManager) has no validator
    /// installed and every call returns
    /// `CoherenceVerdict::Unknown("no_coherence_validator")`. External
    /// callers that want a no-op verdict should call this method rather
    /// than reaching into a private field on the manager.
    #[must_use]
    pub fn evaluate(&self) -> CoherenceVerdict {
        CoherenceVerdict::unknown("no_coherence_validator")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CoherencePort
// ─────────────────────────────────────────────────────────────────────────────

/// Network-identity coherence port.
///
/// Implementors decide whether `ctx` is safe to send through: a clean
/// context returns [`CoherenceVerdict::Coherent`], a disagreement on a
/// specific vector returns
/// [`CoherenceVerdict::Mismatch`](CoherenceVerdict::Mismatch) with the
/// matching [`MismatchField`] and [`MismatchSeverity`], and a missing
/// observation (no DNS data, WebRTC disabled, …) returns
/// [`CoherenceVerdict::Unknown`].
///
/// `Send + Sync + 'static` so the implementation can live behind an
/// `Arc<dyn CoherencePort>` on the manager. The default implementation
/// in
/// [`crate::adapters::coherence::DefaultCoherenceValidator`](crate::adapters::coherence::DefaultCoherenceValidator)
/// is `Send + Sync + 'static` and stateless; an alternative adapter
/// that needed caching would add a Mutex but the trait itself never
/// requires one.
///
/// # Example
///
/// ```rust,no_run
/// use std::net::IpAddr;
/// use std::str::FromStr;
/// use stygian_proxy::ports::coherence::{
///     AcceptLanguage, CoherenceContext, CoherencePort, CoherenceVerdict,
///     IsoCountry, Locale, MismatchField, MismatchSeverity, Tz,
/// };
///
/// // Custom adapter: only fail when proxy country disagrees with DNS.
/// struct StrictGeo;
///
/// impl CoherencePort for StrictGeo {
///     fn evaluate(&self, ctx: &CoherenceContext) -> CoherenceVerdict {
///         match (ctx.proxy_geo_country.as_ref(), ctx.dns_resolver_country.as_ref()) {
///             (Some(proxy), Some(dns)) if proxy == dns => CoherenceVerdict::Coherent,
///             (Some(_), Some(_)) => CoherenceVerdict::Mismatch {
///                 field: MismatchField::ProxyGeoVsDns,
///                 severity: MismatchSeverity::Hard,
///             },
///             _ => CoherenceVerdict::unknown("missing_geo"),
///         }
///     }
/// }
///
/// let ctx = CoherenceContext {
///     proxy_geo_country: Some(IsoCountry::new("US").unwrap()),
///     dns_resolver_country: Some(IsoCountry::new("PK").unwrap()),
///     browser_locale: Locale::new("en-US").unwrap(),
///     browser_timezone: Tz::new("America/New_York").unwrap(),
///     accept_language: AcceptLanguage::new("en-US").unwrap(),
///     webrtc_local_ip: None,
///     webrtc_public_ip: Some(IpAddr::from_str("192.0.2.7").unwrap()),
///     proxy_ip: Some(IpAddr::from_str("192.0.2.7").unwrap()),
/// };
/// let v = StrictGeo.evaluate(&ctx);
/// assert!(matches!(
///     v,
///     CoherenceVerdict::Mismatch {
///         field: MismatchField::ProxyGeoVsDns,
///         severity: MismatchSeverity::Hard,
///     }
/// ));
/// ```
pub trait CoherencePort: Send + Sync + 'static {
    /// Run the coherence check on `ctx`.
    fn evaluate(&self, ctx: &CoherenceContext) -> CoherenceVerdict;
}

/// Shared-ownership type alias for a [`CoherencePort`] implementation.
///
/// Mirrors [`crate::strategy::BoxedRotationStrategy`] and
/// [`crate::strategy::BoxedBayesianObserver`] so the manager holds a
/// single `Arc<dyn CoherencePort>` regardless of which adapter
/// implementation it was built with.
pub type BoxedCoherencePort = std::sync::Arc<dyn CoherencePort>;

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn us_country() -> IsoCountry {
        IsoCountry::new("US").unwrap()
    }

    fn en_us_locale() -> Locale {
        Locale::new("en-US").unwrap()
    }

    fn ny_tz() -> Tz {
        Tz::new("America/New_York").unwrap()
    }

    fn en_us_al() -> AcceptLanguage {
        AcceptLanguage::new("en-US,en;q=0.9").unwrap()
    }

    fn ctx_us() -> CoherenceContext {
        CoherenceContext {
            proxy_geo_country: Some(us_country()),
            dns_resolver_country: Some(us_country()),
            browser_locale: en_us_locale(),
            browser_timezone: ny_tz(),
            accept_language: en_us_al(),
            webrtc_local_ip: None,
            webrtc_public_ip: Some(IpAddr::from_str("192.0.2.42").unwrap()),
            proxy_ip: Some(IpAddr::from_str("192.0.2.7").unwrap()),
        }
    }

    // ── IsoCountry ───────────────────────────────────────────────────────────

    #[test]
    fn iso_country_normalises_case() {
        assert_eq!(IsoCountry::new("us").unwrap().as_str(), "US");
        assert_eq!(IsoCountry::new("Gb").unwrap().as_str(), "GB");
    }

    #[test]
    fn iso_country_rejects_invalid_lengths() {
        assert!(IsoCountry::new("USA").is_none());
        assert!(IsoCountry::new("U").is_none());
        assert!(IsoCountry::new("").is_none());
    }

    #[test]
    fn iso_country_rejects_non_alpha() {
        assert!(IsoCountry::new("U1").is_none());
        assert!(IsoCountry::new("12").is_none());
    }

    #[test]
    fn iso_country_eq_ignore_ascii_case_works() {
        let us = us_country();
        assert!(us.eq_ignore_ascii_case("us"));
        assert!(us.eq_ignore_ascii_case("US"));
        assert!(us.eq_ignore_ascii_case("Us"));
        assert!(!us.eq_ignore_ascii_case("GB"));
    }

    #[test]
    fn iso_country_round_trips_through_json() {
        let us = us_country();
        let json = serde_json::to_string(&us).expect("serialize");
        assert_eq!(json, "\"US\"");
        let parsed: IsoCountry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, us);
    }

    // ── Tz ────────────────────────────────────────────────────────────────────

    #[test]
    fn tz_region_and_city() {
        let tz = ny_tz();
        assert_eq!(tz.region(), Some("America"));
        assert_eq!(tz.city(), Some("New_York"));
    }

    #[test]
    fn tz_rejects_empty() {
        assert!(Tz::new("").is_none());
        assert!(Tz::new("   ").is_none());
    }

    #[test]
    fn tz_round_trips_through_json() {
        let tz = ny_tz();
        let json = serde_json::to_string(&tz).expect("serialize");
        let parsed: Tz = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, tz);
    }

    // ── Locale ───────────────────────────────────────────────────────────────

    #[test]
    fn locale_normalises_case_and_underscore() {
        let l = Locale::new("en_us").unwrap();
        assert_eq!(l.as_str(), "en-US");
        assert_eq!(l.language(), "en");
        assert_eq!(l.region(), Some("US"));
    }

    #[test]
    fn locale_rejects_bare_language_tag() {
        assert!(Locale::new("en").is_none());
        assert!(Locale::new("EN").is_none());
    }

    #[test]
    fn locale_rejects_malformed_region() {
        assert!(Locale::new("en-USA").is_none());
        assert!(Locale::new("en-U1").is_none());
    }

    #[test]
    fn locale_round_trips_through_json() {
        let l = en_us_locale();
        let json = serde_json::to_string(&l).expect("serialize");
        let parsed: Locale = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, l);
    }

    // ── AcceptLanguage ───────────────────────────────────────────────────────

    #[test]
    fn accept_language_primary_region() {
        let al = en_us_al();
        let primary = al.primary_region().unwrap();
        assert_eq!(primary.as_str(), "en-US");
    }

    #[test]
    fn accept_language_strips_quality_value() {
        let al = AcceptLanguage::new("fr-FR;q=0.8").unwrap();
        assert_eq!(al.primary_region().unwrap().as_str(), "fr-FR");
    }

    #[test]
    fn accept_language_rejects_empty() {
        assert!(AcceptLanguage::new("").is_none());
    }

    #[test]
    fn accept_language_bare_language_tag_yields_none_primary() {
        let al = AcceptLanguage::new("en;q=1.0").unwrap();
        assert!(al.primary_region().is_none());
    }

    // ── MismatchField / MismatchSeverity ─────────────────────────────────────

    #[test]
    fn mismatch_field_labels_are_stable() {
        assert_eq!(MismatchField::ProxyGeoVsDns.label(), "proxy_geo_vs_dns");
        assert_eq!(MismatchField::WebRtcPublicIp.label(), "web_rtc_public_ip");
        assert_eq!(MismatchField::Timezone.label(), "timezone");
        assert_eq!(MismatchField::Locale.label(), "locale");
        assert_eq!(MismatchField::AcceptLanguage.label(), "accept_language");
    }

    #[test]
    fn mismatch_severity_labels_are_stable() {
        assert_eq!(MismatchSeverity::Advisory.label(), "advisory");
        assert_eq!(MismatchSeverity::Hard.label(), "hard");
        assert!(!MismatchSeverity::Advisory.is_hard());
        assert!(MismatchSeverity::Hard.is_hard());
    }

    // ── CoherenceVerdict ─────────────────────────────────────────────────────

    #[test]
    fn verdict_display_is_stable() {
        assert_eq!(CoherenceVerdict::Coherent.to_string(), "coherent");
        let v = CoherenceVerdict::Mismatch {
            field: MismatchField::ProxyGeoVsDns,
            severity: MismatchSeverity::Hard,
        };
        assert_eq!(v.to_string(), "mismatch:hard:proxy_geo_vs_dns");
        let v = CoherenceVerdict::unknown("missing_dns");
        assert_eq!(v.to_string(), "unknown:missing_dns");
        assert_eq!(v.unknown_reason(), Some("missing_dns"));
    }

    #[test]
    fn verdict_is_coherent_and_unknown_helpers() {
        assert!(CoherenceVerdict::Coherent.is_coherent());
        assert!(!CoherenceVerdict::Coherent.is_unknown());
        assert!(CoherenceVerdict::unknown("x").is_unknown());
        assert!(!CoherenceVerdict::unknown("x").is_coherent());
    }

    #[test]
    fn verdict_round_trips_through_json() {
        let v = CoherenceVerdict::Mismatch {
            field: MismatchField::WebRtcPublicIp,
            severity: MismatchSeverity::Hard,
        };
        let json = serde_json::to_string(&v).expect("serialize");
        let parsed: CoherenceVerdict = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, v);
    }

    #[test]
    fn coherent_verdict_round_trips_through_json() {
        let v = CoherenceVerdict::Coherent;
        let json = serde_json::to_string(&v).expect("serialize");
        let parsed: CoherenceVerdict = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, v);
    }

    // ── CoherencePolicy ──────────────────────────────────────────────────────

    #[test]
    fn advisory_policy_blocks_nothing() {
        let p = CoherencePolicy::advisory();
        assert!(p.is_advisory_only());
        assert!(!p.is_hard_fail(MismatchField::ProxyGeoVsDns));
        assert_eq!(
            p.severity(MismatchField::ProxyGeoVsDns),
            MismatchSeverity::Advisory
        );
        assert_eq!(p.hard_fail_count(), 0);
    }

    #[test]
    fn hard_fail_on_policy_blocks_a_single_field() {
        let p = CoherencePolicy::hard_fail_on(MismatchField::WebRtcPublicIp);
        assert!(p.is_hard_fail(MismatchField::WebRtcPublicIp));
        assert!(!p.is_hard_fail(MismatchField::ProxyGeoVsDns));
        assert_eq!(
            p.severity(MismatchField::WebRtcPublicIp),
            MismatchSeverity::Hard
        );
        assert_eq!(
            p.severity(MismatchField::Timezone),
            MismatchSeverity::Advisory
        );
        assert_eq!(p.hard_fail_count(), 1);
        assert!(!p.is_advisory_only());
    }

    #[test]
    fn with_hard_fail_accumulates() {
        let p = CoherencePolicy::advisory()
            .with_hard_fail(MismatchField::ProxyGeoVsDns)
            .with_hard_fail(MismatchField::Timezone);
        assert!(p.is_hard_fail(MismatchField::ProxyGeoVsDns));
        assert!(p.is_hard_fail(MismatchField::Timezone));
        assert!(!p.is_hard_fail(MismatchField::Locale));
        assert_eq!(p.hard_fail_count(), 2);
    }

    #[test]
    fn policy_round_trips_through_json() {
        let p = CoherencePolicy::advisory()
            .with_hard_fail(MismatchField::ProxyGeoVsDns)
            .with_hard_fail(MismatchField::WebRtcPublicIp);
        let json = serde_json::to_string(&p).expect("serialize");
        let parsed: CoherencePolicy = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, p);
    }

    #[test]
    fn empty_policy_round_trips_through_json() {
        let p = CoherencePolicy::advisory();
        let json = serde_json::to_string(&p).expect("serialize");
        // `skip_serializing_if = "BTreeSet::is_empty"` keeps the wire
        // form empty rather than emitting `"hard_fail_on": []`.
        assert!(!json.contains("hard_fail_on"));
        let parsed: CoherencePolicy = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, p);
    }

    // ── CoherenceContext ─────────────────────────────────────────────────────

    #[test]
    fn slash_16_agrees_within_prefix() {
        let a = IpAddr::from_str("192.0.2.42").unwrap();
        let b = IpAddr::from_str("192.0.2.7").unwrap();
        let c = IpAddr::from_str("203.0.113.5").unwrap();
        assert_eq!(CoherenceContext::same_slash_16(a, b), Some(true));
        assert_eq!(CoherenceContext::same_slash_16(a, c), Some(false));
    }

    #[test]
    fn slash_16_returns_none_for_ipv6() {
        let v4 = IpAddr::from_str("192.0.2.42").unwrap();
        let v6 = IpAddr::from_str("2001:db8::1").unwrap();
        assert_eq!(CoherenceContext::same_slash_16(v4, v6), None);
    }

    #[test]
    fn evaluate_with_no_validator_returns_unknown() {
        let ctx = ctx_us();
        assert!(matches!(
            ctx.evaluate(),
            CoherenceVerdict::Unknown(_) if ctx.evaluate().unknown_reason() == Some("no_coherence_validator")
        ));
    }

    #[test]
    fn context_round_trips_through_json() {
        let ctx = ctx_us();
        let json = serde_json::to_string(&ctx).expect("serialize");
        let parsed: CoherenceContext = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, ctx);
    }

    // ── Trait object dispatch ─────────────────────────────────────────────────

    /// `CoherencePort` is dyn-safe: `BoxedCoherencePort` must compile.
    #[test]
    fn boxed_coherence_port_is_object_safe() {
        fn _assert_object_safe(_: BoxedCoherencePort) {}
        // The closure form also compiles via the manager's `Arc<dyn ...>`.
        let _boxed: BoxedCoherencePort = std::sync::Arc::new(NoopCoherenceValidator);
    }

    /// Always-coherent stub adapter used to exercise the object-safe
    /// path. Lives in the test module because no production code needs
    /// it; production code is wired to
    /// `adapters::coherence::DefaultCoherenceValidator`.
    #[derive(Debug)]
    struct NoopCoherenceValidator;

    impl CoherencePort for NoopCoherenceValidator {
        fn evaluate(&self, _: &CoherenceContext) -> CoherenceVerdict {
            CoherenceVerdict::Coherent
        }
    }

    #[test]
    fn trait_object_dispatches_through_arc() {
        let v: BoxedCoherencePort = std::sync::Arc::new(NoopCoherenceValidator);
        let ctx = ctx_us();
        assert_eq!(v.evaluate(&ctx), CoherenceVerdict::Coherent);
    }
}
