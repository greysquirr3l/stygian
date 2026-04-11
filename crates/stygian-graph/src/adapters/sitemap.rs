//! Sitemap / sitemap-index [`ScrapingService`](crate::ports::ScrapingService) adapter
//!
//! Parses XML sitemaps (`<urlset>`) and sitemap index files (`<sitemapindex>`),
//! emitting discovered URLs with metadata for downstream pipeline nodes.
//!
//! Supports:
//! - Standard sitemaps (`<urlset>` with `<url>` entries)
//! - Sitemap index files (`<sitemapindex>` with nested `<sitemap>` refs)
//! - Gzipped sitemaps (`.xml.gz`) via `flate2`
//! - Filtering by `lastmod` date range or `priority` threshold
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::adapters::sitemap::SitemapAdapter;
//! use stygian_graph::ports::{ScrapingService, ServiceInput};
//! use serde_json::json;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let adapter = SitemapAdapter::new(reqwest::Client::new(), 5);
//! let input = ServiceInput {
//!     url: "https://example.com/sitemap.xml".into(),
//!     params: json!({}),
//! };
//! let output = adapter.execute(input).await.unwrap();
//! println!("{}", output.data); // JSON array of discovered URLs
//! # });
//! ```

use crate::domain::error::{Result, ServiceError, StygianError};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};
use async_trait::async_trait;
use flate2::read::GzDecoder;
use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::Read;

// ─── Domain types ─────────────────────────────────────────────────────────────

/// A single URL entry extracted from a sitemap.
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::sitemap::SitemapEntry;
///
/// let entry = SitemapEntry {
///     loc: "https://example.com/page".into(),
///     lastmod: Some("2026-03-01".into()),
///     changefreq: Some("weekly".into()),
///     priority: Some(0.8),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SitemapEntry {
    /// Absolute URL.
    pub loc: String,
    /// Last-modified date string (ISO 8601).
    pub lastmod: Option<String>,
    /// Change frequency hint.
    pub changefreq: Option<String>,
    /// Priority (0.0–1.0).
    pub priority: Option<f64>,
}

// ─── Adapter ──────────────────────────────────────────────────────────────────

/// Sitemap / sitemap-index source adapter.
///
/// Fetches and parses XML sitemaps, recursively resolving sitemap index files
/// up to a configurable depth limit.
///
/// # Example
///
/// ```no_run
/// use stygian_graph::adapters::sitemap::SitemapAdapter;
///
/// let adapter = SitemapAdapter::new(reqwest::Client::new(), 3);
/// ```
pub struct SitemapAdapter {
    client: reqwest::Client,
    max_depth: usize,
}

impl SitemapAdapter {
    /// Create a new sitemap adapter.
    ///
    /// `max_depth` controls how many levels of sitemap-index nesting to follow.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::adapters::sitemap::SitemapAdapter;
    ///
    /// let adapter = SitemapAdapter::new(reqwest::Client::new(), 5);
    /// ```
    pub const fn new(client: reqwest::Client, max_depth: usize) -> Self {
        Self { client, max_depth }
    }

