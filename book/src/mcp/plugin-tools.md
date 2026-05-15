# Plugin MCP Tools

`stygian-plugin` exposes recorded-template extraction tools for interactive fallback scraping.
Use this surface when direct graph/browser extraction is insufficient and you need a reusable
selector-and-transformation template.

The bundled browser extension is intentionally a basic reference implementation for recording
and applying templates. Treat it as a starting point that demonstrates the MCP integration
surface, and extend it for your product UX, auth model, and persistence requirements.

---

## Enabling

```toml
[dependencies]
stygian-plugin = "*"
```

Embed as an MCP server in your binary using `McpPluginServer`, or consume these tools through
the [aggregator](./aggregator.md), where names are prefixed with `plugin_`.

---

## Workflow

Typical lifecycle:

```
plugin_create_template → plugin_add_region (repeat) → plugin_apply_template
```

Optional operations:

```
plugin_inspect_selector, plugin_get_template, plugin_list_templates,
plugin_extract_batch, plugin_delete_template
```

---

## Tools

### `plugin_create_template`

Create a new extraction template and return its UUID.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `name` | string | ✓ | Template name, e.g. `Product Listing` |
| `description` | string | | Optional human-readable description |
| `tags` | string[] | | Optional labels for grouping templates |

**Returns:** `template_id`, `name`, `created_at`

---

### `plugin_add_region`

Add a named extraction region to an existing template.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `template_id` | string | ✓ | Template UUID |
| `region_name` | string | ✓ | Region key, e.g. `title`, `price` |
| `selector_css` | string | | CSS selector |
| `selector_xpath` | string | | `XPath` selector fallback/alternative |
| `transformations` | string[] | | Ordered transforms, e.g. `Trim`, `Lowercase`, `Regex:...` |

Provide at least one selector field (`selector_css` or `selector_xpath`).

---

### `plugin_apply_template`

Apply a template to HTML content and return extracted region values.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `template_id` | string | ✓ | Template UUID |
| `html` | string | ✓ | HTML input |
| `url` | string | ✓ | Source URL for context/metadata |

---

### `plugin_extract_batch`

Run template extraction over repeated containers (for lists/cards/tables).

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `template_id` | string | ✓ | Template UUID |
| `html` | string | ✓ | HTML input |
| `url` | string | ✓ | Source URL |
| `root_selector` | string | ✓ | CSS selector for item containers |

---

### `plugin_inspect_selector`

Validate selector behavior against HTML before saving regions.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `html` | string | ✓ | HTML to test |
| `selector_css` | string | | CSS selector candidate |
| `selector_xpath` | string | | `XPath` selector candidate |

Returns whether the selector matched and a small preview.

---

### `plugin_get_template`

Get full template configuration by UUID.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `template_id` | string | ✓ | Template UUID |

---

### `plugin_list_templates`

List all saved templates with metadata.

No input parameters.

---

### `plugin_delete_template`

Delete a template permanently.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `template_id` | string | ✓ | Template UUID |

---

## Example request

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "plugin_apply_template",
    "arguments": {
      "template_id": "6f991ec6-3f8d-4ed9-96e2-17ecbcf4a87a",
      "url": "https://example.com/products",
      "html": "<html>...</html>"
    }
  }
}
```

---

## Storage and idempotency

- Template storage uses the configured `PluginTemplateStore` adapter.
- The default aggregator setup stores templates in `./plugin-templates`.
- Idempotency and deduplication are handled by the configured `IdempotencyKeyStore` adapter.

---

## Fallback chain integration

The MCP aggregator wires the plugin extraction adapter as the **last-resort fallback**
in a circuit-breaker-protected chain.  The `scrape_with_plugin_fallback` cross-crate
tool exposes this directly over MCP.

### How the chain works

```
HTTP scrape (primary)
  ├── success → return HTML / data
  └── failure or circuit open
        └── Plugin extraction (fallback)
              ├── success → return structured JSON
              └── failure → propagate last error
