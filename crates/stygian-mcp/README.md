# stygian-mcp

Unified [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) server aggregating
[`stygian-graph`], [`stygian-browser`], and [`stygian-proxy`] into a single JSON-RPC 2.0
process over stdin/stdout.

An LLM agent connecting to this server can scrape URLs, run pipeline DAGs, automate browsers,
manage proxy pools, and combine all three capabilities — without needing to connect to three
separate processes.

## Installation

```bash
# Standalone binary
cargo install stygian-mcp

# Or add to your project
cargo add stygian-mcp
```

## Usage

Run the binary directly:

```bash
stygian-mcp
```

This starts a JSON-RPC 2.0 server on stdin/stdout. Connect any MCP-compatible client (VS Code,
Claude, IDE plugins, etc.) to begin calling scraping, browser, and proxy tools.

## Features

| Feature | Description | Default |
| --------- | ------------- | --------- |
| Base | Proxy + browser tools | ✓ |
| `extract` | Enable `browser_extract` and `graph_extract` tools for structured data | — |

Enable extraction (requires `stygian-browser/extract` and `stygian-graph/extract`):

```bash
cargo install stygian-mcp --features extract
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
  LLM / IDE / Chat Interface
     │  JSON-RPC 2.0 (stdin/stdout)
     ▼
┌─────────────────────────────────────┐
│        McpAggregator                │
│  tools/list ── merge & dispatch     │
│  tools/call ── route by prefix ─┐  │
└──────────────────┬──────────────┼──┘
                   │              │
        ┌──────────┼──────────────┘
        ▼          ▼              ▼
   GraphHandler  BrowserHandler  ProxyHandler
```

### Tool Execution Flow

1. Client calls `tools/list` → aggregator merges all three sub-server tool lists
2. Client calls `tools/call` with a tool name + params
3. Aggregator routes:
   - `graph_*` → strips prefix, forwards to graph sub-server
   - `browser_*` → strips prefix, forwards to browser sub-server
   - `proxy_*` → strips prefix, forwards to proxy sub-server
   - `scrape_proxied`, `browser_proxied` → handled by aggregator (cross-crate coordination)

## Examples

### Scrape with Proxy Rotation

```json
{
  "jsonrpc": "2.0",
  "method": "tools/call",
  "params": {
    "name": "scrape_proxied",
    "arguments": {
      "url": "https://example.com",
      "proxy_id": "proxy-1"
    }
  }
}
```

### Browser Screenshot Through Proxy

```json
{
  "jsonrpc": "2.0",
  "method": "tools/call",
  "params": {
    "name": "browser_proxied",
    "arguments": {
      "url": "https://example.com",
      "stealth_level": "advanced",
      "action": "screenshot"
    }
  }
}
```

### Extract Structured Data

With `extract` feature enabled, use `browser_extract`:

```json
{
  "jsonrpc": "2.0",
  "method": "tools/call",
  "params": {
    "name": "browser_extract",
    "arguments": {
      "url": "https://example.com/products",
      "selector": "a.product-link",
      "extract_format": "json",
      "stealth_level": "advanced"
    }
  }
}
```

## License

Licensed under either the [GNU Affero General Public License v3.0](../../LICENSE) (`AGPL-3.0-only`)
or the [Commercial License](../../LICENSE-COMMERCIAL.md) at your option.

[`stygian-graph`]: https://crates.io/crates/stygian-graph
[`stygian-browser`]: https://crates.io/crates/stygian-browser
[`stygian-proxy`]: https://crates.io/crates/stygian-proxy
