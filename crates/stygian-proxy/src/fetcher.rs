//! Proxy list fetching — port trait and free-list HTTP adapter.
//!
//! [`ProxyFetcher`] is the port trait.  Implement it to pull proxies from any
//! source (remote HTTP list, database, commercial API, etc.) and integrate with
//! [`ProxyManager`] via [`load_from_fetcher`].
//!
//! The built-in [`FreeListFetcher`] downloads plain-text `host:port` proxy
//! lists from public URLs (e.g. the `TheSpeedX/PROXY-List` feeds on GitHub)
//! and parses them into [`Proxy`] records.  It is suitable for development,
//! testing, and low-stakes scraping where proxy quality is less critical.
//!
//! ## Example — load from a free list and populate the pool
//!
//! ```no_run
//! use std::sync::Arc;
//! use stygian_proxy::{
//!     ProxyManager,
//!     storage::MemoryProxyStore,
//!     fetcher::{FreeListFetcher, ProxyFetcher, FreeListSource},
//! };
//!
//! # async fn run() -> stygian_proxy::error::ProxyResult<()> {
//! let fetcher = FreeListFetcher::new(vec![
//!     FreeListSource::TheSpeedXHttp,
//! ]);
//!
//! let manager = ProxyManager::builder()
//!     .storage(Arc::new(MemoryProxyStore::default()))
//!     .build()?;
//! let loaded = stygian_proxy::fetcher::load_from_fetcher(&manager, &fetcher).await?;
//! println!("Loaded {loaded} proxies");
//! # Ok(())
//! # }
//! ```

use std::time::Duration;

use async_trait::async_trait;
use futures::future::join_all;
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, warn};

use crate::{
    Proxy, ProxyManager, ProxyType,
    error::{ProxyError, ProxyResult},
};

// ─── Port trait ───────────────────────────────────────────────────────────────

/// A source that can produce a list of [`Proxy`] records asynchronously.
///
/// Implement this trait to integrate any proxy source (remote HTTP list,
/// commercial API, database, file) with [`load_from_fetcher`].
///
/// # Example
///
/// ```
/// use async_trait::async_trait;
/// use stygian_proxy::{Proxy, ProxyType};
/// use stygian_proxy::fetcher::ProxyFetcher;
/// use stygian_proxy::error::ProxyResult;
/// use stygian_proxy::types::ProxyCapabilities;
///
/// struct MyStaticFetcher;
///
/// #[async_trait]
/// impl ProxyFetcher for MyStaticFetcher {
///     async fn fetch(&self) -> ProxyResult<Vec<Proxy>> {
///         Ok(vec![Proxy {
///             url: "http://192.168.1.1:8080".into(),
///             proxy_type: ProxyType::Http,
///             username: None,
///             password: None,
///             weight: 1,
///             tags: vec!["static".into()],
///             capabilities: ProxyCapabilities::default(),
///         }])
///     }
/// }
/// ```
#[async_trait]
pub trait ProxyFetcher: Send + Sync {
    /// Fetch the current proxy list.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::FetchFailed`] if the source is unreachable or
    /// returns malformed data.
    async fn fetch(&self) -> ProxyResult<Vec<Proxy>>;
}

// ─── Free-list sources ────────────────────────────────────────────────────────

/// A well-known free/public proxy list feed.
///
/// These lists are community-maintained and quality varies.  They are suitable
/// for development and testing.  For production use, prefer a commercial
/// provider adapter.
///
/// # Example
///
/// ```
/// use stygian_proxy::fetcher::FreeListSource;
/// let _src = FreeListSource::TheSpeedXHttp;
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FreeListSource {
    /// HTTP proxies from `TheSpeedX/PROXY-List` (GitHub, plain `host:port`).
    TheSpeedXHttp,
    #[cfg(feature = "socks")]
    /// SOCKS4 proxies from `TheSpeedX/PROXY-List` (requires the `socks` feature).
    TheSpeedXSocks4,
    #[cfg(feature = "socks")]
    /// SOCKS5 proxies from `TheSpeedX/PROXY-List` (requires the `socks` feature).
    TheSpeedXSocks5,
    /// HTTP proxies from `clarketm/proxy-list` (GitHub, plain `host:port`).
    ClarketmHttp,
    /// Mixed HTTP proxies from `openproxylist.xyz`.
    OpenProxyListHttp,
    /// A custom URL.  Content must be one `host:port` entry per line.
    Custom {
        /// The URL to fetch.
        url: String,
        /// The [`ProxyType`] to assign all parsed entries.
        proxy_type: ProxyType,
    },
}

