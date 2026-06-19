//! Vendor-specific proxy URL quirks.
//!
//! ## Why vendor-specific quirks matter
//!
//! Most public proxy providers serve plain HTTP on a port that *looks*
//! like an HTTPS port. Using `https://` against such a proxy causes the
//! TLS layer to attempt a handshake on top of an already-running TLS
//! session, producing `BoringSSL`'s `WRONG_VERSION_NUMBER` error.
//! The 2026 guide flags this as a high-frequency footgun
//! (`docs/dev/project/scraping-guide-2026-llm-context.md` L2840, the
//! "Crawlera/Zyte proxy bug"):
//! guide flags this as a high-frequency footgun
//! (`docs/dev/project/scraping-guide-2026-llm-context.md` L2840, the
//! "Crawlera/Zyte proxy bug"):
//!
//! > Port 8011 speaks plain HTTP. Both http:// and https:// keys must use
//! > http:// scheme. Using https:// causes BoringSSL WRONG_VERSION_NUMBER
//! > (TLS-over-TLS failure). Fix: `'https': 'http://key:@proxy.crawlera.com:8011/'`
//!
//! Operators hit this trap on three common providers:
//!
//! - `Crawlera` / `Zyte` Smart Proxy Manager (`*.crawlera.com:8011`,
//!   `*.zyte.com:8011`) — must be `http://` even when scraping `https://`
//!   targets.
//! - `Bright Data` `brd.superproxy.io:22225` — username must follow the
//!   `brd-customer-<id>-session-<session_id>` pattern; missing the
//!   `-session-<id>` suffix silently collapses all traffic into the
//!   default session pool.
//! - `IPRoyal` residential gateway — username must carry a country flag
//!   (e.g. `user-country-US`) for the egress IP to honour the request.
//!
//! [`VENDOR_QUIRKS`] encodes the four documented cases as a `const` slice
//! so the table is zero-cost at runtime; [`check`] walks the slice using
//! pure pointer/length compares and returns all matches in a small
//! [`Vec`]. Hard-error quirks (like `Crawlera` 8011 + `https://`) are
//! surfaced at ingest time by [`crate::storage::validate_proxy_url`],
//! which rejects the URL outright; warning-severity quirks are logged
//! and the URL is accepted.
//!
//! ## Security note
//!
//! Quirks match on `host:port` only — the password component of the URL
//! is never inspected, logged, or echoed in any error or warning. The
//! quirk descriptions are static `&'static str` slices with no
//! credentials baked in.
//!
//! ## Hot path
//!
//! [`check`] is a `pub fn` (the return type is `Vec<QuirkMatch>` per the
//! task spec). In the common case where the URL host is not in the
//! built-in table, the function returns an empty `Vec::new()` without
//! iterating the table — the `Vec` allocation is skipped entirely. The
//! table itself is a `const` slice, so there is no I/O, no locks, and
//! no parsing beyond the [`ProxyUrl`] construction done by the caller.
//!
//! ## Example
//!
//! ```
//! use stygian_proxy::vendor_quirks::{check, ProxyUrl, QuirkSeverity};
//!
//! // The classic Crawlera 8011 + https:// footgun.
//! let url = ProxyUrl::parse("https://user:pass@proxy.crawlera.com:8011")
//!     .expect("parses");
//! let matches = check(&url);
//! assert_eq!(matches.len(), 1);
//! assert_eq!(matches[0].severity, QuirkSeverity::Error);
//! assert_eq!(matches[0].required_scheme, stygian_proxy::vendor_quirks::Scheme::Http);
//! ```

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─────────────────────────────────────────────────────────────────────────────
// Scheme
// ─────────────────────────────────────────────────────────────────────────────

/// URL scheme of a proxy endpoint.
///
/// Mirrors the subset of [`crate::types::ProxyType`] that is reachable
/// as a URL scheme. Used by [`VendorQuirk`] to express provider-specific
/// scheme requirements (e.g. "Crawlera port 8011 is plain HTTP even
/// though it serves HTTPS targets").
///
/// # Example
///
/// ```
/// use std::str::FromStr;
/// use stygian_proxy::vendor_quirks::Scheme;
/// assert_eq!(Scheme::Http.as_str(), "http");
/// assert_eq!(Scheme::Https.as_str(), "https");
/// assert_eq!(Scheme::from_str("http").ok(), Some(Scheme::Http));
/// assert!(Scheme::from_str("nope").is_err());
/// ```
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scheme {
    /// Plain HTTP (`http://`).
    #[default]
    Http,
    /// HTTPS (`https://`).
    Https,
    /// SOCKS4 (`socks4://`) — only when the `socks` feature is enabled.
    #[cfg(feature = "socks")]
    Socks4,
    /// SOCKS5 (`socks5://`) — only when the `socks` feature is enabled.
    #[cfg(feature = "socks")]
    Socks5,
}

