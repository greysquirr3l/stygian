//! Core domain types for proxy management.
//!
//! ## IP class and target compatibility
//!
//! The 2026 scraping guide (see
//! `docs/dev/project/scraping-guide-2026-llm-context.md` §"PROXY PROVIDERS
//! AND TYPES") ranks egress IPs into a four-tier trust hierarchy used by
//! every Tier-1 anti-bot vendor:
//!
//! | Rank | [`IpClass`]     | Typical use                                    |
//! | ---- | --------------- | ---------------------------------------------- |
//! | 4    | [`Mobile`](IpClass::Mobile) | 3G/4G/5G carrier egress — defeats `DataDome`, `PerimeterX`, `Kasada` |
//! | 3    | [`Isp`](IpClass::Isp)      | Static/ISP allocation — defeats `Akamai`, `Cloudflare`, `PerimeterX` |
//! | 2    | [`Residential`](IpClass::Residential) | Rotating residential pool — defeats most Tier-1 vendors |
//! | 1    | [`Datacenter`](IpClass::Datacenter) | Hosted VPS / bare-metal — defeated by `DataDome`, `PerimeterX` |
//! | 0    | [`Unknown`](IpClass::Unknown) | Provider did not tag the egress — fail-secure default |
//!
//! Each [`Proxy`] and [`ProxyCapabilities`] carries two typed fields that
//! drive capability-aware acquisition:
//!
//! - `ip_class: IpClass` — the proxy's egress tier. Acquisition matches via
//!   `ip_class.rank() >= requirement.rank()` so a [`Mobile`](IpClass::Mobile)
//!   proxy satisfies a request that requires [`Isp`](IpClass::Isp).
//! - `target_compatibility: TargetVendorCompatibility` — a
//!   `BTreeMap<VendorId, TrustTier>` mapping each anti-bot vendor to a
//!   declared effectiveness tier. Free-list fetchers tag every ingested
//!   proxy as `default_blocked()` (no vendor confirmed) so callers cannot
//!   accidentally route premium traffic through a public free-list pool.
//!
//! ## Geo enrichment
//!
//! Operators targeting specific cities / ASNs / postal codes — the
//! "Infatica-style city, ZIP, and ASN" filter cited by the 2026
//! scraping guide (L2837) — populate the optional
//! [`asn`](ProxyCapabilities::asn),
//! [`city`](ProxyCapabilities::city), and
//! [`postal_code`](ProxyCapabilities::postal_code) fields on
//! [`ProxyCapabilities`]. The corresponding
//! [`require_asn`](CapabilityRequirement::require_asn),
//! [`require_city`](CapabilityRequirement::require_city), and
//! [`require_postal_code`](CapabilityRequirement::require_postal_code)
//! fields on [`CapabilityRequirement`] select proxies whose geo
//! metadata matches. Empty requirement still matches any proxy (the
//! existing invariant is preserved).
//!
//! ```rust
//! use stygian_proxy::types::{CapabilityRequirement, ProxyCapabilities};
//! use stygian_proxy::types::well_known::KNOWN_ASN_CLOUDFLARE;
//!
//! // Akamai scrape: insist the egress IP is in Cloudflare's AS.
//! let caps = ProxyCapabilities {
//!     asn: Some(KNOWN_ASN_CLOUDFLARE),
//!     city: Some("San Francisco".into()),
//!     postal_code: Some("94110".into()),
//!     ..Default::default()
//! };
//! let req = CapabilityRequirement {
//!     require_asn: Some(KNOWN_ASN_CLOUDFLARE),
//!     require_city: Some("San Francisco".into()),
//!     require_postal_code: Some("94110".into()),
//!     ..Default::default()
//! };
//! assert!(caps.satisfies(&req));
//! ```
//!
//! ```rust
//! use stygian_proxy::types::{IpClass, TargetVendorCompatibility, TrustTier, VendorId};
//!
//! // A static-ISP proxy confirmed effective against Akamai and Cloudflare.
//! let compat = TargetVendorCompatibility::default()
//!     .set(VendorId::Akamai, TrustTier::Preferred)
//!     .set(VendorId::Cloudflare, TrustTier::Acceptable);
//! assert_eq!(compat.get(VendorId::Akamai), Some(TrustTier::Preferred));
//! assert_eq!(IpClass::Isp.rank(), 3);
//! ```

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The protocol variant of a proxy endpoint.
///
/// # Example
/// ```
/// use stygian_proxy::types::ProxyType;
/// assert_eq!(ProxyType::Http, ProxyType::Http);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyType {
    /// Plain HTTP proxy (CONNECT / forwarding).
    Http,
    /// HTTPS proxy over TLS.
    Https,
    #[cfg(feature = "socks")]
    /// SOCKS4 proxy (requires the `socks` feature).
    Socks4,
    #[cfg(feature = "socks")]
    /// SOCKS5 proxy (requires the `socks` feature).
    Socks5,
    /// CDN edge relay (`Cloudflare`, `CloudFront`, `Azure Front Door`, etc.).
    ///
    /// Traffic egresses through a CDN point-of-presence rather than a traditional proxy
    /// server.  Provider metadata is carried in
    /// [`ProxyCapabilities::cdn_provider`].
    CdnEdge,
}

/// TLS-profiled request mode for proxy-side HTTP operations.
///
/// Used by `tls-profiled` integrations to decide how strictly browser TLS
/// profiles should be mapped onto rustls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfiledRequestMode {
    /// Broad compatibility: skip unknown entries and use safe fallbacks.
    Compatible,
    /// Profile-aware preset selected from the profile name.
    Preset,
    /// Strict cipher-suite mapping with compatibility group fallback.
    Strict,
    /// Strict cipher-suite + group mapping without fallback.
    StrictAll,
}

/// IP trust class for a proxy egress.
///
/// Encodes the four-tier IP trust hierarchy cited by the 2026 scraping
/// guide: mobile carriers > static ISP allocations > rotating residential
/// pools > datacenter ranges. The 5th variant, [`Unknown`](IpClass::Unknown),
/// is the fail-secure default for any proxy whose provider did not declare
/// its class.
///
/// `Copy + Eq + Hash` so [`IpClass`] can be used as a `BTreeMap` key and
/// embedded in `Copy` structs without an extra allocation.
///
/// # Example
/// ```
/// use stygian_proxy::types::IpClass;
/// assert!(IpClass::Mobile.rank() > IpClass::Isp.rank());
/// assert!(IpClass::Isp.rank() > IpClass::Residential.rank());
/// assert!(IpClass::Residential.rank() > IpClass::Datacenter.rank());
/// assert_eq!(IpClass::default(), IpClass::Unknown);
/// ```
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum IpClass {
    /// Mobile-carrier (3G/4G/5G) egress.
    Mobile,
    /// Static ISP allocation.
    Isp,
    /// Rotating residential pool.
    Residential,
    /// Datacenter / VPS / bare-metal egress.
    Datacenter,
    /// Provider did not tag the egress. Fail-secure default.
    #[default]
    Unknown,
}

impl IpClass {
    /// Rank used to express "at least this tier" requirements.
    ///
    /// Higher rank = higher trust. Mobile (4) outranks ISP (3) which
    /// outranks Residential (2) which outranks Datacenter (1); `Unknown`
    /// is the lowest (0) so a proxy that does not declare its class
    /// never satisfies a non-empty `IpClassRequirement`.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::IpClass;
    /// assert_eq!(IpClass::Mobile.rank(), 4);
    /// assert_eq!(IpClass::Isp.rank(), 3);
    /// assert_eq!(IpClass::Residential.rank(), 2);
    /// assert_eq!(IpClass::Datacenter.rank(), 1);
    /// assert_eq!(IpClass::Unknown.rank(), 0);
    /// ```
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Mobile => 4,
            Self::Isp => 3,
            Self::Residential => 2,
            Self::Datacenter => 1,
            Self::Unknown => 0,
        }
    }

    /// Stable, `snake_case` wire label (matches the [`serde`][Self] representation).
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::IpClass;
    /// assert_eq!(IpClass::Mobile.label(), "mobile");
    /// assert_eq!(IpClass::Datacenter.label(), "datacenter");
    /// assert_eq!(IpClass::Unknown.label(), "unknown");
    /// ```
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Mobile => "mobile",
            Self::Isp => "isp",
            Self::Residential => "residential",
            Self::Datacenter => "datacenter",
            Self::Unknown => "unknown",
        }
    }

    /// Parse an [`IpClass`] from its [`label`][Self] `snake_case` string.
    ///
    /// Mirrors [`VendorId::from_label`] so MCP and external configs can use
    /// the same string vocabulary.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::IpClass;
    /// assert_eq!(IpClass::from_label("mobile"), Some(IpClass::Mobile));
    /// assert_eq!(IpClass::from_label("datacenter"), Some(IpClass::Datacenter));
    /// assert_eq!(IpClass::from_label("nope"), None);
    /// ```
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "mobile" => Some(Self::Mobile),
            "isp" => Some(Self::Isp),
            "residential" => Some(Self::Residential),
            "datacenter" => Some(Self::Datacenter),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// Declared effectiveness of a proxy against a given anti-bot vendor.
///
/// `Preferred` is the highest trust, `Blocked` means the proxy is
/// known to be defeated by the vendor (used as the default for free-list
/// fetches so callers cannot accidentally route premium traffic through
/// a public free-list pool).
///
/// # Example
/// ```
/// use stygian_proxy::types::TrustTier;
/// assert!(TrustTier::Preferred.rank() > TrustTier::Acceptable.rank());
/// assert!(TrustTier::Acceptable.rank() > TrustTier::Marginal.rank());
/// assert!(TrustTier::Marginal.rank() > TrustTier::Blocked.rank());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustTier {
    /// The proxy is known to defeat the vendor on the first try.
    Preferred,
    /// The proxy typically succeeds; expect occasional friction.
    Acceptable,
    /// The proxy is hit-or-miss; budget for retries and CAPTCHA solves.
    Marginal,
    /// The proxy is known to be blocked. Used as the default for free-list
    /// fetches so operators must opt-in to using them for premium vendors.
    Blocked,
}