impl FreeListSource {
    const fn url(&self) -> &str {
        match self {
            Self::TheSpeedXHttp => {
                "https://raw.githubusercontent.com/TheSpeedX/PROXY-List/master/http.txt"
            }
            #[cfg(feature = "socks")]
            Self::TheSpeedXSocks4 => {
                "https://raw.githubusercontent.com/TheSpeedX/PROXY-List/master/socks4.txt"
            }
            #[cfg(feature = "socks")]
            Self::TheSpeedXSocks5 => {
                "https://raw.githubusercontent.com/TheSpeedX/PROXY-List/master/socks5.txt"
            }
            Self::ClarketmHttp => {
                "https://raw.githubusercontent.com/clarketm/proxy-list/master/proxy-list-raw.txt"
            }
            Self::OpenProxyListHttp => "https://openproxylist.xyz/http.txt",
            Self::Custom { url, .. } => url.as_str(),
        }
    }

    const fn proxy_type(&self) -> ProxyType {
        match self {
            Self::TheSpeedXHttp | Self::ClarketmHttp | Self::OpenProxyListHttp => ProxyType::Http,
            #[cfg(feature = "socks")]
            Self::TheSpeedXSocks4 => ProxyType::Socks4,
            #[cfg(feature = "socks")]
            Self::TheSpeedXSocks5 => ProxyType::Socks5,
            Self::Custom { proxy_type, .. } => *proxy_type,
        }
    }
}

// ─── FreeListFetcher ──────────────────────────────────────────────────────────

/// Fetches plain-text `host:port` proxy lists from one or more public URLs.
///
/// Each source is fetched concurrently.  Lines that do not parse as valid
/// `host:port` entries are silently skipped.  An empty or unreachable source
/// logs a warning but does not fail the entire fetch — at least one source
/// must return results for the call to succeed.
///
/// # Example
///
/// ```no_run
/// use stygian_proxy::fetcher::{FreeListFetcher, FreeListSource, ProxyFetcher};
///
/// # async fn run() -> stygian_proxy::error::ProxyResult<()> {
/// let fetcher = FreeListFetcher::new(vec![FreeListSource::TheSpeedXHttp]);
/// let proxies = fetcher.fetch().await?;
/// println!("Got {} proxies", proxies.len());
/// # Ok(())
/// # }
/// ```
pub struct FreeListFetcher {
    sources: Vec<FreeListSource>,
    client: Client,
    tags: Vec<String>,
}