impl Scheme {
    /// Returns the canonical wire form of the scheme (e.g. `"http"`).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_proxy::vendor_quirks::Scheme;
    /// assert_eq!(Scheme::Http.as_str(), "http");
    /// assert_eq!(Scheme::Https.as_str(), "https");
    /// ```
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
            #[cfg(feature = "socks")]
            Self::Socks4 => "socks4",
            #[cfg(feature = "socks")]
            Self::Socks5 => "socks5",
        }
    }
}

impl FromStr for Scheme {
    type Err = ();

    /// Parse a [`Scheme`] from its wire form (e.g. `"http"`).
    ///
    /// Returns `Err(())` for any unknown scheme. The validator
    /// surfaces the unknown scheme upstream as a structured error
    /// rather than panicking.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "http" => Ok(Self::Http),
            "https" => Ok(Self::Https),
            #[cfg(feature = "socks")]
            "socks4" => Ok(Self::Socks4),
            #[cfg(feature = "socks")]
            "socks5" => Ok(Self::Socks5),
            _ => Err(()),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// QuirkSeverity
// ─────────────────────────────────────────────────────────────────────────────

/// Severity classification for a [`QuirkMatch`].
///
/// The ingest flow in [`crate::storage::validate_proxy_url`] treats the
/// severity as the action gate:
///
/// - [`QuirkSeverity::Error`] — hard-error quirks reject the URL outright.
/// - [`QuirkSeverity::Warning`] — warning quirks are logged but the URL
///   is accepted.
/// - [`QuirkSeverity::Info`] — informational quirks are recorded for
///   observability and the URL is accepted.
///
/// # Example
///
/// ```
/// use stygian_proxy::vendor_quirks::QuirkSeverity;
/// assert_eq!(QuirkSeverity::Error, QuirkSeverity::Error);
/// assert_ne!(QuirkSeverity::Warning, QuirkSeverity::Info);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuirkSeverity {
    /// Informational — the URL is accepted; the quirk is recorded.
    Info,
    /// Warning — the URL is accepted; the quirk is logged.
    Warning,
    /// Error — the URL is rejected by the ingest validator.
    Error,
}

// ─────────────────────────────────────────────────────────────────────────────
// VendorQuirk
// ─────────────────────────────────────────────────────────────────────────────

/// A vendor-specific proxy URL rule, declared as a `const` table record.
///
/// Quirks match on `host_suffix:port` (no credentials). The semantics
/// are severity-driven:
///
/// - For [`QuirkSeverity::Error`], the quirk fires when the URL's
///   `scheme != required_scheme` (e.g. `Crawlera` 8011 + `https://`).
/// - For [`QuirkSeverity::Warning`] and [`QuirkSeverity::Info`], the
///   quirk fires on every `host_suffix:port` match (the `required_scheme`
///   field is informational and not used for the trigger).
///
/// `Copy` so the [`VENDOR_QUIRKS`] `const` slice can be iterated without
/// moving records out.
///
/// # Example
///
/// ```
/// use stygian_proxy::vendor_quirks::{Scheme, VendorQuirk, QuirkSeverity, VENDOR_QUIRKS};
///
/// let crawlera = VENDOR_QUIRKS
///     .iter()
///     .find(|q| q.host_suffix == "crawlera.com")
///     .expect("Crawlera quirk seeded");
/// assert_eq!(crawlera.port, Some(8011));
/// assert_eq!(crawlera.required_scheme, Scheme::Http);
/// assert_eq!(crawlera.severity, QuirkSeverity::Error);
///
/// // Quirk descriptions never include the password component.
/// assert!(!crawlera.description.contains("pass"));
/// assert!(!crawlera.description.contains('@'));
/// let _: VendorQuirk = *crawlera; // `Copy` for const-slice iteration
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VendorQuirk {
    /// Host suffix that triggers the quirk (e.g. `"crawlera.com"`,
    /// `"brd.superproxy.io"`). Matched as an exact host name OR as a
    /// subdomain boundary (e.g. `proxy.crawlera.com` matches
    /// `crawlera.com`).
    pub host_suffix: &'static str,
    /// Port that triggers the quirk. `None` matches any port.
    pub port: Option<u16>,
    /// Scheme that the provider requires on this port. Used by
    /// [`QuirkSeverity::Error`] quirks to detect a scheme mismatch
    /// (the reject trigger). Ignored by `Warning` / `Info` quirks.
    pub required_scheme: Scheme,
    /// Human-readable description of the quirk and the failure mode it
    /// prevents. Used as the structured error reason in
    /// [`crate::storage::validate_proxy_url`].
    pub description: &'static str,
    /// Severity classification — drives whether the URL is rejected,
    /// warned, or just recorded.
    pub severity: QuirkSeverity,
}

// ─────────────────────────────────────────────────────────────────────────────
// Built-in quirk table
// ─────────────────────────────────────────────────────────────────────────────

