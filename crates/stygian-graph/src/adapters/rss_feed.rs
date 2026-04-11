//! RSS / Atom feed [`ScrapingService`](crate::ports::ScrapingService) adapter
//!
//! Parses RSS 1.0, RSS 2.0, Atom, and JSON Feed formats via the `feed-rs`
//! crate, returning feed items as structured JSON for downstream pipeline nodes.
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::adapters::rss_feed::RssFeedAdapter;
//! use stygian_graph::ports::{ScrapingService, ServiceInput};
//! use serde_json::json;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let adapter = RssFeedAdapter::new(reqwest::Client::new());
//! let input = ServiceInput {
//!     url: "https://example.com/feed.xml".into(),
//!     params: json!({}),
//! };
//! let output = adapter.execute(input).await.unwrap();
//! println!("{}", output.data); // JSON array of feed items
//! # });
//! ```

use crate::domain::error::{Result, ServiceError, StygianError};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};
use async_trait::async_trait;
use feed_rs::parser;
use serde::{Deserialize, Serialize};
use serde_json::json;

// ─── Domain types ─────────────────────────────────────────────────────────────

/// A single feed item extracted from RSS/Atom.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeedItem {
    /// Item title.
    pub title: Option<String>,
    /// Primary link URL.
    pub link: Option<String>,
    /// Published or updated timestamp (ISO 8601).
    pub published: Option<String>,
    /// Item summary / description.
    pub summary: Option<String>,
    /// Category labels.
    pub categories: Vec<String>,
    /// Author names.
    pub authors: Vec<String>,
    /// Unique identifier (guid / id).
    pub id: String,
}

// ─── Adapter ──────────────────────────────────────────────────────────────────

/// RSS / Atom feed source adapter.
///
/// Fetches and parses feeds using the `feed-rs` crate which handles
/// RSS 1.0, RSS 2.0, Atom, and JSON Feed formats transparently.
pub struct RssFeedAdapter {
    client: reqwest::Client,
}