```

Each service has its own `CircuitBreakerImpl`.  After `failure_threshold` consecutive
failures the circuit opens and subsequent calls skip that service entirely until the
reset timeout elapses.  The first call after the timeout is a half-open probe — the
circuit returns to `Closed` on success or stays `Open` on failure.

### `scrape_with_plugin_fallback`

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `url` | string | ✓ | Target URL to scrape |
| `template_id` | string | ✓ | UUID of the template to apply if HTTP fails |
| `idempotency_key` | string | | ULID/UUID to deduplicate repeated calls |

**Returns:** extracted data from whichever service succeeded plus a `metadata` object
describing the response (service name, template ID, extraction timestamps, etc.).

### Shared stores

Templates created via `plugin_create_template` are **immediately available** to the
fallback chain because both the `McpPluginServer` and the `PluginExtractionAdapter`
inside the chain share the same `Arc<FileTemplateStore>` instance.  No reload or cache
invalidation is required.

### Programmatic construction

```rust
use std::sync::Arc;
use stygian_graph::adapters::fallback::{
    FallbackChainService, default_fallback_breaker, default_primary_breaker,
};
use stygian_graph::adapters::http::{HttpAdapter, HttpConfig};
use stygian_plugin::adapters::{ExtractionEngine, PluginExtractionAdapter};
use stygian_plugin::storage::{FileTemplateStore, MemoryIdempotencyStore};

let template_store = Arc::new(FileTemplateStore::new("./plugin-templates".into()));
let idempotency_store = Arc::new(MemoryIdempotencyStore::new());

let http_primary = Arc::new(HttpAdapter::with_config(HttpConfig::default()));
let plugin_fallback = Arc::new(PluginExtractionAdapter::new(
    Arc::clone(&template_store),
    Arc::new(ExtractionEngine),
    Arc::clone(&idempotency_store),
));

let chain = Arc::new(
    FallbackChainService::builder()
        .add(http_primary,    default_primary_breaker())
        .add(plugin_fallback, default_fallback_breaker())
        .named("http-to-plugin")
        .build(),
);
```

`default_primary_breaker()` uses a 5-failure threshold and 30-second reset window.
`default_fallback_breaker()` uses a 3-failure threshold and 60-second reset window.

---

## HTTP Transport

`stygian-plugin-mcp` ships with a built-in HTTP server for use with browser
extensions or any HTTP client.  The server speaks JSON-RPC 2.0 over HTTP and
supports CORS (`Access-Control-Allow-Origin: *`), making it safe to call from
Chrome extensions with opaque `chrome-extension://` origins.

### Starting the HTTP server

```sh
# Default: binds 0.0.0.0:3000
stygian-plugin-mcp --transport http

# Custom port
stygian-plugin-mcp --transport http --http-port 8080

# With a templates directory
stygian-plugin-mcp --transport http --templates-dir ~/.stygian/templates
```

The binary must be compiled with the `http` feature (enabled by default in
release builds):

```sh
cargo install stygian-plugin-mcp --features http
```

### Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET`  | `/health` | Liveness probe. Returns `{"status":"ok","service":"stygian-plugin-mcp"}`. |
| `GET`  | `/mcp/tools/list` | List all registered MCP tools. |
| `POST` | `/mcp/tools/call` | Invoke a single tool (bare or full JSON-RPC envelope). |
| `POST` | `/mcp` | Full JSON-RPC 2.0 dispatch — any method. |

### Request formats

**Full JSON-RPC envelope** (preferred):

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "plugin_apply_template",
    "arguments": {
      "template_id": "my-template",
      "html": "<html>...",
      "url": "https://example.com"
    }
  }
}
```

**Bare call** (Chrome extension shorthand, `POST /mcp/tools/call` only):

```json
{
  "name": "plugin_apply_template",
  "arguments": {
    "template_id": "my-template",
    "html": "<html>...",
    "url": "https://example.com"
  }
}
```

Both forms return a standard JSON-RPC 2.0 response:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": { "content": [{ "type": "text", "text": "..." }], "isError": false }
}
```

### Chrome extension integration

The bundled Chrome extension (`crates/stygian-plugin/extension/`) connects to the
HTTP server automatically.  The backend URL defaults to `http://localhost:3000` and
can be changed from the **Settings** tab in the extension popup.

```
Extension popup → Settings → MCP Server URL → http://localhost:3000 → Save
```

The Settings tab also shows a live connection status dot (green/red) that pings
`/health` on demand.

The reference extension now also includes:

- batch extraction mode backed by `plugin_extract_batch`
- root selector validation against the current page
- richer results summaries with JSON and CSV export

For an end-to-end persistence pattern (extension extraction -> MCP routing -> sink/database
ingestion), see [Plugin Persistence Pattern](./plugin-persistence-pattern.md).

### Error responses

JSON-RPC errors follow the spec:

| Code | Meaning |
|------|---------|
| `-32700` | Parse error — request body is not valid JSON |
| `-32600` | Invalid request — missing `jsonrpc` or `method` |
| `-32602` | Invalid params — `name` field missing from `tools/call` |
| `-32601` | Method not found — unknown JSON-RPC method |

Tool-level errors (e.g. unknown template) are returned as `result.isError = true`
content, not as JSON-RPC error objects.  This matches the MCP specification.