impl FreeListFetcher {
    /// Create a fetcher for the given sources with default HTTP client settings
    /// (10 s timeout, TLS enabled).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_proxy::fetcher::{FreeListFetcher, FreeListSource};
    /// let _f = FreeListFetcher::new(vec![FreeListSource::TheSpeedXHttp]);
    /// ```
    pub fn new(sources: Vec<FreeListSource>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|e| {
                warn!("Failed to build HTTP client with 10 s timeout (TLS backend issue?): {e}; falling back to default client with per-request timeout enforcement");
                Client::default()
            });
        Self {
            sources,
            client,
            tags: vec!["free-list".into()],
        }
    }

    /// Replace the internal HTTP client with a TLS-profiled one.
    ///
    /// Proxy-list fetch requests will carry a browser TLS fingerprint and
    /// matching `Accept` / `Sec-CH-UA` headers.
    ///
    /// Only available with the `tls-profiled` feature.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_proxy::fetcher::{FreeListFetcher, FreeListSource};
    /// use stygian_proxy::http_client::{ProfiledRequestMode, ProfiledRequester};
    ///
    /// # fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let fetcher = FreeListFetcher::new(vec![FreeListSource::TheSpeedXHttp])
    ///     .with_profiled_client(ProfiledRequester::chrome_mode(ProfiledRequestMode::Preset)?);
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "tls-profiled")]
    #[must_use]
    pub fn with_profiled_client(
        mut self,
        requester: crate::http_client::ProfiledRequester,
    ) -> Self {
        self.client = requester.client().clone();
        drop(requester);
        self
    }

    /// Build and attach a profile-mode-based requester.
    ///
    /// Uses Chrome 131 as the baseline browser identity and applies `mode`
    /// to TLS control mapping.
    ///
    /// Only available when the `tls-profiled` feature is enabled.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::ProxyError::ConfigError`] if the profiled
    /// requester cannot be constructed.
    #[cfg(feature = "tls-profiled")]
    pub fn with_profiled_mode(
        self,
        mode: crate::types::ProfiledRequestMode,
    ) -> crate::error::ProxyResult<Self> {
        let requester = crate::http_client::ProfiledRequester::chrome_mode(mode)
            .map_err(|e| crate::error::ProxyError::ConfigError(e.to_string()))?;
        Ok(self.with_profiled_client(requester))
    }

    /// Attach extra tags to every proxy produced by this fetcher.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_proxy::fetcher::{FreeListFetcher, FreeListSource};
    /// let _f = FreeListFetcher::new(vec![FreeListSource::TheSpeedXHttp])
    ///     .with_tags(vec!["dev".into(), "http".into()]);
    /// ```
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags.extend(tags);
        self
    }

    /// Parse one `host:port` line, including bracketed IPv6 addresses.
    fn parse_host_port_line(line: &str) -> Option<(String, u16)> {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }

        let (host, port_str) = if line.starts_with('[') {
            let end = line.find(']')?;
            let host = line.get(..=end)?.trim();
            let remainder = line.get(end + 1..)?.trim();
            let (_, port_str) = remainder.rsplit_once(':')?;
            (host, port_str.trim())
        } else {
            let (host, port_str) = line.rsplit_once(':')?;
            let host = host.trim();
            if host.contains(':') {
                return None;
            }
            (host, port_str.trim())
        };

        if host.is_empty() || host == "[]" {
            return None;
        }

        let port = port_str.parse::<u16>().ok()?;
        if port == 0 {
            return None;
        }

        Some((host.to_string(), port))
    }

    /// Fetch a single source, returning parsed proxies (empty on failure).
    async fn fetch_source(&self, source: &FreeListSource) -> Vec<Proxy> {
        let url = source.url();
        let proxy_type = source.proxy_type();

        let body = match self
            .client
            .get(url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => match resp.text().await {
                Ok(t) => t,
                Err(e) => {
                    warn!("Failed to read body from {url}: {e}");
                    return vec![];
                }
            },
            Ok(resp) => {
                warn!(
                    "Non-success status {} fetching proxy list from {url}",
                    resp.status()
                );
                return vec![];
            }
            Err(e) => {
                warn!("Failed to fetch proxy list from {url}: {e}");
                return vec![];
            }
        };

        let proxies: Vec<Proxy> = body
            .lines()
            .filter_map(|line| {
                let (host, port) = Self::parse_host_port_line(line)?;
                let scheme = match proxy_type {
                    ProxyType::Http => "http",
                    ProxyType::Https => "https",
                    #[cfg(feature = "socks")]
                    ProxyType::Socks4 => "socks4",
                    #[cfg(feature = "socks")]
                    ProxyType::Socks5 => "socks5",
                };
                Some(Proxy {
                    url: format!("{scheme}://{host}:{port}"),
                    proxy_type,
                    username: None,
                    password: None,
                    weight: 1,
                    tags: self.tags.clone(),
                    capabilities: crate::types::ProxyCapabilities::default(),
                })
            })
            .collect();

        debug!(source = url, count = proxies.len(), "Fetched proxy list");
        proxies
    }
}

