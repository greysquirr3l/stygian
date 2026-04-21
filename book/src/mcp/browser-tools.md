# Browser MCP Tools

`stygian-browser` exposes thirteen tools for headless browser automation plus a pool stats tool.

---

## Enabling

```toml
[dependencies]
stygian-browser = { version = "*", features = ["mcp"] }
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

### `browser_attach` *(requires `mcp-attach` feature)*

Attach an MCP session to an existing DevTools websocket endpoint (`cdp_ws`) or
use the extension bridge contract path (`extension_bridge`, currently not implemented).

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `mode` | string | ✓ | `cdp_ws` \| `extension_bridge` |
| `endpoint` | string | | Required for `cdp_ws`; DevTools websocket URL |
| `profile_hint` | string | | Optional label for external profile identity |
| `target_profile` | string | | Optional tuning profile: `default` \| `reddit` |

`cdp_ws` returns a new MCP `session_id` that can be used with the normal
browser lifecycle tools (`browser_navigate`, `browser_eval`, `browser_content`,
`browser_screenshot`, `browser_release`).

**Example (`cdp_ws`):**

```json
{
  "mode": "cdp_ws",
  "endpoint": "ws://127.0.0.1:9222/devtools/browser/abcd1234",
  "target_profile": "default"
}
```

---

### `browser_query`

Query all elements matching a CSS selector and return their text content (and optionally named
attributes) as a structured list. Does not require deserialising the full page HTML.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `session_id` | string | ✓ | Session ID from `browser_acquire` |
| `url` | string | ✓ | URL to navigate to before querying |
| `selector` | string | ✓ | CSS selector — e.g. `"article.post h2"` |
| `fields` | object | | Map of `{ "name": "attr_name" }` pairs — extra attribute values to include per node |

**Returns:**

```json
[
  { "text": "Post title", "href": "https://example.com/post-1" },
  { "text": "Another title", "href": "https://example.com/post-2" }
]
```

When `fields` is omitted, each item contains only `"text"` (the element's `textContent`).

---

### `browser_extract`

Extract structured records from a page using a root selector + per-field schema. Equivalent
to calling `page.extract_all::<T>()` with an inline schema definition.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `session_id` | string | ✓ | Session ID |
| `url` | string | ✓ | URL to navigate to before extracting |
| `root_selector` | string | ✓ | CSS selector for the repeating container element |
| `schema` | object | ✓ | Map of field name to `{ selector, attr?, required? }` field descriptor |

**Schema field descriptor:**

| Key | Type | Required | Description |
| --- | ---- | -------- | ----------- |
| `selector` | string | ✓ | CSS selector scoped to the root element |
| `attr` | string | | If present, captures this attribute instead of `textContent` |
| `required` | boolean | | `true` (default) — omit or set to `false` for optional fields |

**Returns:**

```json
[
  {
    "title":  "Example post",
    "url":    "https://example.com/post-1",
    "author": "Alice",
    "date":   "2025-01-15"
  }
]
```

**Example call:**

```json
{
  "session_id":    "01HV4...",
  "url":           "https://news.example.com",
  "root_selector": "article.story",
  "schema": {
    "title":  { "selector": "h2" },
    "url":    { "selector": "h2 a", "attr": "href" },
    "author": { "selector": "span.author" },
    "date":   { "selector": "time", "attr": "datetime", "required": false }
  }
}
```

---

### `browser_extract_with_fallback`

Structured extraction with multiple root selectors. Selectors are tried in order,
and the first selector that yields results is used.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `session_id` | string | ✓ | Session ID |
| `url` | string | ✓ | URL to navigate to before extracting |
| `root_selectors` | array[string] | ✓ | Candidate root selectors in priority order |
| `schema` | object | ✓ | Same schema object used by `browser_extract` |
| `timeout_secs` | number | | Navigation/extraction timeout (default: 30) |

**Returns:** same structured `results` array as `browser_extract`, plus the selected root selector.

---

### `browser_extract_resilient`

Structured extraction mode that tolerates partial records by skipping invalid root
nodes instead of failing the full extraction call.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `session_id` | string | ✓ | Session ID |
| `url` | string | ✓ | URL to navigate to before extracting |
| `root_selector` | string | ✓ | Root selector for repeated record containers |
| `schema` | object | ✓ | Same schema object used by `browser_extract` |
| `timeout_secs` | number | | Navigation/extraction timeout (default: 30) |

**Returns:**

```json
{
  "url": "https://news.example.com",
  "root_selector": "article.story",
  "count": 42,
  "skipped": 3,
  "results": [
    { "title": "...", "url": "..." }
  ]
}
```

---

### `browser_find_similar`

Find elements that are structurally similar to a reference fingerprint, even when class names or
depth differ across page versions. Uses a weighted Jaccard similarity score (tag 40 %, classes
35 %, attribute names 15 %, depth 10 %).

> **Note:** Requires the `similarity` feature to be enabled on `stygian-browser`.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `session_id` | string | ✓ | Session ID |
| `url` | string | ✓ | URL to navigate to before searching |
| `fingerprint` | object | ✓ | `ElementFingerprint` JSON — capture with `node.fingerprint()` in the Rust API |
| `threshold` | number | | Minimum similarity score 0–1 (default: `0.7`) |
| `max_results` | integer | | Maximum number of matches to return (default: `10`) |

**Returns:**

```json
[
  { "score": 0.92, "outer_html": "<div class=\"post post-featured\">...</div>" },
  { "score": 0.81, "outer_html": "<div class=\"post\">...</div>" }
]
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
  "checks": [
    { "id": "WebDriverFlag", "passed": true, "details": "undefined" },
    { "id": "ChromeObject", "passed": true, "details": "present" },
    { "id": "PluginCount", "passed": true, "details": "5" },
    { "id": "LanguagesPresent", "passed": true, "details": "en-US,en" },
    { "id": "CanvasConsistency", "passed": true, "details": "data:image/png;..." },
    { "id": "WebGlVendor", "passed": true, "details": "Intel Inc. -- ANGLE" },
    { "id": "AutomationGlobals", "passed": true, "details": "none" },
    { "id": "OuterWindowSize", "passed": true, "details": "1920x1080" },
    { "id": "HeadlessUserAgent", "passed": true, "details": "Mozilla/5.0..." },
    { "id": "NotificationPermission", "passed": true, "details": "default" },
    { "id": "MatchMediaPresent", "passed": true, "details": "function" },
    { "id": "ElementFromPointPresent", "passed": true, "details": "function" },
    { "id": "RequestAnimationFramePresent", "passed": true, "details": "function" },
    { "id": "GetComputedStylePresent", "passed": true, "details": "function" },
    { "id": "CssSupportsPresent", "passed": true, "details": "function" },
    { "id": "SendBeaconPresent", "passed": true, "details": "function" },
    { "id": "ExecCommandPresent", "passed": true, "details": "function" },
    { "id": "NodeJsAbsent", "passed": true, "details": "absent" }
  ],
  "passed_count": 18,
  "failed_count": 0,
  "transport": null
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

## Live Attach Test (End-to-End)

Run a local Chrome with remote debugging enabled:

```sh
/Applications/Google\ Chrome.app/Contents/MacOS/Google\ Chrome \
  --remote-debugging-port=9222 \
  --user-data-dir=/tmp/stygian-attach-profile
```

Resolve the websocket endpoint and run the ignored attach integration test:

```sh
export STYGIAN_ATTACH_WS_ENDPOINT=$(curl -s http://127.0.0.1:9222/json/version | jq -r .webSocketDebuggerUrl)
cargo test -p stygian-browser \
  --features "mcp,mcp-attach" \
  --test mcp_integration \
  -- --ignored --nocapture
```

If `STYGIAN_ATTACH_WS_ENDPOINT` is not set, the live attach test is skipped.

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
