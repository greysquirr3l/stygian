# Introduction

**stygian** is a high-performance web scraping toolkit for Rust, delivered as four complementary
crates in a single workspace.

| Crate | Purpose |
| --- | --- |
| [`stygian-graph`](./graph/architecture.md) | Graph-based scraping engine — DAG pipelines, AI extraction, distributed execution |
| [`stygian-browser`](./browser/overview.md) | Anti-detection browser automation — stealth profiles, browser pooling, CDP automation |
| [`stygian-proxy`](./proxy/overview.md) | Proxy pool management — rotation strategies, circuit breakers, sticky sessions |
| [`stygian-mcp`](./mcp/overview.md) | Unified [Model Context Protocol](./mcp/overview.md) server — LLM agent integration |

All crates share a common philosophy: **zero-cost abstractions, extreme composability, and
secure defaults**.

---

## At a glance

### Design goals

- **Hexagonal architecture** — the domain core has zero I/O dependencies; all external
  capabilities are declared as port traits and injected via adapters.
- **DAG execution** — scraping pipelines are directed acyclic graphs. Nodes run concurrently
  within each topological wave, maximising parallelism.
- **AI-first extraction** — Claude, GPT-4o, Gemini, GitHub Copilot, and Ollama are
  first-class adapters. Structured data flows out of raw HTML without writing parsers.
- **Anti-bot resilience** — the browser crate ships stealth scripts that pass Cloudflare,
  DataDome, PerimeterX, and Akamai checks on Advanced stealth level.
- **Fault-tolerant** — circuit breakers, retry policies, and idempotency keys are built
  into the execution path, not bolted on.

### Minimum supported Rust version

`1.94.0` — Rust 2024 edition. Requires stable toolchain only.

---

## Installation

Add crates to `Cargo.toml`:

```toml
[dependencies]
stygian-graph   = "*"
stygian-browser = "*"   # optional — only needed for JS-rendered pages
stygian-proxy   = "*"   # optional — proxy pool management
tokio            = { version = "1", features = ["full"] }
serde_json       = "1"
```

Enable optional feature groups on `stygian-graph`:

```toml
stygian-graph = { version = "*", features = ["browser", "redis", "mcp"] }
```

Available features:

| Feature | Includes |
| --- | --- |
| `browser` | `BrowserAdapter` backed by `stygian-browser` (default) |
| `redis` | Redis/Valkey cache and distributed work queue adapters |
| `object-storage` | S3-compatible object storage adapter |
| `api` | REST API server binary |
| `postgres` | PostgreSQL storage adapter |
| `cloudflare-crawl` | Cloudflare Browser Rendering crawl adapter |
| `escalation` | Default tiered escalation policy adapter |
| `wasm-plugins` | WASM plugin system via wasmtime |
| `mcp` | MCP server — exposes scraping & pipeline tools over JSON-RPC 2.0 |
| `full` | All of the above |

---

## Quick start — scraping pipeline

```rust
use stygian_graph::domain::graph::{Pipeline, Node};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut pipeline = Pipeline::new("my_scraper");
    pipeline.add_node(Node::new(
        "fetch",
        "http",
        json!({"url": "https://example.com"}),
    ));

    pipeline.validate()?;
    println!("Pipeline '{}' has {} nodes", pipeline.name, pipeline.nodes.len());
    Ok(())
}
```

## Quick start — browser automation

```rust
use stygian_browser::{BrowserConfig, BrowserPool, WaitUntil};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool   = BrowserPool::new(BrowserConfig::default()).await?;
    let handle = pool.acquire().await?;

    let mut page = handle.browser().expect("browser is available").new_page().await?;
    page.navigate(
        "https://example.com",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(30),
    ).await?;

    println!("Title: {}", page.title().await?);
    handle.release().await;
    Ok(())
}
```

---

## Repository layout

```
stygian/
├── crates/
│   ├── stygian-graph/     # Scraping engine
│   ├── stygian-browser/   # Browser automation
│   ├── stygian-proxy/     # Proxy pool management
│   └── stygian-mcp/       # Unified MCP aggregator binary
├── book/                   # This documentation (mdBook)
├── docs/                   # Architecture reference docs
├── examples/               # Example pipeline configs (.toml)
└── .github/workflows/      # CI, release, security, docs
```

Source, issues, and pull requests live at
[github.com/greysquirr3l/stygian](https://github.com/greysquirr3l/stygian).

---

## Documentation

| Resource | URL |
| --- | --- |
| This guide | [greysquirr3l.github.io/stygian](https://greysquirr3l.github.io/stygian/) |
| API reference (`stygian-graph`) | [greysquirr3l.github.io/stygian/api/stygian_graph](https://greysquirr3l.github.io/stygian/api/stygian_graph/index.html) |
| API reference (`stygian-browser`) | [greysquirr3l.github.io/stygian/api/stygian_browser](https://greysquirr3l.github.io/stygian/api/stygian_browser/index.html) |
| crates.io (`stygian-graph`) | [crates.io/crates/stygian-graph](https://crates.io/crates/stygian-graph) |
| crates.io (`stygian-browser`) | [crates.io/crates/stygian-browser](https://crates.io/crates/stygian-browser) |