#[async_trait]
impl ProxyFetcher for FreeListFetcher {
    async fn fetch(&self) -> ProxyResult<Vec<Proxy>> {
        if self.sources.is_empty() {
            return Err(ProxyError::ConfigError(
                "no sources configured for FreeListFetcher".into(),
            ));
        }

        // Drive all source fetches concurrently.
        let results = join_all(self.sources.iter().map(|s| self.fetch_source(s))).await;
        let all: Vec<Proxy> = results.into_iter().flatten().collect();

        if all.is_empty() {
            return Err(ProxyError::FetchFailed {
                origin: self
                    .sources
                    .iter()
                    .map(FreeListSource::url)
                    .collect::<Vec<_>>()
                    .join(", "),
                message: "all sources returned empty or failed".into(),
            });
        }

        Ok(all)
    }
}

// ─── FreeAPIProxies adapter ──────────────────────────────────────────────────

/// Fetches proxies from a JSON API compatible with FreeAPIProxies-style
/// payloads.
///
/// The adapter accepts either a top-level array payload or an object payload
/// with `data` or `results` arrays.  Optional query parameters (`limit`,
/// `protocol`, `country`) are appended to the endpoint URL when set.
///
/// # Example
///
/// ```no_run
/// use stygian_proxy::fetcher::{FreeApiProxiesFetcher, ProxyFetcher};
///
/// # async fn run() -> stygian_proxy::error::ProxyResult<()> {
/// let fetcher = FreeApiProxiesFetcher::new()
///     .with_limit(100)
///     .with_protocol_filter("http")
///     .with_country_filter("US");
/// let proxies = fetcher.fetch().await?;
/// println!("Got {} proxies", proxies.len());
/// # Ok(())
/// # }
/// ```
pub struct FreeApiProxiesFetcher {
    endpoint: String,
    client: Client,
    tags: Vec<String>,
    /// Maximum number of proxies to request from the API.
    limit: Option<u32>,
    /// Protocol filter sent as a query parameter (e.g. `"http"`, `"socks5"`).
    protocol_filter: Option<String>,
    /// ISO 3166-1 alpha-2 country code filter (e.g. `"US"`, `"DE"`).
    country_filter: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum FreeApiProxiesResponse {
    List(Vec<FreeApiProxyRecord>),
    Data { data: Vec<FreeApiProxyRecord> },
    Results { results: Vec<FreeApiProxyRecord> },
}

impl FreeApiProxiesResponse {
    fn into_records(self) -> Vec<FreeApiProxyRecord> {
        match self {
            Self::List(records)
            | Self::Data { data: records }
            | Self::Results { results: records } => records,
        }
    }
}

#[derive(Debug, Deserialize)]
struct FreeApiProxyRecord {
    #[serde(default, alias = "ip", alias = "host")]
    address_host: String,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default, alias = "proxy", alias = "address")]
    address: Option<String>,
    #[serde(default, alias = "protocol", alias = "type", alias = "proxy_type")]
    protocol: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default, alias = "countryCode", alias = "country_code")]
    country_code: Option<String>,
}

impl FreeApiProxiesFetcher {
    const DEFAULT_ENDPOINT: &str = "https://freeapiproxies.azurewebsites.net/";

    /// Create a `FreeAPIProxies` fetcher using the default endpoint.
    #[must_use]
    pub fn new() -> Self {
        Self::with_endpoint(Self::DEFAULT_ENDPOINT)
    }