impl RssFeedAdapter {
    /// Create a new RSS feed adapter.
    pub const fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ScrapingService for RssFeedAdapter {
    /// Fetch and parse a feed, returning items as JSON.
    ///
    /// # Params (optional)
    ///
    /// * `since` — ISO 8601 datetime string; exclude items published before this.
    /// * `limit` — integer; maximum number of items to return.
    /// * `categories` — array of strings; only include items matching any of these categories.
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        let resp = self
            .client
            .get(&input.url)
            .header(
                reqwest::header::ACCEPT,
                "application/rss+xml, application/atom+xml, application/xml, text/xml, */*",
            )
            .send()
            .await
            .map_err(|e| {
                StygianError::Service(ServiceError::Unavailable(format!("feed fetch failed: {e}")))
            })?;

        if !resp.status().is_success() {
            return Err(StygianError::Service(ServiceError::InvalidResponse(
                format!("feed returned HTTP {}", resp.status()),
            )));
        }

        let bytes = resp.bytes().await.map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "feed body read failed: {e}"
            )))
        })?;

        let feed = parser::parse(&bytes[..]).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "feed parse failed: {e}"
            )))
        })?;

        // Convert feed entries into our domain type
        let mut items: Vec<FeedItem> = feed
            .entries
            .iter()
            .map(|entry| {
                let title = entry.title.as_ref().map(|t| t.content.clone());
                let link = entry.links.first().map(|l| l.href.clone());
                let published = entry.published.or(entry.updated).map(|dt| dt.to_rfc3339());
                let summary = entry.summary.as_ref().map(|s| s.content.clone());
                let categories = entry.categories.iter().map(|c| c.term.clone()).collect();
                let authors = entry.authors.iter().map(|a| a.name.clone()).collect();
                let id = entry.id.clone();

                FeedItem {
                    title,
                    link,
                    published,
                    summary,
                    categories,
                    authors,
                    id,
                }
            })
            .collect();

        // Apply optional filters
        if let Some(since) = input.params.get("since").and_then(|v| v.as_str()) {
            items.retain(|item| {
                item.published
                    .as_deref()
                    .is_some_and(|pub_date| pub_date >= since)
            });
        }

        if let Some(cats) = input.params.get("categories").and_then(|v| v.as_array()) {
            let filter_cats: Vec<&str> = cats.iter().filter_map(|c| c.as_str()).collect();
            if !filter_cats.is_empty() {
                items.retain(|item| {
                    item.categories
                        .iter()
                        .any(|c| filter_cats.contains(&c.as_str()))
                });
            }
        }

        if let Some(limit) = input
            .params
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
        {
            items.truncate(limit);
        }

        let count = items.len();
        let data = serde_json::to_string(&items).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "feed serialization failed: {e}"
            )))
        })?;

        // Feed-level metadata
        let feed_title = feed.title.map(|t| t.content);
        let feed_description = feed.description.map(|d| d.content);
        let feed_updated = feed.updated.map(|dt| dt.to_rfc3339());

        Ok(ServiceOutput {
            data,
            metadata: json!({
                "source": "rss_feed",
                "feed_title": feed_title,
                "feed_description": feed_description,
                "last_updated": feed_updated,
                "item_count": count,
                "source_url": input.url,
            }),
        })
    }

    fn name(&self) -> &'static str {
        "rss_feed"
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const RSS_FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Example Blog</title>
    <link>https://example.com</link>
    <description>An example RSS feed</description>
    <item>
      <title>First Post</title>
      <link>https://example.com/post/1</link>
      <pubDate>Mon, 01 Mar 2026 00:00:00 +0000</pubDate>
      <description>Summary of first post</description>
      <category>tech</category>
      <guid>post-1</guid>
    </item>
    <item>
      <title>Second Post</title>
      <link>https://example.com/post/2</link>
      <pubDate>Sun, 15 Feb 2026 00:00:00 +0000</pubDate>
      <description>Summary of second post</description>
      <category>science</category>
      <guid>post-2</guid>
    </item>
    <item>
      <title>Third Post</title>
      <link>https://example.com/post/3</link>
      <pubDate>Sat, 01 Feb 2026 00:00:00 +0000</pubDate>
      <description>Summary of third post</description>
      <category>tech</category>
      <category>news</category>
      <guid>post-3</guid>
    </item>
  </channel>
</rss>"#;

    const ATOM_FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Example Atom Feed</title>
  <link href="https://example.com"/>
  <updated>2026-03-01T00:00:00Z</updated>
  <entry>
    <title>Atom Entry One</title>
    <link href="https://example.com/atom/1"/>
    <id>urn:uuid:atom-1</id>
    <updated>2026-03-01T00:00:00Z</updated>
    <summary>First atom entry</summary>
    <author><name>Alice</name></author>
    <category term="rust"/>
  </entry>
  <entry>
    <title>Atom Entry Two</title>
    <link href="https://example.com/atom/2"/>
    <id>urn:uuid:atom-2</id>
    <updated>2026-02-15T00:00:00Z</updated>
    <summary>Second atom entry</summary>
    <author><name>Bob</name></author>
  </entry>
