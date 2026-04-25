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

Stygian is a **monorepo** containing five complementary Rust crates for building robust, scalable web scraping systems:

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

MCP tool matrix (aggregator surface):

| Namespace | Representative tools | Purpose |
| --------- | -------------------- | ------- |
| `graph_*` | `graph_scrape`, `graph_scrape_rest`, `graph_scrape_graphql`, `graph_pipeline_validate`, `graph_pipeline_run` | HTTP/API/feed scraping and DAG execution |
| `browser_*` | `browser_acquire`, `browser_acquire_and_extract`, `browser_navigate`, `browser_query`, `browser_extract`, `browser_extract_with_fallback`, `browser_extract_resilient`, `browser_release` | Headless browser automation and structured extraction |
| `proxy_*` | `proxy_add`, `proxy_remove`, `proxy_pool_stats`, `proxy_acquire`, `proxy_acquire_for_domain`, `proxy_acquire_with_capabilities`, `proxy_fetch_freelist`, `proxy_fetch_freeapiproxies`, `proxy_release` | Proxy pool management, capability-aware leasing, and feed bootstrap |
| cross-crate | `scrape_proxied`, `browser_proxied` | End-to-end orchestration across graph/browser/proxy |

### [stygian-extract-derive](crates/stygian-extract-derive)

Proc-macro backend that powers `#[derive(Extract)]` in `stygian-browser`:

- **Declarative extraction** — annotate structs with CSS selectors and attribute targets
- **Internal crate** — do not add directly; enable via `stygian-browser`'s `extract` feature
- **Zero boilerplate** — generates typed DOM-to-struct deserialization at compile time

```toml
stygian-browser = { version = "*", features = ["extract"] }
```

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

    let browser = handle
        .browser()
        .ok_or_else(|| std::io::Error::other("browser handle already released"))?;
    let mut page = browser.new_page().await?;
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
stygian-graph = { version = "*", features = ["browser"] }
stygian-browser = "*"     # optional, for JavaScript rendering
stygian-proxy = "*"       # optional, for proxy pool management
tokio = { version = "1", features = ["full"] }
```

For MCP integration, install the `stygian-mcp` binary with the `extract` feature for full tool coverage:

```bash
# From crates.io
cargo install stygian-mcp --features extract

# Or from source
cargo install --path crates/stygian-mcp --features extract --locked
```

Then wire it into your MCP client. **VS Code** (`.vscode/mcp.json` or `settings.json`):

```json
{
  "mcp": {
    "servers": {
      "stygian": {
        "command": "stygian-mcp",
        "args": [],
        "type": "stdio"
      }
    }
  }
}
```

**Claude Desktop** (`~/Library/Application Support/Claude/claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "stygian": {
      "command": "stygian-mcp",
      "args": []
    }
  }
}
```

> **Note:** Browser tools require Chrome/Chromium. On macOS: `brew install --cask google-chrome`

### Common Feature Combinations

```toml
# Minimal: HTTP scraping only
stygian-graph = "*"

# Full-featured: browser, AI extraction, distributed queue
stygian-graph = { version = "*", features = ["full"] }

# Browser + Proxy integration
stygian-browser = { version = "*", features = ["stealth", "tls-config"] }
stygian-proxy = { version = "*", features = ["browser", "socks"] }
```

### Runner-First Acquisition (Recommended)

For hostile or variable targets, prefer a single `browser_acquire_and_extract` call over manually chaining low-level browser tools.

Mode guide:

| Mode | When to use |
| ---- | ----------- |
| `fast` | Low-friction pages where speed matters most |
| `resilient` | Default for general production scraping with moderate anti-bot pressure |
| `hostile` | High-friction targets needing heavier escalation and retries |
| `investigate` | Diagnostics-first runs to understand which strategy tier succeeds |

End-to-end example:

```json
{
  "jsonrpc": "2.0",
  "method": "tools/call",
  "params": {
    "name": "browser_acquire_and_extract",
    "arguments": {
      "url": "https://example.com/products",
      "mode": "resilient",
      "wait_for_selector": "article.product",
      "extraction_js": "Array.from(document.querySelectorAll('article.product h2')).map(n => n.textContent?.trim()).filter(Boolean)",
      "total_timeout_secs": 45
    }
  }
}
```

Migration note (old path vs runner path):

- Old low-level path: `browser_acquire` -> `browser_navigate` -> `browser_eval`/`browser_extract` -> `browser_release`.
- New runner path: one `browser_acquire_and_extract` call with `mode` and optional `wait_for_selector`/`extraction_js`.
- Keep low-level tools when you need custom multi-step interaction. Use runner-first for deterministic escalation with fewer moving parts.

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
│   ├── stygian-graph/          # Scraping engine
│   ├── stygian-browser/        # Browser automation
│   ├── stygian-proxy/          # Proxy pool management
│   ├── stygian-mcp/            # MCP aggregator server
│   └── stygian-extract-derive/ # Proc-macro for #[derive(Extract)]
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