impl TrustTier {
    /// Rank used for tier-comparison helpers.
    ///
    /// Higher = better. `Preferred` (4) > `Acceptable` (3) > `Marginal` (2) >
    /// `Blocked` (1). The value is `1`-based so `Blocked` is still
    /// "ranked" (i.e. not zero) — that lets `is_blocked()` be a simple
    /// `rank() == 1` check.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::TrustTier;
    /// assert!(TrustTier::Preferred.is_blocked() == false);
    /// assert!(TrustTier::Blocked.is_blocked());
    /// ```
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Preferred => 4,
            Self::Acceptable => 3,
            Self::Marginal => 2,
            Self::Blocked => 1,
        }
    }

    /// `true` when the tier is [`TrustTier::Blocked`].
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::TrustTier;
    /// assert!(TrustTier::Blocked.is_blocked());
    /// assert!(!TrustTier::Marginal.is_blocked());
    /// ```
    #[must_use]
    pub const fn is_blocked(self) -> bool {
        matches!(self, Self::Blocked)
    }
}

/// Anti-bot vendor identifier.
///
/// This is a local mirror of the same taxonomy used by `stygian-charon`'s
/// `vendor_classifier::VendorId` so the labels round-trip through
/// `serde` identically across crates. The wire labels are stable
/// `snake_case` strings; see [`VendorId::label`].
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum VendorId {
    /// `Akamai` Bot Manager.
    Akamai,
    /// `Cloudflare` bot management.
    Cloudflare,
    /// `DataDome`.
    DataDome,
    /// `PerimeterX` / HUMAN Security.
    PerimeterX,
    /// hCaptcha challenge provider.
    Hcaptcha,
    /// Google reCAPTCHA challenge provider.
    Recaptcha,
    /// Kasada challenge provider.
    Kasada,
    /// Fingerprint.com identification.
    FingerprintCom,
    /// Shape Security (F5).
    ShapeSecurity,
    /// Imperva (Incapsula) bot management.
    Imperva,
    /// Catch-all when no vendor was declared.
    #[default]
    Unknown,
}

impl VendorId {
    /// Stable, lower-case wire label used by [`VendorId::from_label`].
    ///
    /// Mirrors the `#[serde(rename_all = "snake_case")]` wire form so
    /// `serde_json::to_string(&variant) == format!("\"{label}\"")` for
    /// every variant — see `vendor_id_round_trips_through_json`.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::VendorId;
    /// assert_eq!(VendorId::DataDome.label(), "data_dome");
    /// assert_eq!(VendorId::PerimeterX.label(), "perimeter_x");
    /// assert_eq!(VendorId::Cloudflare.label(), "cloudflare");
    /// assert_eq!(VendorId::Akamai.label(), "akamai");
    /// ```
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Akamai => "akamai",
            Self::Cloudflare => "cloudflare",
            Self::DataDome => "data_dome",
            Self::PerimeterX => "perimeter_x",
            Self::Hcaptcha => "hcaptcha",
            Self::Recaptcha => "recaptcha",
            Self::Kasada => "kasada",
            Self::FingerprintCom => "fingerprint_com",
            Self::ShapeSecurity => "shape_security",
            Self::Imperva => "imperva",
            Self::Unknown => "unknown",
        }
    }

    /// Parse a [`VendorId`] from its [`label`][Self::label].
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::VendorId;
    /// assert_eq!(VendorId::from_label("data_dome"), Some(VendorId::DataDome));
    /// assert_eq!(VendorId::from_label("cloudflare"), Some(VendorId::Cloudflare));
    /// assert_eq!(VendorId::from_label("nope"), None);
    /// ```
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "akamai" => Some(Self::Akamai),
            "cloudflare" => Some(Self::Cloudflare),
            "data_dome" => Some(Self::DataDome),
            "perimeter_x" => Some(Self::PerimeterX),
            "hcaptcha" => Some(Self::Hcaptcha),
            "recaptcha" => Some(Self::Recaptcha),
            "kasada" => Some(Self::Kasada),
            "fingerprint_com" => Some(Self::FingerprintCom),
            "shape_security" => Some(Self::ShapeSecurity),
            "imperva" => Some(Self::Imperva),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// Mapping from anti-bot [`VendorId`] to the proxy's declared
/// [`TrustTier`] against that vendor.
///
/// `default_blocked()` returns a populated map with every known vendor
/// marked as [`TrustTier::Blocked`], which is the safe choice for
/// free-list ingest: callers must explicitly opt-in to trusting a
/// free-list pool for premium vendors.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", transparent)]
pub struct TargetVendorCompatibility {
    /// Per-vendor trust tier.
    defeats: BTreeMap<VendorId, TrustTier>,
}

impl TargetVendorCompatibility {
    /// Empty compatibility — every vendor falls back to
    /// [`TrustTier::Blocked`] at the requirement gate.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::TargetVendorCompatibility;
    /// let c = TargetVendorCompatibility::default();
    /// assert!(c.is_empty());
    /// assert_eq!(c.get(stygian_proxy::types::VendorId::DataDome), None);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a compatibility map with every known vendor marked
    /// [`TrustTier::Blocked`].
    ///
    /// Used by free-list fetchers to fail-secure on ingest: a free-list
    /// proxy cannot satisfy a `target_vendor` capability requirement
    /// unless the operator explicitly upgrades the tier.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::{TargetVendorCompatibility, TrustTier, VendorId};
    /// let c = TargetVendorCompatibility::default_blocked();
    /// assert_eq!(c.get(VendorId::DataDome), Some(TrustTier::Blocked));
    /// assert_eq!(c.get(VendorId::Cloudflare), Some(TrustTier::Blocked));
    /// assert!(!c.is_empty());
    /// ```
    #[must_use]
    pub fn default_blocked() -> Self {
        let mut defeats = BTreeMap::new();
        for vendor in [
            VendorId::Akamai,
            VendorId::Cloudflare,
            VendorId::DataDome,
            VendorId::PerimeterX,
            VendorId::Hcaptcha,
            VendorId::Recaptcha,
            VendorId::Kasada,
            VendorId::FingerprintCom,
            VendorId::ShapeSecurity,
            VendorId::Imperva,
        ] {
            defeats.insert(vendor, TrustTier::Blocked);
        }
        Self { defeats }
    }

    /// Returns the declared tier for `vendor`, or `None` when unknown.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::{TargetVendorCompatibility, TrustTier, VendorId};
    /// let c = TargetVendorCompatibility::default().set(VendorId::Akamai, TrustTier::Preferred);
    /// assert_eq!(c.get(VendorId::Akamai), Some(TrustTier::Preferred));
    /// assert_eq!(c.get(VendorId::DataDome), None);
    /// ```
    #[must_use]
    pub fn get(&self, vendor: VendorId) -> Option<TrustTier> {
        self.defeats.get(&vendor).copied()
    }

    /// Set the declared tier for `vendor`, replacing any prior value.
    ///
    /// Builder-style: takes `self` by value and returns the updated
    /// `TargetVendorCompatibility` so calls can be chained.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::{TargetVendorCompatibility, TrustTier, VendorId};
    /// let c = TargetVendorCompatibility::default()
    ///     .set(VendorId::Cloudflare, TrustTier::Acceptable)
    ///     .set(VendorId::Akamai, TrustTier::Preferred);
    /// assert_eq!(c.get(VendorId::Cloudflare), Some(TrustTier::Acceptable));
    /// assert_eq!(c.get(VendorId::Akamai), Some(TrustTier::Preferred));
    /// ```
    #[must_use]
    pub fn set(mut self, vendor: VendorId, tier: TrustTier) -> Self {
        self.defeats.insert(vendor, tier);
        self
    }

    /// Returns `true` when no vendor tiers have been declared.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::{TargetVendorCompatibility, TrustTier, VendorId};
    /// assert!(TargetVendorCompatibility::default().is_empty());
    /// assert!(!TargetVendorCompatibility::default_blocked().is_empty());
    /// let c = TargetVendorCompatibility::default().set(VendorId::Cloudflare, TrustTier::Preferred);
    /// assert!(!c.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.defeats.is_empty()
    }

    /// Returns the number of declared vendor tiers.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::{TargetVendorCompatibility, TrustTier, VendorId};
    /// let c = TargetVendorCompatibility::default();
    /// assert_eq!(c.len(), 0);
    /// let c = TargetVendorCompatibility::default().set(VendorId::Cloudflare, TrustTier::Preferred);
    /// assert_eq!(c.len(), 1);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.defeats.len()
    }

    /// Iterate `(vendor, tier)` pairs in deterministic (sorted) order.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::{TargetVendorCompatibility, TrustTier, VendorId};
    /// let c = TargetVendorCompatibility::default()
    ///     .set(VendorId::DataDome, TrustTier::Preferred)
    ///     .set(VendorId::Akamai, TrustTier::Acceptable);
    /// let entries: Vec<_> = c.iter().collect();
    /// // Sorted by VendorId discriminant order (Akamai < DataDome).
    /// assert_eq!(entries.first().map(|(v, _)| *v), Some(VendorId::Akamai));
    /// assert_eq!(entries.get(1).map(|(v, _)| *v), Some(VendorId::DataDome));
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = (VendorId, TrustTier)> + '_ {
        self.defeats.iter().map(|(v, t)| (*v, *t))
    }
}

