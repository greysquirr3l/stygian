# Graph MCP Tools

`stygian-graph` exposes seven tools for HTTP scraping, API querying, and pipeline execution.

---

## Enabling

```toml
[dependencies]
stygian-graph = { version = "*", features = ["mcp"] }
```

To use as a standalone MCP server (without the aggregator), embed `McpGraphServer` in
your own binary:

```rust,no_run
use stygian_graph::mcp::McpGraphServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    McpGraphServer::new().run().await
}
```

When using the [aggregator](./aggregator.md), all tools are prefixed with `graph_`
(e.g. `graph_scrape` instead of `scrape`).

---

## Tools

### `scrape`

Fetch a URL with anti-bot User-Agent rotation and automatic retries. Returns raw HTML or JSON
content with response metadata.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `url` | string | ✓ | Target URL |
| `timeout_secs` | integer | | Request timeout in seconds (default: 30) |
| `proxy_url` | string | | HTTP/SOCKS5 proxy URL — e.g. `socks5://user:pass@host:1080` |
| `rotate_ua` | boolean | | Rotate the User-Agent header on each request (default: `true`) |

**Returns:**

```json
{
  "data": "<html>...</html>",
  "metadata": { "status": 200, "url": "https://...", "content_type": "text/html" }
}
```

---

### `scrape_rest`

Call a REST/JSON API endpoint. Supports all common HTTP methods, authentication schemes,
query parameters, arbitrary request bodies, pagination, and dot-path response extraction.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `url` | string | ✓ | API endpoint URL |
| `method` | string | | HTTP method: `GET`, `POST`, `PUT`, `PATCH`, `DELETE` (default: `GET`) |
| `auth` | object | | Authentication config (see below) |
| `query` | object | | URL query parameters as key-value pairs |
| `body` | object | | JSON request body |
| `headers` | object | | Custom request headers |
| `pagination` | object | | Pagination config (see below) |
| `data_path` | string | | Dot-separated path to extract from response — e.g. `data.items` |

**`auth` object:**

| Field | Values | Description |
| ----- | ------ | ----------- |
| `type` | `bearer` \| `api_key` \| `basic` \| `header` | Auth scheme |
| `token` | string | Token or credential value |
| `header` | string | Custom header name (when `type = "header"`) |

**`pagination` object:**

| Field | Values | Description |
| ----- | ------ | ----------- |
| `strategy` | `link_header` \| `offset` \| `cursor` | Pagination style |
| `max_pages` | integer | Maximum pages to fetch (default: 1) |

**Example — GitHub issues list:**

```json
{
  "url": "https://api.github.com/repos/owner/repo/issues",
  "auth": { "type": "bearer", "token": "ghp_..." },
  "query": { "state": "open", "per_page": "100" },
  "data_path": ""
}
```

---

### `scrape_graphql`

Execute a GraphQL query or mutation against any spec-compliant endpoint.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `url` | string | ✓ | GraphQL endpoint URL |
| `query` | string | ✓ | GraphQL query or mutation string |
| `variables` | object | | Query variables (JSON object) |
| `auth` | object | | Auth config (see below) |
| `data_path` | string | | Dot-separated path to extract — e.g. `data.countries` |
| `timeout_secs` | integer | | Request timeout in seconds (default: 30) |

**`auth` object:**

| Field | Values | Description |
| ----- | ------ | ----------- |
| `kind` | `bearer` \| `api_key` \| `header` \| `none` | Auth scheme |
| `token` | string | Auth token or key |
| `header_name` | string | Custom header name (default: `X-Api-Key`) |

**Example — countries query:**

```json
{
  "url": "https://countries.trevorblades.com/graphql",
  "query": "{ countries { name capital currency } }",
  "data_path": "data.countries"
}
```

---

### `scrape_sitemap`

Parse a `sitemap.xml` or sitemap index and return all discovered URLs with their priorities and
change frequencies.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `url` | string | ✓ | Sitemap URL (`sitemap.xml` or sitemap index) |
| `max_depth` | integer | | Maximum sitemap index recursion depth (default: 5) |

**Returns:** A JSON array of URL entries:

```json
{
  "data": [
    { "url": "https://example.com/page", "priority": 0.8, "changefreq": "weekly" }
  ],
  "metadata": { "total_urls": 1234, "source": "https://example.com/sitemap.xml" }
}
```

---

### `scrape_rss`

Parse an RSS 2.0 or Atom feed and return all items as structured JSON.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `url` | string | ✓ | RSS/Atom feed URL |

**Returns:** A JSON array of feed items:

```json
{
  "data": [
    {
      "title": "Article title",
      "link": "https://...",
      "published": "2025-03-01T12:00:00Z",
      "description": "..."
    }
  ],
  "metadata": { "feed_title": "My Blog", "total_items": 20 }
}
```

---

### `pipeline_validate`

Parse and validate a TOML pipeline definition without executing it. Returns the parsed node and
service lists, detected cycles, and computed topological execution order.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `toml` | string | ✓ | TOML pipeline definition string |

**Returns on success:**

```json
{
  "valid": true,
  "services": ["http_default", "graphql_api"],
  "nodes": ["fetch_homepage", "extract_links"],
  "execution_order": ["fetch_homepage", "extract_links"]
}
```

**Returns on failure:**

```json
{
  "valid": false,
  "error": "Cycle detected: node_a → node_b → node_a"
}
```

---

### `pipeline_run`

Parse, validate, and execute a TOML pipeline DAG.

- Nodes of kind `http`, `rest`, `graphql`, `sitemap`, and `rss` are executed directly.
- Nodes of kind `ai` are recorded in the `skipped` list.
- Nodes of kind `browser` are executed only when the node includes an opt-in `acquisition` block
  **and** `stygian-graph` is built with the `acquisition-runner` feature. If either condition is
  missing — no `acquisition` block, or the feature is not enabled — the node is skipped.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `toml` | string | ✓ | TOML pipeline definition string |
| `timeout_secs` | integer | | Per-node timeout in seconds (default: 30) |

**Returns:**

```json
{
  "outputs": {
    "fetch_homepage": { "data": "<html>...", "metadata": { "status": 200 } }
  },
  "skipped": ["ai_extract"],
  "errors": {}
}
```

Browser acquisition opt-in example:

```toml
[[services]]
name = "browser_service"
kind = "browser"

[[nodes]]
name = "render"
service = "browser_service"
url = "https://example.com"

[nodes.params.acquisition]
enabled = true
mode = "resilient"
wait_for_selector = "main"
total_timeout_secs = 45
```

If the `acquisition` block is omitted, browser nodes remain non-breaking and are added to
`skipped` as before.

**Example pipeline TOML:**

```toml
[[services]]
name  = "http_default"
kind  = "http"

[[nodes]]
name    = "fetch_homepage"
service = "http_default"
url     = "https://example.com"

[[nodes]]
name       = "fetch_about"
service    = "http_default"
url        = "https://example.com/about"
depends_on = ["fetch_homepage"]
```
