# stygian

![stygian](assets/img/stygian-logo.png)

**High-performance web scraping toolkit for Rust** — graph execution, anti-detection browser automation, and diagnostics-driven acquisition planning.

[![CI](https://github.com/greysquirr3l/stygian/actions/workflows/ci.yml/badge.svg)](https://github.com/greysquirr3l/stygian/actions/workflows/ci.yml)
[![Security Audit](https://github.com/greysquirr3l/stygian/actions/workflows/security.yml/badge.svg)](https://github.com/greysquirr3l/stygian/actions/workflows/security.yml)
[![Documentation](https://github.com/greysquirr3l/stygian/actions/workflows/docs.yml/badge.svg)](https://github.com/greysquirr3l/stygian/actions/workflows/docs.yml)
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/greysquirr3l/stygian/badge)](https://securityscorecards.dev/viewer/?uri=github.com/greysquirr3l/stygian)
[![OpenSSF Best Practices](https://www.bestpractices.dev/projects/9118/badge)](https://www.bestpractices.dev/projects/9118)
[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](LICENSE)
[![License: Commercial](https://img.shields.io/badge/License-Commercial-green.svg)](LICENSE-COMMERCIAL.md)

---

## What is stygian?

Stygian is a monorepo with six complementary Rust crates for building robust, scalable web scraping systems:

- **[stygian-graph](crates/stygian-graph)** — Graph-based scraping engine (DAGs, AI extraction, distributed execution)
- **[stygian-browser](crates/stygian-browser)** — Anti-detection browser automation (stealth features, human behavior)
- **[stygian-proxy](crates/stygian-proxy)** — Proxy pool management (multi-protocol, health checking, sticky sessions)
- **[stygian-charon](crates/stygian-charon)** — Diagnostics & policy planning (HAR forensics, provider classification, SLOs)
- **[stygian-mcp](crates/stygian-mcp)** — MCP aggregator (LLM tool integration via JSON-RPC 2.0)
- **[stygian-plugin](crates/stygian-plugin)** — Visual data extraction with MCP tools and browser extension

---

## Quick Start

### Install

```bash
# From crates.io
cargo install stygian-mcp --features extract

# Or add to Cargo.toml
stygian-graph = { version = "*", features = ["browser"] }
stygian-browser = "*"
```

### Browser Automation

```rust,ignore
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = BrowserPool::new(BrowserConfig::default()).await?;
    let handle = pool.acquire().await?;
    let mut page = handle.browser()?.new_page().await?;

    page.navigate(
        "https://example.com",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(30),
    ).await?;

    println!("Page loaded: {} bytes", page.content().await?.len());
    handle.release().await;
    Ok(())
}
```

### MCP Integration

Wire into Claude or VS Code via JSON-RPC 2.0 for LLM-powered tool access. See [the book](https://greysquirr3l.github.io/stygian) for setup.

---

## Documentation

**→ [Complete docs and guides](https://greysquirr3l.github.io/stygian)**

Key sections:

- [stygian-browser API](https://greysquirr3l.github.io/stygian/browser/overview.html)
- [stygian-graph architecture](https://greysquirr3l.github.io/stygian/graph/overview.html)
- [MCP tool reference](https://greysquirr3l.github.io/stygian/mcp/overview.html)
- [Example pipelines](examples/)

---

## Architecture

Hexagonal design with zero I/O in domain layer:

```
Domain (business logic)
  ↓
Ports (traits)
  ↓
Adapters (HTTP, browser, AI, storage)
```

All crates follow this pattern for maximum testability and modularity.

---

## Development

```bash
# Build & test
cargo test --workspace --all-features

# Strict linting
cargo clippy --workspace -- -D warnings

# Format check
cargo fmt --all -- --check
```

---

## License

Dual-licensed under:

- **[AGPL-3.0](LICENSE)** — open-source use
- **[Commercial License](LICENSE-COMMERCIAL.md)** — proprietary use

See [LICENSE-COMMERCIAL.md](LICENSE-COMMERCIAL.md) for details.

---

## Acknowledgments

Built with [chromiumoxide](https://github.com/mattsse/chromiumoxide), [petgraph](https://github.com/petgraph/petgraph), [tokio](https://tokio.rs/), and [reqwest](https://github.com/seanmonstar/reqwest).

---

**Status**: Active development | Rust 2024 edition | Linux + macOS

For feature requests or issues, see [GitHub Issues](https://github.com/greysquirr3l/stygian/issues).