/// Minimum [`IpClass`] required for a capability-aware acquisition.
///
/// A `Mobile` proxy satisfies a `Isp` requirement because
/// `Mobile.rank() > Isp.rank()`. The reverse (`Isp` does not satisfy
/// `Mobile`) is also true. The default is [`IpClass::Unknown`] which
/// matches every non-empty proxy (and is also the only way to match an
/// `Unknown` proxy).
///
/// # Example
/// ```
/// use stygian_proxy::types::{IpClass, IpClassRequirement};
/// let req = IpClassRequirement { minimum: IpClass::Isp };
/// assert!(req.is_satisfied_by(IpClass::Mobile));
/// assert!(req.is_satisfied_by(IpClass::Isp));
/// assert!(!req.is_satisfied_by(IpClass::Residential));
/// assert!(!req.is_satisfied_by(IpClass::Datacenter));
/// ```
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", transparent)]
pub struct IpClassRequirement {
    /// Minimum IP class rank required.
    pub minimum: IpClass,
}

impl IpClassRequirement {
    /// Returns `true` when `proxy` meets or exceeds the requirement.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::{IpClass, IpClassRequirement};
    /// let req = IpClassRequirement { minimum: IpClass::Isp };
    /// assert!(req.is_satisfied_by(IpClass::Mobile));
    /// assert!(req.is_satisfied_by(IpClass::Isp));
    /// assert!(!req.is_satisfied_by(IpClass::Datacenter));
    /// ```
    #[must_use]
    pub const fn is_satisfied_by(&self, proxy: IpClass) -> bool {
        proxy.rank() >= self.minimum.rank()
    }
}

/// Protocol-level capabilities advertised by a proxy endpoint.
///
/// These flags are set when the proxy is registered and consulted during
/// capability-aware selection (see [`crate::manager::ProxyManager::acquire_with_capabilities`]).
///
/// # Example
/// ```
/// use stygian_proxy::types::ProxyCapabilities;
/// let caps = ProxyCapabilities::default();
/// assert!(!caps.supports_https_connect);
/// assert!(!caps.supports_socks5_udp);
/// assert!(!caps.supports_http3_tunnel);
/// assert_eq!(caps.ip_class, stygian_proxy::types::IpClass::Unknown);
/// assert!(caps.target_compatibility.is_empty());
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::struct_excessive_bools)] // 4 capability flags are clearer as named bools than a u8 bitmask
pub struct ProxyCapabilities {
    /// Proxy supports the `CONNECT` method for HTTPS tunnelling.
    #[serde(default)]
    pub supports_https_connect: bool,
    /// Proxy supports SOCKS5 with UDP relay (for UDP-based transports).
    #[serde(default)]
    pub supports_socks5_udp: bool,
    /// Proxy supports HTTP/3 (QUIC) tunnelling — future-compatible flag.
    #[serde(default)]
    pub supports_http3_tunnel: bool,
    /// Optional ISO-3166-1 alpha-2 country code for the proxy egress location.
    #[serde(default)]
    pub geo_country: Option<String>,
    /// Confidence score `[0.0, 1.0]` for the geo-location data.
    ///
    /// `None` means the provider did not supply confidence metadata.
    #[serde(default)]
    pub geo_confidence: Option<f32>,
    /// `true` when this proxy routes through a CDN edge node rather than a
    /// traditional SOCKS/HTTP proxy server.
    #[serde(default)]
    pub is_cdn_edge: bool,
    /// CDN provider name when `is_cdn_edge` is `true`.
    ///
    /// Advisory — used for monitoring and routing hints.
    /// Examples: `"cloudflare"`, `"cloudfront"`, `"azure-front-door"`.
    #[serde(default)]
    pub cdn_provider: Option<String>,
    /// TLS fingerprint profile this proxy presents toward the upstream target.
    ///
    /// Advisory identifier such as `"chrome-131"`, `"firefox-120"`, or
    /// `"curl"`.  Use with [`CapabilityRequirement::require_tls_profile`] to
    /// select proxies by their TLS stack identity.  `None` means unknown.
    #[serde(default)]
    pub tls_profile: Option<String>,
    /// IP trust class for the proxy egress.
    ///
    /// Defaults to [`IpClass::Unknown`] so legacy serialised proxies
    /// deserialize cleanly. See the module-level docs for the four-tier
    /// trust hierarchy and routing rationale.
    #[serde(default)]
    pub ip_class: IpClass,
    /// Per-vendor trust tier overrides.
    ///
    /// Free-list fetchers populate this with
    /// [`TargetVendorCompatibility::default_blocked`] so callers cannot
    /// accidentally route premium traffic through a public free-list
    /// pool. Operator-curated pools typically leave this empty and rely
    /// on per-vendor metadata from the provider.
    #[serde(default)]
    pub target_compatibility: TargetVendorCompatibility,
    /// Autonomous System Number (ASN) of the proxy's egress IP.
    ///
    /// Cited by the 2026 guide (L2837) as a "filter by ASN" feature
    /// offered by commercial providers (e.g. Infatica). `None` means
    /// the provider did not tag the proxy's AS; an exact-match
    /// [`CapabilityRequirement::require_asn`] filter is the only way to
    /// surface it. See the [`well_known`] module for the
    /// `Cloudflare` / `Akamai` / `Fastly` / `CloudFront` ASN constants.
    #[serde(default)]
    pub asn: Option<u32>,
    /// City of the proxy's egress IP (operator-declared, no validation
    /// beyond UTF-8).
    ///
    /// Format follows the operator's convention; the 2026 guide does
    /// not mandate a particular scheme. `None` means the city is
    /// unknown.
    #[serde(default)]
    pub city: Option<String>,
    /// Postal / ZIP code of the proxy's egress IP (operator-declared,
    /// no format enforced).
    ///
    /// The 2026 guide cites ZIP-level filtering as a commercial
    /// provider capability (L2837); the format is per-country (e.g.
    /// `"94110"` for US ZIP, `"SW1A 1AA"` for UK). `None` means the
    /// postal code is unknown.
    #[serde(default)]
    pub postal_code: Option<String>,
}

impl ProxyCapabilities {
    /// Returns `true` if every required flag in `req` is satisfied by `self`.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::{ProxyCapabilities, CapabilityRequirement};
    /// let caps = ProxyCapabilities { supports_https_connect: true, ..Default::default() };
    /// let req = CapabilityRequirement { require_https_connect: true, ..Default::default() };
    /// assert!(caps.satisfies(&req));
    /// let req2 = CapabilityRequirement { require_socks5_udp: true, ..Default::default() };
    /// assert!(!caps.satisfies(&req2));
    /// ```
    #[must_use]
    pub fn satisfies(&self, req: &CapabilityRequirement) -> bool {
        if req.require_https_connect && !self.supports_https_connect {
            return false;
        }
        if req.require_socks5_udp && !self.supports_socks5_udp {
            return false;
        }
        if req.require_http3_tunnel && !self.supports_http3_tunnel {
            return false;
        }
        if let Some(ref required_country) = req.require_geo_country
            && self.geo_country.as_deref() != Some(required_country.as_str())
        {
            return false;
        }
        if req.require_cdn_edge && !self.is_cdn_edge {
            return false;
        }
        if let Some(ref required_profile) = req.require_tls_profile
            && self.tls_profile.as_deref() != Some(required_profile.as_str())
        {
            return false;
        }
        if let Some(ref minimum_class) = req.require_ip_class
            && !minimum_class.is_satisfied_by(self.ip_class)
        {
            return false;
        }
        if let Some(required_vendor) = req.target_vendor
            && self
                .target_compatibility
                .get(required_vendor)
                .is_none_or(TrustTier::is_blocked)
        {
            return false;
        }
        if let Some(required_asn) = req.require_asn
            && self.asn != Some(required_asn)
        {
            return false;
        }
        if let Some(ref required_city) = req.require_city
            && self.city.as_deref() != Some(required_city.as_str())
        {
            return false;
        }
        if let Some(ref required_postal) = req.require_postal_code
            && self.postal_code.as_deref() != Some(required_postal.as_str())
        {
            return false;
        }
        true
    }
}

/// Required capability set used as a filter when acquiring a proxy.
///
/// All fields default to `false`/`None` — an empty requirement matches any proxy.
///
/// # Example
/// ```
/// use stygian_proxy::types::CapabilityRequirement;
/// let req = CapabilityRequirement::default();
/// // empty requirement — any proxy qualifies
/// assert!(!req.require_https_connect);
/// assert!(req.require_ip_class.is_none());
/// assert!(req.target_vendor.is_none());
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::struct_excessive_bools)] // 4 requirement flags mirror ProxyCapabilities 1:1; bitmask refactor would be a breaking change
pub struct CapabilityRequirement {
    /// Require `supports_https_connect`.
    #[serde(default)]
    pub require_https_connect: bool,
    /// Require `supports_socks5_udp`.
    #[serde(default)]
    pub require_socks5_udp: bool,
    /// Require `supports_http3_tunnel`.
    #[serde(default)]
    pub require_http3_tunnel: bool,
    /// Require a specific egress country (ISO-3166-1 alpha-2).
    #[serde(default)]
    pub require_geo_country: Option<String>,
    /// Require a CDN-edge proxy (`is_cdn_edge` must be `true`).
    #[serde(default)]
    pub require_cdn_edge: bool,
    /// Require a specific TLS fingerprint profile.
    ///
    /// When `Some`, only proxies whose [`ProxyCapabilities::tls_profile`]
    /// matches this value exactly are eligible.  Examples: `"chrome-131"`,
    /// `"firefox-120"`, `"curl"`.
    #[serde(default)]
    pub require_tls_profile: Option<String>,
    /// Minimum IP trust class required.
    ///
    /// When `Some`, the proxy's [`IpClass`] must outrank
    /// `require_ip_class.minimum`. A [`Mobile`](IpClass::Mobile) proxy
    /// satisfies an `Isp` requirement because
    /// `Mobile.rank() > Isp.rank()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_ip_class: Option<IpClassRequirement>,
    /// When `Some`, the proxy's [`TargetVendorCompatibility`] must carry
    /// a tier other than [`TrustTier::Blocked`] for this vendor (or be
    /// absent, in which case the requirement is treated as blocked).
    ///
    /// Used to gate free-list pools away from premium vendors: a proxy
    /// with `target_compatibility.get(DataDome) == Some(Blocked)` does
    /// not satisfy `target_vendor = Some(DataDome)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_vendor: Option<VendorId>,
    /// When `Some`, the proxy's [`ProxyCapabilities::asn`] must equal
    /// this value exactly. `None` on the proxy side never satisfies a
    /// `Some` requirement (no enrichment, no match). See
    /// [`crate::types::well_known`] for the canonical CDN ASN constants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_asn: Option<u32>,
    /// When `Some`, the proxy's [`ProxyCapabilities::city`] must equal
    /// this value exactly. `None` on the proxy side never satisfies a
    /// `Some` requirement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_city: Option<String>,
    /// When `Some`, the proxy's [`ProxyCapabilities::postal_code`] must
    /// equal this value exactly. `None` on the proxy side never
    /// satisfies a `Some` requirement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_postal_code: Option<String>,
}