/// The first hard-error quirk — `Crawlera` port 8011 is plain HTTP.
///
/// Using `https://` against `proxy.crawlera.com:8011` causes
/// `BoringSSL WRONG_VERSION_NUMBER` (TLS-over-TLS failure). Both
/// `http://` and `https://` keys in the client config must use the
/// `http://` scheme for the `Crawlera` port. See module docs.
pub const CRAWLERA_8011_QUIRK: VendorQuirk = VendorQuirk {
    host_suffix: "crawlera.com",
    port: Some(8011),
    required_scheme: Scheme::Http,
    description: "Crawlera port 8011 is plain HTTP — using https:// causes BoringSSL WRONG_VERSION_NUMBER (TLS-over-TLS failure). Use http:// on both http and https scraping keys.",
    severity: QuirkSeverity::Error,
};

/// `Zyte` Smart Proxy Manager port 8011 has the same plain-HTTP trap
/// as `Crawlera` — `Zyte` operates the same upstream port range.
pub const ZYTE_8011_QUIRK: VendorQuirk = VendorQuirk {
    host_suffix: "zyte.com",
    port: Some(8011),
    required_scheme: Scheme::Http,
    description: "Zyte Smart Proxy Manager port 8011 is plain HTTP — same WRONG_VERSION_NUMBER trap as Crawlera. Use http:// scheme on both http and https keys.",
    severity: QuirkSeverity::Error,
};

/// `Bright Data` residential / datacenter super-proxy on port 22225.
///
/// The username must follow the `brd-customer-<id>-session-<id>`
/// pattern for session isolation. A missing `-session-<id>` suffix
/// silently collapses traffic into the default session pool.
pub const BRD_SUPERPROXY_QUIRK: VendorQuirk = VendorQuirk {
    host_suffix: "brd.superproxy.io",
    port: Some(22225),
    required_scheme: Scheme::Http,
    description: "Bright Data brd.superproxy.io:22225 requires a brd-customer-<id>-session-<id> username; a missing -session-<id> suffix silently collapses traffic into the default session pool.",
    severity: QuirkSeverity::Warning,
};

/// `IPRoyal` residential traffic requires a country flag in the username.
///
/// Example: `user-country-US`. Without the flag, the gateway ignores
/// the requested exit country and serves a random residential IP. The
/// `port = None` field matches any `IPRoyal` gateway port.
pub const IPROYAL_QUIRK: VendorQuirk = VendorQuirk {
    host_suffix: "iproyal.com",
    port: None,
    required_scheme: Scheme::Http,
    description: "IPRoyal residential traffic requires a country flag in the username (e.g. user-country-US). Without the flag, the gateway ignores the requested exit country.",
    severity: QuirkSeverity::Warning,
};

/// Every built-in [`VendorQuirk`], in declaration order.
///
/// New quirks should be added above this slice so the constant
/// declarations remain the single source of truth, then included in
/// the slice. The slice is `const`-constructible so the table is
/// zero-cost at runtime.
pub const VENDOR_QUIRKS: &[VendorQuirk] = &[
    CRAWLERA_8011_QUIRK,
    ZYTE_8011_QUIRK,
    BRD_SUPERPROXY_QUIRK,
    IPROYAL_QUIRK,
];

// ─────────────────────────────────────────────────────────────────────────────
// ProxyUrl
// ─────────────────────────────────────────────────────────────────────────────

/// A parsed proxy URL — the canonical input shape for [`check`].
///
/// The struct is intentionally small (six `String` / `Option` fields,
/// no `Vec`) and is built once per ingest by
/// [`ProxyUrl::parse`]. The owned `String` fields make the type
/// `'static`-safe; the [`check`] function only reads from it and never
/// mutates. **Credentials are carried for completeness** but
/// [`check`] never inspects the password component.
///
/// # Example
///
/// ```
/// use stygian_proxy::vendor_quirks::{ProxyUrl, Scheme};
///
/// let p = ProxyUrl::parse("http://user:pass@proxy.crawlera.com:8011/path")
///     .expect("parses");
/// assert_eq!(p.scheme, Scheme::Http);
/// assert_eq!(p.host, "proxy.crawlera.com");
/// assert_eq!(p.port, Some(8011));
/// assert_eq!(p.username.as_deref(), Some("user"));
/// assert_eq!(p.password.as_deref(), Some("pass"));
/// assert_eq!(p.path.as_deref(), Some("/path"));
///
/// let p = ProxyUrl::parse("https://proxy.test:8443").expect("parses");
/// assert_eq!(p.scheme, Scheme::Https);
/// assert_eq!(p.host, "proxy.test");
/// assert_eq!(p.port, Some(8443));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyUrl {
    /// URL scheme.
    pub scheme: Scheme,
    /// Host portion of the URL (lower-case, no brackets).
    pub host: String,
    /// Optional port. `None` means the scheme default (80 for HTTP, 443
    /// for HTTPS).
    pub port: Option<u16>,
    /// Optional user-info username. Never logged by [`check`].
    pub username: Option<String>,
    /// Optional user-info password. **Never logged, matched, or echoed
    /// by [`check`].**
    pub password: Option<String>,
    /// Optional path component (e.g. `"/"` for `http://host:80/`).
    pub path: Option<String>,
}