</feed>"#;

    fn parse_test_feed(
        xml: &str,
    ) -> std::result::Result<Vec<FeedItem>, Box<dyn std::error::Error>> {
        let feed = parser::parse(xml.as_bytes())?;
        let items = feed
            .entries
            .iter()
            .map(|entry| {
                let title = entry.title.as_ref().map(|t| t.content.clone());
                let link = entry.links.first().map(|l| l.href.clone());
                let published = entry.published.or(entry.updated).map(|dt| dt.to_rfc3339());
                let summary = entry.summary.as_ref().map(|s| s.content.clone());
                let categories = entry.categories.iter().map(|c| c.term.clone()).collect();
                let authors = entry.authors.iter().map(|a| a.name.clone()).collect();
                let id = entry.id.clone();

                FeedItem {
                    title,
                    link,
                    published,
                    summary,
                    categories,
                    authors,
                    id,
                }
            })
            .collect();
        Ok(items)
    }

    #[test]
    fn parse_rss_with_3_items() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let items = parse_test_feed(RSS_FEED)?;
        assert_eq!(items.len(), 3);
        let first = items
            .first()
            .ok_or_else(|| std::io::Error::other("expected first item"))?;
        assert_eq!(first.title.as_deref(), Some("First Post"));
        assert_eq!(first.link.as_deref(), Some("https://example.com/post/1"));
        assert!(first.published.is_some());
        assert_eq!(first.summary.as_deref(), Some("Summary of first post"));
        Ok(())
    }

    #[test]
    fn parse_atom_with_authors() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let items = parse_test_feed(ATOM_FEED)?;
        assert_eq!(items.len(), 2);
        let first = items
            .first()
            .ok_or_else(|| std::io::Error::other("expected first atom item"))?;
        assert_eq!(first.title.as_deref(), Some("Atom Entry One"));
        assert_eq!(first.authors, vec!["Alice".to_string()]);
        assert_eq!(first.categories, vec!["rust".to_string()]);
        assert_eq!(first.link.as_deref(), Some("https://example.com/atom/1"));
        Ok(())
    }

    #[test]
    fn filter_by_since_date() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut items = parse_test_feed(RSS_FEED)?;
        // Keep only items published in March 2026 or later
        items.retain(|item| {
            item.published
                .as_deref()
                .is_some_and(|pub_date| pub_date >= "2026-03-01")
        });
        assert_eq!(items.len(), 1);
        let first = items
            .first()
            .ok_or_else(|| std::io::Error::other("expected one filtered item"))?;
        assert_eq!(first.title.as_deref(), Some("First Post"));
        Ok(())
    }

    #[test]
    fn filter_by_categories() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut items = parse_test_feed(RSS_FEED)?;
        let filter_cats = ["tech"];
        items.retain(|item| {
            item.categories
                .iter()
                .any(|c| filter_cats.contains(&c.as_str()))
        });
        assert_eq!(items.len(), 2);
        let first = items
            .first()
            .ok_or_else(|| std::io::Error::other("expected first filtered item"))?;
        let second = items
            .get(1)
            .ok_or_else(|| std::io::Error::other("expected second filtered item"))?;
        assert_eq!(first.title.as_deref(), Some("First Post"));
        assert_eq!(second.title.as_deref(), Some("Third Post"));
        Ok(())
    }

    #[test]
    fn empty_feed_returns_empty_array() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Empty Feed</title>
  </channel>
</rss>"#;
        let items = parse_test_feed(xml)?;
        assert!(items.is_empty());
        Ok(())
    }

    #[test]
    fn malformed_feed_returns_error() {
        let bad = b"<not-a-feed><broken";
        let result = parser::parse(&bad[..]);
        assert!(result.is_err());
    }

    #[test]
    fn limit_truncates_items() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut items = parse_test_feed(RSS_FEED)?;
        assert_eq!(items.len(), 3);
        items.truncate(2);
        assert_eq!(items.len(), 2);
        Ok(())
    }

    #[test]
    fn rss_items_have_ids() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let items = parse_test_feed(RSS_FEED)?;
        let first = items
            .first()
            .ok_or_else(|| std::io::Error::other("expected first rss item"))?;
        let second = items
            .get(1)
            .ok_or_else(|| std::io::Error::other("expected second rss item"))?;
        let third = items
            .get(2)
            .ok_or_else(|| std::io::Error::other("expected third rss item"))?;
        assert!(!first.id.is_empty());
        assert!(!second.id.is_empty());
        assert!(!third.id.is_empty());
        Ok(())
    }

    #[test]
    fn atom_feed_has_categories() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let items = parse_test_feed(ATOM_FEED)?;
        let first = items
            .first()
            .ok_or_else(|| std::io::Error::other("expected first atom item"))?;
        let second = items
            .get(1)
            .ok_or_else(|| std::io::Error::other("expected second atom item"))?;
        assert_eq!(first.categories, vec!["rust"]);
        assert!(second.categories.is_empty());
        Ok(())
    }
}