/// The protocol routing path resolved for an outbound request.
///
/// Returned by [`crate::routing::resolve_routing_path`] to indicate how the
/// proxy should forward the connection.
///
/// # Example
/// ```
/// use stygian_proxy::types::RoutingPath;
/// let path = RoutingPath::H1H2OverTcp;
/// assert_eq!(format!("{path:?}"), "H1H2OverTcp");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingPath {
    /// HTTP/1.1 or HTTP/2 multiplexed over a TCP CONNECT tunnel.
    H1H2OverTcp,
    /// HTTP/3 (QUIC) over a UDP relay — requires `supports_http3_tunnel`.
    H3OverUdp,
    /// Persistent TCP CONNECT tunnel — connection is kept alive between requests.
    ///
    /// Selected when [`crate::routing::TransportPreference::PersistentTcp`] is used.
    PersistentTcp,
}

/// A proxy endpoint with optional authentication credentials.
///
/// `Debug` output masks `password` to prevent accidental credential logging.
///
/// # Example
/// ```
/// use stygian_proxy::types::{IpClass, Proxy, ProxyCapabilities, ProxyType, TrustTier, VendorId};
/// let compat = stygian_proxy::types::TargetVendorCompatibility::default()
///     .set(VendorId::Akamai, TrustTier::Preferred);
/// let p = Proxy {
///     url: "http://proxy.example.com:8080".into(),
///     proxy_type: ProxyType::Http,
///     username: Some("alice".into()),
///     password: Some("secret".into()),
///     weight: 1,
///     tags: vec!["prod".into()],
///     capabilities: ProxyCapabilities::default(),
///     ip_class: IpClass::Isp,
///     target_compatibility: compat,
/// };
/// let debug = format!("{p:?}");
/// assert!(debug.contains("***"), "password must be masked in Debug output");
/// assert_eq!(p.ip_class, IpClass::Isp);
/// assert_eq!(p.target_compatibility.get(VendorId::Akamai), Some(TrustTier::Preferred));
/// ```
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Proxy {
    /// The proxy URL, e.g. `http://proxy.example.com:8080`.
    pub url: String,
    pub proxy_type: ProxyType,
    pub username: Option<String>,
    pub password: Option<String>,
    /// Relative selection weight for weighted rotation (default: `1`).
    pub weight: u32,
    /// User-defined tags for filtering and grouping.
    pub tags: Vec<String>,
    /// Protocol-level capabilities advertised by this proxy.
    #[serde(default)]
    pub capabilities: ProxyCapabilities,
    /// IP trust class for this proxy's egress.
    ///
    /// Defaults to [`IpClass::Unknown`] when not provided. Free-list
    /// fetchers tag every ingested proxy as
    /// [`IpClass::Datacenter`]; operator-curated pools can override
    /// per-proxy. See the module-level docs for the trust hierarchy.
    #[serde(default)]
    pub ip_class: IpClass,
    /// Per-vendor trust tier overrides for this proxy.
    ///
    /// Defaults to an empty map (no overrides). Free-list fetchers
    /// populate this with
    /// [`TargetVendorCompatibility::default_blocked`] to prevent
    /// accidental use of free-list pools against premium vendors.
    #[serde(default)]
    pub target_compatibility: TargetVendorCompatibility,
}

impl std::fmt::Debug for Proxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Proxy")
            .field("url", &self.url)
            .field("proxy_type", &self.proxy_type)
            .field("username", &self.username)
            .field("password", &self.password.as_deref().map(|_| "***"))
            .field("weight", &self.weight)
            .field("tags", &self.tags)
            .field("capabilities", &self.capabilities)
            .field("ip_class", &self.ip_class)
            .field("target_compatibility", &self.target_compatibility)
            .finish()
    }
}

/// A [`Proxy`] with a stable identity and insertion timestamp.
///
/// # Example
/// ```
/// use stygian_proxy::types::{IpClass, Proxy, ProxyRecord, ProxyType};
/// let proxy = Proxy {
///     url: "http://proxy.example.com:8080".into(),
///     proxy_type: ProxyType::Http,
///     username: None,
///     password: None,
///     weight: 1,
///     tags: vec![],
///     capabilities: Default::default(),
///     ip_class: IpClass::Unknown,
///     target_compatibility: Default::default(),
/// };
/// let record = ProxyRecord::new(proxy);
/// assert!(!record.id.is_nil());
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ProxyRecord {
    pub id: Uuid,
    pub proxy: Proxy,
    /// Wall-clock time the proxy was added. Not serialized — `Instant` is
    /// not meaningfully portable; defaults to `Instant::now()` on deserialization.
    #[serde(skip, default = "Instant::now")]
    pub added_at: Instant,
}

impl ProxyRecord {
    /// Create a new [`ProxyRecord`] wrapping `proxy` with a freshly generated UUID.
    #[must_use]
    pub fn new(proxy: Proxy) -> Self {
        Self {
            id: Uuid::new_v4(),
            proxy,
            added_at: Instant::now(),
        }
    }
}

/// Per-proxy runtime metrics using lock-free atomic counters.
///
/// Intended to be shared via `Arc<ProxyMetrics>`.
///
/// # Example
/// ```
/// use stygian_proxy::types::ProxyMetrics;
/// let m = ProxyMetrics::default();
/// assert_eq!(m.success_rate(), 0.0);
/// assert_eq!(m.avg_latency_ms(), 0.0);
/// ```
#[derive(Debug, Default)]
pub struct ProxyMetrics {
    pub requests_total: AtomicU64,
    pub successes: AtomicU64,
    pub failures: AtomicU64,
    pub total_latency_ms: AtomicU64,
}

impl ProxyMetrics {
    /// Cast a `u64` counter to `f64` for ratio computation.
    ///
    /// `u64` can represent values up to ~1.8 × 10¹⁹; `f64` has 53-bit
    /// mantissa, so precision loss begins around 9 × 10¹⁵.  For long-running
    /// proxies that number is never reached in practice, and direct casting
    /// preserves ratios correctly (unlike saturating to `u32::MAX`).
    #[allow(clippy::cast_precision_loss)]
    const fn u64_as_f64(value: u64) -> f64 {
        value as f64
    }

    /// Returns the fraction of requests that succeeded, in `[0.0, 1.0]`.
    ///
    /// Returns `0.0` when no requests have been recorded.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::ProxyMetrics;
    /// use std::sync::atomic::Ordering;
    /// let m = ProxyMetrics::default();
    /// m.requests_total.store(10, Ordering::Relaxed);
    /// m.successes.store(8, Ordering::Relaxed);
    /// assert!((m.success_rate() - 0.8).abs() < f64::EPSILON);
    /// ```
    pub fn success_rate(&self) -> f64 {
        let total = self.requests_total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        Self::u64_as_f64(self.successes.load(Ordering::Relaxed)) / Self::u64_as_f64(total)
    }

    /// Returns the average request latency in milliseconds.
    ///
    /// Returns `0.0` when no requests have been recorded.
    ///
    /// # Example
    /// ```
    /// use stygian_proxy::types::ProxyMetrics;
    /// use std::sync::atomic::Ordering;
    /// let m = ProxyMetrics::default();
    /// m.requests_total.store(4, Ordering::Relaxed);
    /// m.total_latency_ms.store(400, Ordering::Relaxed);
    /// assert!((m.avg_latency_ms() - 100.0).abs() < f64::EPSILON);
    /// ```
    pub fn avg_latency_ms(&self) -> f64 {
        let total = self.requests_total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        Self::u64_as_f64(self.total_latency_ms.load(Ordering::Relaxed)) / Self::u64_as_f64(total)
    }
}

mod serde_duration_secs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        d.as_secs().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        Ok(Duration::from_secs(u64::deserialize(d)?))
    }
}