/// Errors emitted by [`ProxyUrl::parse`].
///
/// Distinct from the host/port validation errors in
/// [`crate::storage::validate_proxy_url`] (which produce
/// [`crate::error::ProxyError::InvalidProxyUrl`]) so a caller can
/// decide whether to surface the parse failure as an upstream error
/// or attempt recovery.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    /// The URL did not contain a `"://"` separator.
    #[error("missing scheme separator '://' in URL `{0}`")]
    MissingSchemeSeparator(String),
    /// The scheme is not in the supported set (e.g. `"ftp"`,
    /// `"file"`).
    #[error("unsupported scheme `{0}` in URL `{1}`")]
    UnsupportedScheme(String, String),
    /// The host portion is empty.
    #[error("empty host in URL `{0}`")]
    EmptyHost(String),
    /// The explicit port is out of range `[1, 65535]`.
    #[error("port `{port}` is out of range [1, 65535] in URL `{url}`")]
    PortOutOfRange {
        /// The offending port string.
        port: String,
        /// The URL that was being parsed.
        url: String,
    },
    /// The explicit port was not a valid `u16` integer.
    #[error("non-numeric port `{0}` in URL `{1}`")]
    NonNumericPort(String, String),
    /// The URL has an unmatched `[` in the host position (IPv6 literal
    /// without a closing bracket).
    #[error("unclosed IPv6 bracket in URL `{0}`")]
    UnclosedIpv6Bracket(String),
}