    /// Fetch raw bytes from a URL, transparently decompressing `.xml.gz`.
    ///
    /// # Errors
    ///
    /// Returns [`StygianError::Service`] on HTTP or decompression failure.
    async fn fetch_bytes(&self, url: &str) -> Result<String> {
        let resp = self.client.get(url).send().await.map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "sitemap fetch failed: {e}"
            )))
        })?;

        if !resp.status().is_success() {
            return Err(StygianError::Service(ServiceError::InvalidResponse(
                format!("sitemap returned HTTP {}", resp.status()),
            )));
        }

        let bytes = resp.bytes().await.map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "sitemap body read failed: {e}"
            )))
        })?;

        // Attempt gzip decompression if URL ends in .gz or content looks gzipped
        if url.to_ascii_lowercase().ends_with(".gz") || bytes.starts_with(&[0x1f, 0x8b]) {
            let mut decoder = GzDecoder::new(&bytes[..]);
            let mut xml = String::new();
            decoder.read_to_string(&mut xml).map_err(|e| {
                StygianError::Service(ServiceError::InvalidResponse(format!(
                    "gzip decompression failed: {e}"
                )))
            })?;
            Ok(xml)
        } else {
            String::from_utf8(bytes.to_vec()).map_err(|e| {
                StygianError::Service(ServiceError::InvalidResponse(format!(
                    "sitemap not valid UTF-8: {e}"
                )))
            })
        }
    }

    /// Recursively resolve a sitemap URL, returning all discovered entries.
    ///
    /// # Errors
    ///
    /// Returns [`StygianError::Service`] on fetch, parse, or depth-limit errors.
    async fn resolve(&self, url: &str, depth: usize) -> Result<Vec<SitemapEntry>> {
        if depth > self.max_depth {
            return Err(StygianError::Service(ServiceError::InvalidResponse(
                format!(
                    "sitemap index nesting exceeded max depth ({depth} > {})",
                    self.max_depth
                ),
            )));
        }

        let xml = self.fetch_bytes(url).await?;
        let root_kind = detect_root_element(&xml)?;

        match root_kind {
            RootElement::UrlSet => parse_urlset(&xml),
            RootElement::SitemapIndex => {
                let nested_urls = parse_sitemapindex(&xml)?;
                let mut all = Vec::new();
                for nested_url in &nested_urls {
                    let entries = Box::pin(self.resolve(nested_url, depth + 1)).await?;
                    all.extend(entries);
                }
                Ok(all)
            }
        }
    }
}

#[async_trait]
impl ScrapingService for SitemapAdapter {
    /// Fetch and parse a sitemap, returning discovered URLs as JSON.
    ///
    /// # Params (optional)
    ///
    /// * `min_priority` — f64, filter entries with priority >= this value.
    /// * `lastmod_after` — string, include only entries with lastmod >= this date.
    /// * `lastmod_before` — string, include only entries with lastmod <= this date.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::adapters::sitemap::SitemapAdapter;
    /// # use stygian_graph::ports::{ScrapingService, ServiceInput};
    /// # use serde_json::json;
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// let adapter = SitemapAdapter::new(reqwest::Client::new(), 5);
    /// let input = ServiceInput {
    ///     url: "https://example.com/sitemap.xml".into(),
    ///     params: json!({ "min_priority": 0.5 }),
    /// };
    /// let out = adapter.execute(input).await.unwrap();
    /// # });
    /// ```
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        let mut entries = self.resolve(&input.url, 0).await?;

        // Apply optional filters
        if let Some(min_pri) = input
            .params
            .get("min_priority")
            .and_then(serde_json::Value::as_f64)
        {
            entries.retain(|e| e.priority.unwrap_or(0.0) >= min_pri);
        }
        if let Some(after) = input.params.get("lastmod_after").and_then(|v| v.as_str()) {
            entries.retain(|e| e.lastmod.as_deref().is_some_and(|lm| lm >= after));
        }
        if let Some(before) = input.params.get("lastmod_before").and_then(|v| v.as_str()) {
            entries.retain(|e| e.lastmod.as_deref().is_some_and(|lm| lm <= before));
        }

        let count = entries.len();
        let data = serde_json::to_string(&entries).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "sitemap serialization failed: {e}"
            )))
        })?;

        Ok(ServiceOutput {
            data,
            metadata: json!({
                "source": "sitemap",
                "url_count": count,
                "source_url": input.url,
            }),
        })
    }

    fn name(&self) -> &'static str {
        "sitemap"
    }
}

// ─── XML parsing helpers ──────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum RootElement {
    UrlSet,
    SitemapIndex,
}

/// Detect whether the XML document is a `<urlset>` or `<sitemapindex>`.
fn detect_root_element(xml: &str) -> Result<RootElement> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e) | Event::Empty(ref e)) => {
                let local = e.local_name();
                let name = std::str::from_utf8(local.as_ref()).unwrap_or("");
                return match name {
                    "urlset" => Ok(RootElement::UrlSet),
                    "sitemapindex" => Ok(RootElement::SitemapIndex),
                    _ => Err(StygianError::Service(ServiceError::InvalidResponse(
                        format!("unexpected XML root element: <{name}>"),
                    ))),
                };
            }
            Ok(Event::Eof) => {
                return Err(StygianError::Service(ServiceError::InvalidResponse(
                    "empty or invalid XML document".into(),
                )));
            }
            Err(e) => {
                return Err(StygianError::Service(ServiceError::InvalidResponse(
                    format!("XML parse error: {e}"),
                )));
            }
            _ => {} // skip processing instructions, comments, decl
        }
        buf.clear();
    }
}