    /// Create a `FreeAPIProxies` fetcher using a custom JSON endpoint.
    #[must_use]
    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|e| {
                warn!("Failed to build HTTP client with 10 s timeout (TLS backend issue?): {e}; falling back to default client with per-request timeout enforcement");
                Client::default()
            });

        Self {
            endpoint: endpoint.into(),
            client,
            tags: vec!["freeapiproxies".into()],
            limit: None,
            protocol_filter: None,
            country_filter: None,
        }
    }

    /// Attach extra tags to every proxy produced by this fetcher.
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags.extend(tags);
        self
    }

    /// Set the maximum number of proxies to request from the API.
    ///
    /// Appended to the request as `?limit=<n>`.  Ignored when `None`.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_proxy::fetcher::FreeApiProxiesFetcher;
    /// let _f = FreeApiProxiesFetcher::new().with_limit(50);
    /// ```
    #[must_use]
    pub const fn with_limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Filter by proxy protocol on the server side.
    ///
    /// Appended to the request as `?protocol=<value>`.  Common values are
    /// `"http"`, `"https"`, `"socks4"`, and `"socks5"`.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_proxy::fetcher::FreeApiProxiesFetcher;
    /// let _f = FreeApiProxiesFetcher::new().with_protocol_filter("http");
    /// ```
    #[must_use]
    pub fn with_protocol_filter(mut self, protocol: impl Into<String>) -> Self {
        self.protocol_filter = Some(protocol.into());
        self
    }

    /// Filter by ISO 3166-1 alpha-2 country code on the server side.
    ///
    /// Appended to the request as `?country=<value>` (uppercased).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_proxy::fetcher::FreeApiProxiesFetcher;
    /// let _f = FreeApiProxiesFetcher::new().with_country_filter("US");
    /// ```
    #[must_use]
    pub fn with_country_filter(mut self, country_code: impl Into<String>) -> Self {
        self.country_filter = Some(country_code.into().to_ascii_uppercase());
        self
    }

    /// Build the full request URL with any configured query parameters.
    fn request_url(&self) -> String {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(limit) = self.limit {
            params.push(("limit", limit.to_string()));
        }
        if let Some(ref protocol) = self.protocol_filter {
            params.push(("protocol", protocol.clone()));
        }
        if let Some(ref country) = self.country_filter {
            params.push(("country", country.clone()));
        }
        if params.is_empty() {
            return self.endpoint.clone();
        }
        let qs = params
            .iter()
            .enumerate()
            .fold(String::new(), |mut acc, (i, (k, v))| {
                use std::fmt::Write as _;
                let sep = if i == 0 { "?" } else { "&" };
                let _ = write!(acc, "{sep}{k}={v}");
                acc
            });
        format!("{}{qs}", self.endpoint)
    }

    fn protocol_to_proxy_type(protocol: Option<&str>) -> Option<ProxyType> {
        let normalized = protocol.map(str::trim).map(str::to_ascii_lowercase);
        match normalized.as_deref() {
            None | Some("" | "http") => Some(ProxyType::Http),
            Some("https") => Some(ProxyType::Https),
            #[cfg(feature = "socks")]
            Some("socks" | "socks5") => Some(ProxyType::Socks5),
            #[cfg(feature = "socks")]
            Some("socks4") => Some(ProxyType::Socks4),
            _ => None,
        }
    }

    fn parse_address(record: &FreeApiProxyRecord) -> Option<(String, u16)> {
        if let Some(address) = record.address.as_deref() {
            if let Some((host, port)) = FreeListFetcher::parse_host_port_line(address) {
                return Some((host, port));
            }

            if let Ok(url) = reqwest::Url::parse(address)
                && let Some(port) = url.port_or_known_default()
            {
                return Some((url.host_str()?.to_string(), port));
            }
        }

        let host = record.address_host.trim();
        let port = record.port?;
        if host.is_empty() || port == 0 {
            return None;
        }
        Some((host.to_string(), port))
    }

    fn record_to_proxy(&self, record: FreeApiProxyRecord) -> Option<Proxy> {
        let proxy_type = Self::protocol_to_proxy_type(record.protocol.as_deref())?;
        let (host, port) = Self::parse_address(&record)?;

        let scheme = match proxy_type {
            ProxyType::Http => "http",
            ProxyType::Https => "https",
            #[cfg(feature = "socks")]
            ProxyType::Socks4 => "socks4",
            #[cfg(feature = "socks")]
            ProxyType::Socks5 => "socks5",
        };

        let mut tags = self.tags.clone();
        if let Some(country_code) = record.country_code.as_deref()
            && !country_code.trim().is_empty()
        {
            tags.push(format!(
                "country:{}",
                country_code.trim().to_ascii_uppercase()
            ));
        }

        Some(Proxy {
            url: format!("{scheme}://{host}:{port}"),
            proxy_type,
            username: record.username.filter(|v| !v.trim().is_empty()),
            password: record.password.filter(|v| !v.trim().is_empty()),
            weight: 1,
            tags,
            capabilities: crate::types::ProxyCapabilities::default(),
        })
    }

    fn parse_payload(&self, body: &str) -> ProxyResult<Vec<Proxy>> {
        let response: FreeApiProxiesResponse =
            serde_json::from_str(body).map_err(|e| ProxyError::FetchFailed {
                origin: self.endpoint.clone(),
                message: format!("invalid freeapiproxies json payload: {e}"),
            })?;

        let proxies: Vec<Proxy> = response
            .into_records()
            .into_iter()
            .filter_map(|record| self.record_to_proxy(record))
            .collect();

        if proxies.is_empty() {
            return Err(ProxyError::FetchFailed {
                origin: self.endpoint.clone(),
                message: "freeapiproxies payload contained no usable proxies".into(),
            });
        }

        Ok(proxies)
    }
}

