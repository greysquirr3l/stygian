# MCP — Model Context Protocol

All three Stygian crates expose their capabilities over the
[Model Context Protocol (MCP)](https://modelcontextprotocol.io/) with negotiated protocol
version support for `2025-11-25`, `2025-06-18`, and `2024-11-05`, enabling LLM
agents, IDE plug-ins, and automation pipelines to scrape the web, control browsers, and manage
proxy pools without writing any Rust code.

---

## Deployment modes

| Mode | Crate / deployment | When to use |
| ---- | ------------------ | ----------- |
| **Graph MCP** | `stygian-graph` (embed `McpGraphServer` in your binary) | Scraping pipelines and DAG execution only |
| **Browser MCP** | `stygian-browser` (embed `McpBrowserServer` in your binary) | Browser automation only |
| **Proxy MCP** | `stygian-proxy` (embed `McpProxyServer` in your binary) | Proxy pool management only |
| **Aggregator** | `stygian-mcp` (binary) | All capabilities in one server — recommended |

For most LLM agent integrations, run the **aggregator** — it merges all three tool surfaces into
a single stdin/stdout MCP server and adds two cross-crate tools (`scrape_proxied`,
`browser_proxied`) that orchestrate proxies and scraping/browser together.

---

## Quick start — aggregator

```sh
# Build the unified server
cargo build --release -p stygian-mcp

# Run it (MCP clients communicate over stdin/stdout)
./target/release/stygian-mcp
```

### VS Code configuration

Add to `.vscode/mcp.json` (or your client's MCP config file):

```json
{
  "servers": {
    "stygian": {
      "type": "stdio",
      "command": "/path/to/target/release/stygian-mcp",
      "env": {
        "RUST_LOG": "info"
      }
    }
  }
}
```

---

## Protocol

All servers implement **JSON-RPC 2.0 over stdin/stdout**. Newline-delimited requests in,
newline-delimited responses out.

Notifications (messages without an `id`) do not produce responses.

| Method | Description |
| ------ | ----------- |
| `initialize` | Handshake — returns `protocolVersion`, capabilities, and `serverInfo` |
| `tools/list` | Returns the complete list of available tools with JSON Schema for each |
| `tools/call` | Invoke a tool by name with `arguments` |
| `resources/list` | List active sessions / pool state as MCP resources |
| `resources/read` | Read a resource by URI |

---

## Tool namespaces (aggregator)

When using the aggregator, all tools are namespaced:

| Prefix | Sub-server |
| ------ | ---------- |
| `graph_` | [stygian-graph](./graph-tools.md) — e.g. `graph_scrape`, `graph_pipeline_run` |
| `browser_` | [stygian-browser](./browser-tools.md) — e.g. `browser_acquire`, `browser_navigate` |
| `proxy_` | [stygian-proxy](./proxy-tools.md) — e.g. `proxy_add`, `proxy_acquire` |
| *(none)* | Aggregator cross-crate — `scrape_proxied`, `browser_proxied` |

When using a per-crate MCP server standalone, graph tools are *un-prefixed* (e.g. `scrape`
instead of `graph_scrape`).

---

## Cross-crate tools

These tools are only available in the aggregator and orchestrate multiple sub-systems
automatically.

### `scrape_proxied`

Fetch a URL through a proxy automatically selected from the pool.

1. Acquires an available proxy via `proxy_acquire`.
2. Performs an HTTP scrape through that proxy.
3. Releases the proxy, marking it as healthy or failed for circuit-breaker accounting.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `url` | string | ✓ | Target URL |
| `timeout_secs` | integer | | Request timeout (default: 30) |

Returns the scraped content in MCP `content` format.

### `browser_proxied`

Navigate in a headless browser routed through a proxy from the pool.

1. Acquires a proxy via `proxy_acquire`.
2. Acquires a browser session configured to use that proxy.
3. Navigates to the URL and captures navigation metadata + full HTML.
4. Releases the browser session and proxy.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `url` | string | ✓ | Target URL |

Returns `{ "navigation": { ... }, "html": "<html>..." }` as MCP text content.

---

## Logging

The servers write diagnostic output to **stderr** only; stdout is reserved for the JSON-RPC
channel. Control verbosity via the `RUST_LOG` environment variable:

```sh
RUST_LOG=stygian_mcp=debug,stygian_graph=info ./stygian-mcp
```