/// Parse a `<urlset>` document into a list of [`SitemapEntry`].
fn parse_urlset(xml: &str) -> Result<Vec<SitemapEntry>> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut entries = Vec::new();

    // Current entry being built
    let mut current: Option<SitemapEntryBuilder> = None;
    let mut current_tag: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = local_name(e);
                match name.as_str() {
                    "url" => {
                        current = Some(SitemapEntryBuilder::default());
                    }
                    "loc" | "lastmod" | "changefreq" | "priority" => {
                        current_tag = Some(name);
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref t)) => {
                if let (Some(builder), Some(tag)) = (&mut current, &current_tag) {
                    let text = t.unescape().unwrap_or_default().trim().to_string();
                    if !text.is_empty() {
                        match tag.as_str() {
                            "loc" => builder.loc = Some(text),
                            "lastmod" => builder.lastmod = Some(text),
                            "changefreq" => builder.changefreq = Some(text),
                            "priority" => builder.priority = text.parse().ok(),
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let name = local_name_end(e);
                if name == "url"
                    && let Some(builder) = current.take()
                    && let Some(entry) = builder.build()
                {
                    entries.push(entry);
                }
                if current_tag.as_deref() == Some(&name) {
                    current_tag = None;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(StygianError::Service(ServiceError::InvalidResponse(
                    format!("sitemap XML parse error: {e}"),
                )));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(entries)
}

/// Parse a `<sitemapindex>` document, returning the `<loc>` URLs of nested sitemaps.
fn parse_sitemapindex(xml: &str) -> Result<Vec<String>> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut urls = Vec::new();
    let mut in_sitemap = false;
    let mut in_loc = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = local_name(e);
                match name.as_str() {
                    "sitemap" => in_sitemap = true,
                    "loc" if in_sitemap => in_loc = true,
                    _ => {}
                }
            }
            Ok(Event::Text(ref t)) => {
                if in_loc {
                    let text = t.unescape().unwrap_or_default().trim().to_string();
                    if !text.is_empty() {
                        urls.push(text);
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let name = local_name_end(e);
                match name.as_str() {
                    "sitemap" => {
                        in_sitemap = false;
                        in_loc = false;
                    }
                    "loc" => in_loc = false,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(StygianError::Service(ServiceError::InvalidResponse(
                    format!("sitemapindex XML parse error: {e}"),
                )));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(urls)
}

/// Extract the local name (without namespace prefix) from a start element.
fn local_name(e: &quick_xml::events::BytesStart<'_>) -> String {
    std::str::from_utf8(e.local_name().as_ref())
        .unwrap_or("")
        .to_string()
}

/// Extract the local name from an end element.
fn local_name_end(e: &quick_xml::events::BytesEnd<'_>) -> String {
    std::str::from_utf8(e.local_name().as_ref())
        .unwrap_or("")
        .to_string()
}

// ─── Builder ──────────────────────────────────────────────────────────────────

#[derive(Default)]
struct SitemapEntryBuilder {
    loc: Option<String>,
    lastmod: Option<String>,
    changefreq: Option<String>,
    priority: Option<f64>,
}

impl SitemapEntryBuilder {
    fn build(self) -> Option<SitemapEntry> {
        Some(SitemapEntry {
            loc: self.loc?,
            lastmod: self.lastmod,
            changefreq: self.changefreq,
            priority: self.priority,
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const URLSET_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url>
    <loc>https://example.com/page1</loc>
    <lastmod>2026-03-01</lastmod>
    <changefreq>daily</changefreq>
    <priority>0.8</priority>
  </url>
  <url>
    <loc>https://example.com/page2</loc>
    <lastmod>2026-02-15</lastmod>
    <priority>0.5</priority>
  </url>
  <url>
    <loc>https://example.com/page3</loc>
  </url>
</urlset>"#;

    const SITEMAPINDEX_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <sitemap>
    <loc>https://example.com/sitemap1.xml</loc>
    <lastmod>2026-03-01</lastmod>
  </sitemap>
  <sitemap>
    <loc>https://example.com/sitemap2.xml.gz</loc>
  </sitemap>
</sitemapindex>"#;

    #[test]
    fn parse_urlset_with_3_urls() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let entries = parse_urlset(URLSET_XML)?;
        assert_eq!(entries.len(), 3);

        let first = entries.first().ok_or("missing first entry")?;
        assert_eq!(first.loc, "https://example.com/page1");
        assert_eq!(first.lastmod.as_deref(), Some("2026-03-01"));
        assert_eq!(first.changefreq.as_deref(), Some("daily"));
        assert_eq!(first.priority, Some(0.8));

        let second = entries.get(1).ok_or("missing second entry")?;
        assert_eq!(second.loc, "https://example.com/page2");
        assert_eq!(second.priority, Some(0.5));
        assert!(second.changefreq.is_none());

        let third = entries.get(2).ok_or("missing third entry")?;
        assert_eq!(third.loc, "https://example.com/page3");
        assert!(third.lastmod.is_none());
        assert!(third.priority.is_none());

        Ok(())
    }

    #[test]
    fn parse_sitemapindex_extracts_nested_urls()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let urls = parse_sitemapindex(SITEMAPINDEX_XML)?;
        assert_eq!(urls.len(), 2);
        assert_eq!(
            urls.first().map(String::as_str),
            Some("https://example.com/sitemap1.xml")
        );
        assert_eq!(
            urls.get(1).map(String::as_str),
            Some("https://example.com/sitemap2.xml.gz")
        );
        Ok(())
    }

    #[test]
    fn detect_root_urlset() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = detect_root_element(URLSET_XML)?;
        assert_eq!(root, RootElement::UrlSet);
        Ok(())
    }

    #[test]
    fn detect_root_sitemapindex() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = detect_root_element(SITEMAPINDEX_XML)?;
        assert_eq!(root, RootElement::SitemapIndex);
        Ok(())
    }

    #[test]
    fn filter_by_lastmod_range() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut entries = parse_urlset(URLSET_XML)?;
        // Only entries on or after 2026-03-01
        entries.retain(|e| e.lastmod.as_deref().is_some_and(|lm| lm >= "2026-03-01"));
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries.first().map(|entry| entry.loc.as_str()),
            Some("https://example.com/page1")
        );
        Ok(())
    }

    #[test]
    fn filter_by_priority_threshold() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut entries = parse_urlset(URLSET_XML)?;
        entries.retain(|e| e.priority.unwrap_or(0.0) >= 0.6);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries.first().map(|entry| entry.loc.as_str()),
            Some("https://example.com/page1")
        );
        Ok(())
    }

    #[test]
    fn gzip_decompression() -> std::result::Result<(), Box<dyn std::error::Error>> {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;

        let xml = URLSET_XML;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(xml.as_bytes())?;
        let compressed = encoder.finish()?;

        // Decompress and parse
        let mut decoder = GzDecoder::new(&compressed[..]);
        let mut decompressed = String::new();
        decoder.read_to_string(&mut decompressed)?;

        let entries = parse_urlset(&decompressed)?;
        assert_eq!(entries.len(), 3);
        Ok(())
    }

    #[test]
    fn malformed_xml_returns_error() {
        let bad = "<not-a-sitemap><broken";
        let result = detect_root_element(bad);
        assert!(result.is_err());
    }

    #[test]
    fn empty_xml_returns_error() {
        let result = detect_root_element("");
        assert!(result.is_err());
    }

    #[test]
    fn unexpected_root_element_returns_error() {
        let xml = r#"<?xml version="1.0"?><html><body>nope</body></html>"#;
        let result = detect_root_element(xml);
        assert!(result.is_err());
    }

    #[test]
    fn urlset_with_no_urls_returns_empty() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let xml = r#"<?xml version="1.0"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"></urlset>"#;
        let entries = parse_urlset(xml)?;
        assert!(entries.is_empty());
        Ok(())
    }

    #[test]
    fn url_without_loc_is_skipped() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let xml = r#"<?xml version="1.0"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url>
    <lastmod>2026-01-01</lastmod>
  </url>
  <url>
    <loc>https://example.com/valid</loc>
  </url>
</urlset>"#;
        let entries = parse_urlset(xml)?;
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries.first().map(|entry| entry.loc.as_str()),
            Some("https://example.com/valid")
        );
        Ok(())
    }
}