impl Default for FreeApiProxiesFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ProxyFetcher for FreeApiProxiesFetcher {
    async fn fetch(&self) -> ProxyResult<Vec<Proxy>> {
        let url = self.request_url();
        let body = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| ProxyError::FetchFailed {
                origin: url.clone(),
                message: e.to_string(),
            })?
            .error_for_status()
            .map_err(|e| ProxyError::FetchFailed {
                origin: url.clone(),
                message: e.to_string(),
            })?
            .text()
            .await
            .map_err(|e| ProxyError::FetchFailed {
                origin: url.clone(),
                message: e.to_string(),
            })?;

        self.parse_payload(&body)
    }
}

// ─── Helper ───────────────────────────────────────────────────────────────────

/// Fetch proxies from `fetcher` and add them all to `manager`.
///
/// Returns the number of proxies successfully added.  Individual `add_proxy`
/// failures (e.g. duplicate URL) are logged as warnings and do not abort the
/// load.
///
/// # Errors
///
/// Returns any [`ProxyError`] emitted by `fetcher.fetch()` if the fetcher
/// itself fails.
///
/// # Example
///
/// ```no_run
/// use std::sync::Arc;
/// use stygian_proxy::{ProxyManager, storage::MemoryProxyStore, fetcher::{FreeListFetcher, FreeListSource, load_from_fetcher}};
///
/// # async fn run() -> stygian_proxy::error::ProxyResult<()> {
/// let manager = ProxyManager::builder()
///     .storage(Arc::new(MemoryProxyStore::default()))
///     .build()?;
/// let fetcher = FreeListFetcher::new(vec![FreeListSource::TheSpeedXHttp]);
/// let n = load_from_fetcher(&manager, &fetcher).await?;
/// println!("Loaded {n} proxies");
/// # Ok(())
/// # }
/// ```
pub async fn load_from_fetcher(
    manager: &ProxyManager,
    fetcher: &dyn ProxyFetcher,
) -> ProxyResult<usize> {
    let proxies = fetcher.fetch().await?;
    let total = proxies.len();
    let mut loaded = 0usize;

    for proxy in proxies {
        match manager.add_proxy(proxy).await {
            Ok(_) => loaded += 1,
            Err(e) => warn!("Skipped proxy during load: {e}"),
        }
    }

    debug!(total, loaded, "Proxy list loaded into manager");
    Ok(loaded)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_api_proxies_fetcher_request_url_no_params() {
        let f = FreeApiProxiesFetcher::with_endpoint("https://example.test/api");
        assert_eq!(f.request_url(), "https://example.test/api");
    }

    #[test]
    fn free_api_proxies_fetcher_request_url_with_params() {
        let f = FreeApiProxiesFetcher::with_endpoint("https://example.test/api")
            .with_limit(50)
            .with_protocol_filter("http")
            .with_country_filter("us");
        let url = f.request_url();
        assert!(url.contains("limit=50"), "expected limit param in {url}");
        assert!(
            url.contains("protocol=http"),
            "expected protocol param in {url}"
        );
        assert!(
            url.contains("country=US"),
            "expected country uppercased in {url}"
        );
        assert!(url.starts_with("https://example.test/api?"), "missing ?");
    }

    #[test]
    fn free_api_proxies_fetcher_country_filter_uppercased() {
        let f = FreeApiProxiesFetcher::new().with_country_filter("de");
        assert_eq!(f.country_filter.as_deref(), Some("DE"));
    }

    /// Integration test — hits the live `FreeAPIProxies` endpoint.
    /// Run with: `cargo test -p stygian-proxy --all-features -- --ignored`
    #[test]
    #[ignore = "requires live network access to freeapiproxies.azurewebsites.net"]
    fn free_api_proxies_fetcher_live_fetch() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let fetcher = FreeApiProxiesFetcher::new().with_limit(20);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| std::io::Error::other(format!("failed to build runtime for test: {e}")))?;
        let proxies = rt.block_on(fetcher.fetch())?;
        assert!(
            !proxies.is_empty(),
            "expected at least one proxy from live endpoint"
        );
        for proxy in &proxies {
            assert!(
                proxy.url.starts_with("http://")
                    || proxy.url.starts_with("https://")
                    || proxy.url.starts_with("socks4://")
                    || proxy.url.starts_with("socks5://"),
                "unexpected proxy url scheme: {}",
                proxy.url
            );
        }
        Ok(())
    }

    #[test]
    fn free_list_source_url_is_nonempty() {
        #[cfg(not(feature = "socks"))]
        let sources = vec![
            FreeListSource::TheSpeedXHttp,
            FreeListSource::ClarketmHttp,
            FreeListSource::OpenProxyListHttp,
            FreeListSource::Custom {
                url: "https://example.com/proxies.txt".into(),
                proxy_type: ProxyType::Http,
            },
        ];
        #[cfg(feature = "socks")]
        let sources = {
            let mut s = vec![
                FreeListSource::TheSpeedXHttp,
                FreeListSource::ClarketmHttp,
                FreeListSource::OpenProxyListHttp,
                FreeListSource::Custom {
                    url: "https://example.com/proxies.txt".into(),
                    proxy_type: ProxyType::Http,
                },
            ];
            s.extend([
                FreeListSource::TheSpeedXSocks4,
                FreeListSource::TheSpeedXSocks5,
            ]);
            s
        };
        for src in &sources {
            assert!(
                !src.url().is_empty(),
                "FreeListSource::{src:?} has empty URL"
            );
        }
    }

    #[test]
    fn free_list_source_proxy_types() {
        assert_eq!(FreeListSource::TheSpeedXHttp.proxy_type(), ProxyType::Http);
        #[cfg(feature = "socks")]
        assert_eq!(
            FreeListSource::TheSpeedXSocks4.proxy_type(),
            ProxyType::Socks4
        );
        #[cfg(feature = "socks")]
        assert_eq!(
            FreeListSource::TheSpeedXSocks5.proxy_type(),
            ProxyType::Socks5
        );
        assert_eq!(FreeListSource::ClarketmHttp.proxy_type(), ProxyType::Http);
    }

    #[test]
    fn free_api_proxies_fetcher_parses_array_payload() -> crate::error::ProxyResult<()> {
        let fetcher = FreeApiProxiesFetcher::with_endpoint("https://example.test/freeapi");
        let body = r#"
[
    {"host":"1.2.3.4","port":8080,"protocol":"http","countryCode":"us"},
    {"address":"5.6.7.8:8443","protocol":"https"}
]
"#;

        let proxies = fetcher.parse_payload(body)?;
        assert_eq!(proxies.len(), 2);
        assert_eq!(
            proxies.first().map(|proxy| proxy.url.as_str()),
            Some("http://1.2.3.4:8080")
        );
        assert_eq!(
            proxies.get(1).map(|proxy| proxy.url.as_str()),
            Some("https://5.6.7.8:8443")
        );
        Ok(())
    }

    #[test]
    fn free_api_proxies_fetcher_parses_wrapped_results_payload() -> crate::error::ProxyResult<()> {
        let fetcher = FreeApiProxiesFetcher::with_endpoint("https://example.test/freeapi");
        let body = r#"
{
    "results": [
        {"ip":"9.9.9.9","port":3128,"type":"http"}
    ]
}
"#;

        let proxies = fetcher.parse_payload(body)?;
        assert_eq!(proxies.len(), 1);
        assert_eq!(
            proxies.first().map(|proxy| proxy.url.as_str()),
            Some("http://9.9.9.9:3128")
        );
        Ok(())
    }

    #[test]
    fn free_list_fetcher_parse_valid_lines() {
        let fetcher = FreeListFetcher::new(vec![]);
        // Test the parsing logic directly by calling parse on synthetic text.
        let text = "1.2.3.4:8080\n# comment\n\nbad-line\n5.6.7.8:3128\n[2001:db8::1]:8081\n";
        let parsed: Vec<Proxy> = text
            .lines()
            .filter_map(|line| {
                let (host, port) = FreeListFetcher::parse_host_port_line(line)?;
                Some(Proxy {
                    url: format!("http://{host}:{port}"),
                    proxy_type: ProxyType::Http,
                    username: None,
                    password: None,
                    weight: 1,
                    tags: fetcher.tags.clone(),
                    capabilities: crate::types::ProxyCapabilities::default(),
                })
            })
            .collect();

        assert_eq!(parsed.len(), 3);
        assert_eq!(
            parsed.first().map(|proxy| proxy.url.as_str()),
            Some("http://1.2.3.4:8080")
        );
        assert_eq!(
            parsed.get(1).map(|proxy| proxy.url.as_str()),
            Some("http://5.6.7.8:3128")
        );
        assert_eq!(
            parsed.get(2).map(|proxy| proxy.url.as_str()),
            Some("http://[2001:db8::1]:8081")
        );
    }

    #[test]
    fn free_list_fetcher_with_tags_extends() {
        let f = FreeListFetcher::new(vec![]).with_tags(vec!["custom".into()]);
        assert!(f.tags.contains(&"free-list".to_string()));
        assert!(f.tags.contains(&"custom".to_string()));
    }

    #[test]
    fn free_list_fetcher_skips_invalid_port() {
        assert!(FreeListFetcher::parse_host_port_line("1.2.3.4:notaport").is_none());
        assert!(FreeListFetcher::parse_host_port_line("1.2.3.4:0").is_none());
        assert!(FreeListFetcher::parse_host_port_line(":8080").is_none());
        assert!(FreeListFetcher::parse_host_port_line("2001:db8::1:8080").is_none());
    }

    #[test]
    fn free_list_fetcher_empty_sources_is_config_error()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let fetcher = FreeListFetcher::new(vec![]);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(|e| std::io::Error::other(format!("failed to build runtime for test: {e}")))?;
        let err = rt
            .block_on(fetcher.fetch())
            .err()
            .ok_or_else(|| std::io::Error::other("empty sources should fail"))?;
        match err {
            ProxyError::ConfigError(msg) => {
                assert!(msg.contains("no sources configured"));
            }
            other => {
                return Err(
                    std::io::Error::other(format!("unexpected error variant: {other}")).into(),
                );
            }
        }
        Ok(())
    }

    #[test]
    fn proxy_error_fetch_failed_display() {
        let e = ProxyError::FetchFailed {
            origin: "https://example.com".into(),
            message: "timed out".into(),
        };
        assert!(e.to_string().contains("https://example.com"));
        assert!(e.to_string().contains("timed out"));
    }
}
