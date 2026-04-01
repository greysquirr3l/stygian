# stygian-mcp

Unified [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) server aggregating
[`stygian-graph`], [`stygian-browser`], and [`stygian-proxy`] into a single JSON-RPC 2.0
process over stdin/stdout.

An LLM agent connecting to this server can scrape URLs, run pipeline DAGs, automate browsers,
manage proxy pools, and combine all three capabilities — without needing to connect to three
separate processes.

## Usage

Add to `Cargo.toml`:

```toml
[dependencies]
stygian-mcp = { version = "0.6.0" }
```

Or run the bundled binary directly:

```bash
cargo install stygian-mcp
stygian-mcp
```

## MCP Tools

All tools from the three underlying crates are available under their respective prefixes:

| Prefix | Crate | Example tools |
| ------ | ----- | ------------- |
| `graph_*` | `stygian-graph` | `graph_scrape`, `graph_scrape_rest`, `graph_pipeline_run` |
| `browser_*` | `stygian-browser` | `browser_acquire`, `browser_navigate`, `browser_screenshot` |
| `proxy_*` | `stygian-proxy` | `proxy_add`, `proxy_acquire`, `proxy_pool_stats` |

The aggregator also adds two cross-crate tools:

| Tool | Description |
| ---- | ----------- |
| `scrape_proxied` | HTTP scrape routed through an acquired proxy |
| `browser_proxied` | Browser session with a proxy from the pool |

## Architecture

```text
  LLM / IDE
     │  JSON-RPC 2.0 (stdin/stdout)
     ▼
┌─────────────────────────────┐
│        McpAggregator        │
│  tools/list ── merge        │
│  tools/call ── route ──┐   │
└──────────────────────┬─┘   │
     ┌─────────────────┘     │
     ▼          ▼            ▼
GraphHandler  BrowserHandler  ProxyHandler
```

## License

Licensed under either the [MIT License](../../LICENSE) or the
[Commercial License](../../LICENSE-COMMERCIAL.md) at your option.

[`stygian-graph`]: https://crates.io/crates/stygian-graph
[`stygian-browser`]: https://crates.io/crates/stygian-browser
[`stygian-proxy`]: https://crates.io/crates/stygian-proxy