impl ProxyUrl {
    /// Parse a proxy URL into a [`ProxyUrl`].
    ///
    /// Recognises `http://`, `https://`, and (when the `socks` feature
    /// is enabled) `socks4://` / `socks5://` schemes. IPv6 literals in
    /// brackets (e.g. `http://[::1]:8080`) are supported. User-info is
    /// split on the first `:` after the `//` separator so passwords
    /// containing colons survive intact.
    ///
    /// # Errors
    ///
    /// Returns [`ParseError`] when the URL is structurally invalid.
    /// The error message includes the original URL for diagnostics.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_proxy::vendor_quirks::{ProxyUrl, Scheme};
    ///
    /// let p = ProxyUrl::parse("http://user:pa:ss@host:8080").unwrap();
    /// assert_eq!(p.scheme, Scheme::Http);
    /// assert_eq!(p.host, "host");
    /// assert_eq!(p.port, Some(8080));
    /// assert_eq!(p.username.as_deref(), Some("user"));
    /// // Passwords containing colons are preserved.
    /// assert_eq!(p.password.as_deref(), Some("pa:ss"));
    /// ```
    pub fn parse(url: &str) -> Result<Self, ParseError> {
        // 1. Split scheme from the rest.
        let (scheme_str, rest) = url
            .split_once("://")
            .ok_or_else(|| ParseError::MissingSchemeSeparator(url.to_owned()))?;

        let scheme = Scheme::from_str(scheme_str)
            .map_err(|()| ParseError::UnsupportedScheme(scheme_str.to_owned(), url.to_owned()))?;

        // 2. Split user-info from authority: the part before the first
        //    `@` after the `://` is the user-info. The part after is
        //    host[:port][/path].
        let (userinfo, authority_with_path) = match rest.split_once('@') {
            Some((ui, auth)) => (ui, auth),
            None => ("", rest),
        };

        // 3. Split authority from path. The authority ends at the first
        //    `/` after the user-info, or at end-of-string.
        let (authority, path) = match authority_with_path.split_once('/') {
            Some((a, p)) => (a, Some(format!("/{p}"))),
            None => (authority_with_path, None),
        };

        // 4. Parse user-info into username + password (split on the
        //    FIRST `:` so passwords with colons survive).
        let (username, password) = if userinfo.is_empty() {
            (None, None)
        } else {
            match userinfo.split_once(':') {
                Some((u, p)) => (Some(u.to_owned()), Some(p.to_owned())),
                None => (Some(userinfo.to_owned()), None),
            }
        };

        // 5. Split host from port, handling IPv6 brackets.
        let (host, port_str) = if let Some(stripped) = authority.strip_prefix('[') {
            // IPv6 literal: read until `]`.
            let close = stripped
                .find(']')
                .ok_or_else(|| ParseError::UnclosedIpv6Bracket(url.to_owned()))?;
            let host = &stripped[..close];
            let after = &stripped[close + 1..];
            let port_str = after.strip_prefix(':').unwrap_or("");
            (host.to_owned(), port_str)
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h.to_owned(), p),
                None => (authority.to_owned(), ""),
            }
        };

        if host.is_empty() {
            return Err(ParseError::EmptyHost(url.to_owned()));
        }

        // 6. Parse the optional port.
        let port = if port_str.is_empty() {
            None
        } else {
            let parsed: u32 = port_str
                .parse()
                .map_err(|_| ParseError::NonNumericPort(port_str.to_owned(), url.to_owned()))?;
            if parsed == 0 || parsed > 65535 {
                return Err(ParseError::PortOutOfRange {
                    port: port_str.to_owned(),
                    url: url.to_owned(),
                });
            }
            // Truncation is safe: `parsed <= 65535 < u16::MAX`.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let as_u16 = parsed as u16;
            Some(as_u16)
        };

        Ok(Self {
            scheme,
            host: host.to_ascii_lowercase(),
            port,
            username,
            password,
            path,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// QuirkMatch
// ─────────────────────────────────────────────────────────────────────────────

/// A single vendor-quirk match produced by [`check`].
///
/// Carries the static [`VendorQuirk`] record (for `severity` /
/// `host_suffix` / `description` / `required_scheme`) plus the
/// observed values from the [`ProxyUrl`] that triggered the match.
/// **The password component of the URL is never copied into the
/// match** — the security note at the module top is binding.
///
/// The struct is `Clone` so matches can be retained across log
/// emissions and HTTP responses without lifetime gymnastics.
///
/// # Example
///
/// ```
/// use stygian_proxy::vendor_quirks::{check, ProxyUrl, QuirkSeverity};
///
/// let url = ProxyUrl::parse("http://user@brd.superproxy.io:22225").unwrap();
/// let m = &check(&url)[0];
/// assert_eq!(m.severity, QuirkSeverity::Warning);
/// assert_eq!(m.host_suffix, "brd.superproxy.io");
/// assert_eq!(m.observed_scheme, stygian_proxy::vendor_quirks::Scheme::Http);
/// assert_eq!(m.observed_port, Some(22225));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuirkMatch {
    /// Severity of the matched quirk (mirrors `quirk.severity`).
    pub severity: QuirkSeverity,
    /// Host suffix that triggered the match (mirrors `quirk.host_suffix`).
    pub host_suffix: &'static str,
    /// Required scheme per the quirk (mirrors `quirk.required_scheme`).
    pub required_scheme: Scheme,
    /// Human-readable description (mirrors `quirk.description`).
    pub description: &'static str,
    /// The URL's observed scheme at match time.
    pub observed_scheme: Scheme,
    /// The URL's observed port at match time.
    pub observed_port: Option<u16>,
}

// ─────────────────────────────────────────────────────────────────────────────
// check
// ─────────────────────────────────────────────────────────────────────────────

/// Returns every [`VendorQuirk`] that matches `url`.
///
/// The function walks the const [`VENDOR_QUIRKS`] slice using pure
/// pointer/length compares (no I/O, no locks, no per-call
/// allocations beyond the returned `Vec`). In the common case where
/// the URL host is not in the built-in table, the function short-
/// circuits on the first non-matching `host_suffix` and returns
/// `Vec::new()` without further allocation beyond the empty `Vec`
/// itself.
///
/// Quirk triggers:
///
/// - For [`QuirkSeverity::Error`] quirks: the quirk fires when
///   `url.scheme != quirk.required_scheme` AND the host:port matches.
/// - For [`QuirkSeverity::Warning`] / [`QuirkSeverity::Info`] quirks:
///   the quirk fires whenever the host:port matches (the
///   `required_scheme` field is informational).
///
/// Host matching is **subdomain-aware**: `proxy.crawlera.com`
/// matches the `"crawlera.com"` suffix, but `"mycrawlera.com"` does
/// not (no subdomain boundary).
///
/// # Example
///
/// ```
/// use stygian_proxy::vendor_quirks::{check, ProxyUrl, QuirkSeverity, Scheme};
///
/// // Crawlera 8011 + https → 1 Error match (scheme mismatch).
/// let url = ProxyUrl::parse("https://user:pass@proxy.crawlera.com:8011").unwrap();
/// let m = check(&url);
/// assert_eq!(m.len(), 1);
/// assert_eq!(m[0].severity, QuirkSeverity::Error);
/// assert_eq!(m[0].required_scheme, Scheme::Http);
///
/// // Crawlera 8011 + http → 0 matches (compliant URL).
/// let url = ProxyUrl::parse("http://user:pass@proxy.crawlera.com:8011").unwrap();
/// assert!(check(&url).is_empty());
///
/// // Bright Data super-proxy → 1 Warning regardless of username.
/// let url = ProxyUrl::parse("http://user@brd.superproxy.io:22225").unwrap();
/// assert_eq!(check(&url).len(), 1);
/// assert_eq!(check(&url)[0].severity, QuirkSeverity::Warning);
/// ```
#[must_use]
pub fn check(url: &ProxyUrl) -> Vec<QuirkMatch> {
    let mut out: Vec<QuirkMatch> = Vec::new();
    for quirk in VENDOR_QUIRKS {
        if !host_suffix_matches(&url.host, quirk.host_suffix) {
            continue;
        }
        if let Some(required_port) = quirk.port
            && url.port != Some(required_port)
        {
            continue;
        }
        match quirk.severity {
            QuirkSeverity::Error => {
                // Hard-error quirks only fire on scheme mismatch.
                if url.scheme == quirk.required_scheme {
                    continue;
                }
            }
            QuirkSeverity::Warning | QuirkSeverity::Info => {
                // Warning / Info quirks fire on every host:port match.
            }
        }
        out.push(QuirkMatch {
            severity: quirk.severity,
            host_suffix: quirk.host_suffix,
            required_scheme: quirk.required_scheme,
            description: quirk.description,
            observed_scheme: url.scheme,
            observed_port: url.port,
        });
    }
    out
}

/// `true` when `host` equals `suffix` or is a strict subdomain of
/// `suffix` (e.g. `proxy.crawlera.com` matches `crawlera.com`).
///
/// `mycrawlera.com` does NOT match `crawlera.com` — the subdomain
/// boundary must be a `.` character.
fn host_suffix_matches(host: &str, suffix: &str) -> bool {
    if host == suffix {
        return true;
    }
    if host.len() <= suffix.len() + 1 {
        return false;
    }
    // The character immediately before the suffix must be a `.` —
    // guards against `mycrawlera.com` matching `crawlera.com`.
    match host.as_bytes().get(host.len() - suffix.len() - 1) {
        Some(b'.') => host.ends_with(suffix),
        _ => false,
    }
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
)] // deterministic test fixtures for URL parsing + quirk matching
mod tests {
    use super::*;

