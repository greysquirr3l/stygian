//! Storage port and in-memory adapter for proxy records.
//!
//! ## Ingest validation
//!
//! The [`MemoryProxyStore::add`] path runs
//! [`crate::vendor_quirks::check`] on every URL before inserting the
//! record. Hard-error quirks (e.g. `Crawlera` 8011 + `https://`) reject
//! the URL outright; warning-severity quirks (e.g. `Bright Data` session
//! format) are logged via `tracing::warn!` and the URL is accepted.
//! See [`crate::vendor_quirks`] for the full quirk table and
//! [`ProxyUrl`](crate::vendor_quirks::ProxyUrl) for the canonical URL
//! parser.

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::ProxyResult;
use crate::types::{Proxy, ProxyRecord};

/// Abstract storage interface for persisting and querying proxy records.
///
/// Implementors must be `Send + Sync + 'static` to support concurrent access
/// across async tasks. The trait is object-safe via [`macro@async_trait`].
///
/// # Example
/// ```rust,no_run
/// use stygian_proxy::storage::ProxyStoragePort;
/// use stygian_proxy::types::{IpClass, Proxy, ProxyCapabilities, ProxyType, TargetVendorCompatibility};
/// use uuid::Uuid;
///
/// async fn demo(store: &dyn ProxyStoragePort) {
///     let proxy = Proxy {
///         url: "http://proxy.example.com:8080".into(),
///         proxy_type: ProxyType::Http,
///         username: None,
///         password: None,
///         weight: 1,
///         tags: vec![],
///         capabilities: ProxyCapabilities::default(),
///         ip_class: IpClass::Unknown,
///         target_compatibility: TargetVendorCompatibility::default(),
///     };
///     let record = store.add(proxy).await.unwrap();
///     let _ = store.get(record.id).await.unwrap();
/// }
/// ```
#[async_trait]
pub trait ProxyStoragePort: Send + Sync + 'static {
    /// Add a new proxy to the store and return its [`ProxyRecord`].
    async fn add(&self, proxy: Proxy) -> ProxyResult<ProxyRecord>;

    /// Remove a proxy by its UUID. Returns an error if the ID is not found.
    async fn remove(&self, id: Uuid) -> ProxyResult<()>;

    /// Return all stored proxy records.
    async fn list(&self) -> ProxyResult<Vec<ProxyRecord>>;

    /// Fetch a single proxy record by UUID.
    async fn get(&self, id: Uuid) -> ProxyResult<ProxyRecord>;

    /// Record the outcome of a request through a proxy.
    ///
    /// - `success`: whether the request succeeded.
    /// - `latency_ms`: elapsed time in milliseconds.
    async fn update_metrics(&self, id: Uuid, success: bool, latency_ms: u64) -> ProxyResult<()>;

    /// Return all stored proxy records paired with their live metrics reference.
    ///
    /// Used by [`ProxyManager`](crate::manager::ProxyManager) when building
    /// [`ProxyCandidate`](crate::strategy::ProxyCandidate) slices so that
    /// latency-aware strategies (e.g. least-used) see up-to-date counters.
    async fn list_with_metrics(&self) -> ProxyResult<Vec<(ProxyRecord, Arc<ProxyMetrics>)>>;
}

/// Convenience alias for a heap-allocated, type-erased [`ProxyStoragePort`].
pub type BoxedProxyStorage = Box<dyn ProxyStoragePort>;

// ─────────────────────────────────────────────────────────────────────────────
// URL validation helper
// ─────────────────────────────────────────────────────────────────────────────

