# The stygian-mcp Aggregator

`stygian-mcp` is a standalone binary that runs a single MCP server merging all three Stygian
sub-crate tool surfaces into one JSON-RPC 2.0 endpoint. It is the recommended way to integrate
Stygian with LLM agents and IDE plug-ins.

---

## Architecture

```asciidoc
┌──────────────────────────────────────────────────────────────┐
│                       stygian-mcp                            │
│  ┌────────────────────────────────────────────────────────┐  │
│  │                   McpAggregator                        │  │
│  │                                                        │  │
│  │  tools/list  ──► merge all tools + namespace           │  │
│  │  tools/call  ──► route by prefix to sub-server         │  │
│  │                                                        │  │
│  │  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐    │  │
│  │  │ McpGraph     │ │ McpBrowser   │ │ McpProxy     │    │  │
│  │  │ graph_*      │ │ browser_*    │ │ proxy_*      │    │  │
│  │  └──────────────┘ └──────────────┘ └──────────────┘    │  │
│  │                                                        │  │
│  │  Cross-crate tools: scrape_proxied, browser_proxied    │  │
│  └────────────────────────────────────────────────────────┘  │
│                                                              │
│  JSON-RPC 2.0 over stdin/stdout                              │
└──────────────────────────────────────────────────────────────┘
        ▲                                   ▼
   MCP requests                       MCP responses
  (LLM agent /                      (newline-delimited
  IDE plugin)                             JSON)
```

---

## Installation

Build from source:

```sh
cargo build --release -p stygian-mcp
# binary: ./target/release/stygian-mcp
```

---

## Configuration

The aggregator has no configuration file. All runtime behaviour is controlled via environment
variables:

| Variable | Default | Description |
| -------- | ------- | ----------- |
| `RUST_LOG` | `info` | Log verbosity — written to stderr. E.g. `stygian_mcp=debug,stygian_proxy=trace` |

Browser pool and proxy manager are created with their default configurations. To customise
pool size, stealth settings, or proxy strategies, embed the aggregator crate as a library
and call `McpAggregator::try_new()` with custom sub-server instances.

---

## IDE / agent integration

### VS Code (`.vscode/mcp.json`)

```json
{
  "servers": {
    "stygian": {
      "type": "stdio",
      "command": "${workspaceFolder}/target/release/stygian-mcp",
      "env": {
        "RUST_LOG": "info"
      }
    }
  }
}
```

### Claude Desktop (`claude_desktop_config.json`)

```json
{
  "mcpServers": {
    "stygian": {
      "command": "/path/to/stygian-mcp",
      "args": [],
      "env": {
        "RUST_LOG": "warn"
      }
    }
  }
}
```

### Any MCP-compatible client

The server reads newline-delimited JSON from **stdin** and writes newline-delimited JSON to
**stdout**. All diagnostic logging goes to **stderr** and will not corrupt the JSON channel.

---

## Tool routing

The aggregator inspects the `name` field of every `tools/call` request and routes it:

| Name starts with | Routes to | Example |
| ---------------- | --------- | ------- |
| `graph_` | `McpGraphServer` (prefix stripped before dispatch) | `graph_scrape` → `scrape` |
| `browser_` | `McpBrowserServer` | `browser_acquire` |
| `proxy_` | `McpProxyServer` | `proxy_add` |
| `scrape_proxied` | Aggregator (cross-crate) | — |
| `browser_proxied` | Aggregator (cross-crate) | — |

---

## Cross-crate tools

### `scrape_proxied`

HTTP fetch through an automatically acquired proxy from the pool.

```
proxy_acquire → graph.scrape(url, proxy_url) → proxy_release(success)
```

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `url` | string | ✓ | Target URL |
| `timeout_secs` | integer | | Per-request timeout (default: 30) |

> Requires at least one proxy registered via `proxy_add` before calling.

### `browser_proxied`

Full browser navigation through a proxy from the pool.

```
proxy_acquire → browser_acquire(proxy) → browser_navigate → browser_content
             → browser_release → proxy_release(success)
```

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `url` | string | ✓ | Target URL |

Returns combined navigation metadata and full HTML content.

> Requires both a registered proxy **and** a running Chrome/Chromium binary for browser launch.

---

## Resource aggregation

`resources/list` returns resources from both the browser and proxy sub-servers:

| URI prefix | Description |
| ---------- | ----------- |
| `browser://session/{id}` | Active browser session state |
| `proxy://pool/stats` | Live proxy pool statistics |

`resources/read` routes by URI prefix to the correct sub-server.

---

## Embedding as a library

Instead of running the binary, embed the aggregator in your own Rust binary:

```rust,no_run
use stygian_mcp::aggregator::McpAggregator;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let aggregator = McpAggregator::try_new().await?;
    aggregator.run().await
}
```

For custom sub-server configurations, instantiate each server manually and compose them
using the crate's public API.
