//! Unified MCP (Model Context Protocol) aggregator for Stygian.
#![allow(clippy::multiple_crate_versions)]
//!
//! Merges the tool surfaces of `stygian-graph`, `stygian-browser`, and
//! `stygian-proxy` into a single MCP server.  An LLM agent connecting to
//! this server can scrape URLs, run pipeline DAGs, automate browsers,
//! manage proxy pools, and combine all three capabilities — without needing
//! to connect to three separate processes.
//!
//! ## Architecture
//!
//! ```text
//!
//!   LLM / IDE
//!      │  JSON-RPC 2.0 (stdin/stdout)
//!      ▼
//! ┌─────────────────────────────┐
//! │        McpAggregator         │
//! │                             │
//! │  tools/list  ─── merge ─── ┤
//! │  tools/call  ─── route ─┐  │
//! └─────────────────────────┼──┘
//!      ┌──────────────────┬─┘
//!      ▼                  ▼
//!  GraphHandler      BrowserHandler  ProxyHandler
//!  (stygian-graph)   (stygian-       (stygian-
//!   scrape, rest,      browser)       proxy)
//!   graphql, rss,     acquire, nav,   add, acquire,
//!   sitemap,          eval, shot,     release, stats
//!   pipeline_*        verify, release
//! ```
//!
//! Tools are prefixed by crate: `graph_scrape`, `browser_acquire`,
//! `proxy_add`.  The aggregator itself adds two cross-crate tools:
//!
//! | Tool | Description |
//! | ---- | ----------- |
//! | `scrape_proxied` | HTTP scrape routed through the proxy pool |
//! | `browser_proxied` | Browser session with proxy from the pool |

pub mod aggregator;
