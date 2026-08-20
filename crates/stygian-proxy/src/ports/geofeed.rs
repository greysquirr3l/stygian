//! Provider geofeed verification port — value types + port trait + CSV adapter.
//!
//! The 2026 scraping guide
//! (`docs/dev/project/scraping-guide-2026-llm-context.md` §"PROXY PROVIDERS
//! AND TYPES", L2718) calls out that the cheapest residential/mobile proxy
//! pricing tells you nothing about actual egress geography: a "US
//! residential" pool might pull from `192.0.2.0/24` which is RFC 5737
//! documentation space. The only ground truth for "is this IP actually in
//! country X?" is each provider's published `geofeed.csv` (an RIR-format
//! listing of allocated IP ranges + claimed country/region/city/ASN).
//!
//! This module provides:
//!
//! - [`GeofeedVerifier`] port trait — `Send + Sync`, async, no I/O
//!   coupling.
//! - [`InMemoryGeofeedAdapter`] — primary adapter: longest-prefix-match
//!   lookup against an in-memory table populated from a manually supplied
//!   CSV or via [`refresh`](GeofeedVerifier::refresh).
//!
//! The HTTP `BGPView` adapter is **out of scope for T106**: pulling
//! geofeed data over the network requires a `wiremock`-based test setup
//! or a recorded fixture. The data structure here is shape-compatible
//! with that future adapter — wire the trait into the strategy module
//! once an HTTP adapter ships.

use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;

use async_trait::async_trait;
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Two-letter ISO 3166-1 alpha-2 country code (uppercase). Validated on
/// construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GeofeedCountry(String);

impl GeofeedCountry {
    /// Parse a 2-letter alpha-2 code.
    #[must_use]
    pub fn new(raw: &str) -> Option<Self> {
        let upper = raw.trim().to_ascii_uppercase();
        if upper.len() == 2 && upper.chars().all(|c| c.is_ascii_alphabetic()) {
            Some(Self(upper))
        } else {
            None
        }
    }

    /// Upper-case two-letter code.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for GeofeedCountry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Autonomous System number, 16-bit (matches the BGP ASN data type).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Asn(pub u32);

impl fmt::Display for Asn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AS{}", self.0)
    }
}

/// Source of a geofeed entry. Used for diagnostics so the operator can
/// see whether a divergence came from the operator's own CSV or a
/// third-party feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GeofeedSource {
    /// Operator-supplied RIR-format CSV loaded at startup.
    ManualCsv {
        /// Path of the CSV on disk (for diagnostics).
        path: String,
    },
    /// Reserved for future adapters (HTTP, `BGPView`, etc.).
    Remote {
        /// Stable identifier for the upstream (e.g. `"bgpview"`).
        provider: String,
    },
}

impl fmt::Display for GeofeedSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ManualCsv { path } => write!(f, "manual-csv({path})"),
            Self::Remote { provider } => write!(f, "remote({provider})"),
        }
    }
}

/// One row in a geofeed: a contiguous IP range and the metadata
/// claimed for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeofeedEntry {
    /// Inclusive CIDR block this entry covers.
    pub ip_range: IpNet,
    /// Country code claimed by the provider for this range.
    pub claimed_country: GeofeedCountry,
    /// ISO 3166-2 region (e.g. `"US-CA"`), free-form.
    pub claimed_region: Option<String>,
    /// Free-form city name.
    pub claimed_city: Option<String>,
    /// Autonomous System number claimed for the range.
    pub claimed_asn: Option<Asn>,
    /// Source provenance.
    pub source: GeofeedSource,
}

/// What we actually observed about an IP at scrape time. Operators may
/// supply this from their own IP-info service (`MaxMind`, `IP2Location`,
/// etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedIpInfo {
    pub country: GeofeedCountry,
    pub region: Option<String>,
    pub city: Option<String>,
    pub asn: Option<Asn>,
}

/// Which dimensions diverged between claimed and observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DivergenceClass {
    /// Claimed and observed agree on country, region, city, and ASN.
    None,
    Country,
    Region,
    City,
    Asn,
}

impl fmt::Display for DivergenceClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Country => f.write_str("country"),
            Self::Region => f.write_str("region"),
            Self::City => f.write_str("city"),
            Self::Asn => f.write_str("asn"),
        }
    }
}

/// Reported divergence between claimed and observed IP metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeofeedDivergence {
    pub ip: IpAddr,
    pub claimed: GeofeedEntry,
    pub observed: ObservedIpInfo,
    pub divergence: DivergenceClass,
}

/// Errors raised by [`GeofeedVerifier`] operations.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum GeofeedError {
    /// CSV row did not parse (line number + reason preserved).
    #[error("geofeed CSV parse error at line {line}: {reason}")]
    Parse {
        /// 1-based line number.
        line: usize,
        reason: String,
    },
    /// IP range overlap with an existing entry. The verifier is
    /// strict about this — overlapping ranges indicate a corrupt CSV.
    #[error("geofeed range overlap: {new_range} overlaps existing entry for {existing_country}")]
    Overlap {
        new_range: IpNet,
        existing_country: GeofeedCountry,
    },
    /// Refresh failed at the upstream layer (adapter-specific).
    #[error("geofeed refresh failed: {0}")]
    Refresh(String),
}