/// Configuration governing health checking and circuit-breaker behaviour.
///
/// Duration fields serialize as integer seconds for TOML/JSON compatibility.
///
/// # Example
/// ```
/// use stygian_proxy::types::ProxyConfig;
/// use std::time::Duration;
/// let cfg = ProxyConfig::default();
/// assert_eq!(cfg.health_check_url, "https://httpbin.org/ip");
/// assert_eq!(cfg.health_check_interval, Duration::from_secs(60));
/// assert_eq!(cfg.health_check_timeout, Duration::from_secs(5));
/// assert_eq!(cfg.circuit_open_threshold, 5);
/// assert_eq!(cfg.circuit_half_open_after, Duration::from_secs(30));
/// assert!(cfg.profiled_request_mode.is_none());
/// assert_eq!(cfg.health_check_jitter_pct, 0.20_f32);
/// assert!(cfg.max_requests_per_connection.is_none());
/// assert!(cfg.connection_max_age_secs.is_none());
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ProxyConfig {
    /// URL called during health checks to verify proxy liveness.
    pub health_check_url: String,
    /// How often to run health checks (seconds).
    #[serde(with = "serde_duration_secs")]
    pub health_check_interval: Duration,
    /// Per-probe HTTP timeout (seconds).
    #[serde(with = "serde_duration_secs")]
    pub health_check_timeout: Duration,
    /// Jitter factor applied to the health-check sleep window.
    ///
    /// `0.20` distributes each check window uniformly over
    /// `interval × [0.80, 1.20)`, preventing synchronised fleet-wide check
    /// storms.  Set to `0.0` to disable jitter.  Clamped to `[0.0, 0.99]`
    /// at runtime.
    ///
    /// Default: `0.20` (±20 %).
    #[serde(default = "default_health_check_jitter_pct")]
    pub health_check_jitter_pct: f32,
    /// Consecutive failures before the circuit trips to OPEN.
    pub circuit_open_threshold: u32,
    /// How long to wait in OPEN before transitioning to HALF-OPEN (seconds).
    #[serde(with = "serde_duration_secs")]
    pub circuit_half_open_after: Duration,
    /// Sticky-session policy for domain→proxy binding.
    #[serde(default)]
    pub sticky_policy: crate::session::StickyPolicy,
    /// Optional default mode for TLS-profiled helper clients.
    ///
    /// When set and `tls-profiled` is enabled, `ProxyManager` initializes its
    /// `HealthChecker` with a Chrome-profiled requester using this mode.
    ///
    /// Ignored when `tls-profiled` is disabled.
    #[serde(default)]
    pub profiled_request_mode: Option<ProfiledRequestMode>,
    /// Maximum requests routed through one persistent TCP connection before it
    /// is recycled.  `None` means no limit.  Only consulted when
    /// [`crate::routing::TransportPreference::PersistentTcp`] is active.
    #[serde(default)]
    pub max_requests_per_connection: Option<u32>,
    /// Maximum age of a persistent TCP connection in seconds before it is
    /// replaced.  `None` means no age limit.
    #[serde(default)]
    pub connection_max_age_secs: Option<u64>,
}