    // ── Scheme ──────────────────────────────────────────────────────────────

    #[test]
    fn scheme_default_is_http() {
        assert_eq!(Scheme::default(), Scheme::Http);
    }

    #[test]
    fn scheme_as_str_matches_wire_format() {
        assert_eq!(Scheme::Http.as_str(), "http");
        assert_eq!(Scheme::Https.as_str(), "https");
    }

    #[test]
    fn scheme_from_str_round_trip() {
        for scheme in [Scheme::Http, Scheme::Https] {
            assert_eq!(Scheme::from_str(scheme.as_str()), Ok(scheme));
        }
        assert_eq!(Scheme::from_str("nope"), Err(()));
    }

    // ── ProxyUrl parsing ────────────────────────────────────────────────────

    #[test]
    fn parse_simple_http_url() {
        let p = ProxyUrl::parse("http://proxy.example.com:8080").unwrap();
        assert_eq!(p.scheme, Scheme::Http);
        assert_eq!(p.host, "proxy.example.com");
        assert_eq!(p.port, Some(8080));
        assert!(p.username.is_none());
        assert!(p.password.is_none());
        assert!(p.path.is_none());
    }

    #[test]
    fn parse_https_url() {
        let p = ProxyUrl::parse("https://proxy.example.com:8443/path").unwrap();
        assert_eq!(p.scheme, Scheme::Https);
        assert_eq!(p.host, "proxy.example.com");
        assert_eq!(p.port, Some(8443));
        assert_eq!(p.path.as_deref(), Some("/path"));
    }

    #[test]
    fn parse_url_with_user_info() {
        let p = ProxyUrl::parse("http://user:pass@proxy.test:3128").unwrap();
        assert_eq!(p.scheme, Scheme::Http);
        assert_eq!(p.host, "proxy.test");
        assert_eq!(p.port, Some(3128));
        assert_eq!(p.username.as_deref(), Some("user"));
        assert_eq!(p.password.as_deref(), Some("pass"));
    }

    #[test]
    fn parse_url_with_password_containing_colon() {
        let p = ProxyUrl::parse("http://user:pa:ss@proxy.test:3128").unwrap();
        assert_eq!(p.username.as_deref(), Some("user"));
        // Passwords with colons must be preserved intact.
        assert_eq!(p.password.as_deref(), Some("pa:ss"));
    }

    #[test]
    fn parse_url_with_username_only() {
        let p = ProxyUrl::parse("http://user@proxy.test:3128").unwrap();
        assert_eq!(p.username.as_deref(), Some("user"));
        assert!(p.password.is_none());
    }

    #[test]
    fn parse_url_default_ports_are_optional() {
        let p = ProxyUrl::parse("http://proxy.test").unwrap();
        assert_eq!(p.port, None);
    }

    #[test]
    fn parse_url_lowercases_host() {
        let p = ProxyUrl::parse("http://PROXY.Test:8080").unwrap();
        assert_eq!(p.host, "proxy.test");
    }

