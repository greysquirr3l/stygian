# stygian

![stygian](assets/img/stygian-logo.png)

**High-performance web scraping toolkit for Rust** — graph-based execution engine + anti-detection browser automation.

[![CI](https://github.com/greysquirr3l/stygian/actions/workflows/ci.yml/badge.svg)](https://github.com/greysquirr3l/stygian/actions/workflows/ci.yml)
[![Security Audit](https://github.com/greysquirr3l/stygian/actions/workflows/security.yml/badge.svg)](https://github.com/greysquirr3l/stygian/actions/workflows/security.yml)
[![Documentation](https://github.com/greysquirr3l/stygian/actions/workflows/docs.yml/badge.svg)](https://github.com/greysquirr3l/stygian/actions/workflows/docs.yml)
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/greysquirr3l/stygian/badge)](https://securityscorecards.dev/viewer/?uri=github.com/greysquirr3l/stygian)
[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](LICENSE)
[![License: Commercial](https://img.shields.io/badge/License-Commercial-green.svg)](LICENSE-COMMERCIAL.md)

---

## What is stygian?

Stygian is a **monorepo** containing four complementary Rust crates for building robust, scalable web scraping systems:

### [stygian-graph](crates/stygian-graph)

Graph-based scraping engine treating pipelines as DAGs with pluggable service modules:

- **Hexagonal architecture** — domain core isolated from infrastructure
- **Extreme concurrency** — Tokio for I/O, Rayon for CPU-bound tasks
- **AI extraction** — Claude, GPT, Gemini, GitHub Copilot, Ollama support
- **Multi-modal** — images, PDFs, videos via LLM vision APIs
- **Distributed execution** — Redis/Valkey-backed work queues
- **Circuit breaker** — graceful degradation when services fail
- **Idempotency** — safe retries with deduplication keys
- **Graph introspection** — runtime inspection, impact analysis, execution waves

### [stygian-browser](crates/stygian-browser)

Anti-detection browser automation library for bypassing modern bot protection:

- **Browser pooling** — warm pool, sub-100ms acquisition
- **CDP-based** — Chrome DevTools Protocol via chromiumoxide
- **Stealth features** — navigator spoofing, canvas noise, WebGL randomization
- **Human behavior** — Bézier mouse paths, realistic typing
- **TLS fingerprinting** — profile-matched JA3/JA4 signatures
- **Cloudflare/DataDome/PerimeterX** — bypass detection layers

### [stygian-proxy](crates/stygian-proxy)

Proxy pool management with intelligent rotation:

- **Multi-protocol** — HTTP, HTTPS, SOCKS5 support
- **Health checking** — automatic dead proxy removal
- **Sticky sessions** — domain-bound proxy affinity
- **Weighted selection** — prioritize faster/more reliable proxies

### [stygian-mcp](crates/stygian-mcp)

MCP (Model Context Protocol) aggregator for LLM tool integration:

- **Unified interface** — single JSON-RPC 2.0 server over stdin/stdout
- **Tool namespacing** — `graph_*`, `browser_*`, `proxy_*` prefixes
- **Cross-crate tools** — `scrape_proxied`, `browser_proxied`
- **VS Code/Claude** — direct integration with MCP-compatible clients

---

## Quick Start

### Graph Scraping Pipeline

```rust,ignore
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pipeline = PipelineBuilder::new()
        .node("fetch", HttpAdapter::new())
        .node("parse", MyParserAdapter)
        .edge("fetch", "parse")
        .build()?;

    let results = pipeline
        .execute(json!({"url": "https://example.com"}))
        .await?;
    
    println!("Results: {:?}", results);
    Ok(())
}
```

### Browser Automation

```rust,ignore
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = BrowserPool::new(BrowserConfig::default()).await?;
    let handle = pool.acquire().await?;
    
    let mut page = handle.browser().new_page().await?;
    page.navigate(
        "https://example.com",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(30),
    ).await?;
    
    let html = page.content().await?;
    println!("Page loaded: {} bytes", html.len());
    
    handle.release().await;
    Ok(())
}
```

---

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
stygian-graph = "*"
stygian-browser = "*"  # optional, for JavaScript rendering
stygian-proxy = "*"    # optional, for proxy pool management
tokio = { version = "1", features = ["full"] }
```

For MCP integration, use the `stygian-mcp` binary directly.

---

## Architecture

### stygian-graph: Hexagonal (Ports & Adapters)

```
Domain Layer (business logic)
    ↑
Ports (trait definitions)
    ↑
Adapters (HTTP, browser, AI providers, storage)
```

- **Zero I/O dependencies** in domain layer
- **Dependency inversion** — adapters depend on ports, not vice versa
- **Extreme testability** — mock any external system

### stygian-browser: Modular

- Self-contained modules with clear interfaces
- Pool management with resource limits
- Graceful degradation on browser unavailability

---

## Project Structure

```
stygian/
├── crates/
│   ├── stygian-graph/      # Scraping engine
│   ├── stygian-browser/    # Browser automation
│   ├── stygian-proxy/      # Proxy pool management
│   └── stygian-mcp/        # MCP aggregator server
├── examples/                # Example pipelines
├── book/                    # mdBook documentation
├── docs/                    # Architecture docs
└── assets/                  # Diagrams, images
```

---

## Development

### Setup

```bash
# Install Rust 1.94.0+
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Build workspace
cargo build --workspace

# Run tests
cargo test --workspace

# Run clippy
cargo clippy --workspace -- -D warnings
```

### Testing

```bash
# Unit tests
cargo test --lib

# Integration tests
cargo test --test '*'

# All tests (browser integration tests require Chrome)
cargo test --all-features

# Measure coverage (requires cargo-tarpaulin)
cargo tarpaulin --workspace --all-features --ignore-tests --out Lcov
```

`stygian-graph` achieves strong unit coverage across domain, ports, and adapter layers.
`stygian-browser` coverage is structurally bounded by the Chrome CDP requirement — all tests
that spin up a real browser are marked `#[ignore = "requires Chrome"]`; pure-logic tests are
fully covered.

---

## Contributing

Contributions welcome! Please:

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'feat: add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

### Commit Convention

Use [Conventional Commits](https://www.conventionalcommits.org/):

- `feat:` — new feature
- `fix:` — bug fix
- `refactor:` — code restructuring
- `test:` — test additions/changes
- `docs:` — documentation updates

---

## License

Dual-licensed under:

- **[GNU Affero General Public License v3.0](LICENSE)** (`AGPL-3.0-only`) — free for open-source use
- **[Commercial License](LICENSE-COMMERCIAL.md)** — available for proprietary/closed-source use

Under the AGPL, any modifications or derivative works must also be released under the AGPL-3.0, including when the software is used to provide a network service. For commercial licensing options that permit proprietary use, see [LICENSE-COMMERCIAL.md](LICENSE-COMMERCIAL.md).

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you shall be dual-licensed as above, without any additional terms or conditions.

---

## Acknowledgments

Built with:

- [chromiumoxide](https://github.com/mattsse/chromiumoxide) — CDP client
- [petgraph](https://github.com/petgraph/petgraph) — graph algorithms
- [tokio](https://tokio.rs/) — async runtime
- [reqwest](https://github.com/seanmonstar/reqwest) — HTTP client

---

**Status**: Active development | Rust 2024 edition | Linux + macOS

For detailed documentation, see the [project docs site](https://greysquirr3l.github.io/stygian).