const fn default_health_check_jitter_pct() -> f32 {
    0.20
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            health_check_url: "https://httpbin.org/ip".into(),
            health_check_interval: Duration::from_mins(1),
            health_check_timeout: Duration::from_secs(5),
            health_check_jitter_pct: 0.20,
            circuit_open_threshold: 5,
            circuit_half_open_after: Duration::from_secs(30),
            sticky_policy: crate::session::StickyPolicy::default(),
            profiled_request_mode: None,
            max_requests_per_connection: None,
            connection_max_age_secs: None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// well_known CDN ASN constants
// ─────────────────────────────────────────────────────────────────────────────

/// Canonical ASN values for major public CDNs and infrastructure providers.
///
/// Use these as the source-of-truth
/// `CapabilityRequirement::require_asn` / `ProxyCapabilities::asn`
/// values when filtering the proxy pool by Autonomous System.
///
/// The list is ordered by frequency-of-use in scraping guides and the
/// 2026 guide's coverage of vendor fingerprinting. Values come from the
/// public IANA AS-number registry; they are stable and do not change
/// without a formal re-assignment.
///
/// # Example
///
/// ```
/// use stygian_proxy::types::{CapabilityRequirement, ProxyCapabilities};
/// use stygian_proxy::types::well_known::KNOWN_ASN_CLOUDFLARE;
///
/// let caps = ProxyCapabilities {
///     asn: Some(KNOWN_ASN_CLOUDFLARE),
///     ..Default::default()
/// };
/// let req = CapabilityRequirement {
///     require_asn: Some(KNOWN_ASN_CLOUDFLARE),
///     ..Default::default()
/// };
/// assert!(caps.satisfies(&req));
/// ```
pub mod well_known {
    /// `Cloudflare` AS — `13335`.
    pub const KNOWN_ASN_CLOUDFLARE: u32 = 13335;
    /// `Akamai` AS — `20940` (the public AS used by most Akamai edges).
    pub const KNOWN_ASN_AKAMAI: u32 = 20940;
    /// `Fastly` AS — `54113`.
    pub const KNOWN_ASN_FASTLY: u32 = 54113;
    /// `Amazon CloudFront` AS — `16509`.
    pub const KNOWN_ASN_CLOUDFRONT: u32 = 16509;
    /// `Google` AS — `15169` (covers `Google Cloud`, `YouTube` egress).
    pub const KNOWN_ASN_GOOGLE: u32 = 15169;
    /// `Microsoft Azure` AS — `8075`.
    pub const KNOWN_ASN_AZURE: u32 = 8075;
    /// `Limelight Networks` AS — `22822`.
    pub const KNOWN_ASN_LIMELIGHT: u32 = 22822;
    /// `StackPath / Highwinds` AS — `20446`.
    pub const KNOWN_ASN_HIGHWINDS: u32 = 20446;
    /// `Verizon Digital Media Services` (Edgecast) AS — `15133`.
    pub const KNOWN_ASN_EDGECAST: u32 = 15133;
    /// `Sucuri` AS — `51167`.
    pub const KNOWN_ASN_SUCURI: u32 = 51167;
    /// `OVH` AS — `16276` (a common datacenter provider that shows up on
    /// free-list feeds).
    pub const KNOWN_ASN_OVH: u32 = 16276;
    /// `Hetzner` AS — `24940` (a common datacenter provider that shows up
    /// on free-list feeds).
    pub const KNOWN_ASN_HETZNER: u32 = 24940;
    /// `DigitalOcean` AS — `14061` (a common datacenter provider that
    /// shows up on free-list feeds).
    pub const KNOWN_ASN_DIGITALOCEAN: u32 = 14061;
    /// `Linode / Akamai Connected Cloud` AS — `63949`.
    pub const KNOWN_ASN_LINODE: u32 = 63949;
    /// `Vultr` AS — `204957`.
    pub const KNOWN_ASN_VULTR: u32 = 204_957;

    /// Every constant in this module, in declaration order. Useful for
    /// exhaustiveness checks and operators that want a quick
    /// "is this AS a known CDN / major provider?" lookup.
    pub const ALL_KNOWN_ASNS: &[u32] = &[
        KNOWN_ASN_CLOUDFLARE,
        KNOWN_ASN_AKAMAI,
        KNOWN_ASN_FASTLY,
        KNOWN_ASN_CLOUDFRONT,
        KNOWN_ASN_GOOGLE,
        KNOWN_ASN_AZURE,
        KNOWN_ASN_LIMELIGHT,
        KNOWN_ASN_HIGHWINDS,
        KNOWN_ASN_EDGECAST,
        KNOWN_ASN_SUCURI,
        KNOWN_ASN_OVH,
        KNOWN_ASN_HETZNER,
        KNOWN_ASN_DIGITALOCEAN,
        KNOWN_ASN_LINODE,
        KNOWN_ASN_VULTR,
    ];
}

// ─────────────────────────────────────────────────────────────────────────────
// Geo-metadata ingest validation
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum length of an operator-supplied `city` string.
pub const CITY_MAX_LEN: usize = 100;
/// Maximum length of an operator-supplied `postal_code` string.
pub const POSTAL_CODE_MAX_LEN: usize = 16;

/// Returns `Ok(())` when `asn` is a valid public Autonomous System
/// Number, otherwise an [`crate::error::ProxyError::InvalidGeoMetadata`]
/// describing the failure.
///
/// A "public" AS number is `1..=u32::MAX - 1` — `0` is reserved by
/// IANA and `u32::MAX` is the RFC-defined "private use / reserved"
/// placeholder, neither of which is meaningful on the wire.
///
/// # Example
///
/// ```
/// use stygian_proxy::types::validate_asn;
/// assert!(validate_asn(13_335).is_ok());
/// assert!(validate_asn(0).is_err());
/// assert!(validate_asn(u32::MAX).is_err());
/// ```
pub fn validate_asn(asn: u32) -> Result<(), crate::error::ProxyError> {
    use crate::error::ProxyError;
    if asn == 0 {
        return Err(ProxyError::InvalidGeoMetadata {
            field: "asn".into(),
            reason: "ASN 0 is reserved by IANA and must not be used as a proxy ASN".into(),
        });
    }
    if asn == u32::MAX {
        return Err(ProxyError::InvalidGeoMetadata {
            field: "asn".into(),
            reason: format!("ASN {asn} is the RFC reserved/private-use placeholder"),
        });
    }
    Ok(())
}

/// Returns `Ok(())` when `city` is a valid operator-supplied city
/// label, otherwise an [`crate::error::ProxyError::InvalidGeoMetadata`].
///
/// `city` must be 1-100 characters (UTF-8 byte length). The format is
/// operator-defined; common conventions include `"San Francisco"`,
/// `"São Paulo"`, `"Saint-Étienne"`.
pub fn validate_city(city: &str) -> Result<(), crate::error::ProxyError> {
    use crate::error::ProxyError;
    if city.is_empty() {
        return Err(ProxyError::InvalidGeoMetadata {
            field: "city".into(),
            reason: "city must be 1 character or longer".into(),
        });
    }
    if city.len() > CITY_MAX_LEN {
        return Err(ProxyError::InvalidGeoMetadata {
            field: "city".into(),
            reason: format!(
                "city length {} exceeds the {CITY_MAX_LEN}-char limit",
                city.len()
            ),
        });
    }
    Ok(())
}

/// Returns `Ok(())` when `postal_code` is a valid operator-supplied
/// postal / ZIP code, otherwise an
/// [`crate::error::ProxyError::InvalidGeoMetadata`].
///
/// `postal_code` must be 1-16 characters. The format is per-country
/// (e.g. `"94110"` for US ZIP, `"SW1A 1AA"` for UK, `"100-0001"` for
/// Japan); the validator only enforces a length ceiling and rejects
/// empty strings.
pub fn validate_postal_code(postal_code: &str) -> Result<(), crate::error::ProxyError> {
    use crate::error::ProxyError;
    if postal_code.is_empty() {
        return Err(ProxyError::InvalidGeoMetadata {
            field: "postal_code".into(),
            reason: "postal_code must be 1 character or longer".into(),
        });
    }
    if postal_code.len() > POSTAL_CODE_MAX_LEN {
        return Err(ProxyError::InvalidGeoMetadata {
            field: "postal_code".into(),
            reason: format!(
                "postal_code length {} exceeds the {POSTAL_CODE_MAX_LEN}-char limit",
                postal_code.len()
            ),
        });
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)] // serde round-trips and unwraps in test fixtures are deterministic
mod tests {
    use super::*;
    use crate::well_known::{
        KNOWN_ASN_AKAMAI, KNOWN_ASN_CLOUDFLARE, KNOWN_ASN_FASTLY, KNOWN_ASN_OVH,
    };

    // ── IpClass ─────────────────────────────────────────────────────────────

    #[test]
    fn ip_class_default_is_unknown() {
        assert_eq!(IpClass::default(), IpClass::Unknown);
        assert_eq!(IpClass::Unknown.rank(), 0);
    }

    #[test]
    fn ip_class_rank_ordering_matches_t95_spec() {
        assert!(IpClass::Mobile.rank() > IpClass::Isp.rank());
        assert!(IpClass::Isp.rank() > IpClass::Residential.rank());
        assert!(IpClass::Residential.rank() > IpClass::Datacenter.rank());
        assert!(IpClass::Datacenter.rank() > IpClass::Unknown.rank());
    }

    #[test]
    fn ip_class_round_trips_through_json() {
        for variant in [
            IpClass::Mobile,
            IpClass::Isp,
            IpClass::Residential,
            IpClass::Datacenter,
            IpClass::Unknown,
        ] {
            let json = serde_json::to_string(&variant).expect("serialize IpClass");
            let parsed: IpClass = serde_json::from_str(&json).expect("deserialize IpClass");
            assert_eq!(parsed, variant, "round-trip for {variant:?}");
        }
    }

    #[test]
    fn ip_class_from_label_matches_serde_label() {
        for variant in [
            IpClass::Mobile,
            IpClass::Isp,
            IpClass::Residential,
            IpClass::Datacenter,
            IpClass::Unknown,
        ] {
            assert_eq!(
                IpClass::from_label(variant.label()),
                Some(variant),
                "label/from_label round-trip for {variant:?}"
            );
        }
        assert_eq!(IpClass::from_label("nope"), None);
    }

    // ── TrustTier ───────────────────────────────────────────────────────────

    #[test]
    fn trust_tier_rank_ordering_matches_t95_spec() {
        assert!(TrustTier::Preferred.rank() > TrustTier::Acceptable.rank());
        assert!(TrustTier::Acceptable.rank() > TrustTier::Marginal.rank());
        assert!(TrustTier::Marginal.rank() > TrustTier::Blocked.rank());
    }

    #[test]
    fn trust_tier_is_blocked() {
        assert!(TrustTier::Blocked.is_blocked());
        for tier in [
            TrustTier::Preferred,
            TrustTier::Acceptable,
            TrustTier::Marginal,
        ] {
            assert!(!tier.is_blocked());
        }
    }

    #[test]
    fn trust_tier_round_trips_through_json() {
        for tier in [
            TrustTier::Preferred,
            TrustTier::Acceptable,
            TrustTier::Marginal,
            TrustTier::Blocked,
        ] {
            let json = serde_json::to_string(&tier).expect("serialize TrustTier");
            let parsed: TrustTier = serde_json::from_str(&json).expect("deserialize TrustTier");
            assert_eq!(parsed, tier, "round-trip for {tier:?}");
        }
    }

    // ── VendorId ────────────────────────────────────────────────────────────

    #[test]
    fn vendor_id_label_matches_serde_wire_format() {
        assert_eq!(VendorId::DataDome.label(), "data_dome");
        assert_eq!(VendorId::PerimeterX.label(), "perimeter_x");
        assert_eq!(VendorId::Cloudflare.label(), "cloudflare");
        assert_eq!(VendorId::Akamai.label(), "akamai");
    }

    #[test]
    fn vendor_id_from_label_round_trip() {
        for variant in [
            VendorId::Akamai,
            VendorId::Cloudflare,
            VendorId::DataDome,
            VendorId::PerimeterX,
            VendorId::Hcaptcha,
            VendorId::Recaptcha,
            VendorId::Kasada,
            VendorId::FingerprintCom,
            VendorId::ShapeSecurity,
            VendorId::Imperva,
            VendorId::Unknown,
        ] {
            assert_eq!(VendorId::from_label(variant.label()), Some(variant));
        }
        assert_eq!(VendorId::from_label("nope"), None);
    }

    #[test]
    fn vendor_id_round_trips_through_json() {
        let variant = VendorId::DataDome;
        let json = serde_json::to_string(&variant).expect("serialize VendorId");
        // `#[serde(rename_all = "snake_case")]` on the enum rewrites
        // `DataDome` to `data_dome` (the `O` boundary is preserved).
        assert_eq!(json, "\"data_dome\"", "snake_case wire format");
        let parsed: VendorId = serde_json::from_str(&json).expect("deserialize VendorId");
        assert_eq!(parsed, variant);
    }

    // ── TargetVendorCompatibility ───────────────────────────────────────────

    #[test]
    fn target_vendor_compatibility_default_is_empty() {
        let c = TargetVendorCompatibility::default();
        assert!(c.is_empty());
        assert_eq!(c.len(), 0);
        assert_eq!(c.get(VendorId::DataDome), None);
    }

    #[test]
    fn target_vendor_compatibility_default_blocked_covers_known_vendors() {
        let c = TargetVendorCompatibility::default_blocked();
        assert!(!c.is_empty());
        assert_eq!(c.get(VendorId::DataDome), Some(TrustTier::Blocked));
        assert_eq!(c.get(VendorId::Akamai), Some(TrustTier::Blocked));
        assert_eq!(c.get(VendorId::Cloudflare), Some(TrustTier::Blocked));
        assert_eq!(c.get(VendorId::PerimeterX), Some(TrustTier::Blocked));
        assert_eq!(c.get(VendorId::Hcaptcha), Some(TrustTier::Blocked));
        assert_eq!(c.get(VendorId::Recaptcha), Some(TrustTier::Blocked));
        assert_eq!(c.get(VendorId::Kasada), Some(TrustTier::Blocked));
        assert_eq!(c.get(VendorId::FingerprintCom), Some(TrustTier::Blocked));
        assert_eq!(c.get(VendorId::ShapeSecurity), Some(TrustTier::Blocked));
        assert_eq!(c.get(VendorId::Imperva), Some(TrustTier::Blocked));
        assert_eq!(c.get(VendorId::Unknown), None);
    }

    #[test]
    fn target_vendor_compatibility_set_is_builder_style() {
        let c = TargetVendorCompatibility::default()
            .set(VendorId::Akamai, TrustTier::Preferred)
            .set(VendorId::Cloudflare, TrustTier::Acceptable);
        assert_eq!(c.len(), 2);
        assert_eq!(c.get(VendorId::Akamai), Some(TrustTier::Preferred));
        assert_eq!(c.get(VendorId::Cloudflare), Some(TrustTier::Acceptable));
    }

    #[test]
    fn target_vendor_compatibility_iterates_in_sorted_order() {
        let c = TargetVendorCompatibility::default()
            .set(VendorId::DataDome, TrustTier::Preferred)
            .set(VendorId::Akamai, TrustTier::Acceptable);
        let entries: Vec<_> = c.iter().collect();
        assert_eq!(entries.first().map(|(v, _)| *v), Some(VendorId::Akamai));
        assert_eq!(entries.get(1).map(|(v, _)| *v), Some(VendorId::DataDome));
    }

    #[test]
    fn target_vendor_compatibility_round_trips_through_json() {
        let original = TargetVendorCompatibility::default()
            .set(VendorId::DataDome, TrustTier::Preferred)
            .set(VendorId::Akamai, TrustTier::Acceptable)
            .set(VendorId::Cloudflare, TrustTier::Marginal);
        let json = serde_json::to_string(&original).expect("serialize");
        let parsed: TargetVendorCompatibility = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, original);
    }

    #[test]
    fn target_vendor_compatibility_round_trips_through_toml() {
        let original = TargetVendorCompatibility::default()
            .set(VendorId::DataDome, TrustTier::Preferred)
            .set(VendorId::Akamai, TrustTier::Acceptable);
        let toml_str = toml::to_string(&original).expect("serialize toml");
        let parsed: TargetVendorCompatibility =
            toml::from_str(&toml_str).expect("deserialize toml");
        assert_eq!(parsed, original);
    }

    #[test]
    fn target_vendor_compatibility_transparent_serde() {
        // `#[serde(transparent)]` on TargetVendorCompatibility means the
        // wire form is the BTreeMap directly. Verify that BTreeMap
        // ordering on the wire matches the sorted VendorId discriminant.
        let original = TargetVendorCompatibility::default()
            .set(VendorId::DataDome, TrustTier::Preferred)
            .set(VendorId::Akamai, TrustTier::Acceptable);
        let json = serde_json::to_string(&original).expect("serialize");
        // Akamai < DataDome in discriminant order, so `akamai` must
        // appear before `data_dome` in the serialised map.
        let akamai_pos = json.find("\"akamai\"").expect("akamai present");
        let datadome_pos = json.find("\"data_dome\"").expect("data_dome present");
        assert!(akamai_pos < datadome_pos, "expected sorted order: {json}");
    }

    // ── IpClassRequirement ──────────────────────────────────────────────────

    #[test]
    fn ip_class_requirement_default_minimum_is_unknown() {
        let req = IpClassRequirement::default();
        assert_eq!(req.minimum, IpClass::Unknown);
    }

    #[test]
    fn ip_class_requirement_is_satisfied_by_rank_gte_minimum() {
        let req = IpClassRequirement {
            minimum: IpClass::Isp,
        };
        assert!(req.is_satisfied_by(IpClass::Mobile));
        assert!(req.is_satisfied_by(IpClass::Isp));
        assert!(!req.is_satisfied_by(IpClass::Residential));
        assert!(!req.is_satisfied_by(IpClass::Datacenter));
        assert!(!req.is_satisfied_by(IpClass::Unknown));
    }

    #[test]
    fn ip_class_requirement_round_trips_through_json() {
        let req = IpClassRequirement {
            minimum: IpClass::Isp,
        };
        let json = serde_json::to_string(&req).expect("serialize");
        let parsed: IpClassRequirement = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, req);
    }

    // ── T95 mobile beats isp via rank ordering ──────────────────────────────

    /// The headline test from T95: a `Mobile` proxy with
    /// `defeats[DataDome] = Preferred` passes
    /// `require_ip_class = Some(Isp) for an Akamai target
    /// (Mobile > Isp wins via tier ordering)`.
    #[test]
    fn t95_mobile_beats_isp_requirement() {
        let compat =
            TargetVendorCompatibility::default().set(VendorId::DataDome, TrustTier::Preferred);
        let caps = ProxyCapabilities {
            ip_class: IpClass::Mobile,
            target_compatibility: compat,
            ..Default::default()
        };
        let req = CapabilityRequirement {
            require_ip_class: Some(IpClassRequirement {
                minimum: IpClass::Isp,
            }),
            ..Default::default()
        };
        assert!(caps.satisfies(&req));
    }

    // ── CapabilityRequirement backward compatibility ───────────────────────

    #[test]
    fn capability_requirement_default_matches_any_proxy() {
        let req = CapabilityRequirement::default();
        let caps_proxy_datacenter = ProxyCapabilities {
            ip_class: IpClass::Datacenter,
            ..Default::default()
        };
        let caps_proxy_unknown = ProxyCapabilities::default();
        let caps_proxy_full = ProxyCapabilities {
            supports_https_connect: true,
            supports_socks5_udp: true,
            supports_http3_tunnel: true,
            geo_country: Some("GB".into()),
            geo_confidence: Some(0.9),
            is_cdn_edge: true,
            cdn_provider: Some("cloudflare".into()),
            tls_profile: Some("chrome-131".into()),
            asn: Some(KNOWN_ASN_CLOUDFLARE),
            city: Some("London".into()),
            postal_code: Some("SW1A".into()),
            ip_class: IpClass::Mobile,
            target_compatibility: TargetVendorCompatibility::default(),
        };
        assert!(caps_proxy_datacenter.satisfies(&req));
        assert!(caps_proxy_unknown.satisfies(&req));
        assert!(caps_proxy_full.satisfies(&req));
    }

    #[test]
    fn capability_requirement_target_vendor_blocks_free_list_pool() {
        // A free-list proxy has every vendor marked Blocked; a
        // target_vendor requirement rejects it (fail-secure).
        let caps = ProxyCapabilities {
            ip_class: IpClass::Datacenter,
            target_compatibility: TargetVendorCompatibility::default_blocked(),
            ..Default::default()
        };
        let req = CapabilityRequirement {
            target_vendor: Some(VendorId::DataDome),
            ..Default::default()
        };
        assert!(
            !caps.satisfies(&req),
            "free-list proxy must not satisfy a DataDome vendor requirement"
        );
    }

    #[test]
    fn capability_requirement_target_vendor_accepts_acceptable_tier() {
        // A proxy with `defeats[DataDome] = Acceptable` passes the
        // target_vendor requirement (Blocked is the only disqualifier).
        let caps = ProxyCapabilities {
            target_compatibility: TargetVendorCompatibility::default()
                .set(VendorId::DataDome, TrustTier::Acceptable),
            ..Default::default()
        };
        let req = CapabilityRequirement {
            target_vendor: Some(VendorId::DataDome),
            ..Default::default()
        };
        assert!(caps.satisfies(&req));
    }

    #[test]
    fn capability_requirement_require_ip_class_fails_for_datacenter() {
        // An ISP requirement must reject a Datacenter proxy.
        let caps = ProxyCapabilities {
            ip_class: IpClass::Datacenter,
            ..Default::default()
        };
        let req = CapabilityRequirement {
            require_ip_class: Some(IpClassRequirement {
                minimum: IpClass::Isp,
            }),
            ..Default::default()
        };
        assert!(!caps.satisfies(&req));
    }

    #[test]
    fn capability_requirement_require_ip_class_fails_for_unknown() {
        // An ISP requirement must reject an Unknown proxy (Unknown rank 0).
        let caps = ProxyCapabilities::default();
        let req = CapabilityRequirement {
            require_ip_class: Some(IpClassRequirement {
                minimum: IpClass::Isp,
            }),
            ..Default::default()
        };
        assert!(!caps.satisfies(&req));
    }

    // ── Proxy backward compatibility ────────────────────────────────────────

    /// Existing `Proxy` literals (from the `make_proxy` test helper in
    /// storage/manager/etc.) build with `IpClass::Unknown` defaults.
    #[test]
    fn proxy_default_ip_class_is_unknown() {
        let proxy = Proxy {
            url: "http://example.test:8080".into(),
            proxy_type: ProxyType::Http,
            username: None,
            password: None,
            weight: 1,
            tags: vec![],
            capabilities: ProxyCapabilities::default(),
            ip_class: IpClass::Unknown,
            target_compatibility: TargetVendorCompatibility::default(),
        };
        assert_eq!(proxy.ip_class, IpClass::Unknown);
        assert!(proxy.target_compatibility.is_empty());
    }

    /// Existing serialised proxies deserialize cleanly with the new
    /// `IpClass::Unknown` and empty compatibility.
    #[test]
    fn proxy_legacy_serde_backward_compatibility() {
        // Pre-T95 wire format: no `ip_class`, no `target_compatibility`.
        let legacy = r#"{
            "url": "http://legacy.test:8080",
            "proxy_type": "http",
            "username": null,
            "password": null,
            "weight": 1,
            "tags": [],
            "capabilities": {}
        }"#;
        let parsed: Proxy = serde_json::from_str(legacy).expect("legacy parses");
        assert_eq!(parsed.url, "http://legacy.test:8080");
        assert_eq!(parsed.ip_class, IpClass::Unknown);
        assert!(parsed.target_compatibility.is_empty());
    }

    // ── T98: ASN / city / postal_code capability fields ────────────────────

    /// Headline round-trip: every new T98 field populates and
    /// deserialises back identically.
    #[test]
    fn t98_capabilities_round_trip_through_json_with_all_geo_fields() {
        let original = ProxyCapabilities {
            asn: Some(KNOWN_ASN_CLOUDFLARE),
            city: Some("San Francisco".into()),
            postal_code: Some("94110".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let parsed: ProxyCapabilities = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, original);
    }

    /// Round-trip with all three T98 fields set to `None` (the
    /// default) — confirms the `#[serde(default)]` annotations
    /// preserve the no-enrichment path.
    #[test]
    fn t98_capabilities_round_trip_through_json_with_none_geo_fields() {
        let original = ProxyCapabilities::default();
        let json = serde_json::to_string(&original).expect("serialize");
        let parsed: ProxyCapabilities = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, original);
        assert!(parsed.asn.is_none());
        assert!(parsed.city.is_none());
        assert!(parsed.postal_code.is_none());
    }

    /// `#[serde(default)]` on the new fields means a pre-T98 wire
    /// payload (no `asn` / `city` / `postal_code` keys) deserialises
    /// cleanly with `None` values.
    #[test]
    fn t98_capabilities_legacy_wire_payload_deserialises_to_none() {
        // Pre-T98 wire format: no `asn`, no `city`, no `postal_code`.
        let legacy = "{}";
        let parsed: ProxyCapabilities = serde_json::from_str(legacy).expect("legacy parses");
        assert!(parsed.asn.is_none());
        assert!(parsed.city.is_none());
        assert!(parsed.postal_code.is_none());
    }

    /// `CapabilityRequirement` round-trips with all three new fields
    /// populated; the `skip_serializing_if = "Option::is_none"` pattern
    /// keeps the wire form compact for empty requirements.
    #[test]
    fn t98_capability_requirement_round_trip_through_json_with_geo_fields() {
        let original = CapabilityRequirement {
            require_asn: Some(KNOWN_ASN_AKAMAI),
            require_city: Some("London".into()),
            require_postal_code: Some("SW1A".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let parsed: CapabilityRequirement = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, original);
    }

    /// Empty requirement still serialises to `{}` (not the
    /// pre-T98 wire form) and matches every proxy.
    #[test]
    fn t98_empty_requirement_matches_any_proxy() {
        let req = CapabilityRequirement::default();
        let json = serde_json::to_string(&req).expect("serialize");
        // All `Option` fields are skipped → `{}` (or whatever the
        // existing default-derive shape is).
        let parsed: CapabilityRequirement = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, req);
        // And it still satisfies every proxy variant.
        for caps in [
            ProxyCapabilities::default(),
            ProxyCapabilities {
                asn: Some(KNOWN_ASN_CLOUDFLARE),
                city: Some("Anywhere".into()),
                postal_code: Some("00000".into()),
                ..Default::default()
            },
        ] {
            assert!(caps.satisfies(&req));
        }
    }

    /// `require_asn` exact-match: a Cloudflare-tagged proxy matches
    /// `require_asn = Some(CLOUDFLARE)`; an Akamai-tagged proxy does
    /// not.
    #[test]
    fn t98_require_asn_exact_match() {
        let req = CapabilityRequirement {
            require_asn: Some(KNOWN_ASN_CLOUDFLARE),
            ..Default::default()
        };
        let cf_caps = ProxyCapabilities {
            asn: Some(KNOWN_ASN_CLOUDFLARE),
            ..Default::default()
        };
        let ak_caps = ProxyCapabilities {
            asn: Some(KNOWN_ASN_AKAMAI),
            ..Default::default()
        };
        let none_caps = ProxyCapabilities::default();
        assert!(cf_caps.satisfies(&req));
        assert!(!ak_caps.satisfies(&req));
        assert!(!none_caps.satisfies(&req));
    }

    /// `require_city` exact-match: "San Francisco" matches; "Berlin"
    /// and `None` do not.
    #[test]
    fn t98_require_city_exact_match() {
        let req = CapabilityRequirement {
            require_city: Some("San Francisco".into()),
            ..Default::default()
        };
        let sf_caps = ProxyCapabilities {
            city: Some("San Francisco".into()),
            ..Default::default()
        };
        let b_caps = ProxyCapabilities {
            city: Some("Berlin".into()),
            ..Default::default()
        };
        let none_caps = ProxyCapabilities::default();
        assert!(sf_caps.satisfies(&req));
        assert!(!b_caps.satisfies(&req));
        assert!(!none_caps.satisfies(&req));
    }

    /// `require_postal_code` exact-match: "94110" matches; "10001"
    /// and `None` do not.
    #[test]
    fn t98_require_postal_code_exact_match() {
        let req = CapabilityRequirement {
            require_postal_code: Some("94110".into()),
            ..Default::default()
        };
        let sf_caps = ProxyCapabilities {
            postal_code: Some("94110".into()),
            ..Default::default()
        };
        let ny_caps = ProxyCapabilities {
            postal_code: Some("10001".into()),
            ..Default::default()
        };
        let none_caps = ProxyCapabilities::default();
        assert!(sf_caps.satisfies(&req));
        assert!(!ny_caps.satisfies(&req));
        assert!(!none_caps.satisfies(&req));
    }

    /// Composite filter — the headline "Akamai scrape: Cloudflare AS +
    /// SF city + 94110 ZIP" example from the task spec.
    #[test]
    fn t98_composite_geo_filter_akamai_scrape() {
        let req = CapabilityRequirement {
            require_asn: Some(KNOWN_ASN_CLOUDFLARE),
            require_city: Some("San Francisco".into()),
            require_postal_code: Some("94110".into()),
            ..Default::default()
        };
        let matching = ProxyCapabilities {
            asn: Some(KNOWN_ASN_CLOUDFLARE),
            city: Some("San Francisco".into()),
            postal_code: Some("94110".into()),
            ..Default::default()
        };
        let wrong_asn = ProxyCapabilities {
            asn: Some(KNOWN_ASN_OVH),
            city: Some("San Francisco".into()),
            postal_code: Some("94110".into()),
            ..Default::default()
        };
        let wrong_city = ProxyCapabilities {
            asn: Some(KNOWN_ASN_CLOUDFLARE),
            city: Some("Oakland".into()),
            postal_code: Some("94110".into()),
            ..Default::default()
        };
        let wrong_zip = ProxyCapabilities {
            asn: Some(KNOWN_ASN_CLOUDFLARE),
            city: Some("San Francisco".into()),
            postal_code: Some("94609".into()),
            ..Default::default()
        };
        assert!(matching.satisfies(&req));
        assert!(!wrong_asn.satisfies(&req));
        assert!(!wrong_city.satisfies(&req));
        assert!(!wrong_zip.satisfies(&req));
    }

    /// Round-trip through TOML for both `ProxyCapabilities` and
    /// `CapabilityRequirement` — covers operators that store config in
    /// `stygian.toml` files.
    #[test]
    fn t98_capabilities_and_requirement_round_trip_through_toml() {
        let caps = ProxyCapabilities {
            asn: Some(KNOWN_ASN_FASTLY),
            city: Some("Berlin".into()),
            postal_code: Some("10115".into()),
            ..Default::default()
        };
        let caps_toml = toml::to_string(&caps).expect("serialize caps toml");
        let parsed_caps: ProxyCapabilities =
            toml::from_str(&caps_toml).expect("deserialize caps toml");
        assert_eq!(parsed_caps, caps);

        let req = CapabilityRequirement {
            require_asn: Some(KNOWN_ASN_FASTLY),
            require_city: Some("Berlin".into()),
            ..Default::default()
        };
        let req_toml = toml::to_string(&req).expect("serialize req toml");
        let parsed_req: CapabilityRequirement =
            toml::from_str(&req_toml).expect("deserialize req toml");
        assert_eq!(parsed_req, req);
    }

    /// `#[serde(default, skip_serializing_if = "Option::is_none")]` on
    /// the new requirement fields means a legacy requirement payload
    /// (no `require_asn` / `require_city` / `require_postal_code`
    /// keys) deserialises cleanly with `None` values.
    #[test]
    fn t98_capability_requirement_legacy_wire_payload_deserialises_to_none() {
        let legacy = r#"{
            "require_https_connect": false,
            "require_socks5_udp": false,
            "require_http3_tunnel": false,
            "require_geo_country": null,
            "require_cdn_edge": false,
            "require_tls_profile": null
        }"#;
        let parsed: CapabilityRequirement = serde_json::from_str(legacy).expect("legacy parses");
        assert!(parsed.require_asn.is_none());
        assert!(parsed.require_city.is_none());
        assert!(parsed.require_postal_code.is_none());
    }

    // ── T98: well_known ASN constants ───────────────────────────────────────

    /// The `well_known` constants match the documented public ASNs.
    #[test]
    fn t98_well_known_asns_match_documented_values() {
        assert_eq!(super::well_known::KNOWN_ASN_CLOUDFLARE, 13_335);
        assert_eq!(super::well_known::KNOWN_ASN_AKAMAI, 20_940);
        assert_eq!(super::well_known::KNOWN_ASN_FASTLY, 54_113);
        assert_eq!(super::well_known::KNOWN_ASN_CLOUDFRONT, 16_509);
        assert_eq!(super::well_known::KNOWN_ASN_GOOGLE, 15_169);
        assert_eq!(super::well_known::KNOWN_ASN_AZURE, 8075);
    }

    /// `ALL_KNOWN_ASNS` is a non-empty slice; every constant is
    /// unique (no duplicates); the well-known CDNs are present.
    #[test]
    fn t98_well_known_all_known_asns_includes_major_cdns() {
        let all = super::well_known::ALL_KNOWN_ASNS;
        assert!(all.contains(&KNOWN_ASN_CLOUDFLARE));
        assert!(all.contains(&KNOWN_ASN_AKAMAI));
        assert!(all.contains(&KNOWN_ASN_FASTLY));
        assert!(all.contains(&super::well_known::KNOWN_ASN_CLOUDFRONT));
        // No duplicates.
        let mut sorted = all.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len());
    }

    // ── T98: ingest validation helpers ─────────────────────────────────────

    /// `validate_asn` accepts every valid public ASN.
    #[test]
    fn t98_validate_asn_accepts_valid_asns() {
        assert!(super::validate_asn(1).is_ok());
        assert!(super::validate_asn(KNOWN_ASN_CLOUDFLARE).is_ok());
        assert!(super::validate_asn(u32::MAX - 1).is_ok());
    }

    /// `validate_asn` rejects the reserved `0` and `u32::MAX` values.
    #[test]
    fn t98_validate_asn_rejects_reserved_values() {
        let err_zero = super::validate_asn(0).expect_err("0 should fail");
        assert!(matches!(
            err_zero,
            crate::error::ProxyError::InvalidGeoMetadata { ref field, .. } if field == "asn"
        ));
        let err_max = super::validate_asn(u32::MAX).expect_err("u32::MAX should fail");
        assert!(matches!(
            err_max,
            crate::error::ProxyError::InvalidGeoMetadata { ref field, .. } if field == "asn"
        ));
    }

    /// `validate_city` rejects empty and over-length strings; accepts
    /// the documented length range `[1, 100]`.
    #[test]
    fn t98_validate_city_enforces_length_bounds() {
        assert!(super::validate_city("").is_err());
        assert!(super::validate_city("A").is_ok());
        assert!(super::validate_city("San Francisco").is_ok());
        let over = "x".repeat(super::CITY_MAX_LEN + 1);
        assert!(super::validate_city(&over).is_err());
    }

    /// `validate_postal_code` enforces the `[1, 16]` length ceiling
    /// without enforcing a country-specific format.
    #[test]
    fn t98_validate_postal_code_enforces_length_bounds() {
        assert!(super::validate_postal_code("").is_err());
        assert!(super::validate_postal_code("94110").is_ok());
        assert!(super::validate_postal_code("SW1A 1AA").is_ok());
        assert!(super::validate_postal_code("100-0001").is_ok());
        let over = "x".repeat(super::POSTAL_CODE_MAX_LEN + 1);
        assert!(super::validate_postal_code(&over).is_err());
    }
}