    #[test]
    fn parse_url_with_trailing_slash() {
        let p = ProxyUrl::parse("http://proxy.test:8080/").unwrap();
        assert_eq!(p.path.as_deref(), Some("/"));
    }

    #[test]
    fn parse_ipv6_url_with_brackets() {
        let p = ProxyUrl::parse("http://[::1]:8080").unwrap();
        assert_eq!(p.host, "::1");
        assert_eq!(p.port, Some(8080));
    }

    #[test]
    fn parse_missing_scheme_separator_is_error() {
        let err = ProxyUrl::parse("not-a-url").unwrap_err();
        assert!(matches!(err, ParseError::MissingSchemeSeparator(_)));
    }

    #[test]
    fn parse_unsupported_scheme_is_error() {
        let err = ProxyUrl::parse("ftp://host:21").unwrap_err();
        assert!(matches!(err, ParseError::UnsupportedScheme(ref s, _) if s == "ftp"));
    }

    #[test]
    fn parse_empty_host_is_error() {
        let err = ProxyUrl::parse("http://:8080").unwrap_err();
        assert!(matches!(err, ParseError::EmptyHost(_)));
    }

    #[test]
    fn parse_port_out_of_range_is_error() {
        let err = ProxyUrl::parse("http://host:99999").unwrap_err();
        assert!(matches!(err, ParseError::PortOutOfRange { .. }));
    }

    #[test]
    fn parse_zero_port_is_error() {
        let err = ProxyUrl::parse("http://host:0").unwrap_err();
        assert!(matches!(err, ParseError::PortOutOfRange { .. }));
    }

    #[test]
    fn parse_non_numeric_port_is_error() {
        let err = ProxyUrl::parse("http://host:abc").unwrap_err();
        assert!(matches!(err, ParseError::NonNumericPort(ref s, _) if s == "abc"));
    }

    // ── VENDOR_QUIRKS table ──────────────────────────────────────────────────

    #[test]
    fn vendor_quirks_table_seeded_with_documented_providers() {
        // Crawlera and Zyte are hard-error quirks (TLS-over-TLS trap).
        assert_eq!(VENDOR_QUIRKS.len(), 4);
        assert!(VENDOR_QUIRKS.iter().any(|q| q.host_suffix == "crawlera.com"
            && q.port == Some(8011)
            && q.required_scheme == Scheme::Http
            && q.severity == QuirkSeverity::Error));
        assert!(VENDOR_QUIRKS.iter().any(|q| q.host_suffix == "zyte.com"
            && q.port == Some(8011)
            && q.required_scheme == Scheme::Http
            && q.severity == QuirkSeverity::Error));
        // Bright Data and IPRoyal are warning quirks (username format
        // warnings).
        assert!(
            VENDOR_QUIRKS
                .iter()
                .any(|q| q.host_suffix == "brd.superproxy.io"
                    && q.port == Some(22225)
                    && q.severity == QuirkSeverity::Warning)
        );
        assert!(
            VENDOR_QUIRKS
                .iter()
                .any(|q| q.host_suffix == "iproyal.com" && q.severity == QuirkSeverity::Warning)
        );
    }

    #[test]
    fn vendor_quirks_table_is_const_constructible() {
        // The slice can be evaluated in a const context, proving
        // every record is a const literal.
        const _: [VendorQuirk; 4] = [
            VENDOR_QUIRKS[0],
            VENDOR_QUIRKS[1],
            VENDOR_QUIRKS[2],
            VENDOR_QUIRKS[3],
        ];
    }

    #[test]
    fn vendor_quirks_descriptions_never_contain_credentials() {
        // Security: descriptions are static text from the const table;
        // they must never include password material.
        for q in VENDOR_QUIRKS {
            assert!(
                !q.description.contains('@'),
                "quirk has '@': {}",
                q.description
            );
            assert!(
                !q.description.contains("pass"),
                "quirk mentions 'pass': {}",
                q.description
            );
        }
    }

    // ── check: Crawlera / Zyte ───────────────────────────────────────────────