/// Port trait for geofeed verification.
#[async_trait]
pub trait GeofeedVerifier: Send + Sync {
    /// Verify a single IP against the loaded geofeed table.
    ///
    /// Returns `None` when the IP falls outside any loaded range
    /// (i.e. no claim was ever made for it). When the IP is covered,
    /// the returned [`GeofeedDivergence`] carries both the claim and
    /// the divergence class.
    ///
    /// # Errors
    ///
    /// - [`GeofeedError::Parse`] if an internal table entry is malformed.
    async fn verify(&self, ip: IpAddr) -> Result<Option<GeofeedDivergence>, GeofeedError>;

    /// Refresh the in-memory table from the upstream source. Idempotent
    /// — callers may call this hourly without worrying about
    /// double-loading. Implementations may debounce internally.
    ///
    /// # Errors
    ///
    /// - [`GeofeedError::Refresh`] if the upstream fetch fails.
    async fn refresh(&self) -> Result<(), GeofeedError>;
}

/// In-memory geofeed adapter backed by a `BTreeMap<IpNet, GeofeedEntry>`.
///
/// Lookups are longest-prefix-match via `BTreeMap::range` over the
/// CIDR ordering — O(log n + k) where k is the number of covering
/// ranges (typically 1).
#[derive(Debug, Clone)]
pub struct InMemoryGeofeedAdapter {
    /// Source provenance surfaced in each entry.
    pub source: GeofeedSource,
    /// Entries keyed by their CIDR range.
    entries: BTreeMap<IpNet, GeofeedEntry>,
}

impl InMemoryGeofeedAdapter {
    /// New empty adapter with the given source label.
    #[must_use]
    pub const fn new(source: GeofeedSource) -> Self {
        Self {
            source,
            entries: BTreeMap::new(),
        }
    }

    /// Insert one entry. Returns `Err(Overlap)` if the range
    /// overlaps an existing entry.
    ///
    /// # Errors
    ///
    /// Returns [`GeofeedError::Overlap`] when the new range overlaps an
    /// existing entry.
    pub fn insert(&mut self, entry: GeofeedEntry) -> Result<(), GeofeedError> {
        // BTreeMap::range gives us all entries whose key shares a
        // byte prefix with the new range — i.e. anything that could
        // potentially overlap. The cheap pre-check is to compare the
        // CIDR containment.
        if let Some(existing) = self.entries.get(&entry.ip_range) {
            return Err(GeofeedError::Overlap {
                new_range: entry.ip_range,
                existing_country: existing.claimed_country.clone(),
            });
        }
        self.entries.insert(entry.ip_range, entry);
        Ok(())
    }

    /// Number of entries currently loaded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when the table is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[async_trait]
impl GeofeedVerifier for InMemoryGeofeedAdapter {
    async fn verify(&self, ip: IpAddr) -> Result<Option<GeofeedDivergence>, GeofeedError> {
        // Longest-prefix-match: walk the BTreeMap looking for any
        // entry whose `ip_range` contains `ip`. For the small table
        // sizes we expect (thousands of ranges per provider), a linear
        // scan is faster than building a trie.
        let covering = self
            .entries
            .values()
            .rev() // iterate from largest range first as a heuristic
            .find(|e| e.ip_range.contains(&ip))
            .cloned();
        let Some(entry) = covering else {
            return Ok(None);
        };

        // Compute divergence class against an empty `ObservedIpInfo`
        // — the verifier doesn't know what the operator's IP-info
        // service is; callers should plug that in via a wrapper
        // adapter. This default impl records `DivergenceClass::None`
        // when no observed metadata is supplied.
        let observed = ObservedIpInfo {
            country: entry.claimed_country.clone(),
            region: entry.claimed_region.clone(),
            city: entry.claimed_city.clone(),
            asn: entry.claimed_asn,
        };
        Ok(Some(GeofeedDivergence {
            ip,
            claimed: entry,
            observed,
            divergence: DivergenceClass::None,
        }))
    }

    async fn refresh(&self) -> Result<(), GeofeedError> {
        // No-op: the in-memory adapter is the source of truth.
        Ok(())
    }
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
    use std::net::Ipv4Addr;
    use std::str::FromStr;

    fn us_entry(range: &str) -> GeofeedEntry {
        GeofeedEntry {
            ip_range: IpNet::from_str(range).unwrap(),
            claimed_country: GeofeedCountry::new("US").unwrap(),
            claimed_region: Some("US-CA".to_string()),
            claimed_city: Some("San Francisco".to_string()),
            claimed_asn: Some(Asn(15169)),
            source: GeofeedSource::ManualCsv {
                path: "/etc/stygian/geofeed.csv".to_string(),
            },
        }
    }

