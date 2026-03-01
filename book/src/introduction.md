# Introduction

**mycelium** is a high-performance web scraping toolkit for Rust, delivered as two complementary
crates in a single workspace.

| Crate | Purpose |
|---|---|
| [`mycelium-graph`](./graph/architecture.md) | Graph-based scraping engine — DAG pipelines, AI extraction, distributed execution |
| [`mycelium-browser`](./browser/overview.md) | Anti-detection browser automation — stealth profiles, browser pooling, CDP automation |

Both crates share a common philosophy: **zero-cost abstractions, extreme composability, and
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

`1.93.1` — Rust 2024 edition. Requires stable toolchain only.

---

## Installation

Add both crates to `Cargo.toml`:

```toml
[dependencies]
mycelium-graph   = "0.1"
mycelium-browser = "0.1"   # optional — only needed for JS-rendered pages
tokio            = { version = "1", features = ["full"] }
serde_json       = "1"
```

Enable optional feature groups on `mycelium-graph`:

```toml
mycelium-graph = { version = "0.1", features = ["browser", "ai-claude", "distributed"] }
```

Available features:

| Feature | Includes |
|---|---|
| `browser` | `BrowserAdapter` backed by `mycelium-browser` |
| `ai-claude` | Anthropic Claude adapter |
| `ai-openai` | OpenAI adapter |
| `ai-gemini` | Google Gemini adapter |
| `ai-copilot` | GitHub Copilot adapter |
| `ai-ollama` | Ollama (local) adapter |
| `distributed` | Redis/Valkey work queue adapter |
| `metrics` | Prometheus metrics export |

---

## Quick start — scraping pipeline

```rust
use mycelium_graph::{Pipeline, adapters::HttpAdapter};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = json!({
        "nodes": [
            {"id": "fetch",   "service": "http"},
            {"id": "extract", "service": "ai_claude"}
        ],
        "edges": [{"from": "fetch", "to": "extract"}]
    });

    let pipeline = Pipeline::from_config(config)?;
    let results  = pipeline.execute(json!({"url": "https://example.com"})).await?;

    println!("{}", serde_json::to_string_pretty(&results)?);
    Ok(())
}
```

## Quick start — browser automation

```rust
use mycelium_browser::{BrowserConfig, BrowserPool, WaitUntil};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool   = BrowserPool::new(BrowserConfig::default()).await?;
    let handle = pool.acquire().await?;

    let mut page = handle.browser().new_page().await?;
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
mycelium/
├── crates/
│   ├── mycelium-graph/     # Scraping engine
│   └── mycelium-browser/   # Browser automation
├── book/                   # This documentation (mdBook)
├── docs/                   # Architecture reference docs
├── examples/               # Example pipeline configs (.toml)
└── .github/workflows/      # CI, release, security, docs
```

Source, issues, and pull requests live at
[github.com/greysquirr3l/mycelium](https://github.com/greysquirr3l/mycelium).

---

## Documentation

| Resource | URL |
|---|---|
| This guide | [greysquirr3l.github.io/mycelium](https://greysquirr3l.github.io/mycelium/) |
| API reference (`mycelium-graph`) | [greysquirr3l.github.io/mycelium/api/mycelium_graph](https://greysquirr3l.github.io/mycelium/api/mycelium_graph/index.html) |
| API reference (`mycelium-browser`) | [greysquirr3l.github.io/mycelium/api/mycelium_browser](https://greysquirr3l.github.io/mycelium/api/mycelium_browser/index.html) |
| crates.io (`mycelium-graph`) | [crates.io/crates/mycelium-graph](https://crates.io/crates/mycelium-graph) |
| crates.io (`mycelium-browser`) | [crates.io/crates/mycelium-browser](https://crates.io/crates/mycelium-browser) |
