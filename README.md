# stygian

![stygian](assets/img/stygian-logo.png)

**High-performance web scraping toolkit for Rust** — graph-based execution engine + anti-detection browser automation.

[![CI](https://github.com/greysquirr3l/stygian/actions/workflows/ci.yml/badge.svg)](https://github.com/greysquirr3l/stygian/actions/workflows/ci.yml)
[![Security Audit](https://github.com/greysquirr3l/stygian/actions/workflows/security.yml/badge.svg)](https://github.com/greysquirr3l/stygian/actions/workflows/security.yml)
[![Documentation](https://github.com/greysquirr3l/stygian/actions/workflows/docs.yml/badge.svg)](https://github.com/greysquirr3l/stygian/actions/workflows/docs.yml)
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/greysquirr3l/stygian/badge)](https://securityscorecards.dev/viewer/?uri=github.com/greysquirr3l/stygian)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)

---

## What is stygian?

Stygian is a **monorepo** containing two complementary Rust crates for building robust, scalable web scraping systems:

### 📊 [stygian-graph](crates/stygian-graph)

Graph-based scraping engine treating pipelines as DAGs with pluggable service modules:

- **Hexagonal architecture** — domain core isolated from infrastructure
- **Extreme concurrency** — Tokio for I/O, Rayon for CPU-bound tasks
- **AI extraction** — Claude, GPT, Gemini, GitHub Copilot, Ollama support
- **Multi-modal** — images, PDFs, videos via LLM vision APIs
- **Distributed execution** — Redis/Valkey-backed work queues
- **Circuit breaker** — graceful degradation when services fail
- **Idempotency** — safe retries with deduplication keys

### 🌐 [stygian-browser](crates/stygian-browser)

Anti-detection browser automation library for bypassing modern bot protection:

- **Browser pooling** — warm pool, sub-100ms acquisition
- **CDP-based** — Chrome DevTools Protocol via chromiumoxide
- **Stealth features** — navigator spoofing, canvas noise, WebGL randomization
- **Human behavior** — Bézier mouse paths, realistic typing
- **Cloudflare/DataDome/PerimeterX** — bypass detection layers

---

## Quick Start

### Graph Scraping Pipeline

```rust
use stygian_graph::{PipelineBuilder, adapters::HttpAdapter};
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

```rust
use stygian_browser::{BrowserConfig, BrowserPool};
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
stygian-graph = "0.1"
stygian-browser = "0.1"  # optional, for JavaScript rendering
tokio = { version = "1", features = ["full"] }
```

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
│   └── stygian-browser/    # Browser automation
├── examples/                # Example pipelines
├── docs/                    # Architecture docs
└── assets/                  # Diagrams, images
```

---

## Development

### Setup

```bash
# Install Rust 1.93.1+
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

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

---

## Acknowledgments

Built with:

- [chromiumoxide](https://github.com/mattsse/chromiumoxide) — CDP client
- [petgraph](https://github.com/petgraph/petgraph) — graph algorithms
- [tokio](https://tokio.rs/) — async runtime
- [reqwest](https://github.com/seanmonstar/reqwest) — HTTP client

---

**Status**: Active development | Version 0.1.1 | Rust 2024 edition | 842 tests | Linux + macOS

For detailed documentation, see [docs.rs/stygian-graph](https://docs.rs/stygian-graph) and [docs.rs/stygian-browser](https://docs.rs/stygian-browser).