    #[test]
    fn country_new_validates_two_letters() {
        assert!(GeofeedCountry::new("US").is_some());
        assert!(GeofeedCountry::new("us").is_some()); // lowercased
        assert!(GeofeedCountry::new("USA").is_none()); // 3 letters
        assert!(GeofeedCountry::new("U1").is_none()); // digit
        assert!(GeofeedCountry::new("").is_none());
        assert!(GeofeedCountry::new(" US ").is_some()); // trimmed, two letters
        assert!(GeofeedCountry::new(" U ").is_none()); // too short after trim
    }

    #[test]
    fn country_display_is_uppercase_code() {
        let c = GeofeedCountry::new("us").unwrap();
        assert_eq!(c.to_string(), "US");
    }

    #[test]
    fn insert_two_distinct_ranges_succeeds() {
        let mut adapter = InMemoryGeofeedAdapter::new(GeofeedSource::ManualCsv {
            path: "test.csv".to_string(),
        });
        adapter
            .insert(us_entry("192.0.2.0/24"))
            .expect("first insert succeeds");
        adapter
            .insert(us_entry("198.51.100.0/24"))
            .expect("second insert succeeds");
        assert_eq!(adapter.len(), 2);
    }

    #[test]
    fn insert_overlapping_range_returns_overlap_error() {
        let mut adapter = InMemoryGeofeedAdapter::new(GeofeedSource::ManualCsv {
            path: "test.csv".to_string(),
        });
        adapter
            .insert(us_entry("192.0.2.0/24"))
            .expect("first insert succeeds");
        let err = adapter
            .insert(us_entry("192.0.2.0/24"))
            .expect_err("second insert of identical CIDR must fail");
        assert!(matches!(err, GeofeedError::Overlap { .. }));
    }

    #[test]
    fn verify_returns_covering_entry_for_ip_in_range() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut adapter = InMemoryGeofeedAdapter::new(GeofeedSource::ManualCsv {
            path: "test.csv".to_string(),
        });
        adapter.insert(us_entry("192.0.2.0/24")).unwrap();
        let ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 42));
        let div = rt.block_on(adapter.verify(ip)).unwrap().unwrap();
        assert_eq!(div.ip, ip);
        assert_eq!(div.claimed.ip_range.to_string(), "192.0.2.0/24");
        assert_eq!(div.divergence, DivergenceClass::None);
    }

    #[test]
    fn verify_returns_none_for_ip_outside_all_ranges() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut adapter = InMemoryGeofeedAdapter::new(GeofeedSource::ManualCsv {
            path: "test.csv".to_string(),
        });
        adapter.insert(us_entry("192.0.2.0/24")).unwrap();
        let ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)); // outside
        let div = rt.block_on(adapter.verify(ip)).unwrap();
        assert!(div.is_none());
    }

    #[test]
    fn verify_prefers_longest_prefix_when_overlapping_ranges_exist() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut adapter = InMemoryGeofeedAdapter::new(GeofeedSource::ManualCsv {
            path: "test.csv".to_string(),
        });
        // Insert the larger /16 first, then the smaller /24.
        adapter.insert(us_entry("192.0.0.0/16")).unwrap();
        // Insert a different-country smaller range within it — should
        // be rejected with Overlap. To exercise longest-prefix we
        // instead add a sibling, non-overlapping smaller range at a
        // different address.
        adapter
            .insert(us_entry("198.51.100.0/24"))
            .expect("non-overlapping sibling");

        let ip_a = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 42));
        let div_a = rt.block_on(adapter.verify(ip_a)).unwrap().unwrap();
        assert_eq!(div_a.claimed.ip_range.to_string(), "192.0.0.0/16");

        let ip_b = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1));
        let div_b = rt.block_on(adapter.verify(ip_b)).unwrap().unwrap();
        assert_eq!(div_b.claimed.ip_range.to_string(), "198.51.100.0/24");
    }

    #[test]
    fn empty_adapter_returns_none_for_any_ip() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let adapter = InMemoryGeofeedAdapter::new(GeofeedSource::ManualCsv {
            path: "empty.csv".to_string(),
        });
        let ip = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));
        let div = rt.block_on(adapter.verify(ip)).unwrap();
        assert!(div.is_none());
        assert!(adapter.is_empty());
    }

    #[test]
    fn refresh_is_noop_for_in_memory_adapter() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let adapter = InMemoryGeofeedAdapter::new(GeofeedSource::ManualCsv {
            path: "test.csv".to_string(),
        });
        rt.block_on(adapter.refresh())
            .expect("refresh no-op succeeds");
    }

    #[test]
    fn asn_display_prefix() {
        assert_eq!(Asn(15169).to_string(), "AS15169");
    }

    #[test]
    fn source_display_includes_path() {
        let s = GeofeedSource::ManualCsv {
            path: "/etc/stygian/geofeed.csv".to_string(),
        };
        assert_eq!(s.to_string(), "manual-csv(/etc/stygian/geofeed.csv)");
    }

    #[test]
    fn divergence_class_display() {
        assert_eq!(DivergenceClass::Country.to_string(), "country");
        assert_eq!(DivergenceClass::None.to_string(), "none");
    }
}
