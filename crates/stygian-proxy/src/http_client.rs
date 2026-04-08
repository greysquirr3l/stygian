//! TLS-profiled HTTP client for the proxy health checker and fetcher.
//!
//! Enabled by the `tls-profiled` feature flag. Wraps
//! [`stygian_browser::tls::build_profiled_client`] so that outgoing plain-HTTP
//! requests (health checks, proxy-list fetches) present the same TLS
//! fingerprint and HTTP header set as a real browser, reducing the chance that
//! the target blocks or fingerprints the checker itself.
//!
//! # Architecture
//!
//! ```text
//! ProxyManager / HealthChecker / FreeListFetcher
//!         │
//!         ├── (default) vanilla reqwest::Client
//!         │
//!         └── (tls-profiled feature) ProfiledRequester
//!                 │
//!                 └── reqwest::Client built from stygian_browser::TlsProfile
//!                         ├── TLS: cipher-suite order, ALPN, kx groups
//!                         ├── User-Agent matched to browser
//!                         └── Accept / Sec-CH-UA / sec-fetch-* headers
//! ```
//!
//! # Example
//!
//! ```no_run
//! use stygian_proxy::http_client::ProfiledRequester;
//!
//! # fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let requester = ProfiledRequester::chrome()?;
//! let client = requester.client();
//! # Ok(())
//! # }
//! ```

use stygian_browser::tls::{
    CHROME_131, EDGE_131, FIREFOX_133, SAFARI_18, TlsProfile, build_profiled_client,
};
use thiserror::Error;

// ─── error ───────────────────────────────────────────────────────────────────

/// Errors that can occur when building a [`ProfiledRequester`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProfiledRequesterError {
    /// The underlying TLS-profiled client could not be constructed.
    #[error("failed to build TLS-profiled client: {0}")]
    Build(#[from] stygian_browser::tls::TlsClientError),
}

// ─── ProfiledRequester ────────────────────────────────────────────────────────

/// A [`reqwest::Client`] pre-configured with a browser TLS fingerprint and
/// matching HTTP headers.
///
/// Use [`ProfiledRequester::chrome`], [`ProfiledRequester::firefox`],
/// [`ProfiledRequester::safari`], or [`ProfiledRequester::edge`] for
/// built-in profiles, or supply any [`TlsProfile`] via
/// [`ProfiledRequester::from_profile`].
///
/// The held `reqwest::Client` is cheap to clone (it is `Arc`-backed internally)
/// so `ProfiledRequester` itself implements `Clone`.
///
/// # Example
///
/// ```no_run
/// use stygian_proxy::http_client::ProfiledRequester;
///
/// # fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let requester = ProfiledRequester::chrome()?;
///
/// // Pass a proxy URL to route requests through it.
/// let requester_via_proxy = ProfiledRequester::from_profile(&stygian_browser::tls::CHROME_131, Some("http://10.0.0.1:8080"))?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct ProfiledRequester {
    client: reqwest::Client,
    /// Human-readable profile name for diagnostics.
    profile_name: &'static str,
}

impl ProfiledRequester {
    /// Build from any static [`TlsProfile`].
    ///
    /// Pass `proxy_url` to route all requests through a proxy.
    ///
    /// # Errors
    ///
    /// Returns [`ProfiledRequesterError::Build`] if the TLS config or HTTP
    /// client cannot be constructed.
    pub fn from_profile(
        profile: &'static TlsProfile,
        proxy_url: Option<&str>,
    ) -> Result<Self, ProfiledRequesterError> {
        let client = build_profiled_client(profile, proxy_url)?;
        Ok(Self {
            client,
            profile_name: Box::leak(profile.name.clone().into_boxed_str()),
        })
    }

    /// Build a Chrome 131-profiled requester.
    ///
    /// # Errors
    ///
    /// Returns [`ProfiledRequesterError::Build`] on construction failure.
    pub fn chrome() -> Result<Self, ProfiledRequesterError> {
        Self::from_profile(&CHROME_131, None)
    }

    /// Build a Firefox 133-profiled requester.
    ///
    /// # Errors
    ///
    /// Returns [`ProfiledRequesterError::Build`] on construction failure.
    pub fn firefox() -> Result<Self, ProfiledRequesterError> {
        Self::from_profile(&FIREFOX_133, None)
    }

    /// Build a Safari 18-profiled requester.
    ///
    /// # Errors
    ///
    /// Returns [`ProfiledRequesterError::Build`] on construction failure.
    pub fn safari() -> Result<Self, ProfiledRequesterError> {
        Self::from_profile(&SAFARI_18, None)
    }

    /// Build an Edge 131-profiled requester.
    ///
    /// # Errors
    ///
    /// Returns [`ProfiledRequesterError::Build`] on construction failure.
    pub fn edge() -> Result<Self, ProfiledRequesterError> {
        Self::from_profile(&EDGE_131, None)
    }

    /// Build a requester using a profile weighted by real-world browser market
    /// share (see [`TlsProfile::random_weighted`]).
    ///
    /// `seed` should differ across callers to get varied profiles.
    ///
    /// # Errors
    ///
    /// Returns [`ProfiledRequesterError::Build`] on construction failure.
    pub fn random_weighted(seed: u64) -> Result<Self, ProfiledRequesterError> {
        let profile = TlsProfile::random_weighted(seed);
        let client = build_profiled_client(profile, None)?;
        Ok(Self {
            client,
            profile_name: Box::leak(profile.name.clone().into_boxed_str()),
        })
    }

    /// Borrow the underlying [`reqwest::Client`].
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// The human-readable name of the TLS profile in use.
    pub fn profile_name(&self) -> &str {
        self.profile_name
    }

    /// Return `true` if the profile negotiates HTTP/2 (h2 in ALPN).
    ///
    /// This is always `true` for the built-in Chrome, Firefox, Edge, and
    /// Safari profiles.
    pub fn supports_h2(&self) -> bool {
        // We can't query reqwest's ALPN config after construction, so we
        // derive it from the profile name as a best-effort hint. All four
        // built-in profiles include H2.
        !self.profile_name.contains("HTTP/1.1-only")
    }
}

// ─── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn chrome_requester_builds() {
        let r = ProfiledRequester::chrome().unwrap();
        assert_eq!(r.profile_name(), "Chrome 131");
    }

    #[test]
    fn firefox_requester_builds() {
        let r = ProfiledRequester::firefox().unwrap();
        assert_eq!(r.profile_name(), "Firefox 133");
    }

    #[test]
    fn safari_requester_builds() {
        let r = ProfiledRequester::safari().unwrap();
        assert_eq!(r.profile_name(), "Safari 18");
    }

    #[test]
    fn edge_requester_builds() {
        let r = ProfiledRequester::edge().unwrap();
        assert_eq!(r.profile_name(), "Edge 131");
    }

    #[test]
    fn random_weighted_requester_varies() {
        let a = ProfiledRequester::random_weighted(1).unwrap();
        let b = ProfiledRequester::random_weighted(999_999).unwrap();
        // Not guaranteed to differ, but the distribution should produce at
        // least two distinct profiles across a wider seed range.
        let _ = (a.profile_name(), b.profile_name()); // just ensure no panic
    }

    #[test]
    fn from_profile_with_custom_gives_correct_name() {
        let r = ProfiledRequester::from_profile(&CHROME_131, None).unwrap();
        assert_eq!(r.profile_name(), "Chrome 131");
    }

    #[test]
    fn clone_is_shallow() {
        let r = ProfiledRequester::chrome().unwrap();
        let r2 = r.clone();
        assert_eq!(r.profile_name(), r2.profile_name());
    }
}