/// Validate a proxy URL: scheme must be recognised, host must be non-empty,
/// and the explicit port (if present) must be in [1, 65535].
///
/// Hard-error vendor quirks (e.g. `Crawlera` 8011 + `https://`) reject
/// the URL outright. Warning-severity quirks (e.g. `Bright Data`
/// session-id format) are surfaced via `tracing::warn!` and the URL
/// is accepted. See [`crate::vendor_quirks`] for the full quirk
/// table.
fn validate_proxy_url(url: &str) -> ProxyResult<()> {
    use crate::error::ProxyError;
    use crate::vendor_quirks::{self, ParseError, QuirkSeverity};

    // ── T100: structural validation via the canonical `ProxyUrl` parser ───
    //
    // The `ProxyUrl::parse` function is the single source of truth for
    // scheme/host/port/user-info structure. We re-emit the same error
    // surface as before (ProxyError::InvalidProxyUrl) so the public
    // behaviour and the existing `invalid_url_rejected` /
    // `invalid_url_empty_host` tests remain stable.
    let parsed = vendor_quirks::ProxyUrl::parse(url).map_err(|e| match e {
        ParseError::MissingSchemeSeparator(_) => ProxyError::InvalidProxyUrl {
            url: url.to_owned(),
            reason: "missing scheme separator '://'".into(),
        },
        ParseError::UnsupportedScheme(ref s, _) => ProxyError::InvalidProxyUrl {
            url: url.to_owned(),
            reason: format!("unsupported scheme '{s}'"),
        },
        ParseError::EmptyHost(_) => ProxyError::InvalidProxyUrl {
            url: url.to_owned(),
            reason: "empty host".into(),
        },
        ParseError::NonNumericPort(ref p, _) => ProxyError::InvalidProxyUrl {
            url: url.to_owned(),
            reason: format!("non-numeric port '{p}'"),
        },
        ParseError::PortOutOfRange { ref port, .. } => ProxyError::InvalidProxyUrl {
            url: url.to_owned(),
            reason: format!("port {port} is out of range [1, 65535]"),
        },
        ParseError::UnclosedIpv6Bracket(_) => ProxyError::InvalidProxyUrl {
            url: url.to_owned(),
            reason: "unclosed IPv6 bracket in host".into(),
        },
    })?;

    // ── T100: vendor quirk check ───────────────────────────────────────────
    //
    // `vendor_quirks::check` is the canonical entry point for
    // provider-specific rules. We:
    //
    // 1. Reject URLs that match an `Error`-severity quirk (e.g. the
    //    Crawlera 8011 + `https://` WRONG_VERSION_NUMBER trap).
    // 2. Log Warning-severity quirks via `tracing::warn!` — the URL is
    //    accepted, but operators see the message in the ingest log.
    // 3. Record Info-severity quirks via `tracing::info!` (no reject).
    //
    // The match is on `host:port` only; the password component of the
    // URL is never inspected, logged, or echoed.
    let quirks = vendor_quirks::check(&parsed);
    for m in &quirks {
        match m.severity {
            QuirkSeverity::Error => {
                return Err(ProxyError::InvalidProxyUrl {
                    url: url.to_owned(),
                    reason: m.description.to_owned(),
                });
            }
            QuirkSeverity::Warning => {
                tracing::warn!(
                    proxy_url_host = %parsed.host,
                    proxy_url_port = ?parsed.port,
                    quirk_host_suffix = m.host_suffix,
                    observed_scheme = m.observed_scheme.as_str(),
                    required_scheme = m.required_scheme.as_str(),
                    quirk_description = m.description,
                    "proxy URL matches a vendor quirk warning (URL accepted)"
                );
            }
            QuirkSeverity::Info => {
                tracing::info!(
                    proxy_url_host = %parsed.host,
                    proxy_url_port = ?parsed.port,
                    quirk_host_suffix = m.host_suffix,
                    quirk_description = m.description,
                    "proxy URL matches a vendor quirk info record"
                );
            }
        }
    }

    Ok(())
}

