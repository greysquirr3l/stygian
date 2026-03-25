# Browser MCP Tools

`stygian-browser` exposes eight tools for headless browser automation plus a pool stats tool.

---

## Enabling

```toml
[dependencies]
stygian-browser = { version = "0.5.0", features = ["mcp"] }
```

To run as a standalone MCP server:

```sh
cargo run --example mcp_server -p stygian-browser --features mcp
```

When using the [aggregator](./aggregator.md), tools keep their `browser_` prefix.

---

## Session lifecycle

Browser sessions are identified by a `session_id` (ULID string). The lifecycle is:

```
browser_acquire → browser_navigate → browser_eval / browser_screenshot / browser_content → browser_release
```

Always call `browser_release` when done; unreleased sessions hold a browser process open
against the pool's `max` limit.

---

## Tools

### `browser_acquire`

Acquire a browser session from the warm pool. Returns within ~100 ms for warm pools, ~2 s for
cold launch.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `stealth_level` | string | | `none` \| `basic` \| `advanced` (default: `advanced`) |
| `tls_profile` | string | | TLS fingerprint profile name — e.g. `chrome131`, `firefox133`, `safari18`, `edge131` |
| `webrtc_policy` | string | | `allow_all` \| `disable_non_proxied` \| `block_all` (default from pool config) |
| `cdp_fix_mode` | string | | CDP leak mitigation mode: `addBinding` \| `isolatedWorld` \| `enableDisable` \| `none` |
| `proxy` | string | | Proxy URL for this session — e.g. `http://user:pass@proxy:8080` |

**Returns:**

```json
{
  "session_id": "01HV4...",
  "requested_metadata": {
    "stealth_level": "advanced",
    "tls_profile": "chrome131"
  }
}
```

---

### `browser_navigate`

Navigate the browser to a URL and wait for the page to load.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `session_id` | string | ✓ | Session ID from `browser_acquire` |
| `url` | string | ✓ | Target URL |
| `timeout_secs` | integer | | Navigation timeout in seconds (default: 30) |

**Returns:**

```json
{
  "title": "Example Domain",
  "url": "https://example.com"
}
```

---

### `browser_eval`

Evaluate arbitrary JavaScript in the page context and return the result.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `session_id` | string | ✓ | Session ID |
| `script` | string | ✓ | JavaScript expression to evaluate |

**Returns:**

```json
{ "result": 42 }
```

**Example — extracting all links:**

```json
{
  "script": "Array.from(document.querySelectorAll('a')).map(a => a.href)"
}
```

---

### `browser_screenshot`

Capture a full-page screenshot as a base64-encoded PNG.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `session_id` | string | ✓ | Session ID |

**Returns:**

```json
{ "data": "iVBORw0KGgoAAAANSUhEUgAA..." }
```

The returned `data` field is a standard base64 PNG suitable for embedding in an `<img>` tag or
writing directly to a `.png` file.

---

### `browser_content`

Retrieve the current page's full outer HTML.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `session_id` | string | ✓ | Session ID |

**Returns:**

```json
{ "html": "<!DOCTYPE html><html>...</html>" }
```

---

### `browser_verify_stealth`

Run a full stealth diagnostic: navigate to a detection-test URL and return a structured report
of all signals checked (WebDriver flag, CDP artefacts, navigator properties, WebRTC leaks, TLS
fingerprint, etc.).

> **Note:** Requires the `stealth` feature to be enabled on `stygian-browser`.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `session_id` | string | ✓ | Session ID |
| `url` | string | ✓ | URL to navigate to before running diagnostics (e.g. `https://bot.sannysoft.com`) |
| `timeout_secs` | integer | | Navigation timeout (default: 15) |

**Returns:** A `DiagnosticReport` JSON object:

```json
{
  "webdriver_present":    false,
  "cdp_runtime_enabled":  false,
  "navigator_webdriver":  false,
  "webrtc_leak":          false,
  "tls_fingerprint":      "chrome_120",
  "canvas_noise_active":  true,
  "requested_stealth_level": "advanced"
}
```

---

### `browser_release`

Release a browser session back to the pool.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `session_id` | string | ✓ | Session ID to release |

**Returns:**

```json
{ "released": true }
```

---

### `pool_stats`

Return current browser pool statistics. No parameters required.

**Returns:**

```json
{
  "active":    2,
  "max":       8,
  "available": 6
}
```

---

## Resources

The browser MCP exposes active sessions as MCP resources, readable via `resources/read`.

| URI pattern | Description |
| ----------- | ----------- |
| `browser://session/{session_id}` | State of a specific browser session |

**Example `resources/read` request:**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "resources/read",
  "params": { "uri": "browser://session/01HV4..." }
}
```

**Returns:**

```json
{
  "uri": "browser://session/01HV4...",
  "mimeType": "application/json",
  "text": "{ \"session_id\": \"01HV4...\", \"config\": { \"stealth_level\": \"advanced\", \"proxy\": null }, \"pool_active\": 1, \"pool_max\": 8 }"
}
```
