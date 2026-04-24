# acquisition-runner-modes.md

Runner-first MCP examples for the browser acquisition ladder.

Tool name: `browser_acquire_and_extract`

Supported modes:

- `fast`
- `resilient`
- `hostile`
- `investigate`

## End-to-end call (`resilient`)

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

## `fast`

```json
{
  "jsonrpc": "2.0",
  "method": "tools/call",
  "params": {
    "name": "browser_acquire_and_extract",
    "arguments": {
      "url": "https://example.com",
      "mode": "fast"
    }
  }
}
```

## `hostile`

```json
{
  "jsonrpc": "2.0",
  "method": "tools/call",
  "params": {
    "name": "browser_acquire_and_extract",
    "arguments": {
      "url": "https://example.com/challenge",
      "mode": "hostile",
      "wait_for_selector": "main",
      "total_timeout_secs": 60
    }
  }
}
```

## `investigate`

```json
{
  "jsonrpc": "2.0",
  "method": "tools/call",
  "params": {
    "name": "browser_acquire_and_extract",
    "arguments": {
      "url": "https://example.com",
      "mode": "investigate",
      "extraction_js": "({ title: document.title, href: location.href })"
    }
  }
}
```

## Migration quick reference

- Old low-level MCP flow: `browser_acquire` -> `browser_navigate` -> `browser_eval`/`browser_extract` -> `browser_release`
- New runner-first flow: one `browser_acquire_and_extract` call with `mode`

Use the old low-level path only when you need custom multi-step interaction control.