/// Validate the optional geo-metadata fields on
/// [`crate::types::ProxyCapabilities`].
///
/// Runs [`validate_asn`](crate::types::validate_asn),
/// [`validate_city`](crate::types::validate_city), and
/// [`validate_postal_code`](crate::types::validate_postal_code) on the
/// populated fields and returns the first failure encountered, or
/// `Ok(())` when every populated field passes. `None` fields are
/// always accepted (the existing "no enrichment" default).
///
/// Called from the storage adapter's `add` path so free-list fetchers
/// and operator-supplied `add_proxy_with_metadata` calls reject
/// malformed values before the record reaches the pool.
fn validate_geo_metadata(caps: &crate::types::ProxyCapabilities) -> ProxyResult<()> {
    use crate::types::{validate_asn, validate_city, validate_postal_code};

    if let Some(asn) = caps.asn {
        validate_asn(asn)?;
    }
    if let Some(ref city) = caps.city {
        validate_city(city)?;
    }
    if let Some(ref postal) = caps.postal_code {
        validate_postal_code(postal)?;
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// MemoryProxyStore
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;
use tokio::sync::RwLock;

use crate::types::ProxyMetrics;
use std::sync::Arc;

type StoreMap = HashMap<Uuid, (ProxyRecord, Arc<ProxyMetrics>)>;

/// In-memory implementation of [`ProxyStoragePort`].
///
/// Uses a `tokio::sync::RwLock`-guarded `HashMap` for thread-safe access.
/// Metrics are updated via atomic operations, so only a **read** lock is
/// needed for [`update_metrics`](MemoryProxyStore::update_metrics) calls —
/// write contention stays low even under heavy concurrent load.
///
/// # Example
/// ```
/// # tokio_test::block_on(async {
/// use stygian_proxy::storage::{MemoryProxyStore, ProxyStoragePort};
/// use stygian_proxy::types::{IpClass, Proxy, ProxyCapabilities, ProxyType, TargetVendorCompatibility};
///
/// let store = MemoryProxyStore::default();
/// let proxy = Proxy { url: "http://proxy.example.com:8080".into(), proxy_type: ProxyType::Http,
///                     username: None, password: None, weight: 1, tags: vec![],
///                     capabilities: ProxyCapabilities::default(),
///                     ip_class: IpClass::Unknown,
///                     target_compatibility: TargetVendorCompatibility::default() };
/// let record = store.add(proxy).await.unwrap();
/// assert_eq!(store.list().await.unwrap().len(), 1);
/// store.remove(record.id).await.unwrap();
/// assert!(store.list().await.unwrap().is_empty());
/// # })
/// ```
#[derive(Debug, Default, Clone)]
pub struct MemoryProxyStore {
    inner: Arc<RwLock<StoreMap>>,
}

impl MemoryProxyStore {
    /// Build a store pre-populated with `proxies`, validating each URL.
    ///
    /// Returns an error on the first invalid URL encountered.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::ProxyError::StorageError`] when any supplied
    /// proxy URL is invalid
    /// or a duplicate of an existing entry.
    pub async fn with_proxies(proxies: Vec<Proxy>) -> ProxyResult<Self> {
        let store = Self::default();
        for proxy in proxies {
            store.add(proxy).await?;
        }
        Ok(store)
    }
}

#[async_trait]
impl ProxyStoragePort for MemoryProxyStore {
    async fn add(&self, proxy: Proxy) -> ProxyResult<ProxyRecord> {
        validate_proxy_url(&proxy.url)?;
        validate_geo_metadata(&proxy.capabilities)?;
        let record = ProxyRecord::new(proxy);
        let metrics = Arc::new(ProxyMetrics::default());
        self.inner
            .write()
            .await
            .insert(record.id, (record.clone(), metrics));
        Ok(record)
    }

    async fn remove(&self, id: Uuid) -> ProxyResult<()> {
        self.inner
            .write()
            .await
            .remove(&id)
            .map(|_| ())
            .ok_or_else(|| crate::error::ProxyError::StorageError(format!("proxy {id} not found")))
    }

    async fn list(&self) -> ProxyResult<Vec<ProxyRecord>> {
        Ok(self
            .inner
            .read()
            .await
            .values()
            .map(|(r, _)| r.clone())
            .collect())
    }

    async fn get(&self, id: Uuid) -> ProxyResult<ProxyRecord> {
        self.inner
            .read()
            .await
            .get(&id)
            .map(|(r, _)| r.clone())
            .ok_or_else(|| crate::error::ProxyError::StorageError(format!("proxy {id} not found")))
    }

    async fn list_with_metrics(&self) -> ProxyResult<Vec<(ProxyRecord, Arc<ProxyMetrics>)>> {
        Ok(self
            .inner
            .read()
            .await
            .values()
            .map(|(r, m)| (r.clone(), Arc::clone(m)))
            .collect())
    }

    async fn update_metrics(&self, id: Uuid, success: bool, latency_ms: u64) -> ProxyResult<()> {
        use std::sync::atomic::Ordering;

        let metrics = self
            .inner
            .read()
            .await
            .get(&id)
            .map(|(_, m)| Arc::clone(m))
            .ok_or_else(|| {
                crate::error::ProxyError::StorageError(format!("proxy {id} not found"))
            })?;

        // Lock released before the atomic updates — no long critical section.
        metrics.requests_total.fetch_add(1, Ordering::Relaxed);
        if success {
            metrics.successes.fetch_add(1, Ordering::Relaxed);
        } else {
            metrics.failures.fetch_add(1, Ordering::Relaxed);
        }
        metrics
            .total_latency_ms
            .fetch_add(latency_ms, Ordering::Relaxed);
        Ok(())
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
)] // serde + storage round-trips and unwraps in test fixtures are deterministic
mod tests {
    use super::*;
    use crate::types::ProxyType;
    use std::sync::atomic::Ordering;

    fn make_proxy(url: &str) -> Proxy {
        Proxy {
            url: url.into(),
            proxy_type: ProxyType::Http,
            username: None,
            password: None,
            weight: 1,
            tags: vec![],
            capabilities: crate::types::ProxyCapabilities::default(),
            ip_class: crate::types::IpClass::Unknown,
            target_compatibility: crate::types::TargetVendorCompatibility::default(),
        }
    }

    #[tokio::test]
    async fn add_list_remove() -> crate::error::ProxyResult<()> {
        let store = MemoryProxyStore::default();
        let r1 = store.add(make_proxy("http://a.test:8080")).await?;
        let r2 = store.add(make_proxy("http://b.test:8080")).await?;
        let r3 = store.add(make_proxy("http://c.test:8080")).await?;
        assert_eq!(store.list().await?.len(), 3);
        store.remove(r2.id).await?;
        let remaining = store.list().await?;
        assert_eq!(remaining.len(), 2);
        let ids: Vec<_> = remaining.iter().map(|r| r.id).collect();
        assert!(ids.contains(&r1.id));
        assert!(ids.contains(&r3.id));
        Ok(())
    }

    #[tokio::test]
    async fn invalid_url_rejected() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryProxyStore::default();
        let err = store
            .add(make_proxy("not-a-url"))
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("invalid URL should be rejected"))?;
        assert!(matches!(
            err,
            crate::error::ProxyError::InvalidProxyUrl { .. }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn invalid_url_empty_host() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryProxyStore::default();
        let err = store
            .add(make_proxy("http://:8080"))
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("empty host URL should be rejected"))?;
        assert!(matches!(
            err,
            crate::error::ProxyError::InvalidProxyUrl { .. }
        ));
        Ok(())
    }

    // ── T98: geo-metadata ingest validation ────────────────────────────────

    /// `asn = 0` must be rejected at ingest time.
    #[tokio::test]
    async fn invalid_geo_metadata_asn_zero_rejected()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryProxyStore::default();
        let mut p = make_proxy("http://cf.test:8080");
        p.capabilities.asn = Some(0);
        let err = store
            .add(p)
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("asn=0 should be rejected"))?;
        assert!(matches!(
            err,
            crate::error::ProxyError::InvalidGeoMetadata { ref field, .. } if field == "asn"
        ));
        Ok(())
    }

    /// `asn = u32::MAX` must be rejected at ingest time.
    #[tokio::test]
    async fn invalid_geo_metadata_asn_max_rejected()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryProxyStore::default();
        let mut p = make_proxy("http://cf.test:8080");
        p.capabilities.asn = Some(u32::MAX);
        let err = store
            .add(p)
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("asn=u32::MAX should be rejected"))?;
        assert!(matches!(
            err,
            crate::error::ProxyError::InvalidGeoMetadata { ref field, .. } if field == "asn"
        ));
        Ok(())
    }

    /// `city = ""` must be rejected at ingest time.
    #[tokio::test]
    async fn invalid_geo_metadata_empty_city_rejected()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryProxyStore::default();
        let mut p = make_proxy("http://cf.test:8080");
        p.capabilities.city = Some(String::new());
        let err = store
            .add(p)
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("empty city should be rejected"))?;
        assert!(matches!(
            err,
            crate::error::ProxyError::InvalidGeoMetadata { ref field, .. } if field == "city"
        ));
        Ok(())
    }

    /// `postal_code = ""` must be rejected at ingest time.
    #[tokio::test]
    async fn invalid_geo_metadata_empty_postal_code_rejected()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryProxyStore::default();
        let mut p = make_proxy("http://cf.test:8080");
        p.capabilities.postal_code = Some(String::new());
        let err = store
            .add(p)
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("empty postal_code should be rejected"))?;
        assert!(matches!(
            err,
            crate::error::ProxyError::InvalidGeoMetadata { ref field, .. } if field == "postal_code"
        ));
        Ok(())
    }

    /// Valid geo metadata is accepted.
    #[tokio::test]
    async fn valid_geo_metadata_accepted() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryProxyStore::default();
        let mut p = make_proxy("http://cf.test:8080");
        p.capabilities.asn = Some(13_335);
        p.capabilities.city = Some("San Francisco".into());
        p.capabilities.postal_code = Some("94110".into());
        let record = store
            .add(p)
            .await
            .map_err(|e| std::io::Error::other(format!("expected accept, got {e}")))?;
        assert_eq!(record.proxy.capabilities.asn, Some(13_335));
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_metrics_updates() -> std::result::Result<(), Box<dyn std::error::Error>> {
        use tokio::task::JoinSet;

        let store = Arc::new(MemoryProxyStore::default());
        let record = store
            .add(make_proxy("http://proxy.test:3128"))
            .await
            .map_err(|e| std::io::Error::other(format!("failed to add proxy: {e}")))?;
        let id = record.id;

        let mut tasks = JoinSet::new();
        for i in 0u64..50 {
            let s = Arc::clone(&store);
            tasks.spawn(async move { s.update_metrics(id, i % 2 == 0, i * 10).await });
        }
        while let Some(res) = tasks.join_next().await {
            let inner = res.map_err(|e| std::io::Error::other(format!("join failed: {e}")))?;
            inner.map_err(|e| std::io::Error::other(format!("update_metrics failed: {e}")))?;
        }

        // Verify totals are internally consistent.
        let guard = store.inner.read().await;
        let metrics = guard
            .get(&id)
            .map(|(_, m)| Arc::clone(m))
            .ok_or_else(|| std::io::Error::other("missing metrics for inserted proxy"))?;
        drop(guard);

        let total = metrics.requests_total.load(Ordering::Relaxed);
        let successes = metrics.successes.load(Ordering::Relaxed);
        let failures = metrics.failures.load(Ordering::Relaxed);
        assert_eq!(total, 50);
        assert_eq!(successes + failures, 50);
        Ok(())
    }

    // ── T100: vendor-quirk ingest validation ────────────────────────────────

    /// The headline `Crawlera` 8011 + `https://` trap must be rejected
    /// at ingest time. The error reason is the static quirk description
    /// (no credentials).
    #[tokio::test]
    async fn validate_crawlera_https_8011_rejected()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryProxyStore::default();
        let err = store
            .add(make_proxy("https://user:secret@proxy.crawlera.com:8011"))
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("Crawlera 8011 + https:// must be rejected"))?;
        match err {
            crate::error::ProxyError::InvalidProxyUrl { url, reason } => {
                assert_eq!(url, "https://user:secret@proxy.crawlera.com:8011");
                // The reason must not echo the password.
                assert!(
                    !reason.contains("secret"),
                    "reason leaked password: {reason}"
                );
                assert!(
                    !reason.contains("user:secret"),
                    "reason leaked credentials: {reason}"
                );
                // The reason must be the quirk description (snippet check).
                assert!(
                    reason.contains("WRONG_VERSION_NUMBER") || reason.contains("plain HTTP"),
                    "reason should reference the quirk, got: {reason}"
                );
            }
            other => panic!("expected InvalidProxyUrl, got {other:?}"),
        }
        Ok(())
    }

    /// The `Crawlera` 8011 + `http://` URL is the compliant form and
    /// must be accepted.
    #[tokio::test]
    async fn validate_crawlera_http_8011_accepted()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryProxyStore::default();
        let record = store
            .add(make_proxy("http://apikey:@proxy.crawlera.com:8011"))
            .await
            .map_err(|e| {
                std::io::Error::other(format!("Crawlera 8011 + http:// must be accepted: {e}"))
            })?;
        assert_eq!(record.proxy.url, "http://apikey:@proxy.crawlera.com:8011");
        Ok(())
    }

    /// The `Zyte` 8011 + `https://` trap must be rejected (same
    /// `WRONG_VERSION_NUMBER` failure mode as `Crawlera`).
    #[tokio::test]
    async fn validate_zyte_https_8011_rejected()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryProxyStore::default();
        let err = store
            .add(make_proxy("https://apikey:@proxy.zyte.com:8011"))
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("Zyte 8011 + https:// must be rejected"))?;
        assert!(matches!(
            err,
            crate::error::ProxyError::InvalidProxyUrl { ref reason, .. }
                if reason.contains("WRONG_VERSION_NUMBER") || reason.contains("plain HTTP")
        ));
        Ok(())
    }

    /// `Bright Data` quirk is a Warning — the URL is accepted and the
    /// store adds the record.
    #[tokio::test]
    async fn validate_bright_data_warning_accepted()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryProxyStore::default();
        let record = store
            .add(make_proxy(
                "http://brd-customer-1-session-abc123@brd.superproxy.io:22225",
            ))
            .await
            .map_err(|e| {
                std::io::Error::other(format!("Bright Data Warning URL must be accepted: {e}"))
            })?;
        assert!(record.proxy.url.contains("brd.superproxy.io"));
        Ok(())
    }

    /// `IPRoyal` quirk is a Warning — the URL is accepted.
    #[tokio::test]
    async fn validate_iproyal_warning_accepted()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryProxyStore::default();
        let record = store
            .add(make_proxy(
                "http://user-country-US:pass@residential.iproyal.com:12321",
            ))
            .await
            .map_err(|e| {
                std::io::Error::other(format!("IPRoyal Warning URL must be accepted: {e}"))
            })?;
        assert!(record.proxy.url.contains("iproyal.com"));
        Ok(())
    }

    /// Unknown hosts produce zero false positives — the URL is
    /// accepted without any quirk warnings.
    #[tokio::test]
    async fn validate_unknown_host_no_quirks_accepted()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryProxyStore::default();
        let record = store
            .add(make_proxy(
                "http://user:pass@some-unrelated-host.example:8080",
            ))
            .await
            .map_err(|e| std::io::Error::other(format!("unrelated host must be accepted: {e}")))?;
        assert!(record.proxy.url.contains("some-unrelated-host.example"));
        Ok(())
    }

    /// The pre-existing structural URL validation must still reject
    /// malformed URLs (e.g. missing scheme separator).
    #[tokio::test]
    async fn validate_preserves_structural_rejection()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryProxyStore::default();
        let err = store
            .add(make_proxy("not-a-url"))
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("not-a-url must be rejected"))?;
        assert!(matches!(
            err,
            crate::error::ProxyError::InvalidProxyUrl { ref reason, .. }
                if reason.contains("missing scheme separator")
        ));
        Ok(())
    }

    /// The pre-existing empty-host rejection must still fire
    /// (regression guard for the T100 refactor).
    #[tokio::test]
    async fn validate_preserves_empty_host_rejection()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryProxyStore::default();
        let err = store
            .add(make_proxy("http://:8080"))
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("empty host must be rejected"))?;
        assert!(matches!(
            err,
            crate::error::ProxyError::InvalidProxyUrl { ref reason, .. }
                if reason.contains("empty host")
        ));
        Ok(())
    }

    /// `Crawlera` on a non-8011 port does NOT trigger the quirk (port
    /// filter is applied before the scheme check).
    #[tokio::test]
    async fn validate_crawlera_non_8011_https_accepted()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryProxyStore::default();
        let record = store
            .add(make_proxy("https://user:pass@proxy.crawlera.com:9000"))
            .await
            .map_err(|e| {
                std::io::Error::other(format!("Crawlera 9000 + https:// must be accepted: {e}"))
            })?;
        assert!(record.proxy.url.contains("crawlera.com:9000"));
        Ok(())
    }
}