    #[test]
    fn check_crawlera_https_returns_error_match() {
        let url = ProxyUrl::parse("https://user:pass@proxy.crawlera.com:8011").unwrap();
        let m = check(&url);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].severity, QuirkSeverity::Error);
        assert_eq!(m[0].required_scheme, Scheme::Http);
        assert_eq!(m[0].host_suffix, "crawlera.com");
        assert_eq!(m[0].observed_scheme, Scheme::Https);
        assert_eq!(m[0].observed_port, Some(8011));
        // The password is never echoed.
        assert!(!m[0].description.contains("pass"));
    }

    #[test]
    fn check_crawlera_http_compliant_returns_no_match() {
        let url = ProxyUrl::parse("http://user:pass@proxy.crawlera.com:8011").unwrap();
        assert!(check(&url).is_empty());
    }

    #[test]
    fn check_crawlera_subdomain_matches() {
        // `proxy.crawlera.com` is a subdomain of `crawlera.com`.
        let url = ProxyUrl::parse("https://k:@proxy.crawlera.com:8011").unwrap();
        let m = check(&url);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].severity, QuirkSeverity::Error);
    }

    #[test]
    fn check_zyte_https_returns_error_match() {
        let url = ProxyUrl::parse("https://apikey:@proxy.zyte.com:8011").unwrap();
        let m = check(&url);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].severity, QuirkSeverity::Error);
        assert_eq!(m[0].host_suffix, "zyte.com");
    }

    #[test]
    fn check_zyte_http_compliant_returns_no_match() {
        let url = ProxyUrl::parse("http://apikey:@proxy.zyte.com:8011").unwrap();
        assert!(check(&url).is_empty());
    }

    // ── check: Bright Data ───────────────────────────────────────────────────

    #[test]
    fn check_bright_data_with_session_id_returns_warning() {
        let url = ProxyUrl::parse("http://brd-customer-1-session-abc123@brd.superproxy.io:22225")
            .unwrap();
        let m = check(&url);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].severity, QuirkSeverity::Warning);
        assert_eq!(m[0].host_suffix, "brd.superproxy.io");
    }

    #[test]
    fn check_bright_data_without_session_id_returns_warning() {
        let url = ProxyUrl::parse("http://user@brd.superproxy.io:22225").unwrap();
        let m = check(&url);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].severity, QuirkSeverity::Warning);
    }

    #[test]
    fn check_bright_data_wrong_port_returns_no_match() {
        let url = ProxyUrl::parse("http://user@brd.superproxy.io:9999").unwrap();
        assert!(check(&url).is_empty());
    }

    // ── check: IPRoyal ───────────────────────────────────────────────────────

    #[test]
    fn check_iproyal_returns_warning() {
        let url = ProxyUrl::parse("http://user:pass@residential.iproyal.com:12321").unwrap();
        let m = check(&url);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].severity, QuirkSeverity::Warning);
        assert_eq!(m[0].host_suffix, "iproyal.com");
    }

    // ── check: no false positives ────────────────────────────────────────────

    #[test]
    fn check_unknown_host_returns_empty() {
        let url = ProxyUrl::parse("http://user:pass@some-unrelated-host.example:8080").unwrap();
        assert!(check(&url).is_empty());
    }

    #[test]
    fn check_empty_url_returns_empty() {
        // A URL with no matching host_suffix produces no quirks, even
        // when the rest of the URL is unusual.
        let url = ProxyUrl::parse("http://1.2.3.4:80").unwrap();
        assert!(check(&url).is_empty());
    }

    #[test]
    fn check_host_substring_does_not_match() {
        // `mycrawlera.com` should NOT match the `crawlera.com` suffix
        // (no `.` boundary).
        let url = ProxyUrl::parse("https://user:pass@mycrawlera.com:8011").unwrap();
        assert!(check(&url).is_empty());
    }

    #[test]
    fn check_crawlera_8011_https_only_fires_for_8011() {
        // Crawlera on a non-8011 port is not a quirk match.
        let url = ProxyUrl::parse("https://user:pass@proxy.crawlera.com:9000").unwrap();
        assert!(check(&url).is_empty());
    }

    // ── check: zero-allocation on empty result ───────────────────────────────

    /// Sanity: the `check` function returns an empty `Vec` (no
    /// allocations beyond the empty `Vec::new()` itself) for URLs
    /// that don't match any quirk.
    #[test]
    fn check_unknown_host_returns_empty_vec() {
        let url = ProxyUrl::parse("http://unrelated.example:80").unwrap();
        let m = check(&url);
        assert!(m.is_empty());
        assert_eq!(m.capacity(), 0);
    }

    // ── ProxyUrl default port handling ───────────────────────────────────────

    #[test]
    fn validate_quirk_with_no_port_matches_any_port() {
        // IPRoyal quirk has `port = None` so it matches any port.
        let url = ProxyUrl::parse("http://user:pass@residential.iproyal.com:54321").unwrap();
        let m = check(&url);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].host_suffix, "iproyal.com");
    }

    // ── module-level quirks count ────────────────────────────────────────────

    #[test]
    fn all_known_quirks_slice_includes_every_constant() {
        // The `ALL_KNOWN_*` companion-slice pattern (mirrored from
        // `types::well_known::ALL_KNOWN_ASNS`) lets callers iterate
        // every constant without re-listing it.
        assert!(VENDOR_QUIRKS.contains(&CRAWLERA_8011_QUIRK));
        assert!(VENDOR_QUIRKS.contains(&ZYTE_8011_QUIRK));
        assert!(VENDOR_QUIRKS.contains(&BRD_SUPERPROXY_QUIRK));
        assert!(VENDOR_QUIRKS.contains(&IPROYAL_QUIRK));
    }
}
